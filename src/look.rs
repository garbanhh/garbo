//! Editor de aspecto: un panel a la derecha con cuatro secciones (Temas, Pestañas, Terminal y
//! Ventana) y, a la izquierda, el terminal entero y en vivo (o una sesión de ejemplo). Cada opción
//! se ve con un dibujo de cómo queda, hecho con el mismo código que la interfaz de verdad, y los
//! números van con un deslizador. Cada clic se aplica al momento y se guarda en config.toml.
//!
//! El terminal no cambia de tamaño mientras se edita (los programas no se enteran): el panel se
//! dibuja encima de su parte derecha.

use crate::atlas::UiVariant;
use crate::blocks::BlockState;
use crate::config::{Config, CursorStyle, Rgb, Theme};
use crate::font::Style as FontStyle;
use crate::settings::{theme_card, Change};
use crate::style::{
    self, Background, BarSize, Blocks, Buttons, Interface, Prompt, Style, StyleOverrides, Tabs,
};
use crate::tabs::{Align, Position, Width, Working};
use crate::ui::{cross, dim, surface};
use crate::view::Painter;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::{test::TermSize, Term};
use std::time::Duration;

/// Secciones del panel, en orden.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Themes,
    Tabs,
    Terminal,
    Window,
}

pub const SECTIONS: [Section; 4] = [
    Section::Themes,
    Section::Tabs,
    Section::Terminal,
    Section::Window,
];

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::Themes => "Temas",
            Section::Tabs => "Pestañas",
            Section::Terminal => "Terminal",
            Section::Window => "Ventana",
        }
    }

    /// Controles de la sección, en orden.
    pub fn controls(self) -> Vec<Control> {
        let all: &[Control] = match self {
            Section::Themes => &[Control::Theme, Control::Keep],
            Section::Tabs => &[
                Control::TabShape,
                Control::Position,
                Control::Width,
                Control::Align,
                Control::Working,
            ],
            Section::Terminal => &[
                Control::Prompt,
                Control::BlockGap,
                Control::LineGap,
                Control::Gap,
                Control::OutputGap,
                Control::Blocks,
                Control::Cursor,
            ],
            Section::Window => &[
                Control::Interface,
                Control::Bar,
                Control::Buttons,
                Control::Background,
                Control::Radius,
                Control::Opacity,
            ],
        };
        all.to_vec()
    }
}

/// Lo que se puede cambiar en el panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Control {
    Theme,
    /// Casilla: al elegir otro tema, conservar los cambios de forma.
    Keep,
    TabShape,
    Position,
    Width,
    Align,
    /// Animación mientras trabaja un agente.
    Working,
    Prompt,
    /// Espacio entre la salida anterior y la línea que separa los bloques.
    BlockGap,
    /// Espacio entre esa línea y la ruta.
    LineGap,
    /// Espacio entre la ruta y el comando.
    Gap,
    /// Espacio entre el comando y su salida.
    OutputGap,
    Blocks,
    Cursor,
    Bar,
    Buttons,
    Background,
    Radius,
    Opacity,
    /// Interfaz de toda la ventana: Flotante o Clásica.
    Interface,
}

/// Cómo se presenta cada control.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// Tarjetas de los temas.
    Themes,
    /// Muestras con un dibujo de cada opción.
    Tiles,
    /// Botones seguidos (pocas opciones con nombre).
    Segmented,
    /// Deslizador (números).
    Slider,
    /// Casilla.
    Check,
}

/// Opacidades que ofrece el editor (%).
const OPACITIES: [u8; 5] = [80, 85, 90, 95, 100];

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

impl Control {
    fn kind(self) -> Kind {
        match self {
            Control::Theme => Kind::Themes,
            Control::Keep => Kind::Check,
            Control::TabShape
            | Control::Prompt
            | Control::Blocks
            | Control::Cursor
            | Control::Buttons
            | Control::Background
            | Control::Interface => Kind::Tiles,
            Control::Position
            | Control::Width
            | Control::Align
            | Control::Working
            | Control::Bar => Kind::Segmented,
            Control::BlockGap
            | Control::LineGap
            | Control::Gap
            | Control::OutputGap
            | Control::Radius
            | Control::Opacity => Kind::Slider,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Control::Theme => "Tema",
            Control::Keep => "Al cambiar de tema, conservar mis cambios",
            Control::TabShape => "Forma",
            Control::Position => "Posición",
            Control::Width => "Ancho",
            Control::Align => "Nombre",
            Control::Working => "Agente trabajando",
            Control::Prompt => "Prompt",
            Control::BlockGap => "Espacio sobre la línea",
            Control::LineGap => "Espacio bajo la línea",
            Control::Gap => "Espacio bajo la ruta",
            Control::OutputGap => "Espacio antes de la salida",
            Control::Blocks => "Bloques de comandos",
            Control::Cursor => "Cursor",
            Control::Bar => "Altura de la barra",
            Control::Buttons => "Botones",
            Control::Interface => "Interfaz",
            Control::Background => "Fondo",
            Control::Radius => "Esquinas",
            Control::Opacity => "Opacidad",
        }
    }

    pub fn len(self) -> usize {
        match self {
            Control::Theme => Theme::names().len(),
            Control::Keep => 2,
            Control::TabShape => Tabs::ALL.len(),
            Control::Position => Position::ALL.len(),
            Control::Width => Width::ALL.len(),
            Control::Align => Align::ALL.len(),
            Control::Working => Working::ALL.len(),
            Control::Prompt => Prompt::ALL.len(),
            Control::BlockGap | Control::LineGap | Control::Gap | Control::OutputGap => {
                style::GAPS.len()
            }
            Control::Blocks => Blocks::ALL.len(),
            Control::Cursor => CursorStyle::ALL.len(),
            Control::Bar => BarSize::ALL.len(),
            Control::Buttons => Buttons::ALL.len(),
            Control::Interface => Interface::ALL.len(),
            Control::Background => Background::ALL.len(),
            Control::Radius => style::RADII.len(),
            Control::Opacity => OPACITIES.len(),
        }
    }

    pub fn option(self, i: usize) -> String {
        match self {
            Control::Theme => Theme::display_name(Theme::names()[i]),
            Control::Keep => String::new(),
            Control::TabShape => Tabs::ALL[i].label().into(),
            Control::Position => Position::ALL[i].label().into(),
            Control::Width => Width::ALL[i].label().into(),
            Control::Align => Align::ALL[i].label().into(),
            Control::Working => Working::ALL[i].label().into(),
            Control::Prompt => Prompt::ALL[i].label().into(),
            Control::BlockGap | Control::LineGap | Control::Gap | Control::OutputGap => {
                format!("{} px", style::GAPS[i])
            }
            Control::Blocks => Blocks::ALL[i].label().into(),
            Control::Cursor => CursorStyle::ALL[i].label().into(),
            Control::Bar => BarSize::ALL[i].label().into(),
            Control::Buttons => Buttons::ALL[i].label().into(),
            Control::Interface => Interface::ALL[i].label().into(),
            Control::Background => Background::ALL[i].label().into(),
            Control::Radius => format!("{} px", style::RADII[i]),
            Control::Opacity => format!("{} %", OPACITIES[i]),
        }
    }

    /// Opción elegida ahora (`style`: la forma en uso).
    pub fn current(self, cfg: &Config, style: &Style) -> usize {
        match self {
            Control::Theme => Theme::names()
                .iter()
                .position(|n| n.eq_ignore_ascii_case(&cfg.theme))
                .unwrap_or(usize::MAX),
            Control::Keep => 0,
            Control::TabShape => pos(Tabs::ALL, &style.tabs),
            Control::Position => pos(Position::ALL, &cfg.tabs.position),
            Control::Width => pos(Width::ALL, &cfg.tabs.width),
            Control::Align => pos(Align::ALL, &cfg.tabs.align),
            Control::Working => pos(Working::ALL, &cfg.tabs.working),
            Control::Prompt => pos(Prompt::ALL, &style.prompt),
            Control::BlockGap => nearest(&style::GAPS, style.block_gap as f32),
            Control::LineGap => nearest(&style::GAPS, style.line_gap as f32),
            Control::Gap => nearest(&style::GAPS, style.prompt_gap as f32),
            Control::OutputGap => nearest(&style::GAPS, style.output_gap as f32),
            Control::Blocks => pos(Blocks::ALL, &style.blocks),
            Control::Cursor => pos(&CursorStyle::ALL, &cfg.terminal.cursor_style),
            Control::Bar => pos(BarSize::ALL, &style.bar),
            Control::Buttons => pos(Buttons::ALL, &style.buttons),
            Control::Interface => pos(Interface::ALL, &style.interface),
            Control::Background => pos(Background::ALL, &style.background),
            Control::Radius => nearest(&style::RADII, style.radius as f32),
            Control::Opacity => nearest(&OPACITIES, cfg.window.opacity * 100.0),
        }
    }

