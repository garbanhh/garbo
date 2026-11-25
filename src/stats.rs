//! Estadísticas de uso de los agentes: tokens, peticiones y tiempo de trabajo, leídos de los
//! archivos de sesión que guardan Claude Code, Codex y Gemini CLI en este equipo.
//!
//! Un hilo (solo mientras el apartado está abierto) recorre esos archivos y resume cada uno por
//! horas (UTC). Claude Code y Codex solo añaden líneas a sus JSONL: de cada archivo se recuerda
//! hasta dónde se leyó y la siguiente vez se sigue desde ahí. El resumen se guarda en
//! `~/.cache/garbo/estadisticas.json`, así que abrir el apartado otra vez es inmediato y el
//! historial se conserva aunque los agentes borren sus sesiones antiguas. El hilo principal
//! convierte las horas a días locales y calcula lo que se dibuja (`summarize`).

mod parse;
pub mod view;

use crate::terminal::UserEvent;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use winit::event_loop::EventLoopProxy;

/// Cambia cuando cambia el formato de la caché o la forma de contar: la caché vieja se descarta
/// y se vuelve a leer todo.
const CACHE_VERSION: u32 = 1;

/// Con el apartado abierto, cada cuánto se buscan sesiones nuevas o que hayan crecido.
const REFRESH: Duration = Duration::from_secs(30);

/// Agentes de los que se leen estadísticas.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum AgentKind {
    #[default]
    Claude,
    Codex,
    Gemini,
}

impl AgentKind {
    pub fn name(self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude Code",
            AgentKind::Codex => "Codex",
            AgentKind::Gemini => "Gemini CLI",
        }
    }

    pub fn logo(self) -> crate::logos::Logo {
        use crate::logos::Logo;
        match self {
            AgentKind::Claude => Logo::ClaudeCode,
            AgentKind::Codex => Logo::Codex,
            AgentKind::Gemini => Logo::GeminiCli,
        }
    }

    pub fn provider(self) -> Provider {
        match self {
            AgentKind::Claude => Provider::Anthropic,
            AgentKind::Codex => Provider::OpenAi,
            AgentKind::Gemini => Provider::Google,
        }
    }
}

/// Empresa detrás de un modelo (para la barra de reparto y los colores).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Provider {
    Anthropic,
    OpenAi,
    Google,
    Other,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic",
            Provider::OpenAi => "OpenAI",
            Provider::Google => "Google",
            Provider::Other => "Otros",
        }
    }

    fn of_model(model: &str) -> Self {
        let m = model.to_ascii_lowercase();
        let has = |w: &str| m.contains(w);
        if has("claude") || has("opus") || has("sonnet") || has("haiku") || has("fable") {
            Provider::Anthropic
        } else if has("gemini") || has("gemma") {
            Provider::Google
        } else if has("gpt")
            || has("codex")
            || (m.starts_with('o') && m[1..].starts_with(|c: char| c.is_ascii_digit()))
        {
            Provider::OpenAi
        } else {
            Provider::Other
        }
    }
}

/// Tokens de una hora con un modelo.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Entrada nueva (ni leída ni escrita en la caché).
    #[serde(rename = "i")]
    pub input: u64,
    #[serde(rename = "cr")]
    pub cache_read: u64,
    #[serde(rename = "cw")]
    pub cache_write: u64,
    #[serde(rename = "o")]
    pub output: u64,
    /// Respuestas del modelo.
    #[serde(rename = "n")]
    pub calls: u32,
}

impl Usage {
    pub fn add(&mut self, o: &Usage) {
        self.input += o.input;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
        self.output += o.output;
        self.calls += o.calls;
    }

    /// Toda la entrada que procesó el modelo, venga o no de la caché.
    pub fn input_total(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }

    fn is_empty(&self) -> bool {
        self.input_total() + self.output == 0
    }
}

