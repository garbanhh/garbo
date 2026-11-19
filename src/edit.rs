//! Edición de la línea de órdenes al estilo Warp.
//!
//! bash (readline) ya edita la línea; aquí se añaden los atajos de Warp que readline no tiene, o
//! tiene en otras teclas, traducidos a lo que entiende cada programa:
//! - **Shift+Intro**: salto de línea sin ejecutar. En bash, `Ctrl+V Ctrl+J` (inserción literal: el
//!   comando de varias líneas se ejecuta entero al pulsar Intro). En Claude Code, Codex, Gemini…,
//!   `Ctrl+J`. En Aider, `Esc Intro`.
//! - **Ctrl+Retroceso / Ctrl+Supr**: borrar la palabra de la izquierda / de la derecha.
//! - **Ctrl+Z**: deshacer (en el prompt, bash no tiene nada que suspender).
//! - **Clic en lo escrito**: lleva el cursor ahí, con flechas, como si se pulsaran.
//! - **↑ / ↓ en un comando de varias líneas**: cambian de línea en vez de recorrer el historial.
//!
//! Fuera del prompt y de los agentes, las teclas siguen su camino de siempre.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Term, TermMode};
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Dónde va lo que se teclea.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    /// bash con la integración de Garbo, esperando un comando.
    Prompt,
    /// Un agente en primer plano (por su nombre: "Claude Code", "Aider"…).
    Agent(&'static str),
    Other,
}

/// ¿Puede esta tecla ser un atajo de edición? (Para no averiguar el contexto en cada tecla.)
pub fn is_candidate(key: &Key, mods: ModifiersState) -> bool {
    let ctrl = mods == ModifiersState::CONTROL;
    match key {
        Key::Named(NamedKey::Enter) => mods == ModifiersState::SHIFT,
        Key::Named(NamedKey::Backspace | NamedKey::Delete) => ctrl,
        Key::Character(c) => ctrl && c.eq_ignore_ascii_case("z"),
        _ => false,
    }
}

/// Lo que se manda por un atajo de edición, o `None` si la tecla sigue su camino normal.
pub fn shortcut(key: &Key, mods: ModifiersState, ctx: Context) -> Option<&'static [u8]> {
    let shift = mods == ModifiersState::SHIFT;
    let ctrl = mods == ModifiersState::CONTROL;
    let bytes: &[u8] = match (ctx, key) {
        (Context::Prompt, Key::Named(NamedKey::Enter)) if shift => b"\x16\n",
        // backward-kill-word (Alt+Retroceso) y kill-word (Alt+D): por palabras, como Warp
        // ("~/Documentos/Proyectos" pierde "Proyectos").
        (Context::Prompt, Key::Named(NamedKey::Backspace)) if ctrl => b"\x1b\x7f",
        (Context::Prompt, Key::Named(NamedKey::Delete)) if ctrl => b"\x1bd",
        (Context::Prompt, Key::Character(c)) if ctrl && c.eq_ignore_ascii_case("z") => b"\x1f",
        (Context::Agent("Aider"), Key::Named(NamedKey::Enter)) if shift => b"\x1b\r",
        (Context::Agent(_), Key::Named(NamedKey::Enter)) if shift => b"\n",
        // Ctrl+W: "borrar la palabra anterior" en todos (en Linux, el ^H de Ctrl+Retroceso de
        // siempre solo borra un carácter en Claude Code).
        (Context::Agent(_), Key::Named(NamedKey::Backspace)) if ctrl => b"\x17",
        _ => return None,
    };
    Some(bytes)
}

/// Lo escrito en la rejilla: la posición de cada carácter y, al final, la de después del último.
/// Un salto de línea dentro del comando cuenta como un carácter, justo detrás del texto de su fila.
#[derive(Debug)]
struct Layout {
    pos: Vec<Point>,
    /// `pos[i]` es un salto de línea o el final (no un carácter de la fila).
    end: Vec<bool>,
    cursor: usize,
    /// Primera y última fila de lo escrito.
    first: Line,
    last: Line,
    /// Hay saltos de línea dentro del comando.
    multiline: bool,
}

