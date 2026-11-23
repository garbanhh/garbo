//! Apartado de estadísticas de los agentes (`Ctrl+Shift+S` o el botón de la barra): ocupa la
//! ventana bajo la barra de pestañas.
//!
//! De arriba abajo: cabecera con el periodo, cuatro cifras con su evolución, tokens por día,
//! cuándo trabajan (día de la semana × hora), modelos y agentes. Pulsar un agente muestra solo
//! sus datos. Todo se mide en píxeles lógicos × escala; si no cabe, la rueda lo desplaza (lo que
//! sube queda bajo la barra de pestañas, que se dibuja después).

use super::{
    compact, duration_long, hours, short_date, AgentKind, AgentRow, FileStats, Period, Provider,
    StatsMsg, Summary, WEEKDAYS, WEEKDAYS_LONG,
};
use crate::atlas::UiVariant;
use crate::config::{Rgb, Theme};
use crate::ui::{cross, dim, surface};
use crate::view::Painter;
use std::sync::Arc;

type Rect = [f32; 4];

/// Medidas en píxeles lógicos.
const MARGIN: f32 = 24.0;
const MAX_W: f32 = 1360.0;
const GAP: f32 = 14.0;
const HEADER_H: f32 = 64.0;
const KPI_H: f32 = 132.0;
const CHART_H: f32 = 272.0;
const TITLE_BAND: f32 = 46.0;
const PROVIDER_BAND: f32 = 46.0;
const COLHEAD: f32 = 24.0;
const ROW: f32 = 30.0;
const FOOTER_H: f32 = 48.0;
/// Ancho de cada columna de cifras de las tablas.
const COL_W: f32 = 98.0;
/// Filas de modelos antes de agrupar el resto en "y N más".
const MAX_MODELS: usize = 7;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Close,
    Period(Period),
    Agent(AgentKind),
    /// Barra de la gráfica de tokens.
    Bar(usize),
    /// Casilla de cuándo trabajan: (día de la semana, hora).
    Cell(usize, usize),
    Nothing,
}

impl Hit {
    /// Se puede pulsar (cursor de mano).
    pub fn clickable(self) -> bool {
        matches!(self, Hit::Close | Hit::Period(_) | Hit::Agent(_))
    }
}

pub struct StatsView {
    pub period: Period,
    pub agent: Option<AgentKind>,
    data: Option<Arc<Vec<FileStats>>>,
    progress: Option<(usize, usize)>,
    summary: Option<Summary>,
    scroll: f32,
    pub hover: Option<Hit>,
    /// Ratón (para colocar la ventanita de detalles).
    pub mouse: (f32, f32),
}

impl Default for StatsView {
    fn default() -> Self {
        Self::new()
    }
}

impl StatsView {
    pub fn new() -> Self {
        Self {
            period: Period::Month,
            agent: None,
            data: None,
            progress: None,
            summary: None,
            scroll: 0.0,
            hover: None,
            mouse: (0.0, 0.0),
        }
    }

    /// Mensaje del hilo de lectura.
    pub fn on_msg(&mut self, msg: StatsMsg) {
        match msg {
            StatsMsg::Progress(done, total) => self.progress = Some((done, total)),
            StatsMsg::Data(d) => {
                self.data = Some(d);
                self.progress = None;
                self.refresh();
            }
        }
    }

    fn refresh(&mut self) {
        self.summary = self
            .data
            .as_ref()
            .map(|d| super::summarize(d, self.period, self.agent, super::today()));
    }

    pub fn set_period(&mut self, p: Period) {
        if self.period != p {
            self.period = p;
            self.refresh();
        }
    }

    /// Periodo anterior o siguiente (← →).
    pub fn step_period(&mut self, d: i32) {
        let i = Period::ALL
            .iter()
            .position(|p| *p == self.period)
            .unwrap_or(1) as i32;
        let n = Period::ALL.len() as i32;
        self.set_period(Period::ALL[(i + d).clamp(0, n - 1) as usize]);
    }

    /// Pulsar un agente: solo sus datos; pulsarlo otra vez, todos.
    pub fn toggle_agent(&mut self, a: AgentKind) {
        self.agent = (self.agent != Some(a)).then_some(a);
        self.refresh();
    }

    /// Tab: el siguiente agente con datos (y al final, todos).
    pub fn cycle_agent(&mut self, back: bool) {
        let list: Vec<AgentKind> = self
            .summary
            .as_ref()
            .map(|s| s.agents.iter().map(|a| a.agent).collect())
            .unwrap_or_default();
        if list.is_empty() {
            return;
        }
        let mut options: Vec<Option<AgentKind>> = vec![None];
        options.extend(list.into_iter().map(Some));
        let i = options.iter().position(|o| *o == self.agent).unwrap_or(0);
        let n = options.len();
        let next = if back { (i + n - 1) % n } else { (i + 1) % n };
        self.agent = options[next];
        self.refresh();
    }

    /// Desplaza el contenido `px` píxeles (positivo: hacia abajo).
    pub fn scroll_by(&mut self, px: f32, w: f32, h: f32, top: f32, s: f32) {
        self.scroll += px;
        let l = self.layout(w, h, top, s);
        self.scroll = self.scroll.clamp(0.0, l.max_scroll);
    }

    pub fn scroll_to(&mut self, end: bool, w: f32, h: f32, top: f32, s: f32) {
        self.scroll = if end { f32::MAX } else { 0.0 };
        self.scroll_by(0.0, w, h, top, s);
    }

    /// Filas de la tabla de agentes: los que tienen datos, y el elegido aunque no tenga.
    fn agent_rows(&self) -> Vec<AgentRow> {
        let mut rows = self
            .summary
            .as_ref()
            .map_or_else(Vec::new, |s| s.agents.clone());
        if let Some(a) = self.agent.filter(|a| !rows.iter().any(|r| r.agent == *a)) {
            rows.push(AgentRow {
                agent: a,
                usage: Default::default(),
                work: 0,
                prompts: 0,
            });
        }
        rows
    }

