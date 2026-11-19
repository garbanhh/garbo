//! Sugerencias de comandos (texto en gris tras el cursor).
//!
//! Mientras se escribe en el prompt se muestra en gris el resto de un comando ya usado que empieza
//! igual; → (o Ctrl+F) lo acepta entero y Ctrl+→ palabra a palabra, escribiéndolo en el shell. Si
//! el historial no tiene nada, se sugiere el único candidato del completado que encaje ("cd Doc" ->
//! "umentos/"). El menú de completado (Tab) está en `complete.rs`.
//!
//! Lo que se está escribiendo se lee de la propia pantalla: desde la marca OSC 133;B (fin del
//! prompt) hasta el cursor. Por eso solo funciona con la integración de shell activa.
//!
//! Historial: `$HISTFILE` (o `~/.bash_history`), releído si cambia, más el historial propio de
//! Garbo (`~/.local/share/garbo/history.tsv`), que guarda cada comando ejecutado con la carpeta
//! donde se ejecutó (bash no la guarda).
//!
//! Para que lo sugerido tenga sentido:
//!   - primero lo ejecutado en esta misma carpeta; después, lo de cualquier sitio;
//!   - un `cd` a una ruta relativa solo se sugiere donde se hizo: `cd Proyectos` ejecutado en
//!     ~/Documentos no se ofrece en ~ aunque exista ~/Proyectos (otra carpeta con el mismo nombre);
//!   - solo líneas cuya primera palabra se puede ejecutar (programa del PATH, orden interna de bash,
//!     o alias/función de los archivos de arranque): fuera erratas y fallos;
//!   - `cd X` solo si X existe desde el directorio actual;
//!   - hacen falta al menos 2 caracteres escritos;
//!   - los comandos que acaban en "orden no encontrada" (126/127) no se guardan;
//!   - nada mientras readline reescribe la línea (búsqueda con Ctrl+R).

use crate::complete::Kind;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// Entradas como máximo (las más recientes).
const MAX_ENTRIES: usize = 20_000;
/// Registros del historial propio: al cargarlo con más de estos, se queda con la mitad.
const MAX_RECORDS: usize = 20_000;
/// Carpetas recordadas por comando.
const MAX_DIRS: usize = 8;
/// Cada cuánto, como mucho, se mira si los archivos de historial cambiaron.
const RELOAD_EVERY: Duration = Duration::from_secs(5);
/// Cada cuánto se rehace la lista de órdenes ejecutables.
const KNOWN_EVERY: Duration = Duration::from_secs(60);
/// Cuánto vale una sugerencia calculada para la misma entrada (el dibujo la pide en cada fotograma).
const CACHE_FOR: Duration = Duration::from_secs(1);
/// Caracteres escritos necesarios para sugerir.
const MIN_PREFIX: usize = 2;

/// Órdenes internas y palabras clave de bash.
const BUILTINS: &[&str] = &[
    ".", ":", "[", "[[", "alias", "bg", "bind", "break", "builtin", "case", "cd", "command", "compgen",
    "complete", "continue", "declare", "dirs", "disown", "echo", "enable", "eval", "exec", "exit",
    "export", "false", "fc", "fg", "for", "function", "getopts", "hash", "help", "history", "if",
    "jobs", "kill", "let", "local", "logout", "mapfile", "popd", "printf", "pushd", "pwd", "read",
    "readonly", "return", "select", "set", "shift", "shopt", "source", "suspend", "test", "time",
    "times", "trap", "true", "type", "typeset", "ulimit", "umask", "unalias", "unset", "until",
    "wait", "while",
];

/// Un comando del historial y las carpetas donde se ejecutó en Garbo (la más reciente primero).
struct Entry {
    cmd: String,
    dirs: Vec<PathBuf>,
}

/// Comando ejecutado en Garbo (historial propio).
struct Record {
    cmd: String,
    dir: Option<PathBuf>,
}

