//! Ajustes: ventana compacta con las categorías a la izquierda y sus opciones a la derecha.
//! Los cambios se aplican en vivo y se guardan solos en config.toml (respetando los comentarios y
//! el resto del archivo).

use crate::atlas::UiVariant;
use crate::config::{Config, CursorStyle, NotifyWhen, Rgb, TabKey, Theme};
use crate::ui::{cross, dim, surface};
use crate::update::{CheckState, UpdateState};
use crate::view::Painter;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    /// Interfaz de toda la ventana: Flotante o Clásica.
    Interface,
    /// Abre el editor de aspecto.
    Appearance,
    Theme,
    /// Pestañas: borde, ancho, nombre, animación de "trabajando" y cambiar el nombre.
    TabsPosition,
    TabsWidth,
    TabsAlign,
    TabsWorking,
    RenameTab,
    FontFamily,
    FontSize,
    LineHeight,
    CursorStyle,
    CursorBlink,
    CopyOnSelect,
    Scrollback,
    Autosuggest,
    TabKey,
    CompleteAsYouType,
    Blocks,
    GarboPrompt,
    SshIntegration,
    ShellIntegration,
    NotifyAgents,
    NotifyMinWork,
    NotifySound,
    NotifyBlocked,
    NotifyTest,
    Padding,
    Opacity,
    Decorations,
    EditFile,
    CheckNow,
    CheckUpdates,
    Version,
    /// Ajustes → Atajos: la lista entera (solo para consultar).
    Shortcuts,
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
    /// "Probar aviso" de la categoría Avisos.
    TestNotice,
    Updates,
    /// Botón "Buscar" / "Actualizar" de la categoría Actualizaciones.
    CheckNow,
    /// Cambió la forma (`[style]`) o el tema desde el editor de aspecto.
    Style,
    /// Abrir el editor de aspecto.
    OpenLook,
    /// Cambiar el nombre de la pestaña actual (cierra Ajustes).
    RenameTab,
}

/// Categorías (columna izquierda) y sus opciones (derecha).
const CATEGORIES: &[(&str, &[(Field, &str)])] = &[
    (
        "Tema",
        &[
            (Field::Interface, "Interfaz"),
            (Field::Appearance, "Aspecto"),
            (Field::Theme, "Tema"),
        ],
    ),
    (
        "Pestañas",
        &[
            (Field::TabsPosition, "Posición"),
            (Field::TabsWidth, "Ancho"),
            (Field::TabsAlign, "Nombre"),
            (Field::TabsWorking, "Agente trabajando"),
            (Field::RenameTab, "Cambiar el nombre"),
        ],
    ),
    (
        "Texto",
        &[
            (Field::FontFamily, "Fuente"),
            (Field::FontSize, "Tamaño"),
            (Field::LineHeight, "Interlineado"),
        ],
    ),
    (
        "Terminal",
        &[
            (Field::CursorStyle, "Cursor"),
            (Field::CursorBlink, "Cursor parpadeante"),
            (Field::CopyOnSelect, "Copiar al seleccionar"),
            (Field::Scrollback, "Historial"),
        ],
    ),
    (
        "Completado",
        &[
            (Field::Autosuggest, "Sugerencias en gris"),
            (Field::TabKey, "Tecla Tab"),
            (Field::CompleteAsYouType, "Menú al escribir"),
        ],
    ),
    (
        "Bloques",
        &[
            (Field::Blocks, "Bloques de comandos"),
            (Field::GarboPrompt, "Prompt de Garbo"),
            (Field::SshIntegration, "Garbo en sesiones ssh"),
            (Field::ShellIntegration, "Integración con el shell"),
        ],
    ),
    (
        "Avisos",
        &[
            (Field::NotifyAgents, "Cuando un agente termine"),
            (Field::NotifyMinWork, "Si trabajó al menos"),
            (Field::NotifySound, "Con sonido"),
            (Field::NotifyBlocked, "Si te necesita"),
            (Field::NotifyTest, "Probar aviso"),
        ],
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
    ("Atajos", &[(Field::Shortcuts, "Atajos")]),
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
    CATEGORIES
        .iter()
        .flat_map(|(_, rows)| rows.iter().map(|r| r.0))
        .collect()
}

/// La opción siguiente (o anterior) de una lista, dando la vuelta.
fn cycle_opt<T: Copy + PartialEq>(all: &[T], cur: T, d: i32) -> T {
    let pos = all.iter().position(|x| *x == cur).unwrap_or(0) as i32;
    all[(pos + d).rem_euclid(all.len() as i32) as usize]
}

fn cat_len(cat: usize) -> usize {
    CATEGORIES[cat].1.len()
}

fn label_of(f: Field) -> &'static str {
    CATEGORIES
        .iter()
        .flat_map(|(_, r)| r.iter())
        .find(|r| r.0 == f)
        .map_or("", |r| r.1)
}

/// Rango [inicio, fin) de los índices de `fields()` de una categoría.
fn cat_range(cat: usize) -> (usize, usize) {
    let start: usize = (0..cat).map(cat_len).sum();
    (start, start + cat_len(cat))
}