/// Hasta dónde se leyó un archivo y lo que hace falta para seguir leyéndolo.
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct ParseState {
    pub offset: u64,
    /// Hora del último evento (ms desde 1970), para medir el trabajo.
    pub last_ts: Option<i64>,
    /// El último evento cerró un turno: lo que pase hasta el siguiente no es trabajo.
    #[serde(default)]
    pub idle: bool,
    /// Claude Code: los últimos identificadores de mensaje (cada mensaje ocupa varias líneas con
    /// el mismo uso, que se cuenta una sola vez).
    #[serde(default)]
    pub recent_ids: Vec<String>,
    /// Codex: totales acumulados de la sesión (entrada, en caché, salida).
    #[serde(default)]
    pub totals: Option<[u64; 3]>,
    /// Codex: modelo del turno en curso (índice en `FileStats::models`).
    #[serde(default)]
    pub model: Option<u16>,
    /// Codex: el archivo marca los turnos con `task_started` (las peticiones se cuentan con él).
    #[serde(default)]
    pub task_events: bool,
}

/// Resumen de un archivo de sesión.
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct FileStats {
    /// Identifica el archivo aunque se mueva (Codex archiva las sesiones en otra carpeta).
    pub key: String,
    pub path: PathBuf,
    pub agent: AgentKind,
    /// Sesión principal (no un subagente): cuenta como sesión y sus mensajes como peticiones.
    pub main: bool,
    pub size: u64,
    /// Fecha de modificación (ms) con la que se leyó.
    pub mtime: u64,
    pub state: ParseState,
    pub models: Vec<String>,
    /// (hora UTC desde 1970, modelo, tokens)
    pub usage: Vec<(u32, u16, Usage)>,
    /// (hora UTC, peticiones, segundos trabajando)
    pub activity: Vec<(u32, u32, u32)>,
    /// Minutos con trabajo (minuto UTC desde 1970, cuántos seguidos): para saber cuántos agentes
    /// trabajaban a la vez.
    pub runs: Vec<(u32, u32)>,
}

impl FileStats {
    fn reset(&mut self) {
        self.state = ParseState::default();
        self.models.clear();
        self.usage.clear();
        self.activity.clear();
        self.runs.clear();
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    version: u32,
    files: Vec<FileStats>,
}

/// Lo que el hilo manda a la ventana.
#[derive(Clone, Debug)]
pub enum StatsMsg {
    /// Primera lectura en marcha: archivos leídos de cuántos.
    Progress(usize, usize),
    Data(Arc<Vec<FileStats>>),
}

// ---------------------------------------------------------------------------------------------
// Hilo de lectura
// ---------------------------------------------------------------------------------------------

/// Para despertar (y terminar) al hilo; soltar el emisor lo para.
static WORKER: Mutex<Option<mpsc::Sender<()>>> = Mutex::new(None);
/// Generación del hilo vivo: uno que no es el actual deja de leer en cuanto puede.
static GEN: AtomicU64 = AtomicU64::new(0);

pub fn start(proxy: EventLoopProxy<UserEvent>) {
    let (tx, rx) = mpsc::channel::<()>();
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    *WORKER.lock().unwrap() = Some(tx);
    let _ = std::thread::Builder::new()
        .name("estadisticas".into())
        .spawn(move || {
            let alive = || GEN.load(Ordering::SeqCst) == gen;
            let mut cache = load_cache();
            let mut first = true;
            loop {
                let mut last = Instant::now();
                let mut progress = |done: usize, total: usize| {
                    if first && last.elapsed() > Duration::from_millis(150) {
                        last = Instant::now();
                        let _ = proxy.send_event(UserEvent::Stats(StatsMsg::Progress(done, total)));
                    }
                };
                let changed = scan(&mut cache.files, &alive, &mut progress);
                if changed {
                    save_cache(&cache, gen);
                }
                if !alive() {
                    return;
                }
                if changed || first {
                    let data = Arc::new(cache.files.clone());
                    if proxy
                        .send_event(UserEvent::Stats(StatsMsg::Data(data)))
                        .is_err()
                    {
                        return;
                    }
                }
                first = false;
                match rx.recv_timeout(REFRESH) {
                    Err(mpsc::RecvTimeoutError::Timeout) if alive() => {}
                    _ => return,
                }
            }
        });
}

pub fn stop() {
    GEN.fetch_add(1, Ordering::SeqCst);
    WORKER.lock().unwrap().take();
}

fn cache_path() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("garbo").join("estadisticas.json"))
}

fn load_cache() -> Cache {
    let cache = cache_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Cache>(&b).ok());
    match cache {
        Some(c) if c.version == CACHE_VERSION => c,
        _ => Cache {
            version: CACHE_VERSION,
            files: Vec::new(),
        },
    }
}

