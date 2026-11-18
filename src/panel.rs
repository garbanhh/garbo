//! Panel lateral: agentes en ejecución en las pestañas y puertos en escucha.

use crate::config::{Rgb, Theme};
use crate::ports::PortInfo;
use crate::sessions::{self, Agent, Status};
use crate::ui::{cross, dim};
use crate::view::Painter;
use std::time::Instant;

#[derive(Default)]
pub struct PanelState {
    pub agents: bool,
    pub ports: bool,
    pub more_ports: bool,
    pub hover: Option<Hit>,
    pub ports_data: Vec<PortInfo>,
    pub ports_loaded: bool,
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

/// Una fila de la sección de agentes (una pestaña con agente).
pub struct AgentRow<'a> {
    pub label: &'a str,
    pub agent: Agent,
    pub status: Status,
    pub since: Instant,
    pub active: bool,
}

struct Section {
    y: f32,
    close: (f32, f32, f32),
}

struct Layout {
    agents: Option<Section>,
    ports: Option<Section>,
    rows: Vec<(Hit, f32, f32)>,
    header_h: f32,
}

impl PanelState {
    pub fn visible(&self) -> bool {
        self.agents || self.ports
    }

    pub fn width(scale: f32, win_w: f32) -> f32 {
        (340.0 * scale).min(win_w * 0.45).round()
    }

    fn split(&self) -> (Vec<&PortInfo>, Vec<&PortInfo>) {
        self.ports_data.iter().partition(|p| p.web)
    }

