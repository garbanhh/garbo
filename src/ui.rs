//! Interfaz: barra de pestañas (que hace de barra de título) y paleta de comandos.
//! Todo se mide en píxeles lógicos × escala, así se ve igual de compacto en cualquier pantalla.

use crate::config::{Rgb, Theme};
use crate::palette::{Palette, MAX_VISIBLE};
use crate::style::{Buttons, Style, Tabs};
use crate::tabs::{Align, Marquee, Position, Rename, TabsConfig};
use crate::view::Painter;
use std::time::Instant;

/// Color de superficie elevada (paneles, pestaña activa) derivado del tema.
pub fn surface(theme: &Theme, k: f32) -> Rgb {
    theme.background.lerp(theme.foreground, k)
}

pub fn dim(theme: &Theme) -> Rgb {
    theme.foreground.lerp(theme.background, 0.45)
}

/// Intensidad del tinte de acento del fondo con degradado (arriba del todo).
pub const GRADIENT: f32 = 0.10;

/// Color del fondo del terminal justo bajo la barra (con el degradado, ya teñido).
pub fn terminal_top(theme: &Theme) -> Rgb {
    match theme.style.background {
        crate::style::Background::Gradient => theme.background.lerp(theme.accent, GRADIENT),
        crate::style::Background::Solid => theme.background,
    }
}

/// Muestra de una forma de pestañas para el editor de aspecto: tres pestañas (la segunda activa)
/// y, debajo, el terminal, en `r`. Usa el mismo dibujo que la barra de verdad, a escala `k`.
pub fn tab_sample(p: &mut crate::view::Painter, theme: &Theme, r: [f32; 4], k: f32) {
    let [x, y, w, h] = r;
    let bh = (h * 0.55).round();
    let tab_h = (bh - 10.0 * k).round().max(4.0);
    let margin = (5.0 * k).round();
    let n = 3;
    let tab_w = ((w - 2.0 * margin) / n as f32).floor();
    let bar = TabBar {
        height: bh,
        at: Position::Top,
        align: Align::Left,
        zone: [x, y, w, bh],
        thick: bh,
        tabs: (0..n)
            .map(|i| (x + margin + i as f32 * tab_w, tab_w))
            .collect(),
        pill: (((bh - tab_h) / 2.0).round(), tab_h),
        pill_inset: (2.0 * k).round(),
        plus: (0.0, 0.0),
        tab_y: 0.0,
        tab_h,
        stats_btn: (0.0, 0.0),
        changes_btn: (0.0, 0.0),
        agents_btn: (0.0, 0.0),
        ports_btn: (0.0, 0.0),
        settings: (0.0, 0.0),
        update: None,
        controls: None,
        close_w: tab_h,
        hide_tabs: false,
    };
    let st = theme.style;
    p.rect(x, y, w, bh, theme.tab_bar, 1.0);
    p.rect(x, y + bh, w, h - bh, terminal_top(theme), 1.0);
    if st.tabs == Tabs::Segmented {
        let pad = (2.0 * k).round();
        let [rx, ry, rw, rh] = bar.place(
            x + margin,
            bar.pill.0 - pad,
            n as f32 * tab_w,
            tab_h + 2.0 * pad,
        );
        p.rounded(rx, ry, rw, rh, theme.foreground, 0.06, st.r(k) + pad);
    }
    let hair = k.round().max(1.0);
    let mut gap = None;
    for i in 0..n {
        if let Some(g) = bar.draw_tab_shape(p, theme, i, 1, false, k) {
            gap = Some(g);
        }
        // El nombre, como una raya.
        let (tx, tw) = bar.tabs[i];
        let mut pad = (7.0 * k).round();
        if st.tabs == Tabs::Dot {
            pad += (11.0 * k).round();
        }
        let c = match (i == 1, st.tabs) {
            (true, Tabs::Brackets) => theme.accent,
            (true, _) => theme.foreground,
            _ => dim(theme),
        };
        let lh = (3.0 * k).round().max(2.0);
        let lw = (tw - pad - 6.0 * k).min(tw * 0.5).max(2.0);
        let ly = (y + bh / 2.0 - lh / 2.0).round();
        if st.tabs == Tabs::Brackets && i == 1 {
            let b = (2.0 * k).round();
            p.rect(tx + pad - 3.0 * b, ly - b, b, lh + 2.0 * b, c, 1.0);
            p.rect(tx + pad + lw + 2.0 * b, ly - b, b, lh + 2.0 * b, c, 1.0);
        }
        p.rounded(tx + pad, ly, lw, lh, c, 1.0, lh / 2.0);
    }
    match gap {
        Some((a, b)) => {
            p.rect(
                x,
                y + bh - hair,
                (a - x).max(0.0),
                hair,
                theme.foreground,
                0.08,
            );
            p.rect(
                b,
                y + bh - hair,
                (x + w - b).max(0.0),
                hair,
                theme.foreground,
                0.08,
            );
        }
        None => p.rect(x, y + bh - hair, w, hair, theme.foreground, 0.08),
    }
}