#[derive(Default)]
struct History {
    /// De más antiguo a más reciente, sin repetidos.
    entries: Vec<Entry>,
    /// Líneas del historial de bash.
    bash: Vec<String>,
    /// Historial propio, de más antiguo a más reciente.
    garbo: Vec<Record>,
    bash_mtime: Option<SystemTime>,
    garbo_mtime: Option<SystemTime>,
    garbo_loaded: bool,
    checked: Option<Instant>,
    /// Cambia con cada modificación (invalida la caché de sugerencias).
    generation: u64,
}

static HISTORY: Mutex<Option<History>> = Mutex::new(None);

fn history_file() -> Option<PathBuf> {
    std::env::var_os("HISTFILE")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".bash_history")))
}

/// Historial propio de Garbo (`GARBO_HISTORY` para usar otro archivo).
fn garbo_history_file() -> Option<PathBuf> {
    std::env::var_os("GARBO_HISTORY")
        .map(PathBuf::from)
        .or_else(|| dirs::data_dir().map(|d| d.join("garbo").join("history.tsv")))
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Línea del historial propio: `segundos \t código \t carpeta \t comando`.
fn record_line(cmd: &str, code: Option<i32>, dir: Option<&Path>) -> String {
    let secs = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let code = code.map(|c| c.to_string()).unwrap_or_default();
    let dir = dir.and_then(Path::to_str).unwrap_or("");
    format!("{secs}\t{code}\t{dir}\t{cmd}\n")
}

fn parse_records(text: &str) -> Vec<Record> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.splitn(4, '\t');
            let (_secs, _code, dir, cmd) = (f.next()?, f.next()?, f.next()?, f.next()?);
            valid(cmd).then(|| Record { cmd: cmd.to_string(), dir: (!dir.is_empty()).then(|| PathBuf::from(dir)) })
        })
        .collect()
}

/// Lee el historial propio; si ha crecido demasiado, lo reescribe con la mitad más reciente.
fn load_records(path: &Path) -> Vec<Record> {
    let text = std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let recs = parse_records(&text);
    if recs.len() <= MAX_RECORDS {
        return recs;
    }
    let lines: Vec<&str> = text.lines().collect();
    let keep = lines[lines.len().saturating_sub(MAX_RECORDS / 2)..].join("\n") + "\n";
    let tmp = path.with_extension("tsv.tmp");
    if std::fs::write(&tmp, &keep).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
    parse_records(&keep)
}