    #[allow(clippy::too_many_arguments)]
    fn layout(&self, n_agents: usize, x: f32, y: f32, w: f32, h: f32, s: f32, lh: f32) -> Layout {
        let _ = (x, w);
        let header_h = (44.0 * s).round();
        let agent_h = (3.0 * lh + 20.0 * s).round();
        let port_h = (2.0 * lh + 16.0 * s).round();
        let line_h = (lh + 14.0 * s).round();
        let mut rows = Vec::new();

        let agents_content = header_h + (n_agents.max(1) as f32) * agent_h + 8.0 * s;
        let (agents_h, ports_h) = match (self.agents, self.ports) {
            (true, true) => {
                let a = agents_content.min(h * 0.55).max(header_h + agent_h);
                (a, h - a)
            }
            (true, false) => (h, 0.0),
            (false, true) => (0.0, h),
            _ => (0.0, 0.0),
        };
        let close = |sy: f32| (x + w - 22.0 * s, sy + header_h / 2.0, 12.0 * s);

        let agents = self.agents.then(|| {
            let mut ry = y + header_h;
            for i in 0..n_agents {
                if ry + agent_h > y + agents_h {
                    break;
                }
                rows.push((Hit::Agent(i), ry, agent_h));
                ry += agent_h;
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
            Section { y: sy, close: close(sy) }
        });
        Layout { agents, ports, rows, header_h }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn hit(&self, mx: f32, my: f32, n_agents: usize, x: f32, y: f32, w: f32, h: f32, s: f32, lh: f32) -> Option<Hit> {
        if mx < x || mx > x + w || my < y || my > y + h {
            return None;
        }
        let l = self.layout(n_agents, x, y, w, h, s, lh);
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
        phase: usize,
    ) {
        let lh = p.ui_metrics().height;
        let l = self.layout(agents.len(), x, y, w, h, s, lh);
        let hair = s.round().max(1.0);
        let dimc = dim(theme);
        let pad = (16.0 * s).round();
        p.rect(x, y, w, h, theme.tab_bar, 1.0);
        p.rect(x, y, hair, h, theme.foreground, 0.10);

        let header = |p: &mut Painter, sec: &Section, title: &str, extra: &str, robot: bool, hovered: bool| {
            let cy = sec.y + l.header_h / 2.0;
            let ix = x + pad + 7.0 * s;
            if robot {
                robot_icon(p, ix, cy, s, theme.foreground, theme.tab_bar);
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
            p.rect(x, sec.y + l.header_h - hair, w, hair, theme.foreground, 0.08);
        };

        // --- Agentes
        if let Some(sec) = &l.agents {
            let working = agents.iter().filter(|a| a.status == Status::Working).count();
            let extra = if working > 0 { format!("{working} trabajando") } else { String::new() };
            header(p, sec, "Agentes", &extra, true, self.hover == Some(Hit::CloseAgents));
            if agents.is_empty() {
                p.ui_text_in(x + pad, sec.y + l.header_h, lh * 2.5, "Ningún agente en ejecución", dimc, w - 2.0 * pad);
            }
            for &(hit, ry, rh) in &l.rows {
                let Hit::Agent(i) = hit else { continue };
                let a = &agents[i];
                if self.hover == Some(hit) {
                    p.rounded(x + 6.0 * s, ry + 3.0 * s, w - 12.0 * s, rh - 6.0 * s, theme.foreground, 0.06, (8.0 * s).round());
                }
                if a.active {
                    p.rounded(x + 6.0 * s, ry + 10.0 * s, 3.0 * s, rh - 20.0 * s, theme.accent, 1.0, 1.5 * s);
                }
                let bx = x + pad;
                let line = |k: f32| ry + 10.0 * s + k * lh;
                badge(p, bx, line(0.0), lh, a.agent, s);
                let tx = bx + lh + 10.0 * s;
                let time = sessions::ago(now.duration_since(a.since));
                let tw = p.ui_measure(&time);
                p.ui_text(x + w - pad - tw, line(0.0), &time, dimc, tw + 2.0);
                p.ui_text(tx, line(0.0), a.label, theme.foreground, x + w - pad - tw - 12.0 * s - tx);
                p.ui_text(tx, line(1.0), a.agent.name, dimc, w - (tx - x) - pad);
                let (color, txt_color) = match a.status {
                    Status::Working => (theme.accent, theme.foreground),
                    Status::Done => (theme.palette[2], theme.palette[2]),
                    Status::Waiting => (dimc, dimc),
                    Status::Exited => (dimc, dimc),
                };
                let cy = line(2.0) + lh / 2.0;
                let mut sx = tx + 4.0 * s;
                if a.status == Status::Working {
                    spinner(p, sx, cy, s, color, phase);
                    sx += 18.0 * s;
                } else {
                    p.circle(sx, cy, 3.5 * s, color, 1.0);
                    sx += 12.0 * s;
                }
                p.ui_text(sx, line(2.0), a.status.label(), txt_color, x + w - pad - sx);
            }
        }

        // --- Puertos
        if let Some(sec) = &l.ports {
            if l.agents.is_some() {
                p.rect(x, sec.y, w, hair, theme.foreground, 0.10);
            }
            let (web, other) = self.split();
            let extra = match web.len() {
                1 => "1 servidor web".to_string(),
                n => format!("{n} servidores web"),
            };
            header(p, sec, "Puertos", if self.ports_loaded { &extra } else { "buscando…" }, false, self.hover == Some(Hit::ClosePorts));
            if self.ports_loaded && web.is_empty() {
                let t = "No hay servidores web";
                let tw = p.ui_measure(t);
                p.ui_text_in(x + (w - tw) / 2.0, sec.y + l.header_h, 2.0 * lh + 16.0 * s, t, dimc, w);
            }
            for &(hit, ry, rh) in &l.rows {
                match hit {
                    Hit::Port(port) => {
                        let Some(info) = self.ports_data.iter().find(|p| p.port == port) else { continue };
                        if self.hover == Some(hit) {
                            p.rounded(x + 6.0 * s, ry + 2.0 * s, w - 12.0 * s, rh - 4.0 * s, theme.foreground, 0.06, (8.0 * s).round());
                        }
                        let proc_name = info.process.clone().unwrap_or_else(|| "sistema".into());
                        if info.web {
                            p.circle(x + pad + 4.0 * s, ry + 8.0 * s + lh / 2.0, 3.5 * s, theme.palette[2], 1.0);
                            let tx = x + pad + 16.0 * s;
                            p.ui_text(tx, ry + 8.0 * s, &format!("localhost:{port}"), theme.foreground, w - (tx - x) - pad - 20.0 * s);
                            p.ui_text(tx, ry + 8.0 * s + lh, &proc_name, dimc, w - (tx - x) - pad);
                            // Flecha "abrir en el navegador"
                            let ax = x + w - pad - 6.0 * s;
                            let ay = ry + 8.0 * s + lh / 2.0;
                            let c = if self.hover == Some(hit) { theme.foreground } else { dimc };
                            p.line(ax - 5.0 * s, ay + 5.0 * s, ax + 4.0 * s, ay - 4.0 * s, 1.4 * s, c, 1.0);
                            p.line(ax - 2.0 * s, ay - 4.0 * s, ax + 4.0 * s, ay - 4.0 * s, 1.4 * s, c, 1.0);
                            p.line(ax + 4.0 * s, ay - 4.0 * s, ax + 4.0 * s, ay + 2.0 * s, 1.4 * s, c, 1.0);
                        } else {
                            let tx = x + pad + 16.0 * s;
                            let t = format!(":{port}");
                            let tw = p.ui_text_in(tx, ry, rh, &t, dimc, w * 0.3);
                            p.ui_text_in(tx + tw + 12.0 * s, ry, rh, &proc_name, dimc, w - (tx - x) - tw - pad);
                        }
                    }
                    Hit::MorePorts => {
                        let cx = x + pad + 4.0 * s;
                        let cy = ry + rh / 2.0;
                        let c = if self.hover == Some(hit) { theme.foreground } else { dimc };
                        if self.more_ports {
                            p.line(cx - 4.0 * s, cy - 2.0 * s, cx, cy + 2.0 * s, 1.5 * s, c, 1.0);
                            p.line(cx, cy + 2.0 * s, cx + 4.0 * s, cy - 2.0 * s, 1.5 * s, c, 1.0);
                        } else {
                            p.line(cx - 2.0 * s, cy - 4.0 * s, cx + 2.0 * s, cy, 1.5 * s, c, 1.0);
                            p.line(cx + 2.0 * s, cy, cx - 2.0 * s, cy + 4.0 * s, 1.5 * s, c, 1.0);
                        }
                        let n = other.len();
                        let t = if n == 1 { "1 puerto más (no web)".to_string() } else { format!("{n} puertos más (no web)") };
                        p.ui_text_in(x + pad + 16.0 * s, ry, rh, &t, c, w - 2.0 * pad);
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Insignia genérica del agente: inicial sobre un cuadrado redondeado de su color.
pub fn badge(p: &mut Painter, x: f32, y: f32, size: f32, agent: Agent, s: f32) {
    p.rounded(x, y, size, size, agent.color, 0.18, (5.0 * s).round());
    let t = agent.badge.to_string();
    let tw = p.ui_measure(&t);
    p.ui_text_in((x + (size - tw) / 2.0).round(), y, size, &t, agent.color, size);
}

/// Tres puntos que se encienden por turnos.
pub fn spinner(p: &mut Painter, x: f32, cy: f32, s: f32, c: Rgb, phase: usize) {
    for i in 0..3 {
        let a = if i == phase % 3 { 1.0 } else { 0.3 };
        p.circle(x + i as f32 * 6.0 * s, cy, 2.2 * s, c, a);
    }
}

fn robot_icon(p: &mut Painter, cx: f32, cy: f32, s: f32, c: Rgb, bg: Rgb) {
    p.rounded(cx - 7.0 * s, cy - 3.5 * s, 14.0 * s, 10.0 * s, c, 1.0, 3.0 * s);
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
