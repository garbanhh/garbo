//! Subagentes de las sesiones en primer plano. Las fuentes locales se consultan en solo lectura.
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

mod transcripts;
use transcripts::{Location, TranscriptCache};

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
    target: Option<(Provider, i32)>,
    location: Option<Location>,
    resolved: Option<Instant>,
    checked: Option<Instant>,
    /// Actividad ya leída por subagente, con la fecha de modificación de su conversación.
    cache: HashMap<String, (SystemTime, Option<String>)>,
    /// Cabeceras y estado de las sesiones de Codex y Gemini CLI.
    transcripts: TranscriptCache,
    pub list: Vec<SubAgent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Claude,
    Codex,
    Gemini,
}

impl Watch {
    /// Actualiza la lista del agente en primer plano.
    /// Devuelve `true` si cambió algo visible.
    pub fn refresh(&mut self, target: Option<(Provider, i32)>, now: Instant) -> bool {
        let Some((provider, pid)) = target else {
            let had = !self.list.is_empty();
            *self = Self::default();
            return had;
        };
        let changed_target = target != self.target;
        if !changed_target
            && self
                .checked
                .is_some_and(|t| now.duration_since(t) < CHECK_EVERY)
        {
            return false;
        }
        self.checked = Some(now);
        if changed_target
            || self
                .resolved
                .is_none_or(|t| now.duration_since(t) >= RESOLVE_EVERY)
        {
            let location = Location::resolve(provider, pid);
            if location != self.location || changed_target {
                self.cache.clear();
                self.transcripts.clear();
            }
            self.target = target;
            self.location = location;
            self.resolved = Some(now);
        }
        let list = self.location.as_ref().map_or_else(Vec::new, |location| {
            location.scan(&mut self.cache, &mut self.transcripts)
        });
        if list != self.list {
            self.list = list;
            return true;
        }
        false
    }

    /// Fuerza una consulta en el próximo `refresh` (al abrir el panel).
    pub fn invalidate(&mut self) {
        self.checked = None;
        self.resolved = None;
    }
}

/// Cada cuánto se mira si hay un mensaje nuevo para el agente (nombre de la pestaña).
const ASK_EVERY: Duration = Duration::from_secs(2);
/// Lo que se lee como mucho de la conversación de una vez (la primera vez, su final).
const ASK_TAIL: u64 = 4 * 1024 * 1024;

/// Lo último que se le pidió al agente en primer plano, para el nombre de la pestaña ("Claude ·
/// Arregla el menú"). Se mira como mucho cada `ASK_EVERY`, y de la conversación solo se lee lo
/// que se ha añadido desde la última vez (la primera, su final).
#[derive(Default)]
pub struct Request {
    target: Option<(Provider, i32)>,
    path: Option<PathBuf>,
    resolved: Option<Instant>,
    checked: Option<Instant>,
    /// Hasta dónde se ha leído la conversación (byte tras el último salto de línea).
    scanned: u64,
    /// Lo que se le pidió (ver `tabs::task_from_prompt`): el último mensaje que decía algo.
    pub task: Option<String>,
}

impl Request {
    /// Devuelve `true` si cambió la tarea.
    pub fn refresh(&mut self, target: Option<(Provider, i32)>, now: Instant) -> bool {
        let mut changed = false;
        if target != self.target {
            changed = self.task.is_some();
            *self = Self {
                target,
                ..Self::default()
            };
        }
        let Some((provider, pid)) = target else {
            return changed;
        };
        if self
            .checked
            .is_some_and(|t| now.duration_since(t) < ASK_EVERY)
        {
            return changed;
        }
        self.checked = Some(now);
        if self
            .resolved
            .is_none_or(|t| now.duration_since(t) >= RESOLVE_EVERY)
        {
            self.resolved = Some(now);
            let path = transcript_path(provider, pid);
            // Otra conversación (/clear, /resume): lo de antes ya no vale.
            if path != self.path {
                self.path = path;
                self.scanned = 0;
                changed |= self.task.take().is_some();
            }
        }
        let Some(path) = self.path.clone() else {
            return changed;
        };
        if let Some(task) = self.read_new(provider, &path) {
            if self.task.as_ref() != Some(&task) {
                self.task = Some(task);
                changed = true;
            }
        }
        changed
    }