impl History {
    fn refresh(&mut self) {
        let now = Instant::now();
        if self.checked.is_some_and(|t| now.duration_since(t) < RELOAD_EVERY) {
            return;
        }
        self.checked = Some(now);
        let mut changed = false;
        if let Some(path) = history_file() {
            let m = mtime(&path);
            if m.is_some() && m != self.bash_mtime {
                self.bash_mtime = m;
                let text = std::fs::read(&path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
                self.bash = text.lines().filter(|l| !is_timestamp(l)).map(str::to_string).collect();
                changed = true;
            }
        }
        if let Some(path) = garbo_history_file() {
            let m = mtime(&path);
            if !self.garbo_loaded || (m.is_some() && m != self.garbo_mtime) {
                self.garbo_loaded = true;
                self.garbo_mtime = m;
                self.garbo = load_records(&path);
                changed = true;
            }
        }
        if changed {
            self.rebuild();
        }
    }

    /// Une los dos historiales: el de bash primero y el propio después (es el más reciente: bash
    /// solo escribe su archivo al salir).
    fn rebuild(&mut self) {
        let mut dirs: HashMap<&str, Vec<PathBuf>> = HashMap::new();
        for r in self.garbo.iter().rev() {
            if let Some(d) = &r.dir {
                let v = dirs.entry(r.cmd.as_str()).or_default();
                if v.len() < MAX_DIRS && !v.contains(d) {
                    v.push(d.clone());
                }
            }
        }
        let cmds = dedup(self.bash.iter().cloned().chain(self.garbo.iter().map(|r| r.cmd.clone())));
        self.entries = cmds
            .into_iter()
            .map(|cmd| {
                let dirs = dirs.get(cmd.as_str()).cloned().unwrap_or_default();
                Entry { cmd, dirs }
            })
            .collect();
        self.generation += 1;
    }

    fn push(&mut self, cmd: &str, dir: Option<PathBuf>) {
        let mut dirs = match self.entries.iter().position(|e| e.cmd == cmd) {
            Some(i) => self.entries.remove(i).dirs,
            None => Vec::new(),
        };
        if let Some(d) = &dir {
            dirs.retain(|x| x != d);
            dirs.insert(0, d.clone());
            dirs.truncate(MAX_DIRS);
        }
        self.entries.push(Entry { cmd: cmd.to_string(), dirs });
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
        self.garbo.push(Record { cmd: cmd.to_string(), dir });
        self.generation += 1;
    }

    /// Resto del comando más reciente que empieza por `prefix` y pasa el filtro `ok`: primero los
    /// ejecutados en `cwd`; después cualquiera salvo los `cd` relativos (se hicieron en otro sitio).
    fn suggest(&self, prefix: &str, cwd: Option<&Path>, ok: impl Fn(&str) -> bool) -> Option<String> {
        let candidates = || {
            self.entries
                .iter()
                .rev()
                .filter(|e| e.cmd.len() > prefix.len() && e.cmd.starts_with(prefix) && !e.cmd[prefix.len()..].trim().is_empty())
        };
        let here = cwd.and_then(|cwd| candidates().find(|e| e.dirs.iter().any(|d| d == cwd) && ok(&e.cmd)));
        here.or_else(|| candidates().find(|e| !relative_cd(&e.cmd) && ok(&e.cmd)))
            .map(|e| e.cmd[prefix.len()..].to_string())
    }
}

/// ¿Cambia el comando a una carpeta relativa (`cd Proyectos`)? Su destino depende de dónde se
/// ejecutó. `cd`, `cd -`, `cd ..`, `cd ~/x`, `cd /x` y con variables valen en cualquier sitio.
fn relative_cd(cmd: &str) -> bool {
    let toks: Vec<&str> = cmd.split_whitespace().collect();
    toks.windows(2).any(|w| {
        let t = w[1].trim_end_matches([';', '&']).trim_matches(['\'', '"']);
        matches!(w[0], "cd" | "pushd")
            && !(t.is_empty()
                || t.starts_with(['-', '/', '~'])
                || t.contains('$')
                || t.split('/').all(|c| c.is_empty() || c == "." || c == ".."))
    })
}

struct Known {
    names: HashMap<String, Kind>,
    built: Instant,
}

static KNOWN: Mutex<Option<Known>> = Mutex::new(None);

/// Programas del PATH (y carpetas habituales del usuario), órdenes internas, y alias y funciones de
/// los archivos de arranque de bash.
fn build_known() -> HashMap<String, Kind> {
    let mut names: HashMap<String, Kind> = BUILTINS.iter().map(|s| (s.to_string(), Kind::Builtin)).collect();
    let home = dirs::home_dir().unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend([home.join(".local/bin"), home.join("bin"), home.join(".cargo/bin")]);
    for d in dirs {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                names.entry(e.file_name().to_string_lossy().into_owned()).or_insert(Kind::Command);
            }
        }
    }
    let mut rc: Vec<PathBuf> = [".bashrc", ".bash_aliases", ".bash_profile", ".profile"].iter().map(|f| home.join(f)).collect();
    rc.push(PathBuf::from("/etc/bashrc"));
    for dir in [home.join(".bashrc.d"), PathBuf::from("/etc/profile.d")] {
        if let Ok(rd) = std::fs::read_dir(dir) {
            rc.extend(rd.flatten().map(|e| e.path()));
        }
    }
    for f in rc {
        if let Ok(text) = std::fs::read_to_string(f) {
            for n in text.lines().filter_map(defined_name) {
                names.insert(n, Kind::Alias);
            }
        }
    }
    names
}

/// Órdenes que se pueden ejecutar (nombre -> tipo), rehechas como mucho cada minuto.
pub fn with_known<R>(f: impl FnOnce(&HashMap<String, Kind>) -> R) -> R {
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    if known.as_ref().is_none_or(|k| k.built.elapsed() >= KNOWN_EVERY) {
        *known = Some(Known { names: build_known(), built: Instant::now() });
    }
    f(&known.as_ref().unwrap().names)
}

