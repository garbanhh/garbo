//! Bloques de comandos: cada comando y su salida forman una unidad con estado y duración.
//!
//! El shell (con el script de integración) emite marcas OSC 133 invisibles:
//!   A = empieza el prompt, B = empieza el comando, C = empieza la salida, D;código = terminó.
//!   `A;garbo` = el prompt es el de Garbo (se dibuja como etiqueta, ver `view::draw_prompts`).
//! Al ver una marca, el bucle del PTY "ancla" el bloque al propio texto: activa un hyperlink
//! invisible (uri `garbo:block`) que se graba en las celdas del prompt y del comando.
//! Así la posición de cada bloque viaja con el contenido: sobrevive al scroll, al recorte
//! del historial y al reflow al redimensionar, sin llevar cuentas de líneas.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::{Handler, Hyperlink};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const LINK_URI: &str = "garbo:block";
const MAX_BLOCKS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// `true` = prompt de Garbo.
    PromptStart(bool),
    CommandStart,
    OutputStart,
    CommandEnd(Option<i32>),
}

/// Escáner incremental de secuencias OSC 133 (y de los informes OSC 7501, ver `status.rs`).
/// Mantiene estado entre lecturas, así que tolera secuencias partidas entre dos `read()` del PTY.
#[derive(Default)]
pub struct MarkScanner {
    state: Scan,
    buf: Vec<u8>,
    /// El OSC en curso es más largo de lo que se admite: se descarta.
    overflow: bool,
    /// Informes y consultas OSC 7501 (`true` = acabó en ST, `false` = en BEL).
    pub status: Vec<(crate::status::Report, bool)>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Scan {
    #[default]
    Ground,
    Esc,
    Osc,
    OscEsc,
}

impl MarkScanner {
    /// Añade a `out` cada marca completada como (offset justo tras la secuencia, marca).
    pub fn scan(&mut self, bytes: &[u8], out: &mut Vec<(usize, Mark)>) {
        for (i, &b) in bytes.iter().enumerate() {
            self.state = match self.state {
                Scan::Ground if b == 0x1b => Scan::Esc,
                Scan::Ground => Scan::Ground,
                Scan::Esc if b == b']' => {
                    self.buf.clear();
                    self.overflow = false;
                    Scan::Osc
                }
                Scan::Esc if b == 0x1b => Scan::Esc,
                Scan::Esc => Scan::Ground,
                Scan::Osc => match b {
                    0x07 => {
                        self.finish(i + 1, out, false);
                        Scan::Ground
                    }
                    0x1b => Scan::OscEsc,
                    0x18 | 0x1a => Scan::Ground, // CAN / SUB abortan la secuencia
                    _ => {
                        // Las marcas caben en 32 bytes; un informe OSC 7501, en `status::MAX_REPORT`.
                        let cap = if self.buf.starts_with(b"7501;") { crate::status::MAX_REPORT + 5 } else { 32 };
                        if self.buf.len() < cap {
                            self.buf.push(b);
                        } else {
                            self.overflow = true;
                        }
                        Scan::Osc
                    }
                },
                Scan::OscEsc if b == b'\\' => {
                    self.finish(i + 1, out, true);
                    Scan::Ground
                }
                Scan::OscEsc if b == b']' => {
                    self.buf.clear();
                    self.overflow = false;
                    Scan::Osc
                }
                Scan::OscEsc => Scan::Ground,
            };
        }
    }