    fn layout(&self, w: f32, h: f32, top: f32, s: f32) -> Layout {
        let px = |v: f32| (v * s).round();
        let cw = (w - 2.0 * px(MARGIN)).min(px(MAX_W)).max(px(280.0));
        let x = ((w - cw) / 2.0).round();
        let start = top + px(16.0);
        let mut y = start - self.scroll.round();
        let gap = px(GAP);

        // Cabecera: título a la izquierda; periodo y cerrar a la derecha.
        let r = px(15.0);
        let close = (x + cw - r, y + px(24.0), r);
        let seg_w = if cw < px(640.0) { px(58.0) } else { px(70.0) };
        let seg_h = px(30.0);
        let segs_x = close.0 - r - px(14.0) - 4.0 * seg_w;
        let segs: Vec<Rect> = (0..4)
            .map(|i| {
                [
                    segs_x + i as f32 * seg_w,
                    close.1 - seg_h / 2.0,
                    seg_w,
                    seg_h,
                ]
            })
            .collect();
        let header_y = y;
        y += px(HEADER_H);

        let mut l = Layout {
            x,
            cw,
            header_y,
            close,
            segs,
            kpis: Vec::new(),
            tokens: [0.0; 4],
            plot: [0.0; 4],
            heat: [0.0; 4],
            grid: [0.0; 4],
            models: [0.0; 4],
            agents: [0.0; 4],
            model_rows: 0,
            more_models: 0,
            agent_rows: Vec::new(),
            message: None,
            footer_y: 0.0,
            max_scroll: 0.0,
        };

        let summary = match &self.summary {
            Some(sm) if sm.any_data => sm,
            _ => {
                l.message = Some([x, y + px(40.0), cw, px(120.0)]);
                l.footer_y = y + px(180.0);
                return l;
            }
        };

        // Cifras: 4 en fila, o 2 × 2 si no caben.
        // En 4 columnas, por debajo de 1000 px el nombre no cabe junto al cambio.
        let cols = if cw >= px(1000.0) { 4 } else { 2 };
        let kw = ((cw - gap * (cols as f32 - 1.0)) / cols as f32).floor();
        let kh = px(KPI_H);
        for i in 0..4 {
            let (c, r) = (i % cols, i / cols);
            l.kpis
                .push([x + c as f32 * (kw + gap), y + r as f32 * (kh + gap), kw, kh]);
        }
        y += (4 / cols) as f32 * (kh + gap) + gap;

        // Gráficas: tokens por día (más ancha) y cuándo trabajan, lado a lado si caben.
        let wide = cw >= px(920.0);
        let ch = px(CHART_H);
        if wide {
            let tw = ((cw - gap) * 0.58).round();
            l.tokens = [x, y, tw, ch];
            l.heat = [x + tw + gap, y, cw - tw - gap, ch];
            y += ch + gap * 2.0;
        } else {
            l.tokens = [x, y, cw, ch];
            l.heat = [x, y + ch + gap, cw, ch];
            y += 2.0 * ch + gap * 3.0;
        }
        let pad = px(16.0);
        let label_w = px(72.0);
        let [tx, ty, tw, th] = l.tokens;
        l.plot = [
            tx + pad + label_w,
            ty + px(74.0),
            tw - 2.0 * pad - label_w,
            th - px(74.0 + 36.0),
        ];
        let [hx, hy, hw, hh] = l.heat;
        let lab = px(22.0);
        l.grid = [
            hx + pad + lab,
            hy + px(56.0),
            hw - 2.0 * pad - lab,
            hh - px(56.0 + 56.0),
        ];

        // Tablas de modelos y de agentes.
        let n = summary.models.len();
        (l.model_rows, l.more_models) = if n > MAX_MODELS {
            (MAX_MODELS - 1, n - (MAX_MODELS - 1))
        } else {
            (n, 0)
        };
        let shown = (l.model_rows + usize::from(l.more_models > 0)).max(1);
        let mh = px(TITLE_BAND + PROVIDER_BAND + COLHEAD) + shown as f32 * px(ROW) + px(12.0);
        let agents = self.agent_rows();
        let ah = px(TITLE_BAND + COLHEAD) + agents.len().max(1) as f32 * px(ROW) + px(12.0);
        let (models, agents_r) = if wide {
            let half = ((cw - gap) / 2.0).floor();
            let th = mh.max(ah);
            let r = ([x, y, half, th], [x + half + gap, y, cw - half - gap, th]);
            y += th;
            r
        } else {
            let r = ([x, y, cw, mh], [x, y + mh + gap, cw, ah]);
            y += mh + gap + ah;
            r
        };
        l.models = models;
        l.agents = agents_r;
        let row0 = agents_r[1] + px(TITLE_BAND + COLHEAD);
        l.agent_rows = agents
            .iter()
            .enumerate()
            .map(|(i, a)| {
                (
                    a.agent,
                    [
                        agents_r[0] + px(6.0),
                        row0 + i as f32 * px(ROW),
                        agents_r[2] - px(12.0),
                        px(ROW),
                    ],
                )
            })
            .collect();

        l.footer_y = y;
        let content = y + px(FOOTER_H) + self.scroll.round() - start;
        l.max_scroll = (content - (h - start)).max(0.0);
        l
    }

