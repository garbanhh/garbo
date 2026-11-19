//! Adaptadores para los formatos JSONL de Codex y Gemini CLI.

use super::{
    explicit_session, is_uuid, process_env_path, read_tail, resume_args, Provider, SubAgent,
    KEEP_DONE, STALE,
};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub(super) type CodexCache = HashMap<PathBuf, (SystemTime, Option<SubAgent>)>;

#[derive(PartialEq)]
pub(super) enum Location {
    Claude(PathBuf),
    Codex {
        root: PathBuf,
        pid: i32,
        session: String,
    },
    Gemini(PathBuf),
}

impl Location {
    pub(super) fn resolve(provider: Provider, pid: i32) -> Option<Self> {
        match provider {
            Provider::Claude => super::session_dir(pid).map(Self::Claude),
            Provider::Codex => {
                let root = process_env_path(pid, b"CODEX_HOME=")
                    .or_else(|| process_home(pid).map(|home| home.join(".codex")))?
                    .join("sessions")
                    .canonicalize()
                    .ok()?;
                let session = codex_session(&root, pid)?;
                Some(Self::Codex { root, pid, session })
            }
            Provider::Gemini => {
                let args = resume_args(pid)?;
                let session = explicit_session(&args, &["--resume", "-r"])?;
                let root =
                    process_env_path(pid, b"GEMINI_CLI_HOME=").or_else(|| process_home(pid))?;
                gemini_session(&root.join(".gemini/tmp"), &session).map(Self::Gemini)
            }
        }
    }

    pub(super) fn scan(
        &self,
        cache: &mut HashMap<String, (SystemTime, Option<String>)>,
        codex_cache: &mut CodexCache,
    ) -> Vec<SubAgent> {
        match self {
            Self::Claude(dir) => super::scan(dir, cache),
            Self::Codex { root, pid, session } => codex_agents(root, *pid, session, codex_cache),
            Self::Gemini(dir) => gemini_agents(dir),
        }
    }
}

fn process_home(pid: i32) -> Option<PathBuf> {
    process_env_path(pid, b"HOME=").or_else(dirs::home_dir)
}

fn read_header(path: &Path) -> Option<Value> {
    if !fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    // La primera línea de Codex puede incluir las instrucciones base. Nunca se lee el archivo entero.
    let mut reader = BufReader::new(File::open(path).ok()?.take(256 * 1024));
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line).ok()?;
    serde_json::from_slice(&line).ok()
}

fn codex_header(path: &Path) -> Option<Value> {
    let mut header = read_header(path)?;
    if header["type"] != "session_meta" {
        return None;
    }
    header.as_object_mut()?.remove("payload")
}

fn codex_parent(header: &Value) -> Option<&str> {
    header["parent_thread_id"].as_str().or_else(|| {
        header
            .pointer("/source/subagent/thread_spawn/parent_thread_id")?
            .as_str()
    })
}

fn rollout_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
}

fn codex_open_rollouts(root: &Path, pid: i32) -> Vec<PathBuf> {
    // Las instalaciones con npm suelen ejecutar el binario nativo como hijo del lanzador.
    let children =
        fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).unwrap_or_default();
    let pids = std::iter::once(pid).chain(
        children
            .split_whitespace()
            .take(16)
            .filter_map(|s| s.parse().ok()),
    );
    let mut paths: Vec<_> = pids
        .filter_map(|pid| fs::read_dir(format!("/proc/{pid}/fd")).ok())
        .flatten()
        .flatten()
        .filter_map(|fd| fs::read_link(fd.path()).ok())
        .filter(|path| path.starts_with(root) && rollout_path(path))
        .collect();
    paths.sort_unstable();
    paths.dedup();
    paths
}

fn codex_session(root: &Path, pid: i32) -> Option<String> {
    let mut roots = codex_open_rollouts(root, pid)
        .into_iter()
        .filter_map(|path| {
            let header = codex_header(&path)?;
            let id = header["id"].as_str().filter(|id| is_uuid(id))?;
            (codex_parent(&header).is_none() && header["source"].get("subagent").is_none())
                .then(|| id.to_string())
        });
    if let Some(id) = roots.next() {
        return roots.all(|other| other == id).then_some(id);
    }
    let cmdline = resume_args(pid)?;
    explicit_session(&cmdline, &["resume", "--resume"])
}

fn numeric_dirs(parent: &Path, digits: usize) -> Vec<PathBuf> {
    let mut dirs: Vec<_> = entries(parent)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.as_encoded_bytes();
            name.len() == digits && name.iter().all(u8::is_ascii_digit)
        })
        .map(|entry| entry.path())
        .collect();
    dirs.sort_unstable_by(|a, b| b.cmp(a));
    dirs.truncate(2);
    dirs
}

