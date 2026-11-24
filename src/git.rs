//! Cambios de git sin confirmar: el panel "Cambios" (`changes.rs`) y el indicador junto a la rama
//! en el prompt de Garbo.
//!
//! Todo se lee con la orden `git` en un hilo aparte (`Git::refresh`, `Git::diff`), que duerme
//! mientras no se le pide nada; el resultado llega como `UserEvent::Git`. Se pide:
//! - al aparecer un prompt nuevo en un terminal visible (acaba de terminar un comando);
//! - con el panel abierto, cada poco (más espaciado en repositorios lentos).
//!
//! Se pasa `--no-optional-locks` para no bloquear el índice mientras el usuario usa git. Para no
//! ejecutar programas que diga la configuración del propio repositorio (un `.git/config` que venga
//! en un zip, por ejemplo): `core.fsmonitor=false`, `--no-ext-diff`, `--no-textconv`, sin entrar en
//! los submódulos y, en los filtros que toca el repositorio (`filter.<nombre>.*`), las órdenes de la
//! configuración del usuario o del sistema, o ninguna (`RepoCfg::filters`): el repositorio nunca
//! decide qué se ejecuta. Los del usuario (git-lfs) siguen igual.

use crate::terminal::UserEvent;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use winit::event_loop::EventLoopProxy;

/// Archivos que se listan como mucho.
const MAX_FILES: usize = 2000;
/// Lo más que se lee de un diff y las líneas que se muestran.
const MAX_DIFF_BYTES: u64 = 4 << 20;
const MAX_DIFF_LINES: usize = 20_000;
/// Archivos nuevos: lo más que se lee para contar sus líneas o mostrarlos.
const MAX_NEW_BYTES: u64 = 2 << 20;
/// Caracteres que se guardan de cada línea del diff (el panel no muestra más).
const MAX_LINE_CHARS: usize = 400;
/// Cada cuánto se vuelve a leer la configuración de un repositorio (`RepoCfg`).
const CFG_TTL: Duration = Duration::from_secs(30);

/// Qué le pasó a un archivo respecto al último commit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Modified,
    Added,
    Deleted,
    Renamed,
    /// Nuevo y sin `git add`.
    Untracked,
    /// En conflicto (fusión a medias).
    Conflict,
}

impl Kind {
    /// Letra de la insignia, como en `git status --short`.
    pub fn letter(self) -> &'static str {
        match self {
            Kind::Modified => "M",
            Kind::Added => "A",
            Kind::Deleted => "D",
            Kind::Renamed => "R",
            Kind::Untracked => "U",
            Kind::Conflict => "!",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FileChange {
    /// Ruta desde la raíz del repositorio, con `/`.
    pub path: String,
    pub kind: Kind,
    pub added: u32,
    pub removed: u32,
    pub binary: bool,
    /// Cuándo se modificó el archivo (para volver a leer su diff solo si cambió).
    pub modified: Option<std::time::SystemTime>,
}

/// Cambios sin confirmar de un repositorio (lo preparado y lo no preparado, juntos).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Status {
    /// Rama actual o, sin rama, el commit abreviado.
    pub branch: Option<String>,
    pub files: Vec<FileChange>,
    pub added: u32,
    pub removed: u32,
    /// Archivos que no se listan por pasar de `MAX_FILES`.
    pub more: usize,
}

/// Resumen para el prompt: archivos y líneas añadidas y quitadas.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Summary {
    pub files: usize,
    pub added: u32,
    pub removed: u32,
}

