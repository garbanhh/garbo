//! Panel lateral: agentes en ejecución en las pestañas y puertos en escucha.
//!
//! Cada agente ocupa una fila con su nombre, el directorio, el estado y sus subagentes.
//! Al pasar el ratón por encima la fila se despliega (con una animación corta) y muestra las
//! últimas líneas de esa pestaña, sin tener que cambiar a ella.

use crate::atlas::UiVariant;
use crate::config::{Rgb, Theme};
use crate::ports::PortInfo;
use crate::sessions::{self, Agent, Status};
use crate::subagents::SubAgent;
use crate::tabs::{Anim, Working};
use crate::ui::{cross, dim};
use crate::view::Painter;
use std::time::{Duration, Instant, SystemTime};

/// Duración de la animación al desplegar o recoger una fila.
const ANIM: Duration = Duration::from_millis(170);
/// Líneas de la vista previa de una pestaña.
pub const PREVIEW_LINES: usize = 8;

#[derive(Default)]
pub struct PanelState {
    pub agents: bool,
    pub ports: bool,
    pub more_ports: bool,
    pub hover: Option<Hit>,
    pub ports_data: Vec<PortInfo>,
    pub ports_loaded: bool,
    /// Fila de agente que se está desplegando (bajo el ratón) y la que se está recogiendo.
    expand: Option<usize>,
    collapse: Option<usize>,
    anim: Option<Instant>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    CloseAgents,
    ClosePorts,
    Agent(usize),
    Port(u16),
    MorePorts,
    Panel,
}

/// Una línea de la vista previa: tramos de texto con su color.
pub type PreviewLine = Vec<(String, Rgb)>;

/// Una fila de la sección de agentes (una pestaña con agente).
pub struct AgentRow<'a> {
    pub label: &'a str,
    pub agent: Agent,
    pub status: Status,
    /// Lo que el agente dice que hace o espera (OSC 7501).
    pub detail: Option<&'a str>,
    pub since: Instant,
    pub active: bool,
    pub subagents: &'a [SubAgent],
    /// Últimas líneas de la pestaña (solo para las filas desplegadas).
    pub preview: Option<Vec<PreviewLine>>,
}

/// Altos de línea de las variantes de texto que usa el panel.
#[derive(Clone, Copy)]
pub struct Metrics {
    pub lh: f32,
    pub th: f32,
    pub plh: f32,
}

impl Metrics {
    pub fn from_ui(line_h: f32) -> Self {
        Self {
            lh: line_h,
            th: (line_h * UiVariant::Title.scale()).ceil(),
            plh: (line_h * UiVariant::Small.scale()).ceil(),
        }
    }
}

struct Section {
    y: f32,
    close: (f32, f32, f32),
}

struct Layout {
    agents: Option<Section>,
    ports: Option<Section>,
    /// (zona, y, alto) de las filas visibles; para los agentes, el alto incluye el despliegue.
    rows: Vec<(Hit, f32, f32)>,
    header_h: f32,
}

fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

impl PanelState {
    pub fn visible(&self) -> bool {
        self.agents || self.ports
    }

    pub fn width(scale: f32, win_w: f32) -> f32 {
        (340.0 * scale).min(win_w * 0.45).round()
    }

    /// Cambia la zona bajo el ratón. Devuelve `true` si hay que redibujar.
    pub fn set_hover(&mut self, hit: Option<Hit>, now: Instant) -> bool {
        if hit == self.hover {
            return false;
        }
        let target = match hit {
            Some(Hit::Agent(i)) => Some(i),
            _ => None,
        };
        if target != self.expand {
            self.collapse = self.expand;
            self.expand = target;
            self.anim = Some(now);
        }
        self.hover = hit;
        true
    }

    pub fn animating(&self, now: Instant) -> bool {
        self.anim.is_some_and(|t| now.duration_since(t) < ANIM)
    }

    /// Cuánto está desplegada la fila `i` (0 = cerrada, 1 = abierta).
    fn openness(&self, i: usize, now: Instant) -> f32 {
        let t = self.anim.map_or(1.0, |t| {
            now.duration_since(t).as_secs_f32() / ANIM.as_secs_f32()
        });
        let e = ease(t);
        if self.expand == Some(i) {
            e
        } else if self.collapse == Some(i) {
            1.0 - e
        } else {
            0.0
        }
    }

