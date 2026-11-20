//! Interfaz: barra de pestañas (que hace de barra de título) y paleta de comandos.
//! Todo se mide en píxeles lógicos × escala, así se ve igual de compacto en cualquier pantalla.

use crate::config::{Rgb, Theme};
use crate::palette::{Palette, MAX_VISIBLE};
use crate::view::Painter;

/// Color de superficie elevada (paneles, pestaña activa) derivado del tema.
pub fn surface(theme: &Theme, k: f32) -> Rgb {
    theme.background.lerp(theme.foreground, k)
}

pub fn dim(theme: &Theme) -> Rgb {
    theme.foreground.lerp(theme.background, 0.45)
}

/// Lo que la barra necesita saber de cada pestaña.
pub struct TabView {
    pub label: String,
    pub exited: bool,
    pub agent: Option<crate::sessions::Agent>,
    pub status: crate::sessions::Status,
}

pub struct TabBar {
    pub height: f32,
    pub tabs: Vec<(f32, f32)>,
    pub tab_y: f32,
    pub tab_h: f32,
    pub plus: (f32, f32),
    pub agents_btn: (f32, f32),
    pub ports_btn: (f32, f32),
    pub settings: (f32, f32),
    /// Píldora de actualización (x, ancho) si hay algo que mostrar.
    pub update: Option<(f32, f32)>,
    /// Botones de ventana (minimizar, maximizar, cerrar) cuando la barra hace de título.
    pub controls: Option<[(f32, f32); 3]>,
    close_w: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TabHit {
    Tab(usize),
    Close(usize),
    NewTab,
    Agents,
    Ports,
    Settings,
    Update,
    Minimize,
    Maximize,
    CloseWindow,
    /// Zona vacía de la barra: sirve para arrastrar la ventana.
    Empty,
}

/// Estado de los botones conmutables de la barra (y de la ventana, para el botón de maximizar).
#[derive(Clone, Copy, Default)]
pub struct Toggles {
    pub agents: bool,
    pub ports: bool,
    /// Solo en Windows: el botón de maximizar pasa a ser el de restaurar.
    pub maximized: bool,
}

/// Botones de ventana como los del sistema: en Windows, rectángulos de 46 px a toda la altura y
/// pegados al borde; en Linux, los redondos de GNOME.
const NATIVE_CONTROLS: bool = cfg!(windows);

/// Alto de la barra de pestañas en píxeles lógicos. En Windows, más baja: la escala suele ser 1 y
/// la barra de Linux se ve pesada.
pub const BAR_HEIGHT: f32 = if NATIVE_CONTROLS { 34.0 } else { 40.0 };

impl TabBar {
    pub fn layout(n: usize, width: f32, scale: f32, integrated: bool, has_update: bool) -> Self {
        let s = scale;
        let height = (BAR_HEIGHT * s).round();
        let tab_h = ((if NATIVE_CONTROLS { 26.0 } else { 28.0 }) * s).round();
        let tab_y = ((height - tab_h) / 2.0).round();
        let margin = (8.0 * s).round();
        let gap = (4.0 * s).round();
        let btn = tab_h;
        let (ctl, ctl_gap) = if NATIVE_CONTROLS {
            ((46.0 * s).round(), 0.0)
        } else {
            ((26.0 * s).round(), (8.0 * s).round())
        };

        let controls = integrated.then(|| {
            let x0 = if NATIVE_CONTROLS {
                width - 3.0 * ctl
            } else {
                width - margin - 3.0 * ctl - 2.0 * ctl_gap
            };
            [
                (x0, ctl),
                (x0 + ctl + ctl_gap, ctl),
                (x0 + 2.0 * (ctl + ctl_gap), ctl),
            ]
        });
        let right = controls.map_or(width - margin, |c| {
            c[0].0 - ((if NATIVE_CONTROLS { 6.0 } else { 12.0 }) * s).round()
        });
        let settings = (right - btn, btn);
        let ports_btn = (settings.0 - gap - btn, btn);
        let agents_btn = (ports_btn.0 - gap - btn, btn);
        let update = has_update.then(|| {
            let w = (200.0 * s).round();
            (agents_btn.0 - 2.0 * gap - w, w)
        });
        let tabs_end = update.map_or(agents_btn.0 - gap, |u| u.0);
        let avail = (tabs_end - margin - 2.0 * gap - btn).max(0.0);
        let tab_w = (avail / n.max(1) as f32).min(260.0 * s).floor();
        let tabs = (0..n).map(|i| (margin + i as f32 * tab_w, tab_w)).collect();
        let plus = (margin + n as f32 * tab_w + gap, btn);
        Self {
            height,
            tabs,
            tab_y,
            tab_h,
            plus,
            agents_btn,
            ports_btn,
            settings,
            update,
            controls,
            close_w: tab_h,
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<TabHit> {
        if y < 0.0 || y >= self.height {
            return None;
        }
        if let Some(c) = &self.controls {
            for (i, &(bx, bw)) in c.iter().enumerate() {
                if x >= bx && x < bx + bw {
                    return Some([TabHit::Minimize, TabHit::Maximize, TabHit::CloseWindow][i]);
                }
            }
        }
        let in_row = y >= self.tab_y && y < self.tab_y + self.tab_h;
        for (r, hit) in [
            (self.settings, TabHit::Settings),
            (self.ports_btn, TabHit::Ports),
            (self.agents_btn, TabHit::Agents),
        ] {
            if in_row && x >= r.0 && x < r.0 + r.1 {
                return Some(hit);
            }
        }
        if let Some((ux, uw)) = self.update {
            if in_row && x >= ux && x < ux + uw {
                return Some(TabHit::Update);
            }
        }
        // Las pestañas ocupan toda la altura de la barra.
        for (i, &(tx, tw)) in self.tabs.iter().enumerate() {
            if x >= tx && x < tx + tw {
                if in_row && x >= tx + tw - self.close_w {
                    return Some(TabHit::Close(i));
                }
                return Some(TabHit::Tab(i));
            }
        }
        if in_row && x >= self.plus.0 && x < self.plus.0 + self.plus.1 {
            return Some(TabHit::NewTab);
        }
        Some(TabHit::Empty)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        p: &mut Painter,
        theme: &Theme,
        width: f32,
        tabs: &[TabView],
        active: usize,
        hover: Option<TabHit>,
        scale: f32,
        update: Option<&crate::update::UpdateState>,
        toggles: Toggles,
        phase: usize,
    ) {
        use crate::sessions::Status;
        let s = scale;
        let r = (7.0 * s).round();
        let hair = s.round().max(1.0);
        let dimc = dim(theme);
        let pill = surface(theme, 0.08);
        p.rect(0.0, 0.0, width, self.height, theme.tab_bar, 1.0);
        p.rect(0.0, self.height - hair, width, hair, theme.foreground, 0.08);

        let hovered_tab = match hover {
            Some(TabHit::Tab(j) | TabHit::Close(j)) => Some(j),
            _ => None,
        };
        for (i, &(tx, tw)) in self.tabs.iter().enumerate() {
            let is_active = i == active;
            let hovered = hovered_tab == Some(i);
            // Pestañas planas a toda altura; la activa, en un tono más claro.
            if is_active {
                p.rect(tx, 0.0, tw, self.height - hair, pill, 1.0);
            } else if hovered {
                p.rect(tx, 0.0, tw, self.height - hair, pill, 0.5);
            }
            if i + 1 < self.tabs.len() && !is_active && i + 1 != active {
                p.rect(
                    tx + tw - hair,
                    10.0 * s,
                    hair,
                    self.height - 20.0 * s,
                    theme.foreground,
                    0.10,
                );
            }

            let tab = &tabs[i];
            let color = if is_active { theme.foreground } else { dimc };
            let pad = (12.0 * s).round();
            let show_close = is_active || hovered;
            let mut right = tx + tw - pad / 2.0;
            if show_close {
                let cx = (tx + tw - self.close_w / 2.0).round();
                let cy = (self.height / 2.0).round();
                let over = hover == Some(TabHit::Close(i));
                if over {
                    p.circle(cx, cy, 9.0 * s, theme.foreground, 0.12);
                }
                cross(
                    p,
                    cx,
                    cy,
                    3.5 * s,
                    1.4 * s,
                    if over { theme.foreground } else { dimc },
                );
                right = tx + tw - self.close_w;
            }
            // Estado del agente: animación mientras trabaja, punto amarillo si te espera y verde si
            // terminó sin mirarlo.
            if tab.agent.is_some() {
                let cy = (self.height / 2.0).round();
                match tab.status {
                    Status::Working => {
                        right -= 16.0 * s;
                        crate::panel::spinner(p, right, cy, s * 0.9, theme.accent, phase);
                        right -= 8.0 * s;
                    }
                    Status::Done | Status::Blocked(_) => {
                        let c = if tab.status == Status::Done {
                            theme.palette[2]
                        } else {
                            theme.palette[3]
                        };
                        right -= 8.0 * s;
                        p.circle(right, cy, 3.5 * s, c, 1.0);
                        right -= 8.0 * s;
                    }
                    _ => {}
                }
            }
            let label = if tab.exited {
                format!("{} (terminado)", tab.label)
            } else {
                tab.label.clone()
            };
            p.ui_text_in(
                tx + pad,
                0.0,
                self.height,
                &label,
                color,
                (right - tx - pad).max(0.0),
            );
        }

        let cy = (self.tab_y + self.tab_h / 2.0).round();
        // Nueva pestaña
        let (px, pw) = self.plus;
        let over = hover == Some(TabHit::NewTab);
        if over {
            p.rounded(px, self.tab_y, pw, self.tab_h, pill, 0.8, r);
        }
        let c = if over { theme.foreground } else { dimc };
        let cx = (px + pw / 2.0).round();
        let k = 5.0 * s;
        p.line(cx - k, cy, cx + k, cy, 1.5 * s, c, 1.0);
        p.line(cx, cy - k, cx, cy + k, 1.5 * s, c, 1.0);

        // Botones conmutables y menú
        let button = |p: &mut Painter, rect: (f32, f32), hit: TabHit, on: bool| -> Rgb {
            let over = hover == Some(hit);
            if on {
                p.rounded(
                    rect.0,
                    self.tab_y,
                    rect.1,
                    self.tab_h,
                    theme.accent,
                    if over { 0.30 } else { 0.20 },
                    r,
                );
            } else if over {
                p.rounded(rect.0, self.tab_y, rect.1, self.tab_h, pill, 0.8, r);
            }
            if on {
                theme.accent
            } else if over {
                theme.foreground
            } else {
                dimc
            }
        };
        let c = button(p, self.agents_btn, TabHit::Agents, toggles.agents);
        let bx = (self.agents_btn.0 + self.agents_btn.1 / 2.0).round();
        p.rounded(
            bx - 6.0 * s,
            cy - 3.0 * s,
            12.0 * s,
            9.0 * s,
            c,
            1.0,
            2.5 * s,
        );
        let bg = if toggles.agents {
            theme.tab_bar.lerp(theme.accent, 0.2)
        } else {
            theme.tab_bar
        };
        p.circle(bx - 2.6 * s, cy + 1.5 * s, 1.4 * s, bg, 1.0);
        p.circle(bx + 2.6 * s, cy + 1.5 * s, 1.4 * s, bg, 1.0);
        p.line(bx, cy - 3.0 * s, bx, cy - 6.0 * s, 1.3 * s, c, 1.0);

        let c = button(p, self.ports_btn, TabHit::Ports, toggles.ports);
        let gx = (self.ports_btn.0 + self.ports_btn.1 / 2.0).round();
        p.ring(gx, cy, 6.0 * s, 1.4 * s, c, 1.0);
        p.line(gx - 6.0 * s, cy, gx + 6.0 * s, cy, 1.3 * s, c, 1.0);
        p.line(gx, cy - 6.0 * s, gx, cy + 6.0 * s, 1.3 * s, c, 1.0);

        let c = button(p, self.settings, TabHit::Settings, false);
        let mx = (self.settings.0 + self.settings.1 / 2.0).round();
        for dy in [-4.0, 0.0, 4.0] {
            p.line(
                mx - 6.0 * s,
                cy + dy * s,
                mx + 6.0 * s,
                cy + dy * s,
                1.5 * s,
                c,
                1.0,
            );
        }

        // Aviso de actualización
        if let (Some((ux, uw)), Some(state)) = (self.update, update) {
            use crate::update::UpdateState as U;
            let over = hover == Some(TabHit::Update);
            let (label, bg, bg_a, fg) = match state {
                U::Available(rel) => (
                    rel.button_label(),
                    theme.accent,
                    if over { 1.0 } else { 0.85 },
                    theme.background,
                ),
                U::Downloading(v) => (
                    format!("Descargando {v}…"),
                    theme.accent,
                    0.25,
                    theme.foreground,
                ),
                U::Installing(v) => (
                    format!("Instalando {v}…"),
                    theme.accent,
                    0.25,
                    theme.foreground,
                ),
                U::Installed(v) => (
                    format!("Reiniciar para usar {v}"),
                    theme.palette[2],
                    if over { 1.0 } else { 0.85 },
                    theme.background,
                ),
                U::Failed(_) => (
                    "No se pudo actualizar".to_string(),
                    theme.palette[1],
                    0.22,
                    theme.palette[1],
                ),
            };
            let pad = (12.0 * s).round();
            let tw = p.ui_measure(&label).min(uw - 2.0 * pad);
            let w = (tw + 2.0 * pad).round();
            let x = ux + uw - w;
            p.rounded(
                x,
                self.tab_y,
                w,
                self.tab_h,
                bg,
                bg_a,
                (self.tab_h / 2.0).round(),
            );
            p.ui_text_in(x + pad, self.tab_y, self.tab_h, &label, fg, tw + 1.0);
        }

        if let Some(ctl) = &self.controls {
            if NATIVE_CONTROLS {
                self.draw_native_controls(p, theme, ctl, hover, s, toggles.maximized);
                return;
            }
            // Botones de ventana estilo GNOME: círculos con iconos finos.
            let kinds = [TabHit::Minimize, TabHit::Maximize, TabHit::CloseWindow];
            for (i, &(bx, bw)) in ctl.iter().enumerate() {
                let over = hover == Some(kinds[i]);
                let cx = (bx + bw / 2.0).round();
                let cy = (self.height / 2.0).round();
                p.circle(
                    cx,
                    cy,
                    12.0 * s,
                    theme.foreground,
                    if over { 0.18 } else { 0.08 },
                );
                let c = theme.foreground;
                let t = 1.4 * s;
                match kinds[i] {
                    TabHit::Minimize => p.line(
                        cx - 4.0 * s,
                        cy + 0.5 * s,
                        cx + 4.0 * s,
                        cy + 0.5 * s,
                        t,
                        c,
                        1.0,
                    ),
                    TabHit::Maximize => {
                        let k = 3.6 * s;
                        p.line(cx - k, cy - k, cx + k, cy - k, t, c, 1.0);
                        p.line(cx + k, cy - k, cx + k, cy + k, t, c, 1.0);
                        p.line(cx + k, cy + k, cx - k, cy + k, t, c, 1.0);
                        p.line(cx - k, cy + k, cx - k, cy - k, t, c, 1.0);
                    }
                    _ => cross(p, cx, cy, 3.6 * s, t, c),
                }
            }
        }
    }

    /// Botones de ventana de Windows 11: iconos de 10 px con trazo de 1 px, fondo tenue al pasar
    /// el ratón y rojo en el de cerrar.
    fn draw_native_controls(
        &self,
        p: &mut Painter,
        theme: &Theme,
        ctl: &[(f32, f32); 3],
        hover: Option<TabHit>,
        s: f32,
        maximized: bool,
    ) {
        let hair = s.round().max(1.0);
        let k = (10.0 * s).round();
        let cy = ((self.height - hair) / 2.0).floor();
        let kinds = [TabHit::Minimize, TabHit::Maximize, TabHit::CloseWindow];
        for (i, &(bx, bw)) in ctl.iter().enumerate() {
            let over = hover == Some(kinds[i]);
            let close = kinds[i] == TabHit::CloseWindow;
            let mut c = theme.foreground;
            if over && close {
                p.rect(bx, 0.0, bw, self.height - hair, Rgb(0xc4, 0x2b, 0x1c), 1.0);
                c = Rgb(0xff, 0xff, 0xff);
            } else if over {
                p.rect(bx, 0.0, bw, self.height - hair, theme.foreground, 0.08);
            }
            let x0 = (bx + (bw - k) / 2.0).round();
            let y0 = (cy - k / 2.0).round();
            match kinds[i] {
                TabHit::Minimize => p.rect(x0, cy, k, hair, c, 1.0),
                TabHit::Maximize if maximized => {
                    // Restaurar: un cuadro delante y el borde del de detrás asomando.
                    let d = (2.0 * s).round();
                    square(p, x0, y0 + d, k - d, hair, c);
                    p.rect(x0 + d, y0, k - d, hair, c, 1.0);
                    p.rect(x0 + k - hair, y0, hair, k - d, c, 1.0);
                }
                TabHit::Maximize => square(p, x0, y0, k, hair, c),
                _ => {
                    let h = k / 2.0;
                    let (cx, cyc) = (x0 + h, y0 + h);
                    p.line(cx - h, cyc - h, cx + h, cyc + h, hair, c, 1.0);
                    p.line(cx - h, cyc + h, cx + h, cyc - h, hair, c, 1.0);
                }
            }
        }
    }
}

/// Contorno de un cuadrado de lado `k` con trazo `t`, alineado a píxeles.
fn square(p: &mut Painter, x: f32, y: f32, k: f32, t: f32, c: Rgb) {
    p.rect(x, y, k, t, c, 1.0);
    p.rect(x, y + k - t, k, t, c, 1.0);
    p.rect(x, y, t, k, c, 1.0);
    p.rect(x + k - t, y, t, k, c, 1.0);
}

pub fn cross(p: &mut Painter, cx: f32, cy: f32, k: f32, t: f32, c: Rgb) {
    p.line(cx - k, cy - k, cx + k, cy + k, t, c, 1.0);
    p.line(cx - k, cy + k, cx + k, cy - k, t, c, 1.0);
}

/// Carpeta (contorno con pestaña) en la caja (x, y, w, h), trazo `t`.
pub fn folder(p: &mut Painter, x: f32, y: f32, w: f32, h: f32, t: f32, c: Rgb) {
    let (x1, y1) = (x + w, y + h);
    let tab_w = (w * 0.42).round();
    let tab_h = (h * 0.24).round().max(t);
    let ly = y + tab_h;
    p.line(x, y, x + tab_w, y, t, c, 1.0);
    p.line(x + tab_w, y, x + tab_w + tab_h, ly, t, c, 1.0);
    p.line(x + tab_w + tab_h, ly, x1, ly, t, c, 1.0);
    p.line(x1, ly, x1, y1, t, c, 1.0);
    p.line(x1, y1, x, y1, t, c, 1.0);
    p.line(x, y1, x, y, t, c, 1.0);
}

/// Rama de git: tronco con dos nudos y una rama que sale hacia un tercero.
pub fn branch(p: &mut Painter, x: f32, y: f32, w: f32, h: f32, t: f32, c: Rgb) {
    let r = (w * 0.17).max(t);
    let (lx, rx) = (x + r, x + w - r);
    let (ty, by) = (y + r, y + h - r);
    p.ring(lx, ty, r, t * 0.8, c, 1.0);
    p.ring(lx, by, r, t * 0.8, c, 1.0);
    p.ring(rx, ty, r, t * 0.8, c, 1.0);
    p.line(lx, ty + r, lx, by - r, t, c, 1.0);
    let my = y + h * 0.62;
    p.line(rx, ty + r, rx, my - r, t, c, 1.0);
    p.line(rx, my - r, lx + r * 0.6, by - r * 1.2, t, c, 1.0);
}

pub fn magnifier(p: &mut Painter, cx: f32, cy: f32, s: f32, c: Rgb) {
    p.ring(cx - 1.0 * s, cy - 1.0 * s, 5.0 * s, 1.5 * s, c, 1.0);
    p.line(
        cx + 2.8 * s,
        cy + 2.8 * s,
        cx + 6.0 * s,
        cy + 6.0 * s,
        1.6 * s,
        c,
        1.0,
    );
}

/// Nombre bonito de una tecla para mostrarla en un "keycap".
fn key_label(k: &str) -> String {
    match k {
        "ctrl" | "control" => "Ctrl".into(),
        "shift" => "Shift".into(),
        "alt" => "Alt".into(),
        "super" => "Super".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        "pageup" => "RePág".into(),
        "pagedown" => "AvPág".into(),
        "plus" => "+".into(),
        "minus" => "−".into(),
        "equal" => "=".into(),
        "comma" => ",".into(),
        "period" => ".".into(),
        "insert" => "Ins".into(),
        "tab" => "Tab".into(),
        "enter" => "Intro".into(),
        "escape" => "Esc".into(),
        other => {
            let mut c = other.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        }
    }
}

/// Dibuja un atajo como teclas ("Ctrl" "Shift" "P") alineado a la derecha en `right`.
/// Devuelve el ancho ocupado.
#[allow(clippy::too_many_arguments)]
pub fn keycaps(
    p: &mut Painter,
    right: f32,
    y: f32,
    h: f32,
    combo: &str,
    theme: &Theme,
    s: f32,
) -> f32 {
    if combo.is_empty() {
        return 0.0;
    }
    let keys: Vec<String> = combo.split('+').map(|k| key_label(k.trim())).collect();
    let pad = (6.0 * s).round();
    let gap = (4.0 * s).round();
    let kh = (20.0 * s).round();
    let ky = (y + (h - kh) / 2.0).round();
    let mut x = right;
    for k in keys.iter().rev() {
        let tw = p.ui_measure(k);
        let w = (tw + 2.0 * pad).max(kh).round();
        x -= w;
        p.rounded(x, ky, w, kh, theme.foreground, 0.08, (5.0 * s).round());
        p.ui_text_in((x + (w - tw) / 2.0).round(), ky, kh, k, dim(theme), w);
        x -= gap;
    }
    right - x - gap
}

pub fn draw_palette(
    p: &mut Painter,
    theme: &Theme,
    pal: &Palette,
    width: f32,
    height: f32,
    top: f32,
    scale: f32,
) {
    let s = scale;
    p.rect(0.0, 0.0, width, height, Rgb(0, 0, 0), 0.35);

    let box_w = (600.0 * s).min(width - 32.0 * s).max(200.0 * s).round();
    let x = ((width - box_w) / 2.0).round();
    let y = (top + 48.0 * s).round();
    let pad = (10.0 * s).round();
    let row_h = (36.0 * s).round();
    let input_h = (50.0 * s).round();
    let visible = pal.filtered.len().min(MAX_VISIBLE);
    let list_h = if visible > 0 {
        visible as f32 * row_h + pad * 2.0
    } else {
        0.0
    };
    let box_h = input_h + list_h;
    let panel = surface(theme, 0.05);
    p.card(
        x,
        y,
        box_w,
        box_h,
        panel,
        theme.foreground,
        (12.0 * s).round(),
        s,
    );

    // Campo de búsqueda
    let dimc = dim(theme);
    magnifier(p, x + 22.0 * s, y + input_h / 2.0, s, dimc);
    let tx = x + 42.0 * s;
    let lh = p.ui_metrics().height;
    let tw = if pal.query.is_empty() {
        p.ui_text_in(
            tx,
            y,
            input_h,
            "Buscar acciones y temas…",
            dimc,
            box_w - 60.0 * s,
        );
        0.0
    } else {
        p.ui_text_in(
            tx,
            y,
            input_h,
            &pal.query,
            theme.foreground,
            box_w - 60.0 * s,
        )
    };
    p.rect(
        (tx + tw + 1.0).round(),
        (y + (input_h - lh) / 2.0).round(),
        (1.5 * s).round(),
        lh,
        theme.accent,
        1.0,
    );
    if visible > 0 {
        p.rect(
            x,
            y + input_h,
            box_w,
            s.round().max(1.0),
            theme.foreground,
            0.08,
        );
    }

    // Resultados
    let start = pal.scroll_start();
    for row in 0..visible {
        let idx = start + row;
        let Some(e) = pal.entry(idx) else { break };
        let ry = y + input_h + pad + row as f32 * row_h;
        if idx == pal.selected {
            p.rounded(
                x + pad,
                ry,
                box_w - 2.0 * pad,
                row_h,
                theme.accent,
                0.18,
                (8.0 * s).round(),
            );
        }
        let kw = keycaps(p, x + box_w - pad * 2.0, ry, row_h, &e.hint, theme, s);
        p.ui_text_in(
            x + pad * 2.0,
            ry,
            row_h,
            &e.label,
            theme.foreground,
            box_w - pad * 5.0 - kw,
        );
    }
}
