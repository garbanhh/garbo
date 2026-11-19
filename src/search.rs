//! Buscar en el historial de la pestaña (Ctrl+Shift+F).
//!
//! Una barra arriba a la derecha del terminal. Lo que se escribe se busca tal cual (sin
//! expresiones regulares) y sin distinguir mayúsculas salvo que se escriba alguna, como en
//! Alacritty. Todas las coincidencias visibles se resaltan y la actual destaca. Intro o ↑ van a la
//! anterior (más arriba), Shift+Intro o ↓ a la siguiente, y Esc cierra. Si al abrir había un bloque
//! seleccionado, se busca solo en él (la etiqueta "Bloque" lo quita o lo vuelve a poner).
//!
//! Las coincidencias se guardan en coordenadas absolutas (línea + historial), así siguen en su
//! sitio aunque llegue salida nueva. Lo resaltado se calcula en cada fotograma, solo en lo visible.

use crate::config::Theme;
use crate::view::Painter;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Direction, Line, Point};
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::Term;

/// Como mucho se cuentan estas (se muestra "10000+").
const MAX_MATCHES: usize = 10_000;
/// Como mucho se resaltan estas en pantalla.
const MAX_VISIBLE: usize = 2_000;

/// Coincidencia en coordenadas absolutas: (línea + historial, columna) de inicio y de fin.
type Abs = ((i64, usize), (i64, usize));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Field,
    Prev,
    Next,
    Close,
    Block,
}

pub struct Search {
    pub tab: u64,
    pub query: String,
    regex: Option<RegexSearch>,
    matches: Vec<Abs>,
    index: Option<usize>,
    /// Bloque seleccionado al abrir; `block_only` limita la búsqueda a él.
    pub block: Option<u32>,
    pub block_only: bool,
    /// Líneas absolutas del bloque (mientras se busca solo en él).
    range: Option<(i64, i64)>,
    /// Zonas pulsables de la barra (se calculan al dibujarla).
    pub hits: Vec<(Hit, [f32; 4])>,
}

/// Lo que se resalta en el fotograma actual.
#[derive(Default)]
pub struct Highlights {
    pub all: Vec<Match>,
    pub current: Option<Match>,
}

