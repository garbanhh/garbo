//! Editor de aspecto: la ventana se parte en dos. Arriba se ve el terminal en vivo (o una sesión
//! de muestra) y abajo las opciones; una barra divisoria reparte el alto. Cada clic se aplica al
//! momento y se guarda en config.toml como cualquier ajuste.
//!
//! El terminal no cambia de tamaño mientras se edita (los programas no se enteran): se dibuja
//! desplazado hacia arriba lo justo para que se vea la zona del cursor (ver `App::draw`).

use crate::atlas::UiVariant;
use crate::blocks::BlockState;
use crate::config::{Config, CursorStyle, Theme};
use crate::settings::{theme_card, Change};
use crate::style::{
    self, Background, BarSize, Blocks, Buttons, Prompt, Style, StyleOverrides, Tabs,
};
use crate::ui::{cross, dim, surface};
use crate::view::Painter;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::{test::TermSize, Term};
use std::time::Duration;

/// Filas de opciones del panel, en orden.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Themes,
    Tabs,
    Prompt,
    Blocks,
    Cursor,
    Buttons,
    Bar,
    Background,
    Radius,
    Opacity,
}

pub const GROUPS: [Group; 10] = [
    Group::Themes,
    Group::Tabs,
    Group::Prompt,
    Group::Blocks,
    Group::Cursor,
    Group::Buttons,
    Group::Bar,
    Group::Background,
    Group::Radius,
    Group::Opacity,
];

/// Opacidades que ofrece el editor (%).
const OPACITIES: [u8; 5] = [100, 95, 90, 85, 80];

fn pos<T: PartialEq>(all: &[T], v: &T) -> usize {
    all.iter().position(|x| x == v).unwrap_or(0)
}

/// El índice del valor más parecido (radios u opacidades que no están en la lista).
fn nearest(all: &[u8], v: f32) -> usize {
    (0..all.len())
        .min_by(|&a, &b| {
            (all[a] as f32 - v)
                .abs()
                .total_cmp(&(all[b] as f32 - v).abs())
        })
        .unwrap_or(0)
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Themes => "Tema",
            Group::Tabs => "Pestañas",
            Group::Prompt => "Prompt",
            Group::Blocks => "Bloques",
            Group::Cursor => "Cursor",
            Group::Buttons => "Botones",
            Group::Bar => "Barra",
            Group::Background => "Fondo",
            Group::Radius => "Esquinas",
            Group::Opacity => "Opacidad",
        }
    }

    pub fn len(self) -> usize {
        match self {
            Group::Themes => Theme::names().len(),
            Group::Tabs => Tabs::ALL.len(),
            Group::Prompt => Prompt::ALL.len(),
            Group::Blocks => Blocks::ALL.len(),
            Group::Cursor => CursorStyle::ALL.len(),
            Group::Buttons => Buttons::ALL.len(),
            Group::Bar => BarSize::ALL.len(),
            Group::Background => Background::ALL.len(),
            Group::Radius => style::RADII.len(),
            Group::Opacity => OPACITIES.len(),
        }
    }

    pub fn option(self, i: usize) -> String {
        match self {
            Group::Themes => Theme::display_name(Theme::names()[i]),
            Group::Tabs => Tabs::ALL[i].label().into(),
            Group::Prompt => Prompt::ALL[i].label().into(),
            Group::Blocks => Blocks::ALL[i].label().into(),
            Group::Cursor => CursorStyle::ALL[i].label().into(),
            Group::Buttons => Buttons::ALL[i].label().into(),
            Group::Bar => BarSize::ALL[i].label().into(),
            Group::Background => Background::ALL[i].label().into(),
            Group::Radius => format!("{} px", style::RADII[i]),
            Group::Opacity => format!("{} %", OPACITIES[i]),
        }
    }

    /// Opción elegida ahora (`style`: la forma en uso).
    pub fn current(self, cfg: &Config, style: &Style) -> usize {
        match self {
            Group::Themes => Theme::names()
                .iter()
                .position(|n| n.eq_ignore_ascii_case(&cfg.theme))
                .unwrap_or(usize::MAX),
            Group::Tabs => pos(Tabs::ALL, &style.tabs),
            Group::Prompt => pos(Prompt::ALL, &style.prompt),
            Group::Blocks => pos(Blocks::ALL, &style.blocks),
            Group::Cursor => pos(&CursorStyle::ALL, &cfg.terminal.cursor_style),
            Group::Buttons => pos(Buttons::ALL, &style.buttons),
            Group::Bar => pos(BarSize::ALL, &style.bar),
            Group::Background => pos(Background::ALL, &style.background),
            Group::Radius => nearest(&style::RADII, style.radius as f32),
            Group::Opacity => nearest(&OPACITIES, cfg.window.opacity * 100.0),
        }
    }

    /// La opción que trae el tema (se marca con un punto).
    fn theme_default(self, base: &Style) -> Option<usize> {
        Some(match self {
            Group::Tabs => pos(Tabs::ALL, &base.tabs),
            Group::Prompt => pos(Prompt::ALL, &base.prompt),
            Group::Blocks => pos(Blocks::ALL, &base.blocks),
            Group::Buttons => pos(Buttons::ALL, &base.buttons),
            Group::Bar => pos(BarSize::ALL, &base.bar),
            Group::Background => pos(Background::ALL, &base.background),
            Group::Radius => style::RADII.iter().position(|r| *r == base.radius)?,
            _ => return None,
        })
    }
}

