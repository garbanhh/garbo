//! Una pestaña = un PTY + un `Term` de alacritty_terminal + sus bloques de comandos.
//! El PTY se lee en su propio hilo (dormido hasta que hay datos); la UI se entera vía eventos.

use crate::blocks::BlockState;
use crate::config::{Config, CursorStyle};
use crate::pty_loop::{PtyLoop, PtySender};
use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::Msg;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle as VteCursorStyle};
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use winit::event_loop::EventLoopProxy;

/// Eventos que llegan al hilo de la UI desde otros hilos.
#[derive(Debug)]
pub enum UserEvent {
    Term(u64, TermEvent),
    Update(crate::update::UpdateState),
    /// Resultado de "Buscar actualizaciones" en Ajustes.
    UpdateCheck(crate::update::CheckState),
    Ports(Vec<crate::ports::PortInfo>),
    /// Imagen del portapapeles guardada (o error) para pegar su ruta en la pestaña.
    PasteImage(u64, Result<PathBuf, String>),
}

/// Salida que llega justo después de que el usuario escriba es su eco (o el redibujado de la
/// aplicación al teclear), no trabajo del programa.
const ECHO_WINDOW: std::time::Duration = std::time::Duration::from_millis(400);

#[derive(Clone)]
pub struct Listener {
    tab_id: u64,
    proxy: EventLoopProxy<UserEvent>,
}

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let _ = self.proxy.send_event(UserEvent::Term(self.tab_id, event));
    }
}

#[derive(Clone, Copy)]
pub struct TermSize {
    pub columns: usize,
    pub lines: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

pub struct Tab {
    pub id: u64,
    pub term: Arc<FairMutex<Term<Listener>>>,
    pub blocks: Arc<Mutex<BlockState>>,
    /// Bloque seleccionado (navegación con Ctrl+Shift+↑/↓ o clic en el margen).
    pub selected_block: Option<u32>,
    sender: PtySender,
    /// Copia del descriptor maestro del PTY (para saber qué proceso está en primer plano).
    master: Option<std::fs::File>,
    /// Última vez que el programa escribió en pantalla por su cuenta (sin contar el eco).
    pub last_output: std::time::Instant,
    /// Última vez que el usuario escribió en la pestaña (teclado, pegar, ratón).
    last_input: std::cell::Cell<std::time::Instant>,
    /// Lo que se está haciendo, para el título de la pestaña ("cargo build", "GRBTER"…).
    task: String,
    /// Grupo de procesos en primer plano cuando llegó el último título (OSC 0/2).
    title_owner: Option<i32>,
    pub session: crate::sessions::Session,
    /// Subagentes de Claude Code (solo se consultan con el panel de agentes abierto).
    pub subagents: crate::subagents::Watch,
    /// Directorio actual abreviado (título de la pestaña).
    pub cwd_label: String,
    cwd_checked: Option<std::time::Instant>,
    pub title: String,
    pub child_pid: u32,
    pub exited: bool,
}

pub fn term_config(cfg: &Config) -> TermConfig {
    let shape = match cfg.terminal.cursor_style {
        CursorStyle::Block => CursorShape::Block,
        CursorStyle::Beam => CursorShape::Beam,
        CursorStyle::Underline => CursorShape::Underline,
    };
    TermConfig {
        scrolling_history: cfg.terminal.scrollback,
        default_cursor_style: VteCursorStyle { shape, blinking: cfg.terminal.cursor_blink },
        ..Default::default()
    }
}

/// Shell a lanzar. Con bash se inyecta la integración de bloques vía `--rcfile`
/// (carga tu ~/.bashrc igualmente), salvo que el usuario haya puesto argumentos propios.
fn shell_command(cfg: &Config) -> tty::Shell {
    let program = cfg
        .shell
        .program
        .clone()
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/bash".into());
    let mut args = cfg.shell.args.clone();
    let is_bash = program.rsplit('/').next() == Some("bash");
    if cfg.terminal.shell_integration && is_bash && args.is_empty() {
        match crate::blocks::bash_integration_path() {
            Some(path) => args = vec!["--rcfile".into(), path],
            None => log::warn!("No se pudo escribir el script de integración de bash"),
        }
    }
    tty::Shell::new(program, args)
}

impl Tab {
    pub fn spawn(
        id: u64,
        cfg: &Config,
        ws: WindowSize,
        cwd: Option<PathBuf>,
        proxy: EventLoopProxy<UserEvent>,
    ) -> std::io::Result<Self> {
        let listener = Listener { tab_id: id, proxy };
        let size = TermSize { columns: ws.num_cols as usize, lines: ws.num_lines as usize };
        let term = Arc::new(FairMutex::new(Term::new(term_config(cfg), &size, listener.clone())));

        let mut env = cfg.shell.env.clone();
        env.entry("TERM".into()).or_insert_with(|| "xterm-256color".into());
        env.insert("COLORTERM".into(), "truecolor".into());
        env.insert("TERM_PROGRAM".into(), "garbo".into());
        env.insert("TERM_PROGRAM_VERSION".into(), env!("CARGO_PKG_VERSION").into());
        // No propagar al shell el filtro de drivers Vulkan que pone el renderer.
        for var in crate::renderer::env_overrides() {
            env.entry(var.to_string()).or_default();
        }

        let opts = tty::Options {
            shell: Some(shell_command(cfg)),
            working_directory: cwd,
            drain_on_exit: false,
            env,
            ..Default::default()
        };

        let pty = tty::new(&opts, ws, id)?;
        let child_pid = pty.child().id();
        let master = pty.file().try_clone().ok();
        let blocks = Arc::new(Mutex::new(BlockState::default()));
        let pty_loop = PtyLoop::new(term.clone(), listener, pty, blocks.clone())?;
        let sender = pty_loop.sender();
        pty_loop.spawn();

        let title = cfg
            .shell
            .program
            .clone()
            .or_else(|| std::env::var("SHELL").ok())
            .and_then(|s| s.rsplit('/').next().map(str::to_owned))
            .unwrap_or_else(|| "shell".into());

        Ok(Self {
            id,
            term,
            blocks,
            selected_block: None,
            sender,
            master,
            last_output: std::time::Instant::now(),
            last_input: std::cell::Cell::new(std::time::Instant::now()),
            task: String::new(),
            title_owner: None,
            session: Default::default(),
            subagents: Default::default(),
            cwd_label: String::new(),
            cwd_checked: None,
            title,
            child_pid,
            exited: false,
        })
    }

