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

use crate::logos::Logo;
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
    /// Su logo (sin él, se dibuja la inicial).
    pub logo: Option<Logo>,
}

/// Nombre del proceso (o de su script) -> agente.
const AGENTS: &[(&str, &str, Option<Provider>, Option<Logo>)] = &[
    (
        "claude",
        "Claude Code",
        Some(Provider::Claude),
        Some(Logo::ClaudeCode),
    ),
    ("codex", "Codex", Some(Provider::Codex), Some(Logo::Codex)),
    (
        "gemini",
        "Gemini CLI",
        Some(Provider::Gemini),
        Some(Logo::GeminiCli),
    ),
    ("aider", "Aider", None, None),
    ("opencode", "OpenCode", None, Some(Logo::OpenCode)),
    ("cursor-agent", "Cursor Agent", None, Some(Logo::Cursor)),
    ("copilot", "Copilot CLI", None, Some(Logo::Copilot)),
    ("qwen", "Qwen Code", None, Some(Logo::Qwen)),
    ("goose", "Goose", None, Some(Logo::Goose)),
    ("amp", "Amp", None, Some(Logo::Amp)),
    ("crush", "Crush", None, None),
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
    /// En primer plano, una sesión en otro equipo (ssh, mosh…): lo que lee este equipo (carpetas,
    /// git) no vale para lo que se ve.
    pub remote: bool,
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
            remote: false,
            checked: None,
        }
    }
}