    fn finish(&mut self, end: usize, out: &mut Vec<(usize, Mark)>, st: bool) {
        if let Some(payload) = self.buf.strip_prefix(b"7501;") {
            if let Some(report) = (!self.overflow).then(|| crate::status::parse(payload)).flatten() {
                self.status.push((report, st));
            }
            return;
        }
        let Some(rest) = self.buf.strip_prefix(b"133;") else { return };
        let mark = match rest.first() {
            Some(b'A') => Mark::PromptStart(rest[1..].split(|&b| b == b';').any(|p| p == b"garbo")),
            Some(b'B') => Mark::CommandStart,
            Some(b'C') => Mark::OutputStart,
            Some(b'D') => {
                let code = std::str::from_utf8(&rest[1..])
                    .ok()
                    .and_then(|s| s.trim_start_matches(';').split(';').next()?.parse().ok());
                Mark::CommandEnd(code)
            }
            _ => return,
        };
        out.push((end, mark));
    }
}

#[derive(Clone, Debug)]
pub struct BlockInfo {
    pub id: u32,
    /// Prompt de Garbo (marca `A;garbo`).
    pub styled: bool,
    pub started: Option<Instant>,
    pub duration: Option<Duration>,
    pub exit: Option<i32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// Prompt sin comando ejecutado (escribiendo o Enter vacío).
    Idle,
    Running,
    Ok,
    Failed(i32),
}

impl BlockInfo {
    pub fn status(&self) -> Status {
        match (self.started, self.exit) {
            (None, _) => Status::Idle,
            (Some(_), None) => Status::Running,
            (Some(_), Some(0)) => Status::Ok,
            (Some(_), Some(c)) => Status::Failed(c),
        }
    }
}

/// Estado de bloques de una pestaña, compartido entre el hilo del PTY y la UI.
#[derive(Default)]
pub struct BlockState {
    next_id: u32,
    /// Bloque del prompt actual.
    pub current: Option<u32>,
    infos: VecDeque<BlockInfo>,
    /// El shell tiene integración (se ha visto al menos una marca).
    pub active: bool,
    /// Dónde empieza lo que se está escribiendo en el prompt (entre las marcas B y C), en
    /// coordenadas absolutas (ver `suggest::cursor_mark`).
    pub input_start: Option<(i64, usize)>,
    /// Comando aceptado que aún no ha terminado (y su carpeta): pasa al historial al conocer su
    /// código de salida.
    pending_cmd: Option<(String, Option<PathBuf>)>,
    /// Proceso del shell (para leer su carpeta actual).
    shell_pid: u32,
    /// Carpeta del shell cuando empezó a escribirse el comando (marca B).
    input_cwd: Option<PathBuf>,
    /// Estado que informa el programa en marcha (OSC 7501, ver `status.rs`).
    pub program: crate::status::Records,
}

impl BlockState {
    pub fn new(shell_pid: u32) -> Self {
        Self { shell_pid, ..Default::default() }
    }

    /// Carpeta actual del shell (Linux: /proc/<pid>/cwd).
    fn shell_cwd(&self) -> Option<PathBuf> {
        (self.shell_pid != 0).then(|| std::fs::read_link(format!("/proc/{}/cwd", self.shell_pid)).ok()).flatten()
    }

    pub fn info(&self, id: u32) -> Option<&BlockInfo> {
        let i = self.infos.binary_search_by_key(&id, |b| b.id).ok()?;
        self.infos.get(i)
    }

    /// ¿Usa el bloque el prompt de Garbo?
    pub fn styled(&self, id: u32) -> bool {
        self.info(id).is_some_and(|b| b.styled)
    }

    /// El bloque del prompt actual, si todavía no ha ejecutado nada (se está escribiendo).
    pub fn idle_current(&self) -> Option<u32> {
        let id = self.current?;
        self.info(id).filter(|b| b.started.is_none()).map(|b| b.id)
    }

    /// Último bloque que llegó a ejecutar un comando.
    pub fn last_run(&self) -> Option<u32> {
        self.infos.iter().rev().find(|b| b.started.is_some()).map(|b| b.id)
    }

    fn current_mut(&mut self) -> Option<&mut BlockInfo> {
        let id = self.current?;
        let i = self.infos.binary_search_by_key(&id, |b| b.id).ok()?;
        self.infos.get_mut(i)
    }

