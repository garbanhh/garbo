//! Una pestaña = un PTY + un `Term` de alacritty_terminal + sus bloques de comandos.
//! El PTY se lee en su propio hilo (dormido hasta que hay datos); la UI se entera vía eventos.

use crate::blocks::BlockState;
use crate::config::{Config, CursorStyle};
use crate::pty_loop::{PtyLoop, PtySender};
use crate::shell::Shell;
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
    /// Imagen del portapapeles guardada (o archivos copiados, en Windows), o el error, para pegar
    /// sus rutas en la pestaña.
    PasteImage(u64, Result<Vec<PathBuf>, String>),
    /// Aviso mostrado (pestaña, proceso `notify-send`) y aviso pulsado (pestaña, permiso de
    /// activación del escritorio). Ver `notify.rs` y `activation.rs`.
    NoticeShown(u64, u32),
    NoticeClicked(u64, Option<String>),
    /// Estadísticas de los agentes leídas por su hilo (ver `stats.rs`).
    Stats(crate::stats::StatsMsg),
    /// Cambios de git leídos por su hilo (ver `git.rs`).
    Git(crate::git::Msg),
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
    /// Tamaño actual (filas y columnas de su panel).
    pub ws: WindowSize,
    pub term: Arc<FairMutex<Term<Listener>>>,
    pub blocks: Arc<Mutex<BlockState>>,
    /// Bloque seleccionado (navegación con Ctrl+Shift+↑/↓ o clic en el margen).
    pub selected_block: Option<u32>,
    sender: PtySender,
    /// Para saber qué se ejecuta en primer plano (ver `platform`).
    probe: crate::platform::Probe,
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
    /// Lo último que se le pidió al agente en primer plano (nombre de la pestaña).
    pub request: crate::subagents::Request,
    /// Aviso del escritorio cuando el agente termina.
    pub notice: crate::notify::Tracker,
    /// Directorio actual abreviado (título de la pestaña).
    pub cwd_label: String,
    cwd_checked: Option<std::time::Instant>,
    pub title: String,
    pub child_pid: u32,
    /// Shell de la pestaña (los atajos de edición cambian entre ellos).
    pub shell: Shell,
    pub exited: bool,
}

pub fn term_config(cfg: &Config) -> TermConfig {
    let shape = match cfg.terminal.cursor_style {
        CursorStyle::Block => CursorShape::Block,
        CursorStyle::Beam => CursorShape::Beam,
        CursorStyle::Underline => CursorShape::Underline,
        CursorStyle::Hollow => CursorShape::HollowBlock,
    };
    TermConfig {
        scrolling_history: cfg.terminal.scrollback,
        default_cursor_style: VteCursorStyle {
            shape,
            blinking: cfg.terminal.cursor_blink,
        },
        ..Default::default()
    }
}

/// Programa del shell: el de la configuración o el del sistema (`$SHELL` o bash; en Windows,
/// PowerShell).
pub fn shell_program(cfg: &Config) -> String {
    cfg.shell.program.clone().unwrap_or_else(default_shell)
}

#[cfg(unix)]
fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())
}

/// PowerShell 7 (`pwsh`) si está instalado y, si no, Windows PowerShell, que viene con el sistema.
/// `$SHELL` no se mira: si Garbo se abre desde Git Bash apunta a una ruta de MSYS (`/usr/bin/bash`).
#[cfg(windows)]
fn default_shell() -> String {
    static SHELL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SHELL
        .get_or_init(|| {
            crate::platform::find_program("pwsh.exe")
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| "powershell.exe".into())
        })
        .clone()
}