fn cat_of(field_idx: usize) -> usize {
    (0..CATEGORIES.len())
        .find(|&c| field_idx < cat_range(c).1)
        .unwrap_or(0)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Item {
    /// Fila de ajuste (índice en `fields()`).
    Row(usize),
    /// Una fila de la cuadrícula de temas.
    Themes(usize),
    /// Atajos: la nota de arriba ("Se cambian en…").
    Note,
    /// Atajos: el título de una sección.
    Heading(usize),
    /// Atajos: (sección, fila).
    Key(usize, usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Close,
    Outside,
    Category(usize),
    Row(usize),
    Dec(usize),
    Inc(usize),
    /// Tarjeta de un tema (índice en `Theme::names()`).
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
    /// Nota "Se aplica a las pestañas nuevas" (prompt de Garbo).
    tabs_note: bool,
    pub save_failed: bool,
    pub hover: Option<Hit>,
    /// Lista de Ajustes → Atajos y la `[keybindings]` de la que salió.
    shortcuts: Vec<crate::shortcuts::Section>,
    shortcuts_src: Option<std::collections::HashMap<String, String>>,
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
    /// (elemento, y, alto) de la lista de atajos visible.
    keys: Vec<(Item, f32, f32)>,
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
            tabs_note: false,
            save_failed: false,
            hover: None,
            shortcuts: Vec::new(),
            shortcuts_src: None,
        }
    }

    /// Rehace la lista de atajos si cambió `[keybindings]` (o la primera vez).
    pub fn refresh_shortcuts(&mut self, cfg: &Config) {
        if self.shortcuts_src.as_ref() != Some(&cfg.keybindings) {
            self.shortcuts = crate::shortcuts::sections(&cfg.keybindings);
            self.shortcuts_src = Some(cfg.keybindings.clone());
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
            // La siguiente (o anterior), dando la vuelta.
            Nav::NextCategory => self.set_cat((self.cat + 1) % n_cats),
            Nav::PrevCategory => self.set_cat((self.cat + n_cats - 1) % n_cats),
            // En la cuadrícula de temas, ↑ ↓ cambian de fila de tarjetas.
            Nav::Up | Nav::Down if on_theme => {
                let cur = Theme::names()
                    .iter()
                    .position(|t| t.eq_ignore_ascii_case(&cfg.theme))
                    .unwrap_or(0) as i32;
                let step = if n == Nav::Up {
                    -(self.cols as i32)
                } else {
                    self.cols as i32
                };
                let next = cur + step;
                if (0..Theme::names().len() as i32).contains(&next) {
                    self.follow = true;
                    return self.pick_theme(next as usize, cfg);
                }
                // Pasada la primera o la última fila de tarjetas, a la opción de al lado.
                self.move_sel(if n == Nav::Up { -1 } else { 1 });
            }
            // En Atajos (una sola opción, la lista) ↑ ↓ desplazan la lista elemento a elemento.
            Nav::Up | Nav::Down if self.field(self.selected) == Field::Shortcuts => {
                self.first = if n == Nav::Up {
                    self.first.saturating_sub(1)
                } else {
                    self.first + 1
                };
                self.wheel = 0.0;
                self.follow = false;
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
        let Some(name) = Theme::names().get(i) else {
            return Change::None;
        };
        self.theme_idx = i;
        self.selected = fields()
            .iter()
            .position(|f| *f == Field::Theme)
            .unwrap_or(0);
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
            Field::Interface => cfg.interface().label().into(),
            Field::FontFamily => cfg.font.family.clone(),
            Field::FontSize => format!("{} pt", trim_num(cfg.font.size)),
            Field::LineHeight => trim_num(cfg.font.line_height),
            Field::Padding => format!("{} px", cfg.window.padding.round()),
            Field::Opacity => format!("{} %", (cfg.window.opacity * 100.0).round()),
            Field::CursorStyle => cfg.terminal.cursor_style.label().into(),
            Field::Scrollback => format!("{} líneas", cfg.terminal.scrollback),
            Field::TabKey => match cfg.terminal.tab_key {
                TabKey::Menu => "Menú".into(),
                TabKey::Accept => "Sugerencia".into(),
            },
            Field::EditFile => "Abrir".into(),
            Field::Appearance => "Editar".into(),
            Field::TabsPosition => cfg.tabs.position.label().into(),
            Field::TabsWidth => cfg.tabs.width.label().into(),
            Field::TabsAlign => cfg.tabs.align.label().into(),
            Field::TabsWorking => cfg.tabs.working.label().into(),
            Field::RenameTab => "Esta pestaña".into(),
            Field::NotifyTest => "Enviar".into(),
            Field::NotifyAgents => match cfg.notifications.agents {
                NotifyWhen::Unfocused => "Si no lo veo".into(),
                NotifyWhen::Always => "Siempre".into(),
                NotifyWhen::Never => "Nunca".into(),
            },
            Field::NotifyMinWork => match cfg.notifications.min_work_secs {
                s if s >= 60 && s % 60 == 0 => format!("{} min", s / 60),
                s => format!("{s} s"),
            },
            Field::Version => env!("CARGO_PKG_VERSION").into(),
            _ => String::new(),
        }
    }

    fn toggle_value(f: Field, cfg: &Config) -> Option<bool> {
        Some(match f {
            Field::CursorBlink => cfg.terminal.cursor_blink,
            Field::CopyOnSelect => cfg.terminal.copy_on_select,
            Field::Autosuggest => cfg.terminal.autosuggest,
            Field::CompleteAsYouType => cfg.terminal.complete_as_you_type,
            Field::NotifySound => cfg.notifications.sound,
            Field::NotifyBlocked => cfg.notifications.blocked,
            Field::Blocks => cfg.terminal.blocks,
            Field::GarboPrompt => cfg.terminal.garbo_prompt,
            Field::SshIntegration => cfg.terminal.ssh_integration,
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
            let pos = list
                .iter()
                .position(|x| x.eq_ignore_ascii_case(cur))
                .map_or(0, |p| p as i32);
            list[(pos + d).rem_euclid(n.max(1)) as usize].clone()
        };
        match f {
            Field::Theme => {
                let names: Vec<String> = Theme::names().iter().map(|s| s.to_string()).collect();
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
                cfg.font.line_height =
                    ((cfg.font.line_height + 0.05 * d as f32) * 100.0).round() / 100.0;
                cfg.font.line_height = cfg.font.line_height.clamp(0.8, 2.0);
                Change::LineHeight
            }
            Field::Padding => {
                cfg.window.padding = (cfg.window.padding + 2.0 * d as f32).clamp(0.0, 40.0);
                Change::Layout
            }
            Field::Opacity => {
                cfg.window.opacity =
                    ((cfg.window.opacity + 0.05 * d as f32) * 100.0).round() / 100.0;
                cfg.window.opacity = cfg.window.opacity.clamp(0.3, 1.0);
                Change::Redraw
            }
            Field::CursorStyle => {
                cfg.terminal.cursor_style =
                    cycle_opt(&CursorStyle::ALL, cfg.terminal.cursor_style, d);
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
            Field::TabKey => {
                cfg.terminal.tab_key = match cfg.terminal.tab_key {
                    TabKey::Menu => TabKey::Accept,
                    TabKey::Accept => TabKey::Menu,
                };
                Change::Redraw
            }
            Field::CompleteAsYouType => {
                cfg.terminal.complete_as_you_type = !cfg.terminal.complete_as_you_type;
                Change::Redraw
            }
            Field::Blocks => {
                cfg.terminal.blocks = !cfg.terminal.blocks;
                Change::Redraw
            }
            Field::GarboPrompt => {
                cfg.terminal.garbo_prompt = !cfg.terminal.garbo_prompt;
                self.tabs_note = true;
                Change::Redraw
            }
            Field::SshIntegration => {
                cfg.terminal.ssh_integration = !cfg.terminal.ssh_integration;
                self.tabs_note = true;
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
            Field::TabsPosition => {
                use crate::tabs::Position;
                cfg.tabs.position = cycle_opt(Position::ALL, cfg.tabs.position, d);
                Change::Layout
            }
            Field::TabsWidth => {
                use crate::tabs::Width;
                cfg.tabs.width = cycle_opt(Width::ALL, cfg.tabs.width, d);
                Change::Layout
            }
            Field::TabsAlign => {
                use crate::tabs::Align;
                cfg.tabs.align = cycle_opt(Align::ALL, cfg.tabs.align, d);
                Change::Redraw
            }
            Field::TabsWorking => {
                use crate::tabs::Working;
                cfg.tabs.working = cycle_opt(Working::ALL, cfg.tabs.working, d);
                Change::Redraw
            }
            Field::RenameTab if dir == 0 => Change::RenameTab,
            Field::RenameTab => Change::None,
            Field::Appearance if dir == 0 => Change::OpenLook,
            Field::Appearance => Change::None,
            // En Windows, de momento, siempre la Clásica.
            Field::Interface if cfg!(windows) => Change::None,
            Field::Interface => {
                use crate::style::Interface;
                let v = cycle_opt(Interface::ALL, cfg.interface(), d);
                cfg.style.interface = (v != Interface::default_here()).then_some(v);
                Change::Style
            }
            Field::NotifyAgents => {
                let order = [NotifyWhen::Unfocused, NotifyWhen::Always, NotifyWhen::Never];
                let pos = order
                    .iter()
                    .position(|w| *w == cfg.notifications.agents)
                    .unwrap_or(0) as i32;
                cfg.notifications.agents = order[(pos + d).rem_euclid(3) as usize];
                Change::Redraw
            }
            Field::NotifyMinWork => {
                const STEPS: [u64; 6] = [0, 5, 10, 30, 60, 300];
                let cur = cfg.notifications.min_work_secs;
                let pos = STEPS
                    .iter()
                    .position(|s| *s >= cur)
                    .unwrap_or(STEPS.len() - 1) as i32;
                cfg.notifications.min_work_secs =
                    STEPS[(pos + d).clamp(0, STEPS.len() as i32 - 1) as usize];
                Change::Redraw
            }
            Field::NotifySound => {
                cfg.notifications.sound = !cfg.notifications.sound;
                Change::Redraw
            }
            Field::NotifyBlocked => {
                cfg.notifications.blocked = !cfg.notifications.blocked;
                Change::Redraw
            }
            // Como el de actualizaciones: solo con Intro o clic.
            Field::NotifyTest if dir == 0 => Change::TestNotice,
            Field::NotifyTest => Change::None,
            // El botón solo responde a Intro o clic, no a ← →.
            Field::CheckNow if dir == 0 => Change::CheckNow,
            Field::CheckNow => Change::None,
            Field::CheckUpdates => {
                cfg.check_updates = !cfg.check_updates;
                Change::Updates
            }
            Field::Version | Field::Shortcuts => Change::None,
        }
    }

    fn layout(&mut self, width: f32, height: f32, top: f32, s: f32) -> Layout {
        let w = (640.0 * s).min(width - 32.0 * s).max(300.0 * s).round();
        let h = (540.0 * s)
            .min(height - top - 32.0 * s)
            .max(240.0 * s)
            .round();
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
        let cats = (0..CATEGORIES.len())
            .map(|c| (c, y + (64.0 * s).round() + c as f32 * cat_h, cat_h))
            .collect();

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
        // Atajos: filas más bajas que las de los ajustes; los títulos, con aire encima.
        let key_h = (30.0 * s).round();
        let heading_h = (38.0 * s).round();
        let (a, b) = cat_range(self.cat);
        let its: Vec<Item> = (a..b)
            .flat_map(|i| match self.field(i) {
                Field::Theme => (0..Theme::names().len().div_ceil(cols))
                    .map(Item::Themes)
                    .collect::<Vec<_>>(),
                Field::Shortcuts => {
                    let mut v = vec![Item::Note];
                    for (si, sec) in self.shortcuts.iter().enumerate() {
                        v.push(Item::Heading(si));
                        v.extend((0..sec.rows.len()).map(|ri| Item::Key(si, ri)));
                    }
                    v
                }
                _ => vec![Item::Row(i)],
            })
            .collect();
        let heights: Vec<f32> = its
            .iter()
            .map(|it| match it {
                Item::Themes(_) => card_row_h,
                Item::Heading(_) => heading_h,
                Item::Note | Item::Key(..) => key_h,
                Item::Row(_) => row_h,
            })
            .collect();

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
            let on_theme = self.field(self.selected) == Field::Theme;
            let sel = its
                .iter()
                .position(|it| match *it {
                    Item::Themes(r) => on_theme && r == self.theme_row(cols),
                    Item::Row(i) => i == self.selected,
                    _ => false,
                })
                .unwrap_or(0)
                .min(its.len().saturating_sub(1));
            first = first.min(sel);
            while first < max_first.min(sel) && heights[first..=sel].iter().sum::<f32>() > body_h {
                first += 1;
            }
        }
        self.first = first;

        let mut rows = Vec::new();
        let mut cards = Vec::new();
        let mut keys = Vec::new();
        let mut cy = body_y;
        for (k, it) in its.iter().enumerate().skip(first) {
            let ih = heights[k];
            if cy + ih > body_y + body_h + 0.5 {
                break;
            }
            match *it {
                Item::Row(ri) => rows.push((ri, cy, ih)),
                Item::Note | Item::Heading(_) | Item::Key(..) => keys.push((*it, cy, ih)),
                Item::Themes(r) => {
                    for c in 0..cols {
                        let ti = r * cols + c;
                        if ti >= Theme::names().len() {
                            break;
                        }
                        cards.push((
                            ti,
                            pane_x + pad + c as f32 * (card_w + gap),
                            cy + 4.0 * s,
                            card_w,
                            card_h,
                        ));
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
            keys,
        }
    }

    /// Un elemento de Ajustes → Atajos entre `left` y `right`: la nota, el título de una sección o
    /// una acción con sus teclas como etiquetas a la derecha.
    #[allow(clippy::too_many_arguments)]
    fn draw_shortcut(
        &self,
        p: &mut Painter,
        theme: &Theme,
        it: Item,
        left: f32,
        right: f32,
        y: f32,
        h: f32,
        s: f32,
    ) {
        let dimc = dim(theme);
        let small_h = p.ui_line_h(UiVariant::Small);
        match it {
            Item::Note => {
                let t = "Se cambian en [keybindings] de config.toml";
                let ty = (y + (h - small_h) / 2.0).round();
                p.ui_text_v(left, ty, t, dimc, right - left, UiVariant::Small);
            }
            Item::Heading(si) => {
                let Some(sec) = self.shortcuts.get(si) else {
                    return;
                };
                // Pegado a sus filas: el aire queda encima.
                let ty = (y + h - small_h - 4.0 * s).round();
                let t = sec.title.to_uppercase();
                p.ui_text_v(left, ty, &t, dimc, right - left, UiVariant::Small);
            }
            Item::Key(si, ri) => {
                let Some(row) = self.shortcuts.get(si).and_then(|sec| sec.rows.get(ri)) else {
                    return;
                };
                let mid = y + h / 2.0;
                let ty = (mid - small_h / 2.0).round();
                // Teclas a la derecha, dejando sitio al nombre; si no caben todas, las primeras.
                let min_label = (110.0 * s).round();
                let (pad, gap) = ((7.0 * s).round(), (6.0 * s).round());
                let chip_h = (small_h + 6.0 * s).round();
                let mut x = right;
                if row.keys.is_empty() {
                    let t = "Sin atajo";
                    let tw = p.ui_measure_v(t, UiVariant::Small);
                    x = right - tw;
                    p.ui_text_v(x, ty, t, dimc, tw + 2.0, UiVariant::Small);
                }
                let widths: Vec<f32> = row
                    .keys
                    .iter()
                    .map(|k| (p.ui_measure_v(k, UiVariant::Small) + 2.0 * pad).round())
                    .collect();
                let n = keys_that_fit(&widths, right - left - min_label, gap);
                for (k, cw) in row.keys[..n].iter().zip(&widths).rev() {
                    x -= cw;
                    let cy = (mid - chip_h / 2.0).round();
                    p.rounded(
                        x,
                        cy,
                        *cw,
                        chip_h,
                        theme.foreground,
                        0.08,
                        (5.0 * s).round(),
                    );
                    let tw = cw - 2.0 * pad;
                    p.ui_text_v(x + pad, ty, k, theme.foreground, tw + 2.0, UiVariant::Small);
                    x -= gap;
                }
                let max_w = (x - 12.0 * s - left).max(0.0);
                p.ui_text_in(left, y, h, &row.action, theme.foreground, max_w);
            }
            Item::Row(_) | Item::Themes(_) => {}
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
                    return if mx < left + l.control_w / 2.0 {
                        Hit::Dec(i)
                    } else {
                        Hit::Inc(i)
                    };
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
        self.theme_idx = Theme::names()
            .iter()
            .position(|n| n.eq_ignore_ascii_case(&cfg.theme))
            .unwrap_or(0);
        self.refresh_shortcuts(cfg);
        let l = self.layout(width, height, top, s);
        // Interfaz Flotante: hoja más redondeada, fondo algo más oscuro e icono por categoría.
        let chrome = crate::chrome::Chrome::new(&theme.style, s);
        let dim_bg = if chrome.floating { chrome.dim } else { 0.40 };
        p.rect(0.0, 0.0, width, height, Rgb(0, 0, 0), dim_bg);
        let radius = if chrome.floating {
            chrome.sheet
        } else {
            (14.0 * s).round()
        };
        let panel = surface(theme, 0.05);
        p.card(l.x, l.y, l.w, l.h, panel, theme.foreground, radius, s);
        let dimc = dim(theme);
        let pad = (20.0 * s).round();
        let hair = s.round().max(1.0);

        // --- Columna de categorías
        let side = surface(theme, 0.025);
        p.rounded(
            l.x + hair,
            l.y + hair,
            l.side_w,
            l.h - 2.0 * hair,
            side,
            1.0,
            radius - hair,
        );
        p.rect(
            l.x + l.side_w - radius,
            l.y + hair,
            radius,
            l.h - 2.0 * hair,
            side,
            1.0,
        );
        p.rect(
            l.x + l.side_w,
            l.y + hair,
            hair,
            l.h - 2.0 * hair,
            theme.foreground,
            0.08,
        );
        let th = p.ui_line_h(UiVariant::Title);
        p.ui_text_v(
            l.x + pad,
            l.y + ((60.0 * s - th) / 2.0).round(),
            "Ajustes",
            theme.foreground,
            l.side_w - 2.0 * pad,
            UiVariant::Title,
        );
        for &(c, cy, ch) in &l.cats {
            let selected = c == self.cat;
            let hovered = self.hover == Some(Hit::Category(c));
            let (bx, bw) = (l.x + 10.0 * s, l.side_w - 20.0 * s);
            if selected {
                p.rounded(
                    bx,
                    cy + 2.0 * s,
                    bw,
                    ch - 4.0 * s,
                    theme.accent,
                    0.18,
                    (8.0 * s).round(),
                );
            } else if hovered {
                p.rounded(
                    bx,
                    cy + 2.0 * s,
                    bw,
                    ch - 4.0 * s,
                    theme.foreground,
                    0.06,
                    (8.0 * s).round(),
                );
            }
            let c_txt = if selected || hovered {
                theme.foreground
            } else {
                dimc
            };
            if chrome.floating {
                let ic = if selected { theme.accent } else { c_txt };
                category_icon(p, CATEGORIES[c].0, bx + 20.0 * s, cy + ch / 2.0, s, ic);
                let tx = bx + 38.0 * s;
                p.ui_text_in(tx, cy, ch, CATEGORIES[c].0, c_txt, bx + bw - 4.0 * s - tx);
            } else {
                p.ui_text_in(bx + 12.0 * s, cy, ch, CATEGORIES[c].0, c_txt, bw - 16.0 * s);
            }
        }

        // --- Cabecera de la derecha
        let title = CATEGORIES[self.cat].0;
        p.ui_text_v(
            l.pane_x + pad,
            l.y + ((l.header_h - th) / 2.0).round(),
            title,
            theme.foreground,
            l.pane_w / 2.0,
            UiVariant::Title,
        );
        let (cx, cy, r) = l.close;
        if self.hover == Some(Hit::Close) {
            p.circle(cx, cy, r, theme.foreground, 0.12);
        }
        cross(p, cx, cy, 4.0 * s, 1.5 * s, theme.foreground);

        // --- Tarjetas de temas
        for &(ti, tx, ty, tw, tch) in &l.cards {
            let name = Theme::names()[ti];
            let selected = cfg.theme.eq_ignore_ascii_case(name);
            let hovered = self.hover == Some(Hit::Theme(ti));
            if let Some(t) = Theme::builtin(name) {
                theme_card(p, &t, theme, tx, ty, tw, tch, selected, hovered, s);
            }
            let label = Theme::display_name(name);
            let c = if selected || hovered {
                theme.foreground
            } else {
                dimc
            };
            let nh = p.ui_line_h(UiVariant::Small);
            let ny = (ty + tch + (l.name_h - nh) / 2.0).round();
            p.ui_text_v(tx + 2.0 * s, ny, &label, c, tw - 4.0 * s, UiVariant::Small);
        }

        // --- Filas
        for &(i, ry, rh) in &l.rows {
            let f = self.field(i);
            let hovered =
                matches!(self.hover, Some(Hit::Row(j) | Hit::Dec(j) | Hit::Inc(j)) if j == i);
            let (bx, bw) = (l.pane_x + pad / 2.0, l.pane_w - pad);
            if i == self.selected || hovered {
                let a = if i == self.selected { 0.12 } else { 0.06 };
                p.rounded(
                    bx,
                    ry + 3.0 * s,
                    bw,
                    rh - 6.0 * s,
                    theme.accent,
                    a,
                    (8.0 * s).round(),
                );
            }
            let lx = l.pane_x + pad;
            // Nota bajo el nombre (estado de la búsqueda de actualizaciones o aviso de reinicio).
            let note = match f {
                Field::CheckNow => update_button(upd).3,
                Field::Decorations if self.restart_note => "Se aplica al reiniciar".into(),
                Field::GarboPrompt | Field::SshIntegration if self.tabs_note => {
                    "Se aplica a las pestañas nuevas".into()
                }
                Field::SshIntegration => "Bloques, prompt y agentes en el servidor".into(),
                Field::Appearance => "Pestañas, prompt, bloques, botones…".into(),
                Field::Interface => "Paneles, menús y barra".into(),
                Field::TabsPosition => "Arriba, abajo o a un lado".into(),
                Field::TabsWidth if cfg.tabs.position.vertical() => {
                    "Ancho de la columna de pestañas".into()
                }
                Field::TabsWidth => "Lo más que mide cada pestaña".into(),
                Field::TabsWorking => "Animación mientras trabaja".into(),
                Field::RenameTab => "O doble clic en la pestaña".into(),
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
                    (
                        Field::CheckNow,
                        _,
                        Some(UpdateState::Available(_) | UpdateState::Installed(_)),
                    ) => theme.accent,
                    (Field::CheckNow, _, Some(UpdateState::Failed(_)))
                    | (Field::CheckNow, CheckState::Failed(_), None) => theme.palette[1],
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
            } else if matches!(
                f,
                Field::EditFile | Field::NotifyTest | Field::Appearance | Field::RenameTab
            ) {
                let t = &self.value_text(f, cfg);
                let tw = p.ui_measure(t);
                p.ui_text_in(right - tw - 12.0 * s, ry, rh, t, theme.accent, tw + 2.0);
                let ax = right - 4.0 * s;
                p.line(
                    ax - 3.0 * s,
                    mid - 4.0 * s,
                    ax + 1.0 * s,
                    mid,
                    1.5 * s,
                    theme.accent,
                    1.0,
                );
                p.line(
                    ax + 1.0 * s,
                    mid,
                    ax - 3.0 * s,
                    mid + 4.0 * s,
                    1.5 * s,
                    theme.accent,
                    1.0,
                );
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
                    (true, _) => (
                        theme.accent,
                        if over { 1.0 } else { 0.88 },
                        theme.background,
                    ),
                    (false, true) => (
                        theme.foreground,
                        if over { 0.16 } else { 0.09 },
                        theme.foreground,
                    ),
                    (false, false) => (theme.foreground, 0.05, dimc),
                };
                p.rounded(bx, by, bwid, bh, fill, alpha, (bh / 2.0).round());
                p.ui_text_in(bx + bpad, by, bh, &label, txt, tw + 1.0);
            } else {
                stepper(p, theme, self, i, f, cfg, right, ry, rh, l.control_w, s);
            }
        }

        // --- Atajos
        for &(it, ry, rh) in &l.keys {
            self.draw_shortcut(
                p,
                theme,
                it,
                l.pane_x + pad,
                l.pane_x + l.pane_w - pad,
                ry,
                rh,
                s,
            );
        }

        if let Some((off, size)) = l.scrollbar {
            let bw = (4.0 * s).round();
            p.rounded(
                l.x + l.w - bw - 5.0 * s,
                l.body_y + off + 2.0 * s,
                bw,
                size - 4.0 * s,
                theme.foreground,
                0.22,
                bw / 2.0,
            );
        }

        // Pie
        let fy = l.body_y + l.body_h;
        let path = Config::path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let home = std::env::var("HOME").unwrap_or_default();
        let short = if !home.is_empty() {
            path.replacen(&home, "~", 1)
        } else {
            path
        };
        let nh = p.ui_line_h(UiVariant::Small);
        let (message, color) = if self.save_failed {
            (
                "No se pudieron guardar los cambios en config.toml".to_owned(),
                theme.palette[1],
            )
        } else {
            (format!("Se guarda solo en {short}"), dimc)
        };
        p.ui_text_v(
            l.pane_x + pad,
            (fy + (l.y + l.h - fy - nh) / 2.0).round(),
            &message,
            color,
            l.pane_w - 2.0 * pad,
            UiVariant::Small,
        );
    }
}

/// Texto del botón de actualizaciones: (etiqueta, principal, activo, nota de estado).
fn update_button(upd: &UpdateView) -> (String, bool, bool, String) {
    match upd.state {
        Some(UpdateState::Available(rel)) => (
            rel.button_label(),
            true,
            true,
            "Hay una versión nueva".into(),
        ),
        Some(UpdateState::Downloading(_)) => ("Descargando…".into(), false, false, String::new()),
        Some(UpdateState::Installing(_)) => ("Instalando…".into(), false, false, String::new()),
        Some(UpdateState::Installed(v)) => {
            ("Reiniciar".into(), true, true, format!("Instalada la {v}"))
        }
        Some(UpdateState::Failed(_)) => (
            "Reintentar".into(),
            false,
            true,
            "No se pudo actualizar".into(),
        ),
        None => match upd.check {
            CheckState::Checking => ("Buscando…".into(), false, false, String::new()),
            CheckState::UpToDate => ("Buscar".into(), false, true, "Estás al día".into()),
            CheckState::Failed(_) => ("Buscar".into(), false, true, "Sin conexión".into()),
            CheckState::Idle => ("Buscar".into(), false, true, String::new()),
        },
    }
}

/// Opciones con selector ‹ valor ›.
/// Cuántas teclas de una fila de Atajos caben en `avail` px, empezando por la primera (la
/// principal), con `gap` entre una y otra.
fn keys_that_fit(widths: &[f32], avail: f32, gap: f32) -> usize {
    let mut used = 0.0;
    for (i, w) in widths.iter().enumerate() {
        used += w + if i > 0 { gap } else { 0.0 };
        if used > avail {
            return i;
        }
    }
    widths.len()
}

/// Icono de una categoría (interfaz Flotante), centrado en (`cx`, `cy`), de unos 13 px lógicos.
fn category_icon(p: &mut Painter, name: &str, cx: f32, cy: f32, s: f32, c: Rgb) {
    let t = (1.4 * s).max(1.0);
    let k = |v: f32| v * s;
    let line = |p: &mut Painter, a: (f32, f32), b: (f32, f32)| {
        p.line(
            cx + k(a.0),
            cy + k(a.1),
            cx + k(b.0),
            cy + k(b.1),
            t,
            c,
            1.0,
        );
    };
    let frame = |p: &mut Painter, w: f32, h: f32, r: f32| {
        p.outline(
            cx - k(w / 2.0),
            cy - k(h / 2.0),
            k(w),
            k(h),
            c,
            1.0,
            k(r),
            t,
        );
    };
    match name {
        "Tema" => {
            p.ring(cx, cy, k(5.5), t, c, 1.0);
            line(p, (0.0, -5.5), (0.0, 5.5));
        }
        "Pestañas" => {
            frame(p, 13.0, 10.0, 2.0);
            line(p, (-6.5, -1.5), (6.5, -1.5));
        }
        "Texto" => {
            line(p, (-4.5, 5.5), (0.0, -5.5));
            line(p, (0.0, -5.5), (4.5, 5.5));
            line(p, (-2.5, 1.5), (2.5, 1.5));
        }
        "Terminal" => {
            frame(p, 13.0, 11.0, 2.0);
            line(p, (-3.5, -2.0), (-1.0, 0.0));
            line(p, (-1.0, 0.0), (-3.5, 2.0));
            line(p, (0.5, 2.5), (3.5, 2.5));
        }
        "Completado" => {
            line(p, (-5.5, -4.0), (5.5, -4.0));
            line(p, (-5.5, 0.0), (1.0, 0.0));
            line(p, (-5.5, 4.0), (3.5, 4.0));
        }
        "Bloques" => {
            p.outline(cx - k(6.0), cy - k(5.5), k(12.0), k(4.5), c, 1.0, k(1.5), t);
            p.outline(cx - k(6.0), cy + k(1.0), k(12.0), k(4.5), c, 1.0, k(1.5), t);
        }
        "Avisos" => {
            p.outline(cx - k(4.0), cy - k(5.5), k(8.0), k(9.0), c, 1.0, k(4.0), t);
            line(p, (-5.5, 3.5), (5.5, 3.5));
            line(p, (-1.5, 6.0), (1.5, 6.0));
        }
        "Ventana" => {
            frame(p, 13.0, 11.0, 2.0);
            line(p, (-6.5, -2.0), (6.5, -2.0));
        }
        "Atajos" => {
            frame(p, 14.0, 9.0, 2.0);
            for dx in [-3.5, -1.0, 1.5, 4.0] {
                p.circle(cx + k(dx) - k(0.25), cy - k(1.0), k(0.9), c, 1.0);
            }
            line(p, (-3.0, 2.0), (3.0, 2.0));
        }
        "Actualizaciones" => {
            line(p, (0.0, -5.5), (0.0, 2.5));
            line(p, (-3.5, -1.0), (0.0, 2.5));
            line(p, (0.0, 2.5), (3.5, -1.0));
            line(p, (-5.5, 5.5), (5.5, 5.5));
        }
        _ => p.circle(cx, cy, k(2.5), c, 1.0),
    }
}

fn has_stepper(f: Field) -> bool {
    matches!(
        f,
        Field::Interface
            | Field::FontFamily
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
    let kx = if on {
        tx + tw - th / 2.0
    } else {
        tx + th / 2.0
    };
    p.circle(kx, mid, kr, Rgb(255, 255, 255), 0.95);
}

/// Selector ‹ valor ›
#[allow(clippy::too_many_arguments)]
fn stepper(
    p: &mut Painter,
    theme: &Theme,
    set: &Settings,
    i: usize,
    f: Field,
    cfg: &Config,
    right: f32,
    ry: f32,
    rh: f32,
    cw: f32,
    s: f32,
) {
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
        p.line(
            hx + dir * -k * 0.5,
            mid - k,
            hx + dir * k * 0.5,
            mid,
            1.6 * s,
            c,
            1.0,
        );
        p.line(
            hx + dir * k * 0.5,
            mid,
            hx + dir * -k * 0.5,
            mid + k,
            1.6 * s,
            c,
            1.0,
        );
    }
    let value = set.value_text(f, cfg);
    let inner = cw - 56.0 * s;
    let vw = p.ui_measure(&value).min(inner);
    let vx = left + (cw - vw) / 2.0;
    p.ui_text_in(vx.round(), ry, rh, &value, theme.foreground, inner + 1.0);
}

/// Muestra de un tema: un terminal en miniatura con sus colores.
#[allow(clippy::too_many_arguments)]
pub(crate) fn theme_card(
    p: &mut Painter,
    t: &Theme,
    ui: &Theme,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    selected: bool,
    hovered: bool,
    s: f32,
) {
    use UiVariant::Small;
    let r = (8.0 * s).round();
    let hair = s.round().max(1.0);
    if selected {
        let b = (2.0 * s).round();
        p.rounded(
            x - b,
            y - b,
            w + 2.0 * b,
            h + 2.0 * b,
            ui.accent,
            1.0,
            r + b,
        );
    } else {
        let a = if hovered { 0.40 } else { 0.14 };
        p.rounded(
            x - hair,
            y - hair,
            w + 2.0 * hair,
            h + 2.0 * hair,
            ui.foreground,
            a,
            r + hair,
        );
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
    for (word, c) in [
        ("src", t.palette[4]),
        ("docs", t.palette[4]),
        ("main.rs", t.foreground),
    ] {
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
        p.rounded(
            tx + k as f32 * (sw + sgap),
            sy,
            sw,
            sw_h,
            t.palette[k + 1],
            1.0,
            sw_h / 2.0,
        );
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
        let dir = crate::temp_dir::TempDir::new_in(&std::env::temp_dir()).unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "# mi comentario\n[shell]\nprogram = \"/bin/zsh\"\n").unwrap();

        let mut cfg = Config::load_at(&path);
        let mut set = Settings::with_fonts(vec![]);
        assert_eq!(set.adjust(idx(Field::Theme), 1, &mut cfg), Change::Theme);
        assert_eq!(cfg.theme, Theme::names()[1]);
        set.adjust(idx(Field::FontSize), 1, &mut cfg);
        set.adjust(idx(Field::Blocks), 0, &mut cfg);
        set.adjust(idx(Field::Autosuggest), 0, &mut cfg);
        set.adjust(idx(Field::TabKey), 1, &mut cfg);
        cfg.save_at(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# mi comentario"), "{text}");
        assert!(text.contains("program = \"/bin/zsh\""), "{text}");
        let back = Config::load_at(&path);
        assert_eq!(back.theme, Theme::names()[1]);
        assert_eq!(back.font.size, Config::default().font.size + 0.5);
        assert!(!back.terminal.blocks);
        assert!(!back.terminal.autosuggest);
        assert_eq!(back.terminal.tab_key, TabKey::Accept);
        assert!(!back.terminal.complete_as_you_type);
        assert_eq!(back.shell.program.as_deref(), Some("/bin/zsh"));
    }

    #[test]
    fn opciones_de_las_pestanas() {
        use crate::tabs::{Align, Position, Width};
        let mut cfg = Config::default();
        let mut set = Settings::with_fonts(vec![]);
        assert_eq!(
            set.adjust(idx(Field::TabsPosition), 1, &mut cfg),
            Change::Layout
        );
        assert_eq!(cfg.tabs.position, Position::Bottom);
        set.adjust(idx(Field::TabsPosition), -1, &mut cfg);
        set.adjust(idx(Field::TabsPosition), -1, &mut cfg);
        assert_eq!(cfg.tabs.position, Position::Right, "da la vuelta");
        assert_eq!(set.value_text(Field::TabsPosition, &cfg), "Derecha");
        assert_eq!(
            set.adjust(idx(Field::TabsWidth), 1, &mut cfg),
            Change::Layout
        );
        assert_eq!(cfg.tabs.width, Width::Wide);
        assert_eq!(
            set.adjust(idx(Field::TabsAlign), 1, &mut cfg),
            Change::Redraw
        );
        assert_eq!(cfg.tabs.align, Align::Center);
        assert_eq!(
            set.adjust(idx(Field::TabsWorking), 1, &mut cfg),
            Change::Redraw
        );
        assert_eq!(cfg.tabs.working, crate::tabs::Working::Wave);
        assert_eq!(set.value_text(Field::TabsWorking, &cfg), "Onda");
        // "Cambiar el nombre" es un botón: solo con Intro o clic.
        assert_eq!(set.adjust(idx(Field::RenameTab), 1, &mut cfg), Change::None);
        assert_eq!(
            set.adjust(idx(Field::RenameTab), 0, &mut cfg),
            Change::RenameTab
        );
    }

    #[test]
    fn categorias_y_teclado() {
        let mut cfg = Config::default();
        let mut set = Settings::with_fonts(vec![]);
        assert_eq!(CATEGORIES[set.cat].0, "Tema");
        // Tab recorre las categorías y elige su primera opción.
        set.nav(Nav::NextCategory, &mut cfg);
        assert_eq!(set.field(set.selected), Field::TabsPosition);
        set.nav(Nav::PrevCategory, &mut cfg);
        set.nav(Nav::PrevCategory, &mut cfg);
        assert_eq!(CATEGORIES[set.cat].0, "Actualizaciones");
        assert_eq!(set.field(set.selected), Field::CheckNow);
        set.nav(Nav::PrevCategory, &mut cfg);
        assert_eq!(CATEGORIES[set.cat].0, "Atajos");
        assert_eq!(set.field(set.selected), Field::Shortcuts);
        set.nav(Nav::NextCategory, &mut cfg);
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
        set.selected = idx(Field::Theme);
        set.layout(1200.0, 900.0, 40.0, 1.0);
        let cols = set.cols;
        assert_eq!(set.nav(Nav::Down, &mut cfg), Change::Theme);
        assert_eq!(cfg.theme, Theme::names()[cols]);
        assert_eq!(set.nav(Nav::Right, &mut cfg), Change::Theme);
        assert_eq!(cfg.theme, Theme::names()[cols + 1]);
    }

    #[test]
    fn interfaz_en_ajustes() {
        use crate::style::Interface;
        let mut cfg = Config::default();
        let set = &mut Settings::with_fonts(vec![]);
        // La primera opción de Tema.
        assert_eq!(set.field(set.selected), Field::Interface);
        let ch = set.adjust(idx(Field::Interface), 1, &mut cfg);
        if cfg!(windows) {
            assert_eq!(ch, Change::None);
            return;
        }
        assert_eq!(ch, Change::Style);
        assert_eq!(cfg.interface(), Interface::Classic);
        assert_eq!(cfg.style.interface, Some(Interface::Classic));
        // Volver a la de serie no deja nada escrito.
        set.adjust(idx(Field::Interface), 1, &mut cfg);
        assert_eq!(cfg.style.interface, None);
        assert_eq!(cfg.interface(), Interface::Floating);
    }

    #[test]
    fn caben_las_primeras_teclas() {
        assert_eq!(keys_that_fit(&[50.0, 60.0], 200.0, 6.0), 2);
        assert_eq!(keys_that_fit(&[50.0, 60.0], 116.0, 6.0), 2);
        // Si no caben todas, se quedan las primeras, no las últimas.
        assert_eq!(keys_that_fit(&[50.0, 60.0], 115.0, 6.0), 1);
        assert_eq!(keys_that_fit(&[50.0, 60.0], 40.0, 6.0), 0);
        assert_eq!(keys_that_fit(&[], 40.0, 6.0), 0);
    }

    #[test]
    fn atajos_se_desplazan() {
        let mut cfg = Config::default();
        let mut set = Settings::with_fonts(vec![]);
        let atajos = CATEGORIES.iter().position(|c| c.0 == "Atajos").unwrap();
        set.set_cat(atajos);
        set.refresh_shortcuts(&cfg);
        let (sec, row) = (
            set.shortcuts.len() - 1,
            set.shortcuts.last().unwrap().rows.len() - 1,
        );
        let last = |l: &Layout| {
            l.keys
                .iter()
                .any(|(it, ..)| matches!(*it, Item::Key(s, r) if s == sec && r == row))
        };
        // Ventana baja: arriba del todo no se ve la última fila.
        let l = set.layout(1200.0, 400.0, 40.0, 1.0);
        assert_eq!(set.first, 0);
        assert!(matches!(l.keys.first(), Some((Item::Note, ..))));
        assert!(!last(&l));
        // ↓ baja hasta que se ve la última, sin pasarse.
        for _ in 0..200 {
            set.nav(Nav::Down, &mut cfg);
            set.layout(1200.0, 400.0, 40.0, 1.0);
        }
        let l = set.layout(1200.0, 400.0, 40.0, 1.0);
        assert!(set.first > 0);
        assert!(last(&l));
        // ↑ vuelve arriba. Intro y ← → no hacen nada.
        for _ in 0..200 {
            set.nav(Nav::Up, &mut cfg);
            set.layout(1200.0, 400.0, 40.0, 1.0);
        }
        assert_eq!(set.first, 0);
        assert_eq!(set.nav(Nav::Activate, &mut cfg), Change::None);
        assert_eq!(set.nav(Nav::Right, &mut cfg), Change::None);
        assert_eq!(CATEGORIES[set.cat].0, "Atajos");
    }

    #[test]
    fn rueda_desplaza_sin_cambiar_la_seleccion() {
        let mut set = Settings::with_fonts(vec![]);
        let terminal = CATEGORIES.iter().position(|c| c.0 == "Terminal").unwrap();
        set.set_cat(terminal); // 4 filas
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
        assert!(
            l.rows.iter().any(|r| set.field(r.0) == Field::Scrollback),
            "se llega a la última fila"
        );
        set.wheel(100_000.0);
        set.layout(w, h, top, s);
        assert_eq!(set.first, 0);
        set.move_sel(10);
        let l = set.layout(w, h, top, s);
        assert!(
            l.rows.iter().any(|r| r.0 == set.selected),
            "el teclado muestra la fila elegida"
        );
    }

    #[test]
    fn elegir_tema_de_la_cuadricula() {
        let mut cfg = Config::default();
        let mut set = Settings::with_fonts(vec![]);
        set.set_cat(3);
        assert_eq!(set.pick_theme(4, &mut cfg), Change::Theme);
        assert_eq!(cfg.theme, Theme::names()[4]);
        assert_eq!(set.field(set.selected), Field::Theme);
        assert_eq!(set.cat, 0);
        assert_eq!(set.pick_theme(4, &mut cfg), Change::None);
        assert_eq!(set.pick_theme(99, &mut cfg), Change::None);
        let l = set.layout(1200.0, 1400.0, 40.0, 1.0);
        assert_eq!(l.cards.first().map(|c| c.0), Some(0));
        // Desplazando se llega a la última tarjeta.
        set.wheel(-100_000.0);
        let l = set.layout(1200.0, 1400.0, 40.0, 1.0);
        assert_eq!(l.cards.last().map(|c| c.0), Some(Theme::names().len() - 1));
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
