//! Estado de la aplicación: ventana, pestañas, acciones.

use crate::atlas::GlyphCache;
use crate::blocks;
use crate::config::{Config, Theme};
use crate::font::FontSet;
use crate::keybindings::{Action, Keybindings};
use crate::palette::{Item, Palette};
use crate::renderer::{Instance, Renderer};
use crate::terminal::{term_config, Tab, UserEvent};
use crate::ui::{self, TabBar, TabHit};
use crate::view::{self, Painter, Rows, TermView};
use alacritty_terminal::event::WindowSize;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::TermMode;
use copypasta::ClipboardProvider;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::event_loop::EventLoopProxy;
use winit::keyboard::ModifiersState;
use winit::window::Window;

/// Tras arrancar, cuándo devolver al sistema la memoria de la inicialización (ver
/// `platform::trim_memory`): ya hay ventana, fuentes, shaders y el primer prompt.
const TRIM_AFTER_START: Duration = Duration::from_secs(4);
/// Tras cerrar una pestaña o un panel.
const TRIM_AFTER_CLOSE: Duration = Duration::from_secs(2);

pub fn pt_to_px(pt: f32, scale: f32) -> f32 {
    pt * scale * 4.0 / 3.0
}

/// Fuentes del terminal y de la interfaz con un único escaneo del sistema.
pub fn build_glyphs(cfg: &Config, font_pt: f32, scale: f32) -> GlyphCache {
    let db = FontSet::system_db();
    let f = &cfg.font;
    let term = FontSet::from_db(
        &db,
        &f.family,
        &f.fallback,
        pt_to_px(font_pt, scale),
        f.line_height,
        true,
    );
    let ui_px = pt_to_px(f.ui_size, scale);
    let ui = match cfg.ui_font() {
        Some((family, fallback)) => FontSet::from_db(&db, &family, &fallback, ui_px, 1.0, false),
        None => FontSet::from_db(&db, &f.family, &f.fallback, ui_px, 1.0, true),
    };
    GlyphCache::new(term, ui)
}

#[derive(Clone, Copy)]
pub enum BlockPart {
    Output,
    Command,
    Whole,
}

pub struct App {
    pub cfg: Config,
    pub theme: Theme,
    pub keys: Keybindings,
    pub proxy: EventLoopProxy<UserEvent>,
    pub st: Option<State>,
}

pub struct State {
    pub window: Arc<Window>,
    pub renderer: Renderer,
    pub glyphs: GlyphCache,
    /// Terminales (uno por panel). `active`: el del panel con el foco de la pestaña activa.
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// Pestañas de la barra: cada una reparte su espacio entre uno o varios terminales.
    pub groups: Vec<crate::panes::Group>,
    pub group: usize,
    /// Línea divisoria que se está arrastrando: (camino en el árbol, lado a lado, área que reparte).
    pub drag: Option<(Vec<bool>, bool, crate::panes::Rect)>,
    pub next_id: u64,
    pub scale: f32,
    pub font_pt: f32,
    pub mods: ModifiersState,
    pub mouse: (f32, f32),
    /// Enlace o ruta bajo el ratón con Ctrl pulsado (pestaña, celda del ratón, lo encontrado).
    pub link: Option<(u64, (usize, usize), Option<crate::links::Found>)>,
    pub selecting: bool,
    pub mouse_buttons: u8,
    pub last_click: (Instant, u8),
    pub hover: Option<TabHit>,
    /// Nombre de la pestaña bajo el ratón, que se desplaza si no cabe.
    pub marquee: crate::tabs::Marquee,
    /// Ancho de cada pestaña ajustado a su nombre (interfaz Flotante con píldoras), medido al
    /// empezar cada fotograma: el reparto de la barra (`TabBar::layout`) no puede medir texto.
    pub tab_fit: Vec<f32>,
    /// Pestaña a la que se le está cambiando el nombre.
    pub rename: Option<crate::tabs::Rename>,
    /// Último clic en una pestaña (para el doble clic que le cambia el nombre).
    pub tab_click: Option<(usize, Instant)>,
    pub cursor_icon: winit::window::CursorIcon,
    pub palette: Option<Palette>,
    /// Búsqueda en el historial (Ctrl+Shift+F), de la pestaña `Search::tab`.
    pub search: Option<crate::search::Search>,
    /// Menú de completado abierto (Tab).
    pub menu: Option<crate::complete::Menu>,
    /// "Menú al escribir": lo que había escrito antes de la última tecla; el menú se abre cuando
    /// llega su eco (lo escrito cambia).
    pub menu_auto: Option<(String, Instant)>,
    pub settings: Option<crate::settings::Settings>,
    /// Editor de aspecto (terminal arriba, opciones abajo).
    pub look: Option<crate::look::LookEditor>,
    /// Último reparto del editor de aspecto, para volver a abrirlo igual.
    pub look_ratio: f32,
    /// Sesión de muestra del editor de aspecto.
    pub sample: Option<crate::look::Sample>,
    /// Apartado de estadísticas de los agentes (Ctrl+Shift+S).
    pub stats: Option<crate::stats::view::StatsView>,
    pub update: Option<crate::update::UpdateState>,
    /// Búsqueda manual de actualizaciones (Ajustes).
    pub update_check: crate::update::CheckState,
    pub panel: crate::panel::PanelState,
    /// Cambios de git: lo leído de cada repositorio y el panel "Cambios".
    pub git: crate::git::Git,
    pub changes: crate::changes::ChangesPanel,
    /// Indicadores de cambios del prompt dibujados en el último fotograma (terminal, rectángulo),
    /// para el ratón.
    pub git_chips: Vec<(u64, [f32; 4])>,
    pub anim_epoch: Instant,
    pub last_anim: Instant,
    pub clipboard: Box<dyn ClipboardProvider>,
    pub selection_clip: Box<dyn ClipboardProvider>,
    pub focused: bool,
    pub cursor_on: bool,
    pub next_blink: Instant,
    /// Cuándo devolver al sistema la memoria que ya no se usa (`platform::trim_memory`): una vez
    /// tras arrancar y tras cerrar pestañas o paneles. Solo en Windows.
    pub trim_at: Option<Instant>,
    pub scroll_accum: f32,
    pub last_ws: WindowSize,
    /// La ventana se creó con la barra de título del sistema. Cambiar el ajuste se aplica al
    /// reiniciar: hasta entonces la barra de Garbo (con sus botones de ventana) sigue como está.
    pub decorations: bool,
    out: Vec<Instance>,
    fg: Vec<Instance>,
}

/// Geometría del área de terminal en píxeles físicos.
pub struct Geometry {
    pub tab_bar: Option<TabBar>,
    /// Celdas del panel con el foco: origen y tamaño.
    pub x: f32,
    pub y: f32,
    pub ws: WindowSize,
    /// Panel lateral (x, y, ancho, alto) si está abierto.
    pub panel: Option<(f32, f32, f32, f32)>,
    /// Panel de los cambios de git (x, y, ancho, alto), entre los terminales y el lateral.
    pub changes: Option<(f32, f32, f32, f32)>,
    /// Rectángulo del panel con el foco (con su margen).
    pub pane: crate::panes::Rect,
    /// Paneles de la pestaña activa.
    pub panes: Vec<PaneGeo>,
    pub dividers: Vec<crate::panes::Divider>,
    /// Zona de los terminales (sin barra de pestañas ni panel lateral).
    pub area: crate::panes::Rect,
    /// Medidas de la interfaz (márgenes, radios… de la Flotante; ceros con la Clásica).
    pub chrome: crate::chrome::Chrome,
}

/// Un panel: índice de su terminal en `State::tabs`, su rectángulo y dónde empiezan sus celdas.
#[derive(Clone, Copy, Debug)]
pub struct PaneGeo {
    pub idx: usize,
    pub rect: crate::panes::Rect,
    pub x: f32,
    pub y: f32,
    pub ws: WindowSize,
}

impl PaneGeo {
    /// Dónde va cada fila del terminal del panel (ver `view::Rows`). `gaps`: huecos de los
    /// bloques, en píxeles físicos.
    pub fn rows<T>(
        &self,
        term: &alacritty_terminal::Term<T>,
        bs: &blocks::BlockState,
        ch: f32,
        gaps: crate::style::Gaps,
    ) -> Rows {
        let lines = self.ws.num_lines as usize;
        Rows::layout(term, bs, lines, ch, gaps, self.room())
    }

    /// Alto en el que se ven las filas: el del panel sin sus márgenes.
    pub fn room(&self) -> f32 {
        self.rect[3] - 2.0 * (self.y - self.rect[1])
    }
}

impl App {
    pub fn new(cfg: Config, proxy: EventLoopProxy<UserEvent>) -> Self {
        let theme = cfg.theme();
        let keys = Keybindings::new(&cfg.keybindings);
        Self {
            cfg,
            theme,
            keys,
            proxy,
            st: None,
        }
    }

