//! Sesiones: qué se está ejecutando en cada pestaña.
//!
//! Se pregunta al PTY por su grupo de procesos en primer plano (`tcgetpgrp`) y se identifica
//! si es un agente de código conocido. El estado se deduce de la actividad de la salida:
//!   - Trabajando: el agente ha escrito en la pantalla hace muy poco.
//!   - Terminado: dejó de trabajar mientras no se miraba esa pestaña (pendiente de ver).
//!   - Esperando: inactivo y ya visto, esperando un mensaje.
//!   - Cerrado: el agente salió y la pestaña volvió al shell.
//! Todo se recalcula solo cuando hay salida o mientras algún agente trabaja: en reposo, nada.

use crate::config::Rgb;
use std::os::fd::RawFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Sin salida durante este tiempo, el agente se considera parado.
pub const WORK_WINDOW: Duration = Duration::from_millis(1500);
/// Cada cuánto, como mucho, se vuelve a identificar el proceso en primer plano.
const IDENTIFY_EVERY: Duration = Duration::from_millis(700);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Agent {
    pub name: &'static str,
    /// Inicial para la insignia (genérica: no se usan logotipos de terceros).
    pub badge: char,
    pub color: Rgb,
}

const AGENTS: &[(&str, Agent)] = &[
    ("claude", Agent { name: "Claude Code", badge: 'C', color: Rgb(0xD9, 0x77, 0x57) }),
    ("codex", Agent { name: "Codex", badge: 'X', color: Rgb(0xC8, 0xC8, 0xC8) }),
    ("gemini", Agent { name: "Gemini CLI", badge: 'G', color: Rgb(0x6F, 0x9B, 0xF5) }),
    ("aider", Agent { name: "Aider", badge: 'A', color: Rgb(0x4E, 0xC9, 0x8B) }),
    ("opencode", Agent { name: "OpenCode", badge: 'O', color: Rgb(0xE5, 0xC0, 0x7B) }),
    ("cursor-agent", Agent { name: "Cursor Agent", badge: 'U', color: Rgb(0xB0, 0xB0, 0xB0) }),
    ("copilot", Agent { name: "Copilot CLI", badge: 'P', color: Rgb(0xA3, 0x8B, 0xF5) }),
    ("qwen", Agent { name: "Qwen Code", badge: 'Q', color: Rgb(0x7F, 0x6B, 0xF0) }),
    ("goose", Agent { name: "Goose", badge: 'S', color: Rgb(0xC5, 0x86, 0xC0) }),
    ("amp", Agent { name: "Amp", badge: 'M', color: Rgb(0xF1, 0x4C, 0x4C) }),
    ("crush", Agent { name: "Crush", badge: 'R', color: Rgb(0xE0, 0x6C, 0xC0) }),
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
    checked: Option<Instant>,
}

impl Default for Session {
    fn default() -> Self {
        Self { agent: None, status: Status::Waiting, since: Instant::now(), checked: None }
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
            let found = fd.and_then(foreground_pgid).and_then(identify_agent);
            match found {
                Some(a) => {
                    if self.agent != Some(a) || self.status == Status::Exited {
                        self.agent = Some(a);
                        self.set(Status::Waiting, now);
                        changed = true;
                    }
                    present = true;
                }
                None if present => {
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
        let mut s = Session { agent: Some(AGENTS[0].1), status: Status::Waiting, since: Instant::now(), checked: None };
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
