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
    Ports(Vec<crate::ports::PortInfo>),
}

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
    /// Última vez que el programa escribió en pantalla.
    pub last_output: std::time::Instant,
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
        }
        changed
    }

    /// Actualiza los subagentes del agente en primer plano (si sabe lanzarlos).
    pub fn refresh_subagents(&mut self, now: std::time::Instant) -> bool {
        let pid = self.session.agent.filter(|a| a.subagents).and(self.session.pid);
        self.subagents.refresh(pid, now)
    }

    /// Título visible de la pestaña: el directorio actual (o el título del shell si no se sabe).
    pub fn label(&self) -> &str {
        if self.cwd_label.is_empty() { &self.title } else { &self.cwd_label }
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