fn entries(dir: &Path) -> impl Iterator<Item = fs::DirEntry> {
    fs::read_dir(dir).into_iter().flatten().flatten()
}

fn codex_agents(root: &Path, pid: i32, parent: &str, cache: &mut CodexCache) -> Vec<SubAgent> {
    let now = SystemTime::now();
    let days = numeric_dirs(root, 4)
        .iter()
        .flat_map(|y| numeric_dirs(y, 2))
        .flat_map(|m| numeric_dirs(&m, 2))
        .take(2)
        .collect::<Vec<_>>();
    let mut paths = codex_open_rollouts(root, pid);
    paths.extend(
        days.iter()
            .flat_map(|day| entries(day))
            .map(|entry| entry.path())
            .filter(|path| rollout_path(path)),
    );
    paths.sort_unstable();
    paths.dedup();
    let mut recent: Vec<_> = paths
        .into_iter()
        .filter_map(|path| {
            let (age, started, modified) = age(&path, now)?;
            (age <= STALE).then_some((modified, started, path))
        })
        .collect();
    recent.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.0));
    recent.truncate(256);
    cache.retain(|path, _| recent.iter().any(|(_, _, candidate)| path == candidate));
    let mut children = Vec::new();
    for (modified, started, path) in recent {
        let entry = cache
            .entry(path.clone())
            .or_insert_with(|| (modified, codex_child(&path, parent, started)));
        if entry.0 != modified {
            *entry = (modified, codex_child(&path, parent, started));
        }
        let Some(child) = &entry.1 else { continue };
        if !child.running && now.duration_since(modified).unwrap_or_default() > KEEP_DONE {
            continue;
        }
        if !children.iter().any(|agent: &SubAgent| agent.id == child.id) {
            children.push(child.clone());
        }
    }
    children.sort_by_key(|agent| agent.started);
    children
}

fn codex_child(path: &Path, parent: &str, started: SystemTime) -> Option<SubAgent> {
    let meta = codex_header(path)?;
    if codex_parent(&meta) != Some(parent) {
        return None;
    }
    let id = meta["id"].as_str().filter(|id| is_uuid(id))?;
    let (running, activity) = codex_status(&read_tail(path)?);
    let spawn = &meta["source"]["subagent"]["thread_spawn"];
    let role = meta["agent_role"]
        .as_str()
        .or_else(|| spawn["agent_role"].as_str())
        .or_else(|| spawn["agent_type"].as_str());
    let name = meta["agent_nickname"]
        .as_str()
        .or_else(|| spawn["agent_nickname"].as_str())
        .or_else(|| spawn["agent_path"].as_str());
    Some(SubAgent {
        id: id.into(),
        kind: role.map_or_else(|| "subagente".into(), compact),
        description: name.map_or_else(String::new, compact),
        running,
        started,
        activity,
    })
}

fn codex_status(tail: &str) -> (bool, Option<String>) {
    let mut activity = None;
    for row in tail
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        let payload = &row["payload"];
        match (row["type"].as_str(), payload["type"].as_str()) {
            (Some("event_msg"), Some("task_complete" | "turn_complete" | "turn_aborted")) => {
                return (false, None)
            }
            (Some("event_msg"), Some("task_started" | "turn_started")) => break,
            (Some("response_item"), Some("function_call" | "custom_tool_call"))
                if activity.is_none() =>
            {
                activity = payload["name"].as_str().map(compact);
            }
            _ => {}
        }
    }
    (true, activity)
}

fn gemini_session(root: &Path, id: &str) -> Option<PathBuf> {
    let mut matches = entries(root)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .map(|project| project.path().join("chats").join(id))
        .filter(|dir| dir.is_dir());
    let path = matches.next()?;
    matches.next().is_none().then_some(path)
}

fn gemini_agents(dir: &Path) -> Vec<SubAgent> {
    let now = SystemTime::now();
    let mut children: Vec<_> = entries(dir)
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                return None;
            }
            let (age, started, _) = age(&path, now)?;
            if age > STALE {
                return None;
            }
            let header = read_header(&path)?;
            if header["kind"] != "subagent" {
                return None;
            }
            let id = header["sessionId"].as_str().filter(|id| is_uuid(id))?;
            let tail = read_tail(&path)?;
            let (running, activity) = gemini_status(&tail);
            (running || age <= KEEP_DONE).then(|| SubAgent {
                id: id.into(),
                kind: "subagente".into(),
                description: id[..8].into(),
                running,
                started,
                activity,
            })
        })
        .collect();
    children.sort_by_key(|agent| agent.started);
    children
}

