//! Bloques de comandos: cada comando y su salida forman una unidad con estado y duración.
//!
//! El shell (con el script de integración, `shell/`) emite marcas OSC 133 invisibles:
//!   A = empieza el prompt, B = empieza el comando, C = empieza la salida, D;código = terminó.
//!   `A;garbo` = el prompt es el de Garbo (se dibuja como etiqueta, ver `view::draw_prompts`).
//!   `P;k=r` = empieza el prompt derecho (zsh, fish): sus celdas no son parte del comando.
//! Si el shell vuelve a pintar el mismo prompt (fish al arrancar, zsh con `reset-prompt`), la
//! nueva A sigue en el mismo bloque.
//! Además, OSC 7 (`file://equipo/ruta`) y OSC 9;9 (`"ruta"`) informan de la carpeta del shell:
//! en Windows es la única forma de saberla con PowerShell, que no cambia la del proceso con `cd`.
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
    /// Prompt derecho; lo cierra la siguiente `CommandStart`.
    RightPrompt,
    CommandStart,
    OutputStart,
    CommandEnd(Option<i32>),
    /// Carpeta del shell (OSC 7 u OSC 9;9): la siguiente de `MarkScanner::cwds`.
    Cwd,
}

/// Longitud máxima de la carpeta que informa el shell (OSC 7, con su `file://equipo`).
const MAX_CWD: usize = 4096;

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
    /// Carpetas informadas por el shell, una por cada `Mark::Cwd`, en orden.
    pub cwds: VecDeque<PathBuf>,
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
        let mut i = 0;
        while i < bytes.len() {
            if self.state == Scan::Ground {
                // Fuera de un OSC solo importa ESC ]: se salta hasta el siguiente ']' (el
                // estado solo cambia si el byte anterior, ya dentro de este tramo, es ESC).
                // Sin ']', se conserva un ESC final por si la secuencia sigue en otra lectura.
                let Some(k) = memchr::memchr(b']', &bytes[i..]) else {
                    if bytes.last() == Some(&0x1b) {
                        self.state = Scan::Esc;
                    }
                    return;
                };
                let at = i + k;
                if k > 0 && bytes[at - 1] == 0x1b {
                    self.buf.clear();
                    self.overflow = false;
                    self.state = Scan::Osc;
                }
                i = at + 1;
                continue;
            }
            let b = bytes[i];
            self.state = match self.state {
                Scan::Ground => Scan::Ground, // se trata arriba
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
                        // Las marcas caben en 32 bytes; un informe OSC 7501, en
                        // `status::MAX_REPORT`; una carpeta, en `MAX_CWD`.
                        let cap = if self.buf.starts_with(b"7501;") {
                            crate::status::MAX_REPORT + 5
                        } else if self.buf.starts_with(b"7;") || self.buf.starts_with(b"9;9;") {
                            MAX_CWD
                        } else {
                            32
                        };
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
            i += 1;
        }
    }

    fn finish(&mut self, end: usize, out: &mut Vec<(usize, Mark)>, st: bool) {
        if let Some(payload) = self.buf.strip_prefix(b"7501;") {
            if let Some(report) = (!self.overflow)
                .then(|| crate::status::parse(payload))
                .flatten()
            {
                self.status.push((report, st));
            }
            return;
        }
        let cwd = if let Some(uri) = self.buf.strip_prefix(b"7;") {
            Some(cwd_from_osc7(uri))
        } else {
            self.buf.strip_prefix(b"9;9;").map(cwd_from_osc9)
        };
        if let Some(cwd) = cwd {
            if let Some(path) = cwd.filter(|_| !self.overflow) {
                self.cwds.push_back(path);
                out.push((end, Mark::Cwd));
            }
            return;
        }
        let Some(rest) = self.buf.strip_prefix(b"133;") else {
            return;
        };
        let mark = match rest.first() {
            Some(b'A') => Mark::PromptStart(rest[1..].split(|&b| b == b';').any(|p| p == b"garbo")),
            Some(b'B') => Mark::CommandStart,
            Some(b'P') if rest[1..].split(|&b| b == b';').any(|p| p == b"k=r") => Mark::RightPrompt,
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

/// Carpeta de un OSC 7 (`file://equipo/ruta`, con `%XX`). Solo de este equipo: con `ssh` el shell
/// remoto informaría de rutas que aquí no existen.
fn cwd_from_osc7(uri: &[u8]) -> Option<PathBuf> {
    let uri = std::str::from_utf8(uri).ok()?;
    let rest = uri.strip_prefix("file://")?;
    let slash = rest.find('/')?;
    let host = &rest[..slash];
    if !(host.is_empty() || host.eq_ignore_ascii_case("localhost") || is_this_host(host)) {
        return None;
    }
    native_path(&crate::links::percent_decode(&rest[slash..]))
}

/// Carpeta de un OSC 9;9 (ConEmu, Windows Terminal): la ruta tal cual, quizá entre comillas.
fn cwd_from_osc9(path: &[u8]) -> Option<PathBuf> {
    let path = std::str::from_utf8(path).ok()?;
    let path = path
        .strip_prefix('"')
        .and_then(|p| p.strip_suffix('"'))
        .unwrap_or(path);
    native_path(path)
}

/// Ruta del sistema a partir de la de una URL: en Windows `/C:/Users/ana` -> `C:\Users\ana`.
fn native_path(path: &str) -> Option<PathBuf> {
    if path.is_empty() || path.contains(|c: char| c.is_control()) {
        return None;
    }
    if cfg!(windows) {
        let p = path.strip_prefix('/').unwrap_or(path);
        let b = p.as_bytes();
        let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
        let unc = p.starts_with("\\\\") || p.starts_with("//");
        if !(drive || unc) {
            return None;
        }
        let mut p = p.replace('/', "\\");
        if p.len() == 2 {
            p.push('\\');
        }
        Some(PathBuf::from(p))
    } else {
        path.starts_with('/').then(|| PathBuf::from(path))
    }
}

/// ¿Es `host` el nombre de este equipo?
fn is_this_host(host: &str) -> bool {
    let name = if cfg!(windows) {
        std::env::var("COMPUTERNAME").unwrap_or_default()
    } else {
        std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default()
    };
    let name = name.trim();
    !name.is_empty() && name.eq_ignore_ascii_case(host)
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
    /// Última carpeta que informó el shell (OSC 7 u OSC 9;9).
    reported_cwd: Option<PathBuf>,
    /// Carpeta del shell cuando empezó a escribirse el comando (marca B).
    input_cwd: Option<PathBuf>,
    /// Estado que informa el programa en marcha (OSC 7501, ver `status.rs`).
    pub program: crate::status::Records,
    /// ConPTY pidió las teclas como eventos de Windows (`ESC[?9001h`, ver `input::win32_key`).
    #[cfg(windows)]
    pub win32_input: bool,
    /// Dónde estaba el cursor en la última marca A (para reconocer el mismo prompt repintado).
    prompt_at: Option<(i64, usize)>,
    /// Se está escribiendo el prompt derecho: la próxima B solo lo cierra.
    right_prompt: bool,
}

impl BlockState {
    pub fn new(shell_pid: u32) -> Self {
        Self {
            shell_pid,
            ..Default::default()
        }
    }

    /// Carpeta actual del shell. En Linux, la del proceso (`/proc/<pid>/cwd`), que siempre está al
    /// día; si no se puede leer, la que informó. En Windows primero la informada: PowerShell no
    /// cambia la del proceso con `cd`, y la de Git Bash es la de MSYS.
    pub fn shell_cwd(&self) -> Option<PathBuf> {
        let process = || {
            (self.shell_pid != 0)
                .then(|| crate::platform::process_cwd(self.shell_pid as i32))
                .flatten()
        };
        let reported = || self.reported_cwd.clone().filter(|p| p.is_dir());
        if cfg!(windows) {
            reported().or_else(process)
        } else {
            process().or_else(reported)
        }
    }

    /// El shell informó de su carpeta (OSC 7 u OSC 9;9).
    pub fn set_cwd(&mut self, path: PathBuf) {
        self.reported_cwd = Some(path);
    }

    pub fn info(&self, id: u32) -> Option<&BlockInfo> {
        let i = self.infos.binary_search_by_key(&id, |b| b.id).ok()?;
        self.infos.get(i)
    }

    /// Bloque anterior a `id`.
    fn prev_id(&self, id: u32) -> Option<u32> {
        let i = self.infos.partition_point(|b| b.id < id);
        Some(self.infos.get(i.checked_sub(1)?)?.id)
    }

    /// Para las pruebas: (prompt de Garbo, se ejecutó, código) de cada bloque.
    #[cfg(test)]
    pub fn summary(&self) -> Vec<(bool, bool, Option<i32>)> {
        self.infos
            .iter()
            .map(|b| (b.styled, b.started.is_some(), b.exit))
            .collect()
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
        self.infos
            .iter()
            .rev()
            .find(|b| b.started.is_some())
            .map(|b| b.id)
    }

    /// El bloque actual, si una A en `here` es solo el mismo prompt pintado otra vez: aún no se
    /// ha ejecutado nada y el cursor sigue donde la A anterior (nada escrito entre las dos) o en
    /// una línea con celdas de su prompt (volvió al principio para repintarlo). Un Intro en vacío
    /// deja el prompt viejo en la línea de arriba, así que ese sí abre otro bloque.
    fn repainted<T>(&mut self, term: &Term<T>, here: (i64, usize)) -> Option<&mut BlockInfo> {
        let same_spot = self.prompt_at == Some(here);
        let info = self.current_mut().filter(|b| b.started.is_none())?;
        let id = info.id;
        let row = &term.grid()[term.grid().cursor.point.line];
        let on_prompt = || {
            row[..]
                .iter()
                .filter(|c| c.extra.is_some())
                .any(|c| cell_link(c) == Some((b'p', id)))
        };
        if !same_spot && !on_prompt() {
            return None;
        }
        self.current_mut()
    }

    fn current_mut(&mut self) -> Option<&mut BlockInfo> {
        let id = self.current?;
        let i = self.infos.binary_search_by_key(&id, |b| b.id).ok()?;
        self.infos.get_mut(i)
    }

    /// Aplica una marca. Se llama con el terminal bloqueado y el parser justo en la marca.
    pub fn apply<T: EventListener>(&mut self, mark: Mark, term: &mut Term<T>) {
        // La carpeta la aplica el bucle del PTY (`set_cwd`) y no dice si hay integración: fish
        // y otros la informan por su cuenta.
        if mark == Mark::Cwd {
            return;
        }
        self.active = true;
        let link = |kind: char, id: u32| Hyperlink {
            id: Some(format!("garbo:{kind}:{id}")),
            uri: LINK_URI.into(),
        };
        match mark {
            Mark::PromptStart(styled) => {
                // Vuelve el prompt: el programa que informaba ya no está en marcha.
                self.program.drop_live();
                self.right_prompt = false;
                let here = crate::suggest::cursor_mark(term);
                let id = match self.repainted(term, here) {
                    Some(info) => {
                        info.styled = styled;
                        info.id
                    }
                    None => {
                        self.next_id += 1;
                        let id = self.next_id;
                        self.infos.push_back(BlockInfo {
                            id,
                            styled,
                            started: None,
                            duration: None,
                            exit: None,
                        });
                        if self.infos.len() > MAX_BLOCKS {
                            self.infos.pop_front();
                        }
                        id
                    }
                };
                self.prompt_at = Some(here);
                self.current = Some(id);
                self.input_start = None;
                term.set_hyperlink(Some(link('p', id)));
            }
            Mark::RightPrompt => {
                if let Some(id) = self.current {
                    term.set_hyperlink(Some(link('r', id)));
                    self.right_prompt = true;
                }
            }
            Mark::CommandStart if self.right_prompt => {
                // Fin del prompt derecho: lo que se escriba vuelve a ser el comando, que empieza
                // donde dijo la B del prompt.
                self.right_prompt = false;
                if let Some(id) = self.current {
                    term.set_hyperlink(Some(link('c', id)));
                }
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
                self.right_prompt = false;
                term.set_hyperlink(None);
                // El comando aceptado pasará al historial de sugerencias cuando termine.
                let dir = self.input_cwd.take();
                self.pending_cmd = self
                    .input_start
                    .take()
                    .and_then(|s| crate::suggest::executed_command(term, s))
                    .map(|c| (c, dir));
                if let Some(info) = self.current_mut() {
                    info.started = Some(Instant::now());
                }
            }
            Mark::CommandEnd(code) => {
                self.right_prompt = false;
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
            Mark::Cwd => {}
        }
    }
}

/// ¿Es la celda parte de un prompt derecho? No cuenta como texto escrito.
pub fn is_right_prompt(cell: &Cell) -> bool {
    cell.extra.is_some() && matches!(cell_link(cell), Some((b'r', _)))
}

/// (tipo, id) del ancla de bloque de una celda: tipo `p` = prompt, `c` = comando, `r` = prompt
/// derecho.
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
    term.grid()[line][..]
        .iter()
        .take(term.columns())
        .filter(|cell| cell.extra.is_some())
        .find_map(|cell| match cell_link(cell) {
            Some((b'p', id)) => Some(id),
            _ => None,
        })
}

/// Bloque que empieza por encima de `top` (para decorar su parte visible); `first` es la
/// primera ancla de la pantalla. Los bloques aparecen en la rejilla en el orden de sus ids, así
/// que casi siempre se deduce sin recorrer el historial: es el anterior a `first` o, al final
/// del historial y sin anclas a la vista, el actual. Solo con el historial desplazado y sin
/// anclas a la vista se busca hacia arriba.
pub fn block_above<T>(
    term: &Term<T>,
    bs: &BlockState,
    top: Line,
    first: Option<&Anchor>,
) -> Option<Anchor> {
    use alacritty_terminal::term::TermMode;
    if term.mode().contains(TermMode::ALT_SCREEN) {
        return None;
    }
    let id = match first {
        Some(a) => bs.prev_id(a.id)?,
        None if term.grid().display_offset() == 0 => bs.current?,
        None => return anchor_at(term, top, 2000),
    };
    // La línea solo indica que empieza fuera de la pantalla.
    Some(Anchor { line: top - 1, id })
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
            if out.last().is_none_or(|a| a.id != id) {
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
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
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
        term.bounds_to_string(Point::new(start, Column(0)), end)
            .trim_end()
            .to_string()
    }

    pub fn whole<T>(&self, term: &Term<T>) -> String {
        let end = Point::new(self.end, Column(term.columns() - 1));
        term.bounds_to_string(Point::new(self.start, Column(0)), end)
            .trim_end()
            .to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conserva_marcas_y_offsets_entre_lecturas() {
        let mut bytes = vec![b'x'; 100];
        bytes.extend_from_slice(b"\x1b[32mtexto\x1b[0m\x1b]133;B\x07");
        let command_end = bytes.len();
        bytes.extend_from_slice(b"\x1b]7501;?\x1b\\");
        bytes.extend_from_slice(&[b' '; 128]);
        bytes.extend_from_slice(b"]literal\x1b]133;D;2\x1b\\");

        // Prueba cada corte, incluidos ESC al final de un lote y ST partido.
        for cut in 0..=bytes.len() {
            let mut scanner = MarkScanner::default();
            let mut marks = Vec::new();
            scanner.scan(&bytes[..cut], &mut marks);
            let first = marks.len();
            scanner.scan(&bytes[cut..], &mut marks);
            for (end, _) in &mut marks[first..] {
                *end += cut;
            }
            assert_eq!(
                marks,
                [
                    (command_end, Mark::CommandStart),
                    (bytes.len(), Mark::CommandEnd(Some(2)))
                ],
                "corte {cut}"
            );
            assert_eq!(scanner.status, [(crate::status::Report::Query, true)]);
        }
    }

    #[test]
    fn el_bloque_de_arriba_coincide_con_la_busqueda_completa() {
        use alacritty_terminal::event::VoidListener;
        use alacritty_terminal::grid::Scroll;
        use alacritty_terminal::term::test::TermSize;
        use alacritty_terminal::term::TermMode;

        let mut term = Term::new(
            alacritty_terminal::term::Config::default(),
            &TermSize::new(40, 10),
            VoidListener,
        );
        let blocks = std::sync::Mutex::new(BlockState::new(0));
        let mut feed = |s: &str| {
            crate::pty_loop::feed_for_tests(&mut term, &blocks, s.as_bytes(), &VoidListener)
        };
        // Salida previa sin bloque y tres comandos con salidas de distinta longitud; el último
        // sigue en marcha.
        feed("arranque\r\nbanner\r\n");
        for n in [3, 25, 60] {
            feed("\x1b]133;A\x07$ \x1b]133;B\x07orden\r\n\x1b]133;C\x07");
            for i in 0..n {
                feed(&format!("línea {i}\r\n"));
            }
            if n != 60 {
                feed("\x1b]133;D;0\x07");
            }
        }
        let bs = blocks.lock().unwrap();
        let mut checked = 0;
        for delta in 0..term.history_size() as i32 {
            term.scroll_display(Scroll::Bottom);
            term.scroll_display(Scroll::Delta(delta));
            let top = Line(-(term.grid().display_offset() as i32));
            let bottom = top + 9;
            let anchors = anchors(&term, top, bottom);
            if anchors.first().is_some_and(|a| a.line == top) {
                continue;
            }
            let fast = block_above(&term, &bs, top, anchors.first()).map(|a| a.id);
            let slow = anchor_at(&term, top, usize::MAX).map(|a| a.id);
            assert_eq!(fast, slow, "desplazamiento {delta}");
            checked += 1;
        }
        assert!(checked > 50);
        // Con la pantalla alternativa no se decora nada.
        term.scroll_display(Scroll::Bottom);
        drop(bs);
        crate::pty_loop::feed_for_tests(&mut term, &blocks, b"\x1b[?1049h", &VoidListener);
        assert!(term.mode().contains(TermMode::ALT_SCREEN));
        let bs = blocks.lock().unwrap();
        assert_eq!(block_above(&term, &bs, Line(0), None).map(|a| a.id), None);
    }

    #[test]
    fn mismo_resultado_entero_que_byte_a_byte() {
        // Varios ']' sueltos, ESC ESC ], un OSC cortado por otro y marcas con BEL y con ST.
        let bytes =
            b"a]b\x1b\x1b]133;A\x07 ]] \x1b[1m]\x1b]133;B\x1b\\ x]\x1b]133;C\x1b]133;D;0\x07]\x1b";
        let mut whole = MarkScanner::default();
        let mut marks = Vec::new();
        whole.scan(bytes, &mut marks);
        let mut single = MarkScanner::default();
        let mut by_byte = Vec::new();
        for (i, b) in bytes.iter().enumerate() {
            let first = by_byte.len();
            single.scan(std::slice::from_ref(b), &mut by_byte);
            for (end, _) in &mut by_byte[first..] {
                *end += i;
            }
        }
        assert_eq!(marks, by_byte);
        assert_eq!(
            marks.iter().map(|m| m.1).collect::<Vec<_>>(),
            [
                Mark::PromptStart(false),
                Mark::CommandStart,
                Mark::CommandEnd(Some(0))
            ]
        );
        assert!(whole.state == Scan::Esc && single.state == Scan::Esc);
    }

    #[test]
    fn marcas_del_prompt() {
        let mut sc = MarkScanner::default();
        let mut out = Vec::new();
        sc.scan(
            b"\x1b]133;A;garbo\x07  ~/x\n\x1b]133;B\x07ls\x1b]133;A\x07$ \x1b]133;D;1\x1b\\",
            &mut out,
        );
        let marks: Vec<Mark> = out.into_iter().map(|(_, m)| m).collect();
        assert_eq!(
            marks,
            [
                Mark::PromptStart(true),
                Mark::CommandStart,
                Mark::PromptStart(false),
                Mark::CommandEnd(Some(1))
            ]
        );
    }
}