    pub fn init(&mut self, window: Arc<Window>) {
        let scale = window.scale_factor() as f32;
        let font_pt = self.cfg.font.size;
        let glyphs = build_glyphs(&self.cfg, font_pt, scale);
        let renderer = Renderer::new(window.clone(), true);
        let (clipboard, selection_clip) = make_clipboards(&window);
        let now = Instant::now();
        self.st = Some(State {
            window,
            renderer,
            glyphs,
            tabs: Vec::new(),
            active: 0,
            groups: Vec::new(),
            group: 0,
            drag: None,
            next_id: 1,
            scale,
            font_pt,
            mods: ModifiersState::empty(),
            mouse: (0.0, 0.0),
            link: None,
            selecting: false,
            mouse_buttons: 0,
            last_click: (now, 0),
            hover: None,
            marquee: crate::tabs::Marquee::default(),
            tab_fit: Vec::new(),
            rename: None,
            tab_click: None,
            cursor_icon: winit::window::CursorIcon::Default,
            palette: None,
            search: None,
            menu: None,
            menu_auto: None,
            settings: None,
            look: None,
            look_ratio: 0.36,
            sample: None,
            stats: None,
            update: None,
            update_check: Default::default(),
            panel: Default::default(),
            git: crate::git::Git::new(self.proxy.clone()),
            changes: Default::default(),
            git_chips: Vec::new(),
            anim_epoch: now,
            last_anim: now,
            clipboard,
            selection_clip,
            focused: true,
            cursor_on: true,
            next_blink: now,
            trim_at: cfg!(windows).then(|| now + TRIM_AFTER_START),
            scroll_accum: 0.0,
            last_ws: WindowSize {
                num_lines: 0,
                num_cols: 0,
                cell_width: 0,
                cell_height: 0,
            },
            decorations: self.cfg.window.decorations,
            out: Vec::with_capacity(8192),
            fg: Vec::with_capacity(8192),
        });
        self.new_tab();
        // Herramienta de desarrollo: abrir las estadísticas al arrancar (capturas sin teclear).
        if std::env::var_os("GARBO_OPEN_STATS").is_some() {
            self.toggle_stats();
        }
        // Igual con el editor de aspecto: "sample" = con la sesión de muestra; ",N" = la sección N
        // del panel.
        if let Ok(v) = std::env::var("GARBO_OPEN_LOOK") {
            self.open_look();
            if let Some(look) = self.st().look.as_mut() {
                let mut parts = v.split(',');
                look.sample = parts.next() == Some("sample");
                if let Some(n) = parts.next().and_then(|n| n.parse::<usize>().ok()) {
                    look.set_section(n);
                }
            }
        }
        // Igual con el panel de cambios de git.
        if std::env::var_os("GARBO_OPEN_CHANGES").is_some() {
            self.toggle_changes();
        }
        // Herramienta de desarrollo: varias pestañas al arrancar (capturas de la barra).
        let extra_tabs = std::env::var("GARBO_OPEN_TABS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1);
        for _ in 1..extra_tabs.min(12) {
            self.new_tab();
        }
        // Igual con el panel de agentes.
        if std::env::var_os("GARBO_OPEN_AGENTS").is_some() {
            self.toggle_panel(true);
        }
        // Igual con el de puertos.
        if std::env::var_os("GARBO_OPEN_PORTS").is_some() {
            self.toggle_panel(false);
        }
        // Igual con la paleta de comandos, con lo buscado ya escrito.
        if let Ok(q) = std::env::var("GARBO_OPEN_PALETTE") {
            let mut pal = Palette::new(&self.keys);
            pal.query = q;
            pal.refilter();
            self.st().palette = Some(pal);
        }
        // Igual con Ajustes: "N" = la categoría N; ",P" = P píxeles más abajo.
        if let Ok(v) = std::env::var("GARBO_OPEN_SETTINGS") {
            let mut parts = v.split(',').map(|n| n.trim().parse::<usize>().unwrap_or(0));
            let mut set = crate::settings::Settings::new();
            set.set_cat(parts.next().unwrap_or(0));
            set.wheel(-(parts.next().unwrap_or(0) as f32));
            self.st().settings = Some(set);
        }
        crate::update::ENABLED.store(self.cfg.check_updates, std::sync::atomic::Ordering::Relaxed);
        if self.cfg.check_updates {
            crate::update::start_checker(self.proxy.clone());
        }
    }

    /// Clic en el aviso de actualización. Devuelve `true` si la app debe cerrarse (reinicio).
    pub fn on_update_click(&mut self) -> bool {
        use crate::update::{self, UpdateState as U};
        let proxy = self.proxy.clone();
        let st = self.st();
        match st.update.clone() {
            Some(U::Available(rel)) => {
                if update::can_install(&rel) {
                    let version = rel.version.clone();
                    st.update = Some(match update::start_install(rel, proxy) {
                        Ok(()) => U::Downloading(version),
                        Err(e) => U::Failed(e.to_string()),
                    });
                    st.window.request_redraw();
                } else {
                    crate::platform::open(&rel.page);
                }
            }
            Some(U::Installed(_)) => {
                if crate::platform::command(update::restart_path())
                    .spawn()
                    .is_ok()
                {
                    return true;
                }
            }
            Some(U::Failed(_)) => {
                let url = format!("https://github.com/{}/releases/latest", update::REPO);
                crate::platform::open(&url);
                st.update = None;
                st.last_ws.num_cols = 0;
                self.relayout();
            }
            _ => {}
        }
        false
    }

    pub fn st(&mut self) -> &mut State {
        self.st.as_mut().expect("estado sin inicializar")
    }

    pub fn geometry(&self) -> Geometry {
        let st = self.st.as_ref().unwrap();
        let (w, h) = st.renderer.size();
        let n_tabs = st.groups.len().max(1);
        let integrated = !st.decorations;
        let panel_w = if st.panel.visible() {
            crate::panel::PanelState::width(st.scale, w)
        } else {
            0.0
        };
        let changes_w = |avail: f32| {
            if st.changes.open {
                st.changes.width(avail, st.scale)
            } else {
                0.0
            }
        };
        // Paneles a la derecha del terminal (también el del editor de aspecto, que lo tapa): una
        // fila de pestañas no pasa por encima. Con las estadísticas no se ven.
        let side = if st.stats.is_some() {
            0.0
        } else {
            let look = st
                .look
                .as_ref()
                .map_or(0.0, |l| w - l.regions(w, st.scale).x);
            (panel_w + changes_w(w - panel_w)).max(look)
        };
        let tab_bar = (integrated || self.cfg.window.always_show_tabs || n_tabs > 1).then(|| {
            TabBar::layout(
                n_tabs,
                w,
                h,
                st.scale,
                integrated,
                st.update.is_some(),
                &self.theme.style,
                self.cfg.tabs,
                st.stats.is_some(),
                side,
                &st.tab_fit,
                st.tabs
                    .get(st.active)
                    .and_then(|t| st.git.summary_for(t.id))
                    .map(|c| c.files),
            )
        });
        let top = tab_bar.as_ref().map_or(0.0, |t| t.height);
        // Las pestañas fuera de la barra de arriba se quedan con su franja o su columna (también
        // con las estadísticas abiertas: así los terminales no cambian de tamaño).
        let (left, right, bottom) = match tab_bar.as_ref() {
            Some(tb) => {
                use crate::tabs::Position;
                let [_, _, zw, zh] = tb.zone;
                match tb.at {
                    Position::Top => (0.0, 0.0, 0.0),
                    Position::Bottom => (0.0, 0.0, zh),
                    Position::Left => (zw, 0.0, 0.0),
                    Position::Right => (0.0, zw, 0.0),
                }
            }
            None => (0.0, 0.0, 0.0),
        };
        let chrome = crate::chrome::Chrome::new(&self.theme.style, st.scale);
        let changes_w = changes_w(w - left - right - panel_w);
        let edges = crate::chrome::Edges {
            top,
            left,
            right,
            bottom,
        };
        let crate::chrome::Areas {
            area,
            panel,
            changes,
        } = crate::chrome::areas(w, h, edges, panel_w, changes_w, &chrome);
        let gap = chrome.pane_gap(st.scale);
        let (panes, dividers) = match st.groups.get(st.group) {
            Some(g) => (
                self.pane_geos(&g.layout, area, gap),
                g.layout.dividers(area, gap),
            ),
            None => (Vec::new(), Vec::new()),
        };
        let focus = panes
            .iter()
            .find(|p| p.idx == st.active)
            .or(panes.first())
            .copied();
        let fallback = self.pane_geo(usize::MAX, area);
        let f = focus.unwrap_or(fallback);
        Geometry {
            tab_bar,
            x: f.x,
            y: f.y,
            ws: f.ws,
            panel,
            changes,
            pane: f.rect,
            panes,
            dividers,
            area,
            chrome,
        }
    }

    /// Dónde empiezan las celdas de un panel y cuántas caben (con el margen de la ventana).
    fn pane_geo(&self, idx: usize, rect: crate::panes::Rect) -> PaneGeo {
        let st = self.st.as_ref().unwrap();
        let m = st.glyphs.fonts.metrics;
        let pad = (self.cfg.window.padding * st.scale).round();
        // Flotante: algo más de margen dentro de la tarjeta, para que las de los bloques y la de
        // la orden que se escribe queden separadas de su borde igual por los lados y por abajo.
        let floating = self.theme.style.interface == crate::style::Interface::Floating;
        let (px, py) = if floating {
            (
                pad + (14.0 * st.scale).round(),
                pad + (6.0 * st.scale).round(),
            )
        } else {
            (pad, pad)
        };
        let cols = ((rect[2] - 2.0 * px) / m.width).floor().max(2.0);
        let lines = ((rect[3] - 2.0 * py) / m.height).floor().max(1.0);
        PaneGeo {
            idx,
            rect,
            x: rect[0] + px,
            y: rect[1] + py,
            ws: WindowSize {
                num_lines: lines as u16,
                num_cols: cols as u16,
                cell_width: m.width as u16,
                cell_height: m.height as u16,
            },
        }
    }

    fn pane_geos(
        &self,
        layout: &crate::panes::Layout,
        area: crate::panes::Rect,
        gap: f32,
    ) -> Vec<PaneGeo> {
        let st = self.st.as_ref().unwrap();
        layout
            .rects(area, gap)
            .into_iter()
            .filter_map(|(id, r)| {
                st.tabs
                    .iter()
                    .position(|t| t.id == id)
                    .map(|i| self.pane_geo(i, r))
            })
            .collect()
    }

    /// Pone `active` en el terminal con el foco de la pestaña activa.
    fn sync_active(&mut self) {
        let st = self.st();
        if let Some(i) = st
            .groups
            .get(st.group)
            .and_then(|g| st.tabs.iter().position(|t| t.id == g.focus))
        {
            st.active = i;
        }
    }

    /// ¿Tiene la pestaña activa más de un panel?
    pub fn has_splits(&self) -> bool {
        let Some(st) = self.st.as_ref() else {
            return false;
        };
        st.groups
            .get(st.group)
            .is_some_and(|g| matches!(g.layout, crate::panes::Layout::Split { .. }))
    }

    /// Recalcula filas/columnas de cada panel y avisa a los PTY que cambiaron (a todos si
    /// `last_ws.num_cols` es 0: cambió la fuente, la escala…).
    pub fn relayout(&mut self) {
        let geo = self.geometry();
        let area = geo.area;
        let sizes: Vec<(usize, WindowSize)> = {
            let st = self.st.as_ref().unwrap();
            let gap = geo.chrome.pane_gap(st.scale);
            st.groups
                .iter()
                .flat_map(|g| self.pane_geos(&g.layout, area, gap))
                .map(|p| (p.idx, p.ws))
                .collect()
        };
        let same = |a: &WindowSize, b: &WindowSize| {
            (a.num_lines, a.num_cols, a.cell_width, a.cell_height)
                == (b.num_lines, b.num_cols, b.cell_width, b.cell_height)
        };
        let st = self.st();
        let force = st.last_ws.num_cols == 0;
        for (i, ws) in sizes {
            if let Some(tab) = st.tabs.get_mut(i) {
                if force || !same(&ws, &tab.ws) {
                    tab.resize(ws);
                }
            }
        }
        st.last_ws = geo.ws;
        st.window.request_redraw();
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        let st = self.st.as_ref()?;
        st.tabs.get(st.active)
    }

    pub fn new_tab(&mut self) {
        let cwd = self.active_tab().and_then(Tab::cwd);
        let proxy = self.proxy.clone();
        let cfg = self.cfg.clone();
        // Se añade primero para que la geometría cuente la barra de pestañas nueva.
        let st = self.st();
        let id = st.next_id;
        st.next_id += 1;
        let ws_guess = st.last_ws;
        let ws = if ws_guess.num_cols == 0 {
            WindowSize {
                num_lines: 24,
                num_cols: 80,
                ..ws_guess
            }
        } else {
            ws_guess
        };
        match Tab::spawn(id, &cfg, ws, cwd, proxy) {
            Ok(tab) => {
                let st = self.st();
                st.tabs.push(tab);
                st.groups.push(crate::panes::Group::new(id));
                st.group = st.groups.len() - 1;
                st.search = None;
                self.sync_active();
                self.st().last_ws.num_cols = 0; // fuerza resize con la geometría real
                self.relayout();
                self.update_title();
            }
            Err(e) => log::error!("No se pudo abrir el shell: {e}"),
        }
    }

    /// Parte el panel con el foco: el nuevo terminal (en la misma carpeta) va a la derecha
    /// (`side`) o debajo, y se queda con el foco.
    pub fn split(&mut self, side: bool) {
        let cwd = self.active_tab().and_then(Tab::cwd);
        let proxy = self.proxy.clone();
        let cfg = self.cfg.clone();
        let st = self.st();
        let Some(focus) = st.groups.get(st.group).map(|g| g.focus) else {
            return;
        };
        let id = st.next_id;
        st.next_id += 1;
        let ws = st.last_ws;
        match Tab::spawn(id, &cfg, ws, cwd, proxy) {
            Ok(tab) => {
                let st = self.st();
                st.tabs.push(tab);
                let g = &mut st.groups[st.group];
                g.layout.split(focus, id, side);
                g.focus = id;
                st.search = None;
                self.sync_active();
                self.relayout();
                self.update_title();
            }
            Err(e) => log::error!("No se pudo abrir el shell: {e}"),
        }
    }

    /// Pasa el foco al panel vecino en esa dirección.
    pub fn focus_pane(&mut self, dir: crate::panes::Dir) {
        let geo = self.geometry();
        let st = self.st();
        let rects: Vec<(u64, crate::panes::Rect)> = geo
            .panes
            .iter()
            .map(|p| (st.tabs[p.idx].id, p.rect))
            .collect();
        let Some(from) = st.tabs.get(st.active).map(|t| t.id) else {
            return;
        };
        if let Some(to) = crate::panes::neighbor(&rects, from, dir) {
            if let Some(i) = st.tabs.iter().position(|t| t.id == to) {
                self.select_tab(i);
            }
        }
    }

    /// Cierra un panel (terminal `idx`); si era el último de su pestaña, la pestaña. Devuelve
    /// `true` si ya no queda ninguna.
    pub fn close_pane(&mut self, idx: usize) -> bool {
        let st = self.st();
        let Some(id) = st.tabs.get(idx).map(|t| t.id) else {
            return st.tabs.is_empty();
        };
        st.tabs.remove(idx);
        if let Some(g) = st.groups.iter().position(|g| g.layout.contains(id)) {
            let layout = std::mem::replace(&mut st.groups[g].layout, crate::panes::Layout::Leaf(0));
            match layout.remove(id) {
                Some(rest) => {
                    let group = &mut st.groups[g];
                    if group.focus == id {
                        group.focus = rest.leaves()[0];
                    }
                    group.layout = rest;
                }
                None => {
                    st.groups.remove(g);
                    // El campo del nombre apuntaba a esta pestaña o a una que se ha movido.
                    if st.rename.as_ref().is_some_and(|r| r.group >= g) {
                        st.rename = None;
                    }
                    if g < st.group || st.group >= st.groups.len() {
                        st.group = st.group.saturating_sub(1);
                    }
                }
            }
        }
        if st.tabs.is_empty() || st.groups.is_empty() {
            return true;
        }
        // Lo que ocupaba (historial, PTY) ya está libre: se devuelve al sistema poco después.
        self.schedule_trim(TRIM_AFTER_CLOSE);
        self.sync_active();
        self.st().last_ws.num_cols = 0;
        self.relayout();
        self.update_title();
        false
    }

    /// Devuelve al sistema la memoria que no se usa dentro de `delay` (solo en Windows, ver
    /// `platform::trim_memory`). Si ya había un aviso antes, se mantiene.
    pub fn schedule_trim(&mut self, delay: std::time::Duration) {
        if cfg!(windows) {
            let at = Instant::now() + delay;
            let st = self.st();
            st.trim_at = Some(st.trim_at.map_or(at, |t| t.min(at)));
        }
    }

    /// Cierra una pestaña de la barra con todos sus paneles.
    pub fn close_tab(&mut self, g: usize) -> bool {
        let Some(ids) = self.st().groups.get(g).map(|g| g.layout.leaves()) else {
            return false;
        };
        let mut last = false;
        for id in ids {
            if let Some(i) = self.st().tabs.iter().position(|t| t.id == id) {
                last = self.close_pane(i);
            }
        }
        last
    }

    /// Lleva el foco al terminal `idx` (y a su pestaña).
    pub fn select_tab(&mut self, idx: usize) {
        let st = self.st();
        let Some(id) = st.tabs.get(idx).map(|t| t.id) else {
            return;
        };
        let Some(g) = st.groups.iter().position(|g| g.layout.contains(id)) else {
            return;
        };
        st.group = g;
        st.groups[g].focus = id;
        if idx != st.active {
            st.active = idx;
            st.search = None;
            st.window.request_redraw();
            self.update_title();
        }
    }

    /// Activa la pestaña `g` de la barra (con el panel que tenía el foco).
    pub fn select_group(&mut self, g: usize) {
        let Some(id) = self.st().groups.get(g).map(|g| g.focus) else {
            return;
        };
        if let Some(i) = self.st().tabs.iter().position(|t| t.id == id) {
            self.select_tab(i);
        }
    }

    pub fn update_title(&mut self) {
        let st = self.st();
        let named = st.groups.get(st.group).and_then(|g| g.name.clone());
        if let Some(tab) = st.tabs.get(st.active) {
            let label = named.as_deref().unwrap_or(tab.label());
            st.window.set_title(&format!("{label} — Garbo"));
        }
    }

    /// Nombre que se ve en la pestaña `g` de la barra: el puesto a mano o el automático.
    pub fn group_label(&self, g: usize) -> Option<String> {
        let st = self.st.as_ref()?;
        let group = st.groups.get(g)?;
        if let Some(name) = &group.name {
            return Some(name.clone());
        }
        st.tabs
            .iter()
            .find(|t| t.id == group.focus)
            .map(|t| t.label().to_string())
    }

    /// Empieza a cambiar el nombre de la pestaña `g` de la barra (campo de texto sobre ella).
    pub fn start_rename(&mut self, g: usize) {
        let Some(current) = self.group_label(g) else {
            return;
        };
        let st = self.st();
        st.palette = None;
        st.menu = None;
        st.settings = None;
        st.rename = Some(crate::tabs::Rename::new(g, &current));
        st.window.request_redraw();
    }

    /// Termina de cambiar el nombre: lo guarda (`keep`) o lo deja como estaba.
    pub fn finish_rename(&mut self, keep: bool) {
        let st = self.st();
        let Some(rn) = st.rename.take() else { return };
        if keep {
            if let Some(g) = st.groups.get_mut(rn.group) {
                g.name = rn.result();
            }
        }
        st.window.request_redraw();
        self.update_title();
    }

    pub fn set_font_pt(&mut self, pt: f32) {
        let st = self.st();
        st.font_pt = pt.clamp(5.0, 72.0);
        let px = pt_to_px(st.font_pt, st.scale);
        st.glyphs.fonts.set_size(px);
        st.glyphs.clear();
        self.relayout();
    }

    pub fn set_scale(&mut self, scale: f32) {
        let ui_px = pt_to_px(self.cfg.font.ui_size, scale);
        self.st().scale = scale;
        self.st().glyphs.ui.set_size(ui_px);
        let pt = self.st().font_pt;
        self.set_font_pt(pt);
    }

    pub fn copy_selection(&mut self, to_primary: bool) {
        let st = self.st();
        let Some(tab) = st.tabs.get(st.active) else {
            return;
        };
        let text = tab.term.lock().selection_to_string();
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            let cb = if to_primary {
                &mut st.selection_clip
            } else {
                &mut st.clipboard
            };
            if let Err(e) = cb.set_contents(text) {
                log::warn!("portapapeles: {e}");
            }
        }
    }

