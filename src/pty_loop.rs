//! Bucle de E/S del PTY (basado en `alacritty_terminal::event_loop`, Apache-2.0).
//! Diferencia clave: antes de pasar los bytes al parser busca marcas OSC 133 y parte la
//! entrada en ellas, para aplicar cada marca justo en la posición del cursor donde ocurre.

use crate::blocks::{BlockState, Mark, MarkScanner};
use alacritty_terminal::event::{Event, EventListener, OnResize};
use alacritty_terminal::event_loop::Msg;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use alacritty_terminal::tty::{self, EventedPty, EventedReadWrite};
use alacritty_terminal::vte::ansi;
use polling::{Event as PollingEvent, Events, PollMode, Poller};
use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::{self, ErrorKind, Read, Write};
use std::num::NonZeroUsize;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Máximo de bytes leídos y procesados de una vez, con el terminal bloqueado, antes de
/// volver al bucle de eventos.
const READ_BUFFER_SIZE: usize = 64 * 1024;
/// Con salida abundante que llega en migajas (el programa escribe más despacio de lo que se
/// lee), esperar un poco antes de volver a leer junta bloques más grandes. Con `seq 1 3000000`
/// gasta un tercio menos de CPU y tarda lo mismo que la 0.9.1; sin esperar sería un 5 % más
/// rápido pero con casi la misma CPU. El búfer del PTY en el kernel es pequeño: desde ~0,5 ms
/// se llena y frena al programa. Si las lecturas son grandes, el programa va más rápido que
/// nosotros y no se espera. Ver docs/Rendimiento.md.
#[cfg(not(windows))]
const COALESCE: Duration = Duration::from_micros(200);
/// Tamaño mínimo del bloque y tamaño medio de lectura por debajo del cual se considera migaja.
#[cfg(not(windows))]
const COALESCE_MIN_BATCH: usize = 4096;
#[cfg(not(windows))]
const CRUMB: usize = 1024;
/// Windows: un bloque que llega antes de este tiempo tras el anterior es salida en marcha.
#[cfg(windows)]
const STREAM_GAP: Duration = Duration::from_millis(2);
/// Windows: espera para juntar migajas (ver `pty_read`). Con `type` de 3 millones de líneas la
/// CPU de Garbo baja de 6–7 s a ~2 s y tarda menos (12 → 9 s, el límite ya es ConPTY). Con 250
/// µs o 2 ms sale parecido; 1 ms retrasa poco a los programas que redibujan a ráfagas.
#[cfg(windows)]
const COALESCE: Duration = Duration::from_millis(1);
// Mismos valores que los tokens (privados en Unix) de alacritty_terminal::tty.
#[cfg(unix)]
const PTY_READ_WRITE_TOKEN: usize = 0;
#[cfg(unix)]
const PTY_CHILD_EVENT_TOKEN: usize = 1;
// En Windows son públicos (y distintos): la tubería de ConPTY y el aviso de salida del hijo.
#[cfg(windows)]
use tty::{PTY_CHILD_EVENT_TOKEN, PTY_READ_WRITE_TOKEN};

#[derive(Clone)]
pub struct PtySender {
    tx: Sender<Msg>,
    poller: Arc<Poller>,
}

impl PtySender {
    pub fn send(&self, msg: Msg) {
        if self.tx.send(msg).is_ok() {
            let _ = self.poller.notify();
        }
    }
}

pub struct PtyLoop<U: EventListener> {
    poll: Arc<Poller>,
    pty: tty::Pty,
    rx: Receiver<Msg>,
    peeked: Option<Msg>,
    tx: Sender<Msg>,
    terminal: Arc<FairMutex<Term<U>>>,
    event_proxy: U,
    blocks: Arc<Mutex<BlockState>>,
}

/// Resultado de una lectura del PTY.
struct Batch {
    /// La salida llegó en migajas: conviene esperar `COALESCE` antes de volver a leer.
    crumbs: bool,
    /// Quedó salida por leer y no llegará otro aviso: volver a leer sin esperar.
    more: bool,
}

