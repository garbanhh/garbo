//! Subagentes de Claude Code: los agentes que lanza una sesión (herramienta Agent/Task).
//!
//! Claude Code guarda cada sesión en `~/.claude/projects/<proyecto>/<sesión>.jsonl` y, por cada
//! subagente, `<sesión>/subagents/agent-<id>.jsonl` (su conversación) y `agent-<id>.meta.json`
//! (tipo, descripción y `finished`). Aquí solo se leen esos archivos: nada se modifica.
//!
//! Para saber qué sesión corresponde a la pestaña se mira, por orden:
//!   1. los descriptores abiertos del proceso (`/tmp/claude-<uid>/<proyecto>/<sesión>/…`),
//!   2. `--resume <sesión>` / `--session-id <sesión>` en su línea de órdenes,
//!   3. la conversación modificada más recientemente en la carpeta del proyecto.
//!
//! Solo se consulta con el panel de agentes abierto y como mucho una vez por segundo.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Cada cuánto se vuelve a mirar la carpeta de subagentes.
const CHECK_EVERY: Duration = Duration::from_millis(1000);
/// Cada cuánto se vuelve a averiguar la sesión del proceso (por si se cambia con /resume).
const RESOLVE_EVERY: Duration = Duration::from_secs(10);
/// Un subagente terminado se sigue mostrando este tiempo.
const KEEP_DONE: Duration = Duration::from_secs(120);
/// Un subagente sin terminar pero sin actividad durante este tiempo se da por abandonado.
const STALE: Duration = Duration::from_secs(15 * 60);
/// Bytes del final de la conversación que se leen para saber qué está haciendo.
const TAIL: u64 = 48 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct SubAgent {
    pub id: String,
    /// Tipo de agente ("Explore", "general-purpose"…).
    pub kind: String,
    pub description: String,
    pub running: bool,
    pub started: SystemTime,
    /// Lo último que está haciendo ("Read main.rs", "Pensando…").
    pub activity: Option<String>,
}

#[derive(Default)]
pub struct Watch {
    pid: i32,
    dir: Option<PathBuf>,
    resolved: Option<Instant>,
    checked: Option<Instant>,
    /// Actividad ya leída por subagente, con la fecha de modificación de su conversación.
    cache: HashMap<String, (SystemTime, Option<String>)>,
    pub list: Vec<SubAgent>,
}

impl Watch {
    /// Actualiza la lista para el proceso `pid` (None = no hay Claude Code en primer plano).
    /// Devuelve `true` si cambió algo visible.
    pub fn refresh(&mut self, pid: Option<i32>, now: Instant) -> bool {
        let Some(pid) = pid else {
            let had = !self.list.is_empty();
            *self = Self::default();
            return had;
        };
        if pid == self.pid && self.checked.is_some_and(|t| now.duration_since(t) < CHECK_EVERY) {
            return false;
        }
        self.checked = Some(now);
        if pid != self.pid || self.resolved.is_none_or(|t| now.duration_since(t) >= RESOLVE_EVERY) {
            self.pid = pid;
            self.dir = session_dir(pid);
            self.resolved = Some(now);
        }
        let list = match &self.dir {
            Some(dir) => scan(dir, &mut self.cache),
            None => Vec::new(),
        };
        if list != self.list {
            self.list = list;
            return true;
        }
        false
    }

    /// Fuerza una consulta en el próximo `refresh` (al abrir el panel).
    pub fn invalidate(&mut self) {
        self.checked = None;
    }
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Meta {
    agent_type: Option<String>,
    description: Option<String>,
    finished: Option<bool>,
}

/// Carpeta `subagents` de la sesión que ejecuta el proceso `pid`.
fn session_dir(pid: i32) -> Option<PathBuf> {
    let cwd = fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    let slug = project_slug(&cwd.to_string_lossy());
    let (slug, session) = match session_from_fds(pid) {
        Some(found) => found,
        None => {
            let base = config_dir(pid).join("projects").join(&slug);
            let id = session_from_cmdline(pid).or_else(|| newest_session(&base))?;
            (slug, id)
        }
    };
    Some(config_dir(pid).join("projects").join(slug).join(session).join("subagents"))
}

/// Nombre de la carpeta del proyecto: la ruta con todo lo que no es alfanumérico cambiado por '-'.
fn project_slug(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

fn config_dir(pid: i32) -> PathBuf {
    let environ = fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
    environ
        .split(|&b| b == 0)
        .find_map(|kv| kv.strip_prefix(b"CLAUDE_CONFIG_DIR="))
        .filter(|v| !v.is_empty())
        .map(|v| PathBuf::from(String::from_utf8_lossy(v).into_owned()))
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".claude"))
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == '-' } else { c.is_ascii_hexdigit() })
}

/// Busca entre los archivos abiertos una ruta `…/claude-<uid>/<proyecto>/<sesión>/…`.
fn session_from_fds(pid: i32) -> Option<(String, String)> {
    for e in fs::read_dir(format!("/proc/{pid}/fd")).ok()?.flatten() {
        let Ok(target) = fs::read_link(e.path()) else { continue };
        let t = target.to_string_lossy();
        let parts: Vec<&str> = t.split('/').collect();
        for w in parts.windows(3) {
            if w[0].starts_with("claude-") && w[1].starts_with('-') && is_uuid(w[2]) {
                return Some((w[1].to_string(), w[2].to_string()));
            }
        }
    }
    None
}

fn session_from_cmdline(pid: i32) -> Option<String> {
    let cmd = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args: Vec<String> = cmd.split(|&b| b == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    args.windows(2)
        .find(|w| matches!(w[0].as_str(), "--resume" | "-r" | "--session-id") && is_uuid(&w[1]))
        .map(|w| w[1].clone())
}

fn newest_session(base: &Path) -> Option<String> {
    fs::read_dir(base)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let id = name.strip_suffix(".jsonl").filter(|id| is_uuid(id))?.to_string();
            Some((e.metadata().ok()?.modified().ok()?, id))
        })
        .max()
        .map(|(_, id)| id)
}