    /// La opción que trae el tema (para "Deshacer"), si es parte de la forma.
    fn theme_default(self, base: &Style) -> Option<usize> {
        Some(match self {
            Control::TabShape => pos(Tabs::ALL, &base.tabs),
            Control::Prompt => pos(Prompt::ALL, &base.prompt),
            Control::BlockGap => style::GAPS.iter().position(|g| *g == base.block_gap)?,
            Control::LineGap => style::GAPS.iter().position(|g| *g == base.line_gap)?,
            Control::Gap => style::GAPS.iter().position(|g| *g == base.prompt_gap)?,
            Control::OutputGap => style::GAPS.iter().position(|g| *g == base.output_gap)?,
            Control::Blocks => pos(Blocks::ALL, &base.blocks),
            Control::Bar => pos(BarSize::ALL, &base.bar),
            Control::Buttons => pos(Buttons::ALL, &base.buttons),
            Control::Interface => pos(Interface::ALL, &base.interface),
            Control::Background => pos(Background::ALL, &base.background),
            Control::Radius => style::RADII.iter().position(|r| *r == base.radius)?,
            _ => return None,
        })
    }
}

/// Forma que trae el tema elegido, sin los cambios de `[style]`.
fn base_style(cfg: &Config) -> Style {
    cfg.base_style()
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
        prompt_gap: d(cur.prompt_gap, base.prompt_gap),
        block_gap: d(cur.block_gap, base.block_gap),
        line_gap: d(cur.line_gap, base.line_gap),
        output_gap: d(cur.output_gap, base.output_gap),
        interface: d(cur.interface, base.interface),
    }
}

/// El tema con un cambio en su forma (para dibujar la muestra de una opción).
fn themed(theme: &Theme, f: impl FnOnce(&mut Style)) -> Theme {
    let mut t = theme.clone();
    f(&mut t.style);
    t
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    /// Borde izquierdo del panel (se arrastra para cambiar su ancho).
    Divider,
    Close,
    /// Pestaña de sección (índice en `SECTIONS`).
    Section(usize),
    /// (control, opción)
    Option(Control, usize),
    /// Pista de un deslizador.
    Slider(Control),
    /// "Deshacer" de un control (vuelve a lo que trae el tema).
    Reset(Control),
    Keep,
    Sample,
    /// "Restablecer": la forma del tema entera.
    ResetAll,
    Done,
    Panel,
    /// Fuera del panel (el terminal).
    Outside,
}

/// Teclas de navegación.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    NextSection,
    PrevSection,
    Activate,
}

/// Lo que se está arrastrando con el ratón.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Drag {
    Divider,
    Slider(Control),
}

/// Sitio del panel (píxeles físicos): de `x` al borde derecho de la ventana.
#[derive(Clone, Copy, Debug)]
pub struct Regions {
    pub x: f32,
    pub w: f32,
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

/// Lo dibujado en el último fotograma (para el ratón).
#[derive(Default)]
struct Drawn {
    x: f32,
    /// Medio ancho de la zona del borde que se arrastra.
    grip: f32,
    rects: Vec<Rect>,
    /// Pista de cada deslizador: (control, x inicial, x final).
    sliders: Vec<(Control, f32, f32)>,
}

/// Un trozo de la lista desplazable: el título de un control, una fila de sus muestras o todo
/// él (los que ocupan una sola fila).
#[derive(Clone, Copy)]
struct Item {
    control: Control,
    /// `None`: el título; `Some(fila)`: una fila del contenido.
    row: Option<usize>,
    h: f32,
}

/// Reparto de las muestras de un control: columnas, ancho de cada una, alto del dibujo y alto
/// total de una fila (con el nombre y el hueco).
#[derive(Clone, Copy)]
struct Grid {
    cols: usize,
    cell_w: f32,
    preview_h: f32,
    row_h: f32,
}

pub struct LookEditor {
    /// Ancho del panel respecto al de la ventana.
    pub ratio: f32,
    /// Mostrar la sesión de ejemplo en vez del terminal.
    pub sample: bool,
    /// Al elegir otro tema, conservar la forma que se ve ahora.
    pub keep: bool,
    /// Sección visible (índice en `SECTIONS`).
    pub section: usize,
    /// Control con el foco del teclado (índice en los de la sección).
    pub focus: usize,
    pub hover: Option<Hit>,
    pub dragging: Option<Drag>,
    /// Desplazamiento de la lista (píxeles) y hacia dónde va (la animación lo acerca).
    scroll: f32,
    target: f32,
    last: Option<std::time::Instant>,
    /// La lista se movía en el fotograma anterior (si no, la animación empieza ahora).
    moving: bool,
    /// Hay que dejar a la vista el control con el foco (tras moverlo con el teclado).
    follow: bool,
    drawn: Option<Drawn>,
}

/// Rapidez de la animación del desplazamiento (1/s): en ~0,15 s llega casi al destino.
const SCROLL_SPEED: f32 = 22.0;

impl LookEditor {
    pub fn new(ratio: f32) -> Self {
        Self {
            ratio,
            sample: false,
            keep: false,
            section: 0,
            focus: 0,
            hover: None,
            dragging: None,
            scroll: 0.0,
            target: 0.0,
            last: None,
            moving: false,
            follow: true,
            drawn: None,
        }
    }

    pub fn regions(&self, width: f32, s: f32) -> Regions {
        let min = (300.0 * s).min(width * 0.7);
        let max = (width - 200.0 * s).max(min);
        let w = (self.ratio * width).clamp(min, max).round();
        Regions { x: width - w, w }
    }

    /// Arrastre del borde del panel hasta `x`.
    pub fn drag_to(&mut self, x: f32, width: f32) {
        self.ratio = ((width - x) / width.max(1.0)).clamp(0.15, 0.75);
    }

    pub fn wheel(&mut self, px: f32) {
        // Hacia arriba (`px` > 0) la lista baja.
        self.target -= px;
        self.follow = false;
    }

    /// ¿Se está moviendo la lista? (hace falta otro fotograma).
    pub fn animating(&self) -> bool {
        (self.target - self.scroll).abs() >= 0.5
    }

    /// ¿Se ven las muestras animadas de "Agente trabajando"? (sección Pestañas)
    pub fn shows_working(&self) -> bool {
        SECTIONS.get(self.section) == Some(&Section::Tabs)
    }

    /// ¿Está `x` sobre el panel (para la rueda)?
    pub fn in_panel(&self, x: f32) -> bool {
        self.drawn.as_ref().is_some_and(|d| x >= d.x)
    }

    pub fn hit(&self, x: f32, y: f32) -> Hit {
        let Some(d) = &self.drawn else {
            return Hit::Outside;
        };
        if (x - d.x).abs() < d.grip {
            return Hit::Divider;
        }
        if x < d.x {
            return Hit::Outside;
        }
        let _ = y;
        d.rects
            .iter()
            .find(|r| r.contains(x, y))
            .map_or(Hit::Panel, |r| r.hit)
    }

    /// Clic o arrastre en la pista de un deslizador: elige el valor más cercano a `x`.
    pub fn slide(&mut self, c: Control, x: f32, cfg: &mut Config) -> Change {
        let Some(&(_, x0, x1)) = self
            .drawn
            .as_ref()
            .and_then(|d| d.sliders.iter().find(|s| s.0 == c))
        else {
            return Change::None;
        };
        let n = c.len().max(2) - 1;
        let i = (((x - x0) / (x1 - x0).max(1.0)) * n as f32)
            .round()
            .clamp(0.0, n as f32) as usize;
        if i == c.current(cfg, &cfg.theme().style) {
            self.focus_on(c);
            return Change::None;
        }
        self.choose(c, i, cfg)
    }

    pub fn set_section(&mut self, i: usize) {
        if i != self.section {
            self.section = i.min(SECTIONS.len() - 1);
            self.focus = 0;
            self.scroll = 0.0;
            self.target = 0.0;
        }
        self.follow = true;
    }

    fn controls(&self) -> Vec<Control> {
        SECTIONS[self.section].controls()
    }

    /// Pone el foco del teclado en `c` (si está en la sección visible).
    fn focus_on(&mut self, c: Control) {
        if let Some(i) = self.controls().iter().position(|x| *x == c) {
            self.focus = i;
        }
    }

    /// Teclado: ↑ ↓ cambian de control; ← → cambian su valor (y se aplica); Tab y Mayús+Tab,
    /// de sección; Intro o espacio marcan la casilla.
    pub fn nav(&mut self, n: Nav, cfg: &mut Config) -> Change {
        let controls = self.controls();
        let count = SECTIONS.len();
        match n {
            Nav::NextSection => self.set_section((self.section + 1) % count),
            Nav::PrevSection => self.set_section((self.section + count - 1) % count),
            Nav::Up => {
                self.focus = self.focus.saturating_sub(1);
                self.follow = true;
            }
            Nav::Down => {
                self.focus = (self.focus + 1).min(controls.len().saturating_sub(1));
                self.follow = true;
            }
            Nav::Activate => {
                if controls.get(self.focus) == Some(&Control::Keep) {
                    self.keep = !self.keep;
                }
            }
            Nav::Left | Nav::Right => {
                let Some(&c) = controls.get(self.focus) else {
                    return Change::None;
                };
                if c == Control::Keep {
                    self.keep = !self.keep;
                    return Change::None;
                }
                let d = if n == Nav::Left { -1 } else { 1 };
                let len = c.len() as i32;
                let cur = c.current(cfg, &cfg.theme().style);
                let next = if cur >= c.len() {
                    0
                } else if c.kind() == Kind::Slider {
                    (cur as i32 + d).clamp(0, len - 1) as usize
                } else {
                    (cur as i32 + d).rem_euclid(len) as usize
                };
                self.follow = true;
                if next != cur {
                    return self.choose(c, next, cfg);
                }
            }
        }
        Change::None
    }

