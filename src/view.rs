//! Convierte el estado del terminal en instancias para la GPU.

use crate::atlas::{GlyphCache, UiVariant};
use crate::blocks::{self, BlockState, Status};
use crate::complete::{Kind, Menu, MENU_ROWS};
use crate::config::{Rgb, Theme};
use crate::font::{CellMetrics, Style};
use crate::renderer::Instance;
use crate::style::{Blocks, Prompt};
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

    #[allow(clippy::too_many_arguments)]
    pub fn rounded(&mut self, x: f32, y: f32, w: f32, h: f32, c: Rgb, a: f32, r: f32) {
        self.out.push(Instance::rounded(x, y, w, h, c.to_f32(a), r));
    }

    /// Dibuja un glifo cuya celda empieza en (x, y).
    #[allow(clippy::too_many_arguments)]
    pub fn glyph(
        &mut self,
        ch: char,
        style: Style,
        wide: bool,
        emoji: bool,
        x: f32,
        y: f32,
        color: Rgb,
        alpha: f32,
    ) {
        let m = self.metrics();
        if let Some(g) = self
            .glyphs
            .get(self.queue, self.atlas, ch, style, wide, emoji)
        {
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
    pub fn ui_text_v(
        &mut self,
        x: f32,
        y: f32,
        s: &str,
        color: Rgb,
        max_w: f32,
        v: UiVariant,
    ) -> f32 {
        let baseline = y + (self.glyphs.ui.metrics.baseline * v.scale()).round();
        let total = self.ui_measure_v(s, v);
        let ell = self.glyphs.ui_advance('…', v);
        let limit = if total > max_w {
            (max_w - ell).max(0.0)
        } else {
            f32::MAX
        };
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

    /// Texto de interfaz centrado verticalmente en [y, y + h], sin recortar con "…": lo que queda
    /// fuera de `clip` (x inicial y final) no se ve y los glifos del borde se cortan. `fade`:
    /// difumina el borde izquierdo y el derecho (por donde el texto sigue).
    #[allow(clippy::too_many_arguments)]
    pub fn ui_text_clipped(
        &mut self,
        x: f32,
        y: f32,
        h: f32,
        s: &str,
        color: Rgb,
        clip: (f32, f32),
        fade: (bool, bool),
    ) {
        let v = UiVariant::Normal;
        let lh = self.glyphs.ui.metrics.height;
        let top = (y + (h - lh) / 2.0).round();
        let baseline = top + (self.glyphs.ui.metrics.baseline * v.scale()).round();
        let fade_w = lh.max(1.0);
        let mut cx = x;
        for ch in s.chars() {
            if cx >= clip.1 {
                break;
            }
            let adv = self.glyphs.ui_advance(ch, v);
            if ch != ' ' && cx + adv > clip.0 {
                if let (Some(g), _) = self.glyphs.get_ui(self.queue, self.atlas, ch, v) {
                    let gx = (cx + g.left).round();
                    let (x0, x1) = (gx.max(clip.0), (gx + g.width).min(clip.1));
                    if x1 > x0 && g.width > 0.0 {
                        let (f0, f1) = ((x0 - gx) / g.width, (x1 - gx) / g.width);
                        let mid = (x0 + x1) / 2.0;
                        let mut a: f32 = 1.0;
                        if fade.0 {
                            a = a.min(((mid - clip.0) / fade_w).clamp(0.0, 1.0));
                        }
                        if fade.1 {
                            a = a.min(((clip.1 - mid) / fade_w).clamp(0.0, 1.0));
                        }
                        self.fg.push(Instance {
                            pos: [x0, (baseline - g.top).round()],
                            size: [x1 - x0, g.height],
                            uv: [
                                g.uv[0] + f0 * g.uv[2],
                                g.uv[1],
                                (f1 - f0) * g.uv[2],
                                g.uv[3],
                            ],
                            color: color.to_f32(a),
                            kind: if g.is_color { 2 } else { 1 },
                            radius: 0.0,
                        });
                    }
                }
            }
            cx += adv;
        }
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

    /// Degradado vertical de `top` (con alfa `ta`) a `bottom` (con alfa `ba`).
    #[allow(clippy::too_many_arguments)]
    pub fn gradient(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        top: Rgb,
        ta: f32,
        bottom: Rgb,
        ba: f32,
    ) {
        self.out.push(Instance::gradient(
            x,
            y,
            w,
            h,
            top.to_f32(ta),
            bottom.to_f32(ba),
        ));
    }

    /// Contorno redondeado de grosor `t` por dentro del rectángulo.
    #[allow(clippy::too_many_arguments)]
    pub fn outline(&mut self, x: f32, y: f32, w: f32, h: f32, c: Rgb, a: f32, r: f32, t: f32) {
        self.out
            .push(Instance::outline(x, y, w, h, c.to_f32(a), r, t));
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
    pub fn card(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Rgb,
        border: Rgb,
        r: f32,
        scale: f32,
    ) {
        for (i, a) in [(3.0, 0.10), (8.0, 0.08), (16.0, 0.06)] {
            let o = i * scale;
            self.rounded(
                x - o * 0.5,
                y - o * 0.25,
                w + o,
                h + o,
                Rgb(0, 0, 0),
                a,
                r + o * 0.5,
            );
        }
        self.rounded(x, y, w, h, border, 0.14, r);
        let b = scale.round().max(1.0);
        self.rounded(
            x + b,
            y + b,
            w - 2.0 * b,
            h - 2.0 * b,
            fill,
            1.0,
            (r - b).max(0.0),
        );
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
        _ if idx == NamedColor::Foreground as usize
            || idx == NamedColor::BrightForeground as usize =>
        {
            theme.foreground
        }
        _ if idx == NamedColor::Background as usize => theme.background,
        _ if idx == NamedColor::Cursor as usize => theme.cursor,
        _ if idx == NamedColor::DimForeground as usize => {
            theme.foreground.lerp(theme.background, 0.35)
        }
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
/// Omite el editor de entrada y su pie cuando se reconocen en la pantalla. También quita los
/// marcos y los espacios de los extremos y se salta las líneas que solo tienen eso.
pub fn preview_lines<T: EventListener>(
    term: &Term<T>,
    theme: &Theme,
    max: usize,
    provider: Option<crate::subagents::Provider>,
) -> Vec<Vec<(String, Rgb)>> {
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
                    if cell
                        .flags
                        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                    {
                        return None;
                    }
                    let mut fg = resolve(
                        if cell.flags.contains(Flags::INVERSE) {
                            cell.bg
                        } else {
                            cell.fg
                        },
                        theme,
                        colors,
                    );
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
    let cursor = term.grid().cursor.point.line.0;
    let cut = preview_end(&rows, cursor, provider).unwrap_or(n);
    let mut out = Vec::new();
    for cells in rows[..cut].iter().rev() {
        let Some(start) = cells.iter().position(|(c, _)| !is_frame(*c)) else {
            continue;
        };
        let end = cells
            .iter()
            .rposition(|(c, _)| !is_frame(*c))
            .unwrap_or(start);
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

fn preview_end(
    rows: &[Vec<(char, Rgb)>],
    cursor: i32,
    provider: Option<crate::subagents::Provider>,
) -> Option<usize> {
    use crate::subagents::Provider;
    if provider == Some(Provider::Claude) {
        return input_area_top(rows);
    }

    let cursor = usize::try_from(cursor).ok().filter(|&row| row < rows.len());
    let prompt = cursor.and_then(|cursor| {
        // Hasta arriba: la condición de abajo (todo lo de en medio, sangrado o marco) ya la limita.
        (0..=cursor).rev().find(|&row| {
            let mut text = rows[row]
                .iter()
                .map(|&(ch, _)| ch)
                .skip_while(|ch| is_frame(*ch));
            let marker = match provider {
                Some(Provider::Codex) => text.next() == Some('›'),
                Some(Provider::Gemini) | None => matches!(text.next(), Some('>' | '❯' | '›')),
                Some(Provider::Claude) => false,
            };
            marker
                && text.next().is_none_or(char::is_whitespace)
                && rows[row + 1..=cursor]
                    .iter()
                    .all(|line| line.first().is_none_or(|&(ch, _)| is_frame(ch)) && !is_rule(line))
        })
    });
    if let Some(prompt) = prompt {
        let border = prompt.checked_sub(1).filter(|&row| is_rule(&rows[row]));
        return Some(border.unwrap_or(prompt));
    }
    // Evita confundir un separador de la conversación con el editor de un agente distinto.
    let between = cursor.and_then(|cursor| {
        let upper = (cursor.saturating_sub(15)..cursor).rfind(|&row| is_rule(&rows[row]))?;
        let lower = (cursor + 1..rows.len().min(upper + 16)).any(|row| is_rule(&rows[row]));
        lower.then_some(upper)
    });
    // Los CLI hechos con Ink (Gemini CLI…) dejan el cursor bajo el pie: vale un marco completo
    // (línea de arriba y de abajo) cerca del final. Un separador suelto no cuenta.
    between.or_else(|| input_rules(rows).and_then(|(upper, _)| upper))
}

/// Línea horizontal de lado a lado (o borde de caja): solo marcos y al menos 8 trazos.
fn is_rule(cells: &[(char, Rgb)]) -> bool {
    let strokes = cells
        .iter()
        .filter(|(c, _)| ('\u{2500}'..='\u{257F}').contains(c))
        .count();
    strokes >= 8 && cells.iter().all(|(c, _)| is_frame(*c))
}

/// Primera línea de la zona de entrada (la línea superior de su marco), si la hay. Los agentes la
/// dibujan justo debajo de su contenido (no siempre al fondo de la pantalla) y por debajo solo
/// queda la barra de estado: se busca la línea más baja con, como mucho, 12 líneas con texto
/// debajo y, hasta 15 líneas por encima, su pareja.
pub fn input_area_top(rows: &[Vec<(char, Rgb)>]) -> Option<usize> {
    input_rules(rows).map(|(upper, lower)| upper.unwrap_or(lower))
}

/// Líneas del marco de la zona de entrada: la de arriba (si la hay) y la de abajo.
fn input_rules(rows: &[Vec<(char, Rgb)>]) -> Option<(Option<usize>, usize)> {
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
    let upper = (lower.saturating_sub(15)..lower)
        .rev()
        .find(|&r| is_rule(&rows[r]));
    Some((upper, lower))
}

/// Texto sugerido (en gris) a continuación del cursor, sin pasar del borde derecho.
pub fn draw_ghost<T: EventListener>(
    p: &mut Painter,
    term: &Term<T>,
    theme: &Theme,
    v: &TermView,
    text: &str,
) {
    let m = p.metrics();
    let cur = term.grid().cursor.point;
    let row = cur.line.0;
    if !v.shows(row) {
        return;
    }
    let cols = term.columns();
    let color = theme.foreground.lerp(theme.background, 0.55);
    let y = v.row_y(row, m.height);
    let line = &term.grid()[cur.line];
    for (k, ch) in text.chars().enumerate() {
        let col = cur.column.0 + k;
        // Sin pisar lo que ya hay en la línea (un prompt derecho).
        if col >= cols || !matches!(line[Column(col)].c, ' ' | '\0') {
            break;
        }
        if ch != ' ' {
            p.glyph(
                ch,
                Style::Regular,
                false,
                false,
                v.x + col as f32 * m.width,
                y,
                color,
                1.0,
            );
        }
    }
}

/// Prompt de Garbo (la línea de la carpeta, que pone la integración del shell). En el prompt actual
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
    let style = theme.style;
    for a in blocks::anchors(term, top, bottom) {
        let Some(info) = bs.info(a.id).filter(|i| i.styled) else {
            continue;
        };
        let row = &term.grid()[a.line];
        let mine =
            |c: usize| matches!(blocks::cell_link(&row[Column(c)]), Some((b'p', id)) if id == a.id);
        // Última celda con texto de la línea de la carpeta.
        let Some(end) = (0..cols)
            .rev()
            .find(|&c| mine(c) && !matches!(row[Column(c)].c, ' ' | '\0'))
        else {
            continue;
        };
        let shown = a.line.0 + offset;
        if !v.shows(shown) {
            continue;
        }
        let y = v.row_y(shown, ch);
        let (iw, ih) = ((cw * 1.05).round(), (ch * 0.40).round());
        let (ix, iy) = ((v.x + 0.5 * cw).round(), (y + (ch - ih) / 2.0).round());
        if Some(a.id) == current {
            prompt_label(p, theme, cw, ch, cols, v, y, end, branch, s);
        } else {
            if style.prompt == Prompt::Brackets {
                p.text(v.x + cw, y, "[", dim, cw + 1.0);
                p.text(v.x + (end + 1) as f32 * cw, y, "]", dim, cw + 1.0);
            } else {
                crate::ui::folder(p, ix, iy, iw, ih, t, dim);
            }
            let dur = info.duration.map(|d| {
                if d.as_millis() == 0 {
                    "< 1 ms".into()
                } else {
                    blocks::format_duration(d)
                }
            });
            let (text, color) = match (info.status(), dur) {
                (Status::Ok, Some(d)) => (d, dim),
                (Status::Failed(code), Some(d)) => (format!("✗ {code} · {d}"), theme.palette[1]),
                (Status::Failed(code), None) => (format!("✗ {code}"), theme.palette[1]),
                _ => continue,
            };
            let n = text.chars().count();
            // "Estado a la derecha": pegado al borde, con ✓ si fue bien.
            if style.blocks == Blocks::Status {
                let ok = matches!(info.status(), Status::Ok);
                let w = n + if ok { 2 } else { 0 };
                let free = w + 2 < cols
                    && (cols - w - 1..cols).all(|c| matches!(row[Column(c)].c, ' ' | '\0'))
                    && end + 3 + w < cols;
                if free {
                    let x = v.x + (cols - w) as f32 * cw;
                    if ok {
                        p.text(x, y, "✓", theme.palette[2], cw + 1.0);
                    }
                    let x = if ok { x + 2.0 * cw } else { x };
                    p.text(x, y, &text, color, n as f32 * cw + 1.0);
                }
                continue;
            }
            if end + 3 + n < cols {
                p.text(
                    v.x + (end + 3) as f32 * cw,
                    y,
                    &text,
                    color,
                    n as f32 * cw + 1.0,
                );
            }
        }
    }
}

/// Etiqueta del prompt actual (la carpeta y la rama de git) según `Style::prompt`. La ruta es
/// texto real de la rejilla: aquí solo se dibuja lo de alrededor; su color lo pone `draw_terminal`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prompt_label(
    p: &mut Painter,
    theme: &Theme,
    cw: f32,
    ch: f32,
    cols: usize,
    v: &TermView,
    y: f32,
    end: usize,
    branch: Option<&str>,
    s: f32,
) {
    let style = theme.style;
    let t = (1.3 * s).max(1.0);
    let pad = (2.0 * s).round();
    let r = style.r(s);
    let hair = s.round().max(1.0);
    let strip = (3.0 * s).round();
    let line = (2.0 * s).round().max(2.0);
    let fg = theme.foreground;
    let dimc = fg.lerp(theme.background, 0.5);
    let (iw, ih) = ((cw * 1.05).round(), (ch * 0.40).round());
    let (ix, iy) = ((v.x + 0.5 * cw).round(), (y + (ch - ih) / 2.0).round());
    let x0 = v.x - 0.1 * cw;
    let x1 = v.x + (end + 1) as f32 * cw + 0.6 * cw;
    let (cy, chh) = (y + pad, ch - 2.0 * pad);
    let mut icon = theme.accent;
    // Etiqueta de la carpeta
    match style.prompt {
        Prompt::Border => {
            p.rounded(x0, cy, x1 - x0, chh, fg, 0.26, r);
            p.rounded(
                x0 + hair,
                cy + hair,
                x1 - x0 - 2.0 * hair,
                chh - 2.0 * hair,
                crate::ui::surface(theme, 0.08),
                1.0,
                (r - hair).max(0.0),
            );
        }
        Prompt::Soft => p.rounded(
            x0,
            cy,
            x1 - x0,
            chh,
            theme.background.lerp(theme.accent, 0.16),
            1.0,
            r,
        ),
        Prompt::Solid => {
            p.rounded(x0, cy, x1 - x0, chh, theme.accent, 1.0, r);
            icon = theme.background;
        }
        Prompt::Tab => {
            let fill = crate::ui::surface(theme, 0.08);
            p.rounded(x0, cy, x1 - x0, chh, fill, 1.0, r);
            p.rect(x0, cy, r, chh, fill, 1.0);
            p.rect(x0, cy, strip, chh, theme.accent, 1.0);
        }
        Prompt::Underline => {
            p.rect(
                ix,
                y + ch - pad - line,
                v.x + (end + 1) as f32 * cw - ix,
                line,
                theme.accent,
                1.0,
            );
        }
        Prompt::Brackets => {
            p.text(v.x + cw, y, "[", dimc, cw + 1.0);
            p.text(v.x + (end + 1) as f32 * cw, y, "]", dimc, cw + 1.0);
        }
        Prompt::Dimmed | Prompt::Plain => {}
    }
    if style.prompt != Prompt::Brackets {
        crate::ui::folder(p, ix, iy, iw, ih, t, icon);
    }

    // Rama de git, si cabe.
    let Some(b) = branch else { return };
    let n = b.chars().count();
    if end + 7 + n >= cols {
        return;
    }
    let mag = theme.palette[5];
    let name_c = fg.lerp(theme.background, 0.2);
    let (bw, bh) = ((cw * 0.85).round(), (ch * 0.42).round());
    let by = (y + (ch - bh) / 2.0).round();
    match style.prompt {
        Prompt::Brackets => {
            let x = v.x + (end + 2) as f32 * cw;
            p.text(x, y, "─[", dimc, 2.0 * cw + 1.0);
            p.text(x + 2.0 * cw, y, b, mag, n as f32 * cw + 1.0);
            p.text(x + (2 + n) as f32 * cw, y, "]", dimc, cw + 1.0);
        }
        Prompt::Dimmed | Prompt::Plain | Prompt::Underline => {
            let bx0 = v.x + (end + 3) as f32 * cw;
            crate::ui::branch(p, (bx0 + 0.2 * cw).round(), by, bw, bh, t, mag);
            let c = if style.prompt == Prompt::Underline {
                name_c
            } else {
                dimc
            };
            p.text(bx0 + 1.5 * cw, y, b, c, n as f32 * cw + 1.0);
            if style.prompt == Prompt::Underline {
                p.rect(
                    bx0,
                    y + ch - pad - line,
                    (n as f32 + 1.5) * cw,
                    line,
                    mag,
                    1.0,
                );
            }
        }
        _ => {
            let bx0 = x1 + 0.6 * cw;
            let bx1 = bx0 + (n as f32 + 2.6) * cw;
            match style.prompt {
                Prompt::Soft => p.rounded(
                    bx0,
                    cy,
                    bx1 - bx0,
                    chh,
                    theme.background.lerp(mag, 0.16),
                    1.0,
                    r,
                ),
                Prompt::Solid => p.outline(bx0, cy, bx1 - bx0, chh, fg, 0.26, r, hair),
                Prompt::Tab => {
                    let fill = crate::ui::surface(theme, 0.08);
                    p.rounded(bx0, cy, bx1 - bx0, chh, fill, 1.0, r);
                    p.rect(bx0, cy, r, chh, fill, 1.0);
                    p.rect(bx0, cy, strip, chh, mag, 1.0);
                }
                _ => p.rounded(bx0, cy, bx1 - bx0, chh, fg, 0.06, r),
            }
            let lead = if style.prompt == Prompt::Tab {
                0.3 * cw
            } else {
                0.0
            };
            crate::ui::branch(p, (bx0 + 0.5 * cw + lead).round(), by, bw, bh, t, mag);
            p.text(bx0 + 1.8 * cw + lead, y, b, name_c, n as f32 * cw + 1.0);
        }
    }
}

/// Muestra del prompt de Garbo para el editor de aspecto, con la fuente del terminal: la ruta
/// (con su etiqueta según `theme.style.prompt`, la rama al lado si cabe), el hueco bajo la ruta y
/// un comando debajo. `y` es el borde de arriba.
pub fn prompt_sample(p: &mut Painter, theme: &Theme, x: f32, y: f32, w: f32, s: f32) {
    let m = p.metrics();
    let (cw, ch) = (m.width, m.height);
    let (path, branch) = ("~/ui", "main");
    let end = 1 + path.chars().count();
    // La rama solo si cabe de verdad (`prompt_label` pide más sitio del que usa).
    let (n, need) = (
        branch.chars().count(),
        end as f32 + branch.len() as f32 + 4.0,
    );
    let cols = if need * cw <= w { end + 8 + n } else { 0 };
    let v = TermView {
        x,
        y,
        lines: 2,
        focused: true,
        cursor_on: false,
        rows: Rows::default(),
    };
    prompt_label(p, theme, cw, ch, cols, &v, y, end, Some(branch), s);
    let slash = path.rfind('/').unwrap_or(0);
    for (i, c) in path.chars().enumerate() {
        let (style, color) = match theme.style.prompt {
            Prompt::Solid => (Style::Bold, theme.background),
            Prompt::Brackets => (Style::Bold, theme.accent),
            Prompt::Dimmed if i <= slash => (
                Style::Regular,
                theme.foreground.lerp(theme.background, 0.45),
            ),
            Prompt::Dimmed => (Style::Bold, theme.accent),
            _ => (Style::Bold, theme.foreground),
        };
        p.glyph(
            c,
            style,
            false,
            false,
            x + (2 + i) as f32 * cw,
            y,
            color,
            1.0,
        );
    }
    let cy = y + ch + theme.style.gap(s);
    for (i, c) in "ls".chars().enumerate() {
        p.glyph(
            c,
            Style::Bold,
            false,
            false,
            x + i as f32 * cw,
            cy,
            theme.foreground,
            1.0,
        );
    }
}

/// Menú de completado bajo la palabra que se completa (encima si no cabe debajo). Guarda en
/// `menu.rect` dónde quedó, para el ratón.
pub fn draw_menu<T: EventListener>(
    p: &mut Painter,
    term: &Term<T>,
    theme: &Theme,
    v: &TermView,
    menu: &mut Menu,
    scale: f32,
) {
    let items = &menu.comp.items;
    menu.rect = None;
    if items.is_empty() {
        return;
    }
    let m = p.metrics();
    let s = scale;
    let cur = term.grid().cursor.point;
    let row = cur.line.0.max(0);
    let area_w = term.columns() as f32 * m.width;
    let n = items.len();
    let vis = n.min(MENU_ROWS);
    let pad = (6.0 * s).round();
    let row_h = (m.height + 8.0 * s).round();
    let mark_w = (16.0 * s).round();
    let bar_w = if n > vis { (8.0 * s).round() } else { 0.0 };
    let gap = (18.0 * s).round();

    let longest = items
        .iter()
        .map(|i| i.label.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(6, 48);
    // La descripción solo si aporta algo (si todas dicen "carpeta", sobra).
    let same = items.iter().all(|i| i.detail == items[0].detail);
    let detail_w = if same {
        0.0
    } else {
        items
            .iter()
            .take(64)
            .map(|i| p.ui_measure_v(&i.detail, UiVariant::Small))
            .fold(0.0, f32::max)
            .min(260.0 * s)
    };
    let want = 2.0 * pad + mark_w + longest as f32 * m.width + gap + detail_w + 2.0 * pad + bar_w;
    let w = want.min(area_w).max((160.0 * s).min(area_w)).round();
    let h = (vis as f32 * row_h + 2.0 * pad).round();
    // La lista empieza bajo el principio de la palabra.
    let col = cur.column.0.saturating_sub(menu.comp.word.chars().count()) as f32;
    let x = (v.x + col * m.width - pad - mark_w)
        .clamp(v.x, (v.x + area_w - w).max(v.x))
        .round();
    let bottom = v.row_y(v.lines as i32, m.height);
    let below = v.row_y(row, m.height) + m.height + 2.0 * s;
    let above = v.row_y(row, m.height) - h - 2.0 * s;
    let y = if below + h <= bottom || above < v.y {
        below
    } else {
        above
    }
    .round();

    p.card(
        x,
        y,
        w,
        h,
        crate::ui::surface(theme, 0.06),
        theme.foreground,
        (9.0 * s).round(),
        s,
    );
    let dimc = crate::ui::dim(theme);
    let label_x = x + pad + mark_w;
    let detail_room = if detail_w > 0.0 { detail_w + gap } else { 0.0 };
    let fit = (((w - 2.0 * pad - mark_w - pad - bar_w - detail_room) / m.width)
        .floor()
        .max(4.0)) as usize;
    let small_h = p.ui_line_h(UiVariant::Small);
    for r in 0..vis {
        let idx = menu.top + r;
        let Some(it) = items.get(idx) else { break };
        let ry = y + pad + r as f32 * row_h;
        if idx == menu.selected {
            p.rounded(
                x + pad / 2.0,
                ry,
                w - pad - bar_w,
                row_h,
                theme.accent,
                0.20,
                (6.0 * s).round(),
            );
        }
        let kc = theme.palette[it.kind.ansi()];
        p.circle(
            x + pad + mark_w / 2.0 - 1.0 * s,
            ry + row_h / 2.0,
            (3.0 * s).max(2.0),
            kc,
            0.9,
        );
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
                p.glyph(
                    ch,
                    style,
                    false,
                    false,
                    label_x + k as f32 * m.width,
                    ty,
                    color,
                    1.0,
                );
            }
        }
        if detail_w > 0.0 && !it.detail.is_empty() {
            let dw = p.ui_measure_v(&it.detail, UiVariant::Small).min(detail_w);
            let dx = x + w - 2.0 * pad - bar_w - dw;
            p.ui_text_v(
                dx.round(),
                (ry + (row_h - small_h) / 2.0).round(),
                &it.detail,
                dimc,
                detail_w + 1.0,
                UiVariant::Small,
            );
        }
    }
    // Barra de desplazamiento cuando hay más de los que caben.
    if n > vis {
        let track = h - 2.0 * pad;
        let th = (track * vis as f32 / n as f32).max(14.0 * s);
        let ty = y + pad + (track - th) * menu.top as f32 / (n - vis) as f32;
        let bw = (3.0 * s).max(2.0);
        p.rounded(
            x + w - pad - bw,
            ty.round(),
            bw,
            th.round(),
            theme.foreground,
            0.28,
            bw / 2.0,
        );
    }
    menu.rect = Some([x, y, w, h, row_h, pad]);
}

pub struct TermView {
    pub x: f32,
    pub y: f32,
    pub lines: usize,
    pub focused: bool,
    pub cursor_on: bool,
    /// Dónde va cada fila (con el hueco bajo la ruta del prompt de Garbo: ver `Rows`).
    pub rows: Rows,
}

impl TermView {
    /// Borde superior de la fila `row` del viewport (`lines`: el inferior de la última).
    pub fn row_y(&self, row: i32, ch: f32) -> f32 {
        self.y + self.rows.y(row, ch)
    }

    /// ¿Se dibuja la fila `row` del viewport?
    pub fn shows(&self, row: i32) -> bool {
        row >= self.rows.first as i32 && row < self.lines as i32
    }
}

/// Posición vertical de las filas del terminal. Sin huecos van seguidas (fila · alto). Con los de
/// `Style` (`Gaps`), cada bloque del prompt de Garbo puede llevar un hueco sobre la ruta, otro bajo
/// la ruta y otro entre el comando y su salida, y lo de debajo baja; si así no cabe hasta el cursor
/// (o hasta lo último escrito), se ocultan filas enteras por arriba. La rejilla no cambia: los
/// programas ven las mismas filas, solo cambia dónde se dibujan.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct Rows {
    /// y de cada fila respecto a `TermView::y` y, al final, el borde inferior de la última.
    /// Vacío: sin huecos.
    ys: Vec<f32>,
    /// Primera fila que se dibuja (las anteriores quedan por encima del panel).
    pub first: usize,
}

impl Rows {
    /// Reparto para lo que se ve de `term`: `lines` filas de `ch` píxeles, los huecos `gaps` y
    /// `room` píxeles de alto disponibles.
    pub fn layout<T>(
        term: &Term<T>,
        bs: &BlockState,
        lines: usize,
        ch: f32,
        gaps: crate::style::Gaps,
        room: f32,
    ) -> Self {
        if !gaps.any() || !bs.active || lines == 0 {
            return Self::default();
        }
        let offset = term.grid().display_offset() as i32;
        let bottom = Line(-offset + lines as i32 - 1);
        let line = |r: usize| Line(r as i32 - offset);
        let cursor = term.grid().cursor.point.line.0 + offset;
        let mut ys: Vec<f32> = (0..=lines).map(|r| r as f32 * ch).collect();
        let mut any = false;
        // Un hueco antes de la fila `from` (todo lo de debajo baja).
        let mut add = |ys: &mut Vec<f32>, from: usize, gap: f32| {
            if gap >= 0.5 && from < lines {
                any = true;
                for y in &mut ys[from..] {
                    *y += gap;
                }
            }
        };
        let anchors = blocks::anchors(term, Line(-offset), bottom);
        for (k, a) in anchors.iter().enumerate() {
            if !bs.styled(a.id) {
                continue;
            }
            let start = (a.line.0 + offset) as usize;
            // Sobre la ruta, si hay algo encima a la vista.
            if start > 0 {
                add(&mut ys, start, gaps.above);
            }
            // La ruta ocupa más de una fila si no cabe en una: el hueco va tras la última.
            let mut l = a.line;
            while l < bottom && blocks::line_anchor(term, l + 1) == Some(a.id) {
                l += 1;
            }
            let path_end = (l.0 + offset) as usize;
            add(&mut ys, path_end + 1, gaps.path);
            // Antes de la salida: tras la última fila del comando, si ya se ejecutó y debajo hay
            // salida (o va a haberla: el cursor está ahí). Sin salida (`cd`) no hay hueco.
            let started = bs.info(a.id).is_some_and(|i| i.started.is_some());
            if gaps.output >= 0.5 && started {
                let next = anchors
                    .get(k + 1)
                    .map_or(lines, |n| (n.line.0 + offset) as usize)
                    .min(lines);
                let mut cmd_end = path_end + 1;
                let mut r = path_end + 1;
                while r < next && blocks::line_has(term, line(r), b'c', a.id) {
                    cmd_end = r;
                    r += 1;
                }
                let out = cmd_end + 1;
                let used = out as i32 <= cursor || !term.grid()[line(out)].is_clear();
                if out < next && used {
                    add(&mut ys, out, gaps.output);
                }
            }
        }
        if !any {
            return Self::default();
        }
        // Tiene que verse hasta el cursor y lo último escrito (los programas de pantalla completa
        // usan la pantalla alternativa, que no tiene prompts ni huecos).
        let grid = term.grid();
        let mut last = (0..lines)
            .rev()
            .find(|&r| !grid[Line(r as i32 - offset)].is_clear())
            .unwrap_or(0);
        let cursor = grid.cursor.point.line.0 + offset;
        if (0..lines as i32).contains(&cursor) {
            last = last.max(cursor as usize);
        }
        let mut first = 0;
        let over = ys[last + 1] - room;
        if over > 0.0 {
            // Filas enteras: la primera que se dibuja empieza justo arriba.
            first = ys.partition_point(|&y| y < over).min(last);
            let shift = ys[first];
            for y in &mut ys {
                *y -= shift;
            }
        }
        Self { ys, first }
    }

    /// Borde superior de la fila `row` respecto a `TermView::y`.
    pub fn y(&self, row: i32, ch: f32) -> f32 {
        let Some(last) = self.ys.len().checked_sub(1) else {
            return row as f32 * ch;
        };
        let r = row.clamp(0, last as i32);
        self.ys[r as usize] + (row - r) as f32 * ch
    }

    /// Fila del viewport a la altura `y` (respecto a `TermView::y`). El hueco cuenta como la fila
    /// de encima (la ruta).
    pub fn row_at(&self, y: f32, ch: f32, lines: usize) -> usize {
        let max = lines.saturating_sub(1);
        if self.ys.is_empty() {
            return ((y / ch).max(0.0) as usize).min(max);
        }
        self.ys
            .partition_point(|&t| t <= y)
            .saturating_sub(1)
            .clamp(self.first.min(max), max)
    }
}

/// `bs`: bloques de la pestaña, para dar estilo al prompt de Garbo (ver `draw_prompts`).
/// Subraya las celdas de `start` a `end` (enlace bajo el ratón con Ctrl).
#[allow(clippy::too_many_arguments)]
pub fn draw_underline<T: EventListener>(
    p: &mut Painter,
    term: &Term<T>,
    theme: &Theme,
    v: &TermView,
    start: alacritty_terminal::index::Point,
    end: alacritty_terminal::index::Point,
    scale: f32,
) {
    let m = p.metrics();
    let off = term.grid().display_offset() as i32;
    let cols = term.columns();
    let t = (1.5 * scale).round().max(1.0);
    for l in start.line.0..=end.line.0 {
        let row = l + off;
        if !v.shows(row) {
            continue;
        }
        let a = if l == start.line.0 { start.column.0 } else { 0 };
        let b = if l == end.line.0 {
            end.column.0 + 1
        } else {
            cols
        };
        let y = v.row_y(row, m.height) + m.underline_pos.min(m.height - t);
        p.rect(
            v.x + a as f32 * m.width,
            y.round(),
            (b - a) as f32 * m.width,
            t,
            theme.accent,
            1.0,
        );
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
    let show_cursor = v.cursor_on && cursor.shape != CursorShape::Hidden && v.shows(cursor_row);
    let block_cursor = show_cursor && v.focused && cursor.shape == CursorShape::Block;

    // Búsqueda: coincidencias en amarillo suave; la actual, en amarillo.
    let found = theme.palette[3].lerp(theme.background, 0.6);
    let mut hl_next = 0;

    // Fusión de fondos contiguos del mismo color en una sola instancia.
    let mut run: Option<(i32, f32, f32, Rgb)> = None;
    let flush = |p: &mut Painter, run: &mut Option<(i32, f32, f32, Rgb)>| {
        if let Some((row, x0, x1, c)) = run.take() {
            p.rect(x0, v.row_y(row, chh), x1 - x0, chh, c, 1.0);
        }
    };

    // Atajo para la mayoría de celdas: un espacio sobre el fondo por defecto no dibuja nada.
    // Con selección o búsqueda activas se recorre todo por el camino normal.
    let default_bg = AnsiColor::Named(NamedColor::Background);
    let skip_blank = selection.is_none()
        && hl.is_none()
        && resolve(default_bg, theme, colors) == theme.background;
    let visible = Flags::INVERSE | Flags::ALL_UNDERLINES | Flags::STRIKEOUT;
    // Ruta atenuada: (línea, columna de la última '/') del prompt que se está dibujando.
    let mut last_slash: Option<(i32, Option<usize>)> = None;

    for indexed in content.display_iter {
        let point = indexed.point;
        let row = point.line.0 + offset;
        if !v.shows(row) {
            continue;
        }
        let cell = indexed.cell;
        let flags = cell.flags;
        if skip_blank
            && (cell.c == ' ' || cell.c == '\t')
            && cell.bg == default_bg
            && !flags.intersects(visible)
            && !(block_cursor && point == cursor.point)
        {
            continue;
        }
        if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let wide = flags.contains(Flags::WIDE_CHAR);
        let x = v.x + point.column.0 as f32 * cw;
        let y = v.row_y(row, chh);
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
                Some((r, _, x1, c)) if *r == row && (*x1 - x).abs() < 0.5 && *c == bg => {
                    *x1 = x + w
                }
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
        if let Some((kind, id)) =
            bs.and_then(|bs| blocks::cell_link(cell).filter(|(_, id)| bs.styled(*id)))
        {
            match kind {
                b'p' if current_prompt == Some(id) => {
                    bold = true;
                    fg = match theme.style.prompt {
                        Prompt::Solid => theme.background,
                        Prompt::Brackets => theme.accent,
                        // Las carpetas de la ruta en gris; la última, en color.
                        Prompt::Dimmed => {
                            let line = point.line.0;
                            if last_slash.is_none_or(|(l, _)| l != line) {
                                let row = &term.grid()[point.line];
                                let at = (0..term.columns()).rev().find(|&c| {
                                    row[Column(c)].c == '/'
                                        && blocks::cell_link(&row[Column(c)]) == Some((b'p', id))
                                });
                                last_slash = Some((line, at));
                            }
                            let parent = last_slash
                                .and_then(|(_, at)| at)
                                .is_some_and(|at| point.column.0 <= at);
                            if parent {
                                bold = false;
                                theme.foreground.lerp(theme.background, 0.45)
                            } else {
                                theme.accent
                            }
                        }
                        _ => theme.foreground,
                    };
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
            } else if flags
                .intersects(Flags::UNDERCURL | Flags::DOTTED_UNDERLINE | Flags::DASHED_UNDERLINE)
            {
                let step = (cw / 4.0).max(2.0);
                let mut sx = x;
                let mut up = true;
                while sx < x + w {
                    let dy = if flags.contains(Flags::UNDERCURL) && up {
                        -s
                    } else {
                        0.0
                    };
                    let seg = if flags.contains(Flags::DASHED_UNDERLINE) {
                        step * 1.5
                    } else {
                        step / 2.0
                    };
                    p.fg.push(Instance::rect(
                        sx,
                        uy + dy,
                        seg.min(x + w - sx),
                        s,
                        uc.to_f32(1.0),
                    ));
                    sx += step;
                    up = !up;
                }
            } else {
                p.fg.push(Instance::rect(x, uy, w, s, uc.to_f32(1.0)));
            }
        }
        if flags.contains(Flags::STRIKEOUT) {
            p.fg.push(Instance::rect(
                x,
                y + (m.baseline * 0.62).round(),
                w,
                s,
                fg.to_f32(1.0),
            ));
        }
    }
    flush(p, &mut run);

    // Cursor (los modos no-bloque o sin foco se dibujan encima)
    if show_cursor && !block_cursor {
        let x = v.x + cursor.point.column.0 as f32 * cw;
        let y = v.row_y(cursor_row, chh);
        let wide = term.grid()[cursor.point].flags.contains(Flags::WIDE_CHAR);
        let w = if wide { 2.0 * cw } else { cw };
        let t = (m.stroke * 2.0).max(2.0);
        let c = theme.cursor.to_f32(1.0);
        let shape = if v.focused {
            cursor.shape
        } else {
            CursorShape::HollowBlock
        };
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
        if let Some(a) = blocks::block_above(term, bs, top, anchors.first()) {
            anchors.insert(0, a);
        }
    }
    let cursor_line = term.grid().cursor.point.line;
    let dim = theme.foreground.lerp(theme.background, 0.45);
    let (ok, err) = (theme.palette[2], theme.palette[1]);
    let bar_w = (3.0 * scale).round().max(2.0);
    // La barra de estado va en el margen izquierdo del panel (de `span.0` a `v.x`).
    let bar_x = (span.0 + (v.x - span.0) * 0.55 - bar_w / 2.0)
        .round()
        .max(span.0);
    let width = span.1 - span.0;
    let line_w = scale.round().max(1.0);
    let style = theme.style;

    for (i, a) in anchors.iter().enumerate() {
        let end_line = match anchors.get(i + 1) {
            Some(next) => next.line - 1,
            None if Some(a.id) == bs.current => cursor_line,
            None => bottom,
        };
        let first = v.rows.first as i32;
        let start_row = (a.line.0 + offset).max(first);
        let end_row = (end_line.0 + offset).min(lines - 1);
        if end_row < start_row {
            continue;
        }
        let y0 = v.row_y(start_row, ch);
        let y1 = v.row_y(end_row + 1, ch);
        let starts_here = a.line.0 + offset >= first;
        // La línea entre bloques va en medio del hueco sobre la ruta (`Style::block_gap`), no
        // pegada a la ruta: así el espacio queda a los dos lados.
        let above = if start_row > first {
            (y0 - v.row_y(start_row - 1, ch) - ch).max(0.0)
        } else {
            0.0
        };
        let sep_y = (y0 - above / 2.0).round();
        let info = bs.info(a.id);
        let status = info.map_or(Status::Idle, |i| i.status());
        let is_sel = selected == Some(a.id);

        if is_sel {
            p.rect(span.0, y0, width, y1 - y0, theme.accent, 0.10);
        }
        let finished = matches!(status, Status::Ok | Status::Failed(_));
        let failed = matches!(status, Status::Failed(_));
        let fg = theme.foreground;
        let s = scale;
        let r = style.r(s);
        let pad = (3.0 * s).round();
        // Tarjetas y contornos: por fuera de las celdas, sin salirse del panel.
        let (cx0, cx1) = (
            (v.x - 6.0 * s).max(span.0),
            (v.x + cols as f32 * m.width + 6.0 * s).min(span.1),
        );
        let status_bar = |p: &mut Painter, c: Rgb, a: f32| {
            p.rounded(
                bar_x,
                y0 + pad,
                bar_w,
                (y1 - y0 - 2.0 * pad).max(bar_w),
                c,
                a,
                bar_w / 2.0,
            );
        };
        match style.blocks {
            Blocks::Classic => {
                if starts_here && start_row > first {
                    p.rect(v.x, sep_y, width - 2.0 * (v.x - span.0), line_w, fg, 0.10);
                }
                let bar = match status {
                    _ if is_sel => Some((theme.accent, 1.0)),
                    Status::Ok => Some((ok, 0.6)),
                    Status::Failed(_) => Some((err, 1.0)),
                    Status::Running => Some((theme.accent, 1.0)),
                    Status::Idle => None,
                };
                if let Some((c, a)) = bar {
                    status_bar(p, c, a);
                }
            }
            Blocks::Bar => {
                let (c, a) = match status {
                    _ if is_sel => (theme.accent, 1.0),
                    Status::Ok => (fg, 0.22),
                    Status::Failed(_) => (err, 1.0),
                    _ => (theme.accent, 1.0),
                };
                status_bar(p, c, a);
            }
            Blocks::Dot if starts_here => {
                let c = match status {
                    Status::Ok => ok,
                    Status::Failed(_) => err,
                    _ => theme.accent,
                };
                p.circle(bar_x + bar_w / 2.0, y0 + ch / 2.0, 3.0 * s, c, 1.0);
            }
            Blocks::Card if finished => {
                let (h0, h1) = (y0 + 2.0 * s, y1 - 2.0 * s);
                let (c, fa, la) = if failed {
                    (err, 0.07, 0.32)
                } else {
                    (fg, 0.04, 0.08)
                };
                p.rounded(cx0, h0, cx1 - cx0, h1 - h0, c, fa, r);
                p.outline(cx0, h0, cx1 - cx0, h1 - h0, c, la, r, line_w);
            }
            Blocks::Outline if finished => {
                let (h0, h1) = (y0 + 2.0 * s, y1 - 2.0 * s);
                let (c, a) = if failed { (err, 0.40) } else { (fg, 0.16) };
                p.outline(cx0, h0, cx1 - cx0, h1 - h0, c, a, r, line_w);
            }
            Blocks::Header if finished && starts_here => {
                let (c, a) = if failed { (err, 0.13) } else { (fg, 0.06) };
                p.rect(span.0, y0, width, ch, c, a);
            }
            Blocks::Separator if starts_here && start_row > first => {
                p.rect(span.0, sep_y, width, line_w, fg, 0.12);
            }
            Blocks::Dashed if starts_here && start_row > first => {
                let (dash, step) = ((6.0 * s).round(), (11.0 * s).round());
                let x_end = v.x + cols as f32 * m.width;
                let mut x = v.x;
                while x < x_end {
                    p.rect(x, sep_y, dash.min(x_end - x), line_w, fg, 0.26);
                    x += step;
                }
            }
            Blocks::Errors if failed => {
                p.rect(span.0, y0, width, y1 - y0, err, 0.10);
                p.rect(span.0, y0, bar_w, y1 - y0, err, 1.0);
            }
            // Por encima del texto (capa `fg`): lo ya ejecutado se ve más apagado.
            Blocks::Fade if finished && !is_sel => {
                p.fg.push(Instance::rect(
                    span.0,
                    y0,
                    width,
                    y1 - y0,
                    crate::ui::terminal_top(theme).to_f32(0.5),
                ));
            }
            _ => {}
        }

        // Estado y duración a la derecha de la línea del prompt, si ese hueco está libre (con el
        // prompt de Garbo van tras la ruta: ver `draw_prompts`).
        let (Some(info), true) = (info.filter(|i| !i.styled), starts_here) else {
            continue;
        };
        // Sin ruido: en los correctos solo la duración si fue apreciable; los fallos siempre.
        let slow = info
            .duration
            .filter(|d| d.as_secs_f32() >= 1.0)
            .map(blocks::format_duration);
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
            p.text(
                x + 2.0 * m.width,
                y0,
                &text,
                if matches!(status, Status::Failed(_)) {
                    err
                } else {
                    dim
                },
                n as f32 * m.width,
            );
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
    // Todo lo que se dibuja con primitivas está en los bloques de caja y de elementos de bloque.
    if !('\u{2500}'..='\u{259F}').contains(&c) {
        return false;
    }
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
            out.push(Instance::rect(
                l,
                t,
                r - l,
                b - t,
                [col[0], col[1], col[2], a],
            ));
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
            if q & 1 != 0 {
                fill(x, y, mx, my, 1.0)
            }
            if q & 2 != 0 {
                fill(mx, y, x1, my, 1.0)
            }
            if q & 4 != 0 {
                fill(x, my, mx, y1, 1.0)
            }
            if q & 8 != 0 {
                fill(mx, my, x1, y1, 1.0)
            }
        }
        _ => {
            let base = (w / 8.0).round().max(1.0);
            let t = |k: u8| if k == 2 { base * 2.0 } else { base };
            // Líneas discontinuas: (horizontal, nº de trazos, grosor)
            let dashed = match c {
                '╌' => Some((true, 2, 1)),
                '╍' => Some((true, 2, 2)),
                '┄' => Some((true, 3, 1)),
                '┅' => Some((true, 3, 2)),
                '┈' => Some((true, 4, 1)),
                '┉' => Some((true, 4, 2)),
                '╎' => Some((false, 2, 1)),
                '╏' => Some((false, 2, 2)),
                '┆' => Some((false, 3, 1)),
                '┇' => Some((false, 3, 2)),
                '┊' => Some((false, 4, 1)),
                '┋' => Some((false, 4, 2)),
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
                '╭' => Some((1.0, 1.0)),
                '╮' => Some((-1.0, 1.0)),
                '╰' => Some((1.0, -1.0)),
                '╯' => Some((-1.0, -1.0)),
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
                '═' => Some((2, 0)),
                '║' => Some((0, 2)),
                '╔' => Some((1, 1)),
                '╗' => Some((-1, 1)),
                '╚' => Some((1, -1)),
                '╝' => Some((-1, -1)),
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

    /// El hueco bajo la ruta baja lo de debajo; si no cabe, se ocultan filas enteras por arriba y
    /// el cursor sigue a la vista. El ratón cae en la fila que se ve.
    #[test]
    fn hueco_entre_la_ruta_y_el_comando() {
        let cfg = crate::config::Config::default();
        let s = crate::look::Sample::new(&cfg, 80);
        let (ch, lines) = (20.0, s.lines);
        let cur = s.term.grid().cursor.point.line.0;
        use crate::style::Gaps;
        let path = |g: f32| Gaps {
            path: g,
            ..Gaps::default()
        };
        let none = Rows::layout(&s.term, &s.blocks, lines, ch, path(0.0), f32::INFINITY);
        assert_eq!(none, Rows::default());
        assert_eq!(none.y(3, ch), 60.0);
        assert_eq!(none.row_at(61.0, ch, lines), 3);

        let r = Rows::layout(&s.term, &s.blocks, lines, ch, path(8.0), f32::INFINITY);
        assert_eq!(r.first, 0);
        // La muestra empieza con la ruta del primer prompt: el comando, 8 px más abajo.
        assert_eq!((r.y(0, ch), r.y(1, ch)), (0.0, 28.0));
        assert_eq!(r.row_at(24.0, ch, lines), 0, "el hueco es de la ruta");
        assert_eq!(r.row_at(28.0, ch, lines), 1);
        // Tres comandos y el prompt actual: cuatro huecos por encima de lo que se escribe.
        assert_eq!(r.y(cur, ch), cur as f32 * ch + 4.0 * 8.0);
        assert_eq!(r.row_at(r.y(cur, ch) + 1.0, ch, lines), cur as usize);

        // Justo el alto que hacía falta sin hueco: no cabe y se ocultan las dos primeras filas.
        let room = (cur + 1) as f32 * ch;
        let tight = Rows::layout(&s.term, &s.blocks, lines, ch, path(8.0), room);
        assert_eq!(tight.first, 2);
        assert_eq!(tight.y(2, ch), 0.0);
        assert!(tight.y(cur + 1, ch) <= room);
        assert_eq!(
            tight.row_at(-5.0, ch, lines),
            2,
            "lo oculto no se puede señalar"
        );
        // Sobre la ruta y antes de la salida. La muestra: ruta (0), "git status" (1) y su salida
        // desde la fila 2; el segundo bloque empieza en `b`.
        let g = Gaps {
            above: 4.0,
            path: 0.0,
            output: 6.0,
        };
        let r = Rows::layout(&s.term, &s.blocks, lines, ch, g, f32::INFINITY);
        assert_eq!(r.y(1, ch), ch, "nada sobre la primera ruta ni bajo ella");
        assert_eq!(r.y(2, ch) - r.y(1, ch), ch + 6.0, "antes de la salida");
        let all = crate::blocks::anchors(&s.term, Line(0), Line(lines as i32 - 1));
        let b = all[1].line.0;
        assert_eq!(
            r.y(b, ch) - r.y(b - 1, ch),
            ch + 4.0,
            "sobre la ruta del segundo"
        );
        // El prompt actual (sin ejecutar) no lleva hueco antes de la salida.
        let cur = all.last().unwrap().line.0;
        assert_eq!(r.y(cur + 2, ch) - r.y(cur + 1, ch), ch);
    }

    /// Banco de medida del coste de construir un fotograma: prompts, rejilla y bloques (CPU, sin
    /// ventana).
    /// `cargo test --release bench_ -- --ignored --nocapture --test-threads=1`
    #[test]
    #[ignore = "banco de medida"]
    fn bench_frame() {
        use alacritty_terminal::event::VoidListener;
        use alacritty_terminal::term::test::TermSize;

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
            eprintln!("sin GPU: se omite");
            return;
        };
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: crate::atlas::ATLAS_SIZE,
                height: crate::atlas::ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let cfg = crate::config::Config::default();
        let theme = cfg.theme();
        let mut glyphs = crate::app::build_glyphs(&cfg, 11.0, 1.67);
        let (cols, lines) = (180, 50);

        // Pantallas de prueba: texto plano, texto con muchos colores y casi vacía.
        let plain: String = (0..lines)
            .map(|i| format!("{:<180}\r\n", format!("{i:>6}  fn draw_terminal(p: &mut Painter, term: &Term<T>) -> Option<usize> {{ /* {i} */ }}")))
            .collect();
        let colored: String = (0..lines * 12)
            .map(|i| {
                let sep = if i % 12 == 11 { "\r\n" } else { " " };
                format!(
                    "\x1b[38;5;{}m{}palabra{i:<4}\x1b[0m{sep}",
                    i % 230 + 16,
                    if i % 7 == 0 { "\x1b[1;48;5;236m" } else { "" }
                )
            })
            .collect();
        let empty = "\x1b[1m~/Documentos/Proyectos\x1b[0m $ ls -la".to_string();
        // Un comando en marcha cuyo prompt quedó 1500 líneas por encima de la pantalla.
        let running = format!(
            "\x1b]133;A\x07$ \x1b]133;B\x07seq 1500\r\n\x1b]133;C\x07{}",
            (1..=1500).map(|i| format!("{i}\r\n")).collect::<String>()
        );

        for (name, text) in [
            ("plana", plain),
            ("colores", colored),
            ("vacía", empty),
            ("bloques", running),
        ] {
            let mut term = Term::new(
                crate::terminal::term_config(&cfg),
                &TermSize::new(cols, lines),
                VoidListener,
            );
            let blocks = std::sync::Mutex::new(BlockState::new(0));
            crate::pty_loop::feed_bytes(&mut term, &blocks, text.as_bytes(), &VoidListener);
            let bs = blocks.into_inner().unwrap();
            let v = TermView {
                x: 0.0,
                y: 0.0,
                lines,
                focused: true,
                cursor_on: true,
                rows: Rows::default(),
            };
            let (mut out, mut fg) = (Vec::new(), Vec::new());
            let mut times = Vec::new();
            for i in 0..1100 {
                out.clear();
                fg.clear();
                let t = std::time::Instant::now();
                let mut p = Painter {
                    out: &mut out,
                    fg: &mut fg,
                    glyphs: &mut glyphs,
                    queue: &queue,
                    atlas: &atlas,
                };
                draw_prompts(&mut p, &term, &theme, &v, &bs, None, 1.67);
                draw_terminal(&mut p, &term, &theme, &v, Some(&bs), None);
                draw_blocks(&mut p, &term, &theme, &v, &bs, None, (0.0, 1800.0), 1.67);
                p.finish();
                if i >= 100 {
                    times.push(t.elapsed().as_secs_f64() * 1e6);
                }
            }
            times.sort_by(f64::total_cmp);
            eprintln!(
                "fotograma {name:8} {cols}x{lines}: mediana {:7.1} µs, p10 {:7.1} µs, {} instancias",
                times[times.len() / 2],
                times[times.len() / 10],
                out.len()
            );
        }
    }

    fn rows(lines: &[&str]) -> Vec<Vec<(char, Rgb)>> {
        lines
            .iter()
            .map(|l| l.chars().map(|c| (c, Rgb(0, 0, 0))).collect())
            .collect()
    }

    #[test]
    fn corta_la_zona_de_entrada_del_agente() {
        let rule = "────────────────────────────────";
        // Contenido, línea, entrada en dos líneas, línea, barra de estado en tres líneas.
        let r = rows(&[
            "⏺ Hecho",
            "",
            "✻ Pensando…",
            rule,
            "❯ escribo algo",
            "  que sigue",
            rule,
            "Opus 5.5",
            "ctx 29%",
            "auto mode",
        ]);
        assert_eq!(input_area_top(&r), Some(3));
        // Con caja (╭ ╮ ╰ ╯) también.
        let r = rows(&[
            "salida",
            "otra",
            "╭──────────────╮",
            "│ > hola       │",
            "╰──────────────╯",
            "? atajos",
        ]);
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

    #[test]
    fn recorta_el_editor_de_codex_y_conserva_sus_mensajes() {
        use crate::subagents::Provider;
        let r = rows(&[
            "› Revisa el repositorio",
            "• Preparé el resumen",
            "",
            "› borrador sin enviar",
            "  segunda línea",
            "  / for commands",
        ]);
        assert_eq!(preview_end(&r, 4, Some(Provider::Codex)), Some(3));
        assert_eq!(preview_end(&r, 3, Some(Provider::Codex)), Some(3));
        assert_eq!(preview_end(&r[..3], 2, Some(Provider::Codex)), None);
    }

    #[test]
    fn recorta_el_editor_de_gemini_y_no_un_separador_aislado() {
        use crate::subagents::Provider;
        let r = rows(&[
            "✦ Revisé las pruebas",
            "╭──────────────────╮",
            "│ > borrador       │",
            "╰──────────────────╯",
            "  gemini",
        ]);
        assert_eq!(preview_end(&r, 2, Some(Provider::Gemini)), Some(1));
        // Ink deja el cursor bajo el pie: también se recorta el marco.
        let r = rows(&[
            "✦ Revisé las pruebas",
            "╭──────────────────╮",
            "│ > borrador       │",
            "╰──────────────────╯",
            "~/proj (main*)  gemini-2.5-pro",
            "",
        ]);
        assert_eq!(preview_end(&r, 5, Some(Provider::Gemini)), Some(1));
        assert_eq!(
            preview_end(&r, 5, None),
            Some(1),
            "agentes sin adaptador, igual"
        );
        assert_eq!(
            preview_end(&rows(&["historial", "────────", "resultado"]), 2, None),
            None
        );
    }
}
