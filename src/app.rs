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
use crate::view::{self, Painter, TermView};
use alacritty_terminal::event::WindowSize;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::TermMode;
use copypasta::ClipboardProvider;
use std::sync::Arc;
use std::time::Instant;
use winit::event_loop::EventLoopProxy;
use winit::keyboard::ModifiersState;
use winit::window::Window;

pub fn pt_to_px(pt: f32, scale: f32) -> f32 {
    pt * scale * 4.0 / 3.0
}

/// Fuentes del terminal y de la interfaz con un único escaneo del sistema.
pub fn build_glyphs(cfg: &Config, font_pt: f32, scale: f32) -> GlyphCache {
    let db = FontSet::system_db();
    let f = &cfg.font;
    let term = FontSet::from_db(&db, &f.family, &f.fallback, pt_to_px(font_pt, scale), f.line_height, true);
    let ui_px = pt_to_px(f.ui_size, scale);
    let ui = if f.ui_family.trim().is_empty() {
        FontSet::from_db(&db, &f.family, &f.fallback, ui_px, 1.0, true)
    } else {
        FontSet::from_db(&db, &f.ui_family, &f.ui_fallback(), ui_px, 1.0, false)
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
    pub update: Option<crate::update::UpdateState>,
    /// Búsqueda manual de actualizaciones (Ajustes).
    pub update_check: crate::update::CheckState,
    pub panel: crate::panel::PanelState,
    pub anim_epoch: Instant,
    pub last_anim: Instant,
    pub clipboard: Box<dyn ClipboardProvider>,
    pub selection_clip: Box<dyn ClipboardProvider>,
    pub focused: bool,
    pub cursor_on: bool,
    pub next_blink: Instant,
    pub scroll_accum: f32,
    pub last_ws: WindowSize,
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
    /// Rectángulo del panel con el foco (con su margen).
    pub pane: crate::panes::Rect,
    /// Paneles de la pestaña activa.
    pub panes: Vec<PaneGeo>,
    pub dividers: Vec<crate::panes::Divider>,
    /// Zona de los terminales (sin barra de pestañas ni panel lateral).
    pub area: crate::panes::Rect,
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

impl App {
    pub fn new(cfg: Config, proxy: EventLoopProxy<UserEvent>) -> Self {
        let theme = cfg.theme();
        let keys = Keybindings::new(&cfg.keybindings);
        Self { cfg, theme, keys, proxy, st: None }
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
            cursor_icon: winit::window::CursorIcon::Default,
            palette: None,
            search: None,
            menu: None,
            menu_auto: None,
            settings: None,
            update: None,
            update_check: Default::default(),
            panel: Default::default(),
            anim_epoch: now,
            last_anim: now,
            clipboard,
            selection_clip,
            focused: true,
            cursor_on: true,
            next_blink: now,
            scroll_accum: 0.0,
            last_ws: WindowSize { num_lines: 0, num_cols: 0, cell_width: 0, cell_height: 0 },
            out: Vec::with_capacity(8192),
            fg: Vec::with_capacity(8192),
        });
        self.new_tab();
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
                    update::start_install(rel, proxy);
                } else {
                    let _ = std::process::Command::new("xdg-open").arg(&rel.page).spawn();
                }
            }
            Some(U::Installed(_)) => {
                if std::process::Command::new(update::restart_path()).spawn().is_ok() {
                    return true;
                }
            }
            Some(U::Failed(_)) => {
                let url = format!("https://github.com/{}/releases/latest", update::REPO);
                let _ = std::process::Command::new("xdg-open").arg(url).spawn();
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
        let integrated = !self.cfg.window.decorations;
        let tab_bar = (integrated || self.cfg.window.always_show_tabs || n_tabs > 1)
            .then(|| TabBar::layout(n_tabs, w, st.scale, integrated, st.update.is_some()));
        let top = tab_bar.as_ref().map_or(0.0, |t| t.height);
        let panel_w = if st.panel.visible() { crate::panel::PanelState::width(st.scale, w) } else { 0.0 };
        let panel = (panel_w > 0.0).then(|| (w - panel_w, top, panel_w, h - top));
        let area = [0.0, top, w - panel_w, h - top];
        let gap = crate::panes::gap(st.scale);
        let (panes, dividers) = match st.groups.get(st.group) {
            Some(g) => (self.pane_geos(&g.layout, area), g.layout.dividers(area, gap)),
            None => (Vec::new(), Vec::new()),
        };
        let focus = panes.iter().find(|p| p.idx == st.active).or(panes.first()).copied();
        let fallback = self.pane_geo(usize::MAX, area);
        let f = focus.unwrap_or(fallback);
        Geometry { tab_bar, x: f.x, y: f.y, ws: f.ws, panel, pane: f.rect, panes, dividers, area }
    }

    /// Dónde empiezan las celdas de un panel y cuántas caben (con el margen de la ventana).
    fn pane_geo(&self, idx: usize, rect: crate::panes::Rect) -> PaneGeo {
        let st = self.st.as_ref().unwrap();
        let m = st.glyphs.fonts.metrics;
        let pad = (self.cfg.window.padding * st.scale).round();
        let cols = ((rect[2] - 2.0 * pad) / m.width).floor().max(2.0);
        let lines = ((rect[3] - 2.0 * pad) / m.height).floor().max(1.0);
        PaneGeo {
            idx,
            rect,
            x: rect[0] + pad,
            y: rect[1] + pad,
            ws: WindowSize { num_lines: lines as u16, num_cols: cols as u16, cell_width: m.width as u16, cell_height: m.height as u16 },
        }
    }

    fn pane_geos(&self, layout: &crate::panes::Layout, area: crate::panes::Rect) -> Vec<PaneGeo> {
        let st = self.st.as_ref().unwrap();
        layout
            .rects(area, crate::panes::gap(st.scale))
            .into_iter()
            .filter_map(|(id, r)| st.tabs.iter().position(|t| t.id == id).map(|i| self.pane_geo(i, r)))
            .collect()
    }

    /// Pone `active` en el terminal con el foco de la pestaña activa.
    fn sync_active(&mut self) {
        let st = self.st();
        if let Some(i) = st.groups.get(st.group).and_then(|g| st.tabs.iter().position(|t| t.id == g.focus)) {
            st.active = i;
        }
    }

    /// ¿Tiene la pestaña activa más de un panel?
    pub fn has_splits(&self) -> bool {
        let Some(st) = self.st.as_ref() else { return false };
        st.groups.get(st.group).is_some_and(|g| matches!(g.layout, crate::panes::Layout::Split { .. }))
    }

    /// Recalcula filas/columnas de cada panel y avisa a los PTY que cambiaron (a todos si
    /// `last_ws.num_cols` es 0: cambió la fuente, la escala…).
    pub fn relayout(&mut self) {
        let geo = self.geometry();
        let area = geo.area;
        let sizes: Vec<(usize, WindowSize)> = {
            let st = self.st.as_ref().unwrap();
            st.groups.iter().flat_map(|g| self.pane_geos(&g.layout, area)).map(|p| (p.idx, p.ws)).collect()
        };
        let same = |a: &WindowSize, b: &WindowSize| {
            (a.num_lines, a.num_cols, a.cell_width, a.cell_height) == (b.num_lines, b.num_cols, b.cell_width, b.cell_height)
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
        let ws = if ws_guess.num_cols == 0 { WindowSize { num_lines: 24, num_cols: 80, ..ws_guess } } else { ws_guess };
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
        let Some(focus) = st.groups.get(st.group).map(|g| g.focus) else { return };
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
        let rects: Vec<(u64, crate::panes::Rect)> = geo.panes.iter().map(|p| (st.tabs[p.idx].id, p.rect)).collect();
        let Some(from) = st.tabs.get(st.active).map(|t| t.id) else { return };
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
        let Some(id) = st.tabs.get(idx).map(|t| t.id) else { return st.tabs.is_empty() };
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
                    if g < st.group || st.group >= st.groups.len() {
                        st.group = st.group.saturating_sub(1);
                    }
                }
            }
        }
        if st.tabs.is_empty() || st.groups.is_empty() {
            return true;
        }
        self.sync_active();
        self.st().last_ws.num_cols = 0;
        self.relayout();
        self.update_title();
        false
    }

    /// Cierra una pestaña de la barra con todos sus paneles.
    pub fn close_tab(&mut self, g: usize) -> bool {
        let Some(ids) = self.st().groups.get(g).map(|g| g.layout.leaves()) else { return false };
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
        let Some(id) = st.tabs.get(idx).map(|t| t.id) else { return };
        let Some(g) = st.groups.iter().position(|g| g.layout.contains(id)) else { return };
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
        let Some(id) = self.st().groups.get(g).map(|g| g.focus) else { return };
        if let Some(i) = self.st().tabs.iter().position(|t| t.id == id) {
            self.select_tab(i);
        }
    }

    pub fn update_title(&mut self) {
        let st = self.st();
        if let Some(tab) = st.tabs.get(st.active) {
            st.window.set_title(&format!("{} — Garbo", tab.label()));
        }
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
        let Some(tab) = st.tabs.get(st.active) else { return };
        let text = tab.term.lock().selection_to_string();
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            let cb = if to_primary { &mut st.selection_clip } else { &mut st.clipboard };
            if let Err(e) = cb.set_contents(text) {
                log::warn!("portapapeles: {e}");
            }
        }
    }

    /// Abre lo encontrado con Ctrl+clic (ver `links.rs`).
    pub fn open_link(&mut self, target: crate::links::Target) {
        use crate::links::{Open, Target};
        let cwd = self.active_tab().and_then(Tab::cwd);
        let open = match target {
            Target::Url(url) => Open::Spawn(vec!["xdg-open".into(), url]),
            Target::File { path, line, col } => {
                let editor = std::env::var("VISUAL").ok().filter(|e| !e.is_empty()).or_else(|| std::env::var("EDITOR").ok());
                crate::links::open_file(&path, line, col, editor.as_deref())
            }
        };
        match open {
            Open::Spawn(cmd) => {
                let mut c = std::process::Command::new(&cmd[0]);
                c.args(&cmd[1..]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
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
        let cb = if from_primary { &mut st.selection_clip } else { &mut st.clipboard };
        match cb.get_contents() {
            Ok(text) if !text.is_empty() => self.paste_text(&text),
            // Sin texto: quizá una imagen (captura de pantalla). Se guarda y se pega su ruta.
            _ if !from_primary => {
                let Some(id) = st.tabs.get(st.active).map(|t| t.id) else { return };
                let _ = std::thread::Builder::new().name("pegar-imagen".into()).spawn(move || {
                    let r = crate::clipboard::save_image();
                    let _ = proxy.send_event(UserEvent::PasteImage(id, r));
                });
            }
            _ => {}
        }
    }

    pub fn paste_text(&mut self, text: &str) {
        let Some(id) = self.active_tab().map(|t| t.id) else { return };
        self.paste_text_to(id, text);
    }

    /// Pega en la pestaña `id` (respetando el modo "bracketed paste" del programa).
    pub fn paste_text_to(&mut self, id: u64, text: &str) {
        let st = self.st();
        let Some(tab) = st.tabs.iter().find(|t| t.id == id) else { return };
        let mode = *tab.term.lock().mode();
        let bytes = if mode.contains(TermMode::BRACKETED_PASTE) {
            // Evita que el texto pegado cierre el modo bracketed por su cuenta.
            let clean = text.replace("\x1b[201~", "").replace('\x1b', "");
            format!("\x1b[200~{clean}\x1b[201~")
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };
        tab.input(bytes.into_bytes());
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
        let old_font = (self.cfg.font.family.clone(), self.cfg.font.fallback.clone(), self.cfg.font.line_height);
        self.cfg = Config::load();
        self.theme = self.cfg.theme();
        self.keys = Keybindings::new(&self.cfg.keybindings);
        let cfg = self.cfg.clone();
        let st = self.st();
        let tc = term_config(&cfg);
        for tab in &st.tabs {
            tab.term.lock().set_options(tc.clone());
        }
        if old_font != (cfg.font.family.clone(), cfg.font.fallback.clone(), cfg.font.line_height) {
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
        self.st().window.request_redraw();
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
                let j = if action == Action::MoveTabLeft { i.checked_sub(1) } else { Some(i + 1) };
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
                let has_sel = st.tabs.get(st.active).is_some_and(|t| t.term.lock().selection.as_ref().is_some_and(|s| !s.is_empty()));
                let has_block = st.tabs.get(st.active).is_some_and(|t| t.selected_block.is_some());
                if !has_sel && has_block {
                    self.copy_block(BlockPart::Whole);
                } else {
                    self.copy_selection(false);
                }
            }
            Action::Settings => {
                let st = self.st();
                st.palette = None;
                st.settings = if st.settings.is_some() { None } else { Some(crate::settings::Settings::new()) };
                st.window.request_redraw();
            }
            Action::ToggleAgents => self.toggle_panel(true),
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
                st.palette = if st.palette.is_some() { None } else { Some(Palette::new(keys)) };
                st.window.request_redraw();
            }
            Action::NextTheme => {
                let names = Theme::NAMES;
                let i = names.iter().position(|n| *n == self.cfg.theme).map_or(0, |i| (i + 1) % names.len());
                self.set_theme(names[i]);
            }
        }
        false
    }

    /// Recalcula agentes y directorios de todas las pestañas. Barato: una llamada al sistema por
    /// pestaña, y la identificación del proceso como mucho cada 700 ms.
    pub fn refresh_sessions(&mut self) -> bool {
        let now = Instant::now();
        let Some(st) = self.st.as_mut() else { return false };
        let focused = st.focused;
        let mut changed = false;
        let agents_panel = st.panel.agents;
        let ncfg = &self.cfg.notifications;
        let min_work = std::time::Duration::from_secs(ncfg.min_work_secs);
        let visible: Vec<u64> = st.groups.get(st.group).map(|g| g.layout.leaves()).unwrap_or_default();
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
                    notice = Some(crate::notify::notice_text(agent.name, tab.label(), &dir, worked));
                }
            }
            let blocked = match tab.session.status {
                crate::sessions::Status::Blocked(kind) => Some(kind),
                _ => None,
            };
            if tab.notice.blocked_now(blocked.is_some(), now) && ncfg.blocked && !viewing {
                if let (Some(agent), Some(kind)) = (agent, blocked) {
                    notice = Some(crate::notify::blocked_text(agent.name, kind, tab.session.detail.as_deref(), tab.label(), &dir));
                }
            }
            if let Some((title, body)) = notice {
                // Uno por pestaña: el anterior sobra.
                if let Some(old) = tab.notice.shown.take() {
                    crate::notify::close(old);
                }
                crate::notify::send(crate::notify::Notice { tab: tab.id, title, body, sound: ncfg.sound }, self.proxy.clone());
            }
        }
        // Animación del indicador "trabajando" (~7 fps, solo mientras algún agente trabaja).
        let working = st.tabs.iter().any(|t| t.session.is_working());
        if working && now.duration_since(st.last_anim).as_millis() >= 140 {
            st.last_anim = now;
            changed = true;
        }
        if changed {
            st.window.request_redraw();
        }
        changed
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
                push(now + std::time::Duration::from_millis(150));
            } else if now.duration_since(tab.last_output) < crate::sessions::WORK_WINDOW {
                push(tab.last_output + crate::sessions::WORK_WINDOW + std::time::Duration::from_millis(60));
            }
        }
        if st.panel.agents && st.tabs.iter().any(|t| t.session.agent.is_some()) {
            push(now + std::time::Duration::from_secs(1));
        }
        // Despliegue de una fila del panel: fotogramas solo mientras dura la animación.
        if st.panel.animating(now) {
            push(now + std::time::Duration::from_millis(12));
        }
        next
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
                let tab = self.st().tabs.iter().enumerate().filter(|(_, t)| t.session.agent.is_some()).nth(i).map(|(j, _)| j);
                if let Some(j) = tab {
                    self.select_tab(j);
                }
            }
            Hit::Port(port) => {
                if self.st().panel.ports_data.iter().any(|p| p.port == port && p.web) {
                    let _ = std::process::Command::new("xdg-open").arg(format!("http://localhost:{port}")).spawn();
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
            Change::Theme => self.theme = self.cfg.theme(),
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
                for tab in &self.st().tabs {
                    tab.term.lock().set_options(tc.clone());
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
                let dir = st.tabs.get(st.active).map(|t| crate::terminal::short_dir(&t.cwd_label)).unwrap_or_default();
                let (title, body) =
                    crate::notify::notice_text("Claude Code", "Así se verá el aviso", &dir, std::time::Duration::from_secs(125));
                let n = crate::notify::Notice { tab, title, body, sound: self.cfg.notifications.sound };
                crate::notify::send(n, self.proxy.clone());
                return false;
            }
            Change::OpenFile => {
                if let Some(path) = Config::path() {
                    if !path.exists() {
                        let _ = self.cfg.save();
                    }
                    let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
                }
                return false;
            }
        }
        if let Err(e) = self.cfg.save() {
            log::warn!("No se pudo guardar la configuración: {e}");
        }
        self.st().window.request_redraw();
        false
    }

    /// Selecciona el bloque anterior/siguiente y hace scroll hasta él si no está a la vista.
    pub fn goto_block(&mut self, dir: i32) {
        let st = self.st();
        let Some(tab) = st.tabs.get_mut(st.active) else { return };
        let mut term = tab.term.lock();
        let idle = tab.blocks.lock().unwrap().idle_current();
        // Navegables: todos menos el prompt actual si aún no ha ejecutado nada.
        let nav: Vec<blocks::Anchor> = blocks::anchors(&term, term.topmost_line(), term.bottommost_line())
            .into_iter()
            .filter(|a| Some(a.id) != idle)
            .collect();
        if nav.is_empty() {
            return;
        }
        let pos = tab.selected_block.and_then(|id| nav.iter().position(|a| a.id == id));
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
        let Some(tab) = st.tabs.get(st.active) else { return };
        let term = tab.term.lock();
        let id = tab.selected_block.or_else(|| tab.blocks.lock().unwrap().last_run());
        let Some(range) = id.and_then(|id| blocks::find_block(&term, id)) else { return };
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

    pub fn draw(&mut self) {
        let geo = self.geometry();
        let opacity = self.cfg.window.opacity.clamp(0.0, 1.0);
        let show_blocks = self.cfg.terminal.blocks;
        let theme = &self.theme;
        let cfg = &self.cfg;
        let st = self.st.as_mut().unwrap();
        let (w, h) = st.renderer.size();
        let views: Vec<ui::TabView> = st
            .groups
            .iter()
            .filter_map(|g| {
                let panes: Vec<&Tab> = g.layout.leaves().iter().filter_map(|id| st.tabs.iter().find(|t| t.id == *id)).collect();
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
                    label: if n > 1 { format!("{} (+{})", focus.label(), n - 1) } else { focus.label().to_string() },
                    exited: focus.exited,
                    agent: lead.session.agent,
                    status: lead.session.status,
                })
            })
            .collect();
        let toggles = ui::Toggles { agents: st.panel.agents, ports: st.panel.ports };
        let now = Instant::now();
        let phase = (now.duration_since(st.anim_epoch).as_millis() / 300) as usize;

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
            if let Some(tb) = &geo.tab_bar {
                tb.draw(&mut p, theme, w, &views, st.group, st.hover, st.scale, st.update.as_ref(), toggles, phase);
            }
            let multi = geo.panes.len() > 1;
            for pane in &geo.panes {
                let Some(tab) = st.tabs.get(pane.idx) else { continue };
                let is_focus = pane.idx == st.active;
                let term = tab.term.lock();
                let v = TermView {
                    x: pane.x,
                    y: pane.y,
                    lines: pane.ws.num_lines as usize,
                    focused: is_focus && st.focused && st.palette.is_none() && st.settings.is_none(),
                    cursor_on: st.cursor_on || !st.focused,
                };
                let bs = tab.blocks.lock().unwrap();
                // Rama de git para la etiqueta del prompt de Garbo (solo mientras se escribe).
                let branch = bs
                    .idle_current()
                    .filter(|id| bs.styled(*id))
                    .and_then(|_| tab.cwd())
                    .and_then(|d| crate::complete::git_branch(&d));
                view::draw_prompts(&mut p, &term, theme, &v, &bs, branch.as_deref(), st.scale);
                let search = st.search.as_mut().filter(|s| s.tab == tab.id);
                let hl = search.map(|s| s.highlights(&term));
                view::draw_terminal(&mut p, &term, theme, &v, Some(&bs), hl.as_ref());
                if let Some((_, _, Some(found))) = st.link.as_ref().filter(|l| l.0 == tab.id) {
                    view::draw_underline(&mut p, &term, theme, &v, found.start, found.end, st.scale);
                }
                let start = bs.input_start;
                // Menú de completado: se refina con lo que se escribe y se cierra si deja de aplicar.
                if is_focus && (st.menu.is_some() || st.menu_auto.is_some()) {
                    let input = crate::terminal::input_for(&term, start);
                    let cwd = input.as_ref().and_then(|_| tab.cwd());
                    if let Some(menu) = st.menu.as_mut() {
                        let keep = menu.tab == tab.id && input.as_deref().is_some_and(|i| menu.refresh(i, cwd.as_deref()));
                        if !keep {
                            st.menu = None;
                        }
                    } else if let Some((before, since)) = &st.menu_auto {
                        match &input {
                            Some(now) if now != before => {
                                let comp = crate::complete::complete(now, cwd.as_deref()).filter(|c| c.opens_as_you_type());
                                st.menu = comp.map(|c| crate::complete::Menu::new(tab.id, now.clone(), c).opened_while_typing());
                                st.menu_auto = None;
                            }
                            _ if since.elapsed() > std::time::Duration::from_secs(1) => st.menu_auto = None,
                            _ => {}
                        }
                    }
                }
                // Sugerencia en gris tras el cursor (no con el menú abierto).
                if cfg.terminal.autosuggest && v.focused && st.menu.is_none() {
                    let cwd = start.and_then(|_| tab.cwd());
                    if let Some(rest) = crate::terminal::suggestion_for(&term, start, cwd.as_deref()) {
                        view::draw_ghost(&mut p, &term, theme, &v, &rest);
                    }
                }
                if show_blocks {
                    view::draw_blocks(&mut p, &term, theme, &v, &bs, tab.selected_block, (pane.rect[0], pane.rect[0] + pane.rect[2]), st.scale);
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
                    p.rect(x, y, w, h, theme.background, 0.35);
                }
                let line = st.scale.round().max(1.0);
                for d in &geo.dividers {
                    let [x, y, w, h] = d.rect;
                    let dragging = st.drag.as_ref().is_some_and(|g| g.0 == d.path);
                    let (c, a) = if dragging { (theme.accent, 1.0) } else { (theme.foreground, 0.16) };
                    if d.side {
                        p.rect((x + (w - line) / 2.0).round(), y, line, h, c, a);
                    } else {
                        p.rect(x, (y + (h - line) / 2.0).round(), w, line, c, a);
                    }
                }
            }
            if let Some((px, py, pw, ph)) = geo.panel {
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
                                view::preview_lines(&t.term.lock(), theme, crate::panel::PREVIEW_LINES)
                            }),
                        })
                    })
                    .collect();
                st.panel.draw(&mut p, theme, &rows, px, py, pw, ph, st.scale, now, phase);
            }
            if let Some(pal) = &st.palette {
                // El texto del terminal va debajo del overlay: se vuelca antes de pintarlo.
                p.out.append(p.fg);
                let top = geo.tab_bar.as_ref().map_or(0.0, |t| t.height);
                ui::draw_palette(&mut p, theme, pal, w, h, top, st.scale);
            }
            if let Some(set) = st.settings.as_mut() {
                p.out.append(p.fg);
                let top = geo.tab_bar.as_ref().map_or(0.0, |t| t.height);
                let upd = crate::settings::UpdateView { check: &st.update_check, state: st.update.as_ref() };
                set.draw(&mut p, theme, cfg, &upd, w, h, top, st.scale);
            }
            p.finish();
            if !st.glyphs.overflowed {
                break;
            }
        }
        st.renderer.render(&st.out, theme.background.to_f32(opacity));
    }
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
    let prim: Box<dyn ClipboardProvider> = Box::new(copypasta::nop_clipboard::NopClipboardContext::new().unwrap());
    (clip, prim)
}