/// Nombre que define una línea de un archivo de arranque: `alias x=…`, `function x`, `x() {`.
fn defined_name(line: &str) -> Option<String> {
    let l = line.trim();
    let ident = |s: &str| {
        let n: String = s.chars().take_while(|c| c.is_alphanumeric() || "_-.:".contains(*c)).collect();
        (!n.is_empty()).then_some(n)
    };
    if let Some(rest) = l.strip_prefix("alias ") {
        let rest = rest.trim_start().trim_start_matches("-- ");
        return rest.split_once('=').and_then(|(n, _)| ident(n.trim()));
    }
    if let Some(rest) = l.strip_prefix("function ") {
        return ident(rest.trim());
    }
    let name = ident(l)?;
    l[name.len()..].trim_start().starts_with("()").then_some(name)
}

/// Primera palabra que se ejecuta (saltando asignaciones `VAR=valor` y `(`, `{`, `!`).
fn first_word(cmd: &str) -> Option<&str> {
    cmd.split_whitespace()
        .map(|w| w.trim_start_matches(['(', '{', '!']))
        .find(|w| {
            let assign = w.split_once('=').is_some_and(|(n, _)| {
                !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !n.starts_with(|c: char| c.is_ascii_digit())
            });
            !w.is_empty() && !assign
        })
}

fn expand(p: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let p = p.trim_matches(['\'', '"']);
    if let Some(rest) = p.strip_prefix("~") {
        return Some(dirs::home_dir()?.join(rest.trim_start_matches('/')));
    }
    let path = Path::new(p);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        cwd.map(|c| c.join(path))
    }
}

/// ¿Tiene sentido sugerir este comando?
fn runnable(cmd: &str, known: &HashMap<String, Kind>, cwd: Option<&Path>) -> bool {
    let Some(w) = first_word(cmd) else { return false };
    if w.contains('/') {
        return expand(w, cwd).is_none_or(|p| p.exists());
    }
    if !known.contains_key(w) {
        return false;
    }
    if w == "cd" {
        // El destino tiene que existir desde aquí (sin destino, "-" o con variables: vale).
        let target = cmd.split_whitespace().skip_while(|t| *t != "cd").nth(1);
        return match target.map(|t| t.trim_end_matches([';', '&'])) {
            None | Some("-") | Some("") => true,
            Some(t) if t.contains('$') => true,
            Some(t) => expand(t, cwd).is_none_or(|p| p.is_dir()),
        };
    }
    true
}

/// Las marcas de tiempo de bash (`HISTTIMEFORMAT`) son líneas `#1700000000`.
fn is_timestamp(l: &str) -> bool {
    l.len() > 1 && l.starts_with('#') && l[1..].bytes().all(|b| b.is_ascii_digit())
}

fn valid(cmd: &str) -> bool {
    !cmd.trim().is_empty() && !cmd.chars().any(char::is_control)
}

/// Quita repetidos conservando la aparición más reciente, y limita el tamaño.
fn dedup(items: impl DoubleEndedIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out: Vec<String> = items.rev().filter(|e| valid(e) && seen.insert(e.clone())).take(MAX_ENTRIES).collect();
    out.reverse();
    out
}

/// Anota un comando ejecutado en `dir` con su código de salida, en memoria y en el historial
/// propio. No se guardan los que empiezan por espacio (convención de bash) ni los que no se
/// pudieron ejecutar (126/127: no existe o no es ejecutable).
pub fn record(cmd: &str, code: Option<i32>, dir: Option<&Path>) {
    let cmd = cmd.trim_end();
    if cmd.starts_with(' ') || !valid(cmd) || cmd.chars().count() > 500 || matches!(code, Some(126 | 127)) {
        return;
    }
    // Una carpeta con tabuladores o saltos de línea rompería el formato: se guarda sin ella.
    let dir = dir.filter(|d| d.to_str().is_some_and(|s| !s.contains(['\t', '\n'])));
    let path = garbo_history_file();
    if let Some(path) = &path {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(record_line(cmd, code, dir).as_bytes());
        }
    }
    // Sin releer los archivos: esto se llama desde el hilo del PTY con el terminal bloqueado.
    let mut h = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    let h = h.get_or_insert_with(History::default);
    h.push(cmd, dir.map(Path::to_path_buf));
    if h.garbo_loaded {
        // Lo acabamos de escribir nosotros: no hace falta releerlo.
        h.garbo_mtime = path.as_deref().and_then(mtime);
    }
}

