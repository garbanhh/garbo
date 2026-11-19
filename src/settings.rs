//! Panel de Ajustes: cambios en vivo que se guardan solos en config.toml
//! (respetando los comentarios y el resto del archivo).

use crate::config::{Config, CursorStyle, Rgb, Theme};
use crate::ui::{cross, dim, surface};
use crate::view::Painter;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Theme,
    FontFamily,
    FontSize,
    LineHeight,
    Padding,
    Opacity,
    CursorStyle,
    CursorBlink,
    CopyOnSelect,
    Scrollback,
    Blocks,
    ShellIntegration,
    Decorations,
    EditFile,
    CheckUpdates,
    Version,
}

/// Qué tiene que hacer la app tras un cambio.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change {
    None,
    Theme,
    Fonts,
    FontSize,
    LineHeight,
    Layout,
    TermOptions,
    Redraw,
    Restart,
    OpenFile,
    Updates,
}

const SECTIONS: &[(&str, &[(Field, &str)])] = &[
    (
        "APARIENCIA",
        &[
            (Field::Theme, "Tema"),
            (Field::FontFamily, "Fuente"),
            (Field::FontSize, "Tamaño de fuente"),
            (Field::LineHeight, "Interlineado"),
            (Field::Padding, "Margen interior"),
            (Field::Opacity, "Opacidad"),
        ],
    ),
    (
        "TERMINAL",
        &[
            (Field::CursorStyle, "Cursor"),
            (Field::CursorBlink, "Cursor parpadeante"),
            (Field::CopyOnSelect, "Copiar al seleccionar"),
            (Field::Scrollback, "Historial"),
        ],
    ),
    (
        "BLOQUES",
        &[(Field::Blocks, "Bloques de comandos"), (Field::ShellIntegration, "Integración con bash")],
    ),
    (
        "VENTANA",
        &[(Field::Decorations, "Barra de título del sistema"), (Field::EditFile, "Editar config.toml")],
    ),
    (
        "ACTUALIZACIONES",
        &[(Field::CheckUpdates, "Buscar actualizaciones"), (Field::Version, "Versión instalada")],
    ),
];

enum Item {
    Section(&'static str),
    /// Fila de ajuste (índice en `fields()`).
    Row(usize),
    /// Una fila de la cuadrícula de temas.
    Themes(usize),
}

fn items(cols: usize) -> Vec<Item> {
    let mut v = Vec::new();
    let mut ri = 0;
    for (title, rows) in SECTIONS {
        v.push(Item::Section(title));
        for (f, _) in rows.iter() {
            v.push(Item::Row(ri));
            if *f == Field::Theme {
                v.extend((0..Theme::NAMES.len().div_ceil(cols)).map(Item::Themes));
            }
            ri += 1;
        }
    }
    v
}

fn fields() -> Vec<Field> {
    SECTIONS.iter().flat_map(|(_, rows)| rows.iter().map(|r| r.0)).collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Close,
    Outside,
    Row(usize),
    Dec(usize),
    Inc(usize),
    /// Tarjeta de un tema (índice en `Theme::NAMES`).
    Theme(usize),
    Panel,
}

pub struct Settings {
    pub selected: usize,
    /// Primer elemento visible. El desplazamiento va por elementos enteros: nada queda cortado.
    first: usize,
    /// Rueda acumulada en píxeles (positivo = hacia arriba), pendiente de convertirse en elementos.
    wheel: f32,
    /// La selección cambió con el teclado: hay que mantenerla a la vista.
    follow: bool,
    fonts: Vec<String>,
    pub restart_note: bool,
    pub hover: Option<Hit>,
}

/// Geometría del panel (compartida por el dibujo y la detección de clics).
struct Layout {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    header_h: f32,
    body_y: f32,
    body_h: f32,
    close: (f32, f32, f32),
    /// (índice de fila, y, alto) solo de las filas visibles
    rows: Vec<(usize, f32, f32)>,
    /// (y, título) de las cabeceras de sección visibles
    sections: Vec<(f32, &'static str)>,
    /// (índice del tema, x, y, ancho, alto de la muestra) de las tarjetas visibles
    cards: Vec<(usize, f32, f32, f32, f32)>,
    /// Alto del nombre bajo cada tarjeta.
    name_h: f32,
    control_w: f32,
    /// (inicio, tamaño) de la barra de scroll relativa al cuerpo, si el contenido no cabe.
    scrollbar: Option<(f32, f32)>,
}

impl Settings {
    pub fn new() -> Self {
        Self::with_fonts(crate::font::FontSet::monospace_families())
    }