    /// Aplica una marca. Se llama con el terminal bloqueado y el parser justo en la marca.
    pub fn apply<T: EventListener>(&mut self, mark: Mark, term: &mut Term<T>) {
        self.active = true;
        let link = |kind: char, id: u32| Hyperlink { id: Some(format!("garbo:{kind}:{id}")), uri: LINK_URI.into() };
        match mark {
            Mark::PromptStart(styled) => {
                // Vuelve el prompt: el programa que informaba ya no está en marcha.
                self.program.drop_live();
                self.next_id += 1;
                let id = self.next_id;
                self.infos.push_back(BlockInfo { id, styled, started: None, duration: None, exit: None });
                if self.infos.len() > MAX_BLOCKS {
                    self.infos.pop_front();
                }
                self.current = Some(id);
                self.input_start = None;
                term.set_hyperlink(Some(link('p', id)));
            }
            Mark::CommandStart => {
                if let Some(id) = self.current {
                    term.set_hyperlink(Some(link('c', id)));
                }
                self.input_start = Some(crate::suggest::cursor_mark(term));
                // El shell está esperando lo que se escriba: su carpeta es donde se ejecutará.
                self.input_cwd = self.shell_cwd();
            }
            Mark::OutputStart => {
                term.set_hyperlink(None);
                // El comando aceptado pasará al historial de sugerencias cuando termine.
                let dir = self.input_cwd.take();
                self.pending_cmd =
                    self.input_start.take().and_then(|s| crate::suggest::executed_command(term, s)).map(|c| (c, dir));
                if let Some(info) = self.current_mut() {
                    info.started = Some(Instant::now());
                }
            }
            Mark::CommandEnd(code) => {
                term.set_hyperlink(None);
                self.input_start = None;
                if let Some((cmd, dir)) = self.pending_cmd.take() {
                    crate::suggest::record(&cmd, code, dir.as_deref());
                }
                if let Some(info) = self.current_mut() {
                    if let (Some(start), None) = (info.started, info.duration) {
                        info.duration = Some(start.elapsed());
                        info.exit = Some(code.unwrap_or(0));
                    }
                }
            }
        }
    }
}

/// (tipo, id) del ancla de bloque de una celda: tipo `p` = prompt, `c` = comando.
pub fn cell_link(cell: &Cell) -> Option<(u8, u32)> {
    let h = cell.hyperlink()?;
    if h.uri() != LINK_URI {
        return None;
    }
    let rest = h.id().strip_prefix("garbo:")?;
    let (kind, id) = rest.split_once(':')?;
    Some((*kind.as_bytes().first()?, id.parse().ok()?))
}

fn line_anchor<T>(term: &Term<T>, line: Line) -> Option<u32> {
    let row = &term.grid()[line];
    (0..term.columns()).find_map(|c| match cell_link(&row[Column(c)]) {
        Some((b'p', id)) => Some(id),
        _ => None,
    })
}

#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    /// Línea del grid (negativa = historial).
    pub line: Line,
    pub id: u32,
}

/// Inicio de cada bloque entre `from` y `to` (incluidas), en orden.
pub fn anchors<T>(term: &Term<T>, from: Line, to: Line) -> Vec<Anchor> {
    let from = from.max(term.topmost_line());
    let to = to.min(term.bottommost_line());
    let mut out: Vec<Anchor> = Vec::new();
    let mut l = from;
    while l <= to {
        if let Some(id) = line_anchor(term, l) {
            // Un prompt de varias líneas repite el id: solo cuenta la primera.
            if out.last().map_or(true, |a| a.id != id) {
                out.push(Anchor { line: l, id });
            }
        }
        l += 1;
    }
    out
}

/// Ancla del bloque que contiene `line` (busca hacia arriba como mucho `limit` líneas).
pub fn anchor_at<T>(term: &Term<T>, line: Line, limit: usize) -> Option<Anchor> {
    let top = term.topmost_line();
    let mut l = line.min(term.bottommost_line());
    for _ in 0..limit {
        if l < top {
            break;
        }
        if let Some(id) = line_anchor(term, l) {
            // Subir hasta la primera línea del prompt (prompts multilínea)
            let mut first = l;
            while first > top && line_anchor(term, first - 1) == Some(id) {
                first -= 1;
            }
            return Some(Anchor { line: first, id });
        }
        l -= 1;
    }
    None
}