    pub fn write<B: Into<Cow<'static, [u8]>>>(&self, bytes: B) {
        let bytes = bytes.into();
        if !bytes.is_empty() {
            self.sender.send(Msg::Input(bytes));
        }
    }

    /// Entrada del usuario: además de escribirla, la anota para no confundir su eco con trabajo.
    pub fn input<B: Into<Cow<'static, [u8]>>>(&self, bytes: B) {
        self.last_input.set(std::time::Instant::now());
        self.write(bytes);
    }

    /// El programa escribió en pantalla.
    pub fn note_output(&mut self, now: std::time::Instant) {
        if !is_echo(self.last_input.get(), now) {
            self.last_output = now;
        }
    }

    /// Nuevo título (OSC 0/2): se recuerda qué proceso lo puso.
    pub fn set_title(&mut self, title: String) {
        use std::os::fd::AsRawFd;
        self.title_owner = self.master.as_ref().and_then(|f| crate::sessions::foreground_pgid(f.as_raw_fd()));
        self.title = title;
        self.task = self.compute_task();
    }

    /// Resto del comando sugerido desde el historial, si se está escribiendo en el prompt.
    pub fn suggestion(&self) -> Option<String> {
        let cwd = self.cwd();
        let term = self.term.lock();
        let start = self.blocks.lock().unwrap().input_start;
        suggestion_for(&term, start, cwd.as_deref())
    }

    pub fn resize(&mut self, ws: WindowSize) {
        self.sender.send(Msg::Resize(ws));
        let size = TermSize { columns: ws.num_cols as usize, lines: ws.num_lines as usize };
        self.term.lock().resize(size);
    }

    /// Actualiza sesión (agente y estado) y directorio. Devuelve `true` si cambió algo visible.
    pub fn refresh_session(&mut self, viewing: bool, now: std::time::Instant) -> bool {
        use std::os::fd::AsRawFd;
        let fd = self.master.as_ref().map(|f| f.as_raw_fd());
        let mut changed = self.session.refresh(fd, self.last_output, viewing, now);
        if self.cwd_checked.is_none_or(|t| now.duration_since(t).as_millis() >= 1000) {
            self.cwd_checked = Some(now);
            if let Some(l) = crate::sessions::cwd_label(self.child_pid) {
                if l != self.cwd_label {
                    self.cwd_label = l;
                    changed = true;
                }
            }
            let task = self.compute_task();
            if task != self.task {
                self.task = task;
                changed = true;
            }
        }
        changed
    }

    /// Qué se está haciendo en la pestaña:
    ///   - un programa que puso título (Claude Code pone el tema de la conversación): ese título;
    ///   - un agente sin título: su nombre;
    ///   - otro programa: su orden ("cargo build --release");
    ///   - el shell esperando: el nombre de la carpeta.
    fn compute_task(&self) -> String {
        use std::os::fd::AsRawFd;
        let fg = self.master.as_ref().and_then(|f| crate::sessions::foreground_pgid(f.as_raw_fd()));
        match fg {
            Some(pg) if pg != self.child_pid as i32 => {
                let title = clean_title(&self.title);
                if self.title_owner == Some(pg) && !title.is_empty() {
                    title
                } else if let Some(agent) = self.session.agent.filter(|_| self.session.pid == Some(pg)) {
                    agent.name.to_string()
                } else {
                    crate::sessions::command_label(pg).unwrap_or_default()
                }
            }
            _ => short_dir(&self.cwd_label),
        }
    }

    /// Directorio actual abreviado (para el panel de agentes).
    pub fn dir_label(&self) -> &str {
        if self.cwd_label.is_empty() { &self.title } else { &self.cwd_label }
    }

    /// Actualiza los subagentes del agente en primer plano (si sabe lanzarlos).
    pub fn refresh_subagents(&mut self, now: std::time::Instant) -> bool {
        let pid = self.session.agent.filter(|a| a.subagents).and(self.session.pid);
        self.subagents.refresh(pid, now)
    }

    /// Título visible de la pestaña: lo que se está haciendo (ver `compute_task`).
    pub fn label(&self) -> &str {
        if !self.task.is_empty() {
            &self.task
        } else if !self.cwd_label.is_empty() {
            &self.cwd_label
        } else {
            &self.title
        }
    }

    /// Directorio actual del shell (Linux: /proc/<pid>/cwd). Las pestañas nuevas lo heredan.
    pub fn cwd(&self) -> Option<PathBuf> {
        #[cfg(target_os = "linux")]
        {
            std::fs::read_link(format!("/proc/{}/cwd", self.child_pid)).ok()
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }
}