/// El texto se busca literal: se escapan los caracteres especiales de las expresiones regulares.
fn escape(q: &str) -> String {
    let mut out = String::with_capacity(q.len() + 8);
    for c in q.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn abs<T>(term: &Term<T>, p: Point) -> (i64, usize) {
    (p.line.0 as i64 + term.history_size() as i64, p.column.0)
}

fn point<T>(term: &Term<T>, a: (i64, usize)) -> Point {
    Point::new(Line((a.0 - term.history_size() as i64) as i32), Column(a.1))
}

impl Search {
    pub fn new(tab: u64, block: Option<u32>) -> Self {
        Self {
            tab,
            query: String::new(),
            regex: None,
            matches: Vec::new(),
            index: None,
            block,
            block_only: block.is_some(),
            range: None,
            hits: Vec::new(),
        }
    }

    /// Cambió lo buscado o el ámbito: busca de nuevo y va a la coincidencia más cercana por encima
    /// del final de lo que se ve (la más reciente, si no se ha desplazado).
    pub fn update<T: EventListener>(&mut self, term: &mut Term<T>) {
        self.regex = (!self.query.is_empty()).then(|| RegexSearch::new(&escape(&self.query)).ok()).flatten();
        self.refresh(term);
        let bottom = term.screen_lines() as i64 - 1 - term.grid().display_offset() as i64 + term.history_size() as i64;
        self.index = self.matches.iter().rposition(|m| m.0 .0 <= bottom).or((!self.matches.is_empty()).then_some(0));
        self.reveal(term);
    }

    /// Recalcula todas las coincidencias (en todo el historial o en el bloque).
    fn refresh<T>(&mut self, term: &Term<T>) {
        self.matches.clear();
        self.range = None;
        let Some(regex) = self.regex.as_mut() else { return };
        let last_col = Column(term.columns() - 1);
        let mut start = Point::new(term.topmost_line(), Column(0));
        let mut end = Point::new(term.bottommost_line(), last_col);
        if let Some(r) = self.block.filter(|_| self.block_only).and_then(|id| crate::blocks::find_block(term, id)) {
            start = Point::new(r.start, Column(0));
            end = Point::new(r.end, last_col);
            self.range = Some((abs(term, start).0, abs(term, end).0));
        }
        for m in RegexIter::new(start, end, Direction::Right, term, regex).take(MAX_MATCHES) {
            self.matches.push((abs(term, *m.start()), abs(term, *m.end())));
        }
    }

    /// Va a la coincidencia anterior (`up`, más arriba) o a la siguiente, dando la vuelta.
    pub fn step<T: EventListener>(&mut self, term: &mut Term<T>, up: bool) {
        let cur = self.current();
        self.refresh(term); // pudo llegar salida nueva
        let n = self.matches.len();
        if n == 0 {
            self.index = None;
            return;
        }
        let i = cur.and_then(|c| self.matches.iter().position(|m| *m == c));
        self.index = Some(match (i, up) {
            (Some(i), true) => (i + n - 1) % n,
            (Some(i), false) => (i + 1) % n,
            (None, _) => n - 1,
        });
        self.reveal(term);
    }

    fn current(&self) -> Option<Abs> {
        self.index.and_then(|i| self.matches.get(i).copied())
    }

    /// Desplaza la vista para que se vea la coincidencia actual.
    fn reveal<T: EventListener>(&self, term: &mut Term<T>) {
        if let Some((a, _)) = self.current() {
            term.scroll_to_point(point(term, a));
        }
    }

    /// "3/17", "Sin resultados"…
    pub fn counter(&self) -> String {
        let n = self.matches.len();
        let more = if n >= MAX_MATCHES { "+" } else { "" };
        match self.index {
            _ if self.query.is_empty() => String::new(),
            _ if n == 0 => "Sin resultados".into(),
            Some(i) => format!("{}/{n}{more}", i + 1),
            None => format!("{n}{more}"),
        }
    }

    /// Coincidencias de lo que se ve, para resaltarlas, y la actual.
    pub fn highlights<T>(&mut self, term: &Term<T>) -> Highlights {
        let mut h = Highlights::default();
        let current = self.current();
        let Some(regex) = self.regex.as_mut() else { return h };
        let off = term.grid().display_offset() as i32;
        let hs = term.history_size() as i64;
        let mut top = Line(-off);
        let mut bottom = Line(term.screen_lines() as i32 - 1 - off);
        if let Some((a, b)) = self.range {
            top = top.max(Line((a - hs) as i32));
            bottom = bottom.min(Line((b - hs) as i32));
        }
        if top <= bottom {
            let (start, end) = (Point::new(top, Column(0)), Point::new(bottom, Column(term.columns() - 1)));
            h.all = RegexIter::new(start, end, Direction::Right, term, regex).take(MAX_VISIBLE).collect();
        }
        h.current = current.map(|(a, b)| point(term, a)..=point(term, b));
        h
    }

    /// Barra de búsqueda arriba a la derecha de la zona del terminal (`x`, `y`, ancho `area_w`).
    pub fn draw(&mut self, p: &mut Painter, theme: &Theme, x: f32, y: f32, area_w: f32, scale: f32) {
        let s = scale;
        let w = (460.0 * s).min(area_w - 16.0 * s).round();
        let h = (p.ui_metrics().height + 16.0 * s).round();
        let bx = (x + area_w - w - 12.0 * s).round();
        let by = (y + 8.0 * s).round();
        p.card(bx, by, w, h, crate::ui::surface(theme, 0.06), theme.foreground, (9.0 * s).round(), s);
        let dimc = crate::ui::dim(theme);
        let cy = by + h / 2.0;
        self.hits.clear();

        // Derecha a izquierda: cerrar, siguiente, anterior, "Bloque" y el contador.
        let btn = (28.0 * s).round();
        let mut right = bx + w - 6.0 * s;
        let button = |p: &mut Painter, hits: &mut Vec<(Hit, [f32; 4])>, hit: Hit, right: &mut f32| {
            *right -= btn;
            let (cx, k, t) = (*right + btn / 2.0, 4.0 * s, 1.5 * s);
            match hit {
                Hit::Close => crate::ui::cross(p, cx, cy, k * 0.85, t, theme.foreground),
                Hit::Prev => {
                    p.line(cx - k, cy + k * 0.5, cx, cy - k * 0.5, t, theme.foreground, 1.0);
                    p.line(cx, cy - k * 0.5, cx + k, cy + k * 0.5, t, theme.foreground, 1.0);
                }
                _ => {
                    p.line(cx - k, cy - k * 0.5, cx, cy + k * 0.5, t, theme.foreground, 1.0);
                    p.line(cx, cy + k * 0.5, cx + k, cy - k * 0.5, t, theme.foreground, 1.0);
                }
            }
            hits.push((hit, [*right, by, btn, h]));
        };
        button(p, &mut self.hits, Hit::Close, &mut right);
        button(p, &mut self.hits, Hit::Next, &mut right);
        button(p, &mut self.hits, Hit::Prev, &mut right);
        right -= 6.0 * s;
        if self.block.is_some() {
            let label = "Bloque";
            let tw = p.ui_measure(label);
            let (pw, ph) = (tw + 16.0 * s, h - 12.0 * s);
            right -= pw;
            let (fill, alpha, color) = if self.block_only { (theme.accent, 0.25, theme.foreground) } else { (theme.foreground, 0.06, dimc) };
            p.rounded(right, by + 6.0 * s, pw, ph, fill, alpha, (ph / 2.0).round());
            p.ui_text_in(right + 8.0 * s, by, h, label, color, tw + 2.0);
            self.hits.push((Hit::Block, [right, by, pw, h]));
            right -= 8.0 * s;
        }
        let counter = self.counter();
        if !counter.is_empty() {
            let cw = p.ui_measure(&counter);
            right -= cw;
            let color = if self.matches.is_empty() { theme.palette[1] } else { dimc };
            p.ui_text_in(right, by, h, &counter, color, cw + 2.0);
            right -= 10.0 * s;
        }

        // Izquierda: lupa y lo escrito, con el cursor detrás.
        crate::ui::magnifier(p, bx + 18.0 * s, cy, s, dimc);
        let fx = bx + 32.0 * s;
        let room = (right - fx).max(0.0);
        self.hits.push((Hit::Field, [bx, by, (right - bx).max(0.0), h]));
        let drawn = if self.query.is_empty() {
            p.ui_text_in(fx, by, h, "Buscar", dimc, room);
            0.0
        } else {
            p.ui_text_in(fx, by, h, &self.query, theme.foreground, room)
        };
        let lh = p.ui_metrics().height;
        p.rect((fx + drawn + 1.0).min(fx + room), (cy - lh / 2.0).round(), (1.5 * s).max(1.0), lh.round(), theme.cursor, 1.0);
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits.iter().find(|(_, [hx, hy, hw, hh])| x >= *hx && x < hx + hw && y >= *hy && y < hy + hh).map(|(h, _)| *h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    fn term(text: &str) -> Term<VoidListener> {
        let mut t = Term::new(Config::default(), &TermSize::new(30, 5), VoidListener);
        let mut p: Processor = Processor::new();
        p.advance(&mut t, text.replace('\n', "\r\n").as_bytes());
        t
    }

    #[test]
    fn busca_literal_y_recorre() {
        // 8 líneas en 5 de pantalla: hay historial.
        let mut t = term("error: uno\nok\nError: dos\nok\nlinea (a+b)\nerror: tres\nok\nfin");
        let mut s = Search::new(1, None);
        s.query = "error".into();
        s.update(&mut t);
        assert_eq!(s.counter(), "3/3", "sin mayúsculas no distingue; empieza por la última");
        s.step(&mut t, true);
        assert_eq!(s.counter(), "2/3");
        s.step(&mut t, true);
        s.step(&mut t, true);
        assert_eq!(s.counter(), "3/3", "da la vuelta");
        s.query = "Error".into();
        s.update(&mut t);
        assert_eq!(s.counter(), "1/1", "con mayúscula, sí distingue");
        // Al ir a la primera coincidencia (en el historial), la vista sube.
        assert!(t.grid().display_offset() > 0);
        s.query = "(a+b)".into();
        s.update(&mut t);
        assert_eq!(s.counter(), "1/1", "se busca literal, no como expresión regular");
        s.query = "nada".into();
        s.update(&mut t);
        assert_eq!(s.counter(), "Sin resultados");
        s.query.clear();
        s.update(&mut t);
        assert_eq!(s.counter(), "");
    }

    #[test]
    fn resalta_lo_visible() {
        let mut t = term("aa bb aa\nbb aa");
        let mut s = Search::new(1, None);
        s.query = "aa".into();
        s.update(&mut t);
        let h = s.highlights(&t);
        assert_eq!(h.all.len(), 3);
        assert_eq!(h.current.as_ref().map(|m| *m.start()), Some(Point::new(Line(1), Column(3))), "la última");
    }
}