/// Forma que trae el tema elegido, sin los cambios de `[style]`.
fn base_style(cfg: &Config) -> Style {
    Theme::builtin(&cfg.theme)
        .map(|t| t.style)
        .unwrap_or_default()
}

/// Lo que difiere de la forma del tema (lo igual no se guarda: si el tema cambia, llega solo).
fn diff(cur: &Style, base: &Style) -> StyleOverrides {
    fn d<T: PartialEq + Copy>(v: T, b: T) -> Option<T> {
        (v != b).then_some(v)
    }
    StyleOverrides {
        tabs: d(cur.tabs, base.tabs),
        prompt: d(cur.prompt, base.prompt),
        blocks: d(cur.blocks, base.blocks),
        buttons: d(cur.buttons, base.buttons),
        bar: d(cur.bar, base.bar),
        background: d(cur.background, base.background),
        radius: d(cur.radius, base.radius),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    /// Barra divisoria (se arrastra).
    Divider,
    Close,
    Sample,
    Keep,
    Reset,
    /// (grupo, opción)
    Option(usize, usize),
    Panel,
    /// La parte de arriba (el terminal).
    Above,
}

/// Teclas de navegación.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
}

/// Reparto de la ventana bajo la barra de pestañas (píxeles físicos).
#[derive(Clone, Copy, Debug)]
pub struct Regions {
    /// Terminal: de `term_y` a `term_y + term_h`.
    pub term_y: f32,
    pub term_h: f32,
    pub div_y: f32,
    pub div_h: f32,
    pub panel_y: f32,
}

struct Rect {
    hit: Hit,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// Lo dibujado en el último fotograma (para el ratón: el ancho de cada opción depende del texto).
#[derive(Default)]
struct Drawn {
    div: (f32, f32),
    panel_y: f32,
    rects: Vec<Rect>,
}

/// Un elemento de la lista desplazable: una fila de tarjetas de temas o un grupo de opciones.
struct Item {
    group: usize,
    /// Fila de tarjetas (solo en Temas).
    row: usize,
    h: f32,
}

pub struct LookEditor {
    /// Parte del alto disponible (bajo la barra) que ocupa el terminal.
    pub ratio: f32,
    /// Mostrar la sesión de muestra en vez del terminal.
    pub sample: bool,
    /// Al elegir otro tema, conservar la forma que se ve ahora.
    pub keep: bool,
    /// Grupo con el foco del teclado (índice en `GROUPS`).
    pub focus: usize,
    pub hover: Option<Hit>,
    pub dragging: bool,
    first: usize,
    wheel: f32,
    follow: bool,
    drawn: Option<Drawn>,
}

impl LookEditor {
    pub fn new(ratio: f32) -> Self {
        Self {
            ratio,
            sample: false,
            keep: false,
            focus: 0,
            hover: None,
            dragging: false,
            first: 0,
            wheel: 0.0,
            follow: true,
            drawn: None,
        }
    }

