//! Ajustes: ventana compacta con las categorías a la izquierda y sus opciones a la derecha.
//! Los cambios se aplican en vivo y se guardan solos en config.toml (respetando los comentarios y
//! el resto del archivo).

use crate::atlas::UiVariant;
use crate::config::{Config, CursorStyle, Rgb, Theme};
use crate::ui::{cross, dim, surface};
use crate::update::{CheckState, UpdateState};
use crate::view::Painter;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Theme,
    FontFamily,
    FontSize,
    LineHeight,
    CursorStyle,
    CursorBlink,
    CopyOnSelect,
    Scrollback,
    Autosuggest,
    Blocks,
    ShellIntegration,
    Padding,
    Opacity,
    Decorations,
    EditFile,
    CheckNow,
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
    /// Botón "Buscar" / "Actualizar" de la categoría Actualizaciones.
    CheckNow,
}

/// Categorías (columna izquierda) y sus opciones (derecha).
const CATEGORIES: &[(&str, &[(Field, &str)])] = &[
    ("Tema", &[(Field::Theme, "Tema")]),
    (
        "Texto",
        &[(Field::FontFamily, "Fuente"), (Field::FontSize, "Tamaño"), (Field::LineHeight, "Interlineado")],
    ),
    (
        "Terminal",
        &[
            (Field::CursorStyle, "Cursor"),
            (Field::CursorBlink, "Cursor parpadeante"),
            (Field::CopyOnSelect, "Copiar al seleccionar"),
            (Field::Scrollback, "Historial"),
            (Field::Autosuggest, "Sugerencias con Tab"),
        ],
    ),
    (
        "Bloques",
        &[(Field::Blocks, "Bloques de comandos"), (Field::ShellIntegration, "Integración con bash")],
    ),
    (
        "Ventana",
        &[
            (Field::Padding, "Margen interior"),
            (Field::Opacity, "Opacidad"),
            (Field::Decorations, "Barra de título del sistema"),
            (Field::EditFile, "Editar config.toml"),
        ],
    ),
    (
        "Actualizaciones",
        &[
            (Field::CheckNow, "Buscar actualizaciones"),
            (Field::CheckUpdates, "Buscar automáticamente"),
            (Field::Version, "Versión instalada"),
        ],
    ),
];

fn fields() -> Vec<Field> {
    CATEGORIES.iter().flat_map(|(_, rows)| rows.iter().map(|r| r.0)).collect()
}

fn label_of(f: Field) -> &'static str {
    CATEGORIES.iter().flat_map(|(_, r)| r.iter()).find(|r| r.0 == f).map_or("", |r| r.1)
}

/// Rango [inicio, fin) de los índices de `fields()` de una categoría.
fn cat_range(cat: usize) -> (usize, usize) {
    let start: usize = CATEGORIES[..cat].iter().map(|(_, r)| r.len()).sum();
    (start, start + CATEGORIES[cat].1.len())
}

fn cat_of(field_idx: usize) -> usize {
    (0..CATEGORIES.len()).find(|&c| field_idx < cat_range(c).1).unwrap_or(0)
}

enum Item {
    /// Fila de ajuste (índice en `fields()`).
    Row(usize),
    /// Una fila de la cuadrícula de temas.
    Themes(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Close,
    Outside,
    Category(usize),
    Row(usize),
    Dec(usize),
    Inc(usize),
    /// Tarjeta de un tema (índice en `Theme::NAMES`).
    Theme(usize),
    Panel,
}

/// Teclas de navegación.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Activate,
    NextCategory,
    PrevCategory,
}

/// Lo que la categoría Actualizaciones necesita saber para su botón.
pub struct UpdateView<'a> {
    pub check: &'a CheckState,
    pub state: Option<&'a UpdateState>,
}

pub struct Settings {
    /// Categoría visible.
    pub cat: usize,
    /// Opción elegida (índice en `fields()`, siempre dentro de la categoría visible).
    pub selected: usize,
    /// Primer elemento visible. El desplazamiento va por elementos enteros: nada queda cortado.
    first: usize,
    /// Rueda acumulada en píxeles (positivo = hacia arriba), pendiente de convertirse en elementos.
    wheel: f32,
    /// La selección cambió con el teclado: hay que mantenerla a la vista.
    follow: bool,
    /// Columnas de la cuadrícula de temas en el último dibujado (para ↑ ↓).
    cols: usize,
    /// Índice del tema actual (para mantener su fila de tarjetas a la vista).
    theme_idx: usize,
    fonts: Vec<String>,
    pub restart_note: bool,
    pub hover: Option<Hit>,
}

