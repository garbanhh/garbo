//! Convierte el estado del terminal en instancias para la GPU.

use crate::atlas::{GlyphCache, UiVariant};
use crate::blocks::{self, BlockState, Status};
use crate::complete::{Kind, Menu, MENU_ROWS};
use crate::config::{Rgb, Theme};
use crate::font::{CellMetrics, Style};
use crate::renderer::Instance;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, NamedColor};

pub struct Painter<'a> {
    /// Capa inferior: fondos, cursor, paneles.
    pub out: &'a mut Vec<Instance>,
    /// Capa superior: texto. Se concatena después de `out`.
    pub fg: &'a mut Vec<Instance>,
    pub glyphs: &'a mut GlyphCache,
    pub queue: &'a wgpu::Queue,
    pub atlas: &'a wgpu::Texture,
}

impl Painter<'_> {
    pub fn metrics(&self) -> CellMetrics {
        self.glyphs.fonts.metrics
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: Rgb, a: f32) {
        self.out.push(Instance::rect(x, y, w, h, c.to_f32(a)));
    }

    pub fn rounded(&mut self, x: f32, y: f32, w: f32, h: f32, c: Rgb, a: f32, r: f32) {
        self.out.push(Instance::rounded(x, y, w, h, c.to_f32(a), r));
    }

    /// Dibuja un glifo cuya celda empieza en (x, y).
    #[allow(clippy::too_many_arguments)]
    pub fn glyph(&mut self, ch: char, style: Style, wide: bool, emoji: bool, x: f32, y: f32, color: Rgb, alpha: f32) {
        let m = self.metrics();
        if let Some(g) = self.glyphs.get(self.queue, self.atlas, ch, style, wide, emoji) {
            let gx = if g.is_color {
                x + ((if wide { 2.0 } else { 1.0 }) * m.width - g.width) / 2.0
            } else {
                x + g.left
            };
            self.fg.push(Instance {
                pos: [gx.round(), (y + m.baseline - g.top).round()],
                size: [g.width, g.height],
                uv: g.uv,
                color: color.to_f32(alpha),
                kind: if g.is_color { 2 } else { 1 },
                radius: 0.0,
            });
        }
    }

    /// Texto de UI en una línea (celda empezando en y). Devuelve el ancho dibujado.
    pub fn text(&mut self, x: f32, y: f32, s: &str, color: Rgb, max_w: f32) -> f32 {
        let cw = self.metrics().width;
        let n = s.chars().count();
        let fit = (max_w / cw).floor().max(0.0) as usize;
        let mut cx = x;
        for (i, ch) in s.chars().enumerate() {
            if i >= fit {
                break;
            }
            let ch = if n > fit && i + 1 == fit { '…' } else { ch };
            if ch != ' ' {
                self.glyph(ch, Style::Regular, false, false, cx, y, color, 1.0);
            }
            cx += cw;
        }
        cx - x
    }

    // ---------- Interfaz: texto proporcional e iconos vectoriales ----------

    pub fn ui_metrics(&self) -> CellMetrics {
        self.glyphs.ui.metrics
    }

    /// Alto de línea de una variante de texto de interfaz.
    pub fn ui_line_h(&self, v: UiVariant) -> f32 {
        (self.glyphs.ui.metrics.height * v.scale()).ceil()
    }

    pub fn ui_measure(&mut self, s: &str) -> f32 {
        self.ui_measure_v(s, UiVariant::Normal)
    }

    pub fn ui_measure_v(&mut self, s: &str, v: UiVariant) -> f32 {
        s.chars().map(|c| self.glyphs.ui_advance(c, v)).sum()
    }

    /// Texto de interfaz con la fuente proporcional; `y` es el borde superior de la línea.
    /// Se recorta con "…" si no cabe en `max_w`. Devuelve el ancho dibujado.
    pub fn ui_text(&mut self, x: f32, y: f32, s: &str, color: Rgb, max_w: f32) -> f32 {
        self.ui_text_v(x, y, s, color, max_w, UiVariant::Normal)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ui_text_v(&mut self, x: f32, y: f32, s: &str, color: Rgb, max_w: f32, v: UiVariant) -> f32 {
        let baseline = y + (self.glyphs.ui.metrics.baseline * v.scale()).round();
        let total = self.ui_measure_v(s, v);
        let ell = self.glyphs.ui_advance('…', v);
        let limit = if total > max_w { (max_w - ell).max(0.0) } else { f32::MAX };
        let mut cx = x;
        let mut cut = false;
        for ch in s.chars() {
            let adv = self.glyphs.ui_advance(ch, v);
            if cx + adv - x > limit {
                cut = true;
                break;
            }
            self.ui_glyph(ch, cx, baseline, color, v);
            cx += adv;
        }
        if cut && max_w >= ell {
            self.ui_glyph('…', cx, baseline, color, v);
            cx += ell;
        }
        cx - x
    }

    /// Texto centrado verticalmente en la franja [y, y + h].
    pub fn ui_text_in(&mut self, x: f32, y: f32, h: f32, s: &str, color: Rgb, max_w: f32) -> f32 {
        let lh = self.glyphs.ui.metrics.height;
        self.ui_text(x, (y + (h - lh) / 2.0).round(), s, color, max_w)
    }

    fn ui_glyph(&mut self, ch: char, x: f32, baseline: f32, color: Rgb, v: UiVariant) {
        if ch == ' ' {
            return;
        }
        let (g, _) = self.glyphs.get_ui(self.queue, self.atlas, ch, v);
        if let Some(g) = g {
            self.fg.push(Instance {
                pos: [(x + g.left).round(), (baseline - g.top).round()],
                size: [g.width, g.height],
                uv: g.uv,
                color: color.to_f32(1.0),
                kind: if g.is_color { 2 } else { 1 },
                radius: 0.0,
            });
        }
    }

    /// Segmento antialiasado (iconos).
    #[allow(clippy::too_many_arguments)]
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, thick: f32, c: Rgb, a: f32) {
        let pad = thick + 1.0;
        let (mx, my) = (x0.min(x1) - pad, y0.min(y1) - pad);
        let (w, h) = (x0.max(x1) + pad - mx, y0.max(y1) + pad - my);
        self.out.push(Instance {
            pos: [mx, my],
            size: [w, h],
            uv: [x0 - mx, y0 - my, x1 - mx, y1 - my],
            color: c.to_f32(a),
            kind: 4,
            radius: thick / 2.0,
        });
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, c: Rgb, a: f32) {
        self.rounded(cx - r, cy - r, 2.0 * r, 2.0 * r, c, a, r);
    }

    /// Circunferencia (anillo) de radio `r` y grosor `t`.
    pub fn ring(&mut self, cx: f32, cy: f32, r: f32, t: f32, c: Rgb, a: f32) {
        let e = r + t;
        self.out.push(Instance {
            pos: [cx - e, cy - e],
            size: [2.0 * e, 2.0 * e],
            uv: [e, e, r, t],
            color: c.to_f32(a),
            kind: 3,
            radius: 0.0,
        });
    }

    /// Panel flotante moderno: sombra suave, borde fino y esquinas redondeadas.
    #[allow(clippy::too_many_arguments)]
    pub fn card(&mut self, x: f32, y: f32, w: f32, h: f32, fill: Rgb, border: Rgb, r: f32, scale: f32) {
        for (i, a) in [(3.0, 0.10), (8.0, 0.08), (16.0, 0.06)] {
            let o = i * scale;
            self.rounded(x - o * 0.5, y - o * 0.25, w + o, h + o, Rgb(0, 0, 0), a, r + o * 0.5);
        }
        self.rounded(x, y, w, h, border, 0.14, r);
        let b = scale.round().max(1.0);
        self.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, fill, 1.0, (r - b).max(0.0));
    }

    pub fn finish(self) {
        self.out.append(self.fg);
    }
}