    /// Copia al portapapeles el texto seleccionado y quita la selección (Ctrl+C y clic derecho
    /// en Windows). `false` si no hay nada seleccionado.
    #[cfg(windows)]
    pub fn copy_and_clear_selection(&mut self) -> bool {
        let st = self.st();
        let selected = st.tabs.get(st.active).is_some_and(|t| {
            t.term
                .lock()
                .selection
                .as_ref()
                .is_some_and(|s| !s.is_empty())
        });
        if !selected {
            return false;
        }
        self.copy_selection(false);
        let st = self.st();
        if let Some(tab) = st.tabs.get(st.active) {
            tab.term.lock().selection = None;
        }
        st.window.request_redraw();
        true
    }

    /// Clic derecho en Windows: copia lo seleccionado o, si no hay nada, pega.
    #[cfg(windows)]
    pub fn copy_or_paste(&mut self) {
        if !self.copy_and_clear_selection() {
            self.paste(false);
        }
    }

    /// Abre lo encontrado con Ctrl+clic (ver `links.rs`).
    pub fn open_link(&mut self, target: crate::links::Target) {
        use crate::links::{Open, Target};
        let cwd = self.active_tab().and_then(Tab::cwd);
        let open = match target {
            Target::Url(url) => Open::System(url),
            Target::File { path, line, col } => {
                let editor = std::env::var("VISUAL")
                    .ok()
                    .filter(|e| !e.is_empty())
                    .or_else(|| std::env::var("EDITOR").ok().filter(|e| !e.is_empty()))
                    .or_else(crate::platform::default_editor);
                crate::links::open_file(&path, line, col, editor.as_deref())
            }
        };
        match open {
            Open::System(target) => crate::platform::open(&target),
            Open::Spawn(cmd) => {
                // En Windows, `code` es `code.cmd`: se busca con las extensiones de PATHEXT.
                let program = crate::platform::find_program(&cmd[0])
                    .map_or_else(|| cmd[0].clone().into(), std::ffi::OsString::from);
                // Son editores con ventana; en Windows, `code.cmd` abriría además una consola.
                let mut c = crate::platform::command(program);
                c.args(&cmd[1..])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
                if let Some(d) = &cwd {
                    c.current_dir(d);
                }
                match c.spawn() {
                    // Se recoge en segundo plano para que no quede como proceso zombi.
                    Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
                    Err(e) => log::warn!("No se pudo abrir {:?}: {e}", cmd),
                }
            }
            Open::InTab(cmd) => {
                self.new_tab();
                if let Some(tab) = self.active_tab() {
                    tab.input(format!("{cmd}\r").into_bytes());
                }
            }
        }
    }