impl Drop for Tab {
    fn drop(&mut self) {
        self.sender.send(Msg::Shutdown);
    }
}

/// ¿Es la salida de `now` el eco de lo que el usuario acaba de escribir?
fn is_echo(last_input: std::time::Instant, now: std::time::Instant) -> bool {
    now.saturating_duration_since(last_input) <= ECHO_WINDOW
}

/// Sugerencia para un terminal ya bloqueado (`start` = inicio de lo escrito en el prompt).
pub fn suggestion_for<T>(term: &Term<T>, start: Option<(i64, usize)>, cwd: Option<&std::path::Path>) -> Option<String> {
    use alacritty_terminal::term::TermMode;
    if term.mode().contains(TermMode::ALT_SCREEN) || term.grid().display_offset() != 0 {
        return None;
    }
    let input = crate::suggest::current_input(term, start?)?;
    crate::suggest::suggest(&input, cwd)
}

/// Quita del título los símbolos de estado que algunos programas anteponen ("✳ Tarea" -> "Tarea").
pub fn clean_title(t: &str) -> String {
    t.trim_start_matches(|c: char| !c.is_alphanumeric() && !"([{\"'¿¡~/.".contains(c)).trim().to_string()
}

/// Última carpeta de una ruta abreviada ("~/Documentos/Proyectos/GRBTER" -> "GRBTER").
pub fn short_dir(path: &str) -> String {
    match path.trim_end_matches('/') {
        "" if path.starts_with('/') => "/".into(),
        "" => String::new(),
        p => p.rsplit('/').next().unwrap_or(p).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escribir_no_cuenta_como_trabajo() {
        let t0 = std::time::Instant::now();
        assert!(is_echo(t0, t0 + std::time::Duration::from_millis(30)), "eco al teclear");
        assert!(is_echo(t0, t0 + ECHO_WINDOW));
        assert!(!is_echo(t0, t0 + ECHO_WINDOW + std::time::Duration::from_millis(1)), "salida propia");
    }

    #[test]
    fn titulos_de_pestana() {
        assert_eq!(clean_title("✳ Rediseñar los ajustes"), "Rediseñar los ajustes");
        assert_eq!(clean_title("⠂ Claude Code"), "Claude Code");
        assert_eq!(clean_title("~/src"), "~/src");
        assert_eq!(short_dir("~/Documentos/Proyectos/GRBTER"), "GRBTER");
        assert_eq!(short_dir("~"), "~");
        assert_eq!(short_dir("/"), "/");
    }
}