pub fn resolve(color: AnsiColor, theme: &Theme, colors: &Colors) -> Rgb {
    let idx = match color {
        AnsiColor::Spec(c) => return Rgb(c.r, c.g, c.b),
        AnsiColor::Indexed(i) => i as usize,
        AnsiColor::Named(n) => n as usize,
    };
    if let Some(c) = colors[idx] {
        return Rgb(c.r, c.g, c.b);
    }
    match idx {
        0..=255 => theme.color256(idx as u8),
        _ if idx == NamedColor::Foreground as usize || idx == NamedColor::BrightForeground as usize => {
            theme.foreground
        }
        _ if idx == NamedColor::Background as usize => theme.background,
        _ if idx == NamedColor::Cursor as usize => theme.cursor,
        _ if idx == NamedColor::DimForeground as usize => theme.foreground.lerp(theme.background, 0.35),
        // DimBlack..DimWhite
        _ if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&idx) => {
            let base = idx - NamedColor::DimBlack as usize;
            theme.palette[base].lerp(theme.background, 0.35)
        }
        _ => theme.foreground,
    }
}

/// Últimas líneas con contenido de la pantalla de un terminal, en tramos de color, para la
/// vista previa del panel lateral.
///
/// Los agentes dibujan abajo su zona de entrada entre dos líneas horizontales (o en una caja) y,
/// debajo, su barra de estado. Eso no dice nada de lo que está haciendo, así que se corta por
/// encima de esa zona. Además se quitan los marcos y los espacios de los extremos, y se saltan
/// las líneas que solo tienen eso.
pub fn preview_lines<T: EventListener>(term: &Term<T>, theme: &Theme, max: usize) -> Vec<Vec<(String, Rgb)>> {
    let grid = term.grid();
    let colors = term.colors();
    let cols = term.columns();
    let n = term.screen_lines();
    let rows: Vec<Vec<(char, Rgb)>> = (0..n as i32)
        .map(|row| {
            let line = &grid[Line(row)];
            (0..cols)
                .filter_map(|c| {
                    let cell = &line[Column(c)];
                    if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                        return None;
                    }
                    let mut fg =
                        resolve(if cell.flags.contains(Flags::INVERSE) { cell.bg } else { cell.fg }, theme, colors);
                    if cell.flags.contains(Flags::DIM) {
                        fg = fg.lerp(theme.background, 0.35);
                    }
                    if fg == theme.background {
                        fg = theme.foreground;
                    }
                    Some((if cell.c == '\0' { ' ' } else { cell.c }, fg))
                })
                .collect()
        })
        .collect();
    let cut = input_area_top(&rows).unwrap_or(n);
    let mut out = Vec::new();
    for cells in rows[..cut].iter().rev() {
        let Some(start) = cells.iter().position(|(c, _)| !is_frame(*c)) else { continue };
        let end = cells.iter().rposition(|(c, _)| !is_frame(*c)).unwrap_or(start);
        let mut runs: Vec<(String, Rgb)> = Vec::new();
        for &(c, fg) in &cells[start..=end] {
            match runs.last_mut() {
                Some((text, color)) if *color == fg || c == ' ' => text.push(c),
                _ => runs.push((c.to_string(), fg)),
            }
        }
        out.push(runs);
        if out.len() >= max {
            break;
        }
    }
    out.reverse();
    out
}

fn is_frame(c: char) -> bool {
    c == ' ' || ('\u{2500}'..='\u{259F}').contains(&c)
}

/// Línea horizontal de lado a lado (o borde de caja): solo marcos y al menos 8 trazos.
fn is_rule(cells: &[(char, Rgb)]) -> bool {
    let strokes = cells.iter().filter(|(c, _)| ('\u{2500}'..='\u{257F}').contains(c)).count();
    strokes >= 8 && cells.iter().all(|(c, _)| is_frame(*c))
}