    pub fn paste(&mut self, from_primary: bool) {
        let proxy = self.proxy.clone();
        let st = self.st();
        let cb = if from_primary {
            &mut st.selection_clip
        } else {
            &mut st.clipboard
        };
        match cb.get_contents() {
            Ok(text) if !text.is_empty() => self.paste_text(&text),
            // Sin texto: quizá una imagen (captura de pantalla). Se guarda y se pega su ruta.
            _ if !from_primary => {
                let Some(id) = st.tabs.get(st.active).map(|t| t.id) else {
                    return;
                };
                let _ = std::thread::Builder::new()
                    .name("pegar-imagen".into())
                    .spawn(move || {
                        let r = crate::clipboard::save_image();
                        let _ = proxy.send_event(UserEvent::PasteImage(id, r));
                    });
            }
            _ => {}
        }
    }

    pub fn paste_text(&mut self, text: &str) {
        let Some(id) = self.active_tab().map(|t| t.id) else {
            return;
        };
        self.paste_text_to(id, text);
    }

    /// Pega en la pestaña `id` (respetando el modo "bracketed paste" del programa).
    pub fn paste_text_to(&mut self, id: u64, text: &str) {
        let st = self.st();
        let Some(tab) = st.tabs.iter().find(|t| t.id == id) else {
            return;
        };
        let mode = *tab.term.lock().mode();
        let bytes = if mode.contains(TermMode::BRACKETED_PASTE) {
            // Evita que el texto pegado cierre el modo bracketed por su cuenta.
            let clean = text.replace("\x1b[201~", "").replace('\x1b', "");
            format!("\x1b[200~{clean}\x1b[201~").into_bytes()
        } else if let Some(b) = powershell_paste(tab, text) {
            b
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        };
        tab.input(bytes);
        self.scroll_to_bottom();
    }

