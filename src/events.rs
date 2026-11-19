//! Bucle de eventos: winit -> acciones / PTY. Sin polling: la app duerme hasta que pasa algo.

use crate::app::App;
use crate::blocks;
use crate::input::{self, MouseButtonCode};
use crate::terminal::UserEvent;
use crate::ui::TabHit;
use alacritty_terminal::event::Event as TermEvent;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::{ClipboardType, TermMode};
use alacritty_terminal::vte::ansi::Rgb as VteRgb;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, ResizeDirection, Window, WindowId};

fn resize_cursor(dir: ResizeDirection) -> CursorIcon {
    match dir {
        ResizeDirection::East => CursorIcon::EResize,
        ResizeDirection::West => CursorIcon::WResize,
        ResizeDirection::North => CursorIcon::NResize,
        ResizeDirection::South => CursorIcon::SResize,
        ResizeDirection::NorthEast => CursorIcon::NeResize,
        ResizeDirection::NorthWest => CursorIcon::NwResize,
        ResizeDirection::SouthEast => CursorIcon::SeResize,
        ResizeDirection::SouthWest => CursorIcon::SwResize,
    }
}

const BLINK: Duration = Duration::from_millis(530);
const MULTI_CLICK: Duration = Duration::from_millis(400);

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.st.is_some() {
            return;
        }
        // Tamaño inicial aproximado en píxeles lógicos (se corrige al conocer la escala).
        let w = &self.cfg.window;
        let cell_w = self.cfg.font.size * 4.0 / 3.0 * 0.6;
        let cell_h = self.cfg.font.size * 4.0 / 3.0 * self.cfg.font.line_height * 1.25;
        let size = LogicalSize::new(
            w.columns as f32 * cell_w + 2.0 * w.padding,
            w.lines as f32 * cell_h + 2.0 * w.padding + 40.0,
        );
        let attrs = Window::default_attributes()
            .with_title("Garbo")
            .with_inner_size(size)
            .with_min_inner_size(LogicalSize::new(240.0, 120.0))
            .with_decorations(w.decorations)
            // Siempre con canal alfa: así la opacidad se puede cambiar en vivo desde Ajustes.
            .with_transparent(true);
        // Identificador de la app: GNOME lo usa para asociar la ventana con su .desktop e icono.
        #[cfg(all(unix, not(target_os = "macos")))]
        let attrs = {
            use winit::platform::startup_notify::{self, EventLoopExtStartupNotify, WindowAttributesExtStartupNotify};
            use winit::platform::wayland::WindowAttributesExtWayland;
            use winit::platform::x11::WindowAttributesExtX11;
            let attrs = WindowAttributesExtWayland::with_name(attrs, crate::APP_ID, "garbo");
            let mut attrs = WindowAttributesExtX11::with_name(attrs, crate::APP_ID, "garbo");
            // Notificación de arranque: el lanzador (GNOME) pasa un token y muestra el cursor de
            // "cargando" hasta que la ventana lo usa. Sin usarlo, el cursor seguía así ~30 s.
            // Después se quita del entorno para que no lo hereden los programas de las pestañas.
            if let Some(token) = el.read_token_from_env() {
                attrs = attrs.with_activation_token(token);
            }
            startup_notify::reset_activation_token_env();
            attrs
        };
        let window = match el.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("No se pudo crear la ventana: {e}");
                el.exit();
                return;
            }
        };
        self.init(window);
    }

    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause {
            let blink = self.cfg.terminal.cursor_blink;
            if let Some(st) = self.st.as_mut() {
                let now = Instant::now();
                if blink && now >= st.next_blink {
                    st.cursor_on = !st.cursor_on;
                    st.next_blink = now + BLINK;
                    st.window.request_redraw();
                }
                // "hace N s" del panel de agentes
                if st.panel.agents {
                    st.window.request_redraw();
                }
            }
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.refresh_sessions();
        let blink = self.cfg.terminal.cursor_blink;
        let tick = self.next_tick();
        let blink_at = self.st.as_ref().filter(|st| blink && st.focused).map(|st| st.next_blink);
        // Sin animaciones ni parpadeo pendientes, la app duerme hasta el siguiente evento.
        match (blink_at, tick) {
            (Some(a), Some(b)) => el.set_control_flow(ControlFlow::WaitUntil(a.min(b))),
            (Some(t), None) | (None, Some(t)) => el.set_control_flow(ControlFlow::WaitUntil(t)),
            (None, None) => el.set_control_flow(ControlFlow::Wait),
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, event: UserEvent) {
        let (id, ev) = match event {
            UserEvent::Term(id, ev) => (id, ev),
            UserEvent::Ports(list) => {
                if let Some(st) = self.st.as_mut() {
                    st.panel.ports_loaded = true;
                    if st.panel.ports_data != list {
                        st.panel.ports_data = list;
                        st.window.request_redraw();
                    }
                }
                return;
            }
            UserEvent::UpdateCheck(state) => {
                if let Some(st) = self.st.as_mut() {
                    st.update_check = state;
                    st.window.request_redraw();
                }
                return;
            }
            UserEvent::PasteImage(id, result) => {
                match result {
                    Ok(path) => {
                        let text = crate::clipboard::shell_path(&path) + " ";
                        self.paste_text_to(id, &text);
                    }
                    Err(e) => log::info!("Pegar: {e}"),
                }
                return;
            }
            UserEvent::Update(state) => {
                if let Some(st) = self.st.as_mut() {
                    let first = st.update.is_none();
                    st.update = Some(state);
                    if first {
                        st.last_ws.num_cols = 0;
                        self.relayout();
                    }
                    self.st().window.request_redraw();
                }
                return;
            }
        };
        let Some(st) = self.st.as_mut() else { return };
        let Some(idx) = st.tabs.iter().position(|t| t.id == id) else { return };
        let is_active = idx == st.active;
        match ev {
            TermEvent::Wakeup => {
                st.tabs[idx].note_output(Instant::now());
                // Las pestañas en segundo plano no gastan GPU.
                if is_active {
                    st.window.request_redraw();
                }
            }
            TermEvent::Title(t) => {
                st.tabs[idx].set_title(t);
                st.window.request_redraw();
                self.update_title();
            }
            TermEvent::ResetTitle => {
                st.tabs[idx].title = std::env::var("SHELL")
                    .ok()
                    .and_then(|s| s.rsplit('/').next().map(str::to_owned))
                    .unwrap_or_else(|| "shell".into());
                st.window.request_redraw();
            }
            TermEvent::Bell => {
                let viewing = is_active && st.focused;
                if st.tabs[idx].session.bell(viewing, Instant::now()) {
                    st.window.request_redraw();
                }
                if !st.focused {
                    st.window.request_user_attention(Some(winit::window::UserAttentionType::Informational));
                }
            }
            TermEvent::PtyWrite(s) => st.tabs[idx].write(s.into_bytes()),
            TermEvent::ClipboardStore(ty, text) => {
                let cb = if ty == ClipboardType::Selection { &mut st.selection_clip } else { &mut st.clipboard };
                let _ = cb.set_contents(text);
            }
            TermEvent::ClipboardLoad(ty, fmt) => {
                let cb = if ty == ClipboardType::Selection { &mut st.selection_clip } else { &mut st.clipboard };
                let text = cb.get_contents().unwrap_or_default();
                st.tabs[idx].write(fmt(&text).into_bytes());
            }
            TermEvent::ColorRequest(i, fmt) => {
                let term_color = st.tabs[idx].term.lock().colors()[i];
                let c = term_color.map(|c| crate::config::Rgb(c.r, c.g, c.b)).unwrap_or_else(|| match i {
                    0..=255 => self.theme.color256(i as u8),
                    257 => self.theme.background,
                    258 => self.theme.cursor,
                    _ => self.theme.foreground,
                });
                st.tabs[idx].write(fmt(VteRgb { r: c.0, g: c.1, b: c.2 }).into_bytes());
            }
            TermEvent::TextAreaSizeRequest(fmt) => {
                let ws = st.last_ws;
                st.tabs[idx].write(fmt(ws).into_bytes());
            }
            TermEvent::CursorBlinkingChange => {
                st.cursor_on = true;
                st.next_blink = Instant::now() + BLINK;
            }
            TermEvent::Exit | TermEvent::ChildExit(_) => {
                // Como en la mayoría de terminales modernas: al salir el shell se cierra la pestaña.
                if self.close_tab(idx) {
                    el.exit();
                }
            }
            _ => {}
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if self.st.is_none() {
            return;
        }
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::Resized(size) => {
                self.st().renderer.resize(size.width, size.height);
                self.relayout();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => self.set_scale(scale_factor as f32),
            WindowEvent::Focused(f) => {
                let st = self.st();
                st.focused = f;
                st.cursor_on = true;
                st.next_blink = Instant::now() + BLINK;
                if let Some(tab) = st.tabs.get(st.active) {
                    if tab.term.lock().mode().contains(TermMode::FOCUS_IN_OUT) {
                        tab.input(if f { &b"\x1b[I"[..] } else { &b"\x1b[O"[..] });
                    }
                }
                st.window.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => self.st().mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if self.on_key(event) {
                    el.exit();
                }
            }
            WindowEvent::CursorMoved { position, .. } => self.on_mouse_move(position.x as f32, position.y as f32),
            WindowEvent::CursorLeft { .. } => {
                let st = self.st();
                let panel = st.panel.set_hover(None, Instant::now());
                if st.hover.take().is_some() || panel {
                    st.window.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self.on_mouse_button(state, button) {
                    el.exit();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => self.on_wheel(delta),
            WindowEvent::DroppedFile(path) => {
                let s = path.to_string_lossy();
                let quoted = format!("'{}' ", s.replace('\'', "'\\''"));
                self.paste_text(&quoted);
            }
            _ => {}
        }
    }
}

impl App {
    /// Devuelve `true` si la app debe cerrarse.
    fn on_key(&mut self, ev: KeyEvent) -> bool {
        {
            let st = self.st();
            st.cursor_on = true;
            st.next_blink = Instant::now() + BLINK;
        }

        // Ajustes abiertos: capturan el teclado.
        if self.st().settings.is_some() {
            let mods = self.st().mods;
            if self.keys.lookup(&ev.logical_key, mods) == Some(crate::keybindings::Action::Settings) {
                return self.run_action(crate::keybindings::Action::Settings);
            }
            use crate::settings::Nav;
            let shift = mods.shift_key();
            let st = self.st.as_mut().unwrap();
            let nav = match &ev.logical_key {
                Key::Named(NamedKey::Escape) => {
                    st.settings = None;
                    None
                }
                Key::Named(NamedKey::ArrowUp) => Some(Nav::Up),
                Key::Named(NamedKey::ArrowDown) => Some(Nav::Down),
                Key::Named(NamedKey::ArrowLeft) => Some(Nav::Left),
                Key::Named(NamedKey::ArrowRight) => Some(Nav::Right),
                Key::Named(NamedKey::Tab) if shift => Some(Nav::PrevCategory),
                Key::Named(NamedKey::Tab) | Key::Named(NamedKey::PageDown) => Some(Nav::NextCategory),
                Key::Named(NamedKey::PageUp) => Some(Nav::PrevCategory),
                Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => Some(Nav::Activate),
                _ => None,
            };
            let change = match (nav, st.settings.as_mut()) {
                (Some(n), Some(set)) => set.nav(n, &mut self.cfg),
                _ => crate::settings::Change::None,
            };
            st.window.request_redraw();
            return self.apply_setting(change);
        }

        // Paleta de comandos abierta: captura todo el teclado.
        if self.st().palette.is_some() {
            let mods = self.st().mods;
            if self.keys.lookup(&ev.logical_key, mods) == Some(crate::keybindings::Action::CommandPalette) {
                return self.run_action(crate::keybindings::Action::CommandPalette);
            }
            let st = self.st();
            let pal = st.palette.as_mut().unwrap();
            match &ev.logical_key {
                Key::Named(NamedKey::Escape) => st.palette = None,
                Key::Named(NamedKey::Enter) => {
                    if let Some(item) = pal.current() {
                        return self.run_palette_item(item);
                    }
                }
                Key::Named(NamedKey::ArrowUp) => pal.move_sel(-1),
                Key::Named(NamedKey::ArrowDown) | Key::Named(NamedKey::Tab) => pal.move_sel(1),
                Key::Named(NamedKey::Backspace) => {
                    pal.query.pop();
                    pal.refilter();
                }
                _ => {
                    if let Some(t) = ev.text.as_ref().filter(|t| t.chars().all(|c| !c.is_control())) {
                        pal.query.push_str(t);
                        pal.refilter();
                    }
                }
            }
            st.window.request_redraw();
            return false;
        }

        let mods = self.st().mods;
        if let Some(action) = self.keys.lookup(&ev.logical_key, mods) {
            return self.run_action(action);
        }

        let autosuggest = self.cfg.terminal.autosuggest;
        let st = self.st();
        let Some(tab) = st.tabs.get(st.active) else { return false };
        // Sugerencia del historial: Tab (o → al final de la línea) la acepta. Sin sugerencia,
        // Tab llega al shell (su autocompletado de siempre).
        let accept = matches!(ev.logical_key, Key::Named(NamedKey::Tab | NamedKey::ArrowRight));
        if autosuggest && accept && (mods.is_empty()) {
            if let Some(rest) = tab.suggestion() {
                tab.input(rest.into_bytes());
                self.scroll_to_bottom();
                return false;
            }
        }
        let mode = *tab.term.lock().mode();
        if let Some(bytes) = input::key_to_bytes(&ev, mods, mode) {
            {
                let mut term = tab.term.lock();
                term.selection = None;
            }
            tab.input(bytes);
            let st = self.st();
            let active = st.active;
            if st.tabs.get_mut(active).is_some_and(|t| t.selected_block.take().is_some()) {
                st.window.request_redraw();
            }
            self.scroll_to_bottom();
        }
        false
    }

    /// Pixel -> celda del viewport (columna, fila, lado).
    fn cell_at(&self, x: f32, y: f32) -> (usize, usize, Side) {
        let geo = self.geometry();
        let m = self.st.as_ref().unwrap().glyphs.fonts.metrics;
        let fx = ((x - geo.x) / m.width).max(0.0);
        let fy = ((y - geo.y) / m.height).max(0.0);
        let col = (fx as usize).min(geo.ws.num_cols as usize - 1);
        let row = (fy as usize).min(geo.ws.num_lines as usize - 1);
        let side = if fx.fract() < 0.5 { Side::Left } else { Side::Right };
        (col, row, side)
    }

    fn on_mouse_move(&mut self, x: f32, y: f32) {
        let geo = self.geometry();
        let st = self.st();
        st.mouse = (x, y);
        if st.settings.is_some() {
            let (w, h) = st.renderer.size();
            let top = geo.tab_bar.as_ref().map_or(0.0, |t| t.height);
            let scale = st.scale;
            let set = st.settings.as_mut().unwrap();
            let hit = set.hit(x, y, w, h, top, scale);
            if set.hover != Some(hit) {
                set.hover = Some(hit);
                st.window.request_redraw();
            }
            if st.cursor_icon != CursorIcon::Default {
                st.cursor_icon = CursorIcon::Default;
                st.window.set_cursor(CursorIcon::Default);
            }
            return;
        }
        if geo.panel.is_some() {
            let now = Instant::now();
            let hit = self.panel_hit(x, y, now);
            let st = self.st();
            if st.panel.set_hover(hit, now) {
                st.window.request_redraw();
            }
            if hit.is_some() {
                let icon = match hit {
                    Some(crate::panel::Hit::Panel) => CursorIcon::Default,
                    _ => CursorIcon::Pointer,
                };
                if st.cursor_icon != icon {
                    st.cursor_icon = icon;
                    st.window.set_cursor(icon);
                }
                return;
            }
        }
        let st = self.st();
        let hover = geo.tab_bar.as_ref().and_then(|tb| tb.hit(x, y));
        if hover != st.hover {
            st.hover = hover;
            st.window.request_redraw();
        }
        let in_bar = geo.tab_bar.as_ref().is_some_and(|tb| y < tb.height);
        let in_gutter = !in_bar && x < geo.x && self.blocks_active();
        let icon = match self.resize_edge(x, y) {
            Some(dir) => resize_cursor(dir),
            None if in_bar => CursorIcon::Default,
            None if in_gutter => CursorIcon::Pointer,
            None => CursorIcon::Text,
        };
        let st = self.st();
        if icon != st.cursor_icon {
            st.cursor_icon = icon;
            st.window.set_cursor(icon);
        }
        if in_bar {
            return;
        }

        let (col, row, side) = self.cell_at(x, y);
        let st = self.st();
        let Some(tab) = st.tabs.get(st.active) else { return };
        let mut term = tab.term.lock();
        let mode = *term.mode();
        let reporting = mode.intersects(TermMode::MOUSE_MODE) && !st.mods.shift_key();
        if reporting {
            let held = st.mouse_buttons != 0;
            if mode.contains(TermMode::MOUSE_MOTION) || (held && mode.contains(TermMode::MOUSE_DRAG)) {
                let btn = match st.mouse_buttons {
                    b if b & 1 != 0 => MouseButtonCode::Left,
                    b if b & 2 != 0 => MouseButtonCode::Middle,
                    b if b & 4 != 0 => MouseButtonCode::Right,
                    _ => MouseButtonCode::Left, // sin botón: código 3+32 en la práctica
                };
                if let Some(b) = input::mouse_report(btn, true, true, col, row, st.mods, mode) {
                    drop(term);
                    tab.input(b);
                }
            }
            return;
        }
        if st.selecting {
            let offset = term.grid().display_offset() as i32;
            let point = Point::new(Line(row as i32 - offset), Column(col));
            if let Some(sel) = term.selection.as_mut() {
                sel.update(point, side);
            }
            drop(term);
            st.window.request_redraw();
        }
    }

    fn on_mouse_button(&mut self, state: ElementState, button: MouseButton) -> bool {
        let pressed = state == ElementState::Pressed;
        let (x, y) = self.st().mouse;
        let geo = self.geometry();

        // Ajustes abiertos
        if self.st().settings.is_some() {
            if !pressed || button != MouseButton::Left {
                return false;
            }
            use crate::settings::Hit;
            let st = self.st.as_mut().unwrap();
            let (w, h) = st.renderer.size();
            let top = geo.tab_bar.as_ref().map_or(0.0, |t| t.height);
            let scale = st.scale;
            let set = st.settings.as_mut().unwrap();
            let change = match set.hit(x, y, w, h, top, scale) {
                Hit::Close | Hit::Outside => {
                    st.settings = None;
                    crate::settings::Change::None
                }
                Hit::Dec(i) => {
                    set.selected = i;
                    set.adjust(i, -1, &mut self.cfg)
                }
                Hit::Inc(i) => {
                    set.selected = i;
                    set.adjust(i, 1, &mut self.cfg)
                }
                Hit::Row(i) => {
                    set.selected = i;
                    set.adjust(i, 0, &mut self.cfg)
                }
                Hit::Theme(i) => set.pick_theme(i, &mut self.cfg),
                Hit::Category(c) => {
                    set.set_cat(c);
                    crate::settings::Change::None
                }
                Hit::Panel => crate::settings::Change::None,
            };
            st.window.request_redraw();
            return self.apply_setting(change);
        }

        // Paleta abierta: un clic fuera la cierra.
        if self.st().palette.is_some() {
            if pressed {
                self.st().palette = None;
                self.st().window.request_redraw();
            }
            return false;
        }

        // Bordes: redimensionar (solo con barra de título integrada)
        if pressed && button == MouseButton::Left {
            if let Some(dir) = self.resize_edge(x, y) {
                let _ = self.st().window.drag_resize_window(dir);
                return false;
            }
        }

        // Panel lateral
        if let Some((px, py, _, _)) = geo.panel {
            if x >= px && y >= py {
                if pressed && button == MouseButton::Left {
                    if let Some(hit) = self.panel_hit(x, y, Instant::now()) {
                        self.on_panel_click(hit);
                    }
                }
                return false;
            }
        }

        // Barra de pestañas
        if let Some(tb) = geo.tab_bar.as_ref().filter(|tb| y < tb.height) {
            if pressed {
                let st = self.st();
                let now = Instant::now();
                let double = now.duration_since(st.last_click.0) < MULTI_CLICK && st.last_click.1 == 1;
                st.last_click = (now, if double { 0 } else { 1 });
                match (tb.hit(x, y), button) {
                    (Some(TabHit::Tab(i)), MouseButton::Left) => self.select_tab(i),
                    (Some(TabHit::Tab(i) | TabHit::Close(i)), MouseButton::Middle)
                    | (Some(TabHit::Close(i)), MouseButton::Left) => return self.close_tab(i),
                    (Some(TabHit::NewTab), _) => self.new_tab(),
                    (Some(TabHit::Update), MouseButton::Left) => return self.on_update_click(),
                    (Some(TabHit::Agents), MouseButton::Left) => self.toggle_panel(true),
                    (Some(TabHit::Ports), MouseButton::Left) => self.toggle_panel(false),
                    (Some(TabHit::Settings), MouseButton::Left) => {
                        self.run_action(crate::keybindings::Action::Settings);
                    }
                    (Some(TabHit::Minimize), MouseButton::Left) => self.st().window.set_minimized(true),
                    (Some(TabHit::Maximize), MouseButton::Left) => self.toggle_maximized(),
                    (Some(TabHit::CloseWindow), MouseButton::Left) => return true,
                    (Some(TabHit::Empty), MouseButton::Left) if double => self.toggle_maximized(),
                    (Some(TabHit::Empty), MouseButton::Left) => {
                        let _ = self.st().window.drag_window();
                    }
                    (Some(TabHit::Empty), MouseButton::Middle) => self.new_tab(),
                    _ => {}
                }
            }
            return false;
        }

        // Margen izquierdo: clic selecciona (o deselecciona) el bloque de esa línea.
        if pressed && button == MouseButton::Left && x < geo.x && y >= geo.y && self.blocks_active() {
            let row = ((y - geo.y) / self.st().glyphs.fonts.metrics.height).max(0.0) as i32;
            let st = self.st();
            if let Some(tab) = st.tabs.get_mut(st.active) {
                let term = tab.term.lock();
                let line = Line(row.min(term.screen_lines() as i32 - 1) - term.grid().display_offset() as i32);
                let idle = tab.blocks.lock().unwrap().idle_current();
                let hit = blocks::anchor_at(&term, line, 10_000).map(|a| a.id).filter(|id| Some(*id) != idle);
                drop(term);
                tab.selected_block = if hit == tab.selected_block { None } else { hit };
                st.window.request_redraw();
            }
            return false;
        }

        let bit = match button {
            MouseButton::Left => 1,
            MouseButton::Middle => 2,
            MouseButton::Right => 4,
            _ => 0,
        };
        let st = self.st();
        if pressed {
            st.mouse_buttons |= bit;
        } else {
            st.mouse_buttons &= !bit;
        }

        let (col, row, side) = self.cell_at(x, y);
        let st = self.st();
        let Some(tab) = st.tabs.get(st.active) else { return false };
        let mode = *tab.term.lock().mode();

        // La app del terminal quiere el ratón (vim, htop...). Shift fuerza selección local.
        if mode.intersects(TermMode::MOUSE_MODE) && !st.mods.shift_key() {
            let code = match button {
                MouseButton::Left => MouseButtonCode::Left,
                MouseButton::Middle => MouseButtonCode::Middle,
                MouseButton::Right => MouseButtonCode::Right,
                _ => return false,
            };
            if let Some(b) = input::mouse_report(code, pressed, false, col, row, st.mods, mode) {
                tab.input(b);
            }
            return false;
        }

        match (button, pressed) {
            (MouseButton::Left, true) => {
                let now = Instant::now();
                let count = if now.duration_since(st.last_click.0) < MULTI_CLICK { st.last_click.1 % 3 + 1 } else { 1 };
                st.last_click = (now, count);
                let ty = match count {
                    2 => SelectionType::Semantic,
                    3 => SelectionType::Lines,
                    _ if st.mods.control_key() && st.mods.alt_key() => SelectionType::Block,
                    _ => SelectionType::Simple,
                };
                let mut term = tab.term.lock();
                let offset = term.grid().display_offset() as i32;
                let point = Point::new(Line(row as i32 - offset), Column(col));
                term.selection = Some(Selection::new(ty, point, side));
                st.selecting = true;
                drop(term);
                if let Some(tab) = st.tabs.get_mut(st.active) {
                    tab.selected_block = None;
                }
                st.window.request_redraw();
            }
            (MouseButton::Left, false) => {
                if st.selecting {
                    st.selecting = false;
                    let empty = tab.term.lock().selection.as_ref().is_none_or(|s| s.is_empty());
                    if empty {
                        tab.term.lock().selection = None;
                    } else {
                        let to_clipboard = self.cfg.terminal.copy_on_select;
                        self.copy_selection(true);
                        if to_clipboard {
                            self.copy_selection(false);
                        }
                    }
                    self.st().window.request_redraw();
                }
            }
            (MouseButton::Middle, true) => self.paste(true),
            _ => {}
        }
        false
    }

    fn blocks_active(&self) -> bool {
        let Some(st) = self.st.as_ref() else { return false };
        self.cfg.terminal.blocks && st.tabs.get(st.active).is_some_and(|t| t.blocks.lock().unwrap().active)
    }

    fn toggle_maximized(&mut self) {
        let w = &self.st().window;
        w.set_maximized(!w.is_maximized());
    }

    /// Borde de la ventana bajo el ratón, si la barra de título es nuestra.
    fn resize_edge(&self, x: f32, y: f32) -> Option<ResizeDirection> {
        if self.cfg.window.decorations {
            return None;
        }
        let st = self.st.as_ref()?;
        if st.window.is_maximized() {
            return None;
        }
        let (w, h) = st.renderer.size();
        let b = 4.0 * st.scale;
        let (l, r, t, bo) = (x < b, x > w - b, y < b, y > h - b);
        Some(match (l, r, t, bo) {
            (true, _, true, _) => ResizeDirection::NorthWest,
            (_, true, true, _) => ResizeDirection::NorthEast,
            (true, _, _, true) => ResizeDirection::SouthWest,
            (_, true, _, true) => ResizeDirection::SouthEast,
            (true, ..) => ResizeDirection::West,
            (_, true, ..) => ResizeDirection::East,
            (_, _, true, _) => ResizeDirection::North,
            (_, _, _, true) => ResizeDirection::South,
            _ => return None,
        })
    }

    fn on_wheel(&mut self, delta: MouseScrollDelta) {
        let mult = self.cfg.terminal.scroll_multiplier;
        let top = self.geometry().tab_bar.as_ref().map_or(0.0, |t| t.height);
        let st = self.st();
        let ch = st.glyphs.fonts.metrics.height;
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => y * mult,
            MouseScrollDelta::PixelDelta(p) => p.y as f32 / ch,
        };
        if let Some(pal) = st.palette.as_mut() {
            // Un elemento por "muesca" de la rueda (o por cada ~36 px de touchpad), no por evento.
            let steps = match delta {
                MouseScrollDelta::LineDelta(_, y) => y,
                MouseScrollDelta::PixelDelta(p) => p.y as f32 / (36.0 * st.scale),
            };
            let n = pal.wheel(steps);
            if n != 0 {
                pal.move_sel(-n);
                st.window.request_redraw();
            }
            return;
        }
        if let Some(set) = st.settings.as_mut() {
            // La rueda desplaza la lista; la selección solo la mueve el teclado.
            let px = match delta {
                MouseScrollDelta::LineDelta(_, y) => y * 48.0 * st.scale,
                MouseScrollDelta::PixelDelta(p) => p.y as f32,
            };
            set.wheel(px);
            let (w, h) = st.renderer.size();
            let (mx, my) = st.mouse;
            set.hover = Some(set.hit(mx, my, w, h, top, st.scale));
            st.window.request_redraw();
            return;
        }
        st.scroll_accum += lines;
        let n = st.scroll_accum.trunc() as i32;
        if n == 0 {
            return;
        }
        st.scroll_accum -= n as f32;
        let (x, y) = st.mouse;
        let (col, row, _) = self.cell_at(x, y);
        let st = self.st();
        let Some(tab) = st.tabs.get(st.active) else { return };
        let mode = *tab.term.lock().mode();
        if mode.intersects(TermMode::MOUSE_MODE) && !st.mods.shift_key() {
            let code = if n > 0 { MouseButtonCode::WheelUp } else { MouseButtonCode::WheelDown };
            for _ in 0..n.abs() {
                if let Some(b) = input::mouse_report(code, true, false, col, row, st.mods, mode) {
                    tab.input(b);
                }
            }
        } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            // less, man, etc. en pantalla alternativa: la rueda se convierte en flechas.
            let seq: &[u8] = match (n > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            tab.input(seq.repeat(n.unsigned_abs() as usize));
        } else {
            tab.term.lock().scroll_display(Scroll::Delta(n));
            st.window.request_redraw();
        }
    }
}