    fn with_fonts(fonts: Vec<String>) -> Self {
        Self { selected: 0, first: 0, wheel: 0.0, follow: true, fonts, restart_note: false, hover: None }
    }

    fn field(&self, i: usize) -> Field {
        fields()[i]
    }

    pub fn move_sel(&mut self, d: i32) {
        let n = fields().len() as i32;
        self.selected = (self.selected as i32 + d).clamp(0, n - 1) as usize;
        self.follow = true;
    }

    /// Rueda o touchpad: desplaza el contenido (no la selección). `px` > 0 = hacia arriba.
    pub fn wheel(&mut self, px: f32) {
        self.wheel += px;
        self.follow = false;
    }

    /// Elige un tema de la cuadrícula.
    pub fn pick_theme(&mut self, i: usize, cfg: &mut Config) -> Change {
        let Some(name) = Theme::NAMES.get(i) else { return Change::None };
        self.selected = fields().iter().position(|f| *f == Field::Theme).unwrap_or(0);
        if cfg.theme.eq_ignore_ascii_case(name) {
            return Change::None;
        }
        cfg.theme = name.to_string();
        Change::Theme
    }

    pub fn value_text(&self, f: Field, cfg: &Config) -> String {
        match f {
            Field::Theme => Theme::display_name(&cfg.theme),
            Field::FontFamily => cfg.font.family.clone(),
            Field::FontSize => format!("{} pt", trim_num(cfg.font.size)),
            Field::LineHeight => trim_num(cfg.font.line_height),
            Field::Padding => format!("{} px", cfg.window.padding.round()),
            Field::Opacity => format!("{} %", (cfg.window.opacity * 100.0).round()),
            Field::CursorStyle => match cfg.terminal.cursor_style {
                CursorStyle::Block => "Bloque".into(),
                CursorStyle::Beam => "Barra".into(),
                CursorStyle::Underline => "Subrayado".into(),
            },
            Field::Scrollback => format!("{} líneas", cfg.terminal.scrollback),
            Field::EditFile => "Abrir".into(),
            Field::Version => env!("CARGO_PKG_VERSION").into(),
            _ => String::new(),
        }
    }

    fn toggle_value(f: Field, cfg: &Config) -> Option<bool> {
        Some(match f {
            Field::CursorBlink => cfg.terminal.cursor_blink,
            Field::CopyOnSelect => cfg.terminal.copy_on_select,
            Field::Blocks => cfg.terminal.blocks,
            Field::ShellIntegration => cfg.terminal.shell_integration,
            Field::Decorations => cfg.window.decorations,
            Field::CheckUpdates => cfg.check_updates,
            _ => return None,
        })
    }