/// Primera línea de la zona de entrada (la línea superior de su marco), si la hay. Los agentes la
/// dibujan justo debajo de su contenido (no siempre al fondo de la pantalla) y por debajo solo
/// queda la barra de estado: se busca la línea más baja con, como mucho, 12 líneas con texto
/// debajo y, hasta 15 líneas por encima, su pareja.
pub fn input_area_top(rows: &[Vec<(char, Rgb)>]) -> Option<usize> {
    let has_text = |r: &Vec<(char, Rgb)>| r.iter().any(|(c, _)| !is_frame(*c));
    let last = rows.iter().rposition(has_text)?;
    let mut below = 0;
    let mut lower = None;
    for r in (0..rows.len()).rev() {
        if is_rule(&rows[r]) {
            lower = Some(r);
            break;
        }
        if r <= last && has_text(&rows[r]) {
            below += 1;
            if below > 12 {
                return None;
            }
        }
    }
    let lower = lower?;
    let upper = (lower.saturating_sub(15)..lower).rev().find(|&r| is_rule(&rows[r]));
    Some(upper.unwrap_or(lower))
}

/// Texto sugerido (en gris) a continuación del cursor, sin pasar del borde derecho.
pub fn draw_ghost<T: EventListener>(p: &mut Painter, term: &Term<T>, theme: &Theme, v: &TermView, text: &str) {
    let m = p.metrics();
    let cur = term.grid().cursor.point;
    let row = cur.line.0;
    if row < 0 || row as usize >= v.lines {
        return;
    }
    let cols = term.columns();
    let color = theme.foreground.lerp(theme.background, 0.55);
    let y = v.y + row as f32 * m.height;
    for (k, ch) in text.chars().enumerate() {
        let col = cur.column.0 + k;
        if col >= cols {
            break;
        }
        if ch != ' ' {
            p.glyph(ch, Style::Regular, false, false, v.x + col as f32 * m.width, y, color, 1.0);
        }
    }
}

/// Prompt de Garbo (la línea de la carpeta, que pone la integración de bash). En el prompt actual
/// se dibuja como una etiqueta con borde, icono de carpeta y, al lado, la rama de git; en los
/// comandos ya ejecutados, el icono en gris y tras la ruta la duración (o el código si falló).
/// Va antes que `draw_terminal`: los fondos de la selección quedan por encima de la etiqueta.
#[allow(clippy::too_many_arguments)]
pub fn draw_prompts<T: EventListener>(
    p: &mut Painter,
    term: &Term<T>,
    theme: &Theme,
    v: &TermView,
    bs: &BlockState,
    branch: Option<&str>,
    scale: f32,
) {
    if !bs.active {
        return;
    }
    let m = p.metrics();
    let (cw, ch) = (m.width, m.height);
    let s = scale;
    let offset = term.grid().display_offset() as i32;
    let cols = term.columns();
    let top = Line(-offset);
    let bottom = Line(-offset + v.lines as i32 - 1);
    let current = bs.idle_current();
    let dim = theme.foreground.lerp(theme.background, 0.5);
    let t = (1.3 * s).max(1.0);
    let pad = (2.0 * s).round();
    let r = (6.0 * s).round();
    let hair = s.round().max(1.0);
    for a in blocks::anchors(term, top, bottom) {
        let Some(info) = bs.info(a.id).filter(|i| i.styled) else { continue };
        let row = &term.grid()[a.line];
        let mine = |c: usize| matches!(blocks::cell_link(&row[Column(c)]), Some((b'p', id)) if id == a.id);
        // Última celda con texto de la línea de la carpeta.
        let Some(end) = (0..cols).rev().find(|&c| mine(c) && !matches!(row[Column(c)].c, ' ' | '\0')) else { continue };
        let y = v.y + (a.line.0 + offset) as f32 * ch;
        let (iw, ih) = ((cw * 1.05).round(), (ch * 0.40).round());
        let (ix, iy) = ((v.x + 0.5 * cw).round(), (y + (ch - ih) / 2.0).round());
        if Some(a.id) == current {
            let x0 = v.x - 0.1 * cw;
            let x1 = v.x + (end + 1) as f32 * cw + 0.6 * cw;
            let (cy, chh) = (y + pad, ch - 2.0 * pad);
            p.rounded(x0, cy, x1 - x0, chh, theme.foreground, 0.26, r);
            p.rounded(x0 + hair, cy + hair, x1 - x0 - 2.0 * hair, chh - 2.0 * hair, crate::ui::surface(theme, 0.08), 1.0, r - hair);
            crate::ui::folder(p, ix, iy, iw, ih, t, theme.accent);
            // Rama de git, si cabe.
            if let Some(b) = branch {
                let n = b.chars().count();
                if end + 6 + n < cols {
                    let bx0 = x1 + 0.6 * cw;
                    let bx1 = bx0 + (n as f32 + 2.6) * cw;
                    p.rounded(bx0, cy, bx1 - bx0, chh, theme.foreground, 0.06, r);
                    let (bw, bh) = ((cw * 0.85).round(), (ch * 0.42).round());
                    crate::ui::branch(p, (bx0 + 0.5 * cw).round(), (y + (ch - bh) / 2.0).round(), bw, bh, t, theme.palette[5]);
                    p.text(bx0 + 1.8 * cw, y, b, theme.foreground.lerp(theme.background, 0.2), n as f32 * cw + 1.0);
                }
            }
        } else {
            crate::ui::folder(p, ix, iy, iw, ih, t, dim);
            let dur = info.duration.map(|d| if d.as_millis() == 0 { "< 1 ms".into() } else { blocks::format_duration(d) });
            let (text, color) = match (info.status(), dur) {
                (Status::Ok, Some(d)) => (d, dim),
                (Status::Failed(code), Some(d)) => (format!("✗ {code} · {d}"), theme.palette[1]),
                (Status::Failed(code), None) => (format!("✗ {code}"), theme.palette[1]),
                _ => continue,
            };
            let n = text.chars().count();
            if end + 3 + n < cols {
                p.text(v.x + (end + 3) as f32 * cw, y, &text, color, n as f32 * cw + 1.0);
            }
        }
    }
}

