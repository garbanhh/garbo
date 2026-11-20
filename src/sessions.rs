//! Sesiones: qué se está ejecutando en cada pestaña.
//!
//! Se averigua qué se ejecuta en primer plano (en Linux, el grupo de procesos del PTY con
//! `tcgetpgrp`; en Windows, lo último que lanzó el shell: ver `platform`) y se identifica si es un
//! agente de código conocido. El estado se deduce de la actividad de la salida:
//!   - Trabajando: el agente ha escrito en la pantalla hace muy poco.
//!   - Terminado: dejó de trabajar mientras no se miraba esa pestaña (pendiente de ver).
//!   - Esperando: inactivo y ya visto, esperando un mensaje.
//!   - Cerrado: el agente salió y la pestaña volvió al shell.
//!
//! Todo se recalcula solo cuando hay salida o mientras algún agente trabaja: en reposo, nada.

use crate::platform::{base_name, Probe};
use crate::status::{Kind, State, Summary};
use crate::subagents::Provider;
use std::time::{Duration, Instant};

/// Sin salida durante este tiempo, el agente se considera parado.
pub const WORK_WINDOW: Duration = Duration::from_millis(1500);
/// Cada cuánto, como mucho, se vuelve a identificar el proceso en primer plano.
const IDENTIFY_EVERY: Duration = Duration::from_millis(700);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Agent {
    /// Nombre que se muestra ("Claude Code").
    pub name: &'static str,
    pub subagents: Option<Provider>,
}

/// Nombre del proceso (o de su script) -> agente.
const AGENTS: &[(&str, &str, Option<Provider>)] = &[
    ("claude", "Claude Code", Some(Provider::Claude)),
    ("codex", "Codex", Some(Provider::Codex)),
    ("gemini", "Gemini CLI", Some(Provider::Gemini)),
    ("aider", "Aider", None),
    ("opencode", "OpenCode", None),
    ("cursor-agent", "Cursor Agent", None),
    ("copilot", "Copilot CLI", None),
    ("qwen", "Qwen Code", None),
    ("goose", "Goose", None),
    ("amp", "Amp", None),
    ("crush", "Crush", None),
];

/// Paquetes de npm cuyo script no se llama como el agente. En Windows npm no crea un enlace con el
/// nombre de la orden (`/usr/bin/claude`) sino un `.cmd` que ejecuta
/// `node …\node_modules\@anthropic-ai\claude-code\cli.js`.
const NPM_PACKAGES: &[(&str, &str)] = &[
    ("node_modules/@anthropic-ai/claude-code/", "claude"),
    ("node_modules/@openai/codex/", "codex"),
    ("node_modules/@google/gemini-cli/", "gemini"),
    ("node_modules/@qwen-code/qwen-code/", "qwen"),
    ("node_modules/@github/copilot/", "copilot"),
    ("node_modules/opencode-ai/", "opencode"),
    ("node_modules/@sourcegraph/amp/", "amp"),
    ("node_modules/@charmland/crush/", "crush"),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Working,
    /// Espera algo del usuario para seguir (solo si el agente lo dice por OSC 7501).
    Blocked(Option<Kind>),
    Done,
    Waiting,
    Exited,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Working => "Trabajando…",
            Status::Blocked(Some(Kind::Permission)) => "Necesita tu permiso",
            Status::Blocked(Some(Kind::Question)) => "Te ha hecho una pregunta",
            Status::Blocked(Some(Kind::Auth)) => "Necesita que inicies sesión",
            Status::Blocked(None) => "Te necesita",
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
    /// Lo que el agente dice que hace o espera (OSC 7501): "Writing hola.txt", "trust this folder".
    pub detail: Option<String>,
    /// Estado que informa el propio agente (OSC 7501); `None`: se deduce de su salida.
    pub reported: Option<State>,
    checked: Option<Instant>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            agent: None,
            status: Status::Waiting,
            since: Instant::now(),
            pid: None,
            detail: None,
            reported: None,
            checked: None,
        }
    }
}