    /// Elige la opción `i` del control `c`.
    pub fn choose(&mut self, c: Control, i: usize, cfg: &mut Config) -> Change {
        self.focus_on(c);
        if i >= c.len() {
            return Change::None;
        }
        let base = base_style(cfg);
        let o = &mut cfg.style;
        fn set<T: PartialEq + Copy>(slot: &mut Option<T>, v: T, base: T) {
            *slot = (v != base).then_some(v);
        }
        match c {
            Control::Theme => {
                let name = Theme::names()[i];
                if cfg.theme.eq_ignore_ascii_case(name) {
                    return Change::None;
                }
                // "Conservar mis cambios": la forma que se ve ahora sigue igual con los colores
                // del otro tema.
                let cur = cfg.style.apply(base);
                cfg.theme = name.to_string();
                cfg.style = if self.keep {
                    diff(&cur, &base_style(cfg))
                } else {
                    StyleOverrides::default()
                };
            }
            Control::Keep => {
                self.keep = !self.keep;
                return Change::None;
            }
            Control::TabShape => set(&mut o.tabs, Tabs::ALL[i], base.tabs),
            Control::Prompt => set(&mut o.prompt, Prompt::ALL[i], base.prompt),
            Control::BlockGap => set(&mut o.block_gap, style::GAPS[i], base.block_gap),
            Control::LineGap => set(&mut o.line_gap, style::GAPS[i], base.line_gap),
            Control::Gap => set(&mut o.prompt_gap, style::GAPS[i], base.prompt_gap),
            Control::OutputGap => set(&mut o.output_gap, style::GAPS[i], base.output_gap),
            Control::Blocks => set(&mut o.blocks, Blocks::ALL[i], base.blocks),
            Control::Bar => set(&mut o.bar, BarSize::ALL[i], base.bar),
            Control::Buttons => set(&mut o.buttons, Buttons::ALL[i], base.buttons),
            Control::Interface => set(&mut o.interface, Interface::ALL[i], base.interface),
            Control::Background => set(&mut o.background, Background::ALL[i], base.background),
            Control::Radius => set(&mut o.radius, style::RADII[i], base.radius),
            Control::Position => {
                cfg.tabs.position = Position::ALL[i];
                return Change::Layout;
            }
            Control::Width => {
                cfg.tabs.width = Width::ALL[i];
                return Change::Layout;
            }
            Control::Align => {
                cfg.tabs.align = Align::ALL[i];
                return Change::Redraw;
            }
            Control::Working => {
                cfg.tabs.working = Working::ALL[i];
                return Change::Redraw;
            }
            Control::Cursor => {
                cfg.terminal.cursor_style = CursorStyle::ALL[i];
                return Change::TermOptions;
            }
            Control::Opacity => {
                cfg.window.opacity = OPACITIES[i] as f32 / 100.0;
                return Change::Redraw;
            }
        }
        Change::Style
    }

    /// "Deshacer" de un control: vuelve a lo que trae el tema.
    pub fn reset_control(&mut self, c: Control, cfg: &mut Config) -> Change {
        self.focus_on(c);
        let o = &mut cfg.style;
        match c {
            Control::TabShape => o.tabs = None,
            Control::Prompt => o.prompt = None,
            Control::BlockGap => o.block_gap = None,
            Control::LineGap => o.line_gap = None,
            Control::Gap => o.prompt_gap = None,
            Control::OutputGap => o.output_gap = None,
            Control::Blocks => o.blocks = None,
            Control::Bar => o.bar = None,
            Control::Buttons => o.buttons = None,
            Control::Interface => o.interface = None,
            Control::Background => o.background = None,
            Control::Radius => o.radius = None,
            _ => return Change::None,
        }
        Change::Style
    }

    /// "Restablecer": vuelve a la forma del tema.
    pub fn reset(&mut self, cfg: &mut Config) -> Change {
        if cfg.style.is_empty() {
            return Change::None;
        }
        cfg.style = StyleOverrides::default();
        Change::Style
    }

    /// Reparto de las muestras de un control con `w` píxeles de ancho.
    fn grid(c: Control, w: f32, ch: f32, cw: f32, s: f32) -> Grid {
        let gap = (8.0 * s).round();
        let name_h = (22.0 * s).round();
        let min = match c {
            Control::Theme => 118.0 * s,
            Control::Prompt => (13.0 * cw + 26.0 * s).max(140.0 * s),
            Control::Cursor => (4.0 * cw + 24.0 * s).max(64.0 * s),
            // Sus nombres son largos ("Estado a la derecha"): de dos en dos.
            Control::Blocks => 130.0 * s,
            _ => 86.0 * s,
        };
        let cols = (((w + gap) / (min + gap)).floor() as usize).clamp(1, c.len().max(1));
        let cell_w = ((w - (cols - 1) as f32 * gap) / cols as f32).floor();
        let preview_h = match c {
            Control::Theme => (cell_w * 0.5).round(),
            Control::Prompt => (2.0 * ch + 18.0 * s).round(),
            Control::Cursor => (ch + 16.0 * s).round(),
            Control::Blocks => (cell_w * 0.44).round(),
            _ => (cell_w * 0.5).round(),
        };
        let pad = if c == Control::Theme { 0.0 } else { 10.0 * s };
        Grid {
            cols,
            cell_w,
            preview_h,
            row_h: (preview_h + pad + name_h + gap).round(),
        }
    }

