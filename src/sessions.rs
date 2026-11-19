//! Sesiones: qué se está ejecutando en cada pestaña.
//!
//! Se pregunta al PTY por su grupo de procesos en primer plano (`tcgetpgrp`) y se identifica
//! si es un agente de código conocido. El estado se deduce de la actividad de la salida:
//!   - Trabajando: el agente ha escrito en la pantalla hace muy poco.
//!   - Terminado: dejó de trabajar mientras no se miraba esa pestaña (pendiente de ver).
//!   - Esperando: inactivo y ya visto, esperando un mensaje.
//!   - Cerrado: el agente salió y la pestaña volvió al shell.
//! Todo se recalcula solo cuando hay salida o mientras algún agente trabaja: en reposo, nada.

use std::os::fd::RawFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Sin salida durante este tiempo, el agente se considera parado.
pub const WORK_WINDOW: Duration = Duration::from_millis(1500);
/// Cada cuánto, como mucho, se vuelve a identificar el proceso en primer plano.
const IDENTIFY_EVERY: Duration = Duration::from_millis(700);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Agent {
    /// Nombre que se muestra ("Claude Code").
    pub name: &'static str,
    /// Lanza subagentes que se pueden seguir desde el panel (ver `subagents.rs`).
    pub subagents: bool,
}

const fn agent(name: &'static str) -> Agent {
    Agent { name, subagents: false }
}

/// Nombre del proceso (o de su script) -> agente.
const AGENTS: &[(&str, Agent)] = &[
    ("claude", Agent { name: "Claude Code", subagents: true }),
    ("codex", agent("Codex")),
    ("gemini", agent("Gemini CLI")),
    ("aider", agent("Aider")),
    ("opencode", agent("OpenCode")),
    ("cursor-agent", agent("Cursor Agent")),
    ("copilot", agent("Copilot CLI")),
    ("qwen", agent("Qwen Code")),
    ("goose", agent("Goose")),
    ("amp", agent("Amp")),
    ("crush", agent("Crush")),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Working,
    Done,
    Waiting,
    Exited,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Working => "Trabajando…",
            Status::Done => "Terminado",
            Status::Waiting => "Esperando tu mensaje",
            Status::Exited => "Cerrado",
        }
    }
}

#[derive(Debug)]
pub struct Session {
    /// Último agente detectado en la pestaña (se conserva al cerrarse para mostrar "Cerrado").
    pub agent: Option<Agent>,
    pub status: Status,
    /// Momento del último cambio de estado.
    pub since: Instant,
    /// Proceso del agente (líder del grupo en primer plano) mientras se está ejecutando.
    pub pid: Option<i32>,
    checked: Option<Instant>,
}

impl Default for Session {
    fn default() -> Self {
        Self { agent: None, status: Status::Waiting, since: Instant::now(), pid: None, checked: None }
    }
}

impl Session {
    /// Recalcula el estado. `viewing` = la pestaña está a la vista del usuario.
    /// Devuelve `true` si cambió algo visible.
    pub fn refresh(&mut self, fd: Option<RawFd>, last_output: Instant, viewing: bool, now: Instant) -> bool {
        let identify = self.checked.is_none_or(|t| now.duration_since(t) >= IDENTIFY_EVERY);
        let mut present = self.agent.is_some() && self.status != Status::Exited;
        let mut changed = false;
        if identify {
            self.checked = Some(now);
            let pgid = fd.and_then(foreground_pgid);
            match pgid.and_then(identify_agent) {
                Some(a) => {
                    self.pid = pgid;
                    if self.agent != Some(a) || self.status == Status::Exited {
                        self.agent = Some(a);
                        self.set(Status::Waiting, now);
                        changed = true;
                    }
                    present = true;
                }
                None if present => {
                    self.pid = None;
                    self.set(Status::Exited, now);
                    return true;
                }
                None => {}
            }
        }
        if !present {
            return changed;
        }
        let working = now.duration_since(last_output) < WORK_WINDOW;
        let next = match (working, self.status) {
            (true, _) => Status::Working,
            (false, Status::Working) if !viewing => Status::Done,
            (false, Status::Done) if !viewing => Status::Done,
            _ => Status::Waiting,
        };
        if next != self.status {
            self.set(next, now);
            changed = true;
        }
        changed
    }

    /// El agente avisó con la campana (p. ej. al terminar): se marca pendiente de ver.
    pub fn bell(&mut self, viewing: bool, now: Instant) -> bool {
        if self.agent.is_some() && self.status != Status::Exited && !viewing && self.status != Status::Done {
            self.set(Status::Done, now);
            return true;
        }
        false
    }

    fn set(&mut self, s: Status, now: Instant) {
        self.status = s;
        self.since = now;
    }

    pub fn is_working(&self) -> bool {
        self.agent.is_some() && self.status == Status::Working
    }
}

pub fn foreground_pgid(fd: RawFd) -> Option<i32> {
    // SAFETY: tcgetpgrp solo lee el grupo de primer plano del terminal asociado al descriptor.
    let p = unsafe { libc::tcgetpgrp(fd) };
    (p > 0).then_some(p)
}

/// Identifica un agente por el nombre del proceso o de su script (node/python/bun…).
pub fn identify_agent(pid: i32) -> Option<Agent> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let mut names = vec![comm.trim().to_string()];
    names.extend(
        cmdline
            .split(|&b| b == 0)
            .take(3)
            .filter(|a| !a.is_empty() && a[0] != b'-')
            .map(|a| String::from_utf8_lossy(a).rsplit('/').next().unwrap_or_default().to_string()),
    );
    names.iter().find_map(|n| {
        let n = n.trim_end_matches(".js").trim_end_matches(".mjs").to_lowercase();
        AGENTS.iter().find(|(key, _)| n == *key).map(|(_, a)| *a)
    })
}

/// Directorio de trabajo de un proceso, abreviado con `~`.
pub fn cwd_label(pid: u32) -> Option<String> {
    let path: PathBuf = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    let s = path.to_string_lossy().to_string();
    let home = std::env::var("HOME").unwrap_or_default();
    Some(match s.strip_prefix(&home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("~{rest}"),
        _ => s,
    })
}

/// "ahora", "hace 27 s", "hace 3 min", "hace 2 h"
pub fn ago(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=4 => "ahora".into(),
        5..=59 => format!("hace {s} s"),
        60..=3599 => format!("hace {} min", s / 60),
        _ => format!("hace {} h", s / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transiciones_de_estado() {
        let mut s = Session { agent: Some(AGENTS[0].1), pid: None, ..Default::default() };
        let t0 = Instant::now();
        // Evita re-identificar (no hay PTY en el test): se marca como recién comprobado.
        s.checked = Some(t0);
        assert!(s.refresh(None, t0, false, t0));
        assert_eq!(s.status, Status::Working);
        // Deja de escribir sin que se mire la pestaña: Terminado (pendiente de ver).
        let t1 = t0 + WORK_WINDOW + Duration::from_millis(200);
        s.checked = Some(t1);
        s.refresh(None, t0, false, t1);
        assert_eq!(s.status, Status::Done);
        // Al mirarla pasa a Esperando.
        s.refresh(None, t0, true, t1);
        assert_eq!(s.status, Status::Waiting);
    }

    #[test]
    fn textos() {
        assert_eq!(ago(Duration::from_secs(2)), "ahora");
        assert_eq!(ago(Duration::from_secs(27)), "hace 27 s");
        assert_eq!(ago(Duration::from_secs(125)), "hace 2 min");
    }
}