/// Menú de completado bajo la palabra que se completa (encima si no cabe debajo). Guarda en
/// `menu.rect` dónde quedó, para el ratón.
pub fn draw_menu<T: EventListener>(p: &mut Painter, term: &Term<T>, theme: &Theme, v: &TermView, menu: &mut Menu, scale: f32) {
    let items = &menu.comp.items;
    menu.rect = None;
    if items.is_empty() {
        return;
    }
    let m = p.metrics();
    let s = scale;
    let cur = term.grid().cursor.point;
    let row = cur.line.0.max(0) as f32;
    let area_w = term.columns() as f32 * m.width;
    let n = items.len();
    let vis = n.min(MENU_ROWS);
    let pad = (6.0 * s).round();
    let row_h = (m.height + 8.0 * s).round();
    let mark_w = (16.0 * s).round();
    let bar_w = if n > vis { (8.0 * s).round() } else { 0.0 };
    let gap = (18.0 * s).round();

    let longest = items.iter().map(|i| i.label.chars().count()).max().unwrap_or(0).clamp(6, 48);
    // La descripción solo si aporta algo (si todas dicen "carpeta", sobra).
    let same = items.iter().all(|i| i.detail == items[0].detail);
    let detail_w = if same {
        0.0
    } else {
        items.iter().take(64).map(|i| p.ui_measure_v(&i.detail, UiVariant::Small)).fold(0.0, f32::max).min(260.0 * s)
    };
    let want = 2.0 * pad + mark_w + longest as f32 * m.width + gap + detail_w + 2.0 * pad + bar_w;
    let w = want.min(area_w).max((160.0 * s).min(area_w)).round();
    let h = (vis as f32 * row_h + 2.0 * pad).round();
    // La lista empieza bajo el principio de la palabra.
    let col = cur.column.0.saturating_sub(menu.comp.word.chars().count()) as f32;
    let x = (v.x + col * m.width - pad - mark_w).clamp(v.x, (v.x + area_w - w).max(v.x)).round();
    let bottom = v.y + v.lines as f32 * m.height;
    let below = v.y + (row + 1.0) * m.height + 2.0 * s;
    let above = v.y + row * m.height - h - 2.0 * s;
    let y = if below + h <= bottom || above < v.y { below } else { above }.round();

    p.card(x, y, w, h, crate::ui::surface(theme, 0.06), theme.foreground, (9.0 * s).round(), s);
    let dimc = crate::ui::dim(theme);
    let label_x = x + pad + mark_w;
    let detail_room = if detail_w > 0.0 { detail_w + gap } else { 0.0 };
    let fit = (((w - 2.0 * pad - mark_w - pad - bar_w - detail_room) / m.width).floor().max(4.0)) as usize;
    let small_h = p.ui_line_h(UiVariant::Small);
    for r in 0..vis {
        let idx = menu.top + r;
        let Some(it) = items.get(idx) else { break };
        let ry = y + pad + r as f32 * row_h;
        if idx == menu.selected {
            p.rounded(x + pad / 2.0, ry, w - pad - bar_w, row_h, theme.accent, 0.20, (6.0 * s).round());
        }
        let kc = theme.palette[it.kind.ansi()];
        p.circle(x + pad + mark_w / 2.0 - 1.0 * s, ry + row_h / 2.0, (3.0 * s).max(2.0), kc, 0.9);
        let ty = (ry + (row_h - m.height) / 2.0).round();
        let len = it.label.chars().count();
        for (k, ch) in it.label.chars().enumerate().take(fit) {
            let ch = if len > fit && k + 1 == fit { '…' } else { ch };
            let hit = it.hits.contains(&k);
            let color = if hit {
                theme.accent
            } else if it.kind == Kind::Dir {
                kc
            } else {
                theme.foreground
            };
            if ch != ' ' {
                let style = if hit { Style::Bold } else { Style::Regular };
                p.glyph(ch, style, false, false, label_x + k as f32 * m.width, ty, color, 1.0);
            }
        }
        if detail_w > 0.0 && !it.detail.is_empty() {
            let dw = p.ui_measure_v(&it.detail, UiVariant::Small).min(detail_w);
            let dx = x + w - 2.0 * pad - bar_w - dw;
            p.ui_text_v(dx.round(), (ry + (row_h - small_h) / 2.0).round(), &it.detail, dimc, detail_w + 1.0, UiVariant::Small);
        }
    }
    // Barra de desplazamiento cuando hay más de los que caben.
    if n > vis {
        let track = h - 2.0 * pad;
        let th = (track * vis as f32 / n as f32).max(14.0 * s);
        let ty = y + pad + (track - th) * menu.top as f32 / (n - vis) as f32;
        let bw = (3.0 * s).max(2.0);
        p.rounded(x + w - pad - bw, ty.round(), bw, th.round(), theme.foreground, 0.28, bw / 2.0);
    }
    menu.rect = Some([x, y, w, h, row_h, pad]);
}

pub struct TermView {
    pub x: f32,
    pub y: f32,
    pub lines: usize,
    pub focused: bool,
    pub cursor_on: bool,
}

/// `bs`: bloques de la pestaña, para dar estilo al prompt de Garbo (ver `draw_prompts`).
/// Subraya las celdas de `start` a `end` (enlace bajo el ratón con Ctrl).
#[allow(clippy::too_many_arguments)]
pub fn draw_underline<T: EventListener>(p: &mut Painter, term: &Term<T>, theme: &Theme, v: &TermView, start: alacritty_terminal::index::Point, end: alacritty_terminal::index::Point, scale: f32) {
    let m = p.metrics();
    let off = term.grid().display_offset() as i32;
    let cols = term.columns();
    let t = (1.5 * scale).round().max(1.0);
    for l in start.line.0..=end.line.0 {
        let row = l + off;
        if row < 0 || row >= v.lines as i32 {
            continue;
        }
        let a = if l == start.line.0 { start.column.0 } else { 0 };
        let b = if l == end.line.0 { end.column.0 + 1 } else { cols };
        let y = v.y + row as f32 * m.height + m.underline_pos.min(m.height - t);
        p.rect(v.x + a as f32 * m.width, y.round(), (b - a) as f32 * m.width, t, theme.accent, 1.0);
    }
}