    pub fn regions(&self, height: f32, top: f32, s: f32) -> Regions {
        let avail = (height - top).max(1.0);
        let div_h = (12.0 * s).round();
        let min_term = (90.0 * s).min(avail * 0.4);
        let max_term = (avail - div_h - 150.0 * s).max(min_term);
        let term_h = (self.ratio * avail).clamp(min_term, max_term).round();
        Regions {
            term_y: top,
            term_h,
            div_y: top + term_h,
            div_h,
            panel_y: top + term_h + div_h,
        }
    }

    /// Arrastre de la barra divisoria hasta `y`.
    pub fn drag_to(&mut self, y: f32, height: f32, top: f32) {
        let avail = (height - top).max(1.0);
        self.ratio = ((y - top) / avail).clamp(0.15, 0.9);
    }

    pub fn wheel(&mut self, px: f32) {
        self.wheel += px;
        self.follow = false;
    }

    pub fn hit(&self, x: f32, y: f32) -> Hit {
        let Some(d) = &self.drawn else {
            return Hit::Above;
        };
        if y < d.div.0 {
            return Hit::Above;
        }
        if y < d.div.0 + d.div.1 {
            return Hit::Divider;
        }
        d.rects
            .iter()
            .find(|r| r.contains(x, y))
            .map_or(Hit::Panel, |r| r.hit)
    }

    /// ¿Está `y` en la lista de opciones (para la rueda)?
    pub fn in_panel(&self, y: f32) -> bool {
        self.drawn.as_ref().is_some_and(|d| y >= d.panel_y)
    }

    /// Teclado: ↑ ↓ cambian de fila; ← → cambian la opción de la fila (y se aplica).
    pub fn nav(&mut self, n: Nav, cfg: &mut Config) -> Change {
        match n {
            Nav::Up => {
                self.focus = self.focus.saturating_sub(1);
                self.follow = true;
                Change::None
            }
            Nav::Down => {
                self.focus = (self.focus + 1).min(GROUPS.len() - 1);
                self.follow = true;
                Change::None
            }
            Nav::Left | Nav::Right => {
                let g = GROUPS[self.focus];
                let n_opts = g.len() as i32;
                let cur = g.current(cfg, &cfg.theme().style);
                let d = if n == Nav::Left { -1 } else { 1 };
                let next = if cur >= g.len() {
                    0
                } else {
                    (cur as i32 + d).rem_euclid(n_opts) as usize
                };
                self.follow = true;
                self.choose(self.focus, next, cfg)
            }
        }
    }

    /// Elige la opción `i` del grupo `g`.
    pub fn choose(&mut self, g: usize, i: usize, cfg: &mut Config) -> Change {
        self.focus = g;
        let base = base_style(cfg);
        let o = &mut cfg.style;
        fn set<T: PartialEq + Copy>(slot: &mut Option<T>, v: T, base: T) {
            *slot = (v != base).then_some(v);
        }
        match GROUPS[g] {
            Group::Themes => {
                let Some(name) = Theme::names().get(i) else {
                    return Change::None;
                };
                if cfg.theme.eq_ignore_ascii_case(name) {
                    return Change::None;
                }
                // "Mantener mi forma": la que se ve ahora sigue igual con los colores del otro.
                let cur = cfg.style.apply(base);
                cfg.theme = name.to_string();
                cfg.style = if self.keep {
                    diff(&cur, &base_style(cfg))
                } else {
                    StyleOverrides::default()
                };
                return Change::Style;
            }
            Group::Tabs => set(&mut o.tabs, Tabs::ALL[i], base.tabs),
            Group::Prompt => set(&mut o.prompt, Prompt::ALL[i], base.prompt),
            Group::Blocks => set(&mut o.blocks, Blocks::ALL[i], base.blocks),
            Group::Buttons => set(&mut o.buttons, Buttons::ALL[i], base.buttons),
            Group::Bar => set(&mut o.bar, BarSize::ALL[i], base.bar),
            Group::Background => set(&mut o.background, Background::ALL[i], base.background),
            Group::Radius => set(&mut o.radius, style::RADII[i], base.radius),
            Group::Cursor => {
                cfg.terminal.cursor_style = CursorStyle::ALL[i];
                return Change::TermOptions;
            }
            Group::Opacity => {
                cfg.window.opacity = OPACITIES[i] as f32 / 100.0;
                return Change::Redraw;
            }
        }
        Change::Style
    }

    /// Vuelve a la forma del tema.
    pub fn reset(&mut self, cfg: &mut Config) -> Change {
        if cfg.style.is_empty() {
            return Change::None;
        }
        cfg.style = StyleOverrides::default();
        Change::Style
    }