    pub fn hit(&self, mx: f32, my: f32, w: f32, h: f32, top: f32, s: f32) -> Hit {
        if my < top {
            return Hit::Nothing;
        }
        let l = self.layout(w, h, top, s);
        let (cx, cy, r) = l.close;
        if (mx - cx).hypot(my - cy) <= r {
            return Hit::Close;
        }
        let inside = |r: &Rect| mx >= r[0] && mx < r[0] + r[2] && my >= r[1] && my < r[1] + r[3];
        for (i, seg) in l.segs.iter().enumerate() {
            if inside(seg) {
                return Hit::Period(Period::ALL[i]);
            }
        }
        for (a, r) in &l.agent_rows {
            if inside(r) {
                return Hit::Agent(*a);
            }
        }
        let Some(sm) = &self.summary else {
            return Hit::Nothing;
        };
        let p = l.plot;
        if !sm.bins.is_empty()
            && mx >= p[0]
            && mx < p[0] + p[2]
            && my >= p[1] - 8.0 * s
            && my < p[1] + p[3]
        {
            let slot = p[2] / sm.bins.len() as f32;
            return Hit::Bar((((mx - p[0]) / slot) as usize).min(sm.bins.len() - 1));
        }
        let g = l.grid;
        if inside(&g) {
            let col = (((mx - g[0]) / (g[2] / 24.0)) as usize).min(23);
            let row = (((my - g[1]) / (g[3] / 7.0)) as usize).min(6);
            return Hit::Cell(row, col);
        }
        Hit::Nothing
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, p: &mut Painter, theme: &Theme, w: f32, h: f32, top: f32, s: f32) {
        let l = self.layout(w, h, top, s);
        self.scroll = self.scroll.min(l.max_scroll);
        let px = |v: f32| (v * s).round();
        let dimc = dim(theme);
        let lh = p.ui_line_h(UiVariant::Normal);
        let sh = p.ui_line_h(UiVariant::Small);
        let th = p.ui_line_h(UiVariant::Title);

        // --- Cabecera
        let hy = l.header_y;
        p.ui_text_v(
            l.x,
            hy + px(4.0),
            "Uso de los agentes",
            theme.foreground,
            l.cw / 2.0,
            UiVariant::Title,
        );
        let subtitle = match &self.summary {
            Some(sm) if sm.any_data => {
                let mut t = format!("{} – hoy", short_date(sm.first, sm.today));
                if let Some(a) = sm.agent {
                    t.push_str(&format!(" · solo {}", a.name()));
                }
                t
            }
            _ => "Claude Code, Codex y Gemini CLI".into(),
        };
        p.ui_text_v(
            l.x,
            hy + px(6.0) + th,
            &subtitle,
            dimc,
            l.segs[0][0] - l.x - px(12.0),
            UiVariant::Small,
        );
        // Selector de periodo: una píldora con el elegido resaltado.
        let first = l.segs[0];
        let last = l.segs[3];
        let rad = (first[3] / 2.0).round();
        p.rounded(
            first[0],
            first[1],
            last[0] + last[2] - first[0],
            first[3],
            theme.foreground,
            0.06,
            rad,
        );
        for (i, r) in l.segs.iter().enumerate() {
            let period = Period::ALL[i];
            let on = period == self.period;
            let over = self.hover == Some(Hit::Period(period));
            if on {
                p.rounded(
                    r[0] + px(2.0),
                    r[1] + px(2.0),
                    r[2] - px(4.0),
                    r[3] - px(4.0),
                    theme.accent,
                    0.22,
                    rad - px(2.0),
                );
            } else if over {
                p.rounded(
                    r[0] + px(2.0),
                    r[1] + px(2.0),
                    r[2] - px(4.0),
                    r[3] - px(4.0),
                    theme.foreground,
                    0.07,
                    rad - px(2.0),
                );
            }
            let label = period.label();
            let tw = p.ui_measure(label);
            let c = if on || over { theme.foreground } else { dimc };
            p.ui_text_in(
                (r[0] + (r[2] - tw) / 2.0).round(),
                r[1],
                r[3],
                label,
                c,
                r[2],
            );
        }
        let (cx, cy, r) = l.close;
        p.circle(
            cx,
            cy,
            r,
            theme.foreground,
            if self.hover == Some(Hit::Close) {
                0.14
            } else {
                0.06
            },
        );
        cross(p, cx, cy, 4.0 * s, 1.5 * s, theme.foreground);

        // --- Sin datos (aún)
        if let Some(m) = l.message {
            let (title, note) = match (&self.summary, self.progress) {
                (Some(_), _) => (
                    "Aún no hay sesiones de agentes".to_string(),
                    "Se leen las que guardan Claude Code (~/.claude), Codex (~/.codex) y Gemini CLI (~/.gemini).".to_string(),
                ),
                (None, Some((done, total))) => (
                    "Leyendo las sesiones de los agentes…".to_string(),
                    format!("{done} de {total} archivos"),
                ),
                (None, None) => ("Leyendo las sesiones de los agentes…".to_string(), String::new()),
            };
            let tw = p.ui_measure_v(&title, UiVariant::Title);
            p.ui_text_v(
                (m[0] + (m[2] - tw) / 2.0).round(),
                m[1],
                &title,
                theme.foreground,
                m[2],
                UiVariant::Title,
            );
            let nw = p.ui_measure_v(&note, UiVariant::Small);
            p.ui_text_v(
                (m[0] + (m[2] - nw) / 2.0).round(),
                m[1] + th + px(8.0),
                &note,
                dimc,
                m[2],
                UiVariant::Small,
            );
            return;
        }
        let Some(sm) = self.summary.as_ref() else {
            return;
        };
        let pad = px(16.0);

        // --- Cifras
        let t = &sm.total;
        let prev = sm.prev.as_ref();
        let input = t.usage.input_total();
        let calls = t.usage.calls as u64;
        let cache_pct = (t.usage.cache_read * 100).checked_div(input);
        let kpis: [Kpi; 4] = [
            (
                "Tokens de entrada",
                compact(input),
                cache_pct.map_or("sin actividad".into(), |p| format!("{p} % desde la caché")),
                prev.map(|p| (input, p.usage.input_total())),
                sm.bins
                    .iter()
                    .map(|b| b.usage.input_total() as f64)
                    .collect(),
            ),
            (
                "Tokens de salida",
                compact(t.usage.output),
                (t.usage.output)
                    .checked_div(calls)
                    .map_or("sin respuestas".into(), |o| {
                        format!("{} por respuesta", compact(o))
                    }),
                prev.map(|p| (t.usage.output, p.usage.output)),
                sm.bins.iter().map(|b| b.usage.output as f64).collect(),
            ),
            (
                "Peticiones",
                if t.prompts < 10_000 {
                    t.prompts.to_string()
                } else {
                    compact(t.prompts)
                },
                match t.sessions {
                    1 => "en 1 sesión".into(),
                    n => format!("en {n} sesiones"),
                },
                prev.map(|p| (t.prompts, p.prompts)),
                sm.bins.iter().map(|b| b.prompts as f64).collect(),
            ),
            (
                "Tiempo trabajando",
                if t.work == 0 {
                    "0 h".into()
                } else {
                    hours(t.work)
                },
                match t.peak {
                    0 => "sin trabajo".into(),
                    1 => "un agente a la vez".into(),
                    n => format!("hasta {n} a la vez"),
                },
                prev.map(|p| (t.work, p.work)),
                sm.bins.iter().map(|b| b.work as f64).collect(),
            ),
        ];
        let bh = p.ui_line_h(UiVariant::Big);
        for (r, (label, value, sub, cmp, series)) in l.kpis.iter().zip(kpis.iter()) {
            card(p, theme, *r, s);
            let (x, y, w) = (r[0] + pad, r[1] + px(14.0), r[2] - 2.0 * pad);
            let chip = cmp.and_then(|(c, p)| delta(c, p));
            let chip_w = match &chip {
                Some((text, up)) => {
                    let cw = p.ui_measure_v(text, UiVariant::Small) + px(14.0);
                    let col = if *up { theme.palette[2] } else { dimc };
                    let ch = sh + px(4.0);
                    let cx = r[0] + r[2] - pad - cw;
                    p.rounded(cx, y - px(2.0), cw, ch, col, 0.14, (ch / 2.0).round());
                    p.ui_text_v(cx + px(7.0), y, text, col, cw, UiVariant::Small);
                    cw
                }
                None => 0.0,
            };
            p.ui_text_v(x, y, label, dimc, w - chip_w - px(8.0), UiVariant::Small);
            let vy = y + sh + px(4.0);
            p.ui_text_v(x, vy, value, theme.foreground, w, UiVariant::Big);
            p.ui_text_v(x, vy + bh, sub, dimc, w, UiVariant::Small);
            let spark = [x, r[1] + r[3] - px(30.0), w, px(20.0)];
            sparkline(p, theme, spark, series, s);
        }

        // --- Tokens por día
        let [tx, ty, tw, _] = l.tokens;
        card(p, theme, l.tokens, s);
        let unit = if sm.bin_days == 1 { "día" } else { "semana" };
        p.ui_text(
            tx + pad,
            ty + px(14.0),
            &format!("Tokens por {unit}"),
            theme.foreground,
            tw / 2.0,
        );
        let total_tokens = input + t.usage.output;
        let note = match sm.active_days {
            0 => "sin actividad".to_string(),
            a => format!(
                "{a} de {} días con actividad · {} al día",
                sm.days(),
                compact(total_tokens / a)
            ),
        };
        text_right(
            p,
            tx + tw - pad,
            ty + px(16.0),
            &note,
            dimc,
            tw * 0.55,
            UiVariant::Small,
        );
        // Leyenda
        let mut lx = tx + pad;
        let ly = ty + px(16.0) + lh;
        let (c_cache, c_new, c_out) = (
            (theme.foreground, 0.24),
            (theme.foreground, 0.62),
            (theme.accent, 0.92),
        );
        for (label, (c, a)) in [
            ("caché", c_cache),
            ("entrada nueva", c_new),
            ("salida", c_out),
        ] {
            let sq = px(8.0);
            p.rounded(lx, ly + (sh - sq) / 2.0, sq, sq, c, a, px(2.0));
            lx += sq + px(6.0);
            lx += p.ui_text_v(lx, ly, label, dimc, px(140.0), UiVariant::Small) + px(14.0);
        }
        let [plx, ply, plw, plh] = l.plot;
        let base = (ply + plh * 0.66).round();
        let (up_h, down_h) = (base - ply, ply + plh - base - px(2.0));
        let max_in = sm
            .bins
            .iter()
            .map(|b| b.usage.input_total())
            .max()
            .unwrap_or(0);
        let max_out = sm.bins.iter().map(|b| b.usage.output).max().unwrap_or(0);
        let n = sm.bins.len().max(1);
        let slot = plw / n as f32;
        let bw = (slot * 0.64).clamp(1.0, px(18.0));
        let hovered_bar = match self.hover {
            Some(Hit::Bar(i)) => Some(i),
            _ => None,
        };
        if let Some(i) = hovered_bar {
            p.rounded(
                plx + i as f32 * slot,
                ply - px(6.0),
                slot,
                plh + px(10.0),
                theme.foreground,
                0.06,
                px(3.0),
            );
        }
        p.rect(plx, base, plw, s.round().max(1.0), theme.foreground, 0.12);
        let scale = |v: u64, max: u64, h: f32| {
            if v == 0 || max == 0 {
                0.0
            } else {
                (v as f32 / max as f32 * h).max(1.5 * s)
            }
        };
        for (i, b) in sm.bins.iter().enumerate() {
            let bx = (plx + i as f32 * slot + (slot - bw) / 2.0).round();
            let hc = scale(b.usage.cache_read, max_in, up_h);
            let hn = scale(b.usage.input + b.usage.cache_write, max_in, up_h);
            let ho = scale(b.usage.output, max_out, down_h);
            p.rect(bx, base - hc, bw, hc, c_cache.0, c_cache.1);
            p.rect(bx, base - hc - hn, bw, hn, c_new.0, c_new.1);
            p.rect(bx, base + px(2.0), bw, ho, c_out.0, c_out.1);
        }
        // Escala a la izquierda
        let lx = tx + pad;
        p.ui_text(
            lx,
            ply - px(4.0),
            &compact(max_in),
            theme.foreground,
            px(70.0),
        );
        p.ui_text_v(
            lx,
            ply - px(4.0) + lh,
            "entrada",
            dimc,
            px(70.0),
            UiVariant::Small,
        );
        let oy = ply + plh - lh - sh;
        p.ui_text(lx, oy, &compact(max_out), theme.accent, px(70.0));
        p.ui_text_v(
            lx,
            oy + lh,
            "salida",
            theme.accent,
            px(70.0),
            UiVariant::Small,
        );
        // Fechas
        let xy = ply + plh + px(10.0);
        if !sm.bins.is_empty() {
            let left = short_date(sm.bin_start(0), sm.today);
            p.ui_text_v(plx, xy, &left, dimc, plw / 3.0, UiVariant::Small);
            if n >= 5 {
                let mid = n / 2;
                let label = short_date(sm.bin_start(mid), sm.today);
                let mw = p.ui_measure_v(&label, UiVariant::Small);
                let mx = (plx + (mid as f32 + 0.5) * slot - mw / 2.0).round();
                p.ui_text_v(mx, xy, &label, dimc, mw + 1.0, UiVariant::Small);
            }
            text_right(p, plx + plw, xy, "hoy", dimc, plw / 3.0, UiVariant::Small);
        }

        // --- Cuándo trabajan
        let [hx, hy2, hw, hh] = l.heat;
        card(p, theme, l.heat, s);
        p.ui_text(
            hx + pad,
            hy2 + px(14.0),
            "Cuándo trabajan",
            theme.foreground,
            hw / 2.0,
        );
        let max_cell = sm.heat.iter().flatten().copied().max().unwrap_or(0);
        let busiest = (0..7)
            .flat_map(|d| (0..24).map(move |h| (d, h)))
            .max_by_key(|&(d, h)| sm.heat[d][h])
            .filter(|_| max_cell > 0);
        let note = match busiest {
            Some((d, h)) => format!("más activo: {} a las {h} h", WEEKDAYS[d]),
            None => "sin actividad".into(),
        };
        text_right(
            p,
            hx + hw - pad,
            hy2 + px(16.0),
            &note,
            dimc,
            hw * 0.6,
            UiVariant::Small,
        );
        let [gx, gy, gw, gh] = l.grid;
        let g = px(3.0);
        let cell_w = gw / 24.0;
        let cell_h = gh / 7.0;
        let levels = [0.28, 0.48, 0.72, 1.0];
        for d in 0..7 {
            let cy = gy + d as f32 * cell_h;
            const LETTERS: [&str; 7] = ["L", "M", "X", "J", "V", "S", "D"];
            p.ui_text_v(
                hx + pad,
                (cy + (cell_h - sh) / 2.0).round(),
                LETTERS[d],
                dimc,
                px(20.0),
                UiVariant::Small,
            );
            for (hr, &v) in sm.heat[d].iter().enumerate() {
                let cx = gx + hr as f32 * cell_w;
                let rect = [
                    cx.round(),
                    cy.round(),
                    (cell_w - g).round().max(2.0),
                    (cell_h - g).round().max(2.0),
                ];
                let rr = px(3.0).min(rect[2] / 2.0);
                if self.hover == Some(Hit::Cell(d, hr)) {
                    let o = px(2.0);
                    p.rounded(
                        rect[0] - o,
                        rect[1] - o,
                        rect[2] + 2.0 * o,
                        rect[3] + 2.0 * o,
                        theme.foreground,
                        0.85,
                        rr + o,
                    );
                    p.rounded(
                        rect[0] - o / 2.0,
                        rect[1] - o / 2.0,
                        rect[2] + o,
                        rect[3] + o,
                        surface(theme, 0.045),
                        1.0,
                        rr + o / 2.0,
                    );
                }
                if v == 0 || max_cell == 0 {
                    p.rounded(
                        rect[0],
                        rect[1],
                        rect[2],
                        rect[3],
                        theme.foreground,
                        0.05,
                        rr,
                    );
                } else {
                    let k = ((v as f64 / max_cell as f64 * 4.0).ceil() as usize).clamp(1, 4);
                    p.rounded(
                        rect[0],
                        rect[1],
                        rect[2],
                        rect[3],
                        theme.accent,
                        levels[k - 1],
                        rr,
                    );
                }
            }
        }
        let hy_lab = gy + gh + px(6.0);
        for hr in [0usize, 6, 12, 18] {
            p.ui_text_v(
                (gx + hr as f32 * cell_w).round(),
                hy_lab,
                &hr.to_string(),
                dimc,
                px(30.0),
                UiVariant::Small,
            );
        }
        // Leyenda: menos ▪▪▪▪▪ más
        let ly = hy2 + hh - px(14.0) - sh;
        let sq = px(10.0);
        let mut lx = hx + hw - pad;
        lx -= p.ui_measure_v("más", UiVariant::Small);
        p.ui_text_v(lx, ly, "más", dimc, px(40.0), UiVariant::Small);
        lx -= px(6.0);
        for (i, a) in levels.iter().enumerate().rev() {
            lx -= sq;
            p.rounded(lx, ly + (sh - sq) / 2.0, sq, sq, theme.accent, *a, px(2.0));
            lx -= px(3.0);
            if i == 0 {
                lx -= sq;
                p.rounded(
                    lx,
                    ly + (sh - sq) / 2.0,
                    sq,
                    sq,
                    theme.foreground,
                    0.05,
                    px(2.0),
                );
            }
        }
        lx -= px(6.0) + p.ui_measure_v("menos", UiVariant::Small);
        p.ui_text_v(lx, ly, "menos", dimc, px(60.0), UiVariant::Small);

        // --- Modelos
        let [mx, my, mw, _] = l.models;
        card(p, theme, l.models, s);
        p.ui_text(
            mx + pad,
            my + px(14.0),
            "Modelos",
            theme.foreground,
            mw / 2.0,
        );
        let nmodels = sm.models.len();
        let note = match nmodels {
            0 => "sin uso".to_string(),
            1 => "1 modelo".to_string(),
            n => format!("{n} modelos"),
        };
        text_right(
            p,
            mx + mw - pad,
            my + px(16.0),
            &note,
            dimc,
            mw / 2.0,
            UiVariant::Small,
        );
        // Reparto por empresa: una barra de tramos y su leyenda.
        let total: u64 = sm.providers.iter().map(|p| p.1).sum();
        let by = my + px(TITLE_BAND) + px(2.0);
        let bar_h = px(8.0);
        let (bx0, bw_all) = (mx + pad, mw - 2.0 * pad);
        if total > 0 {
            let gap = px(3.0);
            let usable = bw_all - gap * (sm.providers.len() as f32 - 1.0);
            let mut x = bx0;
            for (prov, v) in &sm.providers {
                let w = (usable * *v as f32 / total as f32).max(bar_h);
                p.rounded(
                    x,
                    by,
                    w.min(bx0 + bw_all - x),
                    bar_h,
                    provider_color(theme, *prov),
                    0.9,
                    bar_h / 2.0,
                );
                x += w + gap;
            }
            let mut x = bx0;
            let ly = by + bar_h + px(10.0);
            for (prov, v) in &sm.providers {
                let c = provider_color(theme, *prov);
                p.circle(x + px(4.0), ly + sh / 2.0, px(4.0), c, 1.0);
                let pct = (*v as f64 / total as f64 * 100.0).round();
                let label = format!("{} {pct} %", prov.name());
                x += px(12.0)
                    + p.ui_text_v(
                        x + px(12.0),
                        ly,
                        &label,
                        dimc,
                        bx0 + bw_all - x,
                        UiVariant::Small,
                    )
                    + px(14.0);
            }
        } else {
            p.rounded(bx0, by, bw_all, bar_h, theme.foreground, 0.06, bar_h / 2.0);
        }
        let cols = Columns::new(mx, mw, s);
        let hy3 = my + px(TITLE_BAND + PROVIDER_BAND);
        cols.header(p, hy3, ["entrada", "salida", "respuestas"], dimc);
        let max_tokens = sm
            .models
            .iter()
            .map(|m| m.usage.input_total() + m.usage.output)
            .max()
            .unwrap_or(0);
        let row0 = hy3 + px(COLHEAD);
        for (i, m) in sm.models.iter().take(l.model_rows).enumerate() {
            let ry = row0 + i as f32 * px(ROW);
            let c = provider_color(theme, m.provider);
            p.circle(mx + pad + px(4.0), ry + px(ROW) / 2.0, px(4.0), c, 1.0);
            p.ui_text_in(
                mx + pad + px(16.0),
                ry,
                px(ROW),
                &m.name,
                theme.foreground,
                cols.name_w - px(16.0),
            );
            cols.bar(
                p,
                ry,
                m.usage.input_total() + m.usage.output,
                max_tokens,
                c,
                0.85,
            );
            cols.values(
                p,
                ry,
                [
                    compact(m.usage.input_total()),
                    compact(m.usage.output),
                    m.usage.calls.to_string(),
                ],
                theme.foreground,
            );
        }
        if l.more_models > 0 {
            let ry = row0 + l.model_rows as f32 * px(ROW);
            let rest = &sm.models[l.model_rows..];
            let mut u = super::Usage::default();
            rest.iter().for_each(|m| u.add(&m.usage));
            p.ui_text_in(
                mx + pad + px(16.0),
                ry,
                px(ROW),
                &format!("y {} más", rest.len()),
                dimc,
                cols.name_w,
            );
            cols.bar(
                p,
                ry,
                u.input_total() + u.output,
                max_tokens,
                theme.foreground,
                0.25,
            );
            cols.values(
                p,
                ry,
                [
                    compact(u.input_total()),
                    compact(u.output),
                    u.calls.to_string(),
                ],
                dimc,
            );
        }
        if nmodels == 0 {
            p.ui_text_in(
                mx + pad,
                row0,
                px(ROW),
                "Sin uso de modelos en este periodo",
                dimc,
                mw - 2.0 * pad,
            );
        }

        // --- Agentes
        let [ax, ay, aw, _] = l.agents;
        card(p, theme, l.agents, s);
        p.ui_text(
            ax + pad,
            ay + px(14.0),
            "Agentes",
            theme.foreground,
            aw / 2.0,
        );
        let (note, nc) = match self.agent {
            Some(a) => (
                format!("Solo {} · púlsalo otra vez para ver todos", a.name()),
                theme.accent,
            ),
            None => ("Pulsa uno para ver solo sus datos".to_string(), dimc),
        };
        text_right(
            p,
            ax + aw - pad,
            ay + px(16.0),
            &note,
            nc,
            aw * 0.62,
            UiVariant::Small,
        );
        let cols = Columns::new(ax, aw, s);
        let hy4 = ay + px(TITLE_BAND);
        cols.header(p, hy4, ["entrada", "salida", "tiempo"], dimc);
        let rows = self.agent_rows();
        let max_tokens = rows
            .iter()
            .map(|a| a.usage.input_total() + a.usage.output)
            .max()
            .unwrap_or(0);
        for ((agent, r), row) in l.agent_rows.iter().zip(rows.iter()) {
            let selected = self.agent == Some(*agent);
            let over = self.hover == Some(Hit::Agent(*agent));
            if selected {
                p.rounded(
                    r[0],
                    r[1] + px(2.0),
                    r[2],
                    r[3] - px(4.0),
                    theme.accent,
                    if over { 0.24 } else { 0.16 },
                    px(7.0),
                );
            } else if over {
                p.rounded(
                    r[0],
                    r[1] + px(2.0),
                    r[2],
                    r[3] - px(4.0),
                    theme.foreground,
                    0.06,
                    px(7.0),
                );
            }
            let faded = self.agent.is_some() && !selected;
            let text_c = if faded { dimc } else { theme.foreground };
            let c = provider_color(theme, agent.provider());
            let ry = r[1];
            p.circle(
                ax + pad + px(4.0),
                ry + px(ROW) / 2.0,
                px(4.0),
                c,
                if faded { 0.45 } else { 1.0 },
            );
            p.ui_text_in(
                ax + pad + px(16.0),
                ry,
                px(ROW),
                agent.name(),
                text_c,
                cols.name_w - px(16.0),
            );
            cols.bar(
                p,
                ry,
                row.usage.input_total() + row.usage.output,
                max_tokens,
                c,
                if faded { 0.35 } else { 0.85 },
            );
            let dash = |v: u64, f: fn(u64) -> String| if v == 0 { "—".to_string() } else { f(v) };
            cols.values(
                p,
                ry,
                [
                    dash(row.usage.input_total(), compact),
                    dash(row.usage.output, compact),
                    dash(row.work, hours),
                ],
                text_c,
            );
        }

        // --- Pie
        let fy = l.footer_y + px(16.0);
        p.ui_text_v(
            l.x,
            fy,
            "Calculado con las sesiones guardadas en este equipo; nada sale de él.",
            dimc,
            l.cw * 0.6,
            UiVariant::Small,
        );
        text_right(
            p,
            l.x + l.cw,
            fy,
            "← → periodo · Tab agente · Esc cerrar",
            dimc,
            l.cw * 0.4,
            UiVariant::Small,
        );

        // --- Detalles bajo el ratón (encima de todo)
        let tip: Vec<(String, Rgb)> = match self.hover {
            Some(Hit::Bar(i)) if i < sm.bins.len() => {
                let b = &sm.bins[i];
                let start = sm.bin_start(i);
                let date = if sm.bin_days == 1 {
                    format!(
                        "{} {}",
                        WEEKDAYS[super::weekday(start)],
                        short_date(start, sm.today)
                    )
                } else {
                    format!("Semana del {}", short_date(start, sm.today))
                };
                let inp = b.usage.input_total();
                let mut lines = vec![(date, theme.foreground)];
                if inp + b.usage.output + b.prompts + b.work == 0 {
                    lines.push(("Sin actividad".into(), dimc));
                } else {
                    let pct = (b.usage.cache_read * 100).checked_div(inp).unwrap_or(0);
                    lines.push((
                        format!("Entrada {} · {pct} % caché", compact(inp)),
                        theme.foreground,
                    ));
                    lines.push((format!("Salida {}", compact(b.usage.output)), theme.accent));
                    let peticiones = match b.prompts {
                        1 => "1 petición".to_string(),
                        n => format!("{n} peticiones"),
                    };
                    lines.push((format!("{peticiones} · {}", duration_long(b.work)), dimc));
                }
                lines
            }
            Some(Hit::Cell(d, hr)) => {
                let v = sm.heat[d][hr];
                vec![
                    (
                        format!("{}, de {hr} a {} h", capitalize(WEEKDAYS_LONG[d]), hr + 1),
                        theme.foreground,
                    ),
                    (
                        if v == 0 {
                            "Sin trabajo".into()
                        } else {
                            format!("{} trabajando en total", duration_long(v))
                        },
                        dimc,
                    ),
                ]
            }
            _ => Vec::new(),
        };
        if !tip.is_empty() {
            p.out.append(p.fg);
            tooltip(p, theme, &tip, self.mouse, w, h, s);
        }
    }
}