/// `hl`: coincidencias de la búsqueda (Ctrl+Shift+F) que se resaltan.
pub fn draw_terminal<T: EventListener>(
    p: &mut Painter,
    term: &Term<T>,
    theme: &Theme,
    v: &TermView,
    bs: Option<&BlockState>,
    hl: Option<&crate::search::Highlights>,
) {
    let m = p.metrics();
    let (cw, chh) = (m.width, m.height);
    let current_prompt = bs.and_then(BlockState::idle_current);
    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let colors = content.colors;
    let selection = content.selection;

    let cursor = content.cursor;
    let cursor_row = cursor.point.line.0 + offset;
    let show_cursor = v.cursor_on && cursor.shape != CursorShape::Hidden && (0..v.lines as i32).contains(&cursor_row);
    let block_cursor = show_cursor && v.focused && cursor.shape == CursorShape::Block;

    // Búsqueda: coincidencias en amarillo suave; la actual, en amarillo.
    let found = theme.palette[3].lerp(theme.background, 0.6);
    let mut hl_next = 0;

    // Fusión de fondos contiguos del mismo color en una sola instancia.
    let mut run: Option<(i32, f32, f32, Rgb)> = None;
    let flush = |p: &mut Painter, run: &mut Option<(i32, f32, f32, Rgb)>| {
        if let Some((row, x0, x1, c)) = run.take() {
            p.rect(x0, v.y + row as f32 * chh, x1 - x0, chh, c, 1.0);
        }
    };

    for indexed in content.display_iter {
        let point = indexed.point;
        let row = point.line.0 + offset;
        if row < 0 || row >= v.lines as i32 {
            continue;
        }
        let cell = indexed.cell;
        let flags = cell.flags;
        if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let wide = flags.contains(Flags::WIDE_CHAR);
        let x = v.x + point.column.0 as f32 * cw;
        let y = v.y + row as f32 * chh;
        let w = if wide { 2.0 * cw } else { cw };

        let mut fg = resolve(cell.fg, theme, colors);
        let mut bg = resolve(cell.bg, theme, colors);
        if flags.contains(Flags::DIM) {
            fg = fg.lerp(bg, 0.35);
        }
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        let selected = selection.is_some_and(|s| s.contains(point));
        let is_cursor = block_cursor && point == cursor.point;
        if selected {
            bg = theme.selection;
        }
        let mut found_here = false;
        if let Some(hl) = hl {
            // Las celdas llegan en orden: se avanza por las coincidencias (también ordenadas).
            while hl.all.get(hl_next).is_some_and(|m| *m.end() < point) {
                hl_next += 1;
            }
            if hl.current.as_ref().is_some_and(|m| m.contains(&point)) {
                bg = theme.palette[3];
                fg = theme.background;
                found_here = true;
            } else if hl.all.get(hl_next).is_some_and(|m| m.contains(&point)) {
                bg = found;
                found_here = true;
            }
        }
        if is_cursor {
            bg = theme.cursor;
            fg = theme.background;
        }

        // Fondo
        if bg != theme.background || selected || is_cursor || found_here {
            match &mut run {
                Some((r, _, x1, c)) if *r == row && (*x1 - x).abs() < 0.5 && *c == bg => *x1 = x + w,
                _ => {
                    flush(p, &mut run);
                    run = Some((row, x, x + w, bg));
                }
            }
        }

        if flags.contains(Flags::HIDDEN) {
            continue;
        }

        let c = cell.c;
        // Prompt de Garbo: la ruta en negrita en el prompt actual y en gris en los ya ejecutados;
        // lo escrito, en negrita.
        let mut bold = flags.contains(Flags::BOLD);
        if let Some((kind, id)) = bs.and_then(|bs| blocks::cell_link(cell).filter(|(_, id)| bs.styled(*id))) {
            match kind {
                b'p' if current_prompt == Some(id) => {
                    fg = theme.foreground;
                    bold = true;
                }
                b'p' => fg = theme.foreground.lerp(theme.background, 0.5),
                b'c' => bold = true,
                _ => {}
            }
        }
        if c != ' ' && c != '\t' && !draw_special(p, c, x, y, cw, chh, fg) {
            let style = match (bold, flags.contains(Flags::ITALIC)) {
                (true, true) => Style::BoldItalic,
                (true, false) => Style::Bold,
                (false, true) => Style::Italic,
                _ => Style::Regular,
            };
            let extra = cell.zerowidth().unwrap_or(&[]);
            // Presentación emoji: carácter ancho no-CJK, o seguido del selector VS16.
            let emoji = (wide && !is_cjk(c)) || extra.contains(&'\u{FE0F}');
            p.glyph(c, style, wide, emoji, x, y, fg, 1.0);
            for &z in extra {
                if !matches!(z, '\u{FE0E}' | '\u{FE0F}' | '\u{200D}') {
                    p.glyph(z, style, wide, emoji, x, y, fg, 1.0);
                }
            }
        }

        // Decoraciones
        let s = m.stroke;
        if flags.intersects(Flags::ALL_UNDERLINES) {
            let uc = cell
                .underline_color()
                .map(|c| resolve(c, theme, colors))
                .unwrap_or(fg);
            let uy = y + m.underline_pos;
            if flags.contains(Flags::DOUBLE_UNDERLINE) {
                p.fg.push(Instance::rect(x, uy - s, w, s, uc.to_f32(1.0)));
                p.fg.push(Instance::rect(x, uy + s, w, s, uc.to_f32(1.0)));
            } else if flags.intersects(Flags::UNDERCURL | Flags::DOTTED_UNDERLINE | Flags::DASHED_UNDERLINE) {
                let step = (cw / 4.0).max(2.0);
                let mut sx = x;
                let mut up = true;
                while sx < x + w {
                    let dy = if flags.contains(Flags::UNDERCURL) && up { -s } else { 0.0 };
                    let seg = if flags.contains(Flags::DASHED_UNDERLINE) { step * 1.5 } else { step / 2.0 };
                    p.fg.push(Instance::rect(sx, uy + dy, seg.min(x + w - sx), s, uc.to_f32(1.0)));
                    sx += step;
                    up = !up;
                }
            } else {
                p.fg.push(Instance::rect(x, uy, w, s, uc.to_f32(1.0)));
            }
        }
        if flags.contains(Flags::STRIKEOUT) {
            p.fg.push(Instance::rect(x, y + (m.baseline * 0.62).round(), w, s, fg.to_f32(1.0)));
        }
    }
    flush(p, &mut run);

    // Cursor (los modos no-bloque o sin foco se dibujan encima)
    if show_cursor && !block_cursor {
        let x = v.x + cursor.point.column.0 as f32 * cw;
        let y = v.y + cursor_row as f32 * chh;
        let wide = term.grid()[cursor.point].flags.contains(Flags::WIDE_CHAR);
        let w = if wide { 2.0 * cw } else { cw };
        let t = (m.stroke * 2.0).max(2.0);
        let c = theme.cursor.to_f32(1.0);
        let shape = if v.focused { cursor.shape } else { CursorShape::HollowBlock };
        match shape {
            CursorShape::Beam => p.fg.push(Instance::rect(x, y, t, chh, c)),
            CursorShape::Underline => p.fg.push(Instance::rect(x, y + chh - t, w, t, c)),
            _ => {
                let s = m.stroke;
                p.fg.push(Instance::rect(x, y, w, s, c));
                p.fg.push(Instance::rect(x, y + chh - s, w, s, c));
                p.fg.push(Instance::rect(x, y, s, chh, c));
                p.fg.push(Instance::rect(x + w - s, y, s, chh, c));
            }
        }
    }
}