    /// Cambia el valor de la fila `i` en `dir` pasos (-1/+1; 0 = activar).
    pub fn adjust(&mut self, i: usize, dir: i32, cfg: &mut Config) -> Change {
        let f = self.field(i);
        let d = if dir == 0 { 1 } else { dir };
        let cycle = |list: &[String], cur: &str| -> String {
            let n = list.len() as i32;
            let pos = list.iter().position(|x| x.eq_ignore_ascii_case(cur)).map_or(0, |p| p as i32);
            list[(pos + d).rem_euclid(n.max(1)) as usize].clone()
        };
        match f {
            Field::Theme => {
                let names: Vec<String> = Theme::NAMES.iter().map(|s| s.to_string()).collect();
                cfg.theme = cycle(&names, &cfg.theme);
                Change::Theme
            }
            Field::FontFamily => {
                if self.fonts.is_empty() {
                    return Change::None;
                }
                cfg.font.family = cycle(&self.fonts, &cfg.font.family);
                Change::Fonts
            }
            Field::FontSize => {
                cfg.font.size = (cfg.font.size + 0.5 * d as f32).clamp(6.0, 32.0);
                Change::FontSize
            }
            Field::LineHeight => {
                cfg.font.line_height = ((cfg.font.line_height + 0.05 * d as f32) * 100.0).round() / 100.0;
                cfg.font.line_height = cfg.font.line_height.clamp(0.8, 2.0);
                Change::LineHeight
            }
            Field::Padding => {
                cfg.window.padding = (cfg.window.padding + 2.0 * d as f32).clamp(0.0, 40.0);
                Change::Layout
            }
            Field::Opacity => {
                cfg.window.opacity = ((cfg.window.opacity + 0.05 * d as f32) * 100.0).round() / 100.0;
                cfg.window.opacity = cfg.window.opacity.clamp(0.3, 1.0);
                Change::Redraw
            }
            Field::CursorStyle => {
                let order = [CursorStyle::Block, CursorStyle::Beam, CursorStyle::Underline];
                let pos = order.iter().position(|c| *c == cfg.terminal.cursor_style).unwrap_or(0) as i32;
                cfg.terminal.cursor_style = order[(pos + d).rem_euclid(3) as usize];
                Change::TermOptions
            }
            Field::Scrollback => {
                let v = cfg.terminal.scrollback as i64 + 1000 * d as i64;
                cfg.terminal.scrollback = v.clamp(1000, 100_000) as usize;
                Change::TermOptions
            }
            Field::CursorBlink => {
                cfg.terminal.cursor_blink = !cfg.terminal.cursor_blink;
                Change::TermOptions
            }
            Field::CopyOnSelect => {
                cfg.terminal.copy_on_select = !cfg.terminal.copy_on_select;
                Change::Redraw
            }
            Field::Blocks => {
                cfg.terminal.blocks = !cfg.terminal.blocks;
                Change::Redraw
            }
            Field::ShellIntegration => {
                cfg.terminal.shell_integration = !cfg.terminal.shell_integration;
                Change::Redraw
            }
            Field::Decorations => {
                cfg.window.decorations = !cfg.window.decorations;
                self.restart_note = true;
                Change::Restart
            }
            Field::EditFile => Change::OpenFile,
            Field::CheckUpdates => {
                cfg.check_updates = !cfg.check_updates;
                Change::Updates
            }
            Field::Version => Change::None,
        }
    }