fn save_cache(cache: &Cache, gen: u64) {
    let Some(path) = cache_path() else { return };
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    // Se escribe aparte y se renombra: nunca queda una caché a medias.
    let tmp = dir.join(format!("estadisticas.{gen}.tmp"));
    let ok = serde_json::to_vec(cache)
        .ok()
        .is_some_and(|b| std::fs::write(&tmp, b).is_ok());
    if !ok || std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

// ---------------------------------------------------------------------------------------------
// Archivos de sesión
// ---------------------------------------------------------------------------------------------

/// Un archivo de sesión encontrado.
struct Found {
    agent: AgentKind,
    key: String,
    path: PathBuf,
    main: bool,
}

fn entries(dir: &Path) -> impl Iterator<Item = std::fs::DirEntry> {
    std::fs::read_dir(dir).into_iter().flatten().flatten()
}

fn is_dir(e: &std::fs::DirEntry) -> bool {
    e.file_type().is_ok_and(|t| t.is_dir())
}

fn is_file(e: &std::fs::DirEntry) -> bool {
    e.file_type().is_ok_and(|t| t.is_file())
}

fn name_of(e: &std::fs::DirEntry) -> String {
    e.file_name().to_string_lossy().into_owned()
}

fn env_dirs(var: &str) -> Option<Vec<PathBuf>> {
    let v = std::env::var_os(var).filter(|v| !v.is_empty())?;
    Some(std::env::split_paths(&v).collect())
}

/// Carpetas de Claude Code: `$CLAUDE_CONFIG_DIR` o `~/.claude` (y `~/.config/claude`).
fn claude_roots(home: &Path) -> Vec<PathBuf> {
    env_dirs("CLAUDE_CONFIG_DIR")
        .unwrap_or_else(|| vec![home.join(".claude"), home.join(".config").join("claude")])
}

fn codex_root(home: &Path) -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
}

/// Todos los archivos de sesión de los tres agentes.
fn find_files() -> Vec<Found> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let mut out = Vec::new();

    // Claude Code: projects/<proyecto>/<sesión>.jsonl y sus subagentes en
    // projects/<proyecto>/<sesión>/subagents/agent-*.jsonl.
    for root in claude_roots(&home) {
        for project in entries(&root.join("projects")).filter(is_dir) {
            let pname = name_of(&project);
            for e in entries(&project.path()) {
                let name = name_of(&e);
                if is_file(&e) && name.ends_with(".jsonl") {
                    out.push(Found {
                        agent: AgentKind::Claude,
                        key: format!("claude/{pname}/{name}"),
                        path: e.path(),
                        main: !name.starts_with("agent-"),
                    });
                } else if is_dir(&e) {
                    for sub in entries(&e.path().join("subagents")).filter(is_file) {
                        let sname = name_of(&sub);
                        if sname.ends_with(".jsonl") {
                            out.push(Found {
                                agent: AgentKind::Claude,
                                key: format!("claude/{pname}/{name}/{sname}"),
                                path: sub.path(),
                                main: false,
                            });
                        }
                    }
                }
            }
        }
    }

    // Codex: sessions/AAAA/MM/DD/rollout-*.jsonl y archived_sessions/. La clave es el nombre:
    // al archivar una sesión cambia de carpeta, no de nombre.
    let codex = codex_root(&home);
    let mut rollout = |e: std::fs::DirEntry| {
        let name = name_of(&e);
        if is_file(&e) && name.starts_with("rollout-") && name.ends_with(".jsonl") {
            out.push(Found {
                agent: AgentKind::Codex,
                key: format!("codex/{name}"),
                path: e.path(),
                main: true,
            });
        }
    };
    for y in entries(&codex.join("sessions")).filter(is_dir) {
        for m in entries(&y.path()).filter(is_dir) {
            for d in entries(&m.path()).filter(is_dir) {
                entries(&d.path()).for_each(&mut rollout);
            }
        }
    }
    entries(&codex.join("archived_sessions")).for_each(&mut rollout);

    // Gemini CLI: tmp/<proyecto>/chats/session-*.json(l) y los subagentes en chats/<id>/.
    for project in entries(&home.join(".gemini").join("tmp")).filter(is_dir) {
        let pname = name_of(&project);
        for e in entries(&project.path().join("chats")) {
            let name = name_of(&e);
            let json = |n: &str| n.ends_with(".json") || n.ends_with(".jsonl");
            if is_file(&e) && json(&name) {
                out.push(Found {
                    agent: AgentKind::Gemini,
                    key: format!("gemini/{pname}/{name}"),
                    path: e.path(),
                    main: true,
                });
            } else if is_dir(&e) {
                for sub in entries(&e.path()).filter(is_file) {
                    let sname = name_of(&sub);
                    if json(&sname) {
                        out.push(Found {
                            agent: AgentKind::Gemini,
                            key: format!("gemini/{pname}/{name}/{sname}"),
                            path: sub.path(),
                            main: false,
                        });
                    }
                }
            }
        }
    }
    out
}