/// Muestra de los botones de ventana para el editor de aspecto: una barra con minimizar,
/// maximizar y cerrar al estilo `b`, a escala `k`.
pub fn buttons_sample(
    p: &mut crate::view::Painter,
    theme: &Theme,
    b: Buttons,
    r: [f32; 4],
    k: f32,
) {
    let [x, y, w, h] = r;
    let bh = (h * 0.55).round();
    p.rect(x, y, w, bh, theme.tab_bar, 1.0);
    p.rect(x, y + bh, w, h - bh, terminal_top(theme), 1.0);
    p.rect(
        x,
        y + bh - k.round().max(1.0),
        w,
        k.round().max(1.0),
        theme.foreground,
        0.08,
    );
    let cy = (y + bh / 2.0).round();
    let t = (1.3 * k).max(1.0);
    let icon = |p: &mut crate::view::Painter, i: usize, cx: f32, kk: f32, c: Rgb| match i {
        0 => p.line(cx - kk, cy + 0.5 * k, cx + kk, cy + 0.5 * k, t, c, 1.0),
        1 => {
            p.line(cx - kk, cy - kk, cx + kk, cy - kk, t, c, 1.0);
            p.line(cx + kk, cy - kk, cx + kk, cy + kk, t, c, 1.0);
            p.line(cx + kk, cy + kk, cx - kk, cy + kk, t, c, 1.0);
            p.line(cx - kk, cy + kk, cx - kk, cy - kk, t, c, 1.0);
        }
        _ => cross(p, cx, cy, kk, t, c),
    };
    match b {
        Buttons::Flat => {
            let cw = (24.0 * k).round();
            for i in 0..3 {
                let cx = x + w - (3 - i) as f32 * cw + cw / 2.0;
                icon(p, i, cx, 3.2 * k, theme.foreground);
            }
        }
        Buttons::Color => {
            for (i, c) in [theme.palette[3], theme.palette[2], theme.palette[1]]
                .into_iter()
                .enumerate()
            {
                let cx = x + w - (8.0 + (2 - i) as f32 * 15.0) * k;
                p.circle(cx.round(), cy, 5.0 * k, c, 0.92);
            }
        }
        Buttons::Icons => {
            for i in 0..3 {
                let cx = x + w - (11.0 + (2 - i) as f32 * 20.0) * k;
                icon(p, i, cx.round(), 3.4 * k, dim(theme));
            }
        }
        Buttons::Round => {
            for i in 0..3 {
                let cx = (x + w - (12.0 + (2 - i) as f32 * 22.0) * k).round();
                p.circle(cx, cy, 9.0 * k, theme.foreground, 0.08);
                icon(p, i, cx, 3.0 * k, theme.foreground);
            }
        }
    }
}

/// Botones de ventana rectangulares a toda altura: los de Windows o el estilo "Planos".
fn flat_controls(style: &Style) -> bool {
    NATIVE_CONTROLS || style.buttons == Buttons::Flat
}

/// Lo que la barra necesita saber de cada pestaña.
pub struct TabView {
    pub label: String,
    pub exited: bool,
    pub agent: Option<crate::sessions::Agent>,
    pub status: crate::sessions::Status,
}

/// Lo que la barra necesita además para dibujar las pestañas: el nombre que se desplaza (el de la
/// pestaña bajo el ratón), el campo para cambiar un nombre y la hora del fotograma.
pub struct TabsExtra<'a> {
    pub marquee: &'a mut Marquee,
    pub rename: Option<&'a Rename>,
    pub now: Instant,
}

pub struct TabBar {
    /// Alto de la barra de arriba (botones, ventana y, si van arriba, las pestañas).
    pub height: f32,
    /// Borde en el que van las pestañas y dónde va su nombre.
    pub at: Position,
    pub align: Align,
    /// Zona de las pestañas (x, y, ancho, alto): la propia barra si van arriba; si no, la franja
    /// de abajo o la columna de un lado.
    pub zone: [f32; 4],
    /// Grosor de la zona: del borde de la ventana al terminal.
    thick: f32,
    /// Cada pestaña a lo largo de la zona (inicio, largo): en x en una barra, en y en una columna.
    pub tabs: Vec<(f32, f32)>,
    /// Fondo de cada pestaña: (distancia al borde de la ventana, grosor) a lo ancho de la zona.
    pill: (f32, f32),
    /// Margen del fondo de cada pestaña a lo largo de la zona.
    pill_inset: f32,
    /// Nueva pestaña: (inicio, largo) a lo largo de la zona.
    pub plus: (f32, f32),
    /// Fila de botones de la barra de arriba.
    pub tab_y: f32,
    pub tab_h: f32,
    pub stats_btn: (f32, f32),
    /// Cambios de git (ancho 0 donde no está, ver `git::ENABLED`).
    pub changes_btn: (f32, f32),
    pub agents_btn: (f32, f32),
    pub ports_btn: (f32, f32),
    pub settings: (f32, f32),
    /// Píldora de actualización (x, ancho) si hay algo que mostrar.
    pub update: Option<(f32, f32)>,
    /// Botones de ventana (minimizar, maximizar, cerrar) cuando la barra hace de título.
    pub controls: Option<[(f32, f32); 3]>,
    close_w: f32,
    /// Pestañas ni dibujadas ni activas: las estadísticas ocupan la ventana y las pestañas no van
    /// en la barra de arriba.
    pub hide_tabs: bool,
}

fn inside(r: [f32; 4], x: f32, y: f32) -> bool {
    x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3]
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TabHit {
    Tab(usize),
    Close(usize),
    NewTab,
    Stats,
    /// Cambios de git.
    Changes,
    Agents,
    Ports,
    Settings,
    Update,
    Minimize,
    Maximize,
    CloseWindow,
    /// Zona vacía de la barra: sirve para arrastrar la ventana.
    Empty,
    /// Hueco de la franja o la columna de pestañas (fuera de la barra de arriba): doble clic o
    /// botón central abren una pestaña.
    Zone,
}