/// Geometría de la ventana (compartida por el dibujo y la detección de clics).
struct Layout {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    side_w: f32,
    pane_x: f32,
    pane_w: f32,
    header_h: f32,
    body_y: f32,
    body_h: f32,
    close: (f32, f32, f32),
    /// (categoría, y, alto)
    cats: Vec<(usize, f32, f32)>,
    /// (índice de fila, y, alto) solo de las filas visibles
    rows: Vec<(usize, f32, f32)>,
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
        Self {
            cat: 0,
            selected: 0,
            first: 0,
            wheel: 0.0,
            follow: true,
            cols: 3,
            theme_idx: 0,
            fonts,
            restart_note: false,
            hover: None,
        }
    }

    fn field(&self, i: usize) -> Field {
        fields()[i]
    }

    /// Muestra otra categoría (con su primera opción elegida).
    pub fn set_cat(&mut self, cat: usize) {
        let cat = cat.min(CATEGORIES.len() - 1);
        if cat != self.cat {
            self.cat = cat;
            self.selected = cat_range(cat).0;
            self.first = 0;
            self.wheel = 0.0;
        }
        self.follow = true;
    }

    /// Mueve la selección dentro de la categoría visible.
    pub fn move_sel(&mut self, d: i32) {
        let (a, b) = cat_range(self.cat);
        self.selected = (self.selected as i32 + d).clamp(a as i32, b as i32 - 1) as usize;
        self.follow = true;
    }

    /// Rueda o touchpad: desplaza el contenido (no la selección). `px` > 0 = hacia arriba.
    pub fn wheel(&mut self, px: f32) {
        self.wheel += px;
        self.follow = false;
    }

    /// Teclado.
    pub fn nav(&mut self, n: Nav, cfg: &mut Config) -> Change {
        let n_cats = CATEGORIES.len();
        let on_theme = self.field(self.selected) == Field::Theme;
        match n {
            Nav::NextCategory => self.set_cat((self.cat + 1) % n_cats),
            Nav::PrevCategory => self.set_cat((self.cat + n_cats - 1) % n_cats),
            // En la cuadrícula de temas, ↑ ↓ cambian de fila de tarjetas.
            Nav::Up | Nav::Down if on_theme => {
                let cur = Theme::NAMES.iter().position(|t| t.eq_ignore_ascii_case(&cfg.theme)).unwrap_or(0) as i32;
                let step = if n == Nav::Up { -(self.cols as i32) } else { self.cols as i32 };
                let next = cur + step;
                if (0..Theme::NAMES.len() as i32).contains(&next) {
                    self.follow = true;
                    return self.pick_theme(next as usize, cfg);
                }
            }
            Nav::Up => self.move_sel(-1),
            Nav::Down => self.move_sel(1),
            Nav::Left => return self.adjust(self.selected, -1, cfg),
            Nav::Right => return self.adjust(self.selected, 1, cfg),
            Nav::Activate => return self.adjust(self.selected, 0, cfg),
        }
        Change::None
    }

    /// Elige un tema de la cuadrícula.
    pub fn pick_theme(&mut self, i: usize, cfg: &mut Config) -> Change {
        let Some(name) = Theme::NAMES.get(i) else { return Change::None };
        self.theme_idx = i;
        self.selected = fields().iter().position(|f| *f == Field::Theme).unwrap_or(0);
        self.cat = cat_of(self.selected);
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
            Field::Autosuggest => cfg.terminal.autosuggest,
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
                self.follow = true;
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
            Field::Autosuggest => {
                cfg.terminal.autosuggest = !cfg.terminal.autosuggest;
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
            // El botón solo responde a Intro o clic, no a ← →.
            Field::CheckNow if dir == 0 => Change::CheckNow,
            Field::CheckNow => Change::None,
            Field::CheckUpdates => {
                cfg.check_updates = !cfg.check_updates;
                Change::Updates
            }
            Field::Version => Change::None,
        }
    }

    fn layout(&mut self, width: f32, height: f32, top: f32, s: f32) -> Layout {
        let w = (640.0 * s).min(width - 32.0 * s).max(300.0 * s).round();
        let h = (540.0 * s).min(height - top - 32.0 * s).max(240.0 * s).round();
        let x = ((width - w) / 2.0).round();
        let y = (top + ((height - top - h) / 2.0).max(16.0 * s)).round();
        let side_w = (176.0 * s).min(w * 0.36).round();
        let pane_x = x + side_w;
        let pane_w = w - side_w;
        let header_h = (60.0 * s).round();
        let footer_h = (36.0 * s).round();
        let body_y = y + header_h;
        let body_h = h - header_h - footer_h;
        let pad = (20.0 * s).round();

        let cat_h = (36.0 * s).round();
        let cats = (0..CATEGORIES.len()).map(|c| (c, y + (64.0 * s).round() + c as f32 * cat_h, cat_h)).collect();

        // Elementos de la categoría visible.
        let row_h = (44.0 * s).round();
        let gap = (12.0 * s).round();
        let inner = pane_w - 2.0 * pad;
        let cols = ((inner + gap) / (118.0 * s + gap)).floor().clamp(2.0, 4.0) as usize;
        self.cols = cols;
        let card_w = ((inner - gap * (cols as f32 - 1.0)) / cols as f32).floor();
        let card_h = (card_w * 0.52).max(56.0 * s).round();
        let name_h = (26.0 * s).round();
        let card_row_h = card_h + name_h + (8.0 * s).round();
        let (a, b) = cat_range(self.cat);
        let its: Vec<Item> = if self.field(a) == Field::Theme {
            (0..Theme::NAMES.len().div_ceil(cols)).map(Item::Themes).collect()
        } else {
            (a..b).map(Item::Row).collect()
        };
        let heights: Vec<f32> =
            its.iter().map(|it| if matches!(it, Item::Themes(_)) { card_row_h } else { row_h }).collect();

        // Último primer elemento posible: el que deja ver el final del contenido.
        let mut max_first = its.len();
        let mut tail = 0.0;
        while max_first > 0 && tail + heights[max_first - 1] <= body_h {
            max_first -= 1;
            tail += heights[max_first];
        }
        let mut first = self.first.min(max_first);

        // Rueda: se avanza un elemento al recorrer más de la mitad de su alto, guardando el resto.
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

        // Teclado: lo elegido, a la vista (en Tema, la fila de tarjetas del tema actual).
        if self.follow {
            self.follow = false;
            let sel = match its.first() {
                Some(Item::Themes(_)) => self.theme_row(cols),
                _ => self.selected.saturating_sub(a),
            }
            .min(its.len().saturating_sub(1));
            first = first.min(sel);
            while first < max_first.min(sel) && heights[first..=sel].iter().sum::<f32>() > body_h {
                first += 1;
            }
        }
        self.first = first;

        let mut rows = Vec::new();
        let mut cards = Vec::new();
        let mut cy = body_y;
        for (k, it) in its.iter().enumerate().skip(first) {
            let ih = heights[k];
            if cy + ih > body_y + body_h + 0.5 {
                break;
            }
            match *it {
                Item::Row(ri) => rows.push((ri, cy, ih)),
                Item::Themes(r) => {
                    for c in 0..cols {
                        let ti = r * cols + c;
                        if ti >= Theme::NAMES.len() {
                            break;
                        }
                        cards.push((ti, pane_x + pad + c as f32 * (card_w + gap), cy + 4.0 * s, card_w, card_h));
                    }
                }
            }
            cy += ih;
        }
        let close = (x + w - 30.0 * s, y + header_h / 2.0, 14.0 * s);
        let max_offset: f32 = heights[..max_first].iter().sum();
        let scrollbar = (max_offset > 0.0).then(|| {
            let total: f32 = heights.iter().sum::<f32>().max(body_h + 1.0);
            let size = (body_h * body_h / total).max(24.0 * s);
            let offset: f32 = heights[..first].iter().sum();
            ((offset / max_offset).min(1.0) * (body_h - size), size)
        });
        Layout {
            x,
            y,
            w,
            h,
            side_w,
            pane_x,
            pane_w,
            header_h,
            body_y,
            body_h,
            close,
            cats,
            rows,
            cards,
            name_h,
            control_w: (170.0 * s).min(pane_w * 0.45).round(),
            scrollbar,
        }
    }

    /// Fila de la cuadrícula en la que está el tema actual.
    fn theme_row(&self, cols: usize) -> usize {
        self.theme_idx / cols.max(1)
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
        if mx < l.pane_x {
            for &(c, cy, ch) in &l.cats {
                if my >= cy && my < cy + ch {
                    return Hit::Category(c);
                }
            }
            return Hit::Panel;
        }
        for &(ti, tx, ty, tw, th) in &l.cards {
            if mx >= tx && mx < tx + tw && my >= ty && my < ty + th + l.name_h {
                return Hit::Theme(ti);
            }
        }
        let pad = (20.0 * s).round();
        for &(i, ry, rh) in &l.rows {
            if my >= ry && my < ry + rh {
                let right = l.pane_x + l.pane_w - pad;
                let left = right - l.control_w;
                let f = self.field(i);
                if has_stepper(f) && mx >= left {
                    return if mx < left + l.control_w / 2.0 { Hit::Dec(i) } else { Hit::Inc(i) };
                }
                return Hit::Row(i);
            }
        }
        Hit::Panel
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        p: &mut Painter,
        theme: &Theme,
        cfg: &Config,
        upd: &UpdateView,
        width: f32,
        height: f32,
        top: f32,
        s: f32,
    ) {
        self.theme_idx = Theme::NAMES.iter().position(|n| n.eq_ignore_ascii_case(&cfg.theme)).unwrap_or(0);
        let l = self.layout(width, height, top, s);
        p.rect(0.0, 0.0, width, height, Rgb(0, 0, 0), 0.40);
        let radius = (14.0 * s).round();
        let panel = surface(theme, 0.05);
        p.card(l.x, l.y, l.w, l.h, panel, theme.foreground, radius, s);
        let dimc = dim(theme);
        let pad = (20.0 * s).round();
        let hair = s.round().max(1.0);

        // --- Columna de categorías
        let side = surface(theme, 0.025);
        p.rounded(l.x + hair, l.y + hair, l.side_w, l.h - 2.0 * hair, side, 1.0, radius - hair);
        p.rect(l.x + l.side_w - radius, l.y + hair, radius, l.h - 2.0 * hair, side, 1.0);
        p.rect(l.x + l.side_w, l.y + hair, hair, l.h - 2.0 * hair, theme.foreground, 0.08);
        let th = p.ui_line_h(UiVariant::Title);
        p.ui_text_v(l.x + pad, l.y + ((60.0 * s - th) / 2.0).round(), "Ajustes", theme.foreground, l.side_w - 2.0 * pad, UiVariant::Title);
        for &(c, cy, ch) in &l.cats {
            let selected = c == self.cat;
            let hovered = self.hover == Some(Hit::Category(c));
            let (bx, bw) = (l.x + 10.0 * s, l.side_w - 20.0 * s);
            if selected {
                p.rounded(bx, cy + 2.0 * s, bw, ch - 4.0 * s, theme.accent, 0.18, (8.0 * s).round());
            } else if hovered {
                p.rounded(bx, cy + 2.0 * s, bw, ch - 4.0 * s, theme.foreground, 0.06, (8.0 * s).round());
            }
            let c_txt = if selected || hovered { theme.foreground } else { dimc };
            p.ui_text_in(bx + 12.0 * s, cy, ch, CATEGORIES[c].0, c_txt, bw - 16.0 * s);
        }

        // --- Cabecera de la derecha
        let title = CATEGORIES[self.cat].0;
        p.ui_text_v(l.pane_x + pad, l.y + ((l.header_h - th) / 2.0).round(), title, theme.foreground, l.pane_w / 2.0, UiVariant::Title);
        let (cx, cy, r) = l.close;
        if self.hover == Some(Hit::Close) {
            p.circle(cx, cy, r, theme.foreground, 0.12);
        }
        cross(p, cx, cy, 4.0 * s, 1.5 * s, theme.foreground);

        // --- Tarjetas de temas
        for &(ti, tx, ty, tw, tch) in &l.cards {
            let name = Theme::NAMES[ti];
            let selected = cfg.theme.eq_ignore_ascii_case(name);
            let hovered = self.hover == Some(Hit::Theme(ti));
            if let Some(t) = Theme::builtin(name) {
                theme_card(p, &t, theme, tx, ty, tw, tch, selected, hovered, s);
            }
            let label = Theme::display_name(name);
            let c = if selected || hovered { theme.foreground } else { dimc };
            let nh = p.ui_line_h(UiVariant::Small);
            let ny = (ty + tch + (l.name_h - nh) / 2.0).round();
            p.ui_text_v(tx + 2.0 * s, ny, &label, c, tw - 4.0 * s, UiVariant::Small);
        }

        // --- Filas
        for &(i, ry, rh) in &l.rows {
            let f = self.field(i);
            let hovered = matches!(self.hover, Some(Hit::Row(j) | Hit::Dec(j) | Hit::Inc(j)) if j == i);
            let (bx, bw) = (l.pane_x + pad / 2.0, l.pane_w - pad);
            if i == self.selected || hovered {
                let a = if i == self.selected { 0.12 } else { 0.06 };
                p.rounded(bx, ry + 3.0 * s, bw, rh - 6.0 * s, theme.accent, a, (8.0 * s).round());
            }
            let lx = l.pane_x + pad;
            // Nota bajo el nombre (estado de la búsqueda de actualizaciones o aviso de reinicio).
            let note = match f {
                Field::CheckNow => update_button(upd).3,
                Field::Decorations if self.restart_note => "Se aplica al reiniciar".into(),
                _ => String::new(),
            };
            if note.is_empty() {
                p.ui_text_in(lx, ry, rh, label_of(f), theme.foreground, l.pane_w * 0.5);
            } else {
                let lh = p.ui_metrics().height;
                let nh = p.ui_line_h(UiVariant::Small);
                let ty = (ry + (rh - lh - nh) / 2.0).round();
                p.ui_text(lx, ty, label_of(f), theme.foreground, l.pane_w * 0.5);
                let nc = match (f, upd.check, upd.state) {
                    (Field::CheckNow, _, Some(UpdateState::Available(_) | UpdateState::Installed(_))) => theme.accent,
                    (Field::CheckNow, _, Some(UpdateState::Failed(_))) | (Field::CheckNow, CheckState::Failed(_), None) => theme.palette[1],
                    (Field::CheckNow, CheckState::UpToDate, None) => theme.palette[2],
                    _ => dimc,
                };
                p.ui_text_v(lx, ty + lh, &note, nc, l.pane_w * 0.5, UiVariant::Small);
            }

            let right = l.pane_x + l.pane_w - pad;
            let mid = ry + rh / 2.0;
            if let Some(on) = Self::toggle_value(f, cfg) {
                toggle(p, theme, right, mid, on, s);
            } else if f == Field::Version {
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
            } else if f == Field::CheckNow {
                let (label, primary, enabled, _) = update_button(upd);
                let bh = (30.0 * s).round();
                let bpad = (14.0 * s).round();
                let tw = p.ui_measure(&label);
                let bwid = (tw + 2.0 * bpad).round();
                let bx = right - bwid;
                let by = (mid - bh / 2.0).round();
                let over = hovered && enabled;
                let (fill, alpha, txt) = match (primary, enabled) {
                    (true, _) => (theme.accent, if over { 1.0 } else { 0.88 }, theme.background),
                    (false, true) => (theme.foreground, if over { 0.16 } else { 0.09 }, theme.foreground),
                    (false, false) => (theme.foreground, 0.05, dimc),
                };
                p.rounded(bx, by, bwid, bh, fill, alpha, (bh / 2.0).round());
                p.ui_text_in(bx + bpad, by, bh, &label, txt, tw + 1.0);
            } else {
                stepper(p, theme, self, i, f, cfg, right, ry, rh, l.control_w, s);
            }
        }

        if let Some((off, size)) = l.scrollbar {
            let bw = (4.0 * s).round();
            p.rounded(l.x + l.w - bw - 5.0 * s, l.body_y + off + 2.0 * s, bw, size - 4.0 * s, theme.foreground, 0.22, bw / 2.0);
        }

        // Pie
        let fy = l.body_y + l.body_h;
        let path = Config::path().map(|p| p.display().to_string()).unwrap_or_default();
        let home = std::env::var("HOME").unwrap_or_default();
        let short = if !home.is_empty() { path.replacen(&home, "~", 1) } else { path };
        let nh = p.ui_line_h(UiVariant::Small);
        p.ui_text_v(
            l.pane_x + pad,
            (fy + (l.y + l.h - fy - nh) / 2.0).round(),
            &format!("Se guarda solo en {short}"),
            dimc,
            l.pane_w - 2.0 * pad,
            UiVariant::Small,
        );
    }
}