    pub fn scroll(&mut self, s: Scroll) {
        let st = self.st();
        if let Some(tab) = st.tabs.get(st.active) {
            tab.term.lock().scroll_display(s);
            st.window.request_redraw();
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        let st = self.st();
        if let Some(tab) = st.tabs.get(st.active) {
            let mut term = tab.term.lock();
            if term.grid().display_offset() != 0 {
                term.scroll_display(Scroll::Bottom);
                st.window.request_redraw();
            }
        }
    }

    pub fn reload_config(&mut self) {
        let old_font = (
            self.cfg.font.family.clone(),
            self.cfg.font.fallback.clone(),
            self.cfg.font.line_height,
        );
        self.cfg = Config::load();
        self.theme = self.cfg.theme();
        self.keys = Keybindings::new(&self.cfg.keybindings);
        let cfg = self.cfg.clone();
        let st = self.st();
        let tc = term_config(&cfg);
        for tab in &st.tabs {
            tab.term.lock().set_options(tc.clone());
        }
        if old_font
            != (
                cfg.font.family.clone(),
                cfg.font.fallback.clone(),
                cfg.font.line_height,
            )
        {
            let px = pt_to_px(cfg.font.size, st.scale);
            let _ = px;
            st.glyphs = build_glyphs(&cfg, cfg.font.size, st.scale);
        }
        st.last_ws.num_cols = 0;
        self.set_font_pt(cfg.font.size);
        log::info!("Configuración recargada");
    }

    pub fn set_theme(&mut self, name: &str) {
        self.cfg.theme = name.to_string();
        self.theme = self.cfg.theme();
        // La forma del tema puede cambiar el alto de la barra.
        self.relayout();
    }

    /// Abre el editor de aspecto (y cierra lo que lo taparía).
    pub fn open_look(&mut self) {
        let st = self.st();
        st.settings = None;
        st.palette = None;
        st.menu = None;
        st.look = Some(crate::look::LookEditor::new(st.look_ratio));
        st.window.request_redraw();
    }

    pub fn close_look(&mut self) {
        let st = self.st();
        if let Some(look) = st.look.take() {
            st.look_ratio = look.ratio;
        }
        st.sample = None;
        st.window.request_redraw();
    }

    /// Ejecuta una acción. Devuelve `true` si la app debe cerrarse.
    pub fn run_action(&mut self, action: Action) -> bool {
        match action {
            Action::NewTab => self.new_tab(),
            // Cierra el panel con el foco (y la pestaña si era el único).
            Action::CloseTab => {
                let idx = self.st().active;
                return self.close_pane(idx);
            }
            Action::NextTab | Action::PrevTab => {
                let st = self.st();
                let n = st.groups.len();
                if n > 1 {
                    let d = if action == Action::NextTab { 1 } else { n - 1 };
                    let g = (st.group + d) % n;
                    self.select_group(g);
                }
            }
            Action::MoveTabLeft | Action::MoveTabRight => {
                let st = self.st();
                let i = st.group;
                let j = if action == Action::MoveTabLeft {
                    i.checked_sub(1)
                } else {
                    Some(i + 1)
                };
                if let Some(j) = j.filter(|&j| j < st.groups.len()) {
                    st.groups.swap(i, j);
                    st.group = j;
                    st.window.request_redraw();
                }
            }
            Action::GotoTab(i) => {
                let n = self.st().groups.len();
                // Alt+9 siempre va a la última, como en los navegadores
                self.select_group(if i == 8 { n - 1 } else { i });
            }
            Action::SplitRight => self.split(true),
            Action::SplitDown => self.split(false),
            Action::FocusLeft => self.focus_pane(crate::panes::Dir::Left),
            Action::FocusRight => self.focus_pane(crate::panes::Dir::Right),
            Action::FocusUp => self.focus_pane(crate::panes::Dir::Up),
            Action::FocusDown => self.focus_pane(crate::panes::Dir::Down),
            Action::Copy => {
                // Sin texto seleccionado pero con un bloque seleccionado: se copia el bloque.
                let st = self.st();
                let has_sel = st.tabs.get(st.active).is_some_and(|t| {
                    t.term
                        .lock()
                        .selection
                        .as_ref()
                        .is_some_and(|s| !s.is_empty())
                });
                let has_block = st
                    .tabs
                    .get(st.active)
                    .is_some_and(|t| t.selected_block.is_some());
                if !has_sel && has_block {
                    self.copy_block(BlockPart::Whole);
                } else {
                    self.copy_selection(false);
                }
            }
            Action::Settings => {
                let st = self.st();
                st.palette = None;
                st.settings = if st.settings.is_some() {
                    None
                } else {
                    Some(crate::settings::Settings::new())
                };
                st.window.request_redraw();
            }
            Action::RenameTab => {
                let g = self.st().group;
                self.start_rename(g);
            }
            Action::Appearance => {
                if self.st().look.is_some() {
                    self.close_look();
                } else {
                    self.open_look();
                }
            }
            Action::ToggleAgents => self.toggle_panel(true),
            Action::ToggleChanges => self.toggle_changes(),
            Action::AgentStats => self.toggle_stats(),
            Action::Search => {
                let st = self.st();
                let tab = st.tabs.get(st.active).map(|t| (t.id, t.selected_block));
                st.search = match (&st.search, tab) {
                    (Some(s), Some((id, _))) if s.tab == id => None,
                    (_, Some((id, block))) => Some(crate::search::Search::new(id, block)),
                    _ => None,
                };
                st.palette = None;
                st.menu = None;
                st.window.request_redraw();
            }
            Action::TogglePorts => self.toggle_panel(false),
            Action::PrevBlock => self.goto_block(-1),
            Action::NextBlock => self.goto_block(1),
            Action::CopyBlockOutput => self.copy_block(BlockPart::Output),
            Action::CopyBlockCommand => self.copy_block(BlockPart::Command),
            Action::CopyBlock => self.copy_block(BlockPart::Whole),
            Action::Paste => self.paste(false),
            Action::FontBigger => {
                let pt = self.st().font_pt + 1.0;
                self.set_font_pt(pt);
            }
            Action::FontSmaller => {
                let pt = self.st().font_pt - 1.0;
                self.set_font_pt(pt);
            }
            Action::FontReset => {
                let pt = self.cfg.font.size;
                self.set_font_pt(pt);
            }
            Action::ScrollPageUp => self.scroll(Scroll::PageUp),
            Action::ScrollPageDown => self.scroll(Scroll::PageDown),
            Action::ScrollTop => self.scroll(Scroll::Top),
            Action::ScrollBottom => self.scroll(Scroll::Bottom),
            Action::ClearScrollback => {
                let st = self.st();
                if let Some(tab) = st.tabs.get(st.active) {
                    tab.term.lock().grid_mut().clear_history();
                    st.window.request_redraw();
                }
            }
            Action::ReloadConfig => self.reload_config(),
            Action::CommandPalette => {
                let keys = &self.keys;
                let st = self.st.as_mut().unwrap();
                st.palette = if st.palette.is_some() {
                    None
                } else {
                    Some(Palette::new(keys))
                };
                st.window.request_redraw();
            }
            Action::NextTheme => {
                let names = Theme::names();
                let i = names
                    .iter()
                    .position(|n| *n == self.cfg.theme)
                    .map_or(0, |i| (i + 1) % names.len());
                self.set_theme(names[i]);
            }
        }
        false
    }

    /// Recalcula agentes y directorios de todas las pestañas. Barato: una llamada al sistema por
    /// pestaña, y la identificación del proceso como mucho cada 700 ms.
    pub fn refresh_sessions(&mut self) -> bool {
        let now = Instant::now();
        let Some(st) = self.st.as_mut() else {
            return false;
        };
        let focused = st.focused;
        let mut changed = false;
        let agents_panel = st.panel.agents;
        let ncfg = &self.cfg.notifications;
        let min_work = std::time::Duration::from_secs(ncfg.min_work_secs);
        let visible: Vec<u64> = st
            .groups
            .get(st.group)
            .map(|g| g.layout.leaves())
            .unwrap_or_default();
        for tab in st.tabs.iter_mut() {
            let viewing = focused && visible.contains(&tab.id);
            changed |= tab.refresh_session(viewing, now);
            if agents_panel {
                changed |= tab.refresh_subagents(now);
            }
            // Aviso del escritorio: al mirar la pestaña se retira; al terminar el agente, se envía.
            if viewing {
                if let Some(id) = tab.notice.shown.take() {
                    crate::notify::close(id);
                }
            }
            // Con OSC 7501 el agente dice si trabaja, te espera o terminó; si no, se deduce de su salida.
            use crate::notify::Activity;
            use crate::status::State;
            let activity = match tab.session.reported {
                Some(State::Working) => Activity::Working,
                Some(State::Blocked) => Activity::Blocked,
                Some(State::Done | State::Error) => Activity::Finished,
                Some(State::Idle) => Activity::Idle,
                None if tab.session.is_working() => Activity::Working,
                None => Activity::Quiet,
            };
            let alive = tab.session.status != crate::sessions::Status::Exited;
            let agent = tab.session.agent.filter(|_| alive);
            let dir = crate::terminal::short_dir(&tab.cwd_label);
            let mut notice = None;
            if let Some(worked) = tab.notice.update(activity, min_work, now) {
                if let Some(agent) = agent.filter(|_| crate::notify::wanted(ncfg, viewing)) {
                    notice = Some(crate::notify::notice_text(
                        agent.name,
                        tab.agent_task().unwrap_or(tab.label()),
                        &dir,
                        worked,
                    ));
                }
            }
            let blocked = match tab.session.status {
                crate::sessions::Status::Blocked(kind) => Some(kind),
                _ => None,
            };
            if tab.notice.blocked_now(blocked.is_some(), now) && ncfg.blocked && !viewing {
                if let (Some(agent), Some(kind)) = (agent, blocked) {
                    notice = Some(crate::notify::blocked_text(
                        agent.name,
                        kind,
                        tab.session.detail.as_deref(),
                        tab.agent_task().unwrap_or(tab.label()),
                        &dir,
                    ));
                }
            }
            if let Some((title, body)) = notice {
                // Uno por pestaña: el anterior sobra.
                if let Some(old) = tab.notice.shown.take() {
                    crate::notify::close(old);
                }
                crate::notify::send(
                    crate::notify::Notice {
                        tab: tab.id,
                        title,
                        body,
                        sound: ncfg.sound,
                    },
                    self.proxy.clone(),
                );
            }
        }
        // Animación del indicador "trabajando", solo mientras algún agente trabaja: ~7 fps los
        // puntos, ~12 las que se mueven suave y ninguno la fija (`tabs::Working::frame`).
        let working = st.tabs.iter().any(|t| t.session.is_working());
        let frame = self.cfg.tabs.working.frame();
        if working && frame.is_some_and(|f| now.duration_since(st.last_anim) >= f) {
            st.last_anim = now;
            changed = true;
        }
        if changed {
            st.window.request_redraw();
        }
        changed
    }

    /// ¿Necesita otro fotograma alguna animación (al despertar por `next_tick`)? El desplazamiento
    /// del editor de aspecto y del panel de cambios, el despliegue de una fila del panel de
    /// agentes, el nombre que se desplaza y las muestras animadas del editor.
    pub fn animating(&self, now: Instant) -> bool {
        let Some(st) = self.st.as_ref() else {
            return false;
        };
        st.panel.animating(now)
            || (st.changes.open && st.changes.animating())
            || st
                .look
                .as_ref()
                .is_some_and(|l| l.animating() || l.shows_working())
            || st
                .marquee
                .next_frame(now)
                .is_some_and(|t| t <= now + std::time::Duration::from_millis(2))
    }

    /// Próximo despertar necesario (animación, cambio de estado o "hace N s"). `None` = dormir.
    pub fn next_tick(&self) -> Option<Instant> {
        let st = self.st.as_ref()?;
        let now = Instant::now();
        let mut next: Option<Instant> = None;
        let mut push = |t: Instant| next = Some(next.map_or(t, |n: Instant| n.min(t)));
        for tab in &st.tabs {
            if let Some(t) = tab.notice.next_wake() {
                push(t);
            }
            if tab.session.is_working() {
                let frame = self.cfg.tabs.working.frame();
                push(
                    now + frame.map_or(std::time::Duration::from_millis(500), |f| {
                        f + std::time::Duration::from_millis(10)
                    }),
                );
            } else if now.duration_since(tab.last_output) < crate::sessions::WORK_WINDOW {
                push(
                    tab.last_output
                        + crate::sessions::WORK_WINDOW
                        + std::time::Duration::from_millis(60),
                );
            }
        }
        if st.panel.agents
            && st.stats.is_none()
            && st.tabs.iter().any(|t| t.session.agent.is_some())
        {
            push(now + std::time::Duration::from_secs(1));
        }
        // Despliegue de una fila del panel: fotogramas solo mientras dura la animación.
        if st.panel.animating(now) {
            push(now + std::time::Duration::from_millis(12));
        }
        // Panel de cambios de git: su próxima lectura y la lista moviéndose tras la rueda.
        if st.changes.open && st.stats.is_none() {
            if let Some(t) = st.changes.poll_at {
                push(t);
            }
            if st.changes.animating() {
                push(now + std::time::Duration::from_millis(16));
            }
        }
        // Lista del editor de aspecto moviéndose tras la rueda o el teclado.
        if st.look.as_ref().is_some_and(|l| l.animating()) {
            push(now + std::time::Duration::from_millis(16));
        }
        // Las muestras animadas de "Agente trabajando" (editor de aspecto, sección Pestañas).
        if st.look.as_ref().is_some_and(|l| l.shows_working()) {
            push(now + std::time::Duration::from_millis(80));
        }
        // Nombre de pestaña que se desplaza: solo con el ratón encima y si no cabe.
        if let Some(t) = st.marquee.next_frame(now) {
            push(t);
        }
        next
    }

    /// Abre o cierra el apartado de estadísticas. Su hilo de lectura solo vive mientras está
    /// abierto.
    pub fn toggle_stats(&mut self) {
        let proxy = self.proxy.clone();
        let st = self.st();
        st.palette = None;
        st.menu = None;
        if st.stats.take().is_some() {
            crate::stats::stop();
        } else {
            st.stats = Some(crate::stats::view::StatsView::new());
            crate::stats::start(proxy);
        }
        st.hover = None;
        st.window.request_redraw();
    }

    pub fn close_stats(&mut self) {
        let st = self.st();
        if st.stats.take().is_some() {
            crate::stats::stop();
            st.window.request_redraw();
        }
    }

    pub fn toggle_panel(&mut self, agents: bool) {
        let proxy = self.proxy.clone();
        let st = self.st();
        st.panel.set_hover(None, Instant::now());
        if agents {
            st.panel.agents = !st.panel.agents;
            for tab in &mut st.tabs {
                tab.subagents.invalidate();
            }
        } else {
            st.panel.ports = !st.panel.ports;
            if st.panel.ports {
                st.panel.ports_loaded = false;
                crate::ports::start(proxy);
            } else {
                crate::ports::stop();
            }
        }
        st.last_ws.num_cols = 0;
        self.relayout();
    }

    /// Abre o cierra el panel de los cambios de git (Ctrl+Shift+G).
    pub fn toggle_changes(&mut self) {
        let st = self.st();
        st.changes.open = !st.changes.open;
        st.changes.hover = None;
        st.changes.dragging = false;
        // Al abrirlo, desde cero: `refresh_git` elige el repositorio y lo lee enseguida.
        st.changes.set_root(None);
        st.last_ws.num_cols = 0;
        self.relayout();
        self.refresh_git();
        self.st().window.request_redraw();
    }

    /// Terminal cuyo indicador de cambios del prompt está bajo (x, y).
    pub fn chip_at(&self, x: f32, y: f32) -> Option<u64> {
        let st = self.st.as_ref()?;
        st.git_chips
            .iter()
            .find(|(_, r)| x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3])
            .map(|(id, _)| *id)
    }

    /// Abre el panel de cambios (el indicador del prompt), si no lo está ya.
    pub fn open_changes(&mut self) {
        if !self.st().changes.open {
            self.toggle_changes();
        }
    }

    /// Sigue los repositorios de los terminales visibles: con cada prompt nuevo se lee el estado
    /// de su repositorio (el indicador del prompt). Con el panel de cambios abierto, muestra el de
    /// la pestaña activa y lo vuelve a leer cada poco. Barato si no hay nada que hacer: un candado
    /// por terminal visible.
    pub fn refresh_git(&mut self) {
        let Some(st) = self.st.as_mut() else { return };
        let visible: Vec<u64> = st
            .groups
            .get(st.group)
            .map(|g| g.layout.leaves())
            .unwrap_or_default();
        for tab in st.tabs.iter().filter(|t| visible.contains(&t.id)) {
            // Por ssh, el repositorio de la carpeta de este equipo no es el del servidor.
            let prompt = tab
                .blocks
                .lock()
                .unwrap()
                .idle_current()
                .filter(|_| !tab.session.remote);
            st.git.track(tab.id, prompt, || tab.cwd());
        }
        if st.git.tracked() > st.tabs.len() {
            let alive: Vec<u64> = st.tabs.iter().map(|t| t.id).collect();
            st.git.forget(&alive, st.changes.root.as_deref());
        }
        if !st.changes.open || st.stats.is_some() {
            return;
        }
        let root = match st.tabs.get(st.active) {
            Some(tab) if !tab.session.remote => st.git.root_of(tab.id, || tab.cwd()),
            _ => None,
        };
        if root != st.changes.root {
            st.changes.set_root(root.clone());
            st.window.request_redraw();
        }
        let now = Instant::now();
        if let Some(root) = root {
            if st.changes.poll_at.is_none_or(|t| now >= t) {
                st.git.refresh(&root);
                let took = st
                    .git
                    .repos
                    .get(&root)
                    .map_or_else(Default::default, |r| r.took);
                st.changes.poll_at = Some(now + crate::changes::POLL.max(took * 20));
            }
        }
    }