fn layout<T>(term: &Term<T>, start: (i64, usize)) -> Option<Layout> {
    if term.mode().contains(TermMode::ALT_SCREEN) {
        return None;
    }
    let grid = term.grid();
    let cur = grid.cursor.point;
    let first = Line((start.0 - term.history_size() as i64) as i32);
    let bottom = Line(term.screen_lines() as i32 - 1);
    if first > cur.line || first < term.topmost_line() || !crate::suggest::prompt_before(term, first, start.1) {
        return None;
    }
    let cols = term.columns();
    let blank = |c: char| c == ' ' || c == '\0';
    // Última fila con algo escrito (lo escrito puede seguir por debajo del cursor).
    let mut last = cur.line;
    for l in (cur.line.0 + 1)..=bottom.0 {
        if (0..cols).any(|c| !blank(grid[Line(l)][Column(c)].c)) {
            last = Line(l);
        }
    }
    let mut pos = Vec::new();
    let mut ends = Vec::new();
    let mut text = String::new();
    let mut multiline = false;
    for l in first.0..=last.0 {
        let l = Line(l);
        let row = &grid[l];
        let from = if l == first { start.1 } else { 0 };
        let wraps = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        let mut end = from;
        if !wraps {
            for c in from..cols {
                let cell = &row[Column(c)];
                if !blank(cell.c) {
                    end = c + if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
                }
            }
            if l == cur.line {
                end = end.max(cur.column.0);
            }
        } else {
            end = cols;
        }
        for c in from..end {
            let cell = &row[Column(c)];
            if !cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                pos.push(Point::new(l, Column(c)));
                ends.push(false);
                text.push(if cell.c == '\0' { ' ' } else { cell.c });
            }
        }
        if !wraps {
            // Salto de línea (o, en la última fila, el final de lo escrito).
            pos.push(Point::new(l, Column(end)));
            ends.push(true);
            if l < last {
                multiline = true;
                text.push('\n');
            }
        }
    }
    if crate::suggest::readline_mode(&text) {
        return None;
    }
    let cursor = pos.iter().position(|p| *p == cur)?;
    Some(Layout { pos, end: ends, cursor, first, last, multiline })
}

impl Layout {
    /// Índice del carácter delante del que queda el cursor al pulsar en `p` (lado `side` de la
    /// celda). `None` fuera de lo escrito.
    fn index_at(&self, p: Point, side: Side, start_col: usize) -> Option<usize> {
        if p.line < self.first || p.line > self.last || (p.line == self.first && p.column.0 < start_col) {
            return None;
        }
        let want = p.column.0 + (side == Side::Right) as usize;
        let after = |q: &Point| q.line > p.line || (q.line == p.line && q.column.0 >= want);
        let i = self.pos.iter().position(after).unwrap_or(self.pos.len() - 1);
        // A la derecha del texto de una fila que acaba en salto de línea: delante de ese salto (en
        // una fila partida por el ancho, el siguiente carácter ya es el de la fila de abajo).
        match self.pos.iter().rposition(|q| q.line == p.line) {
            Some(e) if self.end[e] && self.pos[i].line > p.line => Some(e),
            _ => Some(i),
        }
    }
}

/// Flechas para llevar el cursor del carácter `from` al `to` (lo que pulsaría el usuario).
fn arrows(from: usize, to: usize, app: bool) -> Vec<u8> {
    let (n, key) = if to >= from { (to - from, 'C') } else { (from - to, 'D') };
    let one = crate::input::cursor_key(key, ModifiersState::empty(), app);
    one.repeat(n)
}

/// Flechas para llevar el cursor de readline a donde se pulsó (`p`, celda de la rejilla).
pub fn click_moves<T>(term: &Term<T>, start: (i64, usize), p: Point, side: Side) -> Option<Vec<u8>> {
    let lay = layout(term, start)?;
    let to = lay.index_at(p, side, start.1)?;
    let app = term.mode().contains(TermMode::APP_CURSOR);
    (to != lay.cursor).then(|| arrows(lay.cursor, to, app))
}