impl Session {
    /// Recalcula el estado. `viewing` = la pestaña está a la vista del usuario. `report`: lo que
    /// informa el propio programa (OSC 7501), que manda sobre la heurística de salida reciente.
    /// Devuelve `true` si cambió algo visible.
    pub fn refresh(
        &mut self,
        probe: Option<&Probe>,
        last_output: Instant,
        viewing: bool,
        now: Instant,
        report: Option<Summary>,
    ) -> bool {
        let identify = self
            .checked
            .is_none_or(|t| now.duration_since(t) >= IDENTIFY_EVERY);
        let mut present = self.agent.is_some() && self.status != Status::Exited;
        let mut changed = false;
        if identify {
            self.checked = Some(now);
            let pgid = probe.and_then(Probe::foreground);
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
                    self.detail = None;
                    self.reported = None;
                    self.set(Status::Exited, now);
                    return true;
                }
                None => {}
            }
        }
        if !present {
            return changed;
        }
        self.reported = report.as_ref().map(|r| r.state);
        let detail = report
            .as_ref()
            .filter(|r| matches!(r.state, State::Working | State::Blocked))
            .and_then(|r| r.msg.clone());
        if detail != self.detail {
            self.detail = detail;
            changed = true;
        }
        // "Terminado" queda hasta que se ve la pestaña; al verla, "Esperando tu mensaje".
        let unseen = |s: Status| {
            !viewing && matches!(s, Status::Working | Status::Blocked(_) | Status::Done)
        };
        let next = match report {
            Some(r) => match r.state {
                State::Working => Status::Working,
                State::Blocked => Status::Blocked(r.kind),
                State::Done | State::Error if unseen(self.status) => Status::Done,
                _ => Status::Waiting,
            },
            None => {
                let working = now.duration_since(last_output) < WORK_WINDOW;
                match (working, self.status) {
                    (true, _) => Status::Working,
                    (false, Status::Working | Status::Done) if !viewing => Status::Done,
                    _ => Status::Waiting,
                }
            }
        };
        if next != self.status {
            self.set(next, now);
            changed = true;
        }
        changed
    }

    /// El agente avisó con la campana (p. ej. al terminar): se marca pendiente de ver.
    pub fn bell(&mut self, viewing: bool, now: Instant) -> bool {
        let settled = matches!(
            self.status,
            Status::Exited | Status::Done | Status::Blocked(_)
        );
        if self.agent.is_some() && !settled && !viewing && self.reported.is_none() {
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

/// Identifica un agente por el nombre del proceso o de su script (node/python/bun…).
pub fn identify_agent(pid: i32) -> Option<Agent> {
    let args = crate::platform::process_args(pid).unwrap_or_default();
    let mut names = vec![crate::platform::process_name(pid).unwrap_or_default()];
    names.extend(
        args.iter()
            .take(3)
            .filter(|a| !a.starts_with('-'))
            .map(|a| base_name(a).to_string()),
    );
    // Script de un paquete de npm (`…/node_modules/@anthropic-ai/claude-code/cli.js`).
    names.extend(args.iter().take(3).filter_map(|a| {
        let a = a.replace('\\', "/").to_lowercase();
        NPM_PACKAGES
            .iter()
            .find(|(pkg, _)| a.contains(pkg))
            .map(|(_, key)| key.to_string())
    }));
    names.iter().find_map(|n| {
        let n = n
            .trim_end_matches(".js")
            .trim_end_matches(".mjs")
            .to_lowercase();
        AGENTS
            .iter()
            .find_map(|&(key, name, subagents)| (n == key).then_some(Agent { name, subagents }))
    })
}

/// Orden que ejecuta un proceso, para mostrarla ("cargo build --release", "vite").
/// Con intérpretes (python, node…) se muestra el script en lugar del intérprete.
pub fn command_label(pid: i32) -> Option<String> {
    let mut args = crate::platform::process_args(pid)?;
    if args.is_empty() {
        return None;
    }
    let interp = [
        "python", "python3", "py", "node", "bun", "deno", "perl", "ruby", "sh", "bash", "zsh",
        "fish",
    ];
    let first = base_name(&args[0]).to_lowercase();
    if args.len() > 1 && interp.contains(&first.as_str()) && !args[1].starts_with('-') {
        args.remove(0);
    }
    args[0] = base_name(&args[0]).to_string();
    let mut out = args.join(" ");
    if out.chars().count() > 60 {
        out = out.chars().take(59).collect::<String>() + "…";
    }
    Some(out)
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
        let mut s = Session {
            agent: Some(Agent {
                name: "Claude Code",
                subagents: Some(Provider::Claude),
            }),
            pid: None,
            ..Default::default()
        };
        let t0 = Instant::now();
        // Evita re-identificar (no hay PTY en el test): se marca como recién comprobado.
        s.checked = Some(t0);
        assert!(s.refresh(None, t0, false, t0, None));
        assert_eq!(s.status, Status::Working);
        // Deja de escribir sin que se mire la pestaña: Terminado (pendiente de ver).
        let t1 = t0 + WORK_WINDOW + Duration::from_millis(200);
        s.checked = Some(t1);
        s.refresh(None, t0, false, t1, None);
        assert_eq!(s.status, Status::Done);
        // Al mirarla pasa a Esperando.
        s.refresh(None, t0, true, t1, None);
        assert_eq!(s.status, Status::Waiting);
    }

    #[test]
    fn textos() {
        assert_eq!(ago(Duration::from_secs(2)), "ahora");
        assert_eq!(ago(Duration::from_secs(27)), "hace 27 s");
        assert_eq!(ago(Duration::from_secs(125)), "hace 2 min");
    }
}