/// Shell a lanzar y variables que añadir a su entorno. Con bash, zsh y fish se inyecta la
/// integración de bloques (`shell/`), que carga igualmente sus archivos de arranque de siempre,
/// salvo que el usuario haya puesto argumentos propios.
pub fn shell_command(cfg: &Config) -> (tty::Shell, Shell, Vec<(String, String)>) {
    let program = shell_program(cfg);
    let kind = Shell::from_program(&program);
    let mut args = cfg.shell.args.clone();
    let mut env = Vec::new();
    if cfg.terminal.shell_integration && args.is_empty() && kind != Shell::Other {
        let zdotdir = cfg
            .shell
            .env
            .get("ZDOTDIR")
            .cloned()
            .or_else(|| std::env::var("ZDOTDIR").ok());
        match kind.injection(zdotdir.as_deref()) {
            Some(inj) => (args, env) = (inj.args, inj.env),
            None => log::warn!(
                "No se pudo escribir el script de integración de {}",
                kind.name()
            ),
        }
    }
    (tty::Shell::new(program, args), kind, env)
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
        let size = TermSize {
            columns: ws.num_cols as usize,
            lines: ws.num_lines as usize,
        };
        let term = Arc::new(FairMutex::new(Term::new(
            term_config(cfg),
            &size,
            listener.clone(),
        )));

        let mut env = cfg.shell.env.clone();
        env.entry("TERM".into())
            .or_insert_with(|| "xterm-256color".into());
        env.insert("COLORTERM".into(), "truecolor".into());
        env.insert("TERM_PROGRAM".into(), "garbo".into());
        env.insert(
            "TERM_PROGRAM_VERSION".into(),
            env!("CARGO_PKG_VERSION").into(),
        );
        // Prompt de Garbo (lo pone la integración del shell; ver shell/).
        if cfg.terminal.garbo_prompt {
            env.insert("GARBO_PROMPT".into(), "1".into());
        } else {
            env.remove("GARBO_PROMPT");
        }
        // Con las sugerencias de Garbo, la integración de fish apaga las suyas: se pisarían.
        if cfg.terminal.autosuggest {
            env.insert("GARBO_AUTOSUGGEST".into(), "1".into());
        } else {
            env.remove("GARBO_AUTOSUGGEST");
        }
        // No propagar al shell el filtro de drivers Vulkan que pone el renderer. No se puede quitar
        // una variable, solo vaciarla: el cargador de Vulkan y Garbo tratan la vacía como no puesta.
        for var in crate::renderer::env_overrides() {
            env.entry(var.to_string()).or_default();
        }

        let (shell, kind, shell_env) = shell_command(cfg);
        env.extend(shell_env);
        crate::shell::set_current(kind);
        let opts = tty::Options {
            shell: Some(shell),
            working_directory: cwd,
            drain_on_exit: false,
            env,
            // Rutas con espacios ("C:\Users\Ana López\…") entre comillas, como las espera el shell.
            #[cfg(windows)]
            escape_args: true,
        };

        let pty = tty::new(&opts, ws, id)?;
        let child_pid = crate::platform::child_pid(&pty);
        let probe = crate::platform::Probe::new(&pty);
        let blocks = Arc::new(Mutex::new(BlockState::new(child_pid)));
        let pty_loop = PtyLoop::new(term.clone(), listener, pty, blocks.clone())?;
        let sender = pty_loop.sender();
        pty_loop.spawn();

        let title = crate::platform::base_name(&shell_program(cfg)).to_owned();

        Ok(Self {
            id,
            ws,
            term,
            blocks,
            selected_block: None,
            sender,
            probe,
            last_output: std::time::Instant::now(),
            last_input: std::cell::Cell::new(std::time::Instant::now()),
            task: String::new(),
            title_owner: None,
            session: Default::default(),
            subagents: Default::default(),
            request: Default::default(),
            notice: Default::default(),
            cwd_label: String::new(),
            cwd_checked: None,
            title,
            child_pid,
            shell: kind,
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
        self.title_owner = self.probe.foreground();
        self.title = title;
        self.task = self.compute_task();
    }

    /// Resto del comando sugerido (en gris), si se está escribiendo en el prompt.
    pub fn suggestion(&self) -> Option<String> {
        let cwd = self.cwd();
        let term = self.term.lock();
        let start = self.blocks.lock().unwrap().input_start;
        suggestion_for(&term, start, cwd.as_deref())
    }

    /// Lo escrito en el prompt hasta el cursor, si se puede leer.
    pub fn prompt_input(&self) -> Option<String> {
        let term = self.term.lock();
        let start = self.blocks.lock().unwrap().input_start;
        input_for(&term, start)
    }

    /// ¿Espera el shell un comando (con la integración de Garbo)? Entre las marcas B y C.
    pub fn at_prompt(&self) -> bool {
        let waiting = self.blocks.lock().unwrap().input_start.is_some();
        waiting
            && !self
                .term
                .lock()
                .mode()
                .contains(alacritty_terminal::term::TermMode::ALT_SCREEN)
    }

    /// A quién van los atajos de edición (ver `edit.rs`): el prompt, un agente o lo de siempre.
    pub fn edit_context(&self) -> crate::edit::Context {
        if self.at_prompt() {
            return crate::edit::Context::Prompt(self.shell);
        }
        match self
            .probe
            .foreground()
            .and_then(crate::sessions::identify_agent)
        {
            Some(a) => crate::edit::Context::Agent(a.name),
            None => crate::edit::Context::Other,
        }
    }

    /// Flechas para llevar el cursor del prompt a la celda pulsada (`None`: no es lo escrito).
    pub fn click_moves(
        &self,
        p: alacritty_terminal::index::Point,
        side: alacritty_terminal::index::Side,
    ) -> Option<Vec<u8>> {
        let term = self.term.lock();
        let start = self.blocks.lock().unwrap().input_start?;
        crate::edit::click_moves(&term, start, p, side)
    }

    /// ↑ / ↓ dentro de un comando de varias líneas (`None`: las flechas recorren el historial).
    pub fn vertical_moves(&self, up: bool) -> Option<Vec<u8>> {
        let term = self.term.lock();
        let start = self.blocks.lock().unwrap().input_start?;
        crate::edit::vertical_moves(&term, start, up)
    }

    /// Candidatos del menú de completado para lo escrito en el prompt (y lo escrito).
    pub fn completions(&self) -> Option<(String, crate::complete::Completions)> {
        let input = self.prompt_input()?;
        let cwd = self.cwd();
        let comp = crate::complete::complete(&input, cwd.as_deref())?;
        Some((input, comp))
    }

    pub fn resize(&mut self, ws: WindowSize) {
        self.ws = ws;
        self.sender.send(Msg::Resize(ws));
        let size = TermSize {
            columns: ws.num_cols as usize,
            lines: ws.num_lines as usize,
        };
        self.term.lock().resize(size);
    }

    /// Actualiza sesión (agente y estado) y directorio. Devuelve `true` si cambió algo visible.
    pub fn refresh_session(&mut self, viewing: bool, now: std::time::Instant) -> bool {
        let report = self.blocks.lock().unwrap().program.summary();
        let mut changed =
            self.session
                .refresh(Some(&self.probe), self.last_output, viewing, now, report);
        // Lo que se le pidió al agente, para el nombre de la pestaña.
        let asked = self
            .session
            .agent
            .and_then(|a| a.subagents)
            .zip(self.session.pid);
        if self.request.refresh(asked, now) {
            self.task = self.compute_task();
            changed = true;
        }
        if self
            .cwd_checked
            .is_none_or(|t| now.duration_since(t).as_millis() >= 1000)
        {
            self.cwd_checked = Some(now);
            if let Some(l) = self.cwd().map(|p| crate::platform::tilde(&p)) {
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
    ///   - un agente: su nombre corto y lo último que se le pidió
    ///     ("Claude · Arregla el menú"), o su título si aún no se sabe, o su nombre;
    ///   - otro programa que puso título: ese título;
    ///   - otro programa: su orden ("cargo build --release");
    ///   - el shell esperando: el nombre de la carpeta.
    fn compute_task(&self) -> String {
        match self.probe.foreground() {
            Some(pg) if pg != self.child_pid as i32 => {
                let title = clean_title(&self.title);
                let agent = self.session.agent.filter(|_| self.session.pid == Some(pg));
                if let Some(agent) = agent {
                    let own_title =
                        (self.title_owner == Some(pg) && !title.is_empty()).then_some(title);
                    return match self.request.task.clone().or(own_title) {
                        Some(what) => format!("{} · {what}", crate::tabs::agent_short(agent.name)),
                        None => agent.name.to_string(),
                    };
                }
                if self.title_owner == Some(pg) && !title.is_empty() {
                    title
                } else {
                    crate::sessions::command_label(pg).unwrap_or_default()
                }
            }
            _ => short_dir(&self.cwd_label),
        }
    }

    /// Lo que se le pidió al agente (sin su nombre), si se sabe: para los avisos.
    pub fn agent_task(&self) -> Option<&str> {
        self.request.task.as_deref()
    }

    /// Directorio actual abreviado (para el panel de agentes).
    pub fn dir_label(&self) -> &str {
        if self.cwd_label.is_empty() {
            &self.title
        } else {
            &self.cwd_label
        }
    }

    /// Actualiza los subagentes del agente en primer plano (si sabe lanzarlos).
    pub fn refresh_subagents(&mut self, now: std::time::Instant) -> bool {
        let target = self
            .session
            .agent
            .and_then(|a| a.subagents)
            .zip(self.session.pid);
        self.subagents.refresh(target, now)
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

    /// Directorio actual del shell (ver `BlockState::shell_cwd`). Las pestañas nuevas lo heredan.
    pub fn cwd(&self) -> Option<PathBuf> {
        self.blocks.lock().unwrap().shell_cwd()
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

/// Lo escrito en el prompt para un terminal ya bloqueado (`start` = inicio de la entrada). Nada
/// con la pantalla alternativa (vim, less…) ni con el historial desplazado.
pub fn input_for<T>(term: &Term<T>, start: Option<(i64, usize)>) -> Option<String> {
    use alacritty_terminal::term::TermMode;
    if term.mode().contains(TermMode::ALT_SCREEN) || term.grid().display_offset() != 0 {
        return None;
    }
    crate::suggest::current_input(term, start?)
}

/// Sugerencia para un terminal ya bloqueado (`start` = inicio de lo escrito en el prompt).
pub fn suggestion_for<T>(
    term: &Term<T>,
    start: Option<(i64, usize)>,
    cwd: Option<&std::path::Path>,
) -> Option<String> {
    crate::suggest::suggest(&input_for(term, start)?, cwd)
}

/// Quita del título los símbolos de estado que algunos programas anteponen ("✳ Tarea" -> "Tarea").
pub fn clean_title(t: &str) -> String {
    t.trim_start_matches(|c: char| !c.is_alphanumeric() && !"([{\"'¿¡~/.".contains(c))
        .trim()
        .to_string()
}

/// Última carpeta de una ruta abreviada ("~/Documentos/Proyectos/GRBTER" -> "GRBTER"). En
/// Windows separa también con `\` y la raíz de una unidad se queda entera ("C:\").
pub fn short_dir(path: &str) -> String {
    use crate::platform::is_separator;
    match path.trim_end_matches(is_separator) {
        "" if path.starts_with(is_separator) => path[..1].into(),
        "" => String::new(),
        p if cfg!(windows) && p.len() == 2 && p.ends_with(':') => format!("{p}\\"),
        p => p.rsplit(is_separator).next().unwrap_or(p).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escribir_no_cuenta_como_trabajo() {
        let t0 = std::time::Instant::now();
        assert!(
            is_echo(t0, t0 + std::time::Duration::from_millis(30)),
            "eco al teclear"
        );
        assert!(is_echo(t0, t0 + ECHO_WINDOW));
        assert!(
            !is_echo(t0, t0 + ECHO_WINDOW + std::time::Duration::from_millis(1)),
            "salida propia"
        );
    }

    #[test]
    fn titulos_de_pestana() {
        assert_eq!(
            clean_title("✳ Rediseñar los ajustes"),
            "Rediseñar los ajustes"
        );
        assert_eq!(clean_title("⠂ Claude Code"), "Claude Code");
        assert_eq!(clean_title("~/src"), "~/src");
        assert_eq!(short_dir("~/Documentos/Proyectos/GRBTER"), "GRBTER");
        assert_eq!(short_dir("~"), "~");
        assert_eq!(short_dir("/"), "/");
    }
}