/// Una cifra: nombre, valor, nota, (ahora, periodo anterior) y su evolución por barras.
type Kpi = (&'static str, String, String, Option<(u64, u64)>, Vec<f64>);

/// Posiciones calculadas para un ancho de ventana y un desplazamiento.
struct Layout {
    x: f32,
    cw: f32,
    header_y: f32,
    close: (f32, f32, f32),
    segs: Vec<Rect>,
    kpis: Vec<Rect>,
    tokens: Rect,
    /// Zona de las barras dentro de `tokens`.
    plot: Rect,
    heat: Rect,
    /// Zona de las casillas dentro de `heat`.
    grid: Rect,
    models: Rect,
    agents: Rect,
    model_rows: usize,
    more_models: usize,
    agent_rows: Vec<(AgentKind, Rect)>,
    /// Sin datos: zona del mensaje (y nada más).
    message: Option<Rect>,
    footer_y: f32,
    max_scroll: f32,
}

/// Columnas de las tablas: nombre, barra y tres cifras alineadas a la derecha.
struct Columns {
    name_w: f32,
    bar: (f32, f32),
    rights: [f32; 3],
    s: f32,
}

impl Columns {
    fn new(x: f32, w: f32, s: f32) -> Self {
        let pad = (16.0 * s).round();
        // Caben "respuestas" y "11,6 mil M" con la fuente de interfaz monoespaciada.
        let col = (COL_W * s).round();
        let right = x + w - pad;
        let rights = [right - 2.0 * col, right - col, right];
        let numbers = rights[0] - col;
        let name_w = ((numbers - x - pad) * 0.48).max(110.0 * s).round();
        let bar0 = x + pad + name_w + (8.0 * s).round();
        let bar1 = numbers - (14.0 * s).round();
        Self {
            name_w,
            bar: (bar0, (bar1 - bar0).max(0.0)),
            rights,
            s,
        }
    }

    fn header(&self, p: &mut Painter, y: f32, labels: [&str; 3], c: Rgb) {
        let sh = p.ui_line_h(UiVariant::Small);
        let y = (y + ((COLHEAD * self.s) - sh) / 2.0).round();
        for (r, l) in self.rights.iter().zip(labels) {
            text_right(p, *r, y, l, c, COL_W * self.s, UiVariant::Small);
        }
    }

    fn bar(&self, p: &mut Painter, y: f32, v: u64, max: u64, c: Rgb, a: f32) {
        if self.bar.1 < 24.0 * self.s || max == 0 || v == 0 {
            return;
        }
        let h = (6.0 * self.s).round();
        let w = (self.bar.1 * v as f32 / max as f32).max(h);
        let by = (y + (ROW * self.s - h) / 2.0).round();
        p.rounded(self.bar.0, by, w, h, c, a, h / 2.0);
    }

    fn values(&self, p: &mut Painter, y: f32, vals: [String; 3], c: Rgb) {
        let lh = p.ui_line_h(UiVariant::Normal);
        let ty = (y + (ROW * self.s - lh) / 2.0).round();
        for (r, v) in self.rights.iter().zip(vals.iter()) {
            text_right(p, *r, ty, v, c, (COL_W - 6.0) * self.s, UiVariant::Normal);
        }
    }
}

/// Fondo de una tarjeta: un tono sobre el fondo, sin borde.
fn card(p: &mut Painter, theme: &Theme, r: Rect, s: f32) {
    p.rounded(
        r[0],
        r[1],
        r[2],
        r[3],
        theme.foreground,
        0.045,
        (12.0 * s).round(),
    );
}

fn text_right(p: &mut Painter, right: f32, y: f32, text: &str, c: Rgb, max_w: f32, v: UiVariant) {
    let w = p.ui_measure_v(text, v).min(max_w);
    p.ui_text_v((right - w).round(), y, text, c, max_w, v);
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| {
        f.to_uppercase().collect::<String>() + c.as_str()
    })
}