fn scan(dir: &Path, cache: &mut HashMap<String, (SystemTime, Option<String>)>) -> Vec<SubAgent> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    let now = SystemTime::now();
    let mut list = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_prefix("agent-").and_then(|n| n.strip_suffix(".jsonl")) else { continue };
        if id.contains("compact") {
            continue;
        }
        let Ok(md) = e.metadata() else { continue };
        let modified = md.modified().unwrap_or(now);
        let started = md.created().unwrap_or(modified);
        let meta: Meta = fs::read(dir.join(format!("agent-{id}.meta.json")))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let running = !meta.finished.unwrap_or(false);
        let idle = now.duration_since(modified).unwrap_or_default();
        if idle > if running { STALE } else { KEEP_DONE } {
            continue;
        }
        let activity = if running {
            match cache.get(id) {
                Some((m, a)) if *m == modified => a.clone(),
                _ => {
                    let a = last_activity(&e.path());
                    cache.insert(id.to_string(), (modified, a.clone()));
                    a
                }
            }
        } else {
            None
        };
        list.push(SubAgent {
            id: id.to_string(),
            kind: meta.agent_type.unwrap_or_else(|| "subagente".into()),
            description: meta.description.unwrap_or_default(),
            running,
            started,
            activity,
        });
    }
    cache.retain(|id, _| list.iter().any(|s| &s.id == id));
    list.sort_by_key(|s| s.started);
    list
}

/// Lo último que hizo el subagente según el final de su conversación.
fn last_activity(path: &Path) -> Option<String> {
    let mut f = fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.lines();
    if start > 0 {
        lines.next(); // primera línea cortada
    }
    let lines: Vec<&str> = lines.collect();
    lines.iter().rev().find_map(|l| activity_of(l))
}

fn activity_of(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let content = v.pointer("/message/content")?.as_array()?;
    match v.get("type")?.as_str()? {
        "assistant" => {
            let tool = content.iter().rev().find(|c| c.get("type").and_then(|t| t.as_str()) == Some("tool_use"));
            Some(match tool {
                Some(t) => describe_tool(t.get("name")?.as_str()?, t.get("input")),
                None => "Pensando…".to_string(),
            })
        }
        // Acaba de recibir el resultado de una herramienta: está pensando el siguiente paso.
        "user" if content.iter().any(|c| c.get("type").and_then(|t| t.as_str()) == Some("tool_result")) => {
            Some("Pensando…".to_string())
        }
        _ => None,
    }
}

fn describe_tool(name: &str, input: Option<&serde_json::Value>) -> String {
    let field = |k: &str| input.and_then(|i| i.get(k)).and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty());
    let base = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    let detail = match name {
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => field("file_path").map(base),
        "Bash" => field("description").or_else(|| field("command")).map(str::to_string),
        "Grep" | "Glob" => field("pattern").map(str::to_string),
        "WebFetch" => field("url").map(|u| u.split('/').nth(2).unwrap_or(u).to_string()),
        "WebSearch" => field("query").map(str::to_string),
        _ => None,
    };
    let detail = detail.map(|d| d.lines().next().unwrap_or_default().to_string());
    match detail {
        Some(d) => format!("{name} {d}"),
        None => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rutas_y_sesiones() {
        assert_eq!(project_slug("/home/ana/Proyectos/mi_app.v2"), "-home-ana-Proyectos-mi-app-v2");
        assert!(is_uuid("230899e2-1844-4dde-be21-0099e76c31c5"));
        assert!(!is_uuid("230899e2-1844-4dde-be21-0099e76c31c"));
        assert!(!is_uuid("tasks"));
    }

    #[test]
    fn actividad_desde_la_conversacion() {
        let tool = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"x"},{"type":"tool_use","name":"Read","input":{"file_path":"/a/b/main.rs"}}]}}"#;
        assert_eq!(activity_of(tool).as_deref(), Some("Read main.rs"));
        let bash = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo build","description":"Compilar"}}]}}"#;
        assert_eq!(activity_of(bash).as_deref(), Some("Bash Compilar"));
        let result = r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#;
        assert_eq!(activity_of(result).as_deref(), Some("Pensando…"));
        assert_eq!(activity_of("no es json"), None);
    }

    #[test]
    fn lista_de_subagentes() {
        let dir = std::env::temp_dir().join(format!("garbo-sub-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let running = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Grep","input":{"pattern":"fn main"}}]}}"#;
        fs::write(dir.join("agent-a1.jsonl"), format!("{{\"type\":\"user\"}}\n{running}\n")).unwrap();
        fs::write(dir.join("agent-a1.meta.json"), r#"{"agentType":"Explore","description":"Buscar main"}"#).unwrap();
        fs::write(dir.join("agent-a2.jsonl"), "{}\n").unwrap();
        fs::write(dir.join("agent-a2.meta.json"), r#"{"agentType":"Plan","description":"Planear","finished":true}"#).unwrap();
        let mut cache = HashMap::new();
        let list = scan(&dir, &mut cache);
        fs::remove_dir_all(&dir).ok();
        assert_eq!(list.len(), 2);
        let a1 = list.iter().find(|s| s.id == "a1").unwrap();
        assert!(a1.running);
        assert_eq!(a1.kind, "Explore");
        assert_eq!(a1.activity.as_deref(), Some("Grep fn main"));
        let a2 = list.iter().find(|s| s.id == "a2").unwrap();
        assert!(!a2.running);
        assert_eq!(a2.activity, None);
    }
}
