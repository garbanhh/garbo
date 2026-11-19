//! Sugerencias de comandos desde el historial.
//!
//! Mientras se escribe en el prompt, se muestra en gris el resto del comando más reciente que
//! empieza igual; Tab (o → al final de la línea) lo acepta escribiéndolo en el shell. Si no hay
//! sugerencia, Tab llega al shell como siempre (su autocompletado normal).
//!
//! Lo que se está escribiendo se lee de la propia pantalla: desde la marca OSC 133;B (fin del
//! prompt) hasta el cursor. Por eso solo funciona con la integración de shell activa.
//!
//! Historial: `$HISTFILE` (o `~/.bash_history`), releído si cambia, más los comandos ejecutados
//! en cualquier pestaña de Garbo durante la sesión (bash solo escribe su archivo al salir).
//!
//! Para que lo sugerido tenga sentido:
//!   - solo se sugieren líneas cuya primera palabra se puede ejecutar (programa del PATH, orden
//!     interna de bash, o alias/función de los archivos de arranque): fuera erratas y fallos;
//!   - `cd X` solo si X existe desde el directorio actual;
//!   - hacen falta al menos 2 caracteres escritos;
//!   - los comandos de la sesión que acaban en "orden no encontrada" (126/127) no se guardan;
//!   - nada mientras readline reescribe la línea (búsqueda con Ctrl+R).

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// Entradas como máximo (las más recientes).
const MAX_ENTRIES: usize = 20_000;
/// Cada cuánto, como mucho, se mira si el archivo de historial cambió.
const RELOAD_EVERY: Duration = Duration::from_secs(5);
/// Cada cuánto se rehace la lista de órdenes ejecutables.
const KNOWN_EVERY: Duration = Duration::from_secs(60);
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

#[derive(Default)]
struct History {
    /// De más antiguo a más reciente, sin repetidos.
    entries: Vec<String>,
    /// Comandos ejecutados en esta sesión (se conservan al releer el archivo).
    session: Vec<String>,
    file_mtime: Option<SystemTime>,
    checked: Option<Instant>,
}

static HISTORY: Mutex<Option<History>> = Mutex::new(None);

fn history_file() -> Option<PathBuf> {
    std::env::var_os("HISTFILE")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".bash_history")))
}