/// Color de cada empresa, sacado de la paleta del tema (así combina con cualquiera).
fn provider_color(theme: &Theme, p: Provider) -> Rgb {
    match p {
        // Entre el rojo y el amarillo del tema: un naranja.
        Provider::Anthropic => theme.palette[1].lerp(theme.palette[3], 0.55),
        Provider::OpenAi => theme.palette[6],
        Provider::Google => theme.palette[4],
        Provider::Other => theme.palette[5],
    }
}

/// Cambio respecto al periodo anterior: ("+12 %", sube).
fn delta(cur: u64, prev: u64) -> Option<(String, bool)> {
    if prev == 0 {
        return (cur > 0).then(|| ("nuevo".to_string(), true));
    }
    let ratio = cur as f64 / prev as f64;
    let pct = (ratio - 1.0) * 100.0;
    Some(if pct.abs() < 0.5 {
        ("igual".to_string(), true)
    } else if ratio >= 10.0 {
        (format!("×{}", ratio.round()), true)
    } else if pct > 0.0 {
        (format!("+{pct:.0} %"), true)
    } else {
        (format!("−{:.0} %", -pct), false)
    })
}

/// Línea con el área sombreada debajo (columnas de un píxel) y un punto en el último valor.
fn sparkline(p: &mut Painter, theme: &Theme, r: Rect, values: &[f64], s: f32) {
    let [x, y, w, h] = r;
    let base = y + h;
    let max = values.iter().copied().fold(0.0, f64::max);
    if values.is_empty() || max <= 0.0 {
        p.rect(
            x,
            base - s.round().max(1.0),
            w,
            s.round().max(1.0),
            theme.foreground,
            0.10,
        );
        return;
    }
    let n = values.len();
    let at = |i: usize| -> (f32, f32) {
        let px = if n > 1 {
            x + w * i as f32 / (n - 1) as f32
        } else {
            x + w
        };
        (px, base - (values[i] / max) as f32 * h)
    };
    // Área: una columna cada 2 píxeles con la altura de la línea en ese punto.
    let step = 2.0_f32.max(s.round());
    let mut cx = x;
    while cx < x + w {
        let t = if n > 1 {
            (cx - x) / w * (n - 1) as f32
        } else {
            0.0
        };
        let i = (t.floor() as usize).min(n - 1);
        let j = (i + 1).min(n - 1);
        let f = t - i as f32;
        let v = values[i] as f32 * (1.0 - f) + values[j] as f32 * f;
        let ch = v / max as f32 * h;
        if ch > 0.0 {
            p.rect(cx, base - ch, step, ch, theme.accent, 0.10);
        }
        cx += step;
    }
    let t = 1.4 * s;
    for i in 1..n {
        let (x0, y0) = at(i - 1);
        let (x1, y1) = at(i);
        p.line(x0, y0, x1, y1, t, theme.accent, 0.9);
    }
    let (lx, ly) = at(n - 1);
    p.circle(lx, ly, 2.6 * s, theme.accent, 1.0);
}