    fn layout(&mut self, width: f32, height: f32, top: f32, s: f32) -> Layout {
        use Item::*;
        let w = (560.0 * s).min(width - 32.0 * s).max(260.0 * s).round();
        let header_h = (56.0 * s).round();
        let footer_h = (40.0 * s).round();
        let row_h = (38.0 * s).round();
        let sec_h = (34.0 * s).round();
        let pad = (16.0 * s).round();
        let gap = (10.0 * s).round();
        let inner = w - 2.0 * pad;
        let cols = ((inner + gap) / (118.0 * s + gap)).floor().clamp(2.0, 4.0) as usize;
        let card_w = ((inner - gap * (cols as f32 - 1.0)) / cols as f32).floor();
        let card_h = (card_w * 0.52).max(56.0 * s).round();
        let name_h = (24.0 * s).round();
        let card_row_h = card_h + name_h + (6.0 * s).round();

        let its = items(cols);
        let heights: Vec<f32> = its
            .iter()
            .map(|it| match it {
                Section(_) => sec_h,
                Row(_) => row_h,
                Themes(_) => card_row_h,
            })
            .collect();
        let content: f32 = heights.iter().sum::<f32>() + 8.0 * s;
        let max_h = (height - top - 48.0 * s).max(200.0 * s);
        let h = (header_h + content + footer_h).min(max_h).round();
        let x = ((width - w) / 2.0).round();
        let y = (top + ((height - top - h) / 2.0).max(16.0 * s)).round();
        let body_y = y + header_h;
        let body_h = h - header_h - footer_h;

        // Último primer elemento posible: el que deja ver el final del contenido.
        let mut max_first = its.len();
        let mut tail = 0.0;
        while max_first > 0 && tail + heights[max_first - 1] <= body_h {
            max_first -= 1;
            tail += heights[max_first];
        }
        let mut first = self.first.min(max_first);

        // Rueda: se avanza un elemento cuando se ha recorrido más de la mitad de su alto, y se
        // guarda el resto exacto (nunca supera media fila, así que no rebota hacia atrás).
        while first > 0 && self.wheel > heights[first - 1] * 0.5 {
            self.wheel -= heights[first - 1];
            first -= 1;
        }
        while first < max_first && -self.wheel > heights[first] * 0.5 {
            self.wheel += heights[first];
            first += 1;
        }
        if (first == 0 && self.wheel > 0.0) || (first == max_first && self.wheel < 0.0) {
            self.wheel = 0.0;
        }

        // Teclado: la fila elegida (con su cabecera y, si es el tema, su cuadrícula) a la vista.
        if self.follow {
            self.follow = false;
            if let Some(sel) = its.iter().position(|it| matches!(it, Row(r) if *r == self.selected)) {
                let top_i = if sel > 0 && matches!(its[sel - 1], Section(_)) { sel - 1 } else { sel };
                let mut bot_i = sel;
                while bot_i + 1 < its.len() && matches!(its[bot_i + 1], Themes(_)) {
                    bot_i += 1;
                }
                first = first.min(top_i);
                while first < max_first.min(top_i) && heights[first..=bot_i].iter().sum::<f32>() > body_h {
                    first += 1;
                }
            }
        }
        self.first = first;

        let mut rows = Vec::new();
        let mut sections = Vec::new();
        let mut cards = Vec::new();
        let mut cy = body_y;
        for (k, it) in its.iter().enumerate().skip(first) {
            let ih = heights[k];
            if cy + ih > body_y + body_h + 0.5 {
                break;
            }
            match *it {
                Section(t) => sections.push((cy, t)),
                Row(ri) => rows.push((ri, cy, ih)),
                Themes(r) => {
                    for c in 0..cols {
                        let ti = r * cols + c;
                        if ti >= Theme::NAMES.len() {
                            break;
                        }
                        cards.push((ti, x + pad + c as f32 * (card_w + gap), cy + 2.0 * s, card_w, card_h));
                    }
                }
            }
            cy += ih;
        }
        let close = (x + w - 30.0 * s, y + header_h / 2.0, 14.0 * s);
        let max_offset: f32 = heights[..max_first].iter().sum();
        let scrollbar = (max_offset > 0.0).then(|| {
            let total = content.max(body_h + 1.0);
            let size = (body_h * body_h / total).max(24.0 * s);
            let offset: f32 = heights[..first].iter().sum();
            ((offset / max_offset).min(1.0) * (body_h - size), size)
        });
        Layout {
            x,
            y,
            w,
            h,
            header_h,
            body_y,
            body_h,
            close,
            rows,
            sections,
            cards,
            name_h,
            control_w: (190.0 * s).round(),
            scrollbar,
        }
    }