/// Estado de los botones conmutables de la barra (y de la ventana, para el botón de maximizar).
#[derive(Clone, Copy, Default)]
pub struct Toggles {
    pub stats: bool,
    pub changes: bool,
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
    /// Reparto de la barra de arriba y de la zona de las pestañas para una ventana de
    /// `width` × `win_h` píxeles. `side`: ancho de los paneles abiertos a la derecha del terminal
    /// (cambios, agentes); una fila de pestañas (arriba o abajo) acaba donde acaba el terminal, sin
    /// pasar por encima de ellos.
    #[allow(clippy::too_many_arguments)]
    pub fn layout(
        n: usize,
        width: f32,
        win_h: f32,
        scale: f32,
        integrated: bool,
        has_update: bool,
        style: &Style,
        cfg: TabsConfig,
        hide_tabs: bool,
        side: f32,
    ) -> Self {
        let s = scale;
        let logical_h = BAR_HEIGHT + style.bar_delta();
        let height = (logical_h * s).round();
        let tab_h = ((if NATIVE_CONTROLS {
            26.0
        } else {
            logical_h - 12.0
        }) * s)
            .round();
        let tab_y = ((height - tab_h) / 2.0).round();
        let margin = (8.0 * s).round();
        let gap = (4.0 * s).round();
        let btn = tab_h;
        let flat = flat_controls(style);
        // (ancho de cada botón, hueco entre ellos, distancia al borde derecho)
        let (ctl, ctl_gap, edge) = if flat {
            ((46.0 * s).round(), 0.0, 0.0)
        } else {
            match style.buttons {
                Buttons::Color => ((16.0 * s).round(), (6.0 * s).round(), margin + 4.0 * s),
                Buttons::Icons => ((30.0 * s).round(), (2.0 * s).round(), margin - 2.0 * s),
                _ => ((26.0 * s).round(), (8.0 * s).round(), margin),
            }
        };

        let controls = integrated.then(|| {
            let x0 = (width - edge - 3.0 * ctl - 2.0 * ctl_gap).round();
            [
                (x0, ctl),
                (x0 + ctl + ctl_gap, ctl),
                (x0 + 2.0 * (ctl + ctl_gap), ctl),
            ]
        });
        let right = controls.map_or(width - margin, |c| {
            c[0].0 - ((if flat { 6.0 } else { 12.0 }) * s).round()
        });
        let settings = (right - btn, btn);
        let ports_btn = (settings.0 - gap - btn, btn);
        let agents_btn = (ports_btn.0 - gap - btn, btn);
        let changes_btn = if crate::git::ENABLED {
            (agents_btn.0 - gap - btn, btn)
        } else {
            (agents_btn.0, 0.0)
        };
        let before_stats = if crate::git::ENABLED {
            changes_btn.0
        } else {
            agents_btn.0
        };
        let stats_btn = (before_stats - gap - btn, btn);
        let update = has_update.then(|| {
            let w = (200.0 * s).round();
            (stats_btn.0 - 2.0 * gap - w, w)
        });
        let tabs_end = update.map_or(stats_btn.0 - gap, |u| u.0);

        let at = cfg.position;
        let max_w = cfg.width.max_tab().map_or(f32::INFINITY, |w| w * s);
        // En una barra (arriba o abajo): de izquierda a derecha, con el + detrás.
        let row = |avail: f32| {
            let tab_w = (avail.max(0.0) / n.max(1) as f32).min(max_w).floor();
            let tabs: Vec<(f32, f32)> =
                (0..n).map(|i| (margin + i as f32 * tab_w, tab_w)).collect();
            (tabs, (margin + n as f32 * tab_w + gap, btn))
        };
        let inset = (3.0 * s).round();
        let (zone, thick, tabs, plus, pill, pill_inset) = match at {
            Position::Top => {
                let end = tabs_end.min(width - side);
                let (tabs, plus) = row(end - margin - 2.0 * gap - btn);
                (
                    [0.0, 0.0, width, height],
                    height,
                    tabs,
                    plus,
                    (tab_y, tab_h),
                    inset,
                )
            }
            Position::Bottom => {
                let (tabs, plus) = row(width - side - 2.0 * margin - 2.0 * gap - btn);
                let y = (win_h - height).max(height);
                (
                    [0.0, y, width, height],
                    height,
                    tabs,
                    plus,
                    (tab_y, tab_h),
                    inset,
                )
            }
            Position::Left | Position::Right => {
                // Una columna: una fila por pestaña y otra para la nueva. Si no caben, filas
                // más bajas.
                let w = (cfg.width.column() * s)
                    .round()
                    .min((width * 0.45).round())
                    .max((80.0 * s).round());
                let x = if at == Position::Left { 0.0 } else { width - w };
                let h = (win_h - height).max(0.0);
                let lead = (8.0 * s).round();
                let rows = (n + 1) as f32;
                let mut pitch = (tab_h + 6.0 * s).round();
                if 2.0 * lead + rows * pitch > h {
                    pitch = ((h - 2.0 * lead) / rows).floor().max((14.0 * s).round());
                }
                let y0 = height + lead;
                let tabs = (0..n).map(|i| (y0 + i as f32 * pitch, pitch)).collect();
                let side = (8.0 * s).round();
                (
                    [x, height, w, h],
                    w,
                    tabs,
                    (y0 + n as f32 * pitch, pitch),
                    (side, w - 2.0 * side),
                    ((pitch - tab_h) / 2.0).floor().max(s.round()),
                )
            }
        };
        Self {
            height,
            at,
            align: cfg.align,
            zone,
            thick,
            tabs,
            pill,
            pill_inset,
            plus,
            tab_y,
            tab_h,
            stats_btn,
            changes_btn,
            agents_btn,
            ports_btn,
            settings,
            update,
            controls,
            close_w: tab_h,
            hide_tabs: hide_tabs && at != Position::Top,
        }
    }