/// Ventanita con detalles junto al ratón, siempre dentro de la ventana.
fn tooltip(
    p: &mut Painter,
    theme: &Theme,
    lines: &[(String, Rgb)],
    at: (f32, f32),
    w: f32,
    h: f32,
    s: f32,
) {
    let pad = (10.0 * s).round();
    let lh = p.ui_line_h(UiVariant::Normal);
    let sh = p.ui_line_h(UiVariant::Small);
    let heights: Vec<f32> = (0..lines.len())
        .map(|i| if i == 0 { lh } else { sh + 2.0 * s })
        .collect();
    let tw = lines
        .iter()
        .enumerate()
        .map(|(i, (t, _))| {
            p.ui_measure_v(
                t,
                if i == 0 {
                    UiVariant::Normal
                } else {
                    UiVariant::Small
                },
            )
        })
        .fold(0.0, f32::max);
    let bw = (tw + 2.0 * pad).ceil();
    let bh = (heights.iter().sum::<f32>() + 2.0 * pad).ceil();
    let mut x = at.0 + 14.0 * s;
    let mut y = at.1 + 16.0 * s;
    if x + bw > w - 4.0 * s {
        x = at.0 - 14.0 * s - bw;
    }
    if y + bh > h - 4.0 * s {
        y = at.1 - 12.0 * s - bh;
    }
    let (x, y) = (x.round().max(0.0), y.round().max(0.0));
    p.card(
        x,
        y,
        bw,
        bh,
        surface(theme, 0.10),
        theme.foreground,
        (8.0 * s).round(),
        s,
    );
    let mut ty = y + pad;
    for (i, (t, c)) in lines.iter().enumerate() {
        let v = if i == 0 {
            UiVariant::Normal
        } else {
            UiVariant::Small
        };
        p.ui_text_v(x + pad, ty, t, *c, tw + 1.0, v);
        ty += heights[i];
    }
}