    /// La lista desplazable de la sección visible.
    fn items(&self, w: f32, ch: f32, cw: f32, s: f32) -> Vec<Item> {
        let head = (26.0 * s).round();
        let space = (18.0 * s).round();
        let mut items = Vec::new();
        for (k, c) in self.controls().into_iter().enumerate() {
            let lead = if k == 0 { 0.0 } else { space };
            match c.kind() {
                Kind::Themes | Kind::Tiles => {
                    items.push(Item {
                        control: c,
                        row: None,
                        h: lead + head,
                    });
                    let g = Self::grid(c, w, ch, cw, s);
                    for row in 0..c.len().div_ceil(g.cols) {
                        items.push(Item {
                            control: c,
                            row: Some(row),
                            h: g.row_h,
                        });
                    }
                }
                // Con su dibujo: los botones llevan un icono encima del nombre y el deslizador una
                // muestra a la izquierda.
                Kind::Segmented => items.push(Item {
                    control: c,
                    row: None,
                    h: lead + head + (SEGMENT_H * s).round(),
                }),
                Kind::Slider => items.push(Item {
                    control: c,
                    row: None,
                    h: lead + head + (SLIDER_H * s).round(),
                }),
                Kind::Check => items.push(Item {
                    control: c,
                    row: None,
                    h: lead + (34.0 * s).round(),
                }),
            }
        }
        items
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
        let rg = self.regions(width, s);
        let fg = theme.foreground;
        let dimc = dim(theme);
        let hair = s.round().max(1.0);
        let m = p.metrics();
        let panel_bg = surface(theme, 0.035);
        // Interfaz Flotante: el panel es una tarjeta separada del terminal por un hueco y con
        // margen a la derecha y abajo. Lo que tapa la lista al desplazarse queda dentro de su
        // borde y con sus esquinas.
        let chrome = crate::chrome::Chrome::new(&theme.style, s);
        let full_h = height;
        let (x0, w, height) = if chrome.floating {
            let (g, mg) = (chrome.gap, chrome.margin);
            (rg.x + g, rg.w - g - mg, height - mg)
        } else {
            (rg.x, rg.w, height)
        };
        let cover = |p: &mut Painter, y: f32, h: f32| {
            if chrome.floating {
                let r = (chrome.radius - hair).max(0.0);
                p.rounded(
                    x0 + hair,
                    y.max(top + hair),
                    w - 2.0 * hair,
                    h,
                    panel_bg,
                    1.0,
                    r,
                );
            } else {
                p.rect(x0, y, w, h, panel_bg, 1.0);
            }
        };
        let mut sliders = Vec::new();
        let pad = (18.0 * s).round();
        let (ix, iw) = (x0 + pad, (w - 2.0 * pad).max(40.0 * s));
        if chrome.floating {
            p.rect(rg.x, top, rg.w, full_h - top, theme.tab_bar, 1.0);
            p.card(x0, top, w, height - top, panel_bg, fg, chrome.radius, s);
        } else {
            p.rect(x0, top, w, height - top, panel_bg, 1.0);
        }

        // Dónde va cada parte: cabecera, secciones, lista y pie.
        let th = p.ui_line_h(UiVariant::Title);
        let nh = p.ui_line_h(UiVariant::Small);
        let ty = top + (14.0 * s).round();
        let sy = ty + th + nh + (16.0 * s).round();
        let sh = (34.0 * s).round();
        let foot_h = (58.0 * s).round();
        let fy = height - foot_h;
        let body_y = sy + sh + (18.0 * s).round();
        let body_h = (fy - (10.0 * s).round() - body_y).max(0.0);

        // --- Lista de la sección, desplazada por píxeles (con una animación corta). Lo que asoma
        // por arriba o por abajo lo tapan luego la cabecera y el pie.
        let items = self.items(iw, m.height, m.width, s);
        let total: f32 = items.iter().map(|it| it.h).sum();
        let max = (total - body_h).max(0.0);
        let focus = self.controls().get(self.focus).copied();
        if self.follow {
            // Que se vea lo que tiene el foco (en los temas, la fila del elegido).
            let theme_row = (focus == Some(Control::Theme)).then(|| {
                let g = Self::grid(Control::Theme, iw, m.height, m.width, s);
                Control::Theme
                    .current(cfg, &theme.style)
                    .min(Control::Theme.len() - 1)
                    / g.cols
            });
            let mut y = 0.0;
            let (mut a, mut b) = (None, 0.0);
            for it in &items {
                let wanted =
                    Some(it.control) == focus && theme_row.is_none_or(|r| it.row == Some(r));
                if wanted {
                    a.get_or_insert(y);
                    b = y + it.h;
                }
                y += it.h;
            }
            if let Some(a) = a {
                if b - self.target > body_h {
                    self.target = b - body_h;
                }
                if a < self.target {
                    self.target = a;
                }
            }
            self.follow = false;
        }
        self.target = self.target.clamp(0.0, max);
        let now = std::time::Instant::now();
        // Si estaba quieta, el primer paso es de un fotograma (no del rato que pasó quieta).
        let dt = match self.last.filter(|_| self.moving) {
            Some(t) => now.saturating_duration_since(t).as_secs_f32().min(0.1),
            None => 1.0 / 60.0,
        };
        self.last = Some(now);
        self.scroll += (self.target - self.scroll) * (1.0 - (-dt * SCROLL_SPEED).exp());
        if (self.target - self.scroll).abs() < 0.5 {
            self.scroll = self.target;
        }
        self.scroll = self.scroll.clamp(0.0, max);
        self.moving = self.animating();
        let base = base_style(cfg);
        let mut body_rects = Vec::new();
        let mut y = body_y - self.scroll.round();
        for it in &items {
            // Flotante: lo que se saldría por debajo de la tarjeta no se dibuja (asomaba en el
            // margen, bajo el pie).
            let fits = !chrome.floating || y + it.h <= height;
            if y + it.h > body_y && y < body_y + body_h && fits {
                let ctx = Ctx {
                    theme,
                    cfg,
                    base: &base,
                    focus,
                    s,
                };
                self.draw_item(p, &ctx, *it, (ix, y, iw), &mut body_rects, &mut sliders);
            }
            y += it.h;
        }
        // El texto de la lista, debajo de lo que la tapa.
        p.out.append(p.fg);
        cover(p, top, body_y - top);
        let inside = if chrome.floating { hair } else { 0.0 };
        cover(p, body_y + body_h, height - body_y - body_h - inside);

        // Solo responde lo que se ve.
        let mut rects: Vec<Rect> = Vec::new();
        for mut r in body_rects {
            let (y0, y1) = (r.y.max(body_y), (r.y + r.h).min(body_y + body_h));
            if y1 > y0 {
                r.y = y0;
                r.h = y1 - y0;
                rects.push(r);
            }
        }

        // Borde izquierdo del panel (se arrastra para cambiar el ancho).
        if self.dragging == Some(Drag::Divider) || self.hover == Some(Hit::Divider) {
            p.rect(x0 - hair, top, 2.0 * hair, height - top, theme.accent, 1.0);
        } else if !chrome.floating {
            p.rect(x0, top, hair, height - top, fg, 0.12);
        }

        // --- Cabecera: título, tema y cerrar
        let th = p.ui_line_h(UiVariant::Title);
        let nh = p.ui_line_h(UiVariant::Small);
        let ty = top + (14.0 * s).round();
        let (cx, cy, cr) = (x0 + w - pad - 10.0 * s, (ty + th / 2.0).round(), 13.0 * s);
        let room = (cx - cr - ix - 8.0 * s).max(0.0);
        p.ui_text_v(ix, ty, "Aspecto", fg, room, UiVariant::Title);
        let mut info = Theme::display_name(&cfg.theme);
        if !cfg.style.is_empty() {
            info.push_str(" · con cambios");
        }
        p.ui_text_v(
            ix,
            ty + th + (2.0 * s).round(),
            &info,
            dimc,
            room,
            UiVariant::Small,
        );
        if self.hover == Some(Hit::Close) {
            p.circle(cx, cy, cr, fg, 0.12);
        }
        cross(p, cx, cy, 4.0 * s, 1.5 * s, fg);
        rects.push(Rect {
            hit: Hit::Close,
            x: cx - cr,
            y: cy - cr,
            w: 2.0 * cr,
            h: 2.0 * cr,
        });

        // --- Secciones
        let sy = ty + th + nh + (16.0 * s).round();
        let sh = (34.0 * s).round();
        let labels: Vec<String> = SECTIONS.iter().map(|x| x.label().to_string()).collect();
        let hov = match self.hover {
            Some(Hit::Section(i)) => Some(i),
            _ => None,
        };
        for (i, r) in segmented(
            p,
            theme,
            (ix, sy, iw, sh),
            &labels,
            self.section,
            hov,
            s,
            None,
        )
        .into_iter()
        .enumerate()
        {
            rects.push(Rect {
                hit: Hit::Section(i),
                x: r[0],
                y: r[1],
                w: r[2],
                h: r[3],
            });
        }

        // --- Pie: sesión de ejemplo, restablecer y listo
        let foot_h = (58.0 * s).round();
        let fy = height - foot_h;
        p.rect(x0 + hair, fy, w - hair, hair, fg, 0.08);
        let bh = (30.0 * s).round();
        let by = (fy + (foot_h - bh) / 2.0).round();
        let br = (7.0 * s).round();
        let done = "Listo";
        let dw = (p.ui_measure(done) + 28.0 * s).round();
        let dx = x0 + w - pad - dw;
        let over = self.hover == Some(Hit::Done);
        p.rounded(
            dx,
            by,
            dw,
            bh,
            theme.accent,
            if over { 1.0 } else { 0.88 },
            br,
        );
        p.ui_text_in(dx + 14.0 * s, by, bh, done, theme.background, dw);
        rects.push(Rect {
            hit: Hit::Done,
            x: dx,
            y: by,
            w: dw,
            h: bh,
        });
        let modified = !cfg.style.is_empty();
        let reset = "Restablecer";
        let rw = (p.ui_measure(reset) + 24.0 * s).round();
        let rx = dx - (8.0 * s).round() - rw;
        let over = modified && self.hover == Some(Hit::ResetAll);
        p.rounded(rx, by, rw, bh, fg, if over { 0.12 } else { 0.06 }, br);
        let c = if modified {
            fg
        } else {
            dimc.lerp(theme.background, 0.35)
        };
        p.ui_text_in(rx + 12.0 * s, by, bh, reset, c, rw);
        if modified {
            rects.push(Rect {
                hit: Hit::ResetAll,
                x: rx,
                y: by,
                w: rw,
                h: bh,
            });
        }
        let over = self.hover == Some(Hit::Sample);
        let kw = check(
            p,
            theme,
            (ix, by, bh),
            "Ejemplo",
            self.sample,
            over,
            (rx - 10.0 * s - ix).max(0.0),
            s,
        );
        rects.push(Rect {
            hit: Hit::Sample,
            x: ix,
            y: by,
            w: kw,
            h: bh,
        });

        // Barra de desplazamiento si no cabe todo.
        if max > 0.5 && body_h > 0.0 {
            let size = (body_h * body_h / total).max(24.0 * s);
            let ty = body_y + (self.scroll / max).min(1.0) * (body_h - size);
            let bw = (4.0 * s).round();
            p.rounded(
                x0 + w - pad / 2.0 - bw / 2.0,
                ty,
                bw,
                size,
                fg,
                0.22,
                bw / 2.0,
            );
        }

        // Flotante: bajo la tarjeta, el margen con el fondo de la ventana. Al final y por encima
        // del texto: el de la lista desplazada no asoma por ahí.
        if chrome.floating {
            p.out.append(p.fg);
            p.rect(rg.x, height, rg.w, full_h - height, theme.tab_bar, 1.0);
        }
        self.drawn = Some(Drawn {
            x: x0,
            grip: (5.0 * s).round(),
            rects,
            sliders,
        });
    }

