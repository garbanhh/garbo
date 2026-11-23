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
use winit::event::{
    ElementState, KeyEvent, MouseButton, MouseScrollDelta, StartCause, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, ResizeDirection, Window, WindowId};

/// Línea divisoria de paneles bajo el ratón (con unos píxeles de margen para atinar).
fn divider_at(
    geo: &crate::app::Geometry,
    x: f32,
    y: f32,
    scale: f32,
) -> Option<&crate::panes::Divider> {
    let m = 4.0 * scale;
    geo.dividers.iter().find(|d| {
        let [dx, dy, dw, dh] = d.rect;
        if d.side {
            x >= dx - m && x < dx + dw + m && y >= dy && y < dy + dh
        } else {
            y >= dy - m && y < dy + dh + m && x >= dx && x < dx + dw
        }
    })
}

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

fn pane_is_visible(groups: &[crate::panes::Group], active_group: usize, id: u64) -> bool {
    groups
        .get(active_group)
        .is_some_and(|group| group.layout.contains(id))
}

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
            w.lines as f32 * cell_h + 2.0 * w.padding + crate::ui::BAR_HEIGHT,
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
            use winit::platform::startup_notify::{
                self, EventLoopExtStartupNotify, WindowAttributesExtStartupNotify,
            };
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
        // Windows: el icono del recurso de garbo.exe (ver build.rs) en la ventana y en la barra
        // de tareas. Sin la barra del sistema, Windows 11 no redondea las esquinas si no se le
        // pide (la sombra sí la pone winit).
        #[cfg(windows)]
        let attrs = {
            use winit::platform::windows::{
                CornerPreference, IconExtWindows, WindowAttributesExtWindows,
            };
            use winit::window::Icon;
            let attrs = attrs
                .with_corner_preference(CornerPreference::Round)
                .with_undecorated_shadow(true);
            match Icon::from_resource(1, None) {
                Ok(icon) => attrs
                    .with_window_icon(Some(icon.clone()))
                    .with_taskbar_icon(Some(icon)),
                Err(e) => {
                    log::warn!("Sin icono de la ventana: {e}");
                    attrs
                }
            }
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

    /// Al salir, todo se libera aquí, con la conexión de Wayland aún abierta. En winit 0.30
    /// `run_app` consume el bucle de eventos: si el estado se soltara después, el hilo del
    /// portapapeles destruiría sus objetos de Wayland sobre una conexión ya cerrada (SIGSEGV en
    /// cada cierre).
    fn exiting(&mut self, _el: &ActiveEventLoop) {
        // Los avisos de esta sesión ya no llevarían a ninguna parte.
        if let Some(st) = self.st.as_ref() {
            for pid in st.tabs.iter().filter_map(|t| t.notice.shown) {
                crate::notify::close(pid);
            }
        }
        // Antes de soltar la ventana: winit cierra el display de Wayland con ella.
        crate::activation::release();
        self.st = None;
    }

    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause {
            let blink = self.cfg.terminal.cursor_blink;
            if let Some(st) = self.st.as_mut() {
                let now = Instant::now();
                // Con las estadísticas abiertas no se ve ningún cursor.
                if blink && st.stats.is_none() && now >= st.next_blink {
                    st.cursor_on = !st.cursor_on;
                    st.next_blink = now + BLINK;
                    st.window.request_redraw();
                }
                // "hace N s" del panel de agentes
                if st.panel.agents && st.stats.is_none() {
                    st.window.request_redraw();
                }
            }
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.refresh_sessions();
        // Devolver al sistema la memoria que ya no se usa, si toca (una sola vez por aviso).
        if let Some(st) = self.st.as_mut() {
            if st.trim_at.is_some_and(|t| Instant::now() >= t) {
                st.trim_at = None;
                crate::platform::trim_memory();
            }
        }
        let blink = self.cfg.terminal.cursor_blink;
        let tick = self.next_tick();
        let blink_at = self
            .st
            .as_ref()
            .filter(|st| blink && st.focused && st.stats.is_none())
            .map(|st| st.next_blink);
        let trim_at = self.st.as_ref().and_then(|st| st.trim_at);
        // Sin animaciones, parpadeo ni avisos pendientes, la app duerme hasta el siguiente evento.
        match [blink_at, tick, trim_at].into_iter().flatten().min() {
            Some(t) => el.set_control_flow(ControlFlow::WaitUntil(t)),
            None => el.set_control_flow(ControlFlow::Wait),
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
            UserEvent::Stats(msg) => {
                if let Some(st) = self.st.as_mut() {
                    if let Some(stats) = st.stats.as_mut() {
                        stats.on_msg(msg);
                        st.window.request_redraw();
                    }
                }
                return;
            }
            UserEvent::NoticeShown(tab, pid) => {
                match self
                    .st
                    .as_mut()
                    .and_then(|st| st.tabs.iter_mut().find(|t| t.id == tab))
                {
                    Some(t) => t.notice.shown = Some(pid),
                    None => crate::notify::close(pid), // la pestaña ya se cerró
                }
                return;
            }
            UserEvent::NoticeClicked(tab, token) => {
                log::info!(
                    "Aviso pulsado ({})",
                    if token.is_some() {
                        "con permiso de activación"
                    } else {
                        "sin permiso"
                    }
                );
                let idx = self
                    .st
                    .as_ref()
                    .and_then(|st| st.tabs.iter().position(|t| t.id == tab));
                if let Some(i) = idx {
                    self.select_tab(i);
                    let st = self.st();
                    st.tabs[i].notice.shown = None;
                    crate::activation::raise(&st.window, token.as_deref());
                    st.window.request_redraw();
                }
                return;
            }
            UserEvent::PasteImage(id, result) => {
                match result {
                    Ok(paths) => {
                        let text: String = paths
                            .iter()
                            .map(|p| crate::clipboard::shell_path(p) + " ")
                            .collect();
                        self.paste_text_to(id, &text);
                    }
                    Err(e) => log::info!("Pegar: {e}"),
                }
                return;
            }
            UserEvent::Update(state) => {
                if let Some(st) = self.st.as_mut() {
                    use crate::update::UpdateState;
                    if matches!(state, UpdateState::Available(_))
                        && matches!(
                            st.update,
                            Some(
                                UpdateState::Downloading(_)
                                    | UpdateState::Installing(_)
                                    | UpdateState::Installed(_)
                            )
                        )
                    {
                        return;
                    }
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
        let Some(idx) = st.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        match ev {
            TermEvent::Wakeup => {
                st.tabs[idx].note_output(Instant::now());
                if pane_is_visible(&st.groups, st.group, id) {
                    st.window.request_redraw();
                }
            }
            TermEvent::Title(t) => {
                st.tabs[idx].set_title(t);
                st.window.request_redraw();
                self.update_title();
            }
            TermEvent::ResetTitle => {
                // El mismo título que al abrir la pestaña (en Windows no hay `$SHELL`: salía
                // "shell").
                let program = crate::terminal::shell_program(&self.cfg);
                st.tabs[idx].title = crate::platform::base_name(&program).to_owned();
                st.window.request_redraw();
            }
            TermEvent::Bell => {
                let viewing = st.focused && pane_is_visible(&st.groups, st.group, id);
                if st.tabs[idx].session.bell(viewing, Instant::now()) {
                    st.window.request_redraw();
                }
                if !st.focused {
                    st.window.request_user_attention(Some(
                        winit::window::UserAttentionType::Informational,
                    ));
                }
            }
            TermEvent::PtyWrite(s) => st.tabs[idx].write(s.into_bytes()),
            TermEvent::ClipboardStore(ty, text) => {
                let cb = if ty == ClipboardType::Selection {
                    &mut st.selection_clip
                } else {
                    &mut st.clipboard
                };
                let _ = cb.set_contents(text);
            }
            TermEvent::ClipboardLoad(ty, fmt) => {
                let cb = if ty == ClipboardType::Selection {
                    &mut st.selection_clip
                } else {
                    &mut st.clipboard
                };
                let text = cb.get_contents().unwrap_or_default();
                st.tabs[idx].write(fmt(&text).into_bytes());
            }
            TermEvent::ColorRequest(i, fmt) => {
                let term_color = st.tabs[idx].term.lock().colors()[i];
                let c = term_color
                    .map(|c| crate::config::Rgb(c.r, c.g, c.b))
                    .unwrap_or_else(|| match i {
                        0..=255 => self.theme.color256(i as u8),
                        257 => self.theme.background,
                        258 => self.theme.cursor,
                        _ => self.theme.foreground,
                    });
                st.tabs[idx].write(
                    fmt(VteRgb {
                        r: c.0,
                        g: c.1,
                        b: c.2,
                    })
                    .into_bytes(),
                );
            }
            TermEvent::TextAreaSizeRequest(fmt) => {
                let ws = st.tabs[idx].ws;
                st.tabs[idx].write(fmt(ws).into_bytes());
            }
            TermEvent::CursorBlinkingChange => {
                st.cursor_on = true;
                st.next_blink = Instant::now() + BLINK;
            }
            TermEvent::Exit | TermEvent::ChildExit(_) => {
                // Como en la mayoría de terminales modernas: al salir el shell se cierra su panel.
                if self.close_pane(idx) {
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
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.set_scale(scale_factor as f32)
            }
            // Minimizada no se dibuja nada: lo que no se usa se devuelve al sistema (Windows).
            WindowEvent::Occluded(true) => self.schedule_trim(std::time::Duration::ZERO),
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
            WindowEvent::ModifiersChanged(m) => {
                let st = self.st();
                let ctrl_was = st.mods.control_key();
                st.mods = m.state();
                // Ctrl subraya el enlace bajo el ratón (y al soltarlo, deja de hacerlo).
                if ctrl_was != st.mods.control_key() {
                    st.link = None;
                    let (x, y) = st.mouse;
                    st.window.request_redraw();
                    self.on_mouse_move(x, y);
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if self.on_key(event) {
                    el.exit();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.on_mouse_move(position.x as f32, position.y as f32)
            }
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
                // Entre comillas como las entiende el shell (en Windows, PowerShell).
                let quoted = crate::clipboard::shell_path(&path) + " ";
                self.paste_text(&quoted);
            }
            _ => {}
        }
    }
}

/// Teclas del menú de completado.
pub enum MenuNav {
    Close,
    Move(i32),
    Page(i32),
    Accept,
}

impl App {
    /// Devuelve `true` si la app debe cerrarse.
    fn on_key(&mut self, ev: KeyEvent) -> bool {
        {
            let st = self.st();
            st.cursor_on = true;
            st.next_blink = Instant::now() + BLINK;
        }

        // Editor de aspecto: ↑ ↓ eligen la fila, ← → la opción, Esc cierra.
        if self.st().look.is_some() {
            use crate::keybindings::Action;
            use crate::look::Nav;
            let mods = self.st().mods;
            match self.keys.lookup(&ev.logical_key, mods) {
                Some(Action::Appearance) => return self.run_action(Action::Appearance),
                Some(Action::Settings) => {
                    self.close_look();
                    return self.run_action(Action::Settings);
                }
                _ => {}
            }
            let shift = mods.shift_key();
            let nav = match &ev.logical_key {
                Key::Named(NamedKey::Escape) => {
                    self.close_look();
                    return false;
                }
                Key::Named(NamedKey::ArrowUp) => Some(Nav::Up),
                Key::Named(NamedKey::ArrowDown) => Some(Nav::Down),
                Key::Named(NamedKey::ArrowLeft) => Some(Nav::Left),
                Key::Named(NamedKey::ArrowRight) => Some(Nav::Right),
                Key::Named(NamedKey::Tab) if shift => Some(Nav::Up),
                Key::Named(NamedKey::Tab) => Some(Nav::Down),
                _ => None,
            };
            let st = self.st.as_mut().unwrap();
            let change = match (nav, st.look.as_mut()) {
                (Some(n), Some(look)) => look.nav(n, &mut self.cfg),
                _ => crate::settings::Change::None,
            };
            st.window.request_redraw();
            return self.apply_setting(change);
        }

        // Ajustes abiertos: capturan el teclado.
        if self.st().settings.is_some() {
            let mods = self.st().mods;
            if self.keys.lookup(&ev.logical_key, mods) == Some(crate::keybindings::Action::Settings)
            {
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
                Key::Named(NamedKey::Tab) | Key::Named(NamedKey::PageDown) => {
                    Some(Nav::NextCategory)
                }
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
            if self.keys.lookup(&ev.logical_key, mods)
                == Some(crate::keybindings::Action::CommandPalette)
            {
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
                    if let Some(t) = ev
                        .text
                        .as_ref()
                        .filter(|t| t.chars().all(|c| !c.is_control()))
                    {
                        pal.query.push_str(t);
                        pal.refilter();
                    }
                }
            }
            st.window.request_redraw();
            return false;
        }

        // Estadísticas abiertas: el teclado es suyo, salvo los atajos que tienen sentido con ellas.
        if self.st().stats.is_some() {
            return self.on_stats_key(&ev);
        }

        // Búsqueda abierta (Ctrl+Shift+F): el teclado va a la barra.
        if self.search_open() {
            return self.on_search_key(&ev);
        }

        // Menú de completado abierto: ↑ ↓ (o Tab / Shift+Tab) eligen, Intro o → aceptan y Esc
        // cierra. El resto de teclas llega al shell y el menú se refina con lo escrito (`App::draw`).
        if self.on_menu_key(&ev) {
            return false;
        }

        let mods = self.st().mods;
        // Windows: Ctrl+C con texto seleccionado lo copia, como en Windows Terminal. Sin
        // selección llega al programa (interrumpir).
        #[cfg(windows)]
        if mods == ModifiersState::CONTROL
            && matches!(&ev.logical_key, Key::Character(c) if c.eq_ignore_ascii_case("c"))
            && self.copy_and_clear_selection()
        {
            return false;
        }
        if let Some(action) = self.keys.lookup(&ev.logical_key, mods) {
            use crate::keybindings::Action::{FocusDown, FocusLeft, FocusRight, FocusUp};
            let pane_nav = matches!(action, FocusLeft | FocusRight | FocusUp | FocusDown);
            if !pane_nav || self.has_splits() {
                return self.run_action(action);
            }
        }

        let tcfg = &self.cfg.terminal;
        let (autosuggest, tab_accepts, as_you_type) = (
            tcfg.autosuggest,
            tcfg.tab_key == crate::config::TabKey::Accept,
            tcfg.complete_as_you_type,
        );
        let st = self.st();
        let Some(tab) = st.tabs.get(st.active) else {
            return false;
        };
        let key = &ev.logical_key;
        let plain = mods.is_empty();
        let ctrl = mods == ModifiersState::CONTROL;
        let tab_key = plain && matches!(key, Key::Named(NamedKey::Tab));
        // Sugerencia en gris: → (o Ctrl+F) la acepta entera y Ctrl+→ hasta la siguiente palabra;
        // con "Tecla Tab: Sugerencia", también Tab.
        let accept_all = (plain && matches!(key, Key::Named(NamedKey::ArrowRight)))
            || (ctrl && matches!(key, Key::Character(c) if c.eq_ignore_ascii_case("f")))
            || (tab_key && tab_accepts);
        let accept_word = ctrl && matches!(key, Key::Named(NamedKey::ArrowRight));
        if autosuggest && (accept_all || accept_word) {
            if let Some(rest) = tab.suggestion() {
                let text = if accept_word {
                    crate::suggest::next_word(&rest).to_string()
                } else {
                    rest
                };
                tab.input(text.into_bytes());
                self.scroll_to_bottom();
                return false;
            }
        }
        // Menú de completado: Tab o Ctrl+Espacio. Sin candidatos, la tecla llega al shell (su
        // completado de siempre).
        if (tab_key || (ctrl && matches!(key, Key::Named(NamedKey::Space)))) && self.open_menu() {
            return false;
        }
        let st = self.st();
        let Some(tab) = st.tabs.get(st.active) else {
            return false;
        };
        // Edición al estilo Warp (`edit.rs`): Shift+Intro, Ctrl+Retroceso/Supr, Ctrl+Z y ↑/↓ en
        // un comando de varias líneas.
        let vertical = plain && matches!(key, Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown));
        if crate::edit::is_candidate(key, mods) || vertical {
            let ctx = tab.edit_context();
            let bytes = if vertical {
                matches!(ctx, crate::edit::Context::Prompt(_))
                    .then(|| tab.vertical_moves(matches!(key, Key::Named(NamedKey::ArrowUp))))
                    .flatten()
            } else {
                crate::edit::shortcut(key, mods, ctx).map(<[u8]>::to_vec)
            };
            if let Some(b) = bytes {
                tab.term.lock().selection = None;
                tab.input(b);
                self.scroll_to_bottom();
                return false;
            }
        }
        let mode = *tab.term.lock().mode();
        // En Windows, si ConPTY lo pidió, la tecla exacta como evento de Windows.
        #[cfg(windows)]
        let encoded = if tab.blocks.lock().unwrap().win32_input {
            input::win32_key(&ev)
        } else {
            input::key_to_bytes(&ev, mods, mode)
        };
        #[cfg(not(windows))]
        let encoded = input::key_to_bytes(&ev, mods, mode);
        if let Some(bytes) = encoded {
            {
                let mut term = tab.term.lock();
                term.selection = None;
            }
            // "Menú al escribir": se recuerda lo que había para abrir el menú cuando llegue el eco.
            let typed = !mods.control_key()
                && !mods.alt_key()
                && ev
                    .text
                    .as_ref()
                    .is_some_and(|t| t.chars().all(|c| !c.is_control()));
            let before = (as_you_type && typed && st.menu.is_none())
                .then(|| tab.prompt_input())
                .flatten();
            tab.input(bytes);
            let st = self.st();
            if let Some(b) = before {
                st.menu_auto = Some((b, Instant::now()));
            }
            let active = st.active;
            if st
                .tabs
                .get_mut(active)
                .is_some_and(|t| t.selected_block.take().is_some())
            {
                st.window.request_redraw();
            }
            self.scroll_to_bottom();
        }
        false
    }

    /// ¿Hay una búsqueda abierta en la pestaña activa?
    fn search_open(&self) -> bool {
        let Some(st) = self.st.as_ref() else {
            return false;
        };
        st.search
            .as_ref()
            .is_some_and(|s| st.tabs.get(st.active).is_some_and(|t| t.id == s.tab))
    }

    /// Teclas con la búsqueda abierta: escribir busca, Intro / ↑ van a la anterior, Shift+Intro /
    /// ↓ a la siguiente y Esc cierra. Los atajos de Garbo siguen funcionando (Ctrl+Shift+F cierra;
    /// pegar pega en lo buscado).
    fn on_search_key(&mut self, ev: &KeyEvent) -> bool {
        use crate::keybindings::Action;
        let mods = self.st().mods;
        let mut pasted = None;
        match self.keys.lookup(&ev.logical_key, mods) {
            Some(Action::Paste) => {
                let text = self.st().clipboard.get_contents().unwrap_or_default();
                pasted = Some(
                    text.lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .filter(|c| !c.is_control())
                        .collect::<String>(),
                );
            }
            Some(action) => return self.run_action(action),
            None => {}
        }
        let st = self.st();
        if ev.logical_key == Key::Named(NamedKey::Escape) {
            st.search = None;
            st.window.request_redraw();
            return false;
        }
        let Some(tab) = st.tabs.get(st.active) else {
            return false;
        };
        let Some(search) = st.search.as_mut() else {
            return false;
        };
        let typed = ev.text.as_ref().filter(|t| {
            !mods.control_key() && !mods.alt_key() && t.chars().all(|c| !c.is_control())
        });
        match &ev.logical_key {
            Key::Named(NamedKey::Enter) => search.step(&mut tab.term.lock(), !mods.shift_key()),
            Key::Named(NamedKey::ArrowUp) => search.step(&mut tab.term.lock(), true),
            Key::Named(NamedKey::ArrowDown) => search.step(&mut tab.term.lock(), false),
            Key::Named(NamedKey::Backspace) => {
                if mods.control_key() {
                    search.query.clear();
                } else {
                    search.query.pop();
                }
                search.update(&mut tab.term.lock());
            }
            _ => {
                if let Some(t) = pasted.as_deref().or(typed.map(|t| t.as_str())) {
                    search.query.push_str(t);
                    search.update(&mut tab.term.lock());
                }
            }
        }
        st.window.request_redraw();
        false
    }

    /// Completa la palabra del prompt: con un único candidato lo escribe; con varios abre el menú
    /// (escribiendo antes lo que todos comparten). `false` = no hay nada que ofrecer.
    pub fn open_menu(&mut self) -> bool {
        let st = self.st();
        let Some(tab) = st.tabs.get(st.active) else {
            return false;
        };
        let Some((input, comp)) = tab.completions() else {
            return false;
        };
        match comp.items.len() {
            0 => return false,
            1 => {
                if let Some(b) = comp.accept_bytes(0) {
                    tab.input(b);
                }
            }
            _ => {
                if let Some(ext) = comp.common_extension() {
                    tab.input(ext);
                }
                st.menu = Some(crate::complete::Menu::new(tab.id, input, comp));
                st.menu_auto = None;
                st.window.request_redraw();
            }
        }
        self.scroll_to_bottom();
        true
    }

    /// Teclas de navegación con el menú de completado abierto. `true` = tecla consumida.
    fn on_menu_key(&mut self, ev: &KeyEvent) -> bool {
        let st = self.st();
        if st.menu.is_none() {
            return false;
        }
        let plain = st.mods.is_empty();
        let shift = st.mods == ModifiersState::SHIFT;
        let auto = st.menu.as_ref().is_some_and(|m| m.auto);
        let nav = match &ev.logical_key {
            Key::Named(NamedKey::Escape) => MenuNav::Close,
            // Abierto solo al escribir: Intro ejecuta lo escrito (el menú se cierra) y Tab acepta.
            Key::Named(NamedKey::Enter) if plain && auto => {
                st.menu = None;
                return false;
            }
            Key::Named(NamedKey::Tab) if plain && auto => MenuNav::Accept,
            Key::Named(NamedKey::ArrowUp) if plain => MenuNav::Move(-1),
            Key::Named(NamedKey::ArrowDown) if plain => MenuNav::Move(1),
            Key::Named(NamedKey::Tab) if shift => MenuNav::Move(-1),
            Key::Named(NamedKey::Tab) if plain => MenuNav::Move(1),
            Key::Named(NamedKey::PageUp) if plain => MenuNav::Page(-1),
            Key::Named(NamedKey::PageDown) if plain => MenuNav::Page(1),
            Key::Named(NamedKey::Enter | NamedKey::ArrowRight) if plain => MenuNav::Accept,
            _ => return false,
        };
        self.menu_nav(nav);
        true
    }

    pub fn menu_nav(&mut self, nav: MenuNav) {
        let st = self.st();
        let Some(menu) = st.menu.as_mut() else { return };
        match nav {
            MenuNav::Close => st.menu = None,
            MenuNav::Move(d) => menu.move_sel(d),
            MenuNav::Page(d) => menu.page(d),
            MenuNav::Accept => {
                let (bytes, id) = (menu.accept(), menu.tab);
                st.menu = None;
                if let (Some(b), Some(tab)) = (bytes, st.tabs.iter().find(|t| t.id == id)) {
                    tab.input(b);
                }
                self.scroll_to_bottom();
            }
        }
        self.st().window.request_redraw();
    }

    /// Pixel -> celda del viewport (columna, fila, lado).
    fn cell_at(&self, x: f32, y: f32) -> (usize, usize, Side) {
        let geo = self.geometry();
        let m = self.st.as_ref().unwrap().glyphs.fonts.metrics;
        let fx = ((x - geo.x) / m.width).max(0.0);
        let fy = ((y - geo.y) / m.height).max(0.0);
        let col = (fx as usize).min(geo.ws.num_cols as usize - 1);
        let row = (fy as usize).min(geo.ws.num_lines as usize - 1);
        let side = if fx.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        (col, row, side)
    }

    fn on_mouse_move(&mut self, x: f32, y: f32) {
        let geo = self.geometry();
        // Editor de aspecto: todo lo que queda bajo la barra de pestañas es suyo.
        let in_bar = geo.tab_bar.as_ref().is_some_and(|tb| y < tb.height);
        let dragging = self.st().look.as_ref().is_some_and(|l| l.dragging);
        if self.st().look.is_some() && (!in_bar || dragging) {
            use crate::look::Hit;
            let top = geo.tab_bar.as_ref().map_or(0.0, |t| t.height);
            let st = self.st();
            st.mouse = (x, y);
            let (_, h) = st.renderer.size();
            let look = st.look.as_mut().unwrap();
            if look.dragging {
                look.drag_to(y, h, top);
                st.window.request_redraw();
            } else {
                let hit = look.hit(x, y);
                if look.hover != Some(hit) {
                    look.hover = Some(hit);
                    st.window.request_redraw();
                }
            }
            let icon = if look.dragging || look.hover == Some(Hit::Divider) {
                CursorIcon::RowResize
            } else {
                CursorIcon::Default
            };
            if st.cursor_icon != icon {
                st.cursor_icon = icon;
                st.window.set_cursor(icon);
            }
            return;
        }
        if self.st().stats.is_some() && self.st().settings.is_none() {
            self.st().mouse = (x, y);
            self.on_stats_mouse_move(&geo, x, y);
            return;
        }
        let st = self.st();
        st.mouse = (x, y);
        if let Some(set) = st.settings.as_mut() {
            let (w, h) = st.renderer.size();
            let top = geo.tab_bar.as_ref().map_or(0.0, |t| t.height);
            let scale = st.scale;
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
        // Arrastrando una línea divisoria: cambia el reparto de los paneles.
        if let Some((path, side, area)) = self.st().drag.clone() {
            let ratio = if side {
                (x - area[0]) / area[2].max(1.0)
            } else {
                (y - area[1]) / area[3].max(1.0)
            };
            let st = self.st();
            let g = st.group;
            if let Some(group) = st.groups.get_mut(g) {
                group.layout.set_ratio(&path, ratio);
            }
            self.relayout();
            return;
        }
        let in_bar = geo.tab_bar.as_ref().is_some_and(|tb| y < tb.height);
        let in_gutter = !in_bar
            && self.blocks_active()
            && geo
                .panes
                .iter()
                .any(|p| x >= p.rect[0] && x < p.x && y >= p.y && y < p.rect[1] + p.rect[3]);
        let divider = (!in_bar)
            .then(|| divider_at(&geo, x, y, self.st().scale))
            .flatten();
        let on_link = !in_bar && !in_gutter && divider.is_none() && self.update_link(x, y);
        let icon = match self.resize_edge(x, y) {
            Some(dir) => resize_cursor(dir),
            None if in_bar => CursorIcon::Default,
            None if divider.is_some_and(|d| d.side) => CursorIcon::ColResize,
            None if divider.is_some() => CursorIcon::RowResize,
            None if in_gutter || on_link => CursorIcon::Pointer,
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
        let Some(tab) = st.tabs.get(st.active) else {
            return;
        };
        let mut term = tab.term.lock();
        let mode = *term.mode();
        let reporting = mode.intersects(TermMode::MOUSE_MODE) && !st.mods.shift_key();
        if reporting {
            let held = st.mouse_buttons != 0;
            if mode.contains(TermMode::MOUSE_MOTION)
                || (held && mode.contains(TermMode::MOUSE_DRAG))
            {
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

        // Editor de aspecto (la barra de pestañas sigue funcionando).
        if self.st().look.is_some() {
            use crate::look::Hit;
            use crate::settings::Change;
            let in_bar = geo.tab_bar.as_ref().is_some_and(|tb| y < tb.height);
            let st = self.st.as_mut().unwrap();
            let look = st.look.as_mut().unwrap();
            if !pressed && look.dragging {
                look.dragging = false;
                st.look_ratio = look.ratio;
                return false;
            }
            if !in_bar {
                if !pressed || button != MouseButton::Left {
                    return false;
                }
                let change = match look.hit(x, y) {
                    Hit::Divider => {
                        look.dragging = true;
                        Change::None
                    }
                    Hit::Close => {
                        self.close_look();
                        return false;
                    }
                    Hit::Sample => {
                        look.sample = !look.sample;
                        Change::None
                    }
                    Hit::Keep => {
                        look.keep = !look.keep;
                        Change::None
                    }
                    Hit::Reset => look.reset(&mut self.cfg),
                    Hit::Option(g, i) => look.choose(g, i, &mut self.cfg),
                    Hit::Panel | Hit::Above => Change::None,
                };
                st.window.request_redraw();
                return self.apply_setting(change);
            }
        }

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

        // Estadísticas: lo que queda bajo la barra de pestañas es suyo.
        let in_bar = geo.tab_bar.as_ref().is_some_and(|tb| y < tb.height);
        if self.st().stats.is_some() && !in_bar {
            if pressed && button == MouseButton::Left {
                if let Some(dir) = self.resize_edge(x, y) {
                    let _ = self.st().window.drag_resize_window(dir);
                    return false;
                }
                self.on_stats_click(&geo, x, y);
            }
            return false;
        }

        // Barra de búsqueda: sus botones (el resto de clics sigue su camino).
        if pressed && button == MouseButton::Left && self.search_open() {
            let st = self.st();
            if let Some(hit) = st.search.as_ref().and_then(|s| s.hit(x, y)) {
                use crate::search::Hit;
                if hit == Hit::Close {
                    st.search = None;
                } else if let (Some(tab), Some(search)) =
                    (st.tabs.get(st.active), st.search.as_mut())
                {
                    let mut term = tab.term.lock();
                    match hit {
                        Hit::Prev => search.step(&mut term, true),
                        Hit::Next => search.step(&mut term, false),
                        Hit::Block => {
                            search.block_only = !search.block_only;
                            search.update(&mut term);
                        }
                        _ => {}
                    }
                }
                st.window.request_redraw();
                return false;
            }
        }

        // Menú de completado: un clic en una opción la elige; fuera, lo cierra (y el clic sigue).
        if pressed && self.st().menu.is_some() {
            let st = self.st();
            let menu = st.menu.as_mut().unwrap();
            if menu.contains(x, y) {
                if let Some(i) = menu.row_at(x, y).filter(|_| button == MouseButton::Left) {
                    menu.selected = i;
                    let (bytes, id) = (menu.accept(), menu.tab);
                    st.menu = None;
                    if let (Some(b), Some(tab)) = (bytes, st.tabs.iter().find(|t| t.id == id)) {
                        tab.input(b);
                    }
                }
                st.window.request_redraw();
                return false;
            }
            st.menu = None;
            st.window.request_redraw();
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
                let double = now.duration_since(st.last_click.0)
                    < crate::platform::double_click_time()
                    && st.last_click.1 == 1;
                st.last_click = (now, if double { 0 } else { 1 });
                match (tb.hit(x, y), button) {
                    (Some(TabHit::Tab(i)), MouseButton::Left) => {
                        self.close_stats();
                        self.select_group(i)
                    }
                    (Some(TabHit::Tab(i) | TabHit::Close(i)), MouseButton::Middle)
                    | (Some(TabHit::Close(i)), MouseButton::Left) => return self.close_tab(i),
                    (Some(TabHit::NewTab), _) => {
                        self.close_stats();
                        self.new_tab()
                    }
                    (Some(TabHit::Update), MouseButton::Left) => return self.on_update_click(),
                    (Some(TabHit::Stats), MouseButton::Left) => self.toggle_stats(),
                    (Some(TabHit::Agents), MouseButton::Left) => {
                        self.close_stats();
                        self.toggle_panel(true)
                    }
                    (Some(TabHit::Ports), MouseButton::Left) => {
                        self.close_stats();
                        self.toggle_panel(false)
                    }
                    (Some(TabHit::Settings), MouseButton::Left) => {
                        self.run_action(crate::keybindings::Action::Settings);
                    }
                    (Some(TabHit::Minimize), MouseButton::Left) => {
                        self.st().window.set_minimized(true)
                    }
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

        // Paneles: arrastrar una línea divisoria los redimensiona; un clic en otro panel le da el foco.
        if button == MouseButton::Left && !pressed && self.st().drag.take().is_some() {
            self.st().window.request_redraw();
            return false;
        }
        if pressed && button == MouseButton::Left {
            if let Some(d) = divider_at(&geo, x, y, self.st().scale) {
                let drag = (d.path.clone(), d.side, d.area);
                self.st().drag = Some(drag);
                self.st().window.request_redraw();
                return false;
            }
        }
        let geo = match geo.panes.iter().find(|p| {
            let [px, py, pw, ph] = p.rect;
            x >= px && x < px + pw && y >= py && y < py + ph
        }) {
            Some(p) if pressed && p.idx != self.st().active => {
                self.select_tab(p.idx);
                self.geometry()
            }
            _ => geo,
        };

        // Margen izquierdo: clic selecciona (o deselecciona) el bloque de esa línea.
        if pressed
            && button == MouseButton::Left
            && x >= geo.pane[0]
            && x < geo.x
            && y >= geo.y
            && self.blocks_active()
        {
            let row = ((y - geo.y) / self.st().glyphs.fonts.metrics.height).max(0.0) as i32;
            let st = self.st();
            if let Some(tab) = st.tabs.get_mut(st.active) {
                let term = tab.term.lock();
                let line = Line(
                    row.min(term.screen_lines() as i32 - 1) - term.grid().display_offset() as i32,
                );
                let idle = tab.blocks.lock().unwrap().idle_current();
                let hit = blocks::anchor_at(&term, line, 10_000)
                    .map(|a| a.id)
                    .filter(|id| Some(*id) != idle);
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

        // Ctrl+clic en un enlace o en una ruta: se abre (también en programas que usan el ratón).
        if pressed && button == MouseButton::Left && self.st().mods == ModifiersState::CONTROL {
            self.update_link(x, y);
            if let Some((_, _, Some(found))) = self.st().link.clone() {
                self.open_link(found.target);
                return false;
            }
        }

        let st = self.st();
        let Some(tab) = st.tabs.get(st.active) else {
            return false;
        };
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
                let count =
                    if now.duration_since(st.last_click.0) < crate::platform::double_click_time() {
                        st.last_click.1 % 3 + 1
                    } else {
                        1
                    };
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
                    let empty = tab
                        .term
                        .lock()
                        .selection
                        .as_ref()
                        .is_none_or(|s| s.is_empty());
                    if empty {
                        tab.term.lock().selection = None;
                        // Un clic (sin arrastrar) en lo escrito en el prompt lleva el cursor ahí.
                        if st.last_click.1 == 1 && st.mods.is_empty() {
                            let offset = tab.term.lock().grid().display_offset() as i32;
                            let point = Point::new(Line(row as i32 - offset), Column(col));
                            if let Some(b) = tab.click_moves(point, side) {
                                tab.input(b);
                            }
                        }
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
            // Windows: el clic derecho copia lo seleccionado o, si no hay nada, pega (como en
            // Windows Terminal y la consola de siempre).
            #[cfg(windows)]
            (MouseButton::Right, true) => self.copy_or_paste(),
            _ => {}
        }
        false
    }

    /// Con Ctrl pulsado, busca el enlace o la ruta bajo el ratón (solo al cambiar de celda) para
    /// subrayarlo. Devuelve si hay uno.
    fn update_link(&mut self, x: f32, y: f32) -> bool {
        let cell = self.cell_at(x, y);
        let st = self.st();
        if !st.mods.control_key() || st.settings.is_some() || st.palette.is_some() {
            if st.link.take().is_some_and(|l| l.2.is_some()) {
                st.window.request_redraw();
            }
            return false;
        }
        let Some(tab) = st.tabs.get(st.active) else {
            return false;
        };
        let key = (cell.0, cell.1);
        if let Some((id, k, found)) = &st.link {
            if *id == tab.id && *k == key {
                return found.is_some();
            }
        }
        let cwd = tab.cwd();
        let found = {
            let term = tab.term.lock();
            let offset = term.grid().display_offset() as i32;
            crate::links::find(
                &term,
                Point::new(Line(cell.1 as i32 - offset), Column(cell.0)),
                cwd.as_deref(),
            )
        };
        let changed = st.link.as_ref().map(|l| &l.2) != Some(&found);
        let has = found.is_some();
        st.link = Some((tab.id, key, found));
        if changed {
            st.window.request_redraw();
        }
        has
    }

    fn blocks_active(&self) -> bool {
        let Some(st) = self.st.as_ref() else {
            return false;
        };
        self.cfg.terminal.blocks
            && st
                .tabs
                .get(st.active)
                .is_some_and(|t| t.blocks.lock().unwrap().active)
    }

    fn toggle_maximized(&mut self) {
        let w = &self.st().window;
        w.set_maximized(!w.is_maximized());
    }

    /// Borde de la ventana bajo el ratón, si la barra de título es nuestra.
    fn resize_edge(&self, x: f32, y: f32) -> Option<ResizeDirection> {
        let st = self.st.as_ref()?;
        if st.decorations || st.window.is_maximized() {
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
        if let Some(menu) = st
            .menu
            .as_mut()
            .filter(|m| m.contains(st.mouse.0, st.mouse.1))
        {
            // Sobre el menú de completado, la rueda recorre sus opciones.
            let steps = match delta {
                MouseScrollDelta::LineDelta(_, y) => y,
                MouseScrollDelta::PixelDelta(p) => p.y as f32 / (28.0 * st.scale),
            };
            let n = menu.wheel(steps);
            if n != 0 {
                menu.move_sel(-n);
                st.window.request_redraw();
            }
            return;
        }
        if let Some(look) = st.look.as_mut() {
            // Sobre las opciones, las desplaza; sobre el terminal, nada (se ve la zona del cursor).
            if look.in_panel(st.mouse.1) {
                let px = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 48.0 * st.scale,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                look.wheel(px);
                look.hover = Some(look.hit(st.mouse.0, st.mouse.1));
                st.window.request_redraw();
            }
            return;
        }
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
        if st.settings.is_none() {
            if let Some(stats) = st.stats.as_mut() {
                let px = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y * 48.0 * st.scale,
                    MouseScrollDelta::PixelDelta(p) => -p.y as f32,
                };
                let (w, h) = st.renderer.size();
                let (mx, my) = st.mouse;
                stats.scroll_by(px, w, h, top, st.scale);
                stats.hover = Some(stats.hit(mx, my, w, h, top, st.scale));
                st.window.request_redraw();
                return;
            }
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
        let Some(tab) = st.tabs.get(st.active) else {
            return;
        };
        let mode = *tab.term.lock().mode();
        if mode.intersects(TermMode::MOUSE_MODE) && !st.mods.shift_key() {
            let code = if n > 0 {
                MouseButtonCode::WheelUp
            } else {
                MouseButtonCode::WheelDown
            };
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

/// Apartado de estadísticas (ver `stats/view.rs`).
impl App {
    fn on_stats_key(&mut self, ev: &KeyEvent) -> bool {
        use crate::keybindings::Action;
        let mods = self.st().mods;
        match self.keys.lookup(&ev.logical_key, mods) {
            // Con las estadísticas abiertas.
            Some(
                a @ (Action::AgentStats
                | Action::CommandPalette
                | Action::Settings
                | Action::NextTheme
                | Action::ReloadConfig
                | Action::FontBigger
                | Action::FontSmaller
                | Action::FontReset),
            ) => return self.run_action(a),
            // Las que llevan a un terminal: primero se cierran.
            Some(
                a @ (Action::NewTab
                | Action::NextTab
                | Action::PrevTab
                | Action::GotoTab(_)
                | Action::MoveTabLeft
                | Action::MoveTabRight
                | Action::ToggleAgents
                | Action::TogglePorts
                | Action::Search
                | Action::SplitRight
                | Action::SplitDown),
            ) => {
                self.close_stats();
                return self.run_action(a);
            }
            _ => {}
        }
        let top = self.geometry().tab_bar.as_ref().map_or(0.0, |t| t.height);
        let st = self.st();
        let (w, h) = st.renderer.size();
        let s = st.scale;
        let shift = mods.shift_key();
        let Some(stats) = st.stats.as_mut() else {
            return false;
        };
        let page = ((h - top) * 0.8).max(48.0 * s);
        match &ev.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.close_stats();
                return false;
            }
            Key::Named(NamedKey::ArrowLeft) => stats.step_period(-1),
            Key::Named(NamedKey::ArrowRight) => stats.step_period(1),
            Key::Named(NamedKey::Tab) => stats.cycle_agent(shift),
            Key::Named(NamedKey::ArrowUp) => stats.scroll_by(-48.0 * s, w, h, top, s),
            Key::Named(NamedKey::ArrowDown) => stats.scroll_by(48.0 * s, w, h, top, s),
            Key::Named(NamedKey::PageUp) => stats.scroll_by(-page, w, h, top, s),
            Key::Named(NamedKey::PageDown) | Key::Named(NamedKey::Space) => {
                stats.scroll_by(page, w, h, top, s)
            }
            Key::Named(NamedKey::Home) => stats.scroll_to(false, w, h, top, s),
            Key::Named(NamedKey::End) => stats.scroll_to(true, w, h, top, s),
            Key::Character(c) => {
                if let Some(p) = c
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|i| crate::stats::Period::ALL.get(i))
                {
                    stats.set_period(*p);
                }
            }
            _ => {}
        }
        st.window.request_redraw();
        false
    }

    fn on_stats_mouse_move(&mut self, geo: &crate::app::Geometry, x: f32, y: f32) {
        let edge = self.resize_edge(x, y);
        let st = self.st();
        let (w, h) = st.renderer.size();
        let top = geo.tab_bar.as_ref().map_or(0.0, |t| t.height);
        let scale = st.scale;
        let bar_hover = geo.tab_bar.as_ref().and_then(|tb| tb.hit(x, y));
        if bar_hover != st.hover {
            st.hover = bar_hover;
            st.window.request_redraw();
        }
        let Some(stats) = st.stats.as_mut() else {
            return;
        };
        let hit = stats.hit(x, y, w, h, top, scale);
        stats.mouse = (x, y);
        // La ventanita de detalles sigue al ratón: se redibuja también si no cambia lo de debajo.
        let follows = matches!(
            hit,
            crate::stats::view::Hit::Bar(_) | crate::stats::view::Hit::Cell(..)
        );
        if stats.hover != Some(hit) || follows {
            stats.hover = Some(hit);
            st.window.request_redraw();
        }
        let icon = match edge {
            Some(dir) => resize_cursor(dir),
            None if hit.clickable() => CursorIcon::Pointer,
            None => CursorIcon::Default,
        };
        if icon != st.cursor_icon {
            st.cursor_icon = icon;
            st.window.set_cursor(icon);
        }
    }

    fn on_stats_click(&mut self, geo: &crate::app::Geometry, x: f32, y: f32) {
        use crate::stats::view::Hit;
        let st = self.st();
        let (w, h) = st.renderer.size();
        let top = geo.tab_bar.as_ref().map_or(0.0, |t| t.height);
        let scale = st.scale;
        let Some(stats) = st.stats.as_mut() else {
            return;
        };
        match stats.hit(x, y, w, h, top, scale) {
            Hit::Close => {
                self.close_stats();
                return;
            }
            Hit::Period(p) => stats.set_period(p),
            Hit::Agent(a) => stats.toggle_agent(a),
            _ => {}
        }
        // Lo de debajo del ratón puede haber cambiado (otras filas, otra escala).
        stats.hover = Some(stats.hit(x, y, w, h, top, scale));
        st.window.request_redraw();
    }
}