#[cfg(test)]
mod tests {
    use super::super::{FileStats, Usage};
    use super::*;

    fn data() -> Arc<Vec<FileStats>> {
        let hour = (super::super::today() * 24 + 12) as u32;
        let file = |agent, key: &str| FileStats {
            key: key.into(),
            agent,
            main: true,
            models: vec!["claude-opus-5-5".into()],
            usage: vec![(
                hour,
                0,
                Usage {
                    input: 10,
                    output: 5,
                    calls: 1,
                    ..Default::default()
                },
            )],
            activity: vec![(hour, 1, 600)],
            runs: vec![(hour * 60, 10)],
            ..Default::default()
        };
        Arc::new(vec![
            file(AgentKind::Claude, "a"),
            file(AgentKind::Codex, "b"),
        ])
    }

    #[test]
    fn periodo_agente_y_desplazamiento() {
        let mut v = StatsView::new();
        let (w, h, top, s) = (1400.0, 900.0, 40.0, 1.0);
        // Sin datos todavía: solo cabecera.
        assert_eq!(v.hit(10.0, 5.0, w, h, top, s), Hit::Nothing);
        v.on_msg(StatsMsg::Data(data()));
        let l = v.layout(w, h, top, s);
        let seg = l.segs[0];
        assert_eq!(
            v.hit(seg[0] + 5.0, seg[1] + 5.0, w, h, top, s),
            Hit::Period(Period::Week)
        );
        let (cx, cy, _) = l.close;
        assert_eq!(v.hit(cx, cy, w, h, top, s), Hit::Close);
        // Pulsar un agente filtra; pulsarlo otra vez lo quita.
        let (a, r) = l.agent_rows[0];
        assert_eq!(v.hit(r[0] + 5.0, r[1] + 5.0, w, h, top, s), Hit::Agent(a));
        v.toggle_agent(a);
        assert_eq!(v.summary.as_ref().unwrap().agent, Some(a));
        assert_eq!(v.summary.as_ref().unwrap().total.usage.input_total(), 10);
        v.toggle_agent(a);
        assert_eq!(v.summary.as_ref().unwrap().total.usage.input_total(), 20);
        // Tab recorre: todos → uno → otro → todos.
        v.cycle_agent(false);
        assert!(v.agent.is_some());
        v.cycle_agent(false);
        v.cycle_agent(false);
        assert_eq!(v.agent, None);
        // La barra de hoy es la última.
        let p = l.plot;
        assert_eq!(
            v.hit(p[0] + p[2] - 1.0, p[1] + 5.0, w, h, top, s),
            Hit::Bar(29)
        );
        // En una ventana baja se puede desplazar hasta ver el pie, y no más.
        v.scroll_by(10_000.0, w, 500.0, top, s);
        let max = v.scroll;
        assert!(max > 0.0);
        v.scroll_by(100.0, w, 500.0, top, s);
        assert_eq!(v.scroll, max);
        v.step_period(-1);
        assert_eq!(v.period, Period::Week);
        v.step_period(-1);
        assert_eq!(v.period, Period::Week);
    }

    #[test]
    fn cambios_respecto_al_periodo_anterior() {
        assert_eq!(delta(0, 0), None);
        assert_eq!(delta(5, 0), Some(("nuevo".into(), true)));
        assert_eq!(delta(150, 100), Some(("+50 %".into(), true)));
        assert_eq!(delta(50, 100), Some(("−50 %".into(), false)));
        assert_eq!(delta(1200, 100), Some(("×12".into(), true)));
        assert_eq!(delta(100, 100), Some(("igual".into(), true)));
    }
}