    /// Un trozo de la lista: el título de un control o su contenido (una fila de muestras, los
    /// botones seguidos, el deslizador o la casilla).
    fn draw_item(
        &self,
        p: &mut Painter,
        ctx: &Ctx,
        it: Item,
        (x, y, w): (f32, f32, f32),
        rects: &mut Vec<Rect>,
        sliders: &mut Vec<(Control, f32, f32)>,
    ) {
        let (theme, s) = (ctx.theme, ctx.s);
        let c = it.control;
        let fg = theme.foreground;
        let m = p.metrics();
        let head = (26.0 * s).round();
        let body = (34.0 * s).round();
        let cur = c.current(ctx.cfg, &theme.style);
        let focused = ctx.focus == Some(c);
        match (c.kind(), it.row) {
            (Kind::Check, _) => {
                let lead = it.h - body;
                let over = self.hover == Some(Hit::Keep);
                if focused {
                    focus_mark(p, theme, x, y + lead, body, s);
                }
                let kw = check(
                    p,
                    theme,
                    (x, y + lead, body),
                    c.label(),
                    self.keep,
                    over,
                    w,
                    s,
                );
                rects.push(Rect {
                    hit: Hit::Keep,
                    x,
                    y: y + lead,
                    w: kw,
                    h: body,
                });
            }
            (Kind::Themes | Kind::Tiles, None) => {
                self.heading(p, ctx, c, cur, (x, y + it.h - head, w), rects);
            }
            (Kind::Segmented, _) => {
                let bh = (SEGMENT_H * s).round();
                let lead = it.h - head - bh;
                self.heading(p, ctx, c, cur, (x, y + lead, w), rects);
                let labels: Vec<String> = (0..c.len()).map(|i| c.option(i)).collect();
                let hov = match self.hover {
                    Some(Hit::Option(h, i)) if h == c => Some(i),
                    _ => None,
                };
                let r = (x, y + lead + head, w, bh);
                let icon = |p: &mut Painter, i: usize, r: [f32; 4], col: Rgb| {
                    option_icon(p, c, i, r, col, s);
                };
                for (i, r) in segmented(p, theme, r, &labels, cur, hov, s, Some(&icon))
                    .into_iter()
                    .enumerate()
                {
                    rects.push(Rect {
                        hit: Hit::Option(c, i),
                        x: r[0],
                        y: r[1],
                        w: r[2],
                        h: r[3],
                    });
                }
            }
            (Kind::Slider, _) => {
                let sh = (SLIDER_H * s).round();
                let lead = it.h - head - sh;
                self.heading(p, ctx, c, cur, (x, y + lead, w), rects);
                let sy = y + lead + head;
                let n = c.len();
                let cur = cur.min(n - 1);
                // A la izquierda, lo que se ajusta (con el valor de ahora).
                let bw = (84.0 * s).round();
                slider_sample(p, theme, c, cur, [x, sy, bw, sh - (4.0 * s).round()], s);
                let vw = (p.ui_measure("100 %") + 12.0 * s).round();
                let (t0, t1) = (x + bw + (18.0 * s).round(), x + w - vw - (14.0 * s).round());
                let body = sh - (4.0 * s).round();
                let yc = (sy + body / 2.0).round();
                let th = (4.0 * s).round();
                p.rounded(t0, yc - th / 2.0, t1 - t0, th, fg, 0.14, th / 2.0);
                let at = |j: usize| t0 + j as f32 / (n - 1).max(1) as f32 * (t1 - t0);
                let kx = at(cur);
                p.rounded(t0, yc - th / 2.0, kx - t0, th, theme.accent, 1.0, th / 2.0);
                for j in 0..n {
                    let (col, a) = if j <= cur {
                        (theme.background, 0.6)
                    } else {
                        (fg, 0.35)
                    };
                    p.circle(at(j), yc, 1.4 * s, col, a);
                }
                let active =
                    self.hover == Some(Hit::Slider(c)) || self.dragging == Some(Drag::Slider(c));
                p.circle(
                    kx,
                    yc,
                    if active { 9.0 } else { 8.0 } * s,
                    theme.accent,
                    1.0,
                );
                p.circle(kx, yc, 3.0 * s, theme.background, 1.0);
                p.ui_text_in(t1 + 14.0 * s, sy, body, &c.option(cur), fg, vw);
                rects.push(Rect {
                    hit: Hit::Slider(c),
                    x: t0 - 12.0 * s,
                    y: sy,
                    w: t1 - t0 + 24.0 * s,
                    h: body,
                });
                sliders.push((c, t0, t1));
            }
            (Kind::Themes | Kind::Tiles, Some(row)) => {
                let g = Self::grid(c, w, m.height, m.width, s);
                let gap = (8.0 * s).round();
                let name_h = (22.0 * s).round();
                let pad = if c == Control::Theme {
                    0.0
                } else {
                    (5.0 * s).round()
                };
                for col in 0..g.cols {
                    let i = row * g.cols + col;
                    if i >= c.len() {
                        break;
                    }
                    let tx = x + col as f32 * (g.cell_w + gap);
                    let on = i == cur;
                    let over = self.hover == Some(Hit::Option(c, i));
                    let fh = g.preview_h + 2.0 * pad + name_h;
                    if c == Control::Theme {
                        if let Some(t) = Theme::builtin(Theme::names()[i]) {
                            theme_card(p, &t, theme, tx, y, g.cell_w, g.preview_h, on, over, s);
                        }
                    } else {
                        let r = (8.0 * s).round();
                        let hair = s.round().max(1.0);
                        if on {
                            p.rounded(tx, y, g.cell_w, fh, theme.accent, 0.12, r);
                            p.outline(
                                tx,
                                y,
                                g.cell_w,
                                fh,
                                theme.accent,
                                1.0,
                                r,
                                (1.6 * s).max(hair),
                            );
                        } else {
                            let (fa, la) = if over { (0.07, 0.30) } else { (0.035, 0.10) };
                            p.rounded(tx, y, g.cell_w, fh, fg, fa, r);
                            p.outline(tx, y, g.cell_w, fh, fg, la, r, hair);
                        }
                        let pr = [tx + pad, y + pad, g.cell_w - 2.0 * pad, g.preview_h];
                        preview(p, theme, c, i, pr, s);
                    }
                    let label = c.option(i);
                    let nh = p.ui_line_h(UiVariant::Small);
                    let lw = p
                        .ui_measure_v(&label, UiVariant::Small)
                        .min(g.cell_w - 8.0 * s);
                    let col_c = if on || over { fg } else { dim(theme) };
                    p.ui_text_v(
                        (tx + (g.cell_w - lw) / 2.0).round(),
                        (y + g.preview_h + 2.0 * pad + (name_h - nh) / 2.0 - pad).round(),
                        &label,
                        col_c,
                        g.cell_w - 8.0 * s,
                        UiVariant::Small,
                    );
                    rects.push(Rect {
                        hit: Hit::Option(c, i),
                        x: tx,
                        y,
                        w: g.cell_w,
                        h: fh,
                    });
                }
            }
        }
    }

    /// Título de un control, con la marca del foco y, si se cambió respecto al tema, "Deshacer".
    fn heading(
        &self,
        p: &mut Painter,
        ctx: &Ctx,
        c: Control,
        cur: usize,
        (x, y, w): (f32, f32, f32),
        rects: &mut Vec<Rect>,
    ) {
        let (theme, s) = (ctx.theme, ctx.s);
        let head = (26.0 * s).round();
        let fg = theme.foreground;
        if ctx.focus == Some(c) {
            focus_mark(p, theme, x, y, head, s);
        }
        let mut room = w;
        if c.theme_default(ctx.base).is_some_and(|d| d != cur) {
            let l = "Deshacer";
            let lw = p.ui_measure_v(l, UiVariant::Small);
            let lx = x + w - lw;
            let nh = p.ui_line_h(UiVariant::Small);
            let col = if self.hover == Some(Hit::Reset(c)) {
                fg
            } else {
                theme.accent
            };
            p.ui_text_v(
                lx,
                (y + (head - nh) / 2.0).round(),
                l,
                col,
                lw + 1.0,
                UiVariant::Small,
            );
            rects.push(Rect {
                hit: Hit::Reset(c),
                x: lx - 4.0 * s,
                y,
                w: lw + 8.0 * s,
                h: head,
            });
            room = (lx - x - 12.0 * s).max(0.0);
        }
        let col = if ctx.focus == Some(c) {
            fg
        } else {
            fg.lerp(theme.background, 0.12)
        };
        p.ui_text_in(x, y, head, c.label(), col, room);
    }
}

/// Lo que comparten los trozos de la lista al dibujarse.
struct Ctx<'a> {
    theme: &'a Theme,
    cfg: &'a Config,
    /// Forma que trae el tema (para "Deshacer").
    base: &'a Style,
    focus: Option<Control>,
    s: f32,
}

/// Marca del control con el foco del teclado: una raya de color a la izquierda.
fn focus_mark(p: &mut Painter, theme: &Theme, x: f32, y: f32, h: f32, s: f32) {
    let mh = (14.0 * s).round();
    p.rounded(
        x - (10.0 * s).round(),
        (y + (h - mh) / 2.0).round(),
        (3.0 * s).round(),
        mh,
        theme.accent,
        1.0,
        1.5 * s,
    );
}

/// Alto de los botones seguidos con icono y del deslizador con su muestra (píxeles lógicos).
const SEGMENT_H: f32 = 58.0;
const SLIDER_H: f32 = 50.0;

/// Dibuja el icono de la opción `i` en el rectángulo dado, con el color dado.
type Icon<'a> = &'a dyn Fn(&mut Painter, usize, [f32; 4], Rgb);