    /// Clic en el panel de cambios.
    pub fn on_changes_click(&mut self, hit: crate::changes::Hit) {
        use crate::changes::Hit;
        let st = self.st();
        let Some(root) = st.changes.root.clone() else {
            if hit == Hit::Close {
                self.toggle_changes();
            }
            return;
        };
        let status = match st.git.status(&root) {
            Some(Ok(s)) => Some(s),
            _ => None,
        };
        let mut open = None;
        match hit {
            Hit::Close => {
                self.toggle_changes();
                return;
            }
            Hit::Divider => st.changes.dragging = true,
            Hit::ToggleAll => {
                if let Some(s) = status {
                    for (path, kind) in st.changes.toggle_all(s) {
                        st.git.diff(&root, &path, kind);
                    }
                }
            }
            Hit::File(i) => {
                if let Some((path, kind)) = status.and_then(|s| st.changes.toggle(i, s)) {
                    st.git.diff(&root, &path, kind);
                }
            }
            Hit::Open(i) => {
                if let Some(f) = status.and_then(|s| s.files.get(i)) {
                    let line = st.changes.diff(&f.path).and_then(|d| d.first_change());
                    open = Some((crate::git::file_path(&root, &f.path), line));
                }
            }
            Hit::Line(i, j) => {
                if let Some(f) = status.and_then(|s| s.files.get(i)) {
                    let line = st
                        .changes
                        .diff(&f.path)
                        .and_then(|d| d.lines.get(j))
                        .map(|l| l.new.max(1));
                    open = Some((crate::git::file_path(&root, &f.path), line));
                }
            }
            Hit::Panel => {}
        }
        st.window.request_redraw();
        if let Some((path, line)) = open {
            self.open_link(crate::links::Target::File {
                path,
                line,
                col: None,
            });
        }
    }

    /// Zona del panel lateral bajo el punto (x, y), si el panel está abierto y el punto dentro.
    pub fn panel_hit(&self, x: f32, y: f32, now: Instant) -> Option<crate::panel::Hit> {
        let (px, py, pw, ph) = self.geometry().panel?;
        let st = self.st.as_ref()?;
        let subs: Vec<usize> = st
            .tabs
            .iter()
            .filter(|t| t.session.agent.is_some())
            .map(|t| crate::panel::sub_lines(&t.subagents.list))
            .collect();
        let m = crate::panel::Metrics::from_ui(st.glyphs.ui.metrics.height);
        st.panel.hit(x, y, &subs, px, py, pw, ph, st.scale, m, now)
    }