/// Rango de líneas [inicio, fin] de un bloque y textos derivados.
pub struct BlockRange {
    pub start: Line,
    pub end: Line,
    pub id: u32,
}

/// Localiza un bloque por id en todo el grid.
pub fn find_block<T>(term: &Term<T>, id: u32) -> Option<BlockRange> {
    let all = anchors(term, term.topmost_line(), term.bottommost_line());
    let i = all.iter().position(|a| a.id == id)?;
    let start = all[i].line;
    let end = match all.get(i + 1) {
        Some(next) => next.line - 1,
        None => term.grid().cursor.point.line.max(start),
    };
    Some(BlockRange { start, end, id })
}

impl BlockRange {
    /// Texto del comando (celdas marcadas con el ancla `c`).
    pub fn command<T>(&self, term: &Term<T>) -> String {
        let cols = term.columns();
        let mut out = String::new();
        let mut l = self.start;
        while l <= self.end {
            let row = &term.grid()[l];
            let mut any = false;
            for c in 0..cols {
                let cell = &row[Column(c)];
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                if cell_link(cell) == Some((b'c', self.id)) {
                    out.push(cell.c);
                    any = true;
                }
            }
            if any && !row[Column(cols - 1)].flags.contains(Flags::WRAPLINE) {
                out.push('\n');
            }
            l += 1;
        }
        out.trim_end().to_string()
    }

    /// Primera línea de salida: la siguiente a la última línea del comando.
    fn output_start<T>(&self, term: &Term<T>) -> Line {
        let cols = term.columns();
        let mut last_cmd = self.start;
        let mut l = self.start;
        while l <= self.end {
            let row = &term.grid()[l];
            if (0..cols).any(|c| cell_link(&row[Column(c)]) == Some((b'c', self.id))) {
                last_cmd = l;
            }
            l += 1;
        }
        last_cmd + 1
    }

    pub fn output<T>(&self, term: &Term<T>) -> String {
        let start = self.output_start(term);
        if start > self.end {
            return String::new();
        }
        let end = Point::new(self.end, Column(term.columns() - 1));
        term.bounds_to_string(Point::new(start, Column(0)), end).trim_end().to_string()
    }

    pub fn whole<T>(&self, term: &Term<T>) -> String {
        let end = Point::new(self.end, Column(term.columns() - 1));
        term.bounds_to_string(Point::new(self.start, Column(0)), end).trim_end().to_string()
    }
}

/// "350 ms", "1,2 s", "2 min 05 s"
pub fn format_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", d.as_secs_f32()).replace('.', ",")
    } else {
        let s = d.as_secs();
        format!("{} min {:02} s", s / 60, s % 60)
    }
}

/// Escribe el script de integración de bash y devuelve su ruta.
pub fn bash_integration_path() -> Option<String> {
    const SCRIPT: &str = include_str!("../shell/garbo.bash");
    let dir = dirs::runtime_dir().or_else(dirs::cache_dir)?.join("garbo");
    let path = dir.join("garbo.bash");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(SCRIPT) {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&path, SCRIPT).ok()?;
    }
    Some(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marcas_del_prompt() {
        let mut sc = MarkScanner::default();
        let mut out = Vec::new();
        sc.scan(b"\x1b]133;A;garbo\x07  ~/x\n\x1b]133;B\x07ls\x1b]133;A\x07$ \x1b]133;D;1\x1b\\", &mut out);
        let marks: Vec<Mark> = out.into_iter().map(|(_, m)| m).collect();
        assert_eq!(marks, [Mark::PromptStart(true), Mark::CommandStart, Mark::PromptStart(false), Mark::CommandEnd(Some(1))]);
    }
}