/// ↑ / ↓ dentro de un comando de varias líneas: flechas hasta la misma columna de la fila de
/// encima o de debajo. `None` si no aplica (una sola línea, o ya en la primera / última fila: ahí
/// las flechas recorren el historial como siempre).
pub fn vertical_moves<T>(term: &Term<T>, start: (i64, usize), up: bool) -> Option<Vec<u8>> {
    let lay = layout(term, start)?;
    if !lay.multiline {
        return None;
    }
    let cur = lay.pos[lay.cursor];
    let line = if up { cur.line - 1 } else { cur.line + 1 };
    if line < lay.first || line > lay.last {
        return None;
    }
    let col = if line == lay.first { cur.column.0.max(start.1) } else { cur.column.0 };
    let to = lay.index_at(Point::new(line, Column(col)), Side::Left, start.1)?;
    let app = term.mode().contains(TermMode::APP_CURSOR);
    Some(arrows(lay.cursor, to, app))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::{Handler, Hyperlink, Processor};

    /// Terminal de 20 columnas con el prompt de Garbo ("  ~" enlazado como prompt) y lo escrito.
    fn term(input: &[u8]) -> (Term<VoidListener>, (i64, usize)) {
        let mut t = Term::new(Config::default(), &TermSize::new(20, 8), VoidListener);
        let mut p: Processor = Processor::new();
        p.advance(&mut t, b"salida\r\n");
        // El enlace del prompt, como lo pone `BlockState` (por OSC 8 el id no admite ":").
        t.set_hyperlink(Some(Hyperlink { id: Some("garbo:p:1".into()), uri: crate::blocks::LINK_URI.into() }));
        p.advance(&mut t, b"  ~");
        t.set_hyperlink(None);
        p.advance(&mut t, b"\r\n");
        let start = crate::suggest::cursor_mark(&t);
        p.advance(&mut t, input);
        (t, start)
    }

    fn feed(t: &mut Term<VoidListener>, bytes: &[u8]) {
        let mut p: Processor = Processor::new();
        p.advance(t, bytes);
    }

    #[test]
    fn atajos_segun_el_contexto() {
        let ctrl = ModifiersState::CONTROL;
        let shift = ModifiersState::SHIFT;
        let enter = Key::Named(NamedKey::Enter);
        let bs = Key::Named(NamedKey::Backspace);
        let z = Key::Character("z".into());
        assert_eq!(shortcut(&enter, shift, Context::Prompt), Some(&b"\x16\n"[..]));
        assert_eq!(shortcut(&enter, shift, Context::Agent("Claude Code")), Some(&b"\n"[..]));
        assert_eq!(shortcut(&enter, shift, Context::Agent("Aider")), Some(&b"\x1b\r"[..]));
        assert_eq!(shortcut(&enter, shift, Context::Other), None, "vim, python…: como siempre");
        assert_eq!(shortcut(&enter, ModifiersState::empty(), Context::Prompt), None);
        assert_eq!(shortcut(&bs, ctrl, Context::Prompt), Some(&b"\x1b\x7f"[..]));
        assert_eq!(shortcut(&bs, ctrl, Context::Agent("Codex")), Some(&b"\x17"[..]));
        assert_eq!(shortcut(&z, ctrl, Context::Prompt), Some(&b"\x1f"[..]));
        assert_eq!(shortcut(&z, ctrl, Context::Agent("Claude Code")), None, "Ctrl+Z llega tal cual");
        assert!(is_candidate(&enter, shift) && is_candidate(&z, ctrl) && !is_candidate(&enter, ModifiersState::empty()));
    }

    #[test]
    fn clic_en_lo_escrito() {
        // "echo hola mundo" con el cursor al final (columna 15 de la fila 2).
        let (t, start) = term(b"echo hola mundo");
        let row = Line(2);
        let at = |c: usize| Point::new(row, Column(c));
        // Clic en la "h" (columna 5): 10 a la izquierda.
        assert_eq!(click_moves(&t, start, at(5), Side::Left), Some(b"\x1b[D".repeat(10)));
        // Lado derecho de la "h": detrás de ella.
        assert_eq!(click_moves(&t, start, at(5), Side::Right), Some(b"\x1b[D".repeat(9)));
        // Más allá del final: el final (ya está ahí, nada que mandar).
        assert_eq!(click_moves(&t, start, at(18), Side::Left), None);
        // Con el cursor en medio ("echo |hola mundo"), más allá del final lleva al final.
        let (mut t, start) = term(b"echo hola mundo");
        feed(&mut t, b"\x1b[3;6H");
        assert_eq!(click_moves(&t, start, at(18), Side::Left), Some(b"\x1b[C".repeat(10)));
        // Fuera de lo escrito (la línea del prompt o la salida anterior): nada.
        assert_eq!(click_moves(&t, start, Point::new(Line(1), Column(2)), Side::Left), None);
        assert_eq!(click_moves(&t, start, Point::new(Line(0), Column(2)), Side::Left), None);
    }

    #[test]
    fn clic_con_varias_lineas_y_anchos() {
        // "ls ñ界\necho fin" (con el salto de línea de bash: el resto sigue en la fila siguiente).
        let (mut t, start) = term("ls ñ界\r\necho fin".as_bytes());
        // Cursor al final de "echo fin" (fila 3, columna 8). Clic en la fila de arriba tras "界".
        assert_eq!(click_moves(&t, start, Point::new(Line(2), Column(15)), Side::Left), Some(b"\x1b[D".repeat(9)));
        // Clic sobre la mitad derecha de "界" (columnas 4-5): detrás de él.
        assert_eq!(click_moves(&t, start, Point::new(Line(2), Column(5)), Side::Right), Some(b"\x1b[D".repeat(9)));
        // ↑ desde la columna 8 de la segunda fila: final de la primera (después de "界").
        assert_eq!(vertical_moves(&t, start, true), Some(b"\x1b[D".repeat(9)));
        assert_eq!(vertical_moves(&t, start, false), None, "en la última fila, ↓ recorre el historial");
        // Con el cursor en la primera fila, ↓ baja a la misma columna.
        feed(&mut t, b"\x1b[3;3H");
        assert_eq!(vertical_moves(&t, start, false), Some(b"\x1b[C".repeat(6)));
        assert_eq!(vertical_moves(&t, start, true), None);
    }

    #[test]
    fn una_linea_larga_partida_por_el_ancho() {
        // 25 caracteres en 20 columnas: la fila se parte sola (no es un salto de línea).
        let (t, start) = term(b"echo aaaaaaaaaaaaaaaaaaaa");
        assert_eq!(vertical_moves(&t, start, true), None, "sin saltos de línea, ↑ es el historial");
        // Clic en la columna 0 de la primera fila: 25 a la izquierda.
        assert_eq!(click_moves(&t, start, Point::new(Line(2), Column(0)), Side::Left), Some(b"\x1b[D".repeat(25)));
        // Lado derecho de la última celda de la primera fila: delante del primer carácter de la segunda.
        assert_eq!(click_moves(&t, start, Point::new(Line(2), Column(19)), Side::Right), Some(b"\x1b[D".repeat(5)));
    }

    #[test]
    fn busqueda_de_readline_y_pantalla_alternativa() {
        let (t, start) = term(b"(reverse-i-search)`ec': echo hola");
        assert_eq!(click_moves(&t, start, Point::new(Line(2), Column(2)), Side::Left), None);
        let (mut t, start) = term(b"echo");
        feed(&mut t, b"\x1b[?1049h");
        assert_eq!(click_moves(&t, start, Point::new(Line(2), Column(1)), Side::Left), None);
    }
}