/// Botones seguidos en (x, y, ancho, alto), con un icono encima de cada nombre si se da `icon`.
/// Devuelve el rectángulo de cada uno.
#[allow(clippy::too_many_arguments)]
fn segmented(
    p: &mut Painter,
    theme: &Theme,
    (x, y, w, h): (f32, f32, f32, f32),
    labels: &[String],
    cur: usize,
    hover: Option<usize>,
    s: f32,
    icon: Option<Icon>,
) -> Vec<[f32; 4]> {
    let n = labels.len().max(1);
    let r = (8.0 * s).round();
    let fg = theme.foreground;
    let hair = s.round().max(1.0);
    p.rounded(x, y, w, h, fg, 0.05, r);
    let seg = w / n as f32;
    let inset = (3.0 * s).round();
    let mut out = Vec::with_capacity(n);
    for (i, l) in labels.iter().enumerate() {
        let sx = (x + i as f32 * seg).round();
        let sw = (x + (i + 1) as f32 * seg).round() - sx;
        let on = i == cur;
        let over = hover == Some(i);
        let (bx, by, bw, bh) = (sx + inset, y + inset, sw - 2.0 * inset, h - 2.0 * inset);
        if on {
            p.rounded(bx, by, bw, bh, theme.accent, 0.22, r - inset);
            p.outline(bx, by, bw, bh, theme.accent, 0.9, r - inset, hair);
        } else if over {
            p.rounded(bx, by, bw, bh, fg, 0.07, r - inset);
        }
        let c = if on || over {
            fg
        } else {
            fg.lerp(theme.background, 0.3)
        };
        let room = (sw - 6.0 * s).max(0.0);
        // Si no cabe (o va con icono), con la letra pequeña.
        let v = if icon.is_some() || p.ui_measure(l) > room {
            UiVariant::Small
        } else {
            UiVariant::Normal
        };
        let tw = p.ui_measure_v(l, v).min(room);
        let lh = p.ui_line_h(v);
        let ty = match icon {
            Some(icon) => {
                let (iw, ih) = ((30.0 * s).round(), (19.0 * s).round());
                let iy = (by + 6.0 * s).round();
                let col = if on {
                    theme.accent
                } else if over {
                    fg
                } else {
                    fg.lerp(theme.background, 0.35)
                };
                icon(p, i, [(sx + (sw - iw) / 2.0).round(), iy, iw, ih], col);
                (by + bh - lh - 5.0 * s).round()
            }
            None => (y + (h - lh) / 2.0).round(),
        };
        p.ui_text_v((sx + (sw - tw) / 2.0).round(), ty, l, c, room, v);
        out.push([sx, y, sw, h]);
    }
    out
}

/// Icono de la opción `i` de un control de botones seguidos: dónde van las pestañas, su ancho,
/// dónde va el nombre o la altura de la barra, en `r` y con el color `col`.
fn option_icon(p: &mut Painter, c: Control, i: usize, r: [f32; 4], col: Rgb, s: f32) {
    let [x, y, w, h] = r;
    let t = (1.3 * s).max(1.0);
    let rr = (2.5 * s).round();
    match c {
        Control::Position => {
            // Una ventana con la franja de las pestañas en su borde.
            p.outline(x, y, w, h, col, 0.9, rr, t);
            let k = (4.0 * s).round();
            let (sx, sy, sw, sh) = match Position::ALL[i] {
                Position::Top => (x, y, w, k),
                Position::Bottom => (x, y + h - k, w, k),
                Position::Left => (x, y, k + 2.0 * s, h),
                Position::Right => (x + w - k - 2.0 * s, y, k + 2.0 * s, h),
            };
            p.rounded(sx, sy, sw, sh, col, 0.9, rr.min(sh / 2.0).min(sw / 2.0));
        }
        Control::Width => {
            // Una barra con dos pestañas del ancho de la opción.
            let bh = (h * 0.55).round();
            let by = (y + (h - bh) / 2.0).round();
            p.outline(x, by, w, bh, col, 0.5, rr, t);
            let frac = [0.22, 0.32, 0.42, 0.5][i.min(3)];
            let pw = ((w - 4.0 * s) * frac).round();
            for k in 0..2 {
                let px = x + 2.0 * s + k as f32 * (pw + 1.5 * s);
                p.rounded(px, by + 2.0 * s, pw, bh - 4.0 * s, col, 0.9, rr * 0.6);
            }
        }
        Control::Align => {
            // Una pestaña con el nombre a la izquierda o en medio.
            let bh = (h * 0.6).round();
            let by = (y + (h - bh) / 2.0).round();
            p.outline(x, by, w, bh, col, 0.9, rr, t);
            let lw = (w * 0.42).round();
            let lh = (2.5 * s).round().max(2.0);
            let lx = match Align::ALL[i] {
                Align::Left => x + 4.0 * s,
                Align::Center => x + (w - lw) / 2.0,
            };
            p.rounded(
                lx.round(),
                (by + (bh - lh) / 2.0).round(),
                lw,
                lh,
                col,
                0.9,
                lh / 2.0,
            );
        }
        Control::Working => {
            // La animación misma, en marcha (el editor pide fotogramas mientras se ve).
            let ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            let k = s * 1.4;
            let anim = crate::tabs::Anim {
                ms,
                kind: Working::ALL[i],
            };
            crate::panel::spinner(p, x + w / 2.0 - 6.0 * k, y + h / 2.0, k, col, anim);
        }
        Control::Bar => {
            // Una ventana con la barra de arriba más o menos alta.
            p.outline(x, y, w, h, col, 0.9, rr, t);
            let bh = ([3.0, 5.0, 7.0][i.min(2)] * s).round();
            p.rounded(x, y, w, bh, col, 0.9, rr.min(bh / 2.0));
        }
        _ => {}
    }
}

/// Muestra de lo que ajusta un deslizador con la opción `i`: el hueco marcado en color entre la
/// salida anterior, la línea entre bloques, la ruta, el comando y su salida; las esquinas, o la
/// transparencia.
fn slider_sample(p: &mut Painter, theme: &Theme, c: Control, i: usize, r: [f32; 4], s: f32) {
    let [x, y, w, h] = r;
    let fg = theme.foreground;
    let hair = s.round().max(1.0);
    let rad = (5.0 * s).round();
    match c {
        Control::BlockGap | Control::LineGap | Control::Gap | Control::OutputGap => {
            p.rounded(x, y, w, h, theme.background, 1.0, rad);
            p.outline(x, y, w, h, fg, 0.10, rad, hair);
            let line = (3.0 * s).round().max(2.0);
            let step = (3.0 * s).round();
            let half = (step / 2.0).round();
            let v = (style::GAPS[i] as f32 * s * 0.55).round();
            let which = match c {
                Control::BlockGap => 0,
                Control::LineGap => 1,
                Control::Gap => 2,
                _ => 3,
            };
            let (lx, lw) = (x + (9.0 * s).round(), w - (18.0 * s).round());
            // Salida anterior, línea entre bloques (fina y a todo lo ancho), ruta (con su
            // carpeta), comando y salida; con los huecos de siempre entre ellos.
            let rows: [(f32, Rgb, f32, f32); 5] = [
                (0.75, fg, 0.30, line),
                (1.0, fg, 0.30, hair),
                (0.55, theme.accent, 0.75, line),
                (0.35, fg, 0.85, line),
                (0.65, fg, 0.30, line),
            ];
            let gaps = [half, half, step, step];
            let total = 4.0 * line + hair + gaps.iter().sum::<f32>() + v;
            let mut ly = (y + (h - total) / 2.0).round();
            for (k, (frac, col, a, th)) in rows.into_iter().enumerate() {
                if k == 1 {
                    p.rect(lx, ly, lw, th, col, a);
                } else {
                    p.rounded(lx, ly, (lw * frac).round(), th, col, a, th / 2.0);
                }
                ly += th;
                if let Some(&base) = gaps.get(k) {
                    let g = base + if k == which { v } else { 0.0 };
                    if k == which {
                        // El espacio que se ajusta, marcado.
                        let band = g.max(hair);
                        p.rect(x + 4.0 * s, ly, w - 8.0 * s, band, theme.accent, 0.28);
                    }
                    ly += g;
                }
            }
        }
        Control::Radius => {
            p.rounded(x, y, w, h, theme.background, 1.0, rad);
            let rr = (style::RADII[i] as f32 * s * 0.9).round();
            let (cx, cy, cw, ch) = (x + 12.0 * s, y + 8.0 * s, w - 24.0 * s, h - 16.0 * s);
            p.rounded(cx, cy, cw, ch, surface(theme, 0.12), 1.0, rr);
            p.outline(cx, cy, cw, ch, theme.accent, 1.0, rr, (1.6 * s).max(hair));
        }
        Control::Opacity => {
            // Cuadros detrás y la ventana encima con su transparencia.
            let k = (6.0 * s).round();
            let (cols, rows) = ((w / k).ceil() as usize, (h / k).ceil() as usize);
            for row in 0..rows {
                for col in 0..cols {
                    let a = if (row + col) % 2 == 0 { 0.22 } else { 0.08 };
                    let cw = k.min(x + w - (x + col as f32 * k));
                    let ch = k.min(y + h - (y + row as f32 * k));
                    p.rect(x + col as f32 * k, y + row as f32 * k, cw, ch, fg, a);
                }
            }
            let a = OPACITIES[i] as f32 / 100.0;
            let (wx, wy, ww, wh) = (x + 10.0 * s, y + 7.0 * s, w - 20.0 * s, h - 14.0 * s);
            p.rounded(wx, wy, ww, wh, theme.background, a, rad);
            let line = (3.0 * s).round().max(2.0);
            p.rounded(
                wx + 6.0 * s,
                wy + wh / 2.0 - line / 2.0,
                ww * 0.5,
                line,
                fg,
                0.6,
                line / 2.0,
            );
        }
        _ => {}
    }
}