    /// Clic dentro del panel lateral.
    pub fn on_panel_click(&mut self, hit: crate::panel::Hit) {
        use crate::panel::Hit;
        match hit {
            Hit::CloseAgents => self.toggle_panel(true),
            Hit::ClosePorts => self.toggle_panel(false),
            Hit::Agent(i) => {
                let tab = self
                    .st()
                    .tabs
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.session.agent.is_some())
                    .nth(i)
                    .map(|(j, _)| j);
                if let Some(j) = tab {
                    self.select_tab(j);
                }
            }
            Hit::Port(port) => {
                if self
                    .st()
                    .panel
                    .ports_data
                    .iter()
                    .any(|p| p.port == port && p.web)
                {
                    crate::platform::open(&format!("http://localhost:{port}"));
                }
            }
            Hit::MorePorts => {
                let st = self.st();
                st.panel.more_ports = !st.panel.more_ports;
                st.window.request_redraw();
            }
            Hit::Panel => {}
        }
    }

    /// Aplica en vivo un cambio hecho desde Ajustes y lo guarda en config.toml.
    /// Devuelve `true` si la app debe cerrarse (reinicio tras actualizar).
    pub fn apply_setting(&mut self, change: crate::settings::Change) -> bool {
        use crate::settings::Change;
        match change {
            Change::None => return false,
            Change::CheckNow => {
                use crate::update::{CheckState, UpdateState as U};
                let proxy = self.proxy.clone();
                let st = self.st();
                match &st.update {
                    Some(U::Available(_) | U::Installed(_)) => return self.on_update_click(),
                    Some(U::Downloading(_) | U::Installing(_)) => {}
                    _ if st.update_check == CheckState::Checking => {}
                    _ => {
                        if matches!(st.update, Some(U::Failed(_))) {
                            st.update = None;
                            st.last_ws.num_cols = 0;
                        }
                        st.update_check = CheckState::Checking;
                        crate::update::check_now(proxy);
                        self.relayout();
                    }
                }
                return false;
            }
            Change::Theme | Change::Style => {
                let before = self.theme.style.interface;
                self.theme = self.cfg.theme();
                // Otra interfaz: otra letra para la interfaz (ver `Config::ui_font`).
                if self.theme.style.interface != before {
                    let (cfg, pt) = (self.cfg.clone(), self.st().font_pt);
                    let st = self.st();
                    st.glyphs = build_glyphs(&cfg, pt, st.scale);
                    st.last_ws.num_cols = 0;
                }
                // La forma puede cambiar el alto de la barra (y con él, el de los terminales).
                self.relayout();
            }
            Change::OpenLook => {
                self.open_look();
                return false;
            }
            Change::RenameTab => {
                let g = self.st().group;
                self.start_rename(g);
                return false;
            }
            Change::Fonts => {
                let (cfg, pt) = (self.cfg.clone(), self.st().font_pt);
                let st = self.st();
                st.glyphs = build_glyphs(&cfg, pt, st.scale);
                st.last_ws.num_cols = 0;
                self.relayout();
            }
            Change::FontSize => {
                let pt = self.cfg.font.size;
                self.set_font_pt(pt);
            }
            Change::LineHeight => {
                let lh = self.cfg.font.line_height;
                let st = self.st();
                st.glyphs.fonts.set_line_height(lh);
                st.glyphs.clear();
                self.relayout();
            }
            Change::Layout => self.relayout(),
            Change::TermOptions => {
                let tc = term_config(&self.cfg);
                let st = self.st();
                for tab in &st.tabs {
                    tab.term.lock().set_options(tc.clone());
                }
                if let Some(sample) = st.sample.as_mut() {
                    sample.term.set_options(tc);
                }
            }
            Change::Redraw | Change::Restart => {}
            Change::Updates => {
                let on = self.cfg.check_updates;
                crate::update::ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
                if on {
                    crate::update::start_checker(self.proxy.clone());
                }
            }
            Change::TestNotice => {
                // Aviso de muestra, con el sonido elegido y aunque los avisos estén desactivados.
                let st = self.st();
                let tab = st.tabs.get(st.active).map_or(0, |t| t.id);
                let dir = st
                    .tabs
                    .get(st.active)
                    .map(|t| crate::terminal::short_dir(&t.cwd_label))
                    .unwrap_or_default();
                let (title, body) = crate::notify::notice_text(
                    "Claude Code",
                    "Así se verá el aviso",
                    &dir,
                    std::time::Duration::from_secs(125),
                );
                let n = crate::notify::Notice {
                    tab,
                    title,
                    body,
                    sound: self.cfg.notifications.sound,
                };
                crate::notify::send(n, self.proxy.clone());
                return false;
            }
            Change::OpenFile => {
                if let Some(path) = Config::path() {
                    if !path.exists() {
                        let _ = self.cfg.save();
                    }
                    // En Windows un `.toml` no suele tener programa asociado: va al editor, como
                    // un Ctrl+clic en un archivo.
                    if cfg!(windows) {
                        self.open_link(crate::links::Target::File {
                            path,
                            line: None,
                            col: None,
                        });
                    } else {
                        crate::platform::open(&path.to_string_lossy());
                    }
                }
                return false;
            }
        }
        let saved = self.cfg.save();
        if let Some(settings) = self.st().settings.as_mut() {
            settings.save_failed = saved.is_err();
        }
        if let Err(e) = saved {
            log::warn!("No se pudo guardar la configuración: {e}");
        }
        self.st().window.request_redraw();
        false
    }

    /// Selecciona el bloque anterior/siguiente y hace scroll hasta él si no está a la vista.
    pub fn goto_block(&mut self, dir: i32) {
        let st = self.st();
        let Some(tab) = st.tabs.get_mut(st.active) else {
            return;
        };
        let mut term = tab.term.lock();
        let idle = tab.blocks.lock().unwrap().idle_current();
        // Navegables: todos menos el prompt actual si aún no ha ejecutado nada.
        let nav: Vec<blocks::Anchor> =
            blocks::anchors(&term, term.topmost_line(), term.bottommost_line())
                .into_iter()
                .filter(|a| Some(a.id) != idle)
                .collect();
        if nav.is_empty() {
            return;
        }
        let pos = tab
            .selected_block
            .and_then(|id| nav.iter().position(|a| a.id == id));
        let next = match (pos, dir < 0) {
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) => (i + 1 < nav.len()).then_some(i + 1),
            (None, true) => Some(nav.len() - 1),
            (None, false) => None,
        };
        match next {
            Some(i) => {
                let a = nav[i];
                tab.selected_block = Some(a.id);
                let offset = term.grid().display_offset() as i32;
                let row = a.line.0 + offset;
                if row < 0 || row >= term.screen_lines() as i32 {
                    let target = (-a.line.0).clamp(0, term.history_size() as i32);
                    term.scroll_display(Scroll::Delta(target - offset));
                }
            }
            // Pasado el último: vuelta al prompt.
            None => {
                tab.selected_block = None;
                term.scroll_display(Scroll::Bottom);
            }
        }
        drop(term);
        st.window.request_redraw();
    }

    /// Copia parte del bloque seleccionado (o del último comando ejecutado).
    pub fn copy_block(&mut self, part: BlockPart) {
        let st = self.st();
        let Some(tab) = st.tabs.get(st.active) else {
            return;
        };
        let term = tab.term.lock();
        let id = tab
            .selected_block
            .or_else(|| tab.blocks.lock().unwrap().last_run());
        let Some(range) = id.and_then(|id| blocks::find_block(&term, id)) else {
            return;
        };
        let text = match part {
            BlockPart::Output => range.output(&term),
            BlockPart::Command => range.command(&term),
            BlockPart::Whole => range.whole(&term),
        };
        drop(term);
        if !text.is_empty() {
            if let Err(e) = st.clipboard.set_contents(text) {
                log::warn!("portapapeles: {e}");
            }
        }
    }

    pub fn run_palette_item(&mut self, item: Item) -> bool {
        self.st().palette = None;
        self.st().window.request_redraw();
        match item {
            Item::Action(a) => self.run_action(a),
            Item::Theme(t) => {
                self.set_theme(t);
                false
            }
        }
    }

    /// Interfaz Flotante con píldoras: el ancho de cada pestaña, ajustado a lo que enseña (punto,
    /// logo del agente, nombre, estado y ×). Se mide aquí porque el reparto no puede medir texto.
    fn measure_tabs(&mut self) {
        use crate::sessions::Status;
        let style = self.theme.style;
        let st = self.st.as_mut().unwrap();
        // Con "Ancho: Máximo" se reparten la barra a partes iguales (y "Nombre" se nota).
        let fit = style.interface == crate::style::Interface::Floating
            && style.tabs == crate::style::Tabs::Pill
            && !self.cfg.tabs.position.vertical()
            && self.cfg.tabs.width != crate::tabs::Width::Max;
        if !fit {
            st.tab_fit.clear();
            return;
        }
        let s = st.scale;
        let close = (32.0 * s).round();
        let views = tab_views(st);
        st.tab_fit = views
            .iter()
            .map(|v| {
                let label = if v.exited {
                    format!("{} (terminado)", v.label)
                } else {
                    v.label.clone()
                };
                let lw: f32 = label
                    .chars()
                    .map(|c| st.glyphs.ui_advance(c, crate::atlas::UiVariant::Normal))
                    .sum();
                // Márgenes, punto, nombre y la ×.
                let mut w = (12.0 + 14.0 + 6.0) * s + lw + close;
                if v.agent.is_some() {
                    w += 24.0 * s;
                    if matches!(
                        v.status,
                        Status::Working | Status::Done | Status::Blocked(_)
                    ) {
                        w += 24.0 * s;
                    }
                }
                w.ceil()
            })
            .collect();
    }

    pub fn draw(&mut self) {
        self.measure_tabs();
        let geo = self.geometry();
        let opacity = self.cfg.window.opacity.clamp(0.0, 1.0);
        let show_blocks = self.cfg.terminal.blocks;
        let theme = &self.theme;
        let cfg = &self.cfg;
        let st = self.st.as_mut().unwrap();
        let (w, h) = st.renderer.size();
        let views = tab_views(st);
        let toggles = ui::Toggles {
            stats: st.stats.is_some(),
            changes: st.changes.open,
            agents: st.panel.agents,
            ports: st.panel.ports,
            maximized: cfg!(windows) && st.window.is_maximized(),
            changes_n: st
                .tabs
                .get(st.active)
                .and_then(|t| st.git.summary_for(t.id))
                .map(|c| c.files),
        };
        let now = Instant::now();
        let anim = crate::tabs::Anim {
            ms: now.duration_since(st.anim_epoch).as_millis() as u64,
            kind: cfg.tabs.working,
        };
        // Con las estadísticas abiertas no se dibujan los terminales ni el panel lateral.
        let stats_open = st.stats.is_some();
        let top = geo.tab_bar.as_ref().map_or(0.0, |t| t.height);
        let m = st.glyphs.fonts.metrics;
        let pad = (cfg.window.padding * st.scale).round();
        // Con Flotante, el margen interior de las tarjetas del terminal (ver `pane_geo`).
        let floating = theme.style.interface == crate::style::Interface::Floating;
        let (pad_x, pad_y) = if floating {
            (
                pad + (14.0 * st.scale).round(),
                pad + (6.0 * st.scale).round(),
            )
        } else {
            (pad, pad)
        };
        // Editor de aspecto: el panel a la derecha y, a la izquierda, el terminal (o la muestra,
        // del ancho que queda a la vista).
        let look = st
            .look
            .as_ref()
            .filter(|_| !stats_open)
            .map(|l| (l.regions(w, st.scale), l.sample));
        let showing_sample = look.is_some_and(|(_, sample)| sample);
        if let Some((rg, true)) = look {
            let cols = (((rg.x - geo.area[0] - 2.0 * pad_x) / m.width)
                .floor()
                .max(20.0)) as usize;
            if st.sample.as_ref().is_none_or(|s| s.cols != cols) {
                st.sample = Some(crate::look::Sample::new(cfg, cols));
            }
        }
        let gaps = theme.style.gaps(st.scale);
        let chrome = geo.chrome;
        // Hasta dónde se ve el terminal: con el editor de aspecto abierto, hasta su panel.
        // (Solo con Flotante: con la Clásica el editor tapa el terminal como siempre.)
        let visible_right = look
            .filter(|_| chrome.floating)
            .map_or(f32::INFINITY, |(rg, _)| rg.x);

        for _attempt in 0..2 {
            st.out.clear();
            st.fg.clear();
            st.glyphs.overflowed = false;
            let mut p = Painter {
                out: &mut st.out,
                fg: &mut st.fg,
                glyphs: &mut st.glyphs,
                queue: &st.renderer.queue,
                atlas: &st.renderer.atlas,
            };
            // Interfaz Flotante: cada terminal es una tarjeta sobre el fondo de la ventana.
            if chrome.floating && !stats_open {
                let cards: Vec<[f32; 4]> = if showing_sample {
                    vec![geo.area]
                } else {
                    geo.panes.iter().map(|pane| pane.rect).collect()
                };
                for [x, y, cw, chh] in cards {
                    let cw = cw.min(visible_right - x);
                    if cw <= 0.0 {
                        continue;
                    }
                    // La opacidad de la ventana, también en las tarjetas del terminal.
                    p.card_alpha(
                        x,
                        y,
                        cw,
                        chh,
                        (theme.background, opacity),
                        theme.foreground,
                        chrome.radius,
                        st.scale,
                    );
                }
            }
            // Fondo con degradado: un tinte del acento arriba que se desvanece.
            if theme.style.background == crate::style::Background::Gradient && !stats_open {
                let [ax, ay, aw, ah] = geo.area;
                p.gradient_rounded(
                    ax,
                    ay,
                    aw,
                    (ah * 0.7).round(),
                    theme.accent,
                    ui::GRADIENT,
                    theme.accent,
                    0.0,
                    chrome.radius,
                );
            }
            if let Some(tb) = geo.tab_bar.as_ref().filter(|_| !stats_open) {
                tb.draw(
                    &mut p,
                    theme,
                    w,
                    &views,
                    st.group,
                    st.hover,
                    st.scale,
                    st.update.as_ref(),
                    toggles,
                    anim,
                    &mut ui::TabsExtra {
                        marquee: &mut st.marquee,
                        rename: st.rename.as_ref(),
                        now,
                    },
                );
            }
            let multi = geo.panes.len() > 1 && !stats_open && !showing_sample;
            st.git_chips.clear();
            if let (true, Some(sample)) = (showing_sample, st.sample.as_ref()) {
                let y0 = top + pad_y;
                let rows = Rows::layout(
                    &sample.term,
                    &sample.blocks,
                    sample.lines,
                    m.height,
                    gaps,
                    f32::INFINITY,
                );
                let v = TermView {
                    x: geo.area[0] + pad_x,
                    y: y0,
                    lines: sample.lines,
                    focused: st.focused,
                    cursor_on: st.cursor_on || !st.focused,
                    rows,
                    room: f32::INFINITY,
                };
                let (term, bs) = (&sample.term, &sample.blocks);
                view::draw_prompts(
                    &mut p,
                    term,
                    theme,
                    &v,
                    bs,
                    Some(crate::look::SAMPLE_BRANCH),
                    None,
                    st.scale,
                );
                view::draw_terminal(&mut p, term, theme, &v, Some(bs), None);
                if cfg.terminal.autosuggest {
                    view::draw_ghost(&mut p, term, theme, &v, crate::look::SAMPLE_GHOST);
                }
                if show_blocks {
                    let span = (geo.area[0], (geo.area[0] + geo.area[2]).min(visible_right));
                    view::draw_blocks(&mut p, term, theme, &v, bs, None, span, st.scale);
                }
            }
            for pane in geo.panes.iter().filter(|_| !stats_open && !showing_sample) {
                let Some(tab) = st.tabs.get(pane.idx) else {
                    continue;
                };
                let is_focus = pane.idx == st.active;
                let term = tab.term.lock();
                let mut bs = tab.blocks.lock().unwrap();
                let v = TermView {
                    x: pane.x,
                    y: pane.y,
                    lines: pane.ws.num_lines as usize,
                    focused: is_focus
                        && st.focused
                        && st.palette.is_none()
                        && st.settings.is_none(),
                    cursor_on: st.cursor_on || !st.focused,
                    rows: pane.rows(&term, &bs, m.height, gaps),
                    room: pane.room(),
                };
                // Rama de git para la etiqueta del prompt de Garbo (solo mientras se escribe) y, a
                // su lado, cuántos cambios hay sin confirmar.
                // Por ssh, la carpeta de este equipo no es la del servidor: sin rama.
                let branch = bs
                    .idle_current()
                    .filter(|id| bs.styled(*id) && !tab.session.remote)
                    .and_then(|_| bs.shell_cwd())
                    .and_then(|d| crate::complete::git_branch(&d));
                // Se apunta en el bloque: la etiqueta de los bloques "Etiquetas" la enseña también
                // cuando la orden ya terminó.
                if let Some(id) = bs.idle_current() {
                    bs.set_branch(id, branch.clone());
                }
                let summary = branch.as_ref().and_then(|_| st.git.summary_for(tab.id));
                let chip = view::draw_prompts(
                    &mut p,
                    &term,
                    theme,
                    &v,
                    &bs,
                    branch.as_deref(),
                    summary,
                    st.scale,
                );
                if let Some(r) = chip {
                    st.git_chips.push((tab.id, r));
                }
                let search = st.search.as_mut().filter(|s| s.tab == tab.id);
                let hl = search.map(|s| s.highlights(&term));
                view::draw_terminal(&mut p, &term, theme, &v, Some(&bs), hl.as_ref());
                if let Some((_, _, Some(found))) = st.link.as_ref().filter(|l| l.0 == tab.id) {
                    view::draw_underline(
                        &mut p,
                        &term,
                        theme,
                        &v,
                        found.start,
                        found.end,
                        st.scale,
                    );
                }
                let start = bs.input_start;
                // Menú de completado: se refina con lo que se escribe y se cierra si deja de aplicar.
                // Por ssh no hay menú (listaría lo de este equipo, ver `Tab::completions`).
                if tab.session.remote && st.menu.as_ref().is_some_and(|m| m.tab == tab.id) {
                    st.menu = None;
                }
                if is_focus && !tab.session.remote && (st.menu.is_some() || st.menu_auto.is_some())
                {
                    let input = crate::terminal::input_for(&term, start);
                    let cwd = input.as_ref().and_then(|_| bs.shell_cwd());
                    if let Some(menu) = st.menu.as_mut() {
                        let keep = menu.tab == tab.id
                            && input
                                .as_deref()
                                .is_some_and(|i| menu.refresh(i, cwd.as_deref()));
                        if !keep {
                            st.menu = None;
                        }
                    } else if let Some((before, since)) = &st.menu_auto {
                        match &input {
                            Some(now) if now != before => {
                                let comp = crate::complete::complete(now, cwd.as_deref())
                                    .filter(|c| c.opens_as_you_type());
                                st.menu = comp.map(|c| {
                                    crate::complete::Menu::new(tab.id, now.clone(), c)
                                        .opened_while_typing()
                                });
                                st.menu_auto = None;
                            }
                            _ if since.elapsed() > std::time::Duration::from_secs(1) => {
                                st.menu_auto = None
                            }
                            _ => {}
                        }
                    }
                }
                // Sugerencia en gris tras el cursor (no con el menú abierto).
                // Ni sugerencia: saldría del historial y las carpetas de este equipo.
                if cfg.terminal.autosuggest && v.focused && st.menu.is_none() && !tab.session.remote
                {
                    let cwd = start.and_then(|_| bs.shell_cwd());
                    if let Some(rest) =
                        crate::terminal::suggestion_for(&term, start, cwd.as_deref())
                    {
                        view::draw_ghost(&mut p, &term, theme, &v, &rest);
                    }
                }
                if show_blocks {
                    view::draw_blocks(
                        &mut p,
                        &term,
                        theme,
                        &v,
                        &bs,
                        tab.selected_block,
                        (
                            pane.rect[0],
                            (pane.rect[0] + pane.rect[2]).min(visible_right),
                        ),
                        st.scale,
                    );
                }
                if let Some(menu) = st.menu.as_mut().filter(|m| v.focused && m.tab == tab.id) {
                    // Por encima del texto del terminal.
                    p.out.append(p.fg);
                    view::draw_menu(&mut p, &term, theme, &v, menu, st.scale);
                }
                if let Some(search) = st.search.as_mut().filter(|s| s.tab == tab.id) {
                    p.out.append(p.fg);
                    let area_w = pane.ws.num_cols as f32 * p.metrics().width;
                    search.draw(&mut p, theme, pane.x, pane.y, area_w, st.scale);
                }
            }
            // Con varios paneles: líneas divisorias y los que no tienen el foco, algo atenuados.
            if multi {
                p.out.append(p.fg);
                for pane in geo.panes.iter().filter(|p| p.idx != st.active) {
                    let [x, y, w, h] = pane.rect;
                    p.rounded(x, y, w, h, theme.background, 0.35, chrome.radius);
                }
                let line = st.scale.round().max(1.0);
                for d in &geo.dividers {
                    let [x, y, w, h] = d.rect;
                    let dragging = st.drag.as_ref().is_some_and(|g| g.0 == d.path);
                    // Con la Flotante, el hueco entre tarjetas ya separa: la línea solo al
                    // arrastrar.
                    if chrome.floating && !dragging {
                        continue;
                    }
                    let (c, a) = if dragging {
                        (theme.accent, 1.0)
                    } else {
                        (theme.foreground, 0.16)
                    };
                    if d.side {
                        p.rect((x + (w - line) / 2.0).round(), y, line, h, c, a);
                    } else {
                        p.rect(x, (y + (h - line) / 2.0).round(), w, line, c, a);
                    }
                }
            }
            if let Some(r) = geo.changes.filter(|_| !stats_open) {
                let status = st
                    .changes
                    .root
                    .as_ref()
                    .and_then(|root| st.git.status(root));
                let bottom_free = r.1 + r.3 >= h - 0.5;
                st.changes
                    .draw(&mut p, theme, status, r, st.scale, bottom_free, &chrome);
            }
            if let Some((px, py, pw, ph)) = geo.panel.filter(|_| !stats_open) {
                let previewed: Vec<usize> = st.panel.previewed(now).collect();
                let rows: Vec<crate::panel::AgentRow> = st
                    .tabs
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.session.agent.is_some())
                    .enumerate()
                    .filter_map(|(k, (i, t))| {
                        Some(crate::panel::AgentRow {
                            label: t.dir_label(),
                            agent: t.session.agent?,
                            status: t.session.status,
                            detail: t.session.detail.as_deref(),
                            since: t.session.since,
                            active: i == st.active,
                            subagents: &t.subagents.list,
                            preview: previewed.contains(&k).then(|| {
                                view::preview_lines(
                                    &t.term.lock(),
                                    theme,
                                    crate::panel::PREVIEW_LINES,
                                    t.session.agent.and_then(|agent| agent.subagents),
                                )
                            }),
                        })
                    })
                    .collect();
                st.panel.draw(
                    &mut p, theme, &rows, px, py, pw, ph, st.scale, now, anim, &chrome,
                );
            }
            if let Some(stats) = st.stats.as_mut() {
                let top = geo.tab_bar.as_ref().map_or(0.0, |t| t.height);
                // Flotante: el apartado va en una tarjeta, como el terminal.
                if chrome.floating {
                    let m = chrome.margin;
                    p.card(
                        m,
                        top,
                        w - 2.0 * m,
                        h - top - m,
                        theme.background,
                        theme.foreground,
                        chrome.radius,
                        st.scale,
                    );
                }
                stats.draw(&mut p, theme, w, h, top, st.scale);
                // La barra de pestañas, encima: tapa lo que se desplazó hacia arriba.
                p.out.append(p.fg);
                if let Some(tb) = &geo.tab_bar {
                    tb.draw(
                        &mut p,
                        theme,
                        w,
                        &views,
                        st.group,
                        st.hover,
                        st.scale,
                        st.update.as_ref(),
                        toggles,
                        anim,
                        &mut ui::TabsExtra {
                            marquee: &mut st.marquee,
                            rename: st.rename.as_ref(),
                            now,
                        },
                    );
                }
            }
            if let Some(pal) = &st.palette {
                // El texto del terminal va debajo del overlay: se vuelca antes de pintarlo.
                p.out.append(p.fg);
                let top = geo.tab_bar.as_ref().map_or(0.0, |t| t.height);
                ui::draw_palette(&mut p, theme, pal, w, h, top, st.scale);
            }
            if let Some(editor) = st.look.as_mut().filter(|_| !stats_open) {
                // Encima del texto del terminal.
                p.out.append(p.fg);
                editor.draw(&mut p, theme, cfg, w, h, top, st.scale);
            }
            if let Some(set) = st.settings.as_mut() {
                p.out.append(p.fg);
                let top = geo.tab_bar.as_ref().map_or(0.0, |t| t.height);
                let upd = crate::settings::UpdateView {
                    check: &st.update_check,
                    state: st.update.as_ref(),
                };
                set.draw(&mut p, theme, cfg, &upd, w, h, top, st.scale);
            }
            p.finish();
            if !st.glyphs.overflowed {
                break;
            }
        }
        // Con la Flotante, el fondo de la ventana es el de la barra y el terminal va en tarjetas.
        let ground = if chrome.floating {
            theme.tab_bar
        } else {
            theme.background
        };
        st.renderer.render(&st.out, ground.to_f32(opacity));
    }
}