    /// Filas que necesitan su vista previa en este fotograma.
    pub fn previewed(&self, now: Instant) -> impl Iterator<Item = usize> + '_ {
        [self.expand, self.collapse]
            .into_iter()
            .flatten()
            .filter(move |&i| self.openness(i, now) > 0.0)
    }

    fn split(&self) -> (Vec<&PortInfo>, Vec<&PortInfo>) {
        self.ports_data.iter().partition(|p| p.web)
    }

    /// Alto de una fila de agente con `sub_lines` líneas de subagentes y desplegada `open`.
    fn agent_h(sub_lines: usize, open: f32, m: Metrics, s: f32) -> (f32, f32) {
        let base = 10.0 * s + m.th + 2.0 * s + 2.0 * m.lh + 10.0 * s;
        let subs = if sub_lines > 0 {
            4.0 * s + sub_lines as f32 * (m.lh + 2.0 * s)
        } else {
            0.0
        };
        let preview = open * (PREVIEW_LINES as f32 * m.plh + 16.0 * s + 8.0 * s);
        ((base + subs).round(), preview)
    }

    #[allow(clippy::too_many_arguments)]
    fn layout(
        &self,
        subs: &[usize],
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        s: f32,
        m: Metrics,
        now: Instant,
    ) -> Layout {
        let header_h = (44.0 * s).round();
        let lh = m.lh;
        let port_h = (2.0 * lh + 16.0 * s).round();
        let line_h = (lh + 14.0 * s).round();
        let mut rows = Vec::new();

        let heights: Vec<(f32, f32)> = subs
            .iter()
            .enumerate()
            .map(|(i, &n)| Self::agent_h(n, self.openness(i, now), m, s))
            .collect();
        let agents_content = header_h
            + heights
                .iter()
                .map(|(b, p)| b + p)
                .sum::<f32>()
                .max(3.0 * lh)
            + 8.0 * s;
        let first_h = heights.first().map_or(3.0 * lh, |(b, _)| *b);
        let (agents_h, ports_h) = match (self.agents, self.ports) {
            (true, true) => {
                let a = agents_content.min(h * 0.55).max(header_h + first_h);
                (a, h - a)
            }
            (true, false) => (h, 0.0),
            (false, true) => (0.0, h),
            _ => (0.0, 0.0),
        };
        let close = |sy: f32| (x + w - 22.0 * s, sy + header_h / 2.0, 12.0 * s);

        let agents = self.agents.then(|| {
            let bottom = y + agents_h;
            let mut ry = y + header_h;
            for (i, &(base, preview)) in heights.iter().enumerate() {
                if ry + base > bottom {
                    break;
                }
                // El despliegue se recorta si no cabe: la fila nunca desaparece por desplegarse.
                let rh = (base + preview).min(bottom - ry).round();
                rows.push((Hit::Agent(i), ry, rh));
                ry += rh;
            }
            Section { y, close: close(y) }
        });

        let ports = self.ports.then(|| {
            let sy = y + agents_h;
            let (web, other) = self.split();
            let mut ry = sy + header_h;
            let bottom = sy + ports_h;
            for p in &web {
                if ry + port_h > bottom {
                    break;
                }
                rows.push((Hit::Port(p.port), ry, port_h));
                ry += port_h;
            }
            if web.is_empty() {
                ry += port_h; // mensaje "No hay servidores web"
            }
            if !other.is_empty() && ry + line_h <= bottom {
                rows.push((Hit::MorePorts, ry, line_h));
                ry += line_h;
                if self.more_ports {
                    for p in &other {
                        if ry + line_h > bottom {
                            break;
                        }
                        rows.push((Hit::Port(p.port), ry, line_h));
                        ry += line_h;
                    }
                }
            }
            Section {
                y: sy,
                close: close(sy),
            }
        });
        Layout {
            agents,
            ports,
            rows,
            header_h,
        }
    }

    /// `subs`: líneas de subagentes de cada fila de agente.
    #[allow(clippy::too_many_arguments)]
    pub fn hit(
        &self,
        mx: f32,
        my: f32,
        subs: &[usize],
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        s: f32,
        m: Metrics,
        now: Instant,
    ) -> Option<Hit> {
        if mx < x || mx > x + w || my < y || my > y + h {
            return None;
        }
        let l = self.layout(subs, x, y, w, h, s, m, now);
        for (sec, hit) in [(&l.agents, Hit::CloseAgents), (&l.ports, Hit::ClosePorts)] {
            if let Some(sec) = sec {
                let (cx, cy, r) = sec.close;
                if (mx - cx).hypot(my - cy) <= r {
                    return Some(hit);
                }
            }
        }
        for &(hit, ry, rh) in &l.rows {
            if my >= ry && my < ry + rh {
                return Some(hit);
            }
        }
        Some(Hit::Panel)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        p: &mut Painter,
        theme: &Theme,
        agents: &[AgentRow],
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        s: f32,
        now: Instant,
        anim: Anim,
        chrome: &crate::chrome::Chrome,
    ) {
        let m = Metrics::from_ui(p.ui_metrics().height);
        let lh = m.lh;
        let subs: Vec<usize> = agents.iter().map(|a| sub_lines(a.subagents)).collect();
        let l = self.layout(&subs, x, y, w, h, s, m, now);
        let hair = s.round().max(1.0);
        let dimc = dim(theme);
        let pad = (16.0 * s).round();
        if chrome.floating {
            p.card(
                x,
                y,
                w,
                h,
                theme.background,
                theme.foreground,
                chrome.radius,
                s,
            );
        } else {
            p.rect(x, y, w, h, theme.tab_bar, 1.0);
            p.rect(x, y, hair, h, theme.foreground, 0.10);
        }

        let header = |p: &mut Painter,
                      sec: &Section,
                      title: &str,
                      extra: &str,
                      robot: bool,
                      hovered: bool| {
            let cy = sec.y + l.header_h / 2.0;
            // Flotante: el título y, a la derecha, el resumen y un botón redondo de cerrar.
            if chrome.floating {
                let th = p.ui_line_h(UiVariant::Title);
                let ty = (cy - th / 2.0).round();
                p.ui_text_v(
                    x + pad,
                    ty,
                    title,
                    theme.foreground,
                    w * 0.5,
                    UiVariant::Title,
                );
                let (cx, ccy, r) = sec.close;
                let ew = p.ui_measure_v(extra, UiVariant::Small);
                let sh = p.ui_line_h(UiVariant::Small);
                let ex = cx - r - 10.0 * s - ew;
                p.ui_text_v(
                    ex,
                    (cy - sh / 2.0).round(),
                    extra,
                    dimc,
                    ew + 2.0,
                    UiVariant::Small,
                );
                p.circle(
                    cx,
                    ccy,
                    r,
                    theme.foreground,
                    if hovered { 0.12 } else { 0.05 },
                );
                cross(p, cx, ccy, 3.5 * s, 1.4 * s, dimc);
                return;
            }
            let ix = x + pad + 7.0 * s;
            if robot {
                let under = if chrome.floating {
                    theme.background
                } else {
                    theme.tab_bar
                };
                robot_icon(p, ix, cy, s, theme.foreground, under);
            } else {
                globe_icon(p, ix, cy, s, theme.foreground);
            }
            let tx = x + pad + 22.0 * s;
            let tw = p.ui_text_in(tx, sec.y, l.header_h, title, theme.foreground, w * 0.5);
            p.ui_text_in(tx + tw + 10.0 * s, sec.y, l.header_h, extra, dimc, w * 0.4);
            let (cx, cy, r) = sec.close;
            if hovered {
                p.circle(cx, cy, r, theme.foreground, 0.12);
            }
            cross(p, cx, cy, 3.5 * s, 1.4 * s, dimc);
            p.rect(
                x,
                sec.y + l.header_h - hair,
                w,
                hair,
                theme.foreground,
                0.08,
            );
        };

        // --- Agentes
        if let Some(sec) = &l.agents {
            let working = agents
                .iter()
                .filter(|a| a.status == Status::Working)
                .count();
            let extra = match (chrome.floating, working) {
                (true, _) if !agents.is_empty() => match agents.len() {
                    1 => "1 activo".to_string(),
                    n => format!("{n} activos"),
                },
                (false, w) if w > 0 => format!("{w} trabajando"),
                _ => String::new(),
            };
            header(
                p,
                sec,
                "Agentes",
                &extra,
                true,
                self.hover == Some(Hit::CloseAgents),
            );
            if agents.is_empty() {
                p.ui_text_in(
                    x + pad,
                    sec.y + l.header_h,
                    lh * 2.5,
                    "Ningún agente en ejecución",
                    dimc,
                    w - 2.0 * pad,
                );
            }
            for &(hit, ry, rh) in &l.rows {
                let Hit::Agent(i) = hit else { continue };
                let hovered = self.hover == Some(hit);
                self.draw_agent(
                    p,
                    theme,
                    &agents[i],
                    i,
                    x,
                    ry,
                    w,
                    rh,
                    s,
                    m,
                    now,
                    anim,
                    hovered,
                    chrome.floating,
                );
            }
        }

        // --- Puertos
        if let Some(sec) = &l.ports {
            if l.agents.is_some() {
                p.rect(x, sec.y, w, hair, theme.foreground, 0.10);
            }
            let (web, other) = self.split();
            let extra = match (chrome.floating, web.len()) {
                (true, _) => format!("{} en escucha", self.ports_data.len()),
                (false, 1) => "1 servidor web".to_string(),
                (false, n) => format!("{n} servidores web"),
            };
            header(
                p,
                sec,
                "Puertos",
                if self.ports_loaded {
                    &extra
                } else {
                    "buscando…"
                },
                false,
                self.hover == Some(Hit::ClosePorts),
            );
            if self.ports_loaded && web.is_empty() {
                let t = "No hay servidores web";
                let tw = p.ui_measure(t);
                p.ui_text_in(
                    x + (w - tw) / 2.0,
                    sec.y + l.header_h,
                    2.0 * lh + 16.0 * s,
                    t,
                    dimc,
                    w,
                );
            }
            for &(hit, ry, rh) in &l.rows {
                match hit {
                    Hit::Port(port) => {
                        let Some(info) = self.ports_data.iter().find(|p| p.port == port) else {
                            continue;
                        };
                        let over = self.hover == Some(hit);
                        if chrome.floating && info.web {
                            // Flotante: cada servidor en su tarjeta (puerto, programa y dirección).
                            let a = if over { 0.07 } else { 0.04 };
                            let (cx0, cw) = (x + 8.0 * s, w - 16.0 * s);
                            p.rounded(
                                cx0,
                                ry + 3.0 * s,
                                cw,
                                rh - 6.0 * s,
                                theme.foreground,
                                a,
                                (10.0 * s).round(),
                            );
                        } else if over {
                            p.rounded(
                                x + 6.0 * s,
                                ry + 2.0 * s,
                                w - 12.0 * s,
                                rh - 4.0 * s,
                                theme.foreground,
                                0.06,
                                (8.0 * s).round(),
                            );
                        }
                        let proc_name = info.process.clone().unwrap_or_else(|| "sistema".into());
                        if chrome.floating && info.web {
                            let tx = x + pad + 2.0 * s;
                            let t = port.to_string();
                            p.ui_text_in(tx, ry, rh, &t, theme.foreground, w * 0.3);
                            let nx = tx + (64.0 * s).round();
                            let sh = p.ui_line_h(UiVariant::Small);
                            let ny = (ry + (rh - lh - sh) / 2.0).round();
                            p.ui_text(
                                nx,
                                ny,
                                &proc_name,
                                theme.foreground,
                                x + w - pad - 24.0 * s - nx,
                            );
                            p.ui_text_v(nx, ny + lh, "localhost", dimc, w * 0.4, UiVariant::Small);
                        }
                        if info.web && !chrome.floating {
                            p.circle(
                                x + pad + 4.0 * s,
                                ry + 8.0 * s + lh / 2.0,
                                3.5 * s,
                                theme.palette[2],
                                1.0,
                            );
                            let tx = x + pad + 16.0 * s;
                            p.ui_text(
                                tx,
                                ry + 8.0 * s,
                                &format!("localhost:{port}"),
                                theme.foreground,
                                w - (tx - x) - pad - 20.0 * s,
                            );
                            p.ui_text(tx, ry + 8.0 * s + lh, &proc_name, dimc, w - (tx - x) - pad);
                            // Flecha "abrir en el navegador"
                            let ax = x + w - pad - 6.0 * s;
                            let ay = ry + 8.0 * s + lh / 2.0;
                            let c = if self.hover == Some(hit) {
                                theme.foreground
                            } else {
                                dimc
                            };
                            p.line(
                                ax - 5.0 * s,
                                ay + 5.0 * s,
                                ax + 4.0 * s,
                                ay - 4.0 * s,
                                1.4 * s,
                                c,
                                1.0,
                            );
                            p.line(
                                ax - 2.0 * s,
                                ay - 4.0 * s,
                                ax + 4.0 * s,
                                ay - 4.0 * s,
                                1.4 * s,
                                c,
                                1.0,
                            );
                            p.line(
                                ax + 4.0 * s,
                                ay - 4.0 * s,
                                ax + 4.0 * s,
                                ay + 2.0 * s,
                                1.4 * s,
                                c,
                                1.0,
                            );
                        } else if !info.web {
                            let tx = x + pad + 16.0 * s;
                            let t = format!(":{port}");
                            let tw = p.ui_text_in(tx, ry, rh, &t, dimc, w * 0.3);
                            p.ui_text_in(
                                tx + tw + 12.0 * s,
                                ry,
                                rh,
                                &proc_name,
                                dimc,
                                w - (tx - x) - tw - pad,
                            );
                        }
                    }
                    Hit::MorePorts => {
                        let cx = x + pad + 4.0 * s;
                        let cy = ry + rh / 2.0;
                        let c = if self.hover == Some(hit) {
                            theme.foreground
                        } else {
                            dimc
                        };
                        if self.more_ports {
                            p.line(
                                cx - 4.0 * s,
                                cy - 2.0 * s,
                                cx,
                                cy + 2.0 * s,
                                1.5 * s,
                                c,
                                1.0,
                            );
                            p.line(
                                cx,
                                cy + 2.0 * s,
                                cx + 4.0 * s,
                                cy - 2.0 * s,
                                1.5 * s,
                                c,
                                1.0,
                            );
                        } else {
                            p.line(
                                cx - 2.0 * s,
                                cy - 4.0 * s,
                                cx + 2.0 * s,
                                cy,
                                1.5 * s,
                                c,
                                1.0,
                            );
                            p.line(
                                cx + 2.0 * s,
                                cy,
                                cx - 2.0 * s,
                                cy + 4.0 * s,
                                1.5 * s,
                                c,
                                1.0,
                            );
                        }
                        let n = other.len();
                        let t = if n == 1 {
                            "1 puerto más (no web)".to_string()
                        } else {
                            format!("{n} puertos más (no web)")
                        };
                        p.ui_text_in(x + pad + 16.0 * s, ry, rh, &t, c, w - 2.0 * pad);
                    }
                    _ => {}
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_agent(
        &self,
        p: &mut Painter,
        theme: &Theme,
        a: &AgentRow,
        i: usize,
        x: f32,
        ry: f32,
        w: f32,
        rh: f32,
        s: f32,
        m: Metrics,
        now: Instant,
        anim: Anim,
        hovered: bool,
        floating: bool,
    ) {
        let dimc = dim(theme);
        let pad = (16.0 * s).round();
        let lh = m.lh;
        let open = self.openness(i, now);
        // Interfaz Flotante: cada agente en su tarjeta; el de la pestaña activa, con un tinte del
        // color de acento (sin la barrita lateral).
        if floating {
            let (c, a) = match (a.active, hovered || open > 0.0) {
                (true, over) => (theme.accent, if over { 0.13 } else { 0.09 }),
                (false, true) => (theme.foreground, 0.07),
                (false, false) => (theme.foreground, 0.035),
            };
            p.rounded(
                x + 8.0 * s,
                ry + 3.0 * s,
                w - 16.0 * s,
                rh - 6.0 * s,
                c,
                a,
                (10.0 * s).round(),
            );
        } else if hovered || open > 0.0 {
            p.rounded(
                x + 6.0 * s,
                ry + 3.0 * s,
                w - 12.0 * s,
                rh - 6.0 * s,
                theme.foreground,
                0.06,
                (8.0 * s).round(),
            );
        }
        if a.active && !floating {
            p.rounded(
                x + 6.0 * s,
                ry + 10.0 * s,
                3.0 * s,
                m.th + 2.0 * lh,
                theme.accent,
                1.0,
                1.5 * s,
            );
        }
        let tx = x + pad;
        let right = x + w - pad;

        // Nombre del agente (grande) y tiempo desde el último cambio de estado.
        let top = ry + 10.0 * s;
        let time = sessions::ago(now.duration_since(a.since));
        let tw = p.ui_measure(&time);
        p.ui_text(
            right - tw,
            top + ((m.th - lh) / 2.0).round(),
            &time,
            dimc,
            tw + 2.0,
        );
        let l1 = top + m.th + 2.0 * s;
        if floating {
            // Flotante: el logo en un cuadro y, a su lado, el nombre y la carpeta.
            let tile = (m.th + lh - 2.0 * s).round();
            p.rounded(
                tx,
                top,
                tile,
                tile,
                theme.foreground,
                0.06,
                (9.0 * s).round(),
            );
            let logo = (tile * 0.55).round();
            let (lx, ly) = (
                tx + ((tile - logo) / 2.0).round(),
                top + ((tile - logo) / 2.0).round(),
            );
            p.agent_logo(
                a.agent.logo,
                a.agent.name,
                lx,
                ly,
                logo,
                theme.foreground,
                1.0,
            );
            let nx = tx + tile + (10.0 * s).round();
            let room = right - tw - 12.0 * s - nx;
            p.ui_text(nx, top + 1.0 * s, a.agent.name, theme.foreground, room);
            let sh = p.ui_line_h(UiVariant::Small);
            let dy = (top + tile - sh - 1.0 * s).round();
            p.ui_text_v(nx, dy, a.label, dimc, right - nx, UiVariant::Small);
        } else {
            let logo = (m.th * 0.85).round();
            p.agent_logo(
                a.agent.logo,
                a.agent.name,
                tx,
                top + (m.th - logo) / 2.0,
                logo,
                theme.foreground,
                1.0,
            );
            let nx = tx + logo + (7.0 * s).round();
            p.ui_text_v(
                nx,
                top,
                a.agent.name,
                theme.foreground,
                right - tw - 12.0 * s - nx,
                UiVariant::Title,
            );
            // Directorio
            p.ui_text(tx, l1, a.label, dimc, right - tx);
        }

        // Estado
        let l2 = l1 + lh;
        let (color, txt_color) = match a.status {
            Status::Working => (theme.accent, theme.foreground),
            // Te espera: en amarillo, lo más visible del panel.
            Status::Blocked(_) => (theme.palette[3], theme.palette[3]),
            Status::Done => (theme.palette[2], theme.palette[2]),
            Status::Waiting | Status::Exited => (dimc, dimc),
        };
        let cy = l2 + lh / 2.0;
        let mut sx = tx + 4.0 * s;
        if a.status == Status::Working {
            spinner(p, sx, cy, s, color, anim);
            sx += 18.0 * s;
        } else {
            p.circle(sx, cy, 3.5 * s, color, 1.0);
            sx += 12.0 * s;
        }
        let label = match a.detail {
            Some(d) => format!("{} · {d}", a.status.label()),
            None => a.status.label().to_string(),
        };
        if floating {
            // El estado en una píldora de su color.
            let tw = p.ui_measure(&label).min(right - sx);
            let (px0, ph) = (tx - 2.0 * s, lh + 4.0 * s);
            p.rounded(
                px0,
                l2 - 2.0 * s,
                sx + tw + 8.0 * s - px0,
                ph,
                color,
                0.14,
                (ph / 2.0).round(),
            );
        }
        p.ui_text(sx, l2, &label, txt_color, right - sx);

        // Subagentes, colgando del agente.
        let mut sy = l2 + lh + 4.0 * s;
        let line_h = lh + 2.0 * s;
        let tree_x = tx + 4.0 * s;
        let sub_x = tx + 30.0 * s;
        // Tronco del árbol: une cada subagente con el anterior (o con el estado del agente).
        let mut trunk = sy - 3.0 * s;
        for sub in a.subagents {
            let mid = (sy + lh / 2.0).round();
            p.line(tree_x, trunk, tree_x, mid, 1.2 * s, theme.foreground, 0.22);
            p.line(
                tree_x,
                mid,
                tree_x + 7.0 * s,
                mid,
                1.2 * s,
                theme.foreground,
                0.22,
            );
            trunk = mid;
            let ix = tx + 18.0 * s;
            if sub.running {
                spinner(p, ix - 4.0 * s, mid, s * 0.7, theme.accent, anim);
            } else {
                let c = theme.palette[2];
                p.line(
                    ix - 3.5 * s,
                    mid,
                    ix - 1.0 * s,
                    mid + 2.5 * s,
                    1.4 * s,
                    c,
                    1.0,
                );
                p.line(
                    ix - 1.0 * s,
                    mid + 2.5 * s,
                    ix + 3.5 * s,
                    mid - 2.5 * s,
                    1.4 * s,
                    c,
                    1.0,
                );
            }
            let elapsed = if sub.running {
                short_duration(
                    SystemTime::now()
                        .duration_since(sub.started)
                        .unwrap_or_default(),
                )
            } else {
                String::new()
            };
            let ew = p.ui_measure(&elapsed);
            p.ui_text(right - ew, sy, &elapsed, dimc, ew + 2.0);
            let avail = right - ew - 8.0 * s - sub_x;
            let (main, extra) = if sub.description.is_empty() {
                (sub.kind.as_str(), "")
            } else {
                (sub.description.as_str(), sub.kind.as_str())
            };
            let c = if sub.running { theme.foreground } else { dimc };
            let dw = p.ui_text(sub_x, sy, main, c, avail);
            // El tipo, solo si cabe entero.
            let gap = 8.0 * s;
            if !extra.is_empty() && p.ui_measure(extra) <= avail - dw - gap {
                p.ui_text(sub_x + dw + gap, sy, extra, dimc, avail - dw - gap);
            }
            sy += line_h;
            if let (true, Some(act)) = (sub.running, &sub.activity) {
                p.ui_text(sub_x, sy, act, dimc, right - sub_x);
                sy += line_h;
            }
        }

        // Vista previa de la pestaña (al desplegarse).
        if open > 0.0 {
            let by = (sy + 4.0 * s).round();
            let bh = (ry + rh - 6.0 * s - by).floor();
            if bh > 2.0 * s {
                let bx = x + 12.0 * s;
                let bw = w - 24.0 * s;
                p.rounded(bx, by, bw, bh, theme.background, 1.0, (7.0 * s).round());
                p.rounded(bx, by, bw, bh, theme.foreground, 0.05, (7.0 * s).round());
                let px = bx + 8.0 * s;
                let pmax = bx + bw - 8.0 * s;
                let mut py = by + 8.0 * s;
                let lines = a.preview.as_deref().unwrap_or_default();
                if lines.is_empty() && py + m.plh <= by + bh {
                    p.ui_text_v(
                        px,
                        py,
                        "(pantalla vacía)",
                        dimc,
                        pmax - px,
                        UiVariant::Small,
                    );
                }
                for line in lines {
                    if py + m.plh > by + bh {
                        break;
                    }
                    let mut cx = px;
                    for (text, color) in line {
                        if cx >= pmax {
                            break;
                        }
                        cx += p.ui_text_v(cx, py, text, *color, pmax - cx, UiVariant::Small);
                    }
                    py += m.plh;
                }
            }
        }
    }
}

/// Líneas que ocupan los subagentes de una fila (los que trabajan llevan su actividad debajo).
pub fn sub_lines(subs: &[SubAgent]) -> usize {
    subs.iter()
        .map(|s| 1 + usize::from(s.running && s.activity.is_some()))
        .sum()
}

/// "12 s", "3 min", "1 h"
fn short_duration(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{} min", s / 60),
        _ => format!("{} h", s / 3600),
    }
}

/// Tres puntos que se encienden por turnos.
/// Animación "trabajando" (`tabs::Working`) en el sitio de los tres puntos: de `x` (centro del
/// primero) a `x + 12·s`, centrada en `cy`.
pub fn spinner(p: &mut Painter, x: f32, cy: f32, s: f32, c: Rgb, anim: Anim) {
    use std::f32::consts::TAU;
    let t = (anim.ms % 3_600_000) as f32 / 1000.0;
    let cx = x + 6.0 * s;
    match anim.kind {
        Working::Dots => {
            let phase = (anim.ms / 300) as usize;
            for i in 0..3 {
                let a = if i == phase % 3 { 1.0 } else { 0.3 };
                p.circle(x + i as f32 * 6.0 * s, cy, 2.2 * s, c, a);
            }
        }
        Working::Wave => {
            for i in 0..3 {
                let k = (t / 0.9 * TAU - i as f32 * 0.9).sin().max(0.0);
                let y = cy + 1.2 * s - 3.6 * s * k;
                p.circle(x + i as f32 * 6.0 * s, y, 2.0 * s, c, 0.45 + 0.55 * k);
            }
        }
        Working::Pulse => {
            let q = (t / 1.2).fract();
            p.ring(cx, cy, (2.4 + 4.6 * q) * s, 1.2 * s, c, 0.7 * (1.0 - q));
            p.circle(cx, cy, 2.2 * s, c, 1.0);
        }
        Working::Spin => {
            let n = 8;
            let head = (anim.ms / 90) as usize % n;
            for k in 0..n {
                let age = (head + n - k) % n;
                let a = (1.0 - age as f32 / n as f32).powi(2);
                let ang = k as f32 / n as f32 * TAU - TAU / 4.0;
                let (px, py) = (cx + 4.8 * s * ang.cos(), cy + 4.8 * s * ang.sin());
                p.circle(px, py, 1.25 * s, c, 0.12 + 0.88 * a);
            }
        }
        Working::Bar => {
            let (bw, bh) = (16.0 * s, (3.0 * s).round().max(2.0));
            let (bx, by) = (cx - bw / 2.0, (cy - bh / 2.0).round());
            p.rounded(bx, by, bw, bh, c, 0.22, bh / 2.0);
            let seg = 6.0 * s;
            let q = 0.5 - 0.5 * (t / 1.2 * TAU).cos();
            p.rounded(bx + q * (bw - seg), by, seg, bh, c, 1.0, bh / 2.0);
        }
        Working::Still => p.circle(cx, cy, 2.6 * s, c, 1.0),
    }
}

fn robot_icon(p: &mut Painter, cx: f32, cy: f32, s: f32, c: Rgb, bg: Rgb) {
    p.rounded(
        cx - 7.0 * s,
        cy - 3.5 * s,
        14.0 * s,
        10.0 * s,
        c,
        1.0,
        3.0 * s,
    );
    p.circle(cx - 3.0 * s, cy + 1.5 * s, 1.6 * s, bg, 1.0);
    p.circle(cx + 3.0 * s, cy + 1.5 * s, 1.6 * s, bg, 1.0);
    p.line(cx, cy - 3.5 * s, cx, cy - 6.5 * s, 1.4 * s, c, 1.0);
    p.circle(cx, cy - 7.0 * s, 1.5 * s, c, 1.0);
}

fn globe_icon(p: &mut Painter, cx: f32, cy: f32, s: f32, c: Rgb) {
    p.ring(cx, cy, 6.5 * s, 1.4 * s, c, 1.0);
    p.line(cx - 6.5 * s, cy, cx + 6.5 * s, cy, 1.3 * s, c, 1.0);
    p.line(cx, cy - 6.5 * s, cx, cy + 6.5 * s, 1.3 * s, c, 1.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn despliegue_animado() {
        let mut st = PanelState {
            agents: true,
            ..Default::default()
        };
        let t0 = Instant::now();
        assert!(st.set_hover(Some(Hit::Agent(1)), t0));
        assert_eq!(st.openness(1, t0), 0.0);
        assert!(st.animating(t0));
        let t1 = t0 + ANIM;
        assert_eq!(st.openness(1, t1), 1.0);
        assert!(!st.animating(t1));
        // Al pasar a otra fila, la anterior se recoge mientras la nueva se despliega.
        st.set_hover(Some(Hit::Agent(0)), t1);
        let mid = t1 + ANIM / 2;
        let (a, b) = (st.openness(0, mid), st.openness(1, mid));
        assert!(a > 0.0 && a < 1.0 && (a + b - 1.0).abs() < 1e-4);
        assert_eq!(st.previewed(mid).count(), 2);
        // Fuera del panel: todo recogido al terminar la animación.
        st.set_hover(None, t1 + ANIM);
        assert_eq!(st.openness(0, t1 + 2 * ANIM), 0.0);
        assert_eq!(st.previewed(t1 + 2 * ANIM).count(), 0);
    }

    #[test]
    fn la_fila_desplegada_no_desaparece() {
        let st = PanelState {
            agents: true,
            expand: Some(0),
            ..Default::default()
        };
        let m = Metrics {
            lh: 17.0,
            th: 21.0,
            plh: 15.0,
        };
        let now = Instant::now();
        // Panel muy bajo: la fila cabe sin vista previa; la vista previa se recorta.
        let l = st.layout(&[0], 0.0, 0.0, 300.0, 140.0, 1.0, m, now);
        assert_eq!(l.rows.len(), 1);
        let (_, ry, rh) = l.rows[0];
        assert!(ry + rh <= 140.0);
    }
}