fn gemini_status(tail: &str) -> (bool, Option<String>) {
    let mut activity = None;
    for row in tail
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        if row
            .pointer("/$set/summary")
            .and_then(Value::as_str)
            .is_some()
        {
            return (false, None);
        }
        if row["type"] == "user" {
            break;
        }
        if let Some(tool) = row["toolCalls"].as_array().and_then(|calls| calls.last()) {
            if tool["name"] == "complete_task" && tool["status"] == "success" {
                return (false, None);
            }
            if activity.is_none() {
                activity = tool["name"].as_str().map(compact);
            }
        }
    }
    (true, activity)
}

fn age(path: &Path, now: SystemTime) -> Option<(std::time::Duration, SystemTime, SystemTime)> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let modified = meta.modified().ok()?;
    Some((
        now.duration_since(modified).unwrap_or_default(),
        meta.created().unwrap_or(modified),
        modified,
    ))
}

fn compact(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    const MAIN: &str = "230899e2-1844-4dde-be21-0099e76c31c5";
    const OTHER: &str = "130899e2-1844-4dde-be21-0099e76c31c5";
    const CHILD: &str = "330899e2-1844-4dde-be21-0099e76c31c5";

    #[test]
    fn codex_vincula_la_pestana_y_relee_la_actividad() {
        let temp = crate::temp_dir::TempDir::new_in(&std::env::temp_dir()).unwrap();
        let day = temp.path().join("2026/11/19");
        fs::create_dir_all(&day).unwrap();
        let main = day.join("rollout-main.jsonl");
        fs::write(
            &main,
            format!(
                "{}\n",
                json!({"type":"session_meta","payload":{"id":MAIN,"source":"cli"}})
            ),
        )
        .unwrap();
        let _opened_by_codex = File::open(&main).unwrap();
        let pid = std::process::id() as i32;
        assert_eq!(codex_session(temp.path(), pid).as_deref(), Some(MAIN));

        for (path, parent, id) in [
            ("rollout-child.jsonl", MAIN, CHILD),
            ("rollout-other.jsonl", OTHER, MAIN),
        ] {
            let meta = json!({"type":"session_meta","payload":{"id":id,"source":{"subagent":{"thread_spawn":{
                "parent_thread_id":parent,"agent_nickname":"Atlas","agent_role":"explorer"
            }}}}});
            let task = json!({"type":"event_msg","payload":{"type":"task_started"}});
            let tool = json!({"type":"response_item","payload":{"type":"function_call","name":"read_file"}});
            fs::write(day.join(path), format!("{meta}\n{task}\n{tool}\n")).unwrap();
        }

        let mut cache = CodexCache::new();
        let agents = codex_agents(temp.path(), pid, MAIN, &mut cache);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, CHILD);
        assert_eq!(agents[0].kind, "explorer");
        assert_eq!(agents[0].description, "Atlas");
        assert!(agents[0].running);
        assert_eq!(agents[0].activity.as_deref(), Some("read_file"));

        let mut child = fs::OpenOptions::new()
            .append(true)
            .open(day.join("rollout-child.jsonl"))
            .unwrap();
        writeln!(
            child,
            "{}",
            json!({"type":"event_msg","payload":{"type":"task_complete"}})
        )
        .unwrap();
        // La caché debe invalidarse aunque el sistema de archivos agrupe las escrituras del test.
        child
            .set_modified(SystemTime::now() + std::time::Duration::from_secs(1))
            .unwrap();
        let agents = codex_agents(temp.path(), pid, MAIN, &mut cache);
        assert!(!agents[0].running);
        assert_eq!(agents[0].activity, None);
    }

    #[test]
    fn gemini_solo_muestra_los_hijos_de_la_sesion_reanudada() {
        let temp = crate::temp_dir::TempDir::new_in(&std::env::temp_dir()).unwrap();
        let chats = temp.path().join("proyecto/chats");
        for parent in [MAIN, OTHER] {
            let dir = chats.join(parent);
            fs::create_dir_all(&dir).unwrap();
            let meta = json!({"kind":"subagent","sessionId":CHILD});
            let tool = json!({"type":"gemini","toolCalls":[{"name":"read_file"}]});
            let summary = json!({"$set":{"summary":"Revisión completada"}});
            let records = if parent == MAIN {
                format!("{meta}\n{tool}\n{summary}\n")
            } else {
                format!("{meta}\n{tool}\n")
            };
            fs::write(dir.join("child.jsonl"), records).unwrap();
        }
        let target = gemini_session(temp.path(), MAIN).unwrap();
        let agents = gemini_agents(&target);
        assert_eq!(target, chats.join(MAIN));
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].description, &CHILD[..8]);
        assert!(!agents[0].running);
        let other = gemini_agents(&chats.join(OTHER));
        assert!(other[0].running);
        assert_eq!(other[0].activity.as_deref(), Some("read_file"));
    }
}
