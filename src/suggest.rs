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

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// Entradas como máximo (las más recientes).
const MAX_ENTRIES: usize = 20_000;
/// Cada cuánto, como mucho, se mira si el archivo de historial cambió.
const RELOAD_EVERY: Duration = Duration::from_secs(5);

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

    fn suggest(&self, prefix: &str) -> Option<String> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.len() > prefix.len() && e.starts_with(prefix))
            .map(|e| e[prefix.len()..].to_string())
    }
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

/// Anota un comando ejecutado. Los que empiezan por espacio no se guardan (convención de bash).
pub fn record(cmd: &str) {
    let cmd = cmd.trim_end();
    if cmd.starts_with(' ') || !valid(cmd) || cmd.chars().count() > 500 {
        return;
    }
    // Sin releer el archivo: esto se llama desde el hilo del PTY con el terminal bloqueado.
    let mut h = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    h.get_or_insert_with(History::default).push(cmd);
}

/// Resto del comando sugerido para lo escrito hasta ahora.
pub fn suggest(prefix: &str) -> Option<String> {
    if prefix.trim().is_empty() {
        return None;
    }
    let mut h = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    let h = h.get_or_insert_with(History::default);
    h.refresh();
    h.suggest(prefix)
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
        assert_eq!(h.suggest("git").as_deref(), Some(" commit -m x"), "la más reciente gana");
        assert_eq!(h.suggest("git s").as_deref(), Some("tatus"));
        assert_eq!(h.suggest("ls"), None, "igual no es sugerencia");
        h.push("git status");
        assert_eq!(h.suggest("git").as_deref(), Some(" status"), "ejecutarlo lo vuelve más reciente");
        assert!(!valid("   "));
        assert!(!valid("a\u{1b}b"));
    }
}