    /// Lee lo nuevo de la conversación (la primera vez, su final) y devuelve la tarea del último
    /// mensaje que diga algo.
    fn read_new(&mut self, provider: Provider, path: &Path) -> Option<String> {
        let len = fs::metadata(path).ok()?.len();
        if len < self.scanned {
            self.scanned = 0;
        }
        let from = self.scanned.max(len.saturating_sub(ASK_TAIL));
        if from >= len {
            return None;
        }
        let mut f = fs::File::open(path).ok()?;
        f.seek(SeekFrom::Start(from)).ok()?;
        let mut buf = Vec::with_capacity((len - from) as usize);
        f.take(len - from).read_to_end(&mut buf).ok()?;
        // Solo líneas completas: la última puede estar a medio escribir y, si se empezó a leer a
        // mitad de una, la primera también.
        let end = buf.iter().rposition(|&b| b == b'\n')? + 1;
        let start = if from > self.scanned {
            buf[..end]
                .iter()
                .position(|&b| b == b'\n')
                .map_or(end, |i| i + 1)
        } else {
            0
        };
        self.scanned = from + end as u64;
        String::from_utf8_lossy(&buf[start..end])
            .lines()
            .rev()
            .filter_map(|line| user_prompt(provider, line))
            .find_map(|p| crate::tabs::task_from_prompt(&p))
    }
}

/// Conversación de la sesión que ejecuta `pid` (Claude Code y Codex; Gemini CLI, no).
fn transcript_path(provider: Provider, pid: i32) -> Option<PathBuf> {
    match provider {
        Provider::Claude => {
            // `<proyectos>/<proyecto>/<sesión>/subagents` -> `<proyectos>/<proyecto>/<sesión>.jsonl`
            let session = session_dir(pid)?.parent()?.to_path_buf();
            let mut name = session.file_name()?.to_os_string();
            name.push(".jsonl");
            Some(session.with_file_name(name)).filter(|p| p.is_file())
        }
        Provider::Codex => match Location::resolve(provider, pid)? {
            Location::Codex { root, pid, session } => {
                transcripts::codex_rollout(&root, pid, &session)
            }
            _ => None,
        },
        Provider::Gemini => None,
    }
}

/// Mensaje del usuario en una línea de la conversación, si lo es. En Claude Code, la entrada
/// `last-prompt` (lo último que se escribió); en Codex, los mensajes del usuario.
fn user_prompt(provider: Provider, line: &str) -> Option<String> {
    match provider {
        Provider::Claude => {
            if !line.contains("\"last-prompt\"") {
                return None;
            }
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            (v["type"] == "last-prompt")
                .then(|| v["lastPrompt"].as_str().map(str::to_string))
                .flatten()
        }
        Provider::Codex => transcripts::codex_user_text(line),
        Provider::Gemini => None,
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
    let cwd = crate::platform::process_cwd(pid)?;
    let slug = project_slug(&cwd.to_string_lossy());
    let found = session_from_registry(pid).or_else(|| session_from_fds(pid));
    let (slug, session) = match found {
        Some(found) => found,
        None => {
            let base = config_dir(pid).join("projects").join(&slug);
            let id = session_from_cmdline(pid).or_else(|| newest_session(&base))?;
            (slug, id)
        }
    };
    Some(
        config_dir(pid)
            .join("projects")
            .join(slug)
            .join(session)
            .join("subagents"),
    )
}

/// Nombre de la carpeta del proyecto: la ruta con todo lo que no es alfanumérico cambiado por '-'.
fn project_slug(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn config_dir(pid: i32) -> PathBuf {
    process_env_path(pid, "CLAUDE_CONFIG_DIR")
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".claude"))
}