/// Decoración de los bloques de comandos. Se dibuja después de la rejilla:
/// queda por encima de los fondos de celda y por debajo del texto.
#[allow(clippy::too_many_arguments)]
pub fn draw_blocks<T: EventListener>(
    p: &mut Painter,
    term: &Term<T>,
    theme: &Theme,
    v: &TermView,
    bs: &BlockState,
    selected: Option<u32>,
    span: (f32, f32),
    scale: f32,
) {
    if !bs.active {
        return;
    }
    let m = p.metrics();
    let ch = m.height;
    let lines = v.lines as i32;
    let cols = term.columns();
    let offset = term.grid().display_offset() as i32;
    let top = Line(-offset);
    let bottom = Line(-offset + lines - 1);

    let mut anchors = blocks::anchors(term, top, bottom);
    // El bloque que empieza por encima de la pantalla también se decora.
    if anchors.first().is_none_or(|a| a.line > top) {
        if let Some(a) = blocks::anchor_at(term, top, 2000) {
            anchors.insert(0, a);
        }
    }
    let cursor_line = term.grid().cursor.point.line;
    let dim = theme.foreground.lerp(theme.background, 0.45);
    let (ok, err) = (theme.palette[2], theme.palette[1]);
    let bar_w = (3.0 * scale).round().max(2.0);
    // La barra de estado va en el margen izquierdo del panel (de `span.0` a `v.x`).
    let bar_x = (span.0 + (v.x - span.0) * 0.55 - bar_w / 2.0).round().max(span.0);
    let width = span.1 - span.0;
    let line_w = scale.round().max(1.0);

    for (i, a) in anchors.iter().enumerate() {
        let end_line = match anchors.get(i + 1) {
            Some(next) => next.line - 1,
            None if Some(a.id) == bs.current => cursor_line,
            None => bottom,
        };
        let start_row = (a.line.0 + offset).max(0);
        let end_row = (end_line.0 + offset).min(lines - 1);
        if end_row < start_row {
            continue;
        }
        let y0 = v.y + start_row as f32 * ch;
        let y1 = v.y + (end_row + 1) as f32 * ch;
        let starts_here = a.line >= top;
        let info = bs.info(a.id);
        let status = info.map_or(Status::Idle, |i| i.status());
        let is_sel = selected == Some(a.id);

        if is_sel {
            p.rect(span.0, y0, width, y1 - y0, theme.accent, 0.10);
        }
        if starts_here && start_row > 0 {
            p.rect(v.x, y0, width - 2.0 * (v.x - span.0), line_w, theme.foreground, 0.10);
        }
        let bar = match status {
            _ if is_sel => Some((theme.accent, 1.0)),
            Status::Ok => Some((ok, 0.6)),
            Status::Failed(_) => Some((err, 1.0)),
            Status::Running => Some((theme.accent, 1.0)),
            Status::Idle => None,
        };
        if let Some((c, a)) = bar {
            let pad = (3.0 * scale).round();
            p.rounded(bar_x, y0 + pad, bar_w, (y1 - y0 - 2.0 * pad).max(bar_w), c, a, bar_w / 2.0);
        }

        // Estado y duración a la derecha de la línea del prompt, si ese hueco está libre (con el
        // prompt de Garbo van tras la ruta: ver `draw_prompts`).
        let (Some(info), true) = (info.filter(|i| !i.styled), starts_here) else { continue };
        // Sin ruido: en los correctos solo la duración si fue apreciable; los fallos siempre.
        let slow = info.duration.filter(|d| d.as_secs_f32() >= 1.0).map(blocks::format_duration);
        let (icon, icon_color, text) = match (status, slow) {
            (Status::Ok, Some(d)) => ("✓", ok, d),
            (Status::Failed(code), Some(d)) => ("✗", err, format!("{code} · {d}")),
            (Status::Failed(code), None) => ("✗", err, code.to_string()),
            _ => continue,
        };
        let n = text.chars().count() + 2;
        if n + 4 >= cols {
            continue;
        }
        let row = &term.grid()[a.line];
        let free = (cols - n - 2..cols).all(|c| matches!(row[Column(c)].c, ' ' | '\0'));
        if free {
            let x = v.x + (cols - n) as f32 * m.width;
            p.text(x, y0, icon, icon_color, m.width * 2.0);
            p.text(x + 2.0 * m.width, y0, &text, if matches!(status, Status::Failed(_)) { err } else { dim }, n as f32 * m.width);
        }
    }
}

/// Rangos anchos que NO son emoji (ideogramas, kana, hangul, formas de ancho completo...).
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF | 0xA960..=0xA97F | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x20000..=0x3FFFD)
}