    fn horizontal(&self) -> bool {
        !self.at.vertical()
    }

    /// Rectángulo de la ventana para un tramo de la zona: `a` a lo largo (coordenada de la
    /// ventana: x en una barra, y en una columna) y `c` desde el borde de la ventana hacia el
    /// terminal. Así cada forma de pestaña se escribe una vez y sirve en los cuatro bordes.
    fn place(&self, a: f32, c: f32, la: f32, lc: f32) -> [f32; 4] {
        let [zx, zy, zw, zh] = self.zone;
        match self.at {
            Position::Top => [a, zy + c, la, lc],
            Position::Bottom => [a, zy + zh - c - lc, la, lc],
            Position::Left => [zx + c, a, lc, la],
            Position::Right => [zx + zw - c - lc, a, lc, la],
        }
    }

    fn point(&self, a: f32, c: f32) -> (f32, f32) {
        let [zx, zy, zw, zh] = self.zone;
        match self.at {
            Position::Top => (a, zy + c),
            Position::Bottom => (a, zy + zh - c),
            Position::Left => (zx + c, a),
            Position::Right => (zx + zw - c, a),
        }
    }

    /// Celda entera de una pestaña (todo el grosor de la zona).
    fn cell(&self, (a, la): (f32, f32)) -> [f32; 4] {
        self.place(a, 0.0, la, self.thick)
    }

    /// Fondo de una pestaña dentro de su celda.
    fn pill_rect(&self, (a, la): (f32, f32)) -> [f32; 4] {
        self.place(
            a + self.pill_inset,
            self.pill.0,
            la - 2.0 * self.pill_inset,
            self.pill.1,
        )
    }

    /// Botón de nueva pestaña.
    fn plus_rect(&self) -> [f32; 4] {
        if self.horizontal() {
            self.place(self.plus.0, self.pill.0, self.plus.1, self.pill.1)
        } else {
            self.pill_rect(self.plus)
        }
    }

    /// Borde derecho de lo que ocupa una pestaña (donde va la ×).
    fn tab_right(&self, t: (f32, f32)) -> f32 {
        if self.horizontal() {
            let [x, _, w, _] = self.cell(t);
            x + w
        } else {
            let [x, _, w, _] = self.pill_rect(t);
            x + w
        }
    }

    /// ¿Está el punto en la barra de arriba o en la zona de las pestañas?
    pub fn contains(&self, x: f32, y: f32) -> bool {
        (y >= 0.0 && y < self.height) || (!self.hide_tabs && inside(self.zone, x, y))
    }