impl History {
    fn refresh(&mut self) {
        let now = Instant::now();
        if self.checked.is_some_and(|t| now.duration_since(t) < RELOAD_EVERY) {
            return;
        }
        self.checked = Some(now);
        let Some(path) = history_file() else { return };
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if mtime.is_none() || mtime == self.file_mtime {
            return;
        }
        self.file_mtime = mtime;
        let text = std::fs::read(&path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let file = text.lines().filter(|l| !is_timestamp(l));
        self.entries = dedup(file.map(str::to_string).chain(self.session.iter().cloned()));
    }

    fn push(&mut self, cmd: &str) {
        self.entries.retain(|e| e != cmd);
        self.entries.push(cmd.to_string());
        self.session.retain(|e| e != cmd);
        self.session.push(cmd.to_string());
        if self.session.len() > 1000 {
            self.session.remove(0);
        }
    }

    /// El más reciente que empieza por `prefix`, añade algo y pasa el filtro `ok`.
    fn suggest(&self, prefix: &str, ok: impl Fn(&str) -> bool) -> Option<String> {
        self.entries
            .iter()
            .rev()
            .filter(|e| e.len() > prefix.len() && e.starts_with(prefix) && !e[prefix.len()..].trim().is_empty())
            .find(|e| ok(e))
            .map(|e| e[prefix.len()..].to_string())
    }
}

struct Known {
    names: HashSet<String>,
    built: Instant,
}

static KNOWN: Mutex<Option<Known>> = Mutex::new(None);

/// Programas del PATH (y carpetas habituales del usuario), órdenes internas, y alias y funciones de
/// los archivos de arranque de bash.
fn build_known() -> HashSet<String> {
    let mut names: HashSet<String> = BUILTINS.iter().map(|s| s.to_string()).collect();
    let home = dirs::home_dir().unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend([home.join(".local/bin"), home.join("bin"), home.join(".cargo/bin")]);
    for d in dirs {
        if let Ok(rd) = std::fs::read_dir(d) {
            names.extend(rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()));
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
            names.extend(text.lines().filter_map(defined_name));
        }
    }
    names
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
fn runnable(cmd: &str, known: &HashSet<String>, cwd: Option<&Path>) -> bool {
    let Some(w) = first_word(cmd) else { return false };
    if w.contains('/') {
        return expand(w, cwd).is_none_or(|p| p.exists());
    }
    if !known.contains(w) {
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

/// Anota un comando ejecutado con su código de salida. No se guardan los que empiezan por espacio
/// (convención de bash) ni los que no se pudieron ejecutar (126/127: no existe o no es ejecutable).
pub fn record(cmd: &str, code: Option<i32>) {
    let cmd = cmd.trim_end();
    if cmd.starts_with(' ') || !valid(cmd) || cmd.chars().count() > 500 || matches!(code, Some(126 | 127)) {
        return;
    }
    // Sin releer el archivo: esto se llama desde el hilo del PTY con el terminal bloqueado.
    let mut h = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    h.get_or_insert_with(History::default).push(cmd);
}

/// Resto del comando sugerido para lo escrito hasta ahora (`cwd` = directorio de la pestaña).
pub fn suggest(prefix: &str, cwd: Option<&Path>) -> Option<String> {
    if prefix.trim().chars().count() < MIN_PREFIX {
        return None;
    }
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    if known.as_ref().is_none_or(|k| k.built.elapsed() >= KNOWN_EVERY) {
        *known = Some(Known { names: build_known(), built: Instant::now() });
    }
    let names = &known.as_ref().unwrap().names;
    let mut h = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    let h = h.get_or_insert_with(History::default);
    h.refresh();
    h.suggest(prefix, |cmd| runnable(cmd, names, cwd))
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
    if (cur.column.0..cols).any(|c| cell_char(row[Column(c)].c) != ' ') {
        return None;
    }
    Some(s)
}

/// ¿Sigue estando el prompt justo antes de lo escrito? Readline lo sobrescribe en la búsqueda con
/// Ctrl+R ("(reverse-i-search)…"): entonces lo que hay no es la línea de órdenes.
fn prompt_before<T>(term: &Term<T>, line: Line, col: usize) -> bool {
    let (l, c) = if col > 0 {
        (line, col - 1)
    } else if line > term.topmost_line() {
        (line - 1, term.columns() - 1)
    } else {
        return true;
    };
    matches!(crate::blocks::cell_link(&term.grid()[l][Column(c)]), Some((b'p', _)))
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

    #[test]
    fn historial_y_sugerencias() {
        let mut h = History::default();
        h.entries = dedup(
            ["git status", "cargo build", "git commit -m x", "cargo build", "#1700000000", "ls"]
                .into_iter()
                .filter(|l| !is_timestamp(l))
                .map(str::to_string),
        );
        assert_eq!(h.entries, ["git status", "git commit -m x", "cargo build", "ls"]);
        let all = |_: &str| true;
        assert_eq!(h.suggest("git", all).as_deref(), Some(" commit -m x"), "la más reciente gana");
        assert_eq!(h.suggest("git s", all).as_deref(), Some("tatus"));
        assert_eq!(h.suggest("ls", all), None, "igual no es sugerencia");
        h.push("git status");
        assert_eq!(h.suggest("git", all).as_deref(), Some(" status"), "ejecutarlo lo vuelve más reciente");
        // Con filtro: se salta lo que no pasa y se ofrece el siguiente más reciente.
        assert_eq!(h.suggest("git", |c| !c.contains("status")).as_deref(), Some(" commit -m x"));
        assert!(!valid("   "));
        assert!(!valid("a\u{1b}b"));
    }

    #[test]
    fn solo_comandos_que_se_pueden_ejecutar() {
        let known: HashSet<String> = ["ls", "git", "cd", "servidor", "sudo"].iter().map(|s| s.to_string()).collect();
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