impl Session {
    /// Recalcula el estado. `viewing` = la pestaña está a la vista del usuario. `report`: lo que
    /// informa el propio programa (OSC 7501), que manda sobre la heurística de salida reciente.
    /// `running`: el comando en marcha según las marcas del shell; por ssh, el del servidor (con
    /// la integración de Garbo allí), que es como se sabe qué agente corre al otro lado.
    /// Devuelve `true` si cambió algo visible.
    pub fn refresh(
        &mut self,
        probe: Option<&Probe>,
        last_output: Instant,
        viewing: bool,
        now: Instant,
        report: Option<Summary>,
        running: Option<&str>,
    ) -> bool {
        let identify = self
            .checked
            .is_none_or(|t| now.duration_since(t) >= IDENTIFY_EVERY);
        let mut present = self.agent.is_some() && self.status != Status::Exited;
        let mut changed = false;
        if identify {
            self.checked = Some(now);
            let pgid = probe.and_then(Probe::foreground);
            let remote = pgid.is_some_and(is_remote_session);
            if remote != self.remote {
                self.remote = remote;
                changed = true;
            }
            // Por ssh no hay archivos de sesión que leer aquí: sin subagentes.
            let far = || {
                running
                    .filter(|_| remote)
                    .and_then(agent_from_command)
                    .map(|a| Agent {
                        subagents: None,
                        ..a
                    })
            };
            match pgid.and_then(identify_agent).or_else(far) {
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
    names.iter().find_map(|n| agent_by_name(n))
}

/// Agente por el nombre de su programa o de su script (`claude`, `cli.js` no).
fn agent_by_name(n: &str) -> Option<Agent> {
    let n = n
        .trim_end_matches(".js")
        .trim_end_matches(".mjs")
        .to_lowercase();
    AGENTS.iter().find_map(|&(key, name, subagents, logo)| {
        (n == key).then_some(Agent {
            name,
            subagents,
            logo,
        })
    })
}

/// Agente que lanza una línea de órdenes: `claude`, `~/bin/codex --full-auto`,
/// `FOO=1 gemini -p …`, `npx @openai/codex@latest`, `sudo -E aider`… Para lo que corre por ssh,
/// donde no se ve el proceso: solo lo que se escribió.
pub fn agent_from_command(cmd: &str) -> Option<Agent> {
    let mut words = cmd.split_whitespace();
    let mut w = words.next()?;
    loop {
        let skip = w.contains('=') && !w.starts_with('-')
            || w.starts_with('-')
            || matches!(
                w,
                "sudo" | "exec" | "command" | "nohup" | "time" | "env" | "npx" | "bunx" | "pnpx"
            );
        if !skip {
            break;
        }
        w = words.next()?;
    }
    // Paquete de npm: `@openai/codex@latest` → `@openai/codex`.
    let package = match w.rfind('@') {
        Some(i) if i > 0 => &w[..i],
        _ => w,
    };
    let by_package = NPM_PACKAGES.iter().find_map(|(pkg, key)| {
        let pkg = pkg
            .trim_start_matches("node_modules/")
            .trim_end_matches('/');
        (pkg == package).then(|| agent_by_name(key)).flatten()
    });
    by_package.or_else(|| agent_by_name(base_name(w)))
}

/// Programas que abren una sesión en otro equipo: lo que corre al otro lado (un agente, un shell)
/// no se ve desde aquí. `mosh` es el script que conecta; `mosh-client`, la sesión.
const REMOTE: &[&str] = &[
    "ssh",
    "autossh",
    "sshpass",
    "mosh",
    "mosh-client",
    "et",
    "plink",
];

/// ¿Es el proceso un cliente de sesión remota (ssh, mosh…)?
pub fn is_remote_session(pid: i32) -> bool {
    crate::platform::process_name(pid).is_some_and(|n| is_remote_name(&n))
}

fn is_remote_name(name: &str) -> bool {
    let n = name.to_lowercase();
    REMOTE.contains(&n.as_str())
}

/// Orden que ejecuta un proceso, para mostrarla ("cargo build --release", "vite").
/// Con intérpretes (python, node…) se muestra el script en lugar del intérprete.
pub fn command_label(pid: i32) -> Option<String> {
    label_from_args(crate::platform::process_args(pid)?)
}

fn label_from_args(mut args: Vec<String>) -> Option<String> {
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
    // ssh con Garbo en el servidor (ver shell/ssh.sh): lo que escribiste, sin el `-t` que pone ni
    // la orden de arranque.
    if args[0] == "ssh" {
        if let Some(i) = args
            .iter()
            .position(|a| a.starts_with("exec env GARBO_PROMPT="))
        {
            args.truncate(i);
            if args.get(1).is_some_and(|a| a == "-t") {
                args.remove(1);
            }
        }
    }
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
                logo: Some(Logo::ClaudeCode),
            }),
            pid: None,
            ..Default::default()
        };
        let t0 = Instant::now();
        // Evita re-identificar (no hay PTY en el test): se marca como recién comprobado.
        s.checked = Some(t0);
        assert!(s.refresh(None, t0, false, t0, None, None));
        assert_eq!(s.status, Status::Working);
        // Deja de escribir sin que se mire la pestaña: Terminado (pendiente de ver).
        let t1 = t0 + WORK_WINDOW + Duration::from_millis(200);
        s.checked = Some(t1);
        s.refresh(None, t0, false, t1, None, None);
        assert_eq!(s.status, Status::Done);
        // Al mirarla pasa a Esperando.
        s.refresh(None, t0, true, t1, None, None);
        assert_eq!(s.status, Status::Waiting);
    }

    #[test]
    fn agente_por_lo_que_se_escribio() {
        let name = |c: &str| agent_from_command(c).map(|a| a.name);
        assert_eq!(name("claude"), Some("Claude Code"));
        assert_eq!(name("claude --resume abc"), Some("Claude Code"));
        assert_eq!(name("~/.local/bin/codex --full-auto"), Some("Codex"));
        assert_eq!(name("FOO=1 BAR=2 gemini -p hola"), Some("Gemini CLI"));
        assert_eq!(name("npx -y @openai/codex@latest"), Some("Codex"));
        assert_eq!(name("bunx @anthropic-ai/claude-code"), Some("Claude Code"));
        assert_eq!(name("sudo -E aider --model x"), Some("Aider"));
        assert_eq!(name("cursor-agent"), Some("Cursor Agent"));
        for c in [
            "ls -la",
            "vim claude.md",
            "cat codex",
            "",
            "   ",
            "git commit -m claude",
        ] {
            assert_eq!(name(c), None, "{c:?}");
        }
    }

    #[test]
    fn nombre_de_una_orden() {
        let label = |a: &[&str]| label_from_args(a.iter().map(|s| s.to_string()).collect());
        assert_eq!(
            label(&["/usr/bin/python3", "manage.py", "runserver"]).as_deref(),
            Some("manage.py runserver")
        );
        // ssh con Garbo en el servidor: como se escribió.
        assert_eq!(
            label(&[
                "ssh",
                "-t",
                "-p",
                "22",
                "usuario@servidor",
                "exec env GARBO_PROMPT=1 sh -c 'c=$(echo SGVsbG8= | base64 -d)'",
            ])
            .as_deref(),
            Some("ssh -p 22 usuario@servidor")
        );
        assert_eq!(
            label(&["ssh", "-t", "servidor", "top"]).as_deref(),
            Some("ssh -t servidor top")
        );
    }

    #[test]
    fn sesiones_remotas_por_su_nombre() {
        for n in [
            "ssh",
            "mosh-client",
            "autossh",
            "sshpass",
            "et",
            "plink",
            "SSH",
        ] {
            assert!(is_remote_name(n), "{n}");
        }
        for n in ["bash", "sshd", "ssh-agent", "claude", "vim", ""] {
            assert!(!is_remote_name(n), "{n}");
        }
    }

    #[test]
    fn textos() {
        assert_eq!(ago(Duration::from_secs(2)), "ahora");
        assert_eq!(ago(Duration::from_secs(27)), "hace 27 s");
        assert_eq!(ago(Duration::from_secs(125)), "hace 2 min");
    }
}