    /// Rectángulo de la pestaña `i` (para saber si un clic cae dentro del campo del nombre).
    pub fn tab_rect(&self, i: usize) -> Option<[f32; 4]> {
        self.tabs.get(i).map(|&t| self.pill_rect(t))
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<TabHit> {
        let in_top = y >= 0.0 && y < self.height;
        if in_top {
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
                (self.changes_btn, TabHit::Changes),
                (self.stats_btn, TabHit::Stats),
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
        }
        if !self.hide_tabs && inside(self.zone, x, y) {
            // Las pestañas ocupan todo el grosor de la zona.
            for (i, &t) in self.tabs.iter().enumerate() {
                if inside(self.cell(t), x, y) {
                    let [_, py, _, ph] = self.pill_rect(t);
                    let right = self.tab_right(t);
                    if y >= py && y < py + ph && x >= right - self.close_w && x < right {
                        return Some(TabHit::Close(i));
                    }
                    return Some(TabHit::Tab(i));
                }
            }
            if inside(self.plus_rect(), x, y) {
                return Some(TabHit::NewTab);
            }
            if self.at != Position::Top {
                return Some(TabHit::Zone);
            }
        }
        in_top.then_some(TabHit::Empty)
    }

    /// Color del terminal junto a la pestaña (para las que se unen a él), con el degradado.
    fn joined(&self, theme: &Theme, a: f32) -> Rgb {
        match self.at {
            Position::Top => terminal_top(theme),
            Position::Bottom => theme.background,
            Position::Left | Position::Right => {
                if theme.style.background != crate::style::Background::Gradient {
                    return theme.background;
                }
                let [_, zy, _, zh] = self.zone;
                let k = (1.0 - (a - zy) / (0.7 * zh).max(1.0)).clamp(0.0, 1.0);
                theme.background.lerp(theme.accent, GRADIENT * k)
            }
        }
    }

    /// Fondo de una pestaña según la forma del tema. Devuelve el tramo de la línea entre las
    /// pestañas y el terminal que no hay que dibujar (la pestaña activa que se une al terminal).
    #[allow(clippy::too_many_arguments)]
    fn draw_tab_shape(
        &self,
        p: &mut Painter,
        theme: &Theme,
        i: usize,
        active: usize,
        hovered: bool,
        s: f32,
    ) -> Option<(f32, f32)> {
        let (a0, la) = self.tabs[i];
        let is_active = i == active;
        let st = theme.style;
        let rr = st.r(s);
        let hair = s.round().max(1.0);
        let line = (2.0 * s).round().max(2.0);
        let t = self.thick;
        let [bx, y, bw, h] = self.pill_rect(self.tabs[i]);
        let fg = theme.foreground;
        let rect = |p: &mut Painter, a: f32, c: f32, la: f32, lc: f32, col: Rgb, alpha: f32| {
            let [x, y, w, h] = self.place(a, c, la, lc);
            p.rect(x, y, w, h, col, alpha);
        };
        // Separador fino entre dos pestañas sin seleccionar.
        let separator = |p: &mut Painter| {
            if i + 1 < self.tabs.len() && !is_active && i + 1 != active {
                rect(p, a0 + la - hair, 10.0 * s, hair, t - 20.0 * s, fg, 0.10);
            }
        };
        match st.tabs {
            Tabs::Flat => {
                let pill = surface(theme, 0.08);
                if is_active {
                    rect(p, a0, 0.0, la, t - hair, pill, 1.0);
                } else if hovered {
                    rect(p, a0, 0.0, la, t - hair, pill, 0.5);
                }
                separator(p);
            }
            Tabs::Underline => {
                if hovered && !is_active {
                    p.rounded(bx, y, bw, h, fg, 0.05, rr);
                }
                if is_active {
                    let m = if self.horizontal() {
                        (12.0 * s).round()
                    } else {
                        (la * 0.25).round()
                    };
                    let [x, y, w, h] = self.place(a0 + m, t - hair - line, la - 2.0 * m, line);
                    p.rounded(x, y, w, h, theme.accent, 1.0, line / 2.0);
                }
            }
            Tabs::TopLine => {
                if is_active {
                    rect(p, a0, 0.0, la, t, self.joined(theme, a0 + la / 2.0), 1.0);
                    rect(p, a0, 0.0, la, line, theme.accent, 1.0);
                    return Some((a0, a0 + la));
                }
                if hovered {
                    rect(p, a0, 0.0, la, t - hair, fg, 0.04);
                }
                if i + 1 != active {
                    rect(p, a0 + la - hair, 0.0, hair, t - hair, fg, 0.08);
                }
            }
            Tabs::Pill | Tabs::Segmented => {
                if is_active {
                    p.rounded(bx, y, bw, h, theme.tab_active, 1.0, rr);
                    p.outline(bx, y, bw, h, fg, 0.10, rr, hair);
                } else if hovered {
                    p.rounded(bx, y, bw, h, fg, 0.05, rr);
                }
            }
            Tabs::Outline => {
                if is_active {
                    let g = (2.0 * s).round();
                    p.outline(
                        bx - g,
                        y - g,
                        bw + 2.0 * g,
                        h + 2.0 * g,
                        theme.accent,
                        0.16,
                        rr + g,
                        g,
                    );
                    p.rounded(
                        bx,
                        y,
                        bw,
                        h,
                        theme.tab_bar.lerp(theme.accent, 0.10),
                        1.0,
                        rr,
                    );
                    p.outline(bx, y, bw, h, theme.accent, 1.0, rr, hair.max(1.2 * s));
                } else if hovered {
                    p.outline(bx, y, bw, h, fg, 0.22, rr, hair);
                }
            }
            Tabs::Floating => {
                if is_active {
                    // La sombra, hacia el terminal.
                    let [sx, sy, sw, sh] = self.place(
                        a0 + self.pill_inset,
                        self.pill.0 + s,
                        la - 2.0 * self.pill_inset,
                        self.pill.1 + s,
                    );
                    p.rounded(sx, sy, sw, sh, Rgb(0, 0, 0), 0.28, rr);
                    p.rounded(bx, y, bw, h, theme.tab_active, 1.0, rr);
                    p.outline(bx, y, bw, h, fg, 0.12, rr, hair);
                } else {
                    let k = if hovered { 0.11 } else { 0.06 };
                    p.rounded(bx, y, bw, h, theme.tab_bar.lerp(fg, k), 1.0, rr);
                }
            }
            Tabs::Folder => {
                let top = (6.0 * s).round();
                if is_active {
                    // Llega hasta el terminal (del mismo color) y los rincones de ese lado se
                    // curvan hacia fuera, como la solapa de una carpeta.
                    let joined = self.joined(theme, a0 + la / 2.0);
                    let [x, y, w, h] = self.place(a0, top, la, t - top + rr);
                    p.rounded(x, y, w, h, joined, 1.0, rr);
                    if rr > 0.0 {
                        let cb = t - rr;
                        rect(p, a0 - rr, cb, rr, rr, joined, 1.0);
                        let (cx, cy) = self.point(a0 - rr, cb);
                        p.circle(cx, cy, rr, theme.tab_bar, 1.0);
                        rect(p, a0 + la, cb, rr, rr, joined, 1.0);
                        let (cx, cy) = self.point(a0 + la + rr, cb);
                        p.circle(cx, cy, rr, theme.tab_bar, 1.0);
                    }
                    return Some((a0 - rr, a0 + la + rr));
                }
                if hovered {
                    let [x, y, w, h] = self.place(a0, top, la, t - top - hair);
                    p.rounded(x, y, w, h, fg, 0.05, rr);
                }
                separator(p);
            }
            Tabs::Brackets => {}
            Tabs::Dot => {
                if hovered && !is_active {
                    p.rounded(bx, y, bw, h, fg, 0.05, rr);
                }
                if is_active {
                    let [x, y, _, h] = self.cell(self.tabs[i]);
                    let x0 = if self.horizontal() {
                        x + (12.0 * s).round()
                    } else {
                        bx + (10.0 * s).round()
                    };
                    p.circle(
                        x0 + 3.5 * s,
                        (y + h / 2.0).round(),
                        3.5 * s,
                        theme.accent,
                        1.0,
                    );
                }
            }
        }
        None
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
        anim: crate::tabs::Anim,
        extra: &mut TabsExtra,
    ) {
        let s = scale;
        let st = theme.style;
        let r = (st.r(s) + s).round();
        let hair = s.round().max(1.0);
        let dimc = dim(theme);
        let pill = surface(theme, 0.08);
        p.rect(0.0, 0.0, width, self.height, theme.tab_bar, 1.0);
        if self.at != Position::Top {
            // Franja o columna: el mismo fondo que la barra. La barra de arriba lleva su línea y,
            // en medio, el nombre de la pestaña activa como título.
            if !self.hide_tabs {
                let [x, y, w, h] = self.zone;
                p.rect(x, y, w, h, theme.tab_bar, 1.0);
            }
            p.rect(0.0, self.height - hair, width, hair, theme.foreground, 0.08);
            if let Some(t) = tabs.get(active) {
                let margin = (12.0 * s).round();
                let limit = self.update.map_or(self.stats_btn.0, |u| u.0) - margin;
                let tw = p.ui_measure(&t.label);
                let x = ((width - tw) / 2.0).round().max(margin);
                let x = if x + tw > limit { margin } else { x };
                p.ui_text_in(x, 0.0, self.height, &t.label, dimc, (limit - x).max(0.0));
            }
        }
        if !self.hide_tabs {
            self.draw_tabs(p, theme, tabs, active, hover, s, anim, extra);
        }

        let cy = (self.tab_y + self.tab_h / 2.0).round();

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
        // Estadísticas: tres barras de distinta altura sobre una base.
        let c = button(p, self.stats_btn, TabHit::Stats, toggles.stats);
        let sx = (self.stats_btn.0 + self.stats_btn.1 / 2.0).round();
        let floor = cy + 6.0 * s;
        for (dx, bh) in [(-4.5, 6.0), (0.0, 11.0), (4.5, 8.0)] {
            let bx = (sx + dx * s - 1.5 * s).round();
            p.rounded(bx, floor - bh * s, 3.0 * s, bh * s, c, 1.0, 1.0 * s);
        }

        // Cambios de git: una hoja con + y −.
        if crate::git::ENABLED {
            let c = button(p, self.changes_btn, TabHit::Changes, toggles.changes);
            let gx = (self.changes_btn.0 + self.changes_btn.1 / 2.0).round();
            let (iw, ih) = ((12.0 * s).round(), (15.0 * s).round());
            changes_icon(
                p,
                gx - (iw / 2.0).round(),
                (cy - ih / 2.0).round(),
                iw,
                ih,
                (1.4 * s).max(1.0),
                c,
                true,
            );
        }

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
            if flat_controls(&st) {
                self.draw_native_controls(p, theme, ctl, hover, s, toggles.maximized);
                return;
            }
            let kinds = [TabHit::Minimize, TabHit::Maximize, TabHit::CloseWindow];
            let over_any = kinds.iter().any(|k| hover == Some(*k));
            for (i, &(bx, bw)) in ctl.iter().enumerate() {
                let over = hover == Some(kinds[i]);
                let cx = (bx + bw / 2.0).round();
                let cy = (self.height / 2.0).round();
                let t = 1.4 * s;
                let (c, k) = match st.buttons {
                    // Círculos con los colores del tema; los iconos, al pasar por encima.
                    Buttons::Color => {
                        let fill = [theme.palette[3], theme.palette[2], theme.palette[1]][i];
                        p.circle(cx, cy, 6.5 * s, fill, if over { 1.0 } else { 0.92 });
                        if !over_any {
                            continue;
                        }
                        (Rgb(0, 0, 0), 2.6 * s)
                    }
                    Buttons::Icons => {
                        if over {
                            let close = kinds[i] == TabHit::CloseWindow;
                            let (bgc, a) = if close {
                                (theme.palette[1], 0.22)
                            } else {
                                (theme.foreground, 0.10)
                            };
                            p.rounded(bx, self.tab_y, bw, self.tab_h, bgc, a, r);
                        }
                        let c = if over { theme.foreground } else { dimc };
                        (c, 3.8 * s)
                    }
                    // Botones de ventana estilo GNOME: círculos con iconos finos.
                    _ => {
                        p.circle(
                            cx,
                            cy,
                            12.0 * s,
                            theme.foreground,
                            if over { 0.18 } else { 0.08 },
                        );
                        (theme.foreground, 3.6 * s)
                    }
                };
                let t = if st.buttons == Buttons::Color {
                    1.2 * s
                } else {
                    t
                };
                match kinds[i] {
                    TabHit::Minimize => p.line(
                        cx - k - 0.4 * s,
                        cy + 0.5 * s,
                        cx + k + 0.4 * s,
                        cy + 0.5 * s,
                        t,
                        c,
                        1.0,
                    ),
                    TabHit::Maximize => {
                        p.line(cx - k, cy - k, cx + k, cy - k, t, c, 1.0);
                        p.line(cx + k, cy - k, cx + k, cy + k, t, c, 1.0);
                        p.line(cx + k, cy + k, cx - k, cy + k, t, c, 1.0);
                        p.line(cx - k, cy + k, cx - k, cy - k, t, c, 1.0);
                    }
                    _ => cross(p, cx, cy, k, t, c),
                }
            }
        }
    }