/// Lo que la barra enseña de cada pestaña: nombre, agente y estado (el del panel más urgente).
fn tab_views(st: &State) -> Vec<ui::TabView> {
    st.groups
        .iter()
        .filter_map(|g| {
            let panes: Vec<&Tab> = g
                .layout
                .leaves()
                .iter()
                .filter_map(|id| st.tabs.iter().find(|t| t.id == *id))
                .collect();
            let focus = panes.iter().find(|t| t.id == g.focus).or(panes.first())?;
            // El indicador de la pestaña, del panel más urgente: te necesita, trabaja, terminó.
            use crate::sessions::Status;
            let rank = |t: &&&Tab| match (t.session.agent, t.session.status) {
                (None, _) => 0,
                (_, Status::Blocked(_)) => 4,
                (_, Status::Working) => 3,
                (_, Status::Done) => 2,
                _ => 1,
            };
            let lead = panes.iter().max_by_key(rank).unwrap_or(focus);
            let n = panes.len();
            Some(ui::TabView {
                label: match &g.name {
                    Some(name) => name.clone(),
                    None if n > 1 => format!("{} (+{})", focus.label(), n - 1),
                    None => focus.label().to_string(),
                },
                exited: focus.exited,
                agent: lead.session.agent,
                status: lead.session.status,
            })
        })
        .collect()
}

/// Windows: lo pegado en el prompt de PowerShell (ver `input::powershell_paste`).
#[cfg(windows)]
fn powershell_paste(tab: &Tab, text: &str) -> Option<Vec<u8>> {
    // Primero el candado de `blocks` y soltarlo: `edit_context` lo vuelve a tomar.
    let win32 = tab.blocks.lock().unwrap().win32_input;
    let prompt = win32
        && tab.edit_context() == crate::edit::Context::Prompt(crate::shell::Shell::PowerShell);
    prompt.then(|| crate::input::powershell_paste(text))
}

#[cfg(not(windows))]
fn powershell_paste(_tab: &Tab, _text: &str) -> Option<Vec<u8>> {
    None
}

#[cfg(all(unix, not(target_os = "macos")))]
fn make_clipboards(window: &Window) -> (Box<dyn ClipboardProvider>, Box<dyn ClipboardProvider>) {
    use copypasta::x11_clipboard::{Clipboard, Primary, X11ClipboardContext};
    use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    if let Ok(handle) = window.display_handle() {
        if let RawDisplayHandle::Wayland(wl) = handle.as_raw() {
            // SAFETY: el display de Wayland vive mientras viva la ventana (dueña de la app).
            let (primary, clipboard) = unsafe {
                copypasta::wayland_clipboard::create_clipboards_from_external(wl.display.as_ptr())
            };
            return (Box::new(clipboard), Box::new(primary));
        }
    }
    let clip: Box<dyn ClipboardProvider> = match X11ClipboardContext::<Clipboard>::new() {
        Ok(c) => Box::new(c),
        Err(_) => Box::new(copypasta::nop_clipboard::NopClipboardContext::new().unwrap()),
    };
    let prim: Box<dyn ClipboardProvider> = match X11ClipboardContext::<Primary>::new() {
        Ok(c) => Box::new(c),
        Err(_) => Box::new(copypasta::nop_clipboard::NopClipboardContext::new().unwrap()),
    };
    (clip, prim)
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn make_clipboards(_window: &Window) -> (Box<dyn ClipboardProvider>, Box<dyn ClipboardProvider>) {
    let clip: Box<dyn ClipboardProvider> = Box::new(copypasta::ClipboardContext::new().unwrap());
    let prim: Box<dyn ClipboardProvider> =
        Box::new(copypasta::nop_clipboard::NopClipboardContext::new().unwrap());
    (clip, prim)
}
