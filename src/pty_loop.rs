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
use std::time::Instant;

const READ_BUFFER_SIZE: usize = 0x10_0000;
/// Máximo de bytes procesados con el terminal bloqueado antes de soltarlo para la UI.
const MAX_LOCKED_READ: usize = u16::MAX as usize;
// Mismos valores que los tokens (privados) de alacritty_terminal::tty.
const PTY_READ_WRITE_TOKEN: usize = 0;
const PTY_CHILD_EVENT_TOKEN: usize = 1;

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

#[derive(Default)]
struct State {
    write_list: VecDeque<Cow<'static, [u8]>>,
    writing: Option<(Cow<'static, [u8]>, usize)>,
    parser: ansi::Processor,
    scanner: MarkScanner,
    marks: Vec<(usize, Mark)>,
}

impl<U: EventListener + Send + 'static> PtyLoop<U> {
    pub fn new(
        terminal: Arc<FairMutex<Term<U>>>,
        event_proxy: U,
        pty: tty::Pty,
        blocks: Arc<Mutex<BlockState>>,
    ) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        Ok(Self { poll: Poller::new()?.into(), pty, rx, peeked: None, tx, terminal, event_proxy, blocks })
    }

    pub fn sender(&self) -> PtySender {
        PtySender { tx: self.tx.clone(), poller: self.poll.clone() }
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

    /// Pasa los bytes al parser deteniéndose en cada marca de shell para aplicarla.
    fn feed(state: &mut State, term: &mut Term<U>, bytes: &[u8], blocks: &Mutex<BlockState>) {
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
                bs.apply(mark, term);
            }
            state.parser.advance(term, &bytes[pos..]);
        }
        state.marks = marks;
    }

    fn pty_read(&mut self, state: &mut State, buf: &mut [u8]) -> io::Result<()> {
        let mut unprocessed = 0;
        let mut processed = 0;
        // Reserva el siguiente turno del lock para la lectura del PTY.
        let _lease = self.terminal.lease();
        let mut terminal = None;

        loop {
            match self.pty.reader().read(&mut buf[unprocessed..]) {
                Ok(0) if unprocessed == 0 => break,
                Ok(got) => unprocessed += got,
                Err(err) => match err.kind() {
                    ErrorKind::Interrupted | ErrorKind::WouldBlock => {
                        if unprocessed == 0 {
                            break;
                        }
                    }
                    _ => return Err(err),
                },
            }

            let terminal = match &mut terminal {
                Some(t) => t,
                None => terminal.insert(match self.terminal.try_lock_unfair() {
                    None if unprocessed >= READ_BUFFER_SIZE => self.terminal.lock_unfair(),
                    None => continue,
                    Some(t) => t,
                }),
            };

            Self::feed(state, terminal, &buf[..unprocessed], &self.blocks);
            processed += unprocessed;
            unprocessed = 0;

            if processed >= MAX_LOCKED_READ {
                break;
            }
        }

        // Pedir redibujado salvo que todo fuese una actualización sincronizada pendiente.
        if state.parser.sync_bytes_count() < processed && processed > 0 {
            self.event_proxy.send_event(Event::Wakeup);
        }
        Ok(())
    }

    fn pty_write(&mut self, state: &mut State) -> io::Result<()> {
        loop {
            if state.writing.is_none() {
                state.writing = state.write_list.pop_front().map(|b| (b, 0));
            }
            let Some((bytes, written)) = state.writing.as_mut() else { return Ok(()) };
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
                let mut events = Events::with_capacity(NonZeroUsize::new(1024).unwrap());

                'event_loop: loop {
                    // Despertar si vence el plazo de una actualización sincronizada.
                    let timeout = state
                        .parser
                        .sync_timeout()
                        .sync_timeout()
                        .map(|t| t.saturating_duration_since(Instant::now()));

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

                    if events.is_empty() && !self.has_pending() {
                        state.parser.stop_sync(&mut *self.terminal.lock());
                        self.event_proxy.send_event(Event::Wakeup);
                        continue;
                    }

                    if !self.drain_channel(&mut state) {
                        break;
                    }

                    for event in events.iter() {
                        match event.key {
                            PTY_CHILD_EVENT_TOKEN => {
                                if let Some(tty::ChildEvent::Exited(status)) = self.pty.next_child_event() {
                                    if let Some(status) = status {
                                        self.event_proxy.send_event(Event::ChildExit(status));
                                    }
                                    self.terminal.lock().exit();
                                    self.event_proxy.send_event(Event::Wakeup);
                                    break 'event_loop;
                                }
                            }
                            PTY_READ_WRITE_TOKEN => {
                                if event.is_interrupt() {
                                    continue;
                                }
                                if event.readable {
                                    if let Err(err) = self.pty_read(&mut state, &mut buf) {
                                        // Linux: EIO cuando el hijo cuelga; llegará el evento de salida.
                                        #[cfg(target_os = "linux")]
                                        if err.raw_os_error() == Some(libc::EIO) {
                                            continue;
                                        }
                                        log::error!("Error leyendo del PTY: {err}");
                                        break 'event_loop;
                                    }
                                }
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

                    // Interés de escritura solo mientras haya algo pendiente.
                    let needs_write = state.writing.is_some() || !state.write_list.is_empty();
                    if needs_write != interest.writable {
                        interest.writable = needs_write;
                        let _ = self.pty.reregister(&self.poll, interest, poll_opts);
                    }
                }

                let _ = self.pty.deregister(&self.poll);
            })
            .expect("no se pudo crear el hilo del PTY");
    }
}