struct Cached {
    input: String,
    cwd: Option<PathBuf>,
    generation: u64,
    at: Instant,
    result: Option<String>,
}

static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

/// Resto del comando sugerido para lo escrito hasta ahora (`cwd` = directorio de la pestaña).
pub fn suggest(input: &str, cwd: Option<&Path>) -> Option<String> {
    if input.trim().chars().count() < MIN_PREFIX {
        return None;
    }
    let generation = {
        let mut h = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
        let h = h.get_or_insert_with(History::default);
        h.refresh();
        h.generation
    };
    if let Some(c) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if c.input == input && c.cwd.as_deref() == cwd && c.generation == generation && c.at.elapsed() < CACHE_FOR {
            return c.result.clone();
        }
    }
    // Orden de bloqueo: KNOWN y luego HISTORY (nunca al revés).
    let from_history = with_known(|names| {
        let h = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
        h.as_ref()?.suggest(input, cwd, |cmd| runnable(cmd, names, cwd))
    });
    let result = from_history.or_else(|| crate::complete::complete(input, cwd)?.ghost());
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) =
        Some(Cached { input: input.to_string(), cwd: cwd.map(Path::to_path_buf), generation, at: Instant::now(), result: result.clone() });
    result
}

/// Primera palabra de una sugerencia (Ctrl+→): los espacios iniciales y hasta el siguiente
/// espacio, o hasta una "/" incluida (" --resume" -> " --resume"; "umentos/Proyectos" -> "umentos/").
pub fn next_word(rest: &str) -> &str {
    let lead = rest.len() - rest.trim_start().len();
    let body = &rest[lead..];
    let end = body.find(|c: char| c.is_whitespace()).unwrap_or(body.len());
    let end = body[..end].find('/').map_or(end, |i| i + 1);
    &rest[..lead + end]
}

/// Posición absoluta (línea contando el historial, columna) del cursor: estable aunque la
/// pantalla se desplace mientras se escribe.
pub fn cursor_mark<T>(term: &Term<T>) -> (i64, usize) {
    let p = term.grid().cursor.point;
    (p.line.0 as i64 + term.history_size() as i64, p.column.0)
}

fn cell_char(c: char) -> char {
    if c == '\0' {
        ' '
    } else {
        c
    }
}

/// Texto escrito desde `start` hasta el cursor. `None` si el cursor no está al final de lo escrito
/// o la entrada ocupa varias líneas lógicas.
pub fn current_input<T>(term: &Term<T>, start: (i64, usize)) -> Option<String> {
    let cur = term.grid().cursor.point;
    let start_line = Line((start.0 - term.history_size() as i64) as i32);
    if start_line > cur.line || start_line < term.topmost_line() {
        return None;
    }
    let cols = term.columns();
    if !prompt_before(term, start_line, start.1) {
        return None;
    }
    let mut s = String::new();
    let mut l = start_line;
    loop {
        let row = &term.grid()[l];
        let from = if l == start_line { start.1 } else { 0 };
        let to = if l == cur.line { cur.column.0 } else { cols };
        for c in from..to.min(cols) {
            let cell = &row[Column(c)];
            if !cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                s.push(cell_char(cell.c));
            }
        }
        if l == cur.line {
            break;
        }
        if !row[Column(cols - 1)].flags.contains(Flags::WRAPLINE) {
            return None;
        }
        l += 1;
    }
    // Detrás del cursor no puede haber nada escrito.
    let row = &term.grid()[cur.line];
    if (cur.column.0..cols).any(|c| cell_char(row[Column(c)].c) != ' ') || readline_mode(&s) {
        return None;
    }
    Some(s)
}