/// Caracteres de caja y bloques dibujados con primitivas: encajan al píxel sin huecos ni costuras
/// aunque se use interlineado (las fuentes los diseñan para su propia altura de línea).
/// Devuelve `false` si el carácter no se gestiona aquí.
fn draw_special(p: &mut Painter, c: char, x: f32, y: f32, w: f32, h: f32, fg: Rgb) -> bool {
    let col = fg.to_f32(1.0);
    let (x1, y1) = (x + w, y + h);
    let mx = (x + w / 2.0).round();
    let my = (y + h / 2.0).round();
    let out = &mut *p.fg;
    // Rellena por bordes redondeados: celdas vecinas comparten exactamente el mismo píxel frontera.
    let mut fill = |l: f32, t: f32, r: f32, b: f32, a: f32| {
        let (l, r) = (l.min(r).round(), l.max(r).round());
        let (t, b) = (t.min(b).round(), t.max(b).round());
        if r > l && b > t {
            out.push(Instance::rect(l, t, r - l, b - t, [col[0], col[1], col[2], a]));
        }
    };
    let cp = c as u32;
    match c {
        '█' => fill(x, y, x1, y1, 1.0),
        '▀' => fill(x, y, x1, my, 1.0),
        '▄' => fill(x, my, x1, y1, 1.0),
        '▌' => fill(x, y, mx, y1, 1.0),
        '▐' => fill(mx, y, x1, y1, 1.0),
        '░' => fill(x, y, x1, y1, 0.25),
        '▒' => fill(x, y, x1, y1, 0.5),
        '▓' => fill(x, y, x1, y1, 0.75),
        '▔' => fill(x, y, x1, y + h / 8.0, 1.0),
        '▕' => fill(x1 - w / 8.0, y, x1, y1, 1.0),
        '\u{2581}'..='\u{2587}' => fill(x, y1 - (cp - 0x2580) as f32 * h / 8.0, x1, y1, 1.0),
        '\u{2589}'..='\u{258F}' => fill(x, y, x + (0x2590 - cp) as f32 * w / 8.0, y1, 1.0),
        '\u{2596}'..='\u{259F}' => {
            // Cuadrantes: bits 1=sup-izq 2=sup-der 4=inf-izq 8=inf-der
            const Q: [u8; 10] = [4, 8, 1, 13, 9, 7, 11, 2, 6, 14];
            let q = Q[(cp - 0x2596) as usize];
            if q & 1 != 0 { fill(x, y, mx, my, 1.0) }
            if q & 2 != 0 { fill(mx, y, x1, my, 1.0) }
            if q & 4 != 0 { fill(x, my, mx, y1, 1.0) }
            if q & 8 != 0 { fill(mx, my, x1, y1, 1.0) }
        }
        _ => {
            let base = (w / 8.0).round().max(1.0);
            let t = |k: u8| if k == 2 { base * 2.0 } else { base };
            // Líneas discontinuas: (horizontal, nº de trazos, grosor)
            let dashed = match c {
                '╌' => Some((true, 2, 1)), '╍' => Some((true, 2, 2)),
                '┄' => Some((true, 3, 1)), '┅' => Some((true, 3, 2)),
                '┈' => Some((true, 4, 1)), '┉' => Some((true, 4, 2)),
                '╎' => Some((false, 2, 1)), '╏' => Some((false, 2, 2)),
                '┆' => Some((false, 3, 1)), '┇' => Some((false, 3, 2)),
                '┊' => Some((false, 4, 1)), '┋' => Some((false, 4, 2)),
                _ => None,
            };
            if let Some((horiz, n, k)) = dashed {
                let th = t(k);
                for i in 0..n {
                    let f = i as f32 / n as f32;
                    let g = (i as f32 + 0.6) / n as f32;
                    if horiz {
                        fill(x + f * w, my - th / 2.0, x + g * w, my + th / 2.0, 1.0);
                    } else {
                        fill(mx - th / 2.0, y + f * h, mx + th / 2.0, y + g * h, 1.0);
                    }
                }
                return true;
            }
            // Esquinas redondeadas: arco + tramos rectos hasta los bordes.
            let round = match c {
                '╭' => Some((1.0, 1.0)), '╮' => Some((-1.0, 1.0)),
                '╰' => Some((1.0, -1.0)), '╯' => Some((-1.0, -1.0)),
                _ => None,
            };
            if let Some((dx, dy)) = round {
                let th = base;
                let r = (w.min(h) / 2.0).round();
                let (cx, cy) = (mx + dx * r, my + dy * r);
                let qx0 = (mx - dx * th).min(cx);
                let qx1 = (mx - dx * th).max(cx);
                let qy0 = (my - dy * th).min(cy);
                let qy1 = (my - dy * th).max(cy);
                out.push(Instance {
                    pos: [qx0, qy0],
                    size: [qx1 - qx0, qy1 - qy0],
                    uv: [cx - qx0, cy - qy0, r, th],
                    color: col,
                    kind: 3,
                    radius: 0.0,
                });
                let ex = if dx > 0.0 { x1 } else { x };
                let ey = if dy > 0.0 { y1 } else { y };
                let mut fill = |l: f32, t: f32, r: f32, b: f32| {
                    let (l, r) = (l.min(r).round(), l.max(r).round());
                    let (t, b) = (t.min(b).round(), t.max(b).round());
                    if r > l && b > t {
                        out.push(Instance::rect(l, t, r - l, b - t, col));
                    }
                };
                fill(cx, my - th / 2.0, ex, my + th / 2.0);
                fill(mx - th / 2.0, cy, mx + th / 2.0, ey);
                return true;
            }
            // Líneas dobles (rectas y esquinas): (dx, dy) hacia donde salen los brazos; 0 = recta
            let double = match c {
                '═' => Some((2, 0)), '║' => Some((0, 2)),
                '╔' => Some((1, 1)), '╗' => Some((-1, 1)),
                '╚' => Some((1, -1)), '╝' => Some((-1, -1)),
                _ => None,
            };
            if let Some((dx, dy)) = double {
                let th = base;
                let o = (th * 1.5).round();
                match (dx, dy) {
                    (2, 0) => {
                        fill(x, my - o - th / 2.0, x1, my - o + th / 2.0, 1.0);
                        fill(x, my + o - th / 2.0, x1, my + o + th / 2.0, 1.0);
                    }
                    (0, 2) => {
                        fill(mx - o - th / 2.0, y, mx - o + th / 2.0, y1, 1.0);
                        fill(mx + o - th / 2.0, y, mx + o + th / 2.0, y1, 1.0);
                    }
                    (dx, dy) => {
                        let (dx, dy) = (dx as f32, dy as f32);
                        let ex = if dx > 0.0 { x1 } else { x };
                        let ey = if dy > 0.0 { y1 } else { y };
                        // k = -1 línea exterior, +1 interior
                        for k in [-1.0f32, 1.0] {
                            let lx = mx + k * dx * o; // x de la línea vertical
                            let ly = my + k * dy * o; // y de la línea horizontal
                            fill(lx - dx * th / 2.0, ly - th / 2.0, ex, ly + th / 2.0, 1.0);
                            fill(lx - th / 2.0, ly - dy * th / 2.0, lx + th / 2.0, ey, 1.0);
                        }
                    }
                }
                return true;
            }
            // Líneas de caja: (arriba, abajo, izquierda, derecha); 1 = fina, 2 = gruesa
            let (u, d, l, r) = match c {
                '─' => (0, 0, 1, 1),
                '━' => (0, 0, 2, 2),
                '│' => (1, 1, 0, 0),
                '┃' => (2, 2, 0, 0),
                '┌' => (0, 1, 0, 1),
                '┐' => (0, 1, 1, 0),
                '└' => (1, 0, 0, 1),
                '┘' => (1, 0, 1, 0),
                '├' => (1, 1, 0, 1),
                '┤' => (1, 1, 1, 0),
                '┬' => (0, 1, 1, 1),
                '┴' => (1, 0, 1, 1),
                '┼' => (1, 1, 1, 1),
                '┏' => (0, 2, 0, 2),
                '┓' => (0, 2, 2, 0),
                '┗' => (2, 0, 0, 2),
                '┛' => (2, 0, 2, 0),
                '┣' => (2, 2, 0, 2),
                '┫' => (2, 2, 2, 0),
                '┳' => (0, 2, 2, 2),
                '┻' => (2, 0, 2, 2),
                '╋' => (2, 2, 2, 2),
                '┍' => (0, 1, 0, 2),
                '┑' => (0, 1, 2, 0),
                '┕' => (1, 0, 0, 2),
                '┙' => (1, 0, 2, 0),
                '┎' => (0, 2, 0, 1),
                '┒' => (0, 2, 1, 0),
                '┖' => (2, 0, 0, 1),
                '┚' => (2, 0, 1, 0),
                '┝' => (1, 1, 0, 2),
                '┥' => (1, 1, 2, 0),
                '┠' => (2, 2, 0, 1),
                '┨' => (2, 2, 1, 0),
                '┯' => (0, 1, 2, 2),
                '┷' => (1, 0, 2, 2),
                '┰' => (0, 2, 1, 1),
                '┸' => (2, 0, 1, 1),
                '┿' => (1, 1, 2, 2),
                '╂' => (2, 2, 1, 1),
                '╴' => (0, 0, 1, 0),
                '╵' => (1, 0, 0, 0),
                '╶' => (0, 0, 0, 1),
                '╷' => (0, 1, 0, 0),
                '╸' => (0, 0, 2, 0),
                '╹' => (2, 0, 0, 0),
                '╺' => (0, 0, 0, 2),
                '╻' => (0, 2, 0, 0),
                '╼' => (0, 0, 1, 2),
                '╾' => (0, 0, 2, 1),
                '╽' => (1, 2, 0, 0),
                '╿' => (2, 1, 0, 0),
                _ => return false,
            };
            let tv = t(u.max(d));
            let th = t(l.max(r));
            if l > 0 {
                fill(x, my - t(l) / 2.0, mx + tv / 2.0, my + t(l) / 2.0, 1.0);
            }
            if r > 0 {
                fill(mx - tv / 2.0, my - t(r) / 2.0, x1, my + t(r) / 2.0, 1.0);
            }
            if u > 0 {
                fill(mx - t(u) / 2.0, y, mx + t(u) / 2.0, my + th / 2.0, 1.0);
            }
            if d > 0 {
                fill(mx - t(d) / 2.0, my - th / 2.0, mx + t(d) / 2.0, y1, 1.0);
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(lines: &[&str]) -> Vec<Vec<(char, Rgb)>> {
        lines.iter().map(|l| l.chars().map(|c| (c, Rgb(0, 0, 0))).collect()).collect()
    }

    #[test]
    fn corta_la_zona_de_entrada_del_agente() {
        let rule = "────────────────────────────────";
        // Contenido, línea, entrada en dos líneas, línea, barra de estado en tres líneas.
        let r = rows(&["⏺ Hecho", "", "✻ Pensando…", rule, "❯ escribo algo", "  que sigue", rule, "Opus 5.5", "ctx 29%", "auto mode"]);
        assert_eq!(input_area_top(&r), Some(3));
        // Con caja (╭ ╮ ╰ ╯) también.
        let r = rows(&["salida", "otra", "╭──────────────╮", "│ > hola       │", "╰──────────────╯", "? atajos"]);
        assert_eq!(input_area_top(&r), Some(2));
        // Sin zona de entrada: no se corta nada.
        let r = rows(&["uno", "dos", "tres", "cuatro"]);
        assert_eq!(input_area_top(&r), None);
        // Zona de entrada arriba (pantalla casi vacía) y líneas en blanco debajo: se encuentra.
        let r = rows(&["⏺ Hola", rule, "> ", rule, "estado", "", "", "", ""]);
        assert_eq!(input_area_top(&r), Some(1));
        // Una línea con mucho texto debajo es parte de la salida, no la zona de entrada.
        let mut v = vec!["a", rule];
        v.extend(["texto"; 13]);
        assert_eq!(input_area_top(&rows(&v)), None);
    }
}