fn mtime_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// Lee lo nuevo de cada archivo. Devuelve si cambió algo. Los archivos que ya no existen se
/// conservan: su historial sigue contando.
fn scan(
    files: &mut Vec<FileStats>,
    alive: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(usize, usize),
) -> bool {
    let found = find_files();
    let mut index: HashMap<String, usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.key.clone(), i))
        .collect();
    let mut changed = false;
    let total = found.len();
    for (n, f) in found.into_iter().enumerate() {
        if !alive() {
            break;
        }
        progress(n, total);
        let Ok(meta) = std::fs::metadata(&f.path) else {
            continue;
        };
        let (size, mtime) = (meta.len(), mtime_ms(&meta));
        let i = *index.entry(f.key.clone()).or_insert_with(|| {
            files.push(FileStats {
                key: f.key.clone(),
                agent: f.agent,
                ..Default::default()
            });
            files.len() - 1
        });
        let entry = &mut files[i];
        if entry.size == size && entry.mtime == mtime && entry.path == f.path {
            continue;
        }
        entry.path = f.path;
        entry.agent = f.agent;
        if entry.size == 0 && entry.state.offset == 0 {
            entry.main = f.main;
        }
        parse::update(entry, size);
        entry.size = size;
        entry.mtime = mtime;
        changed = true;
    }
    changed
}

// ---------------------------------------------------------------------------------------------
// Fechas locales
// ---------------------------------------------------------------------------------------------

/// Días desde 1970-01-01 de una fecha civil (algoritmo de Howard Hinnant).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Fecha civil (año, mes, día) de un número de día desde 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Día de la semana (0 = lunes) de un número de día. El 1-1-1970 fue jueves.
pub fn weekday(day: i64) -> usize {
    (day + 3).rem_euclid(7) as usize
}

/// Convierte horas UTC a la hora local, recordando el desfase de cada hora (cambia con el
/// horario de verano).
struct Clock {
    offsets: HashMap<u32, i32>,
}

impl Clock {
    fn new() -> Self {
        Self {
            offsets: HashMap::new(),
        }
    }

    /// (día local, día de la semana, hora del día) de una hora UTC.
    fn local(&mut self, hour: u32) -> (i64, usize, usize) {
        let off = *self
            .offsets
            .entry(hour)
            .or_insert_with(|| crate::platform::utc_offset(hour as i64 * 3600));
        let secs = hour as i64 * 3600 + off as i64;
        let day = secs.div_euclid(86_400);
        (day, weekday(day), (secs.rem_euclid(86_400) / 3600) as usize)
    }
}

/// Hoy (día local).
pub fn today() -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    (now + crate::platform::utc_offset(now) as i64).div_euclid(86_400)
}

// ---------------------------------------------------------------------------------------------
// Resumen para dibujar
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Period {
    Week,
    Month,
    Quarter,
    All,
}

impl Period {
    pub const ALL: [Period; 4] = [Period::Week, Period::Month, Period::Quarter, Period::All];

    pub fn days(self) -> Option<i64> {
        match self {
            Period::Week => Some(7),
            Period::Month => Some(30),
            Period::Quarter => Some(90),
            Period::All => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Period::Week => "7 días",
            Period::Month => "30 días",
            Period::Quarter => "90 días",
            Period::All => "Todo",
        }
    }
}

/// Totales de un periodo.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Totals {
    pub usage: Usage,
    pub prompts: u64,
    /// Segundos trabajando (sumando todos los agentes y subagentes).
    pub work: u64,
    pub sessions: u64,
    /// Máximo de agentes trabajando en el mismo minuto.
    pub peak: u32,
}

