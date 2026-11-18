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
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub next_id: u64,
    pub scale: f32,
    pub font_pt: f32,
    pub mods: ModifiersState,
    pub mouse: (f32, f32),
    pub selecting: bool,
    pub mouse_buttons: u8,
    pub last_click: (Instant, u8),
    pub hover: Option<TabHit>,
    pub cursor_icon: winit::window::CursorIcon,
    pub palette: Option<Palette>,
    pub settings: Option<crate::settings::Settings>,
    pub update: Option<crate::update::UpdateState>,
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
    pub x: f32,
    pub y: f32,
    pub ws: WindowSize,
    /// Panel lateral (x, y, ancho, alto) si está abierto.
    pub panel: Option<(f32, f32, f32, f32)>,
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
            next_id: 1,
            scale,
            font_pt,
            mods: ModifiersState::empty(),
            mouse: (0.0, 0.0),
            selecting: false,
            mouse_buttons: 0,
            last_click: (now, 0),
            hover: None,
            cursor_icon: winit::window::CursorIcon::Default,
            palette: None,
            settings: None,
            update: None,
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
        let m = st.glyphs.fonts.metrics;
        let (w, h) = st.renderer.size();
        let n_tabs = st.tabs.len().max(1);
        let integrated = !self.cfg.window.decorations;
        let tab_bar = (integrated || self.cfg.window.always_show_tabs || n_tabs > 1)
            .then(|| TabBar::layout(n_tabs, w, st.scale, integrated, st.update.is_some()));
        let top = tab_bar.as_ref().map_or(0.0, |t| t.height);
        let pad = (self.cfg.window.padding * st.scale).round();
        let panel_w = if st.panel.visible() { crate::panel::PanelState::width(st.scale, w) } else { 0.0 };
        let panel = (panel_w > 0.0).then(|| (w - panel_w, top, panel_w, h - top));
        let cols = ((w - panel_w - 2.0 * pad) / m.width).floor().max(2.0);
        let lines = ((h - top - 2.0 * pad) / m.height).floor().max(1.0);
        Geometry {
            tab_bar,
            x: pad,
            y: top + pad,
            ws: WindowSize {
                num_lines: lines as u16,
                num_cols: cols as u16,
                cell_width: m.width as u16,
                cell_height: m.height as u16,
            },
            panel,
        }
    }

    /// Recalcula filas/columnas y avisa a todos los PTY si cambiaron.
    pub fn relayout(&mut self) {
        let ws = self.geometry().ws;
        let st = self.st();
        let same = |a: &WindowSize, b: &WindowSize| {
            (a.num_lines, a.num_cols, a.cell_width, a.cell_height) == (b.num_lines, b.num_cols, b.cell_width, b.cell_height)
        };
        if !same(&ws, &st.last_ws) {
            st.last_ws = ws;
            for tab in &mut st.tabs {
                tab.resize(ws);
            }
        }
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
                st.active = st.tabs.len() - 1;
                st.last_ws.num_cols = 0; // fuerza resize con la geometría real
                self.relayout();
                self.update_title();
            }
            Err(e) => log::error!("No se pudo abrir el shell: {e}"),
        }
    }

    /// Cierra una pestaña. Devuelve `true` si ya no queda ninguna.
    pub fn close_tab(&mut self, idx: usize) -> bool {
        let st = self.st();
        if idx >= st.tabs.len() {
            return st.tabs.is_empty();
        }
        st.tabs.remove(idx);
        if st.tabs.is_empty() {
            return true;
        }
        if st.active >= idx && st.active > 0 {
            st.active -= 1;
        }
        st.last_ws.num_cols = 0;
        self.relayout();
        self.update_title();
        false
    }

    pub fn select_tab(&mut self, idx: usize) {
        let st = self.st();
        if idx < st.tabs.len() && idx != st.active {
            st.active = idx;
            st.window.request_redraw();
            self.update_title();
        }
    }

    pub fn update_title(&mut self) {
        let st = self.st();
        if let Some(tab) = st.tabs.get(st.active) {
            st.window.set_title(&format!("{} — Garbo", tab.title));
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

    pub fn paste(&mut self, from_primary: bool) {
        let st = self.st();
        let cb = if from_primary { &mut st.selection_clip } else { &mut st.clipboard };
        let Ok(text) = cb.get_contents() else { return };
        self.paste_text(&text);
    }

    pub fn paste_text(&mut self, text: &str) {
        let st = self.st();
        let Some(tab) = st.tabs.get(st.active) else { return };
        let mode = *tab.term.lock().mode();
        let bytes = if mode.contains(TermMode::BRACKETED_PASTE) {
            // Evita que el texto pegado cierre el modo bracketed por su cuenta.
            let clean = text.replace("\x1b[201~", "").replace('\x1b', "");
            format!("\x1b[200~{clean}\x1b[201~")
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };
        tab.write(bytes.into_bytes());
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
            Action::CloseTab => {
                let idx = self.st().active;
                return self.close_tab(idx);
            }
            Action::NextTab | Action::PrevTab => {
                let st = self.st();
                let n = st.tabs.len();
                if n > 1 {
                    let d = if action == Action::NextTab { 1 } else { n - 1 };
                    let idx = (st.active + d) % n;
                    self.select_tab(idx);
                }
            }
            Action::MoveTabLeft | Action::MoveTabRight => {
                let st = self.st();
                let i = st.active;
                let j = if action == Action::MoveTabLeft { i.checked_sub(1) } else { Some(i + 1) };
                if let Some(j) = j.filter(|&j| j < st.tabs.len()) {
                    st.tabs.swap(i, j);
                    st.active = j;
                    st.window.request_redraw();
                }
            }
            Action::GotoTab(i) => {
                let n = self.st().tabs.len();
                // Alt+9 siempre va a la última, como en los navegadores
                self.select_tab(if i == 8 { n - 1 } else { i });
            }
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
        let (active, focused) = (st.active, st.focused);
        let mut changed = false;
        for (i, tab) in st.tabs.iter_mut().enumerate() {
            changed |= tab.refresh_session(focused && i == active, now);
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
            if tab.session.is_working() {
                push(now + std::time::Duration::from_millis(150));
            } else if now.duration_since(tab.last_output) < crate::sessions::WORK_WINDOW {
                push(tab.last_output + crate::sessions::WORK_WINDOW + std::time::Duration::from_millis(60));
            }
        }
        if st.panel.agents && st.tabs.iter().any(|t| t.session.agent.is_some()) {
            push(now + std::time::Duration::from_secs(1));
        }
        next
    }

    pub fn toggle_panel(&mut self, agents: bool) {
        let proxy = self.proxy.clone();
        let st = self.st();
        if agents {
            st.panel.agents = !st.panel.agents;
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
    pub fn apply_setting(&mut self, change: crate::settings::Change) {
        use crate::settings::Change;
        match change {
            Change::None => return,
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
            Change::OpenFile => {
                if let Some(path) = Config::path() {
                    if !path.exists() {
                        let _ = self.cfg.save();
                    }
                    let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
                }
                return;
            }
        }
        if let Err(e) = self.cfg.save() {
            log::warn!("No se pudo guardar la configuración: {e}");
        }
        self.st().window.request_redraw();
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
            .tabs
            .iter()
            .map(|t| ui::TabView {
                label: t.label().to_string(),
                exited: t.exited,
                agent: t.session.agent,
                status: t.session.status,
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
                tb.draw(&mut p, theme, w, &views, st.active, st.hover, st.scale, st.update.as_ref(), toggles, phase);
            }
            if let Some(tab) = st.tabs.get(st.active) {
                let term = tab.term.lock();
                let v = TermView {
                    x: geo.x,
                    y: geo.y,
                    lines: geo.ws.num_lines as usize,
                    focused: st.focused && st.palette.is_none() && st.settings.is_none(),
                    cursor_on: st.cursor_on || !st.focused,
                };
                view::draw_terminal(&mut p, &term, theme, &v);
                if show_blocks {
                    let bs = tab.blocks.lock().unwrap();
                    view::draw_blocks(&mut p, &term, theme, &v, &bs, tab.selected_block, w, st.scale);
                }
            }
            if let Some((px, py, pw, ph)) = geo.panel {
                let rows: Vec<crate::panel::AgentRow> = st
                    .tabs
                    .iter()
                    .enumerate()
                    .filter_map(|(i, t)| {
                        Some(crate::panel::AgentRow {
                            label: t.label(),
                            agent: t.session.agent?,
                            status: t.session.status,
                            since: t.session.since,
                            active: i == st.active,
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
                set.draw(&mut p, theme, cfg, w, h, top, st.scale);
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