#[derive(Default)]
struct State {
    write_list: VecDeque<Cow<'static, [u8]>>,
    writing: Option<(Cow<'static, [u8]>, usize)>,
    parser: ansi::Processor,
    scanner: MarkScanner,
    marks: Vec<(usize, Mark)>,
    /// Cuándo llegó el último bloque (Windows: para reconocer la salida en migajas).
    #[cfg(windows)]
    last_batch: Option<Instant>,
}

impl<U: EventListener + Send + 'static> PtyLoop<U> {
    pub fn new(
        terminal: Arc<FairMutex<Term<U>>>,
        event_proxy: U,
        pty: tty::Pty,
        blocks: Arc<Mutex<BlockState>>,
    ) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        Ok(Self {
            poll: Poller::new()?.into(),
            pty,
            rx,
            peeked: None,
            tx,
            terminal,
            event_proxy,
            blocks,
        })
    }

    pub fn sender(&self) -> PtySender {
        PtySender {
            tx: self.tx.clone(),
            poller: self.poll.clone(),
        }
    }

    fn has_pending(&mut self) -> bool {
        if self.peeked.is_none() {
            self.peeked = self.rx.try_recv().ok();
        }
        self.peeked.is_some()
    }

    /// Procesa los mensajes de la UI. Devuelve `false` si hay que cerrar.
    fn drain_channel(&mut self, state: &mut State) -> bool {
        while let Some(msg) = self.peeked.take().or_else(|| self.rx.try_recv().ok()) {
            match msg {
                Msg::Input(input) => state.write_list.push_back(input),
                Msg::Resize(ws) => self.pty.on_resize(ws),
                Msg::Shutdown => return false,
            }
        }
        true
    }

    /// Pasa los bytes al parser deteniéndose en cada marca de shell para aplicarla. Los informes
    /// de estado (OSC 7501) van a `BlockState::program` y su consulta se contesta.
    fn feed(
        state: &mut State,
        term: &mut Term<U>,
        bytes: &[u8],
        blocks: &Mutex<BlockState>,
        proxy: &U,
    ) {
        // ConPTY pide al empezar las teclas como eventos de Windows (`win32-input-mode`).
        #[cfg(windows)]
        if let Some(i) = memchr::memmem::find(bytes, b"\x1b[?9001") {
            match bytes.get(i + 7) {
                Some(b'h') => blocks.lock().unwrap().win32_input = true,
                Some(b'l') => blocks.lock().unwrap().win32_input = false,
                _ => {}
            }
        }
        let mut marks = std::mem::take(&mut state.marks);
        marks.clear();
        state.scanner.scan(bytes, &mut marks);
        if marks.is_empty() {
            state.parser.advance(term, bytes);
        } else {
            let mut pos = 0;
            let mut bs = blocks.lock().unwrap();
            for &(end, mark) in &marks {
                state.parser.advance(term, &bytes[pos..end]);
                pos = end;
                if mark == Mark::Cwd {
                    if let Some(path) = state.scanner.cwds.pop_front() {
                        bs.set_cwd(path);
                    }
                } else {
                    bs.apply(mark, term);
                }
            }
            state.parser.advance(term, &bytes[pos..]);
        }
        state.marks = marks;
        if !state.scanner.status.is_empty() {
            let mut bs = blocks.lock().unwrap();
            for (report, st) in state.scanner.status.drain(..) {
                match report {
                    crate::status::Report::Query => {
                        let end = if st { "\x1b\\" } else { "\x07" };
                        proxy.send_event(Event::PtyWrite(format!(
                            "\x1b]7501;{}{end}",
                            crate::status::REPLY
                        )));
                    }
                    other => bs.program.apply(other),
                }
            }
        }
    }

    /// Lee y procesa lo que haya (ver `Batch`).
    fn pty_read(&mut self, state: &mut State, buf: &mut [u8]) -> io::Result<Batch> {
        // Lee hasta vaciar el PTY o llenar el búfer y lo procesa todo de una vez. Con salida
        // abundante el kernel entrega migajas de pocas decenas de bytes y procesarlas una a una
        // costaba el doble de CPU. El terminal solo se bloquea para procesar, no mientras se
        // lee, así que la UI puede dibujar entretanto.
        let (mut len, mut reads) = (0, 0);
        let mut error = None;
        let mut drained = false;
        while len < buf.len() {
            reads += 1;
            match self.pty.reader().read(&mut buf[len..]) {
                Ok(0) => {
                    drained = true;
                    break;
                }
                Ok(got) => len += got,
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    drained = true;
                    break;
                }
                Err(err) => {
                    // Lo ya leído se procesa antes de informar del error (p. ej. EIO al salir).
                    error = Some(err);
                    break;
                }
            }
        }
        if len > 0 {
            let mut terminal = self.terminal.lock();
            Self::feed(
                state,
                &mut terminal,
                &buf[..len],
                &self.blocks,
                &self.event_proxy,
            );
            drop(terminal);
            // Pedir redibujado salvo que todo fuese una actualización sincronizada pendiente.
            if state.parser.sync_bytes_count() < len {
                self.event_proxy.send_event(Event::Wakeup);
            }
        }
        // Windows: ConPTY entrega la salida abundante en trozos de ~75 bytes, unos 31 000 por
        // segundo, uno por lectura: la regla de Linux (lecturas pequeñas dentro de un bloque grande)
        // no salta nunca. Aquí cuenta como migaja un bloque que llega justo tras el anterior.
        #[cfg(windows)]
        let crumbs = {
            let now = Instant::now();
            let streaming = len > 0
                && state
                    .last_batch
                    .is_some_and(|t| now.duration_since(t) < STREAM_GAP);
            if len > 0 {
                state.last_batch = Some(now);
            }
            streaming && len < READ_BUFFER_SIZE / 2
        };
        #[cfg(not(windows))]
        let crumbs = len >= COALESCE_MIN_BATCH && len < reads * CRUMB;
        #[cfg(windows)]
        let _ = reads;
        match error {
            Some(err) => Err(err),
            None => Ok(Batch {
                crumbs,
                // Windows: el lector de alacritty solo vuelve a avisar tras una lectura que
                // encontró la tubería vacía. Si se paró con el búfer lleno, queda salida y hay
                // que seguir leyendo sin esperar aviso (si no, se queda colgado con una escritura
                // grande, p. ej. `[Console]::Write` de 21 MB). En Linux el aviso se repite solo.
                more: cfg!(windows) && !drained,
            }),
        }
    }

    fn pty_write(&mut self, state: &mut State) -> io::Result<()> {
        loop {
            if state.writing.is_none() {
                state.writing = state.write_list.pop_front().map(|b| (b, 0));
            }
            let Some((bytes, written)) = state.writing.as_mut() else {
                return Ok(());
            };
            match self.pty.writer().write(&bytes[*written..]) {
                Ok(0) => return Ok(()),
                Ok(n) => {
                    *written += n;
                    if *written >= bytes.len() {
                        state.writing = None;
                    }
                }
                Err(err) => {
                    return match err.kind() {
                        ErrorKind::Interrupted | ErrorKind::WouldBlock => Ok(()),
                        _ => Err(err),
                    }
                }
            }
        }
    }

    pub fn spawn(mut self) {
        std::thread::Builder::new()
            .name("PTY".into())
            .spawn(move || {
                let mut state = State::default();
                let mut buf = vec![0u8; READ_BUFFER_SIZE];
                let poll_opts = PollMode::Level;
                let mut interest = PollingEvent::readable(0);

                // SAFETY: el PTY se desregistra antes de soltarse, al final del hilo.
                if let Err(err) = unsafe { self.pty.register(&self.poll, interest, poll_opts) } {
                    log::error!("No se pudo registrar el PTY: {err}");
                    return;
                }
                // Dos fuentes propias (PTY y señal del hijo) y dos internas de `polling`
                // (notificador y temporizador), que también ocupan sitio en la lista.
                let mut events = Events::with_capacity(NonZeroUsize::new(4).unwrap());
                // Quedó salida por leer sin aviso pendiente (Windows, ver `Batch::more`).
                let mut more = false;

                'event_loop: loop {
                    // Despertar si vence el plazo de una actualización sincronizada. Con salida
                    // pendiente no se espera: solo se mira si hay algo más que atender.
                    let timeout = if more {
                        Some(Duration::ZERO)
                    } else {
                        state
                            .parser
                            .sync_timeout()
                            .sync_timeout()
                            .map(|t| t.saturating_duration_since(Instant::now()))
                    };

                    events.clear();
                    if let Err(err) = self.poll.wait(&mut events, timeout) {
                        match err.kind() {
                            ErrorKind::Interrupted => continue,
                            _ => {
                                log::error!("Error en el bucle del PTY: {err}");
                                break 'event_loop;
                            }
                        }
                    }

                    if events.is_empty() && !self.has_pending() && !more {
                        state.parser.stop_sync(&mut *self.terminal.lock());
                        self.event_proxy.send_event(Event::Wakeup);
                        continue;
                    }

                    if !self.drain_channel(&mut state) {
                        break;
                    }

                    let (mut readable, mut exited) = (std::mem::take(&mut more), None);
                    for event in events.iter() {
                        match event.key {
                            PTY_CHILD_EVENT_TOKEN => {
                                if let Some(tty::ChildEvent::Exited(status)) =
                                    self.pty.next_child_event()
                                {
                                    exited = Some(status);
                                }
                            }
                            PTY_READ_WRITE_TOKEN => {
                                if event.is_interrupt() {
                                    continue;
                                }
                                readable |= event.readable;
                                if event.writable {
                                    if let Err(err) = self.pty_write(&mut state) {
                                        log::error!("Error escribiendo en el PTY: {err}");
                                        break 'event_loop;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }

                    // Una lectura por vuelta, también si el hijo acaba de salir: así no se pierde
                    // lo último que escribió.
                    let mut coalesce = false;
                    if readable {
                        match self.pty_read(&mut state, &mut buf) {
                            Ok(batch) => (coalesce, more) = (batch.crumbs, batch.more),
                            // Linux: EIO cuando el hijo cuelga; llegará el evento de salida.
                            #[cfg(target_os = "linux")]
                            Err(err) if err.raw_os_error() == Some(libc::EIO) => {}
                            Err(err) => {
                                log::error!("Error leyendo del PTY: {err}");
                                break 'event_loop;
                            }
                        }
                    }
                    if let Some(status) = exited {
                        if let Some(status) = status {
                            self.event_proxy.send_event(Event::ChildExit(status));
                        }
                        self.terminal.lock().exit();
                        self.event_proxy.send_event(Event::Wakeup);
                        break 'event_loop;
                    }

                    // Interés de escritura solo mientras haya algo pendiente.
                    let needs_write = state.writing.is_some() || !state.write_list.is_empty();
                    if needs_write != interest.writable {
                        interest.writable = needs_write;
                        let _ = self.pty.reregister(&self.poll, interest, poll_opts);
                    }
                    if coalesce {
                        std::thread::sleep(COALESCE);
                    }
                }

                let _ = self.pty.deregister(&self.poll);
            })
            .expect("no se pudo crear el hilo del PTY");
    }
}

/// Para las pruebas de otros módulos: procesa `bytes` como si llegasen del PTY.
#[cfg(test)]
pub(crate) fn feed_for_tests<U: EventListener + Send + 'static>(
    term: &mut Term<U>,
    blocks: &Mutex<BlockState>,
    bytes: &[u8],
    proxy: &U,
) {
    PtyLoop::<U>::feed(&mut State::default(), term, bytes, blocks, proxy);
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::test::TermSize;

    /// Avisa cuando el terminal contesta a la consulta de posición del cursor del final.
    #[cfg(unix)]
    #[derive(Clone)]
    struct Done(mpsc::Sender<Instant>);

    #[cfg(unix)]
    impl EventListener for Done {
        fn send_event(&self, event: Event) {
            if let Event::PtyWrite(_) = event {
                let _ = self.0.send(Instant::now());
            }
        }
    }

    #[cfg(unix)]
    fn cpu_time() -> f64 {
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
        let t = |tv: libc::timeval| tv.tv_sec as f64 + tv.tv_usec as f64 / 1e6;
        t(ru.ru_utime) + t(ru.ru_stime)
    }

    /// Banco de medida del camino completo: `seq` real en un PTY, el bucle de lectura y el
    /// terminal, sin ventana. Con «UI» otro hilo bloquea el terminal cada 16 ms y lo recorre,
    /// como haría un fotograma.
    /// `cargo test --release bench_ -- --ignored --nocapture --test-threads=1`
    #[cfg(unix)]
    #[test]
    #[ignore = "banco de medida"]
    fn bench_pty_seq() {
        use alacritty_terminal::event::WindowSize;
        use std::sync::atomic::{AtomicBool, Ordering};
        let cfg = crate::config::Config::default();
        let cmds = [
            ("seq", "seq 1 3000000; printf '\\033[6n'; exec sleep 10"),
            ("líneas largas", "yes \"$(head -c 3000 /dev/zero | tr '\\0' x)\" | head -n 8000; printf '\\033[6n'; exec sleep 10"),
        ];
        for (ui, (cmd_name, cmd)) in [(false, cmds[0]), (true, cmds[0]), (false, cmds[1])] {
            let (mut walls, mut cpus) = (Vec::new(), Vec::new());
            for _ in 0..5 {
                let (tx, rx) = mpsc::channel();
                let term = Arc::new(FairMutex::new(Term::new(
                    crate::terminal::term_config(&cfg),
                    &TermSize::new(180, 50),
                    Done(tx.clone()),
                )));
                let ws = WindowSize {
                    num_lines: 50,
                    num_cols: 180,
                    cell_width: 10,
                    cell_height: 20,
                };
                let opts = tty::Options {
                    shell: Some(tty::Shell::new("sh".into(), vec!["-c".into(), cmd.into()])),
                    working_directory: None,
                    drain_on_exit: false,
                    env: Default::default(),
                };
                let stop = Arc::new(AtomicBool::new(false));
                let frames = ui.then(|| {
                    let (term, stop) = (term.clone(), stop.clone());
                    std::thread::spawn(move || {
                        let mut sum = 0u64;
                        while !stop.load(Ordering::Relaxed) {
                            std::thread::sleep(std::time::Duration::from_millis(16));
                            let t = term.lock();
                            for c in t.renderable_content().display_iter {
                                sum = sum.wrapping_add(c.c as u64);
                            }
                        }
                        sum
                    })
                });
                let cpu0 = cpu_time();
                let t = Instant::now();
                let pty = tty::new(&opts, ws, 0).unwrap();
                let blocks = Arc::new(Mutex::new(BlockState::new(crate::platform::child_pid(
                    &pty,
                ))));
                let lp = PtyLoop::new(term.clone(), Done(tx), pty, blocks).unwrap();
                let sender = lp.sender();
                lp.spawn();
                let end = rx.recv_timeout(std::time::Duration::from_secs(60)).unwrap();
                walls.push(end.duration_since(t).as_secs_f64());
                cpus.push(cpu_time() - cpu0);
                stop.store(true, Ordering::Relaxed);
                if let Some(f) = frames {
                    f.join().unwrap();
                }
                sender.send(Msg::Shutdown);
            }
            walls.sort_by(f64::total_cmp);
            cpus.sort_by(f64::total_cmp);
            eprintln!(
                "pty {cmd_name} {}: mediana {:.3} s (de {:.3} a {:.3}), CPU {:.3} s",
                if ui { "con UI" } else { "sin UI" },
                walls[2],
                walls[0],
                walls[4],
                cpus[2]
            );
        }
    }

    /// Banco de medida del procesado de la salida (escáner de marcas + parser + rejilla).
    /// `cargo test --release bench_ -- --ignored --nocapture --test-threads=1`
    #[test]
    #[ignore = "banco de medida"]
    fn bench_feed() {
        let cfg = crate::config::Config::default();
        let seq: Vec<u8> = (1..=3_000_000u32)
            .flat_map(|n| format!("{n}\r\n").into_bytes())
            .collect();
        let colored: Vec<u8> = (0..400_000u32)
            .flat_map(|n| {
                format!(
                    "\x1b[01;34mcarpeta{n}\x1b[0m  \x1b[38;5;{}march[{n}].rs\x1b[0m\r\n",
                    n % 200
                )
                .into_bytes()
            })
            .collect();
        for (name, data) in [("seq", &seq), ("colores", &colored)] {
            let mut best = f64::MAX;
            for _ in 0..3 {
                let mut term = Term::new(
                    crate::terminal::term_config(&cfg),
                    &TermSize::new(180, 50),
                    VoidListener,
                );
                let blocks = Mutex::new(BlockState::new(0));
                let mut state = State::default();
                let t = Instant::now();
                for chunk in data.chunks(READ_BUFFER_SIZE) {
                    PtyLoop::<VoidListener>::feed(
                        &mut state,
                        &mut term,
                        chunk,
                        &blocks,
                        &VoidListener,
                    );
                }
                best = best.min(t.elapsed().as_secs_f64());
            }
            eprintln!(
                "feed {name:8}: {:.1} MB en {:.0} ms ({:.0} MB/s)",
                data.len() as f64 / 1e6,
                best * 1e3,
                data.len() as f64 / 1e6 / best
            );

            let mut best = f64::MAX;
            for _ in 0..3 {
                let mut scanner = MarkScanner::default();
                let mut marks = Vec::new();
                let t = Instant::now();
                for chunk in data.chunks(READ_BUFFER_SIZE) {
                    marks.clear();
                    scanner.scan(chunk, &mut marks);
                }
                best = best.min(t.elapsed().as_secs_f64());
            }
            eprintln!("  solo el escáner: {:.2} ms", best * 1e3);
        }
    }
}