/// Un día (o una semana, en periodos largos) de la gráfica.
#[derive(Clone, Copy, Default, Debug)]
pub struct Bin {
    pub usage: Usage,
    pub prompts: u64,
    pub work: u64,
}

#[derive(Clone, Debug)]
pub struct ModelRow {
    pub name: String,
    pub provider: Provider,
    pub usage: Usage,
}

#[derive(Clone, Copy, Debug)]
pub struct AgentRow {
    pub agent: AgentKind,
    pub usage: Usage,
    pub work: u64,
    pub prompts: u64,
}

#[derive(Clone, Debug)]
pub struct Summary {
    pub agent: Option<AgentKind>,
    pub today: i64,
    /// Primer día del periodo.
    pub first: i64,
    /// Días por barra (1, o 7 si el periodo pasa de 120 días).
    pub bin_days: i64,
    pub bins: Vec<Bin>,
    pub total: Totals,
    /// Mismos totales en el periodo anterior de igual duración (no con "Todo").
    pub prev: Option<Totals>,
    /// Segundos trabajando por día de la semana (0 = lunes) y hora.
    pub heat: [[u64; 24]; 7],
    pub models: Vec<ModelRow>,
    pub providers: Vec<(Provider, u64)>,
    /// Todos los agentes con datos en el periodo (sin filtrar).
    pub agents: Vec<AgentRow>,
    pub active_days: u64,
    /// Hay algún archivo de sesión, aunque sea de otro periodo.
    pub any_data: bool,
}

impl Summary {
    pub fn days(&self) -> i64 {
        self.today - self.first + 1
    }

    /// Primer día de la barra `i`.
    pub fn bin_start(&self, i: usize) -> i64 {
        let n = self.bins.len() as i64;
        (self.today - (n - 1 - i as i64) * self.bin_days - (self.bin_days - 1)).max(self.first)
    }
}

/// "Claude-opus-4-1-20250805" → "Opus 4.1"; los demás modelos, tal cual.
pub fn model_name(model: &str) -> String {
    let lower = model.to_ascii_lowercase();
    if !lower.starts_with("claude-") {
        return model.to_string();
    }
    let mut family = None;
    let mut version = Vec::new();
    for part in lower.split('-').skip(1) {
        if part.chars().all(|c| c.is_ascii_digit()) {
            // Las fechas (20250805) no son parte de la versión.
            if part.len() <= 2 {
                version.push(part.to_string());
            }
        } else if family.is_none() {
            family = Some(part.to_string());
        }
    }
    match family {
        Some(f) => {
            let mut name: String = f[..1].to_uppercase() + &f[1..];
            if !version.is_empty() {
                name.push(' ');
                name.push_str(&version.join("."));
            }
            name
        }
        None => model.to_string(),
    }
}

fn agent_row(rows: &mut HashMap<AgentKind, AgentRow>, agent: AgentKind) -> &mut AgentRow {
    rows.entry(agent).or_insert(AgentRow {
        agent,
        usage: Usage::default(),
        work: 0,
        prompts: 0,
    })
}