fn process_env_path(pid: i32, key: &str) -> Option<PathBuf> {
    crate::platform::process_env(pid, key).map(PathBuf::from)
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

/// Busca entre los archivos abiertos una ruta `…/claude-<uid>/<proyecto>/<sesión>/…`.
/// Sesión actual según Claude Code: `<config>/sessions/<pid>.json` (cambia con /clear y
/// /resume, al contrario que los descriptores abiertos, que pueden seguir siendo los de la sesión
/// anterior). Si el archivo es de otro proceso con el mismo pid (`procStart` distinto), no vale.
fn session_from_registry(pid: i32) -> Option<(String, String)> {
    let path = config_dir(pid).join("sessions").join(format!("{pid}.json"));
    let text = fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let actual = crate::platform::process_start(pid);
    if let (Some(recorded), Some(actual)) = (v["procStart"].as_str(), actual) {
        if recorded != actual {
            return None;
        }
    }
    let id = v["sessionId"].as_str().filter(|id| is_uuid(id))?;
    let cwd = v["cwd"].as_str()?;
    Some((project_slug(cwd), id.to_string()))
}

fn session_from_fds(pid: i32) -> Option<(String, String)> {
    for target in crate::platform::open_files(pid) {
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
    let args = resume_args(pid)?;
    explicit_session(&args, &["--resume", "-r", "--session-id"])
}

fn resume_args(pid: i32) -> Option<Vec<String>> {
    crate::platform::process_args(pid)
}

fn explicit_session(args: &[String], flags: &[&str]) -> Option<String> {
    args.iter().enumerate().find_map(|(i, arg)| {
        let id = if let Some((flag, id)) = arg.split_once('=') {
            flags.contains(&flag).then_some(id)
        } else if flags.contains(&arg.as_str()) {
            args.get(i + 1).map(String::as_str)
        } else {
            None
        }?;
        is_uuid(id).then(|| id.to_string())
    })
}

fn newest_session(base: &Path) -> Option<String> {
    fs::read_dir(base)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let id = name
                .strip_suffix(".jsonl")
                .filter(|id| is_uuid(id))?
                .to_string();
            Some((e.metadata().ok()?.modified().ok()?, id))
        })
        .max()
        .map(|(_, id)| id)
}

fn scan(dir: &Path, cache: &mut HashMap<String, (SystemTime, Option<String>)>) -> Vec<SubAgent> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let now = SystemTime::now();
    let mut list = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(id) = name
            .strip_prefix("agent-")
            .and_then(|n| n.strip_suffix(".jsonl"))
        else {
            continue;
        };
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
    let text = read_tail(path)?;
    text.lines().rev().find_map(activity_of)
}

fn read_tail(path: &Path) -> Option<String> {
    let mut f = fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.take(TAIL).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    Some(if start > 0 {
        text.split_once('\n')?.1.to_owned()
    } else {
        text.into_owned()
    })
}

fn activity_of(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let content = v.pointer("/message/content")?.as_array()?;
    match v.get("type")?.as_str()? {
        "assistant" => {
            let tool = content
                .iter()
                .rev()
                .find(|c| c.get("type").and_then(|t| t.as_str()) == Some("tool_use"));
            Some(match tool {
                Some(t) => describe_tool(t.get("name")?.as_str()?, t.get("input")),
                None => "Pensando…".to_string(),
            })
        }
        // Acaba de recibir el resultado de una herramienta: está pensando el siguiente paso.
        "user"
            if content
                .iter()
                .any(|c| c.get("type").and_then(|t| t.as_str()) == Some("tool_result")) =>
        {
            Some("Pensando…".to_string())
        }
        _ => None,
    }
}