/// ¿Sigue estando el prompt justo antes de lo escrito? Readline lo sobrescribe en la búsqueda con
/// Ctrl+R ("(reverse-i-search)…"): entonces lo que hay no es la línea de órdenes. Si lo escrito
/// empieza en la primera columna (prompt de Garbo, o un prompt que llena la línea), el prompt está
/// en la línea de encima.
fn prompt_before<T>(term: &Term<T>, line: Line, col: usize) -> bool {
    let is_prompt = |l: Line, c: usize| matches!(crate::blocks::cell_link(&term.grid()[l][Column(c)]), Some((b'p', _)));
    if col > 0 {
        is_prompt(line, col - 1)
    } else if line > term.topmost_line() {
        (0..term.columns()).any(|c| is_prompt(line - 1, c))
    } else {
        true
    }
}

/// Lo que readline muestra en lugar de la línea durante una búsqueda o un argumento numérico. Con
/// el prompt de Garbo lo escribe desde la primera columna, sin tocar el prompt.
fn readline_mode(text: &str) -> bool {
    ["(reverse-i-search)", "(i-search)", "(failed reverse-i-search)", "(failed i-search)", "(arg: "]
        .iter()
        .any(|p| text.starts_with(p))
}

/// Comando completo recién aceptado (el cursor ya bajó de línea al pulsar Intro).
pub fn executed_command<T>(term: &Term<T>, start: (i64, usize)) -> Option<String> {
    let cur = term.grid().cursor.point;
    let start_line = Line((start.0 - term.history_size() as i64) as i32);
    if start_line > cur.line || start_line < term.topmost_line() {
        return None;
    }
    let cols = term.columns();
    let last = if cur.column.0 == 0 && cur.line > start_line { cur.line - 1 } else { cur.line };
    let mut s = String::new();
    let mut l = start_line;
    loop {
        let row = &term.grid()[l];
        let from = if l == start_line { start.1 } else { 0 };
        let mut line = String::new();
        for c in from..cols {
            let cell = &row[Column(c)];
            if !cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                line.push(cell_char(cell.c));
            }
        }
        let wraps = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        if wraps {
            s.push_str(&line);
        } else {
            s.push_str(line.trim_end());
        }
        if l >= last {
            break;
        }
        if !wraps {
            return None; // varias líneas lógicas (heredoc, continuación): no se guarda
        }
        l += 1;
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(bash: &[&str], garbo: &[(&str, Option<&str>)]) -> History {
        let mut h = History {
            bash: bash.iter().filter(|l| !is_timestamp(l)).map(|s| s.to_string()).collect(),
            garbo: garbo.iter().map(|(c, d)| Record { cmd: c.to_string(), dir: d.map(PathBuf::from) }).collect(),
            ..Default::default()
        };
        h.rebuild();
        h
    }

    #[test]
    fn historial_y_sugerencias() {
        let mut h = history(&["git status", "cargo build", "git commit -m x", "cargo build", "#1700000000", "ls"], &[]);
        let cmds: Vec<&str> = h.entries.iter().map(|e| e.cmd.as_str()).collect();
        assert_eq!(cmds, ["git status", "git commit -m x", "cargo build", "ls"]);
        let all = |_: &str| true;
        assert_eq!(h.suggest("git", None, all).as_deref(), Some(" commit -m x"), "la más reciente gana");
        assert_eq!(h.suggest("git s", None, all).as_deref(), Some("tatus"));
        assert_eq!(h.suggest("ls", None, all), None, "igual no es sugerencia");
        h.push("git status", None);
        assert_eq!(h.suggest("git", None, all).as_deref(), Some(" status"), "ejecutarlo lo vuelve más reciente");
        // Con filtro: se salta lo que no pasa y se ofrece el siguiente más reciente.
        assert_eq!(h.suggest("git", None, |c| !c.contains("status")).as_deref(), Some(" commit -m x"));
        assert!(!valid("   "));
        assert!(!valid("a\u{1b}b"));
    }

    #[test]
    fn primero_lo_ejecutado_en_esta_carpeta() {
        let home = Path::new("/home/u");
        let docs = Path::new("/home/u/Documentos");
        let mut h = history(
            &["cd Documentos", "cd Proyectos", "cd ..", "claude --resume"],
            &[("cd Documentos", Some("/home/u")), ("cargo test", Some("/home/u/p")), ("cargo build", Some("/home/u/q"))],
        );
        let all = |_: &str| true;
        // El caso de la captura: en ~, "cd Proyectos" (hecho en ~/Documentos) no se sugiere.
        assert_eq!(h.suggest("cd ", Some(home), all).as_deref(), Some("Documentos"), "lo hecho aquí");
        assert_eq!(h.suggest("cd P", Some(home), all), None, "cd relativo hecho en otro sitio");
        assert_eq!(h.suggest("cd ", Some(docs), all).as_deref(), Some(".."), "cd .. vale en cualquier sitio");
        h.push("cd Proyectos", Some(docs.to_path_buf()));
        assert_eq!(h.suggest("cd ", Some(docs), all).as_deref(), Some("Proyectos"));
        assert_eq!(h.suggest("cd P", Some(home), all), None);
        // Fuera de las carpetas conocidas gana el más reciente; dentro, el de esa carpeta.
        assert_eq!(h.suggest("cargo ", Some(home), all).as_deref(), Some("build"));
        assert_eq!(h.suggest("cargo ", Some(Path::new("/home/u/p")), all).as_deref(), Some("test"));
        assert!(relative_cd("cd GRBTER && claude"));
        assert!(!relative_cd("cd ~/x"));
        assert!(!relative_cd("cd ../.."));
        assert!(!relative_cd("cd \"$X\""));
        assert!(!relative_cd("git status"));
    }

    #[test]
    fn historial_propio_en_archivo() {
        let line = record_line("cd Proyectos", Some(0), Some(Path::new("/home/u/Documentos")));
        assert!(line.ends_with("\t0\t/home/u/Documentos\tcd Proyectos\n"), "{line}");
        let recs = parse_records(&(line + "basura\n1\t\t\tls -la\n"));
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].dir.as_deref(), Some(Path::new("/home/u/Documentos")));
        assert_eq!((recs[1].cmd.as_str(), recs[1].dir.as_deref()), ("ls -la", None));
        assert_eq!(next_word(" --resume"), " --resume");
        assert_eq!(next_word("umentos/Proyectos"), "umentos/");
        assert_eq!(next_word("tatus --short"), "tatus");
    }

    #[test]
    fn solo_comandos_que_se_pueden_ejecutar() {
        let known: HashMap<String, Kind> =
            ["ls", "git", "cd", "servidor", "sudo"].iter().map(|s| (s.to_string(), Kind::Command)).collect();
        let tmp = std::env::temp_dir();
        let cwd = Some(tmp.as_path());
        assert!(runnable("git status", &known, cwd));
        assert!(runnable("servidor", &known, cwd), "alias del usuario");
        assert!(runnable("FOO=1 git log", &known, cwd), "con asignaciones delante");
        assert!(!runnable("calude", &known, cwd), "errata");
        assert!(!runnable("cd..", &known, cwd), "errata sin espacio");
        assert!(runnable("cd", &known, cwd));
        assert!(runnable("cd -", &known, cwd));
        assert!(runnable("cd /", &known, cwd));
        assert!(!runnable("cd /no/existe/garbo", &known, cwd), "destino inexistente");
        assert!(runnable("/bin/sh -c x", &known, cwd));
        assert!(!runnable("/no/existe/x", &known, cwd));
        assert_eq!(defined_name("alias servidor='ssh x'").as_deref(), Some("servidor"));
        assert_eq!(defined_name("  function mkcd {").as_deref(), Some("mkcd"));
        assert_eq!(defined_name("mkcd() {").as_deref(), Some("mkcd"));
        assert_eq!(defined_name("export PATH=x"), None);
        assert_eq!(defined_name("echo hola"), None);
    }
}