/// Calcula lo que muestra el apartado para un periodo y, si se elige, un solo agente.
pub fn summarize(
    files: &[FileStats],
    period: Period,
    agent: Option<AgentKind>,
    today: i64,
) -> Summary {
    let mut clock = Clock::new();
    let keep = |f: &FileStats| agent.is_none_or(|a| a == f.agent);

    // Primer día: con "Todo", el del dato más antiguo del agente elegido.
    let first = match period.days() {
        Some(d) => today - (d - 1),
        None => files
            .iter()
            .filter(|f| keep(f))
            .flat_map(|f| {
                let u = f.usage.iter().map(|r| r.0);
                let a = f.activity.iter().map(|r| r.0);
                u.chain(a).min()
            })
            .min()
            .map_or(today, |h| clock.local(h).0)
            .min(today),
    };
    let days = today - first + 1;
    let bin_days = if days > 120 { 7 } else { 1 };
    let nbins = ((days + bin_days - 1) / bin_days) as usize;
    let prev_range = period.days().map(|d| (first - d, first - 1));
    let bin_of = |day: i64| nbins - 1 - ((today - day) / bin_days) as usize;

    let mut s = Summary {
        agent,
        today,
        first,
        bin_days,
        bins: vec![Bin::default(); nbins],
        total: Totals::default(),
        prev: prev_range.map(|_| Totals::default()),
        heat: [[0; 24]; 7],
        models: Vec::new(),
        providers: Vec::new(),
        agents: Vec::new(),
        active_days: 0,
        any_data: !files.is_empty(),
    };
    let mut models: HashMap<String, ModelRow> = HashMap::new();
    let mut agents: HashMap<AgentKind, AgentRow> = HashMap::new();
    let mut active: HashSet<i64> = HashSet::new();
    let mut minutes: HashMap<u32, u32> = HashMap::new();
    let in_range = |d: i64| d >= first && d <= today;
    let in_prev = |d: i64| prev_range.is_some_and(|(a, b)| d >= a && d <= b);

    for f in files {
        let mine = keep(f);
        let (mut here, mut before) = (false, false);
        for &(hour, model, u) in &f.usage {
            let (day, ..) = clock.local(hour);
            if in_range(day) {
                agent_row(&mut agents, f.agent).usage.add(&u);
                if !mine {
                    continue;
                }
                here = true;
                active.insert(day);
                s.total.usage.add(&u);
                s.bins[bin_of(day)].usage.add(&u);
                let raw = f.models.get(model as usize).map_or("?", |m| m.as_str());
                let name = model_name(raw);
                models
                    .entry(name.clone())
                    .or_insert_with(|| ModelRow {
                        name,
                        provider: Provider::of_model(raw),
                        usage: Usage::default(),
                    })
                    .usage
                    .add(&u);
            } else if mine && in_prev(day) {
                before = true;
                if let Some(p) = s.prev.as_mut() {
                    p.usage.add(&u);
                }
            }
        }
        for &(hour, prompts, work) in &f.activity {
            let (day, wd, h) = clock.local(hour);
            let (prompts, work) = (prompts as u64, work as u64);
            if in_range(day) {
                let r = agent_row(&mut agents, f.agent);
                r.prompts += prompts;
                r.work += work;
                if !mine {
                    continue;
                }
                here = true;
                active.insert(day);
                s.total.prompts += prompts;
                s.total.work += work;
                let b = &mut s.bins[bin_of(day)];
                b.prompts += prompts;
                b.work += work;
                s.heat[wd][h] += work;
            } else if mine && in_prev(day) {
                before = true;
                if let Some(p) = s.prev.as_mut() {
                    p.prompts += prompts;
                    p.work += work;
                }
            }
        }
        if !mine {
            continue;
        }
        if f.main {
            if here {
                s.total.sessions += 1;
            }
            if before {
                if let Some(p) = s.prev.as_mut() {
                    p.sessions += 1;
                }
            }
        }
        if here {
            for &(start, len) in &f.runs {
                for m in start..start.saturating_add(len) {
                    if in_range(clock.local(m / 60).0) {
                        *minutes.entry(m).or_default() += 1;
                    }
                }
            }
        }
    }

    s.total.peak = minutes.values().copied().max().unwrap_or(0);
    s.active_days = active.len() as u64;
    let mut models: Vec<ModelRow> = models
        .into_values()
        .filter(|m| !m.usage.is_empty())
        .collect();
    // Con empates, siempre el mismo orden (las tablas no deben bailar entre lecturas).
    models.sort_by(|a, b| {
        let t = |m: &ModelRow| m.usage.input_total() + m.usage.output;
        t(b).cmp(&t(a)).then_with(|| a.name.cmp(&b.name))
    });
    let mut providers: HashMap<Provider, u64> = HashMap::new();
    for m in &models {
        *providers.entry(m.provider).or_default() += m.usage.input_total() + m.usage.output;
    }
    let mut providers: Vec<(Provider, u64)> = providers.into_iter().filter(|p| p.1 > 0).collect();
    providers.sort_by_key(|p| (std::cmp::Reverse(p.1), p.0 as u8));
    s.models = models;
    s.providers = providers;
    let mut agents: Vec<AgentRow> = agents
        .into_values()
        .filter(|a| !a.usage.is_empty() || a.work > 0 || a.prompts > 0)
        .collect();
    agents.sort_by_key(|a| {
        let tokens = a.usage.input_total() + a.usage.output;
        (std::cmp::Reverse((tokens, a.work)), a.agent as u8)
    });
    s.agents = agents;
    s
}