/// Texto del botón de actualizaciones: (etiqueta, principal, activo, nota de estado).
fn update_button(upd: &UpdateView) -> (String, bool, bool, String) {
    match upd.state {
        Some(UpdateState::Available(rel)) => (format!("Actualizar a {}", rel.version), true, true, "Hay una versión nueva".into()),
        Some(UpdateState::Downloading(_)) => ("Descargando…".into(), false, false, String::new()),
        Some(UpdateState::Installing(_)) => ("Instalando…".into(), false, false, String::new()),
        Some(UpdateState::Installed(v)) => ("Reiniciar".into(), true, true, format!("Instalada la {v}")),
        Some(UpdateState::Failed(_)) => ("Reintentar".into(), false, true, "No se pudo actualizar".into()),
        None => match upd.check {
            CheckState::Checking => ("Buscando…".into(), false, false, String::new()),
            CheckState::UpToDate => ("Buscar".into(), false, true, "Estás al día".into()),
            CheckState::Failed(_) => ("Buscar".into(), false, true, "Sin conexión".into()),
            CheckState::Idle => ("Buscar".into(), false, true, String::new()),
        },
    }
}

/// Opciones con selector ‹ valor ›.
fn has_stepper(f: Field) -> bool {
    matches!(
        f,
        Field::FontFamily
            | Field::FontSize
            | Field::LineHeight
            | Field::CursorStyle
            | Field::Scrollback
            | Field::Padding
            | Field::Opacity
    )
}