    /// Las pestañas, la línea que las separa del terminal y el botón de nueva pestaña.
    #[allow(clippy::too_many_arguments)]
    fn draw_tabs(
        &self,
        p: &mut Painter,
        theme: &Theme,
        tabs: &[TabView],
        active: usize,
        hover: Option<TabHit>,
        s: f32,
        anim: crate::tabs::Anim,
        extra: &mut TabsExtra,
    ) {
        use crate::sessions::Status;
        let st = theme.style;
        let r = (st.r(s) + s).round();
        let hair = s.round().max(1.0);
        let dimc = dim(theme);
        let pill = surface(theme, 0.08);
        let horizontal = self.horizontal();

        // Segmentos: un carril bajo todas las pestañas.
        if st.tabs == Tabs::Segmented {
            if let (Some(&(a0, _)), Some(&(la_, ll))) = (self.tabs.first(), self.tabs.last()) {
                let pad = (3.0 * s).round();
                let [x, y, w, h] = self.place(
                    a0,
                    self.pill.0 - pad,
                    la_ + ll - a0,
                    self.pill.1 + 2.0 * pad,
                );
                p.rounded(x, y, w, h, theme.foreground, 0.06, st.r(s) + pad);
            }
        }

        let hovered_tab = match hover {
            Some(TabHit::Tab(j) | TabHit::Close(j)) => Some(j),
            _ => None,
        };
        let mut gap = None;
        for i in 0..self.tabs.len() {
            let t = self.tabs[i];
            let [tx, ty, tw, th] = self.cell(t);
            let [px, _, pw, _] = self.pill_rect(t);
            let is_active = i == active;
            let hovered = hovered_tab == Some(i);
            if let Some(g) = self.draw_tab_shape(p, theme, i, active, hovered, s) {
                gap = Some(g);
            }
            if let Some(rn) = extra.rename.filter(|rn| rn.group == i) {
                self.draw_rename(p, theme, t, rn, s);
                continue;
            }
            let Some(tab) = tabs.get(i) else { continue };
            let color = match (is_active, st.tabs) {
                (true, Tabs::Brackets) => theme.accent,
                (true, _) => theme.foreground,
                (false, _) if hovered && st.tabs != Tabs::Flat => theme.foreground,
                _ => dimc,
            };
            // Lo que ocupa la pestaña a lo ancho del texto: en una barra, toda la celda; en una
            // columna, su fondo.
            let (start, end) = if horizontal {
                (tx, tx + tw)
            } else {
                (px, px + pw)
            };
            let mut pad = (if horizontal { 12.0 } else { 10.0 } * s).round();
            if st.tabs == Tabs::Dot {
                pad += (13.0 * s).round();
            }
            let show_close = is_active || hovered;
            let mut right = if horizontal {
                end - pad / 2.0
            } else {
                end - (8.0 * s).round()
            };
            let cy = (ty + th / 2.0).round();
            if show_close {
                let cx = (end - self.close_w / 2.0).round();
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
                right = end - self.close_w;
            }
            // Estado del agente: animación mientras trabaja, punto amarillo si te espera y verde si
            // terminó sin mirarlo.
            let mut status_w = 0.0;
            if tab.agent.is_some() {
                match tab.status {
                    Status::Working => {
                        right -= 16.0 * s;
                        crate::panel::spinner(p, right, cy, s * 0.9, theme.accent, anim);
                        right -= 8.0 * s;
                        status_w = 24.0 * s;
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
                        status_w = 16.0 * s;
                    }
                    _ => {}
                }
            }
            let mut label = if tab.exited {
                format!("{} (terminado)", tab.label)
            } else {
                tab.label.clone()
            };
            if st.tabs == Tabs::Brackets && is_active {
                label = format!("[ {label} ]");
            }
            // Centrado: el mismo margen a los dos lados, guardando sitio para la × y el estado
            // aunque no se vean (así el nombre no salta al pasar el ratón).
            let (x0, x1, centered) = match self.align {
                Align::Center => {
                    let side = pad.max(self.close_w + status_w);
                    if end - start - 2.0 * side > 24.0 * s {
                        (start + side, end - side, true)
                    } else {
                        (start + pad, right, false)
                    }
                }
                Align::Left => (start + pad, right, false),
            };
            self.draw_label(p, i, &label, color, (x0, x1), (ty, th), centered, s, extra);
        }
        // Línea entre las pestañas y el terminal, salvo junto a la pestaña que se une a él.
        let [zx, zy, zw, zh] = self.zone;
        let (za, zl) = if horizontal { (zx, zw) } else { (zy, zh) };
        let c = self.thick - hair;
        let seg = |p: &mut Painter, a: f32, la: f32| {
            let [x, y, w, h] = self.place(a, c, la.max(0.0), hair);
            p.rect(x, y, w, h, theme.foreground, 0.08);
        };
        match gap {
            Some((a, b)) => {
                seg(p, za, a - za);
                seg(p, b, za + zl - b);
            }
            None => seg(p, za, zl),
        }

        // Nueva pestaña
        let [px, py, pw, ph] = self.plus_rect();
        let over = hover == Some(TabHit::NewTab);
        if over {
            p.rounded(px, py, pw, ph, pill, 0.8, r);
        }
        let c = if over { theme.foreground } else { dimc };
        let k = 5.0 * s;
        let cy = (py + ph / 2.0).round();
        let cx = if horizontal {
            (px + pw / 2.0).round()
        } else {
            (px + (10.0 * s) + k).round()
        };
        p.line(cx - k, cy, cx + k, cy, 1.5 * s, c, 1.0);
        p.line(cx, cy - k, cx, cy + k, 1.5 * s, c, 1.0);
        if !horizontal {
            let x = cx + k + (10.0 * s).round();
            p.ui_text_in(x, py, ph, "Nueva pestaña", c, (px + pw - x).max(0.0));
        }
    }