/// Casilla con su texto en (x, y, alto). Devuelve el ancho que ocupa.
#[allow(clippy::too_many_arguments)]
fn check(
    p: &mut Painter,
    theme: &Theme,
    (x, y, h): (f32, f32, f32),
    label: &str,
    on: bool,
    over: bool,
    max_w: f32,
    s: f32,
) -> f32 {
    let fg = theme.foreground;
    let k = (16.0 * s).round();
    let ky = (y + (h - k) / 2.0).round();
    let r = (4.0 * s).round();
    if on {
        p.rounded(x, ky, k, k, theme.accent, 1.0, r);
        p.line(
            x + 3.5 * s,
            ky + k * 0.55,
            x + k * 0.42,
            ky + k - 4.0 * s,
            1.8 * s,
            theme.background,
            1.0,
        );
        p.line(
            x + k * 0.42,
            ky + k - 4.0 * s,
            x + k - 3.2 * s,
            ky + 4.0 * s,
            1.8 * s,
            theme.background,
            1.0,
        );
    } else {
        let a = if over { 0.75 } else { 0.45 };
        p.outline(x, ky, k, k, fg, a, r, (1.3 * s).max(1.0));
    }
    let tx = x + k + (10.0 * s).round();
    let room = (max_w - (tx - x)).max(0.0);
    let tw = p.ui_measure(label).min(room);
    let c = if on || over {
        fg
    } else {
        fg.lerp(theme.background, 0.15)
    };
    p.ui_text_in(tx, y, h, label, c, room);
    tx - x + tw
}

/// Dibujo de la opción `i` del control `c` en `r`: cómo queda, con los colores del tema.
fn preview(p: &mut Painter, theme: &Theme, c: Control, i: usize, r: [f32; 4], s: f32) {
    let [x, y, w, h] = r;
    match c {
        Control::TabShape => {
            let t = themed(theme, |st| st.tabs = Tabs::ALL[i]);
            crate::ui::tab_sample(p, &t, r, s * 0.7);
        }
        Control::Buttons => crate::ui::buttons_sample(p, theme, Buttons::ALL[i], r, s * 0.8),
        Control::Interface => {
            // La barra arriba y el terminal debajo: pegado a los bordes o como tarjeta.
            let bh = (h * 0.3).round();
            p.rect(x, y, w, h, theme.tab_bar, 1.0);
            if Interface::ALL[i] == Interface::Floating {
                let m = (5.0 * s).round();
                let side = (w * 0.3).round();
                let rr = (5.0 * s).round();
                let (cy, ch) = (y + bh, h - bh - m);
                p.rounded(x + m, cy, w - 3.0 * m - side, ch, theme.background, 1.0, rr);
                p.rounded(x + w - m - side, cy, side, ch, theme.background, 1.0, rr);
            } else {
                let hair = s.round().max(1.0);
                p.rect(x, y + bh, w, h - bh, theme.background, 1.0);
                p.rect(x, y + bh, w, hair, theme.foreground, 0.08);
                let side = (w * 0.3).round();
                p.rect(x + w - side, y + bh, side, h - bh, theme.tab_bar, 1.0);
            }
        }
        Control::Background => {
            let bh = (h * 0.3).round();
            p.rect(x, y, w, bh, theme.tab_bar, 1.0);
            p.rect(x, y + bh, w, h - bh, theme.background, 1.0);
            if Background::ALL[i] == Background::Gradient {
                p.gradient(
                    x,
                    y + bh,
                    w,
                    ((h - bh) * 0.85).round(),
                    theme.accent,
                    crate::ui::GRADIENT * 1.8,
                    theme.accent,
                    0.0,
                );
            }
            let lh = (3.0 * s).round().max(2.0);
            for (k, f) in [0.55_f32, 0.8, 0.4].iter().enumerate() {
                let ly = y + bh + (8.0 * s) + k as f32 * (7.0 * s);
                let lw = (w - 16.0 * s) * f;
                p.rounded(
                    x + 8.0 * s,
                    ly.round(),
                    lw,
                    lh,
                    theme.foreground,
                    0.35,
                    lh / 2.0,
                );
            }
        }
        Control::Blocks => blocks_sample(p, theme, Blocks::ALL[i], r, s),
        Control::Cursor => {
            p.rect(x, y, w, h, theme.background, 1.0);
            let m = p.metrics();
            let ty = (y + (h - m.height) / 2.0).round();
            let tx = (x + 10.0 * s).round();
            for (k, ch) in "ls".chars().enumerate() {
                p.glyph(
                    ch,
                    FontStyle::Bold,
                    false,
                    false,
                    tx + k as f32 * m.width,
                    ty,
                    theme.foreground,
                    1.0,
                );
            }
            let cx = tx + 3.0 * m.width;
            let t = (2.0 * s).round().max(1.0);
            let col = theme.cursor;
            match CursorStyle::ALL[i] {
                CursorStyle::Block => p.rect(cx, ty, m.width, m.height, col, 1.0),
                CursorStyle::Beam => p.rect(cx, ty, t, m.height, col, 1.0),
                CursorStyle::Underline => p.rect(cx, ty + m.height - t, m.width, t, col, 1.0),
                CursorStyle::Hollow => {
                    p.outline(cx, ty, m.width, m.height, col, 1.0, 0.0, t * 0.75)
                }
            }
        }
        Control::Prompt => {
            let t = themed(theme, |st| st.prompt = Prompt::ALL[i]);
            p.rect(x, y, w, h, crate::ui::terminal_top(theme), 1.0);
            crate::view::prompt_sample(
                p,
                &t,
                (x + 8.0 * s).round(),
                (y + 9.0 * s).round(),
                w - 16.0 * s,
                s,
            );
        }
        _ => {}
    }
}