fn toggle(p: &mut Painter, theme: &Theme, right: f32, mid: f32, on: bool, s: f32) {
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
}

/// Selector ‹ valor ›
#[allow(clippy::too_many_arguments)]
fn stepper(p: &mut Painter, theme: &Theme, set: &Settings, i: usize, f: Field, cfg: &Config, right: f32, ry: f32, rh: f32, cw: f32, s: f32) {
    let dimc = dim(theme);
    let mid = ry + rh / 2.0;
    let left = right - cw;
    let ph = (30.0 * s).round();
    let py = (mid - ph / 2.0).round();
    p.rounded(left, py, cw, ph, theme.foreground, 0.06, (8.0 * s).round());
    for (dir, hx) in [(-1.0f32, left + 14.0 * s), (1.0, right - 14.0 * s)] {
        let over = matches!((set.hover, dir < 0.0), (Some(Hit::Dec(j)), true) | (Some(Hit::Inc(j)), false) if j == i);
        let c = if over { theme.foreground } else { dimc };
        let k = 3.5 * s;
        p.line(hx + dir * -k * 0.5, mid - k, hx + dir * k * 0.5, mid, 1.6 * s, c, 1.0);
        p.line(hx + dir * k * 0.5, mid, hx + dir * -k * 0.5, mid + k, 1.6 * s, c, 1.0);
    }
    let value = set.value_text(f, cfg);
    let inner = cw - 56.0 * s;
    let vw = p.ui_measure(&value).min(inner);
    let vx = left + (cw - vw) / 2.0;
    p.ui_text_in(vx.round(), ry, rh, &value, theme.foreground, inner + 1.0);
}