    /// Lista desplazable: filas de tarjetas de temas y un elemento por grupo de opciones.
    #[allow(clippy::too_many_arguments)]
    fn items(
        &self,
        p: &mut Painter,
        avail_w: f32,
        card: (f32, f32, f32),
        chip_h: f32,
        cpad: f32,
        cgap: f32,
        row_gap: f32,
    ) -> (Vec<Item>, usize) {
        let (card_w, card_full_h, gap) = card;
        let cols = (((avail_w + gap) / (card_w + gap)).floor() as usize).max(1);
        let mut items = Vec::new();
        for row in 0..Theme::names().len().div_ceil(cols) {
            items.push(Item {
                group: 0,
                row,
                h: card_full_h + gap,
            });
        }
        for (g, group) in GROUPS.iter().enumerate().skip(1) {
            let lines = flow(p, *group, avail_w, cpad, cgap)
                .last()
                .map_or(1, |l| l.0 + 1);
            items.push(Item {
                group: g,
                row: 0,
                h: lines as f32 * chip_h + (lines as f32 - 1.0) * cgap + row_gap,
            });
        }
        (items, cols)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        p: &mut Painter,
        theme: &Theme,
        cfg: &Config,
        width: f32,
        height: f32,
        top: f32,
        s: f32,
    ) {
        let rg = self.regions(height, top, s);
        let fg = theme.foreground;
        let dimc = dim(theme);
        let hair = s.round().max(1.0);
        let base = base_style(cfg);
        let style = theme.style;

        // Barra divisoria con su asa.
        p.rect(0.0, rg.div_y, width, rg.div_h, theme.tab_bar, 1.0);
        p.rect(0.0, rg.div_y, width, hair, fg, 0.10);
        let grip_on = self.dragging || self.hover == Some(Hit::Divider);
        let (gw, gh) = ((44.0 * s).round(), (4.0 * s).round());
        p.rounded(
            ((width - gw) / 2.0).round(),
            (rg.div_y + (rg.div_h - gh) / 2.0).round(),
            gw,
            gh,
            if grip_on { theme.accent } else { fg },
            if grip_on { 1.0 } else { 0.25 },
            gh / 2.0,
        );

        // Panel
        let py = rg.panel_y;
        p.rect(0.0, py, width, height - py, surface(theme, 0.035), 1.0);
        let pad = (18.0 * s).round();
        let mut rects = Vec::new();

        // --- Cabecera: título, tema y botones
        let head_h = (44.0 * s).round();
        let th = p.ui_line_h(UiVariant::Title);
        let (cx, ccy, cr) = (width - pad - 12.0 * s, py + head_h / 2.0, 13.0 * s);
        if self.hover == Some(Hit::Close) {
            p.circle(cx, ccy, cr, fg, 0.12);
        }
        cross(p, cx, ccy, 4.0 * s, 1.5 * s, fg);
        rects.push(Rect {
            hit: Hit::Close,
            x: cx - cr,
            y: ccy - cr,
            w: 2.0 * cr,
            h: 2.0 * cr,
        });
        let bh = (28.0 * s).round();
        let by = (py + (head_h - bh) / 2.0).round();
        let mut bx = cx - cr - (10.0 * s).round();
        let modified = !cfg.style.is_empty();
        for (hit, label, on, enabled) in [
            (Hit::Reset, "Forma del tema", false, modified),
            (Hit::Keep, "Mantener mi forma", self.keep, true),
            (Hit::Sample, "Ver con un ejemplo", self.sample, true),
        ] {
            let tw = p.ui_measure(label);
            let check = if hit == Hit::Reset {
                0.0
            } else {
                (18.0 * s).round()
            };
            let w = (tw + check + 24.0 * s).round();
            bx -= w;
            let over = self.hover == Some(hit) && enabled;
            if on {
                p.rounded(bx, by, w, bh, theme.accent, 0.20, (7.0 * s).round());
                p.outline(bx, by, w, bh, theme.accent, 0.9, (7.0 * s).round(), hair);
            } else {
                p.rounded(
                    bx,
                    by,
                    w,
                    bh,
                    fg,
                    if over { 0.12 } else { 0.06 },
                    (7.0 * s).round(),
                );
            }
            let mut tx = bx + (12.0 * s).round();
            if check > 0.0 {
                // Casilla: marcada o vacía.
                let k = (11.0 * s).round();
                let ky = (by + (bh - k) / 2.0).round();
                if on {
                    p.rounded(tx, ky, k, k, theme.accent, 1.0, (3.0 * s).round());
                    p.line(
                        tx + 2.5 * s,
                        ky + k * 0.55,
                        tx + k * 0.42,
                        ky + k - 2.8 * s,
                        1.5 * s,
                        theme.background,
                        1.0,
                    );
                    p.line(
                        tx + k * 0.42,
                        ky + k - 2.8 * s,
                        tx + k - 2.2 * s,
                        ky + 2.8 * s,
                        1.5 * s,
                        theme.background,
                        1.0,
                    );
                } else {
                    p.outline(tx, ky, k, k, fg, 0.45, (3.0 * s).round(), hair.max(1.2 * s));
                }
                tx += check;
            }
            let c = if !enabled {
                dimc.lerp(theme.background, 0.3)
            } else if on || over {
                fg
            } else {
                fg.lerp(theme.background, 0.15)
            };
            p.ui_text_in(tx, by, bh, label, c, tw + 1.0);
            if enabled {
                rects.push(Rect {
                    hit,
                    x: bx,
                    y: by,
                    w,
                    h: bh,
                });
            }
            bx -= (8.0 * s).round();
        }
        // Título a la izquierda, lo que quepa.
        let room = (bx - pad - 12.0 * s).max(0.0);
        let tw = p.ui_text_v(
            pad,
            (py + (head_h - th) / 2.0).round(),
            "Aspecto",
            fg,
            room,
            UiVariant::Title,
        );
        let mut info = format!("  {}", Theme::display_name(&cfg.theme));
        if modified {
            info.push_str(" · forma retocada");
        }
        p.ui_text_in(pad + tw, py, head_h, &info, dimc, (room - tw).max(0.0));
        p.rect(pad, py + head_h - hair, width - 2.0 * pad, hair, fg, 0.08);

        // --- Lista desplazable
        let label_w = (100.0 * s).round();
        let list_x = pad + label_w;
        let bar_w = (12.0 * s).round();
        let avail_w = (width - list_x - pad - bar_w).max(80.0 * s);
        let body_y = py + head_h + (10.0 * s).round();
        let body_h = (height - body_y - (8.0 * s).round()).max(0.0);
        let card_w = (100.0 * s).round();
        let card_h = (52.0 * s).round();
        let name_h = (20.0 * s).round();
        let cgap = (6.0 * s).round();
        let chip_h = (28.0 * s).round();
        let cpad = (12.0 * s).round();
        let row_gap = (12.0 * s).round();
        let card_gap = (8.0 * s).round();
        let (items, cols) = self.items(
            p,
            avail_w,
            (card_w, card_h + name_h, card_gap),
            chip_h,
            cpad,
            cgap,
            row_gap,
        );
        let heights: Vec<f32> = items.iter().map(|it| it.h).collect();
        let first = scroll(
            &mut self.first,
            &mut self.wheel,
            &heights,
            body_h,
            self.follow.then(|| {
                let theme_row = Group::Themes
                    .current(cfg, &style)
                    .min(Theme::names().len().saturating_sub(1))
                    / cols;
                let a = items
                    .iter()
                    .position(|it| {
                        it.group == self.focus && (self.focus != 0 || it.row == theme_row)
                    })
                    .unwrap_or(0);
                (a, a)
            }),
        );
        self.follow = false;

        let mut y = body_y;
        let mut labeled = None;
        for it in items.iter().skip(first) {
            if y + it.h > body_y + body_h + 0.5 {
                break;
            }
            let g = it.group;
            let group = GROUPS[g];
            // Nombre del grupo (una vez, en su primera fila visible).
            if labeled != Some(g) {
                labeled = Some(g);
                let focused = g == self.focus;
                let c = if focused { fg } else { dimc };
                let lh = if g == 0 { card_h } else { chip_h };
                if focused {
                    p.rounded(
                        pad - (8.0 * s).round(),
                        y + (lh - 16.0 * s) / 2.0,
                        (3.0 * s).round(),
                        (16.0 * s).round(),
                        theme.accent,
                        1.0,
                        1.5 * s,
                    );
                }
                p.ui_text_in(pad, y, lh, group.label(), c, label_w - 8.0 * s);
            }
            if g == 0 {
                let cur = group.current(cfg, &style);
                for c in 0..cols {
                    let i = it.row * cols + c;
                    let Some(name) = Theme::names().get(i) else {
                        break;
                    };
                    let x = list_x + c as f32 * (card_w + card_gap);
                    let hovered = self.hover == Some(Hit::Option(0, i));
                    if let Some(t) = Theme::builtin(name) {
                        theme_card(p, &t, theme, x, y, card_w, card_h, i == cur, hovered, s);
                    }
                    let nh = p.ui_line_h(UiVariant::Small);
                    let c = if i == cur || hovered { fg } else { dimc };
                    p.ui_text_v(
                        x + 2.0 * s,
                        (y + card_h + (name_h - nh) / 2.0).round(),
                        &Theme::display_name(name),
                        c,
                        card_w - 4.0 * s,
                        UiVariant::Small,
                    );
                    rects.push(Rect {
                        hit: Hit::Option(0, i),
                        x,
                        y,
                        w: card_w,
                        h: card_h + name_h,
                    });
                }
            } else {
                let cur = group.current(cfg, &style);
                let def = group.theme_default(&base);
                for (line, cx, i, w) in flow(p, group, avail_w, cpad, cgap) {
                    let x = list_x + cx;
                    let cy = y + line as f32 * (chip_h + cgap);
                    let on = i == cur;
                    let over = self.hover == Some(Hit::Option(g, i));
                    let r = (7.0 * s).round();
                    if on {
                        p.rounded(x, cy, w, chip_h, theme.accent, 0.20, r);
                        p.outline(x, cy, w, chip_h, theme.accent, 0.95, r, hair.max(1.2 * s));
                    } else {
                        p.rounded(x, cy, w, chip_h, fg, if over { 0.11 } else { 0.05 }, r);
                    }
                    let label = group.option(i);
                    let lw = p.ui_measure(&label);
                    let c = if on || over {
                        fg
                    } else {
                        fg.lerp(theme.background, 0.3)
                    };
                    p.ui_text_in(x + cpad, cy, chip_h, &label, c, lw + 1.0);
                    if def == Some(i) {
                        p.circle(
                            x + cpad + lw + (8.0 * s).round(),
                            cy + chip_h / 2.0,
                            (2.5 * s).max(1.5),
                            theme.accent,
                            1.0,
                        );
                    }
                    rects.push(Rect {
                        hit: Hit::Option(g, i),
                        x,
                        y: cy,
                        w,
                        h: chip_h,
                    });
                }
            }
            y += it.h;
        }

        // Barra de desplazamiento si no cabe todo.
        let total: f32 = heights.iter().sum();
        if total > body_h + 0.5 && body_h > 0.0 {
            let offset: f32 = heights[..first].iter().sum();
            let size = (body_h * body_h / total).max(24.0 * s);
            let max_off = (total - body_h).max(1.0);
            let ty = body_y + (offset / max_off).min(1.0) * (body_h - size);
            let w = (4.0 * s).round();
            p.rounded(width - pad / 2.0 - w, ty, w, size, fg, 0.22, w / 2.0);
        }

        self.drawn = Some(Drawn {
            div: (rg.div_y, rg.div_h),
            panel_y: py,
            rects,
        });
    }
}

/// Reparte las opciones de un grupo en líneas: (línea, x, opción, ancho).
fn flow(
    p: &mut Painter,
    g: Group,
    avail: f32,
    cpad: f32,
    gap: f32,
) -> Vec<(usize, f32, usize, f32)> {
    let s = cpad / 12.0;
    let mut out = Vec::new();
    let (mut line, mut x) = (0, 0.0);
    for i in 0..g.len() {
        let w = (p.ui_measure(&g.option(i)) + 2.0 * cpad + (12.0 * s).round()).round();
        if x > 0.0 && x + w > avail {
            line += 1;
            x = 0.0;
        }
        out.push((line, x, i, w));
        x += w + gap;
    }
    out
}

/// Desplazamiento por elementos enteros (nada queda cortado), como en Ajustes. `show`: rango de
/// elementos que hay que dejar a la vista. Devuelve el primer elemento visible.
fn scroll(
    first: &mut usize,
    wheel: &mut f32,
    heights: &[f32],
    body_h: f32,
    show: Option<(usize, usize)>,
) -> usize {
    let mut max_first = heights.len();
    let mut tail = 0.0;
    while max_first > 0 && tail + heights[max_first - 1] <= body_h {
        max_first -= 1;
        tail += heights[max_first];
    }
    let mut f = (*first).min(max_first);
    while f > 0 && *wheel > heights[f - 1] * 0.5 {
        *wheel -= heights[f - 1];
        f -= 1;
    }
    while f < max_first && -*wheel > heights[f] * 0.5 {
        *wheel += heights[f];
        f += 1;
    }
    if (f == 0 && *wheel > 0.0) || (f == max_first && *wheel < 0.0) {
        *wheel = 0.0;
    }
    if let Some((a, b)) = show {
        f = f.min(a);
        while f < max_first.min(a)
            && heights[f..=b.min(heights.len() - 1)].iter().sum::<f32>() > body_h
        {
            f += 1;
        }
    }
    *first = f;
    f
}

/// Sesión de muestra: un terminal sin programa detrás con unos cuantos comandos ya ejecutados
/// (uno que falla) y el prompt con algo escrito. Se dibuja con el mismo código que los de verdad.
pub struct Sample {
    pub term: Term<VoidListener>,
    pub blocks: BlockState,
    pub cols: usize,
    pub lines: usize,
}

/// Rama de git que muestra el prompt de la muestra.
pub const SAMPLE_BRANCH: &str = "main";
/// Sugerencia en gris tras lo escrito.
pub const SAMPLE_GHOST: &str = "ease";

impl Sample {
    pub fn new(cfg: &Config, cols: usize) -> Self {
        let cols = cols.max(20);
        let lines = 24;
        let mut term = Term::new(
            crate::terminal::term_config(cfg),
            &TermSize::new(cols, lines),
            VoidListener,
        );
        let mut state = BlockState::default();
        state.no_history = true;
        let blocks = std::sync::Mutex::new(state);
        let prompt = "\x1b]133;A;garbo\x07  ~/Proyectos/garbo\r\n\x1b]133;B\x07";
        let steps: [(&str, &str, i32, u64); 3] = [
            (
                "git status",
                "En la rama main\r\n\
                 Tu rama está actualizada con 'origin/main'.\r\n\r\n\
                 Cambios no rastreados para el commit:\r\n\
                 \t\x1b[31mmodificados:     src/config.rs\x1b[m\r\n\
                 \t\x1b[31mmodificados:     src/ui.rs\x1b[m\r\n",
                0,
                4,
            ),
            (
                "ls",
                "\x1b[1;34massets\x1b[m  Cargo.lock  Cargo.toml  \x1b[1;34mdocs\x1b[m  \
                 README.md  \x1b[1;34mscripts\x1b[m  \x1b[1;34msrc\x1b[m\r\n",
                0,
                2,
            ),
            (
                "cargo test config",
                "   \x1b[1;32mCompiling\x1b[m garbo v0.12.0\r\n\
                 \x20   \x1b[1;32mFinished\x1b[m `test` profile in 3.08s\r\n\
                 running 3 tests\r\n\
                 test config::tests::guarda_solo_lo_cambiado ... \x1b[32mok\x1b[m\r\n\
                 test config::tests::tema_por_nombre ... \x1b[32mok\x1b[m\r\n\
                 test config::tests::tema_desconocido ... \x1b[31mFAILED\x1b[m\r\n\r\n\
                 test result: \x1b[31mFAILED\x1b[m. 2 passed; 1 failed\r\n",
                101,
                3200,
            ),
        ];
        for (cmd, out, code, ms) in steps {
            let text = format!("{prompt}{cmd}\r\n\x1b]133;C\x07{out}\x1b]133;D;{code}\x07");
            crate::pty_loop::feed_bytes(&mut term, &blocks, text.as_bytes(), &VoidListener);
            blocks
                .lock()
                .unwrap()
                .set_current_duration(Duration::from_millis(ms));
        }
        let typed = format!("{prompt}cargo build --rel");
        crate::pty_loop::feed_bytes(&mut term, &blocks, typed.as_bytes(), &VoidListener);
        Self {
            term,
            blocks: blocks.into_inner().unwrap(),
            cols,
            lines,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::Status;

    #[test]
    fn elegir_guarda_solo_lo_que_cambia() {
        let mut cfg = Config::default();
        let mut ed = LookEditor::new(0.5);
        let tabs = GROUPS.iter().position(|g| *g == Group::Tabs).unwrap();
        let pill = pos(Tabs::ALL, &Tabs::Pill);
        assert_eq!(ed.choose(tabs, pill, &mut cfg), Change::Style);
        assert_eq!(cfg.style.tabs, Some(Tabs::Pill));
        // Volver a la del tema no deja nada escrito.
        assert_eq!(ed.choose(tabs, 0, &mut cfg), Change::Style);
        assert!(cfg.style.is_empty());
        // Otro tema: se lleva su forma, salvo con "Mantener mi forma".
        ed.choose(tabs, pill, &mut cfg);
        let bosque = Theme::names().iter().position(|n| *n == "bosque");
        if let Some(i) = bosque {
            assert_eq!(ed.choose(0, i, &mut cfg), Change::Style);
            assert!(cfg.style.is_empty());
            assert_eq!(cfg.theme().style.tabs, Tabs::Folder);
            ed.choose(tabs, pill, &mut cfg);
            ed.keep = true;
            let grafito = Theme::names().iter().position(|n| *n == "grafito").unwrap();
            ed.choose(0, grafito, &mut cfg);
            assert_eq!(cfg.theme, "grafito");
            assert_eq!(cfg.theme().style.tabs, Tabs::Pill);
            // Lo que no se tocó también se conserva (era del tema anterior).
            assert_eq!(cfg.theme().style.prompt, Prompt::Solid);
            assert_eq!(ed.reset(&mut cfg), Change::Style);
            assert_eq!(cfg.theme().style.tabs, Tabs::Underline);
        }
    }

    #[test]
    fn teclado_recorre_filas_y_opciones() {
        let mut cfg = Config::default();
        let mut ed = LookEditor::new(0.5);
        assert_eq!(ed.nav(Nav::Up, &mut cfg), Change::None);
        assert_eq!(ed.focus, 0);
        // ← en la fila de temas da la vuelta al último.
        assert_eq!(ed.nav(Nav::Left, &mut cfg), Change::Style);
        assert_eq!(cfg.theme, *Theme::names().last().unwrap());
        for _ in 0..20 {
            ed.nav(Nav::Down, &mut cfg);
        }
        assert_eq!(GROUPS[ed.focus], Group::Opacity);
        assert_eq!(ed.nav(Nav::Right, &mut cfg), Change::Redraw);
        assert_eq!(cfg.window.opacity, 0.95);
        ed.focus = GROUPS.iter().position(|g| *g == Group::Cursor).unwrap();
        assert_eq!(ed.nav(Nav::Left, &mut cfg), Change::TermOptions);
        assert_eq!(cfg.terminal.cursor_style, CursorStyle::Hollow);
    }

    #[test]
    fn la_muestra_tiene_bloques_con_su_estado() {
        let cfg = Config::default();
        let s = Sample::new(&cfg, 80);
        let bs = &s.blocks;
        assert!(bs.active);
        let ids: Vec<u32> = (1..=4).filter(|id| bs.info(*id).is_some()).collect();
        assert_eq!(ids.len(), 4, "tres comandos y el prompt actual");
        assert_eq!(bs.info(ids[0]).unwrap().status(), Status::Ok);
        assert_eq!(bs.info(ids[2]).unwrap().status(), Status::Failed(101));
        assert_eq!(
            bs.info(ids[2]).unwrap().duration,
            Some(Duration::from_millis(3200))
        );
        assert_eq!(bs.idle_current(), Some(ids[3]));
        assert!(bs.styled(ids[3]));
    }

    #[test]
    fn el_reparto_respeta_los_minimos() {
        let mut ed = LookEditor::new(0.95);
        let r = ed.regions(800.0, 40.0, 1.0);
        assert!(r.panel_y <= 800.0 - 150.0 + 0.5, "{r:?}");
        ed.drag_to(0.0, 800.0, 40.0);
        let r = ed.regions(800.0, 40.0, 1.0);
        assert!(r.term_h >= 90.0, "{r:?}");
        assert_eq!(r.term_y, 40.0);
        assert_eq!(r.div_y, 40.0 + r.term_h);
    }
}