/// Muestra de las marcas de los bloques: dos comandos (el primero bien y el segundo con error),
/// con la ruta y unas líneas de salida como rayas.
fn blocks_sample(p: &mut Painter, theme: &Theme, b: Blocks, r: [f32; 4], s: f32) {
    let [x, y, w, h] = r;
    let fg = theme.foreground;
    let (ok, err) = (theme.palette[2], theme.palette[1]);
    let hair = s.round().max(1.0);
    p.rect(x, y, w, h, theme.background, 1.0);
    let line = (3.0 * s).round().max(2.0);
    let pitch = (7.0 * s).round();
    let gutter = x + (6.0 * s).round();
    let left = x + (14.0 * s).round();
    let text_w = w - (left - x) - 10.0 * s;
    let bh = ((h - 4.0 * s) / 2.0).floor();
    let rr = (theme.style.r(s) * 0.6).round();
    for (n, failed) in [(0usize, false), (1, true)] {
        let y0 = y + (2.0 * s).round() + n as f32 * bh;
        let y1 = y0 + bh - (2.0 * s).round();
        let status = if failed { err } else { ok };
        let (cx0, cw) = (x + 3.0 * s, w - 6.0 * s);
        match b {
            Blocks::Card => {
                let (c, fa, la) = if failed {
                    (err, 0.07, 0.32)
                } else {
                    (fg, 0.04, 0.08)
                };
                p.rounded(cx0, y0, cw, y1 - y0, c, fa, rr);
                p.outline(cx0, y0, cw, y1 - y0, c, la, rr, hair);
            }
            Blocks::Outline => {
                let (c, a) = if failed { (err, 0.40) } else { (fg, 0.16) };
                p.outline(cx0, y0, cw, y1 - y0, c, a, rr, hair);
            }
            Blocks::Chips => {
                let (c, fa, la) = if failed {
                    (err, 0.06, 0.22)
                } else {
                    (fg, 0.03, 0.06)
                };
                p.rounded(cx0, y0, cw, y1 - y0, c, fa, rr);
                p.outline(cx0, y0, cw, y1 - y0, c, la, rr, hair);
                // La píldora del resultado, arriba a la derecha.
                let (pw, ph) = ((12.0 * s).round(), (5.0 * s).round());
                p.rounded(
                    cx0 + cw - pw - 3.0 * s,
                    y0 + 3.0 * s,
                    pw,
                    ph,
                    status,
                    0.5,
                    ph / 2.0,
                );
            }
            Blocks::Header => {
                let (c, a) = if failed { (err, 0.13) } else { (fg, 0.06) };
                p.rect(x, y0, w, pitch + 3.0 * s, c, a);
            }
            Blocks::Errors if failed => {
                p.rect(x, y0, w, y1 - y0, err, 0.10);
                p.rect(x, y0, (3.0 * s).round(), y1 - y0, err, 1.0);
            }
            _ => {}
        }
        // La ruta (más tenue) y dos líneas de salida.
        for (k, (f, a)) in [(0.45_f32, 0.55_f32), (0.8, 0.35), (0.55, 0.35)]
            .into_iter()
            .enumerate()
        {
            let ly = (y0 + 4.0 * s + k as f32 * pitch).round();
            if ly + line > y1 {
                break;
            }
            p.rounded(left, ly, text_w * f, line, fg, a, line / 2.0);
        }
        let bar_w = (2.5 * s).round().max(2.0);
        let mark_h = (y1 - y0 - 6.0 * s).max(bar_w);
        match b {
            Blocks::Classic => {
                if n == 1 {
                    p.rect(left, y0, w - (left - x) - 4.0 * s, hair, fg, 0.12);
                }
                p.rounded(
                    gutter,
                    y0 + 3.0 * s,
                    bar_w,
                    mark_h,
                    status,
                    0.8,
                    bar_w / 2.0,
                );
            }
            Blocks::Bar => {
                let (c, a) = if failed { (err, 1.0) } else { (fg, 0.22) };
                p.rounded(gutter, y0 + 3.0 * s, bar_w, mark_h, c, a, bar_w / 2.0);
            }
            Blocks::Dot => p.circle(gutter + bar_w / 2.0, y0 + 5.5 * s, 2.2 * s, status, 1.0),
            Blocks::Separator if n == 1 => p.rect(x, y0, w, hair, fg, 0.16),
            Blocks::Dashed if n == 1 => {
                let (dash, step) = ((5.0 * s).round(), (9.0 * s).round());
                let mut dx = left;
                while dx < x + w - 4.0 * s {
                    p.rect(dx, y0, dash.min(x + w - 4.0 * s - dx), hair, fg, 0.30);
                    dx += step;
                }
            }
            Blocks::Status => {
                p.circle(x + w - 9.0 * s, y0 + 5.5 * s, 2.4 * s, status, 1.0);
            }
            Blocks::Fade if n == 0 => {
                p.rect(x, y0, w, y1 - y0, theme.background, 0.5);
            }
            _ => {}
        }
    }
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
        // Como la escribe la integración de cada shell: en Windows, la de PowerShell.
        let prompt = if cfg!(windows) {
            "\x1b]133;A;garbo\x07  ~\\Proyectos\\garbo\r\n\x1b]133;B\x07"
        } else {
            "\x1b]133;A;garbo\x07  ~/Proyectos/garbo\r\n\x1b]133;B\x07"
        };
        // `ls` de PowerShell es una tabla larga y sin colores: allí, las ramas.
        let listing = if cfg!(windows) {
            ("git branch", "* \x1b[32mmain\x1b[m\r\n  windows\r\n", 0, 40)
        } else {
            (
                "ls",
                "\x1b[1;34massets\x1b[m  Cargo.lock  Cargo.toml  \x1b[1;34mdocs\x1b[m  \
                 README.md  \x1b[1;34mscripts\x1b[m  \x1b[1;34msrc\x1b[m\r\n",
                0,
                2,
            )
        };
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
            listing,
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

    fn at(c: Control) -> (usize, usize) {
        for (si, sec) in SECTIONS.iter().enumerate() {
            if let Some(ci) = sec.controls().iter().position(|x| *x == c) {
                return (si, ci);
            }
        }
        panic!("{c:?} no está en ninguna sección");
    }

    #[test]
    fn interfaz_en_el_editor() {
        let mut cfg = Config::default();
        let mut ed = LookEditor::new(0.34);
        let clasica = pos(Interface::ALL, &Interface::Classic);
        assert_eq!(
            ed.choose(Control::Interface, clasica, &mut cfg),
            Change::Style
        );
        assert_eq!(cfg.style.interface, Some(Interface::Classic));
        assert_eq!(cfg.theme().style.interface, Interface::Classic);
        assert_eq!(
            ed.reset_control(Control::Interface, &mut cfg),
            Change::Style
        );
        assert!(cfg.style.is_empty());
    }

    #[test]
    fn elegir_guarda_solo_lo_que_cambia() {
        let mut cfg = Config::default();
        let mut ed = LookEditor::new(0.34);
        let pill = pos(Tabs::ALL, &Tabs::Pill);
        let del_tema = pos(Tabs::ALL, &cfg.theme().style.tabs);
        let otra = pos(Tabs::ALL, &Tabs::Underline);
        assert_eq!(ed.choose(Control::TabShape, otra, &mut cfg), Change::Style);
        assert_eq!(cfg.style.tabs, Some(Tabs::Underline));
        // Volver a la del tema no deja nada escrito.
        assert_eq!(
            ed.choose(Control::TabShape, del_tema, &mut cfg),
            Change::Style
        );
        assert!(cfg.style.is_empty());
        // Otro tema: se lleva su forma, salvo con "conservar mis cambios".
        ed.choose(Control::TabShape, pill, &mut cfg);
        let bosque = Theme::names().iter().position(|n| *n == "bosque");
        if let Some(i) = bosque {
            assert_eq!(ed.choose(Control::Theme, i, &mut cfg), Change::Style);
            assert!(cfg.style.is_empty());
            assert_eq!(cfg.theme().style.tabs, Tabs::Folder);
            ed.choose(Control::TabShape, pill, &mut cfg);
            ed.keep = true;
            let grafito = Theme::names().iter().position(|n| *n == "grafito").unwrap();
            ed.choose(Control::Theme, grafito, &mut cfg);
            assert_eq!(cfg.theme, "grafito");
            assert_eq!(cfg.theme().style.tabs, Tabs::Pill);
            // Lo que no se tocó también se conserva (era del tema anterior).
            assert_eq!(cfg.theme().style.prompt, Prompt::Solid);
            // "Deshacer" en un control y "Restablecer" en todos.
            assert_eq!(ed.reset_control(Control::TabShape, &mut cfg), Change::Style);
            assert_eq!(cfg.theme().style.tabs, Tabs::Underline);
            assert_eq!(cfg.theme().style.prompt, Prompt::Solid);
            assert_eq!(ed.reset(&mut cfg), Change::Style);
            assert_eq!(cfg.theme().style.prompt, Prompt::Border);
            assert_eq!(ed.reset(&mut cfg), Change::None);
        }
    }

    #[test]
    fn teclado_recorre_secciones_y_controles() {
        let mut cfg = Config::default();
        let mut ed = LookEditor::new(0.34);
        assert_eq!(SECTIONS[ed.section], Section::Themes);
        // ← en los temas da la vuelta al último.
        assert_eq!(ed.nav(Nav::Left, &mut cfg), Change::Style);
        assert_eq!(cfg.theme, *Theme::names().last().unwrap());
        // ↓ a la casilla y ← → la marcan.
        ed.nav(Nav::Down, &mut cfg);
        assert_eq!(ed.nav(Nav::Right, &mut cfg), Change::None);
        assert!(ed.keep);
        // Tab: la sección siguiente; Mayús+Tab da la vuelta.
        ed.nav(Nav::NextSection, &mut cfg);
        assert_eq!(SECTIONS[ed.section], Section::Tabs);
        assert_eq!(ed.focus, 0);
        ed.nav(Nav::PrevSection, &mut cfg);
        ed.nav(Nav::PrevSection, &mut cfg);
        assert_eq!(SECTIONS[ed.section], Section::Window);
        // Los deslizadores no dan la vuelta.
        let (si, ci) = at(Control::Opacity);
        ed.set_section(si);
        ed.focus = ci;
        assert_eq!(cfg.window.opacity, 1.0);
        assert_eq!(ed.nav(Nav::Right, &mut cfg), Change::None);
        assert_eq!(ed.nav(Nav::Left, &mut cfg), Change::Redraw);
        assert_eq!(cfg.window.opacity, 0.95);
        let (si, ci) = at(Control::Cursor);
        ed.set_section(si);
        ed.focus = ci;
        assert_eq!(ed.nav(Nav::Left, &mut cfg), Change::TermOptions);
        assert_eq!(cfg.terminal.cursor_style, CursorStyle::Hollow);
    }

    #[test]
    fn todos_los_controles_estan_y_una_sola_vez() {
        let all: Vec<Control> = SECTIONS.iter().flat_map(|s| s.controls()).collect();
        for c in &all {
            assert_eq!(all.iter().filter(|x| *x == c).count(), 1, "{c:?}");
            assert!(c.len() > 0);
            let cfg = Config::default();
            let st = cfg.theme().style;
            assert!(c.current(&cfg, &st) < c.len() || *c == Control::Keep);
        }
        assert!(all.contains(&Control::Position));
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
    fn el_panel_respeta_los_minimos() {
        let mut ed = LookEditor::new(0.95);
        let r = ed.regions(1600.0, 1.0);
        assert!(r.x >= 200.0 - 0.5, "queda terminal a la vista: {r:?}");
        ed.drag_to(1590.0, 1600.0);
        let r = ed.regions(1600.0, 1.0);
        assert!(r.w >= 300.0, "{r:?}");
        assert_eq!(r.x + r.w, 1600.0);
    }
}