    pub fn hit(&mut self, mx: f32, my: f32, width: f32, height: f32, top: f32, s: f32) -> Hit {
        let l = self.layout(width, height, top, s);
        if mx < l.x || mx > l.x + l.w || my < l.y || my > l.y + l.h {
            return Hit::Outside;
        }
        let (cx, cy, r) = l.close;
        if (mx - cx).hypot(my - cy) <= r {
            return Hit::Close;
        }
        for &(ti, tx, ty, tw, th) in &l.cards {
            if mx >= tx && mx < tx + tw && my >= ty && my < ty + th + l.name_h {
                return Hit::Theme(ti);
            }
        }
        let pad = (16.0 * s).round();
        for &(i, ry, rh) in &l.rows {
            if my >= ry && my < ry + rh {
                let right = l.x + l.w - pad;
                let left = right - l.control_w;
                let f = self.field(i);
                if !is_toggle(f) && !matches!(f, Field::EditFile | Field::Version | Field::Theme) && mx >= left {
                    return if mx < left + l.control_w / 2.0 { Hit::Dec(i) } else { Hit::Inc(i) };
                }
                return Hit::Row(i);
            }
        }
        Hit::Panel
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, p: &mut Painter, theme: &Theme, cfg: &Config, width: f32, height: f32, top: f32, s: f32) {
        let l = self.layout(width, height, top, s);
        p.rect(0.0, 0.0, width, height, Rgb(0, 0, 0), 0.35);
        let panel = surface(theme, 0.05);
        p.card(l.x, l.y, l.w, l.h, panel, theme.foreground, (14.0 * s).round(), s);
        let dimc = dim(theme);
        let pad = (16.0 * s).round();
        let hair = s.round().max(1.0);

        // Cabecera
        p.ui_text_in(l.x + pad + 4.0 * s, l.y, l.header_h, "Ajustes", theme.foreground, l.w / 2.0);
        let (cx, cy, r) = l.close;
        if self.hover == Some(Hit::Close) {
            p.circle(cx, cy, r, theme.foreground, 0.12);
        }
        cross(p, cx, cy, 4.0 * s, 1.5 * s, theme.foreground);
        p.rect(l.x, l.y + l.header_h - hair, l.w, hair, theme.foreground, 0.08);

        for &(sy, title) in &l.sections {
            p.ui_text(l.x + pad + 4.0 * s, sy + (14.0 * s).round(), title, theme.accent, l.w);
        }

        for &(ti, tx, ty, tw, th) in &l.cards {
            let name = Theme::NAMES[ti];
            let selected = cfg.theme.eq_ignore_ascii_case(name);
            let hovered = self.hover == Some(Hit::Theme(ti));
            if let Some(t) = Theme::builtin(name) {
                theme_card(p, &t, theme, tx, ty, tw, th, selected, hovered, s);
            }
            let label = Theme::display_name(name);
            let c = if selected || hovered { theme.foreground } else { dimc };
            let nh = p.ui_line_h(crate::atlas::UiVariant::Small);
            let ny = (ty + th + (l.name_h - nh) / 2.0).round();
            p.ui_text_v(tx + 2.0 * s, ny, &label, c, tw - 4.0 * s, crate::atlas::UiVariant::Small);
        }

        for &(i, ry, rh) in &l.rows {
            let f = self.field(i);
            let label = SECTIONS.iter().flat_map(|(_, r)| r.iter()).find(|r| r.0 == f).map_or("", |r| r.1);
            let hovered = matches!(self.hover, Some(Hit::Row(j) | Hit::Dec(j) | Hit::Inc(j)) if j == i);
            if i == self.selected || hovered {
                let a = if i == self.selected { 0.14 } else { 0.07 };
                p.rounded(l.x + pad / 2.0, ry + 2.0 * s, l.w - pad, rh - 4.0 * s, theme.accent, a, (8.0 * s).round());
            }
            let mut lw = p.ui_text_in(l.x + pad + 4.0 * s, ry, rh, label, theme.foreground, l.w * 0.45);
            if f == Field::Decorations && self.restart_note {
                lw += p.ui_text_in(l.x + pad + 4.0 * s + lw, ry, rh, "  · al reiniciar", dimc, l.w * 0.3);
            }
            let _ = lw;

            let right = l.x + l.w - pad - 4.0 * s;
            let mid = ry + rh / 2.0;
            if let Some(on) = Self::toggle_value(f, cfg) {
                // Interruptor
                let tw = (38.0 * s).round();
                let th = (22.0 * s).round();
                let tx = right - tw;
                let ty = (mid - th / 2.0).round();
                if on {
                    p.rounded(tx, ty, tw, th, theme.accent, 1.0, th / 2.0);
                } else {
                    p.rounded(tx, ty, tw, th, theme.foreground, 0.18, th / 2.0);
                }
                let kr = th / 2.0 - 3.0 * s;
                let kx = if on { tx + tw - th / 2.0 } else { tx + th / 2.0 };
                p.circle(kx, mid, kr, Rgb(255, 255, 255), 0.95);
            } else if matches!(f, Field::Version | Field::Theme) {
                // Solo lectura (el tema se elige en la cuadrícula de debajo).
                let t = self.value_text(f, cfg);
                let tw = p.ui_measure(&t);
                p.ui_text_in(right - tw, ry, rh, &t, dimc, tw + 2.0);
            } else if f == Field::EditFile {
                let t = "Abrir";
                let tw = p.ui_measure(t);
                p.ui_text_in(right - tw - 12.0 * s, ry, rh, t, theme.accent, tw + 2.0);
                let ax = right - 4.0 * s;
                p.line(ax - 3.0 * s, mid - 4.0 * s, ax + 1.0 * s, mid, 1.5 * s, theme.accent, 1.0);
                p.line(ax + 1.0 * s, mid, ax - 3.0 * s, mid + 4.0 * s, 1.5 * s, theme.accent, 1.0);
            } else {
                // Selector ‹ valor ›
                let left = right - l.control_w;
                let ph = (28.0 * s).round();
                let py = (mid - ph / 2.0).round();
                p.rounded(left, py, l.control_w, ph, theme.foreground, 0.06, (7.0 * s).round());
                for (dir, hx) in [(-1.0f32, left + 14.0 * s), (1.0, right - 14.0 * s)] {
                    let over = matches!((self.hover, dir < 0.0), (Some(Hit::Dec(j)), true) | (Some(Hit::Inc(j)), false) if j == i);
                    let c = if over { theme.foreground } else { dimc };
                    let k = 3.5 * s;
                    p.line(hx + dir * -k * 0.5, mid - k, hx + dir * k * 0.5, mid, 1.6 * s, c, 1.0);
                    p.line(hx + dir * k * 0.5, mid, hx + dir * -k * 0.5, mid + k, 1.6 * s, c, 1.0);
                }
                let value = self.value_text(f, cfg);
                let inner = l.control_w - 56.0 * s;
                let vw = p.ui_measure(&value).min(inner);
                let vx = left + (l.control_w - vw) / 2.0;
                p.ui_text_in(vx.round(), ry, rh, &value, theme.foreground, inner + 1.0);
            }
        }

        if let Some((off, size)) = l.scrollbar {
            let bw = (4.0 * s).round();
            p.rounded(l.x + l.w - bw - 4.0 * s, l.body_y + off + 2.0 * s, bw, size - 4.0 * s, theme.foreground, 0.22, bw / 2.0);
        }

        // Pie
        let fy = l.body_y + l.body_h;
        p.rect(l.x, fy, l.w, hair, theme.foreground, 0.08);
        let path = Config::path().map(|p| p.display().to_string()).unwrap_or_default();
        let home = std::env::var("HOME").unwrap_or_default();
        let short = if !home.is_empty() { path.replacen(&home, "~", 1) } else { path };
        p.ui_text_in(
            l.x + pad + 4.0 * s,
            fy,
            l.y + l.h - fy,
            &format!("Se guarda solo en {short}"),
            dimc,
            l.w - 2.0 * pad,
        );
    }
}

/// Muestra de un tema: un terminal en miniatura con sus colores.
#[allow(clippy::too_many_arguments)]
fn theme_card(p: &mut Painter, t: &Theme, ui: &Theme, x: f32, y: f32, w: f32, h: f32, selected: bool, hovered: bool, s: f32) {
    use crate::atlas::UiVariant::Small;
    let r = (8.0 * s).round();
    let hair = s.round().max(1.0);
    if selected {
        let b = (2.0 * s).round();
        p.rounded(x - b, y - b, w + 2.0 * b, h + 2.0 * b, ui.accent, 1.0, r + b);
    } else {
        let a = if hovered { 0.40 } else { 0.14 };
        p.rounded(x - hair, y - hair, w + 2.0 * hair, h + 2.0 * hair, ui.foreground, a, r + hair);
    }
    p.rounded(x, y, w, h, t.background, 1.0, r);

    let lh = p.ui_line_h(Small);
    let tx = x + (9.0 * s).round();
    let max = w - 18.0 * s;
    let mut ly = y + (7.0 * s).round();
    let pw = p.ui_text_v(tx, ly, "$ ", t.accent, max, Small);
    p.ui_text_v(tx + pw, ly, "ls", t.foreground, max - pw, Small);
    ly += lh;
    // Solo las palabras que caben enteras.
    let mut cx = tx;
    for (word, c) in [("src", t.palette[4]), ("docs", t.palette[4]), ("main.rs", t.foreground)] {
        let ww = p.ui_measure_v(word, Small);
        if cx + ww > tx + max {
            break;
        }
        p.ui_text_v(cx, ly, word, c, ww + 1.0, Small);
        cx += ww + p.ui_measure_v(" ", Small);
    }
    // Paleta del tema
    let sw_h = (5.0 * s).round();
    let sy = y + h - sw_h - (8.0 * s).round();
    let n = 6.0;
    let sgap = (3.0 * s).round();
    let sw = ((max - sgap * (n - 1.0)) / n).floor();
    for k in 0..6 {
        p.rounded(tx + k as f32 * (sw + sgap), sy, sw, sw_h, t.palette[k + 1], 1.0, sw_h / 2.0);
    }
}

fn is_toggle(f: Field) -> bool {
    matches!(
        f,
        Field::CursorBlink | Field::CopyOnSelect | Field::Blocks | Field::ShellIntegration | Field::Decorations | Field::CheckUpdates
    )
}

fn trim_num(v: f32) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.replace('.', ",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guardar_conserva_comentarios_y_relee() {
        let dir = std::env::temp_dir().join(format!("garbo-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("garbo")).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let path = dir.join("garbo/config.toml");
        std::fs::write(&path, "# mi comentario\n[shell]\nprogram = \"/bin/zsh\"\n").unwrap();

        let mut cfg = Config::load();
        let mut set = Settings::with_fonts(vec![]);
        let i_theme = fields().iter().position(|f| *f == Field::Theme).unwrap();
        let i_size = fields().iter().position(|f| *f == Field::FontSize).unwrap();
        let i_blocks = fields().iter().position(|f| *f == Field::Blocks).unwrap();
        assert_eq!(set.adjust(i_theme, 1, &mut cfg), Change::Theme);
        assert_eq!(cfg.theme, Theme::NAMES[1]);
        set.adjust(i_size, 1, &mut cfg);
        set.adjust(i_blocks, 0, &mut cfg);
        cfg.save().unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# mi comentario"), "{text}");
        assert!(text.contains("program = \"/bin/zsh\""), "{text}");
        let back = Config::load();
        assert_eq!(back.theme, Theme::NAMES[1]);
        assert_eq!(back.font.size, 11.5);
        assert!(!back.terminal.blocks);
        assert_eq!(back.shell.program.as_deref(), Some("/bin/zsh"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rueda_desplaza_sin_cambiar_la_seleccion() {
        let mut set = Settings::with_fonts(vec![]);
        let (w, h, top, s) = (900.0, 420.0, 40.0, 1.0);
        set.layout(w, h, top, s);
        assert_eq!(set.first, 0);
        // Muchos eventos pequeños de touchpad: avanzan por elementos enteros, sin saltos.
        for _ in 0..10 {
            set.wheel(-4.0);
            set.layout(w, h, top, s);
        }
        assert_eq!(set.first, 1, "40 px de rueda = un elemento");
        assert_eq!(set.selected, 0, "la rueda no mueve la selección");
        // Hasta el final y de vuelta: siempre dentro de los límites.
        set.wheel(-100_000.0);
        let l = set.layout(w, h, top, s);
        assert!(!l.rows.is_empty());
        let last = fields().len() - 1;
        assert!(l.rows.iter().any(|r| r.0 == last), "se llega a la última fila");
        set.wheel(100_000.0);
        set.layout(w, h, top, s);
        assert_eq!(set.first, 0);
        // El teclado sí vuelve a mostrar la fila elegida.
        set.move_sel(1000);
        let l = set.layout(w, h, top, s);
        assert!(l.rows.iter().any(|r| r.0 == last));
    }

    #[test]
    fn elegir_tema_de_la_cuadricula() {
        let mut cfg = Config::default();
        let mut set = Settings::with_fonts(vec![]);
        set.selected = 3;
        assert_eq!(set.pick_theme(4, &mut cfg), Change::Theme);
        assert_eq!(cfg.theme, Theme::NAMES[4]);
        assert_eq!(set.field(set.selected), Field::Theme);
        assert_eq!(set.pick_theme(4, &mut cfg), Change::None);
        assert_eq!(set.pick_theme(99, &mut cfg), Change::None);
        // Con el ancho normal caben todas las tarjetas en dos filas.
        let l = set.layout(900.0, 1400.0, 40.0, 1.0);
        assert_eq!(l.cards.len(), Theme::NAMES.len());
        assert_eq!(Theme::display_name("catppuccin-mocha"), "Catppuccin Mocha");
    }

    #[test]
    fn limites_de_valores() {
        let mut cfg = Config::default();
        let mut set = Settings::with_fonts(vec![]);
        let i = fields().iter().position(|f| *f == Field::FontSize).unwrap();
        for _ in 0..100 {
            set.adjust(i, -1, &mut cfg);
        }
        assert_eq!(cfg.font.size, 6.0);
        let i = fields().iter().position(|f| *f == Field::Opacity).unwrap();
        for _ in 0..100 {
            set.adjust(i, 1, &mut cfg);
        }
        assert_eq!(cfg.window.opacity, 1.0);
        set.move_sel(-5);
        assert_eq!(set.selected, 0);
        set.move_sel(1000);
        assert_eq!(set.selected, fields().len() - 1);
    }
}