// ---------------------------------------------------------------------------------------------
// Formatos
// ---------------------------------------------------------------------------------------------

/// Número con una cifra decimal (coma) sin ",0" final.
fn one_decimal(v: f64) -> String {
    let t = format!("{v:.1}").replace('.', ",");
    t.strip_suffix(",0").map(str::to_string).unwrap_or(t)
}

/// "950", "12,4 K", "61,3 M", "11,6 mil M".
pub fn compact(n: u64) -> String {
    const UNITS: [(f64, &str); 3] = [(1e9, " mil M"), (1e6, " M"), (1e3, " K")];
    let v = n as f64;
    for (i, &(div, unit)) in UNITS.iter().enumerate() {
        // Redondeando, 999 950 ya es "1 M", no "1000 K".
        let next = i.checked_sub(1).map_or(f64::MAX, |j| UNITS[j].0 * 0.99995);
        if v >= div * 0.99995 && v < next {
            let x = v / div;
            return if x >= 100.0 {
                format!("{x:.0}{unit}")
            } else {
                one_decimal(x) + unit
            };
        }
    }
    n.to_string()
}

/// "347 h", "9,2 h", "23 min", "40 s".
pub fn hours(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min", secs / 60),
        3600..=35_999 => one_decimal(secs as f64 / 3600.0) + " h",
        _ => format!("{} h", (secs as f64 / 3600.0).round()),
    }
}