    /// Nombre de una pestaña en [`x0`, `x1`]. Si no cabe y el ratón está encima, se desplaza
    /// (`Marquee`); si no, se corta con "…".
    #[allow(clippy::too_many_arguments)]
    fn draw_label(
        &self,
        p: &mut Painter,
        i: usize,
        label: &str,
        color: Rgb,
        (x0, x1): (f32, f32),
        (y, h): (f32, f32),
        centered: bool,
        s: f32,
        extra: &mut TabsExtra,
    ) {
        let avail = (x1 - x0).max(0.0);
        let tw = p.ui_measure(label);
        let over = tw - avail;
        let target = extra.marquee.target() == Some(i);
        if target && over > 0.5 {
            let off = extra.marquee.step(over, s, extra.now);
            p.ui_text_clipped(
                x0 - off,
                y,
                h,
                label,
                color,
                (x0, x1),
                (off > 0.5, off < over - 0.5),
            );
            return;
        }
        if target {
            extra.marquee.step(0.0, s, extra.now);
        }
        let x = if centered && over <= 0.0 {
            (x0 + (avail - tw) / 2.0).round()
        } else {
            x0
        };
        p.ui_text_in(x, y, h, label, color, (x1 - x).max(0.0));
    }

    /// Campo para cambiar el nombre, sobre el fondo de la pestaña: el texto (todo seleccionado al
    /// empezar) y el cursor.
    fn draw_rename(&self, p: &mut Painter, theme: &Theme, t: (f32, f32), rn: &Rename, s: f32) {
        let [bx, by, bw, bh] = self.pill_rect(t);
        let [_, ty, _, th] = self.cell(t);
        let r = (theme.style.r(s)).max(3.0 * s);
        let hair = s.round().max(1.0);
        p.rounded(bx, by, bw, bh, theme.background, 1.0, r);
        p.outline(bx, by, bw, bh, theme.accent, 1.0, r, (1.5 * s).max(hair));
        let pad = (8.0 * s).round();
        let (x0, x1) = (bx + pad, bx + bw - pad);
        let avail = (x1 - x0).max(0.0);
        let caret = p.ui_measure(rn.before_cursor());
        let tw = p.ui_measure(&rn.text);
        // Que el cursor quede siempre a la vista.
        let off = (caret - avail + 2.0 * s).max(0.0);
        let lh = p.ui_metrics().height;
        let line_y = (ty + (th - lh) / 2.0).round();
        if rn.all && !rn.text.is_empty() {
            let w = tw.min(avail + off) - off;
            p.rect(x0, line_y, w.max(0.0), lh, theme.selection, 1.0);
        }
        p.ui_text_clipped(
            x0 - off,
            ty,
            th,
            &rn.text,
            theme.foreground,
            (x0, x1),
            (false, false),
        );
        if !rn.all || rn.text.is_empty() {
            let cx = (x0 - off + caret).round();
            p.rect(
                cx,
                line_y,
                (1.5 * s).round().max(1.0),
                lh,
                theme.cursor,
                1.0,
            );
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

/// Cambios de git: una hoja con la esquina doblada en la caja (x, y, w, h), trazo `t`; con
/// `signs`, un "+" y un "−" dentro (el botón de la barra; en el prompt no se distinguirían).
#[allow(clippy::too_many_arguments)]
pub fn changes_icon(p: &mut Painter, x: f32, y: f32, w: f32, h: f32, t: f32, c: Rgb, signs: bool) {
    let (x1, y1) = (x + w, y + h);
    let f = (w * 0.36).round().max(t);
    p.line(x, y, x1 - f, y, t, c, 1.0);
    p.line(x1 - f, y, x1, y + f, t, c, 1.0);
    p.line(x1, y + f, x1, y1, t, c, 1.0);
    p.line(x1, y1, x, y1, t, c, 1.0);
    p.line(x, y1, x, y, t, c, 1.0);
    if signs {
        let cx = x + w / 2.0;
        let k = (w * 0.2).max(t);
        let (ya, yb) = (y + h * 0.42, y + h * 0.74);
        p.line(cx - k, ya, cx + k, ya, t, c, 1.0);
        p.line(cx, ya - k, cx, ya + k, t, c, 1.0);
        p.line(cx - k, yb, cx + k, yb, t, c, 1.0);
    }
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