impl Status {
    pub fn summary(&self) -> Option<Summary> {
        let files = self.files.len() + self.more;
        (files > 0).then_some(Summary {
            files,
            added: self.added,
            removed: self.removed,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineKind {
    /// Cabecera de un fragmento (`@@ -12,7 +12,8 @@ fn draw`).
    Hunk,
    Context,
    Added,
    Removed,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Número de línea antes y después (0 si no aplica).
    pub old: u32,
    pub new: u32,
    pub text: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct FileDiff {
    pub lines: Vec<DiffLine>,
    pub binary: bool,
    /// Se recortó por largo.
    pub cut: bool,
}

impl FileDiff {
    /// Primera línea cambiada (en el archivo de ahora), para abrirlo ahí.
    pub fn first_change(&self) -> Option<u32> {
        self.lines
            .iter()
            .find(|l| matches!(l.kind, LineKind::Added | LineKind::Removed))
            .map(|l| l.new.max(1))
    }
}

/// Lo que el hilo de git devuelve.
#[derive(Debug)]
pub enum Msg {
    /// Estado de un repositorio (raíz), con lo que tardó.
    Status(PathBuf, Result<Status, String>, Duration),
    /// Diff de un archivo (raíz, ruta).
    Diff(PathBuf, String, Result<FileDiff, String>),
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Request {
    Status(PathBuf),
    Diff(PathBuf, String, Kind),
}

/// Estado leído de un repositorio y si hay una lectura en marcha.
#[derive(Default)]
pub struct Repo {
    pub status: Option<Result<Status, String>>,
    /// Lo que tardó la última lectura (el panel espacia las siguientes según esto).
    pub took: Duration,
    busy: bool,
    again: bool,
}

/// Lo que se sabe del repositorio de cada terminal.
#[derive(Default, Clone)]
struct TabRepo {
    /// Prompt para el que se miró (se vuelve a mirar con cada prompt nuevo).
    prompt: Option<u32>,
    root: Option<PathBuf>,
}

/// Lado de la interfaz: pide lecturas al hilo y guarda lo leído.
pub struct Git {
    tx: Option<mpsc::Sender<Request>>,
    proxy: EventLoopProxy<UserEvent>,
    pub repos: HashMap<PathBuf, Repo>,
    tabs: HashMap<u64, TabRepo>,
}

impl Git {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        Self {
            tx: None,
            proxy,
            repos: HashMap::new(),
            tabs: HashMap::new(),
        }
    }

    fn send(&mut self, r: Request) {
        if self.tx.is_none() {
            let (tx, rx) = mpsc::channel();
            let proxy = self.proxy.clone();
            let spawned = std::thread::Builder::new()
                .name("git".into())
                .spawn(move || worker(rx, proxy));
            if let Err(e) = spawned {
                log::warn!("No se pudo crear el hilo de git: {e}");
                return;
            }
            self.tx = Some(tx);
        }
        if let Some(tx) = &self.tx {
            if tx.send(r).is_err() {
                self.tx = None;
            }
        }
    }

    /// Pide leer otra vez el estado de `root` (si ya se está leyendo, se repite al acabar).
    pub fn refresh(&mut self, root: &Path) {
        let repo = self.repos.entry(root.to_path_buf()).or_default();
        if repo.busy {
            repo.again = true;
            return;
        }
        repo.busy = true;
        self.send(Request::Status(root.to_path_buf()));
    }

    /// Pide el diff de un archivo.
    pub fn diff(&mut self, root: &Path, path: &str, kind: Kind) {
        self.send(Request::Diff(root.to_path_buf(), path.to_string(), kind));
    }

    /// Llegó un estado. Devuelve `true` si cambió.
    pub fn on_status(
        &mut self,
        root: &Path,
        status: Result<Status, String>,
        took: Duration,
    ) -> bool {
        let repo = self.repos.entry(root.to_path_buf()).or_default();
        repo.busy = false;
        repo.took = took;
        let changed = repo.status.as_ref() != Some(&status);
        if let Err(e) = &status {
            if changed {
                log::info!("git en {}: {e}", root.display());
            }
        }
        repo.status = Some(status);
        if std::mem::take(&mut repo.again) {
            self.refresh(root);
        }
        changed
    }

    pub fn status(&self, root: &Path) -> Option<&Result<Status, String>> {
        self.repos.get(root)?.status.as_ref()
    }

    /// Mira si el terminal `tab` tiene un prompt nuevo (`prompt`); si es así, averigua su
    /// repositorio con `cwd` y pide su estado.
    pub fn track(&mut self, tab: u64, prompt: Option<u32>, cwd: impl FnOnce() -> Option<PathBuf>) {
        let Some(prompt) = prompt else { return };
        let t = self.tabs.entry(tab).or_default();
        if t.prompt == Some(prompt) {
            return;
        }
        t.prompt = Some(prompt);
        t.root = cwd().and_then(|d| repo_root(&d));
        if let Some(root) = t.root.clone() {
            self.refresh(&root);
        }
    }

    /// Repositorio del terminal `tab`; si aún no se sabe, se averigua con `cwd`.
    pub fn root_of(&mut self, tab: u64, cwd: impl FnOnce() -> Option<PathBuf>) -> Option<PathBuf> {
        let t = self.tabs.entry(tab).or_default();
        if t.prompt.is_none() && t.root.is_none() {
            t.root = cwd().and_then(|d| repo_root(&d));
        }
        t.root.clone()
    }

    /// Resumen de los cambios del repositorio del terminal `tab`, si tiene alguno.
    pub fn summary_for(&self, tab: u64) -> Option<Summary> {
        let root = self.tabs.get(&tab)?.root.as_ref()?;
        self.status(root)?.as_ref().ok()?.summary()
    }

    /// Terminales de los que se lleva la cuenta.
    pub fn tracked(&self) -> usize {
        self.tabs.len()
    }

    /// Olvida los terminales que ya no existen y, si hay muchos, los repositorios que nadie usa.
    pub fn forget(&mut self, alive: &[u64], keep: Option<&Path>) {
        self.tabs.retain(|id, _| alive.contains(id));
        if self.repos.len() > 16 {
            let used: Vec<PathBuf> = self.tabs.values().filter_map(|t| t.root.clone()).collect();
            self.repos
                .retain(|r, repo| repo.busy || used.contains(r) || keep == Some(r.as_path()));
        }
    }
}

/// Ruta completa de un archivo del repositorio (`rel` con `/`, como lo da git), con el separador
/// del sistema: en Windows, `C:\repo\src\ui.rs` y no `C:\repo\src/ui.rs` (para el editor).
pub fn file_path(root: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .filter(|c| !c.is_empty())
        .fold(root.to_path_buf(), |p, c| p.join(c))
}

/// Raíz del repositorio (o *worktree*) que contiene `dir`: la carpeta con `.git`.
pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .take(64)
        .find(|d| std::fs::symlink_metadata(d.join(".git")).is_ok())
        .map(Path::to_path_buf)
}

// ---------- Hilo de git ----------

fn worker(rx: mpsc::Receiver<Request>, proxy: EventLoopProxy<UserEvent>) {
    let mut cfgs: HashMap<PathBuf, (Instant, RepoCfg)> = HashMap::new();
    while let Ok(first) = rx.recv() {
        // Lo que se haya acumulado, sin repetir.
        let mut batch = vec![first];
        while let Ok(r) = rx.try_recv() {
            if !batch.contains(&r) {
                batch.push(r);
            }
        }
        for r in batch {
            let msg = match r {
                Request::Status(root) => {
                    let t = Instant::now();
                    let cfg = repo_cfg(&root, &mut cfgs);
                    let res = read_status(&root, &cfg);
                    Msg::Status(root, res, t.elapsed())
                }
                Request::Diff(root, path, kind) => {
                    let cfg = repo_cfg(&root, &mut cfgs);
                    let res = read_diff(&root, &path, kind, &cfg);
                    Msg::Diff(root, path, res)
                }
            };
            if proxy.send_event(UserEvent::Git(msg)).is_err() {
                return;
            }
        }
    }
}

/// Carpeta de *hooks* que no existe. En Windows, `/dev/null` sería `C:\dev\null` (git solo
/// traduce `/dev/null` al abrir archivos, no en las rutas de los hooks); `NUL` no puede ser una
/// carpeta.
const NO_HOOKS: &str = if cfg!(windows) {
    "core.hooksPath=NUL"
} else {
    "core.hooksPath=/dev/null"
};

fn git(root: &Path) -> Command {
    let mut c = Command::new("git");
    crate::platform::no_console(&mut c);
    c.arg("-C")
        .arg(root)
        .args([
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.quotepath=false",
            "-c",
            "color.ui=false",
            "-c",
            NO_HOOKS,
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_COUNT")
        .stdin(Stdio::null());
    c
}

/// `git` para leer el árbol de trabajo de `root`: los filtros que toca el propio repositorio, con
/// las órdenes del usuario (`RepoCfg::filters`). Van por `GIT_CONFIG_KEY_n`, que manda sobre la
/// configuración del repositorio (no con `-c`, que se lía con un `=` en el nombre).
fn git_reading(root: &Path, cfg: &RepoCfg) -> Command {
    let mut c = git(root);
    for (n, (key, value)) in cfg.filters.iter().enumerate() {
        c.env(format!("GIT_CONFIG_KEY_{n}"), key)
            .env(format!("GIT_CONFIG_VALUE_{n}"), value);
    }
    if !cfg.filters.is_empty() {
        c.env("GIT_CONFIG_COUNT", cfg.filters.len().to_string());
    }
    c
}

/// Ejecuta y devuelve la salida; si falla, la primera línea de su error.
fn run(mut c: Command) -> Result<Vec<u8>, String> {
    let out = c.output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "No se encontró la orden git".to_string(),
        _ => format!("No se pudo ejecutar git: {e}"),
    })?;
    if out.status.success() {
        return Ok(out.stdout);
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let line = err
        .lines()
        .map(|l| l.trim_start_matches("fatal: ").trim())
        .find(|l| !l.is_empty())
        .unwrap_or("git falló");
    Err(line.to_string())
}

/// Ejecuta leyendo como mucho `limit` bytes. Devuelve la salida y si se cortó.
fn run_limited(mut c: Command, limit: u64) -> Result<(Vec<u8>, bool), String> {
    let mut child = c
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("No se pudo ejecutar git: {e}"))?;
    let mut out = Vec::new();
    let mut cut = false;
    if let Some(stdout) = child.stdout.take() {
        let _ = stdout.take(limit + 1).read_to_end(&mut out);
        if out.len() as u64 > limit {
            out.truncate(limit as usize);
            cut = true;
            let _ = child.kill();
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !cut && !status.success() {
        return Err("git diff falló".into());
    }
    Ok((out, cut))
}

/// Lo que importa de la configuración de un repositorio.
#[derive(Clone, Debug, PartialEq)]
struct RepoCfg {
    /// `status.showUntrackedFiles`: se respeta "no" (un repositorio de configuración en la
    /// carpeta personal, por ejemplo); si no, se listan todos los archivos nuevos uno a uno.
    untracked: &'static str,
    /// Para cada filtro que nombra la configuración del propio repositorio, sus claves
    /// (`clean`, `smudge`, `process`, `required`) con el valor del usuario o del sistema, o vacías
    /// (`required`, "false"). Así el repositorio no puede cambiar la orden de un filtro, ni siquiera
    /// la de uno que el usuario tiene (como `lfs`).
    filters: Vec<(String, String)>,
}

/// Claves de un filtro que pueden ejecutar algo o hacerlo obligatorio.
const FILTER_KEYS: [&str; 4] = ["clean", "smudge", "process", "required"];

/// Configuración de `root`, leída como mucho cada `CFG_TTL`.
fn repo_cfg(root: &Path, cache: &mut HashMap<PathBuf, (Instant, RepoCfg)>) -> RepoCfg {
    if let Some((at, cfg)) = cache.get(root) {
        if at.elapsed() < CFG_TTL {
            return cfg.clone();
        }
    }
    let mut c = git(root);
    c.args([
        "config",
        "--show-scope",
        "-z",
        "--get-regexp",
        r"^status\.showuntrackedfiles$|^filter\..*\.(clean|smudge|process|required)$",
    ]);
    // Sin coincidencias, git sale con 1: nada que respetar ni que fijar.
    let cfg = parse_cfg(&run(c).unwrap_or_default());
    if !cfg.filters.is_empty() {
        log::info!(
            "git en {}: filtros del repositorio con las órdenes del usuario: {:?}",
            root.display(),
            cfg.filters
        );
    }
    cache.insert(root.to_path_buf(), (Instant::now(), cfg.clone()));
    cfg
}

/// `git config --show-scope -z --get-regexp …`: ámbito, NUL, clave, salto de línea, valor, NUL.
fn parse_cfg(out: &[u8]) -> RepoCfg {
    let mut untracked = "all";
    let mut repo = std::collections::BTreeSet::new();
    // (filtro, clave) → valor del usuario o del sistema (el último manda, como en git).
    let mut user: HashMap<(String, String), String> = HashMap::new();
    let mut it = out.split(|&b| b == 0);
    while let (Some(scope), Some(entry)) = (it.next(), it.next()) {
        let entry = String::from_utf8_lossy(entry);
        let (key, value) = entry.split_once('\n').unwrap_or((&entry, ""));
        if key == "status.showuntrackedfiles" {
            let v = value.trim().to_lowercase();
            untracked = if matches!(v.as_str(), "no" | "false" | "0" | "off") {
                "no"
            } else {
                "all"
            };
        } else if let Some((name, k)) = key.strip_prefix("filter.").and_then(|r| r.rsplit_once('.'))
        {
            match scope {
                b"global" | b"system" => {
                    user.insert((name.to_string(), k.to_string()), value.to_string());
                }
                // El repositorio (o cualquier otro sitio que no sea el usuario o el sistema).
                _ => {
                    repo.insert(name.to_string());
                }
            }
        }
    }
    let mut filters = Vec::new();
    for name in &repo {
        for k in FILTER_KEYS {
            let none = if k == "required" { "false" } else { "" };
            let v = user
                .get(&(name.clone(), k.to_string()))
                .map_or(none.to_string(), Clone::clone);
            filters.push((format!("filter.{name}.{k}"), v));
        }
    }
    RepoCfg { untracked, filters }
}

fn read_status(root: &Path, cfg: &RepoCfg) -> Result<Status, String> {
    let mut c = git_reading(root, cfg);
    c.args([
        "status",
        "--porcelain=v2",
        "-z",
        "--branch",
        "--no-renames",
        "--ignore-submodules=dirty",
    ])
    .arg(format!("--untracked-files={}", cfg.untracked));
    let out = run(c)?;
    let parsed = parse_status(&out);

    // Líneas añadidas y quitadas de lo que git sigue, respecto al último commit (o a nada, si
    // aún no hay ninguno).
    let base = if parsed.initial {
        empty_tree(root)?
    } else {
        "HEAD".to_string()
    };
    let mut counts = HashMap::new();
    if parsed.files.iter().any(|(_, k)| *k != Kind::Untracked) {
        let mut c = git_reading(root, cfg);
        c.args([
            "diff",
            "--numstat",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=dirty",
            &base,
            "--",
        ]);
        counts = parse_numstat(&run(c)?);
    }

    let mut files: Vec<FileChange> = parsed
        .files
        .into_iter()
        .map(|(path, kind)| {
            let (added, removed, binary) = if kind == Kind::Untracked {
                match read_new(&root.join(&path)) {
                    Some(New::Text(lines)) => (lines.len() as u32, 0, false),
                    Some(New::Binary) => (0, 0, true),
                    None => (0, 0, false),
                }
            } else {
                counts.get(&path).copied().unwrap_or((0, 0, false))
            };
            let modified = std::fs::symlink_metadata(root.join(&path))
                .and_then(|m| m.modified())
                .ok();
            FileChange {
                path,
                kind,
                added,
                removed,
                binary,
                modified,
            }
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let more = files.len().saturating_sub(MAX_FILES);
    files.truncate(MAX_FILES);
    Ok(Status {
        added: files.iter().map(|f| f.added).sum(),
        removed: files.iter().map(|f| f.removed).sum(),
        branch: parsed.branch,
        files,
        more,
    })
}

fn read_diff(root: &Path, path: &str, kind: Kind, cfg: &RepoCfg) -> Result<FileDiff, String> {
    if kind == Kind::Untracked {
        return match read_new(&root.join(path)) {
            Some(New::Text(lines)) => Ok(new_file_diff(lines)),
            Some(New::Binary) => Ok(FileDiff {
                binary: true,
                ..Default::default()
            }),
            None => Err("No se pudo leer el archivo".into()),
        };
    }
    let base = if has_head(root) {
        "HEAD".to_string()
    } else {
        empty_tree(root)?
    };
    let mut c = git_reading(root, cfg);
    c.args([
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--ignore-submodules=dirty",
        "-U3",
        &base,
        "--",
        path,
    ]);
    let (out, cut) = run_limited(c, MAX_DIFF_BYTES)?;
    let mut d = parse_diff(&out);
    d.cut |= cut;
    Ok(d)
}

fn has_head(root: &Path) -> bool {
    let mut c = git(root);
    c.args(["rev-parse", "-q", "--verify", "HEAD"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    c.status().is_ok_and(|s| s.success())
}

/// El árbol vacío (con lo que se compara antes del primer commit), en el formato del repositorio.
fn empty_tree(root: &Path) -> Result<String, String> {
    let mut c = git(root);
    c.args(["hash-object", "--no-filters", "-t", "tree", "/dev/null"]);
    Ok(String::from_utf8_lossy(&run(c)?).trim().to_string())
}

/// Contenido de un archivo nuevo: sus líneas o que es binario. `None` si no se puede leer.
enum New {
    Text(Vec<String>),
    Binary,
}

fn read_new(path: &Path) -> Option<New> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() {
        // Como lo guarda git: una línea con el destino.
        let target = std::fs::read_link(path).ok()?;
        return Some(New::Text(vec![target.to_string_lossy().into_owned()]));
    }
    if !meta.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_NEW_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return Some(New::Binary);
    }
    let text = String::from_utf8_lossy(&bytes);
    Some(New::Text(text.lines().map(clean).collect()))
}

fn new_file_diff(lines: Vec<String>) -> FileDiff {
    let n = lines.len();
    let cut = n > MAX_DIFF_LINES;
    let mut out = Vec::with_capacity(n.min(MAX_DIFF_LINES) + 1);
    if n > 0 {
        out.push(DiffLine {
            kind: LineKind::Hunk,
            old: 0,
            new: 0,
            text: format!("@@ -0,0 +1,{n} @@"),
        });
    }
    out.extend(
        lines
            .into_iter()
            .take(MAX_DIFF_LINES)
            .enumerate()
            .map(|(i, text)| DiffLine {
                kind: LineKind::Added,
                old: 0,
                new: i as u32 + 1,
                text,
            }),
    );
    FileDiff {
        lines: out,
        binary: false,
        cut,
    }
}

/// Lo que se saca de `git status --porcelain=v2 -z --branch`.
#[derive(Debug, Default)]
struct Parsed {
    branch: Option<String>,
    /// Aún no hay ningún commit.
    initial: bool,
    files: Vec<(String, Kind)>,
}

fn parse_status(out: &[u8]) -> Parsed {
    let mut p = Parsed::default();
    let mut oid = None;
    let mut head = None;
    let mut records = out.split(|&b| b == 0);
    while let Some(rec) = records.next() {
        let rec = String::from_utf8_lossy(rec);
        if let Some(h) = rec.strip_prefix("# branch.head ") {
            head = Some(h.to_string());
        } else if let Some(o) = rec.strip_prefix("# branch.oid ") {
            if o == "(initial)" {
                p.initial = true;
            } else {
                oid = o.get(..7).map(str::to_string);
            }
        } else if let Some(rest) = rec.strip_prefix("1 ") {
            // XY sub mH mI mW hH hI ruta
            let f: Vec<&str> = rest.splitn(8, ' ').collect();
            let (Some(xy), Some(path)) = (f.first(), f.get(7)) else {
                continue;
            };
            let xy = xy.as_bytes();
            let (x, y) = (xy.first().copied(), xy.get(1).copied());
            let kind = match (x, y) {
                (Some(b'A'), Some(b'D')) => continue, // añadido y borrado: nada respecto al commit
                (Some(b'A'), _) => Kind::Added,
                (Some(b'D'), _) | (_, Some(b'D')) => Kind::Deleted,
                _ => Kind::Modified,
            };
            p.files.push((path.to_string(), kind));
        } else if let Some(rest) = rec.strip_prefix("2 ") {
            // XY sub mH mI mW hH hI Xpuntuación ruta, y la ruta de antes en el siguiente registro
            let f: Vec<&str> = rest.splitn(9, ' ').collect();
            if let Some(path) = f.get(8) {
                p.files.push((path.to_string(), Kind::Renamed));
            }
            records.next();
        } else if let Some(rest) = rec.strip_prefix("u ") {
            // XY sub m1 m2 m3 mW h1 h2 h3 ruta
            let f: Vec<&str> = rest.splitn(10, ' ').collect();
            if let Some(path) = f.get(9) {
                p.files.push((path.to_string(), Kind::Conflict));
            }
        } else if let Some(path) = rec.strip_prefix("? ") {
            p.files.push((path.to_string(), Kind::Untracked));
        }
    }
    p.branch = match head.as_deref() {
        Some("(detached)") | None => oid,
        Some(h) => Some(h.to_string()),
    };
    p
}

/// `git diff --numstat -z`: ruta → (añadidas, quitadas, binario).
fn parse_numstat(out: &[u8]) -> HashMap<String, (u32, u32, bool)> {
    let mut m = HashMap::new();
    for rec in out.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        let rec = String::from_utf8_lossy(rec);
        let mut f = rec.splitn(3, '\t');
        let (Some(a), Some(r), Some(path)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let binary = a == "-";
        let (a, r) = (a.parse().unwrap_or(0), r.parse().unwrap_or(0));
        let e = m.entry(path.to_string()).or_insert((0, 0, false));
        // Un archivo que pasa a enlace (o al revés) sale dos veces.
        e.0 += a;
        e.1 += r;
        e.2 |= binary;
    }
    m
}

/// Diff unificado de un archivo → sus líneas con número.
fn parse_diff(out: &[u8]) -> FileDiff {
    let mut d = FileDiff::default();
    let text = String::from_utf8_lossy(out);
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    for raw in text.lines() {
        if d.lines.len() >= MAX_DIFF_LINES {
            d.cut = true;
            break;
        }
        if raw.starts_with("diff --git ") {
            in_hunk = false;
            continue;
        }
        if raw.starts_with("Binary files ") || raw.starts_with("GIT binary patch") {
            d.binary = true;
            continue;
        }
        if let Some(rest) = raw.strip_prefix("@@ ") {
            if let Some((o, n)) = hunk_starts(rest) {
                (old, new) = (o, n);
                in_hunk = true;
                d.lines.push(DiffLine {
                    kind: LineKind::Hunk,
                    old: 0,
                    new: 0,
                    text: clean(raw),
                });
                continue;
            }
        }
        if !in_hunk {
            continue;
        }
        let (kind, rest) = match raw.as_bytes().first() {
            Some(b'+') => (LineKind::Added, &raw[1..]),
            Some(b'-') => (LineKind::Removed, &raw[1..]),
            Some(b' ') => (LineKind::Context, &raw[1..]),
            // "\ No newline at end of file"
            Some(b'\\') => continue,
            None => (LineKind::Context, ""),
            _ => {
                in_hunk = false;
                continue;
            }
        };
        let (o, n) = match kind {
            LineKind::Added => (0, new),
            LineKind::Removed => (old, new),
            _ => (old, new),
        };
        match kind {
            LineKind::Added => new += 1,
            LineKind::Removed => old += 1,
            _ => {
                old += 1;
                new += 1;
            }
        }
        d.lines.push(DiffLine {
            kind,
            old: o,
            new: n,
            text: clean(rest),
        });
    }
    d
}

/// "-12,7 +12,8 @@ …" → (12, 12).
fn hunk_starts(rest: &str) -> Option<(u32, u32)> {
    let mut parts = rest.split(' ');
    let o = parts.next()?.strip_prefix('-')?;
    let n = parts.next()?.strip_prefix('+')?;
    let start = |s: &str| s.split(',').next()?.parse::<u32>().ok();
    Some((start(o)?, start(n)?))
}

/// Línea para dibujar: tabuladores a 4 espacios, sin caracteres de control y recortada.
fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(MAX_LINE_CHARS));
    let mut n = 0;
    for c in s.chars() {
        if n >= MAX_LINE_CHARS {
            break;
        }
        match c {
            '\t' => {
                let k = 4 - n % 4;
                out.extend(std::iter::repeat_n(' ', k));
                n += k;
            }
            c if c.is_control() => {}
            c => {
                out.push(c);
                n += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estado_de_git_status() {
        let out = b"# branch.oid 1234567890abcdef\0# branch.head main\0\
1 .M N... 100644 100644 100644 aaa bbb src/ui.rs\0\
1 A. N... 000000 100644 100644 000 ccc nuevo con espacio.txt\0\
1 D. N... 100644 000000 000000 ddd 000 viejo.rs\0\
1 AD N... 000000 100644 000000 000 eee fantasma.rs\0\
2 R. N... 100644 100644 100644 fff fff R100 dest.rs\0orig.rs\0\
u UU N... 100644 100644 100644 100644 a b c conflicto.rs\0\
? sin seguir/a.txt\0";
        let p = parse_status(out);
        assert_eq!(p.branch.as_deref(), Some("main"));
        assert!(!p.initial);
        assert_eq!(
            p.files,
            [
                ("src/ui.rs".to_string(), Kind::Modified),
                ("nuevo con espacio.txt".to_string(), Kind::Added),
                ("viejo.rs".to_string(), Kind::Deleted),
                ("dest.rs".to_string(), Kind::Renamed),
                ("conflicto.rs".to_string(), Kind::Conflict),
                ("sin seguir/a.txt".to_string(), Kind::Untracked),
            ]
        );
        // Sin rama: el commit abreviado; sin commits: `initial`.
        let p = parse_status(b"# branch.oid abcdef1234\0# branch.head (detached)\0");
        assert_eq!(p.branch.as_deref(), Some("abcdef1"));
        let p = parse_status(b"# branch.oid (initial)\0# branch.head main\0");
        assert!(p.initial);
        assert_eq!(p.branch.as_deref(), Some("main"));
    }

    #[test]
    fn lineas_de_numstat() {
        let m = parse_numstat(b"3\t1\tsrc/ui.rs\0-\t-\tlogo.png\x0010\t0\tcon\ttab.txt\0");
        assert_eq!(m["src/ui.rs"], (3, 1, false));
        assert_eq!(m["logo.png"], (0, 0, true));
        assert_eq!(m["con\ttab.txt"], (10, 0, false));
    }

    #[test]
    fn diff_con_numeros_de_linea() {
        let out = b"diff --git a/x.rs b/x.rs\nindex 1..2 100644\n--- a/x.rs\n+++ b/x.rs\n\
@@ -10,4 +10,5 @@ fn draw() {\n uno\n-dos\n+DOS\n+\tdos y medio\n tres\n\\ No newline at end of file\n";
        let d = parse_diff(out);
        assert!(!d.binary && !d.cut);
        let got: Vec<(LineKind, u32, u32, &str)> = d
            .lines
            .iter()
            .map(|l| (l.kind, l.old, l.new, l.text.as_str()))
            .collect();
        use LineKind::*;
        assert_eq!(
            got,
            [
                (Hunk, 0, 0, "@@ -10,4 +10,5 @@ fn draw() {"),
                (Context, 10, 10, "uno"),
                (Removed, 11, 11, "dos"),
                (Added, 0, 11, "DOS"),
                (Added, 0, 12, "    dos y medio"),
                (Context, 12, 13, "tres"),
            ]
        );
        assert_eq!(d.first_change(), Some(11));
        let bin =
            parse_diff(b"diff --git a/l.png b/l.png\nBinary files a/l.png and b/l.png differ\n");
        assert!(bin.binary && bin.lines.is_empty());
    }

    #[test]
    fn archivo_nuevo_como_diff() {
        let d = new_file_diff(vec!["a".into(), "b".into()]);
        assert_eq!(d.lines.len(), 3);
        assert_eq!(d.lines[0].text, "@@ -0,0 +1,2 @@");
        assert_eq!((d.lines[2].kind, d.lines[2].new), (LineKind::Added, 2));
        assert_eq!(d.first_change(), Some(1));
    }

    #[test]
    fn ruta_con_el_separador_del_sistema() {
        let root = Path::new("repo");
        assert_eq!(
            file_path(root, "src/ui.rs"),
            Path::new("repo").join("src").join("ui.rs")
        );
        if cfg!(windows) {
            assert_eq!(
                file_path(Path::new(r"C:\repo"), "src/ui.rs").to_string_lossy(),
                r"C:\repo\src\ui.rs"
            );
        }
    }

    #[test]
    fn limpiar_lineas() {
        assert_eq!(clean("a\tb"), "a   b");
        assert_eq!(clean("\x1b[31mrojo"), "[31mrojo");
        assert_eq!(clean(&"x".repeat(1000)).len(), MAX_LINE_CHARS);
    }

    /// Con un repositorio de verdad (si hay git): cambios, archivo nuevo y diff.
    #[test]
    fn repositorio_de_verdad() {
        let dir = std::env::temp_dir().join(format!("garbo-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let sh = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .is_ok_and(|o| o.status.success())
        };
        if !sh(&["init", "-q"]) {
            return; // sin git
        }
        std::fs::write(dir.join("src/a.rs"), "uno\ndos\ntres\n").unwrap();
        // Sin commits: lo preparado cuenta como añadido.
        assert!(sh(&["add", "."]));
        let cfg = repo_cfg(&dir, &mut HashMap::new());
        let s = read_status(&dir, &cfg).unwrap();
        assert_eq!(s.files.len(), 1);
        assert_eq!((s.files[0].kind, s.files[0].added), (Kind::Added, 3));
        assert_eq!(
            read_diff(&dir, "src/a.rs", Kind::Added, &cfg)
                .unwrap()
                .lines
                .len(),
            4
        );
        assert!(sh(&["commit", "-q", "-m", "x"]));

        std::fs::write(dir.join("src/a.rs"), "uno\nDOS\ntres\ncuatro\n").unwrap();
        std::fs::write(dir.join("nuevo.txt"), "hola\n").unwrap();
        let s = read_status(&dir, &cfg).unwrap();
        let got: Vec<(&str, Kind, u32, u32)> = s
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.kind, f.added, f.removed))
            .collect();
        assert_eq!(
            got,
            [
                ("nuevo.txt", Kind::Untracked, 1, 0),
                ("src/a.rs", Kind::Modified, 2, 1),
            ]
        );
        assert_eq!((s.added, s.removed), (3, 1));
        assert_eq!(repo_root(&dir.join("src")).as_deref(), Some(dir.as_path()));
        let d = read_diff(&dir, "src/a.rs", Kind::Modified, &cfg).unwrap();
        assert_eq!(d.first_change(), Some(2));
        assert_eq!(
            d.lines.iter().filter(|l| l.kind == LineKind::Added).count(),
            2
        );
        let n = read_diff(&dir, "nuevo.txt", Kind::Untracked, &cfg).unwrap();
        assert_eq!(n.lines.last().map(|l| l.text.as_str()), Some("hola"));

        // Un filtro definido en el propio repositorio (como vendría en un zip) no se ejecuta.
        let mark = dir.join("EJECUTADO");
        let filter = format!("touch '{}'; cat", mark.display());
        assert!(sh(&["config", "filter.trampa.clean", &filter]));
        assert!(sh(&["config", "filter.trampa.required", "true"]));
        std::fs::write(dir.join(".gitattributes"), "*.rs filter=trampa\n").unwrap();
        std::fs::write(dir.join("src/a.rs"), "uno\nDOS\ntres\ncinco\n").unwrap();
        // Con git a secas, sí (la prueba tiene sentido)…
        let mut c = git(&dir);
        c.args(["diff", "--numstat", "HEAD", "--"]);
        let _ = run(c);
        assert!(mark.exists());
        std::fs::remove_file(&mark).unwrap();
        // …y leyendo los cambios, no.
        let cfg = repo_cfg(&dir, &mut HashMap::new());
        assert!(cfg
            .filters
            .contains(&("filter.trampa.clean".to_string(), String::new())));
        let s = read_status(&dir, &cfg).unwrap();
        assert!(s.files.iter().any(|f| f.path == "src/a.rs"));
        read_diff(&dir, "src/a.rs", Kind::Modified, &cfg).unwrap();
        assert!(!mark.exists(), "se ejecutó el filtro del repositorio");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Banco de medida: lo que tarda leer el estado de este repositorio (configuración, `status` y
    /// `numstat`), la primera vez y las siguientes.
    #[test]
    #[ignore = "banco de medida"]
    fn bench_status() {
        let root = repo_root(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let mut cache = HashMap::new();
        for i in 0..6 {
            let t = Instant::now();
            let cfg = repo_cfg(&root, &mut cache);
            let s = read_status(&root, &cfg).unwrap();
            println!("{i}: {:?} ({} archivos)", t.elapsed(), s.files.len());
        }
    }

    #[test]
    fn configuracion_del_repositorio() {
        // Como lo da git: primero el sistema y el usuario, luego el repositorio.
        let out = b"global\0filter.lfs.clean\ngit-lfs clean -- %f\0\
global\0filter.lfs.process\ngit-lfs filter-process\0global\0filter.lfs.required\ntrue\0\
local\0filter.lfs.clean\nmalo\0local\0filter.lfs.process\nmalo\0\
local\0filter.mi filtro=raro.process\nx\0worktree\0filter.otro.clean\ny\0\
local\0status.showuntrackedfiles\nno\0";
        let cfg = parse_cfg(out);
        assert_eq!(cfg.untracked, "no");
        let f = |k: &str, v: &str| (k.to_string(), v.to_string());
        assert_eq!(
            cfg.filters,
            [
                // git-lfs: lo del usuario, aunque el repositorio lo cambie.
                f("filter.lfs.clean", "git-lfs clean -- %f"),
                f("filter.lfs.smudge", ""),
                f("filter.lfs.process", "git-lfs filter-process"),
                f("filter.lfs.required", "true"),
                // Los que solo define el repositorio: ninguna orden y no obligatorios.
                f("filter.mi filtro=raro.clean", ""),
                f("filter.mi filtro=raro.smudge", ""),
                f("filter.mi filtro=raro.process", ""),
                f("filter.mi filtro=raro.required", "false"),
                f("filter.otro.clean", ""),
                f("filter.otro.smudge", ""),
                f("filter.otro.process", ""),
                f("filter.otro.required", "false"),
            ]
        );
        // Solo el usuario: nada que fijar.
        let mine = b"global\0filter.lfs.clean\ngit-lfs clean -- %f\0";
        assert!(parse_cfg(mine).filters.is_empty());
        assert_eq!(
            parse_cfg(b""),
            RepoCfg {
                untracked: "all",
                filters: Vec::new()
            }
        );
    }
}