/// "3 h 20 min", "45 min" (tooltips).
pub fn duration_long(secs: u64) -> String {
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    match (h, m) {
        (0, 0) => format!("{secs} s"),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

const MONTHS: [&str; 12] = [
    "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic",
];
pub const WEEKDAYS: [&str; 7] = ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"];
pub const WEEKDAYS_LONG: [&str; 7] = [
    "lunes",
    "martes",
    "miércoles",
    "jueves",
    "viernes",
    "sábado",
    "domingo",
];

/// "23 oct" (o "23 oct 2025" si no es de este año).
pub fn short_date(day: i64, today: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    let (ty, ..) = civil_from_days(today);
    if y == ty {
        format!("{d} {}", MONTHS[m as usize - 1])
    } else {
        format!("{d} {} {y}", MONTHS[m as usize - 1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lee las sesiones reales de este equipo (no escribe la caché):
    /// `cargo test lee_las_sesiones_de_este_equipo -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn lee_las_sesiones_de_este_equipo() {
        let mut files = Vec::new();
        let t = Instant::now();
        scan(&mut files, &|| true, &mut |_, _| {});
        let first = t.elapsed();
        let t = Instant::now();
        let changed = scan(&mut files, &|| true, &mut |_, _| {});
        let again = t.elapsed();
        let t = Instant::now();
        let s = summarize(&files, Period::All, None, today());
        let sum = t.elapsed();
        let bytes = serde_json::to_vec(&Cache {
            version: CACHE_VERSION,
            files: files.clone(),
        })
        .unwrap()
        .len();
        println!("{} archivos; primera lectura {first:?}; segunda {again:?} (cambios: {changed}); resumen {sum:?}; caché {bytes} B", files.len());
        let t = &s.total;
        println!(
            "entrada {} (nueva {}, caché leída {}, escrita {}), salida {}, respuestas {}, peticiones {}, sesiones {}, trabajo {} ({} s), a la vez {}",
            t.usage.input_total(), t.usage.input, t.usage.cache_read, t.usage.cache_write, t.usage.output,
            t.usage.calls, t.prompts, t.sessions, hours(t.work), t.work, t.peak
        );
        for m in &s.models {
            println!(
                "  {:<22} {:?} entrada {} salida {} respuestas {}",
                m.name,
                m.provider,
                m.usage.input_total(),
                m.usage.output,
                m.usage.calls
            );
        }
        for a in &s.agents {
            println!(
                "  {:<12} entrada {} salida {} trabajo {} peticiones {}",
                a.agent.name(),
                a.usage.input_total(),
                a.usage.output,
                hours(a.work),
                a.prompts
            );
        }
        println!(
            "desde {} ({} días, {} con actividad)",
            short_date(s.first, s.today),
            s.days(),
            s.active_days
        );
    }

    #[test]
    fn fechas_civiles() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 11, 23), 20_780);
        assert_eq!(civil_from_days(20_780), (2026, 11, 23));
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        // 23-11-2026 es lunes; 1-1-1970, jueves.
        assert_eq!(weekday(20_780), 0);
        assert_eq!(weekday(0), 3);
    }

    #[test]
    fn formatos() {
        assert_eq!(compact(950), "950");
        assert_eq!(compact(12_400), "12,4 K");
        assert_eq!(compact(999_960), "1 M");
        assert_eq!(compact(61_300_000), "61,3 M");
        assert_eq!(compact(11_600_000_000), "11,6 mil M");
        assert_eq!(compact(402_000_000), "402 M");
        assert_eq!(hours(40), "40 s");
        assert_eq!(hours(23 * 60), "23 min");
        assert_eq!(hours(33_120), "9,2 h");
        assert_eq!(hours(347 * 3600 + 100), "347 h");
        assert_eq!(duration_long(3 * 3600 + 20 * 60), "3 h 20 min");
    }

    #[test]
    fn nombres_de_modelos() {
        assert_eq!(model_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(model_name("claude-opus-4-1-20250805"), "Opus 4.1");
        assert_eq!(model_name("claude-3-5-sonnet-20241022"), "Sonnet 3.5");
        assert_eq!(model_name("gpt-6.2-sol"), "gpt-6.2-sol");
        assert_eq!(Provider::of_model("claude-fable-5-2"), Provider::Anthropic);
        assert_eq!(Provider::of_model("gpt-6.2-sol"), Provider::OpenAi);
        assert_eq!(Provider::of_model("o3"), Provider::OpenAi);
        assert_eq!(Provider::of_model("gemini-2.5-pro"), Provider::Google);
        assert_eq!(Provider::of_model("qwen3-coder"), Provider::Other);
    }

    fn file(agent: AgentKind, main: bool, hour: u32, u: Usage, work: u32) -> FileStats {
        FileStats {
            key: format!("{agent:?}{hour}"),
            agent,
            main,
            models: vec!["claude-opus-5-5".into()],
            usage: vec![(hour, 0, u)],
            activity: vec![(hour, 1, work)],
            runs: vec![(hour * 60, work / 60)],
            ..Default::default()
        }
    }

    #[test]
    fn resumen_por_periodo_y_agente() {
        // Horas UTC al mediodía: el día local es el mismo en cualquier zona de -11 a +11.
        let today = 20_780;
        let noon = |day: i64| (day * 24 + 12) as u32;
        let u = Usage {
            input: 10,
            cache_read: 90,
            output: 5,
            calls: 1,
            ..Default::default()
        };
        let files = vec![
            file(AgentKind::Claude, true, noon(today), u, 600),
            file(AgentKind::Claude, false, noon(today), u, 600),
            file(AgentKind::Codex, true, noon(today - 3), u, 1200),
            // Del periodo anterior a la última semana.
            file(AgentKind::Claude, true, noon(today - 10), u, 60),
        ];
        let s = summarize(&files, Period::Week, None, today);
        assert_eq!(s.bins.len(), 7);
        assert_eq!(s.total.usage.input_total(), 300);
        assert_eq!(s.total.usage.output, 15);
        assert_eq!(s.total.sessions, 2);
        assert_eq!(s.total.prompts, 3);
        assert_eq!(s.total.work, 2400);
        // La sesión y su subagente trabajaban en los mismos minutos.
        assert_eq!(s.total.peak, 2);
        assert_eq!(s.active_days, 2);
        assert_eq!(s.prev.unwrap().usage.input_total(), 100);
        assert_eq!(s.bins[6].usage.calls, 2);
        assert_eq!(s.bins[3].usage.calls, 1);
        assert_eq!(s.agents.len(), 2);

        let only = summarize(&files, Period::Week, Some(AgentKind::Codex), today);
        assert_eq!(only.total.usage.input_total(), 100);
        assert_eq!(only.total.peak, 1);
        // Los demás agentes siguen en su tabla para poder elegirlos.
        assert_eq!(only.agents.len(), 2);

        let all = summarize(&files, Period::All, None, today);
        assert_eq!(all.first, today - 10);
        assert!(all.prev.is_none());
        assert_eq!(all.total.usage.input_total(), 400);
    }
}