/// Muestra de un tema: un terminal en miniatura con sus colores.
#[allow(clippy::too_many_arguments)]
fn theme_card(p: &mut Painter, t: &Theme, ui: &Theme, x: f32, y: f32, w: f32, h: f32, selected: bool, hovered: bool, s: f32) {
    use UiVariant::Small;
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

fn trim_num(v: f32) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.replace('.', ",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx(f: Field) -> usize {
        fields().iter().position(|x| *x == f).unwrap()
    }

    #[test]
    fn guardar_conserva_comentarios_y_relee() {
        let dir = std::env::temp_dir().join(format!("garbo-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("garbo")).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let path = dir.join("garbo/config.toml");
        std::fs::write(&path, "# mi comentario\n[shell]\nprogram = \"/bin/zsh\"\n").unwrap();

        let mut cfg = Config::load();
        let mut set = Settings::with_fonts(vec![]);
        assert_eq!(set.adjust(idx(Field::Theme), 1, &mut cfg), Change::Theme);
        assert_eq!(cfg.theme, Theme::NAMES[1]);
        set.adjust(idx(Field::FontSize), 1, &mut cfg);
        set.adjust(idx(Field::Blocks), 0, &mut cfg);
        set.adjust(idx(Field::Autosuggest), 0, &mut cfg);
        cfg.save().unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# mi comentario"), "{text}");
        assert!(text.contains("program = \"/bin/zsh\""), "{text}");
        let back = Config::load();
        assert_eq!(back.theme, Theme::NAMES[1]);
        assert_eq!(back.font.size, 11.5);
        assert!(!back.terminal.blocks);
        assert!(!back.terminal.autosuggest);
        assert_eq!(back.shell.program.as_deref(), Some("/bin/zsh"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn categorias_y_teclado() {
        let mut cfg = Config::default();
        let mut set = Settings::with_fonts(vec![]);
        assert_eq!(CATEGORIES[set.cat].0, "Tema");
        // Tab recorre las categorías y elige su primera opción.
        set.nav(Nav::NextCategory, &mut cfg);
        assert_eq!(set.field(set.selected), Field::FontFamily);
        set.nav(Nav::PrevCategory, &mut cfg);
        set.nav(Nav::PrevCategory, &mut cfg);
        assert_eq!(CATEGORIES[set.cat].0, "Actualizaciones");
        assert_eq!(set.field(set.selected), Field::CheckNow);
        // ↑ ↓ no salen de la categoría.
        for _ in 0..10 {
            set.nav(Nav::Down, &mut cfg);
        }
        assert_eq!(set.field(set.selected), Field::Version);
        // El botón responde a Intro, no a las flechas.
        set.move_sel(-10);
        assert_eq!(set.nav(Nav::Right, &mut cfg), Change::None);
        assert_eq!(set.nav(Nav::Activate, &mut cfg), Change::CheckNow);
        // En Tema, ↓ baja una fila de tarjetas.
        set.set_cat(0);
        set.layout(1200.0, 900.0, 40.0, 1.0);
        let cols = set.cols;
        assert_eq!(set.nav(Nav::Down, &mut cfg), Change::Theme);
        assert_eq!(cfg.theme, Theme::NAMES[cols]);
        assert_eq!(set.nav(Nav::Right, &mut cfg), Change::Theme);
        assert_eq!(cfg.theme, Theme::NAMES[cols + 1]);
    }

    #[test]
    fn rueda_desplaza_sin_cambiar_la_seleccion() {
        let mut set = Settings::with_fonts(vec![]);
        set.set_cat(2); // Terminal: 5 filas
        let (w, h, top, s) = (900.0, 330.0, 40.0, 1.0);
        set.layout(w, h, top, s);
        assert_eq!(set.first, 0);
        let sel = set.selected;
        for _ in 0..12 {
            set.wheel(-4.0);
            set.layout(w, h, top, s);
        }
        assert_eq!(set.first, 1, "48 px de rueda = una fila");
        assert_eq!(set.selected, sel, "la rueda no mueve la selección");
        set.wheel(-100_000.0);
        let l = set.layout(w, h, top, s);
        assert!(l.rows.iter().any(|r| set.field(r.0) == Field::Autosuggest), "se llega a la última fila");
        set.wheel(100_000.0);
        set.layout(w, h, top, s);
        assert_eq!(set.first, 0);
        set.move_sel(10);
        let l = set.layout(w, h, top, s);
        assert!(l.rows.iter().any(|r| r.0 == set.selected), "el teclado muestra la fila elegida");
    }

    #[test]
    fn elegir_tema_de_la_cuadricula() {
        let mut cfg = Config::default();
        let mut set = Settings::with_fonts(vec![]);
        set.set_cat(3);
        assert_eq!(set.pick_theme(4, &mut cfg), Change::Theme);
        assert_eq!(cfg.theme, Theme::NAMES[4]);
        assert_eq!(set.field(set.selected), Field::Theme);
        assert_eq!(set.cat, 0);
        assert_eq!(set.pick_theme(4, &mut cfg), Change::None);
        assert_eq!(set.pick_theme(99, &mut cfg), Change::None);
        let l = set.layout(1200.0, 1400.0, 40.0, 1.0);
        assert_eq!(l.cards.len(), Theme::NAMES.len());
        assert_eq!(Theme::display_name("catppuccin-mocha"), "Catppuccin Mocha");
    }

    #[test]
    fn limites_de_valores() {
        let mut cfg = Config::default();
        let mut set = Settings::with_fonts(vec![]);
        for _ in 0..100 {
            set.adjust(idx(Field::FontSize), -1, &mut cfg);
        }
        assert_eq!(cfg.font.size, 6.0);
        for _ in 0..100 {
            set.adjust(idx(Field::Opacity), 1, &mut cfg);
        }
        assert_eq!(cfg.window.opacity, 1.0);
    }
}