fn describe_tool(name: &str, input: Option<&serde_json::Value>) -> String {
    let field = |k: &str| {
        input
            .and_then(|i| i.get(k))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    // Las rutas de un agente en Windows llevan `\`.
    let base = |p: &str| p.rsplit(['/', '\\']).next().unwrap_or(p).to_string();
    let detail = match name {
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => field("file_path").map(base),
        "Bash" => field("description")
            .or_else(|| field("command"))
            .map(str::to_string),
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

    /// El último mensaje que dice algo, leyendo solo lo nuevo de la conversación; las líneas a
    /// medio escribir se leen cuando se completan.
    #[test]
    fn lo_que_se_le_pidio_al_agente() {
        use std::io::Write;
        let dir = crate::temp_dir::TempDir::new_in(&std::env::temp_dir()).unwrap();
        let path = dir.path().join("sesion.jsonl");
        let prompt = |t: &str| {
            format!(
                "{{\"type\":\"last-prompt\",\"lastPrompt\":{}}}\n",
                serde_json::json!(t)
            )
        };
        let relleno = format!(
            "{{\"type\":\"assistant\",\"x\":\"{}\"}}\n",
            "a".repeat(5000)
        );
        let mut f = fs::File::create(&path).unwrap();
        write!(f, "{}{}", prompt("Arregla el menú de completado"), relleno).unwrap();
        let mut r = Request::default();
        let task = |r: &mut Request| r.read_new(Provider::Claude, &path);
        assert_eq!(
            task(&mut r).as_deref(),
            Some("Arregla el menú de completado")
        );
        // Nada nuevo: no relee.
        assert_eq!(task(&mut r), None);
        // "sí" no dice qué se hace: no cambia nada.
        write!(f, "{}", prompt("sí")).unwrap();
        assert_eq!(task(&mut r), None);
        // Una línea a medias no se lee hasta que se completa.
        let nuevo = prompt("Quiero pestañas a la izquierda");
        let (a, b) = nuevo.split_at(20);
        write!(f, "{a}").unwrap();
        assert_eq!(task(&mut r), None);
        write!(f, "{b}{relleno}").unwrap();
        assert_eq!(
            task(&mut r).as_deref(),
            Some("Quiero pestañas a la izquierda")
        );
        // Codex: sus mensajes, sin los bloques que añade él.
        let codex = r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>x</environment_context>"}]}}"#;
        assert_eq!(user_prompt(Provider::Codex, codex), None);
        let codex = codex.replace(
            "<environment_context>x</environment_context>",
            "revisa el proyecto",
        );
        assert_eq!(
            user_prompt(Provider::Codex, &codex).as_deref(),
            Some("revisa el proyecto")
        );
    }

    /// Con un Claude Code de verdad (`GARBO_CLAUDE_PID=<pid>`): su sesión según
    /// `sessions/<pid>.json` (comprobando `procStart`) y lo último que se le pidió.
    #[test]
    #[ignore = "necesita un Claude Code en marcha (GARBO_CLAUDE_PID)"]
    fn tarea_de_un_claude_de_verdad() {
        let Some(pid) = std::env::var("GARBO_CLAUDE_PID")
            .ok()
            .and_then(|p| p.parse::<i32>().ok())
        else {
            return;
        };
        let start = crate::platform::process_start(pid);
        let found = session_from_registry(pid);
        println!("procStart {start:?}; sesión {found:?}");
        assert!(
            found.is_some(),
            "sessions/{pid}.json con el mismo procStart"
        );
        let t = std::time::Instant::now();
        let mut r = Request::default();
        r.refresh(Some((Provider::Claude, pid)), Instant::now());
        println!("{:?} -> {:?} en {:?}", r.path, r.task, t.elapsed());
        assert!(r.path.is_some());
    }

    #[test]
    fn rutas_y_sesiones() {
        assert_eq!(
            project_slug("/home/ana/Proyectos/mi_app.v2"),
            "-home-ana-Proyectos-mi-app-v2"
        );
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
        let result =
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#;
        assert_eq!(activity_of(result).as_deref(), Some("Pensando…"));
        assert_eq!(activity_of("no es json"), None);
    }

    #[test]
    fn lista_de_subagentes() {
        let dir = std::env::temp_dir().join(format!("garbo-sub-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let running = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Grep","input":{"pattern":"fn main"}}]}}"#;
        fs::write(
            dir.join("agent-a1.jsonl"),
            format!("{{\"type\":\"user\"}}\n{running}\n"),
        )
        .unwrap();
        fs::write(
            dir.join("agent-a1.meta.json"),
            r#"{"agentType":"Explore","description":"Buscar main"}"#,
        )
        .unwrap();
        fs::write(dir.join("agent-a2.jsonl"), "{}\n").unwrap();
        fs::write(
            dir.join("agent-a2.meta.json"),
            r#"{"agentType":"Plan","description":"Planear","finished":true}"#,
        )
        .unwrap();
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
