//! Panel "Cambios" (Ctrl+Shift+G, el botón de la barra o el indicador del prompt): los cambios de
//! git sin confirmar del repositorio de la pestaña activa, a la derecha del terminal, como la
//! revisión de código de Warp.
//!
//! Una fila por archivo (insignia M/A/D/U, nombre, carpeta y líneas +/−); al pulsarla se despliega
//! su diff debajo. Un clic en una línea del diff abre el archivo en el editor en esa línea. Solo
//! lee: no prepara ni descarta nada. Los datos los trae `git.rs`.

use crate::config::{Rgb, Theme};
use crate::git::{FileDiff, Kind, LineKind, Status};
use crate::ui::{cross, dim};
use crate::view::Painter;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// Lo que identifica la versión de un archivo cuyo diff se leyó: si cambia, se lee otra vez.
type Version = (Option<SystemTime>, u32, u32);

/// Cada cuánto se vuelve a leer el estado con el panel abierto (como poco; en repositorios lentos,
/// 20 veces lo que tardó la última lectura).
pub const POLL: Duration = Duration::from_secs(2);
/// Al abrirlo, se despliegan todos los archivos si son pocos y con pocos cambios.
const AUTO_FILES: usize = 6;
const AUTO_LINES: u32 = 400;
/// Archivos que "Desplegar todo" despliega como mucho.
const MAX_OPEN: usize = 60;
/// Rapidez de la animación del desplazamiento (1/s), como en el editor de aspecto.
const SCROLL_SPEED: f32 = 22.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    /// Borde izquierdo: se arrastra para cambiar el ancho.
    Divider,
    Close,
    /// "Desplegar todo" / "Plegar todo".
    ToggleAll,
    /// Fila de un archivo (índice en `Status::files`).
    File(usize),
    /// Botón "Abrir" de un archivo.
    Open(usize),
    /// Línea `j` del diff del archivo `i`.
    Line(usize, usize),
    Panel,
}

pub struct ChangesPanel {
    pub open: bool,
    /// Parte del ancho disponible que ocupa (su borde izquierdo se arrastra).
    pub ratio: f32,
    /// Repositorio que se muestra (el de la pestaña activa).
    pub root: Option<PathBuf>,
    expanded: HashSet<String>,
    diffs: HashMap<String, Result<FileDiff, String>>,
    /// Versión de cada archivo desplegado cuando se pidió su diff.
    asked: HashMap<String, Version>,
    pub hover: Option<Hit>,
    pub dragging: bool,
    /// Repositorio recién elegido: al llegar su estado se despliega todo si es poco.
    fresh: bool,
    /// Próxima lectura del estado (`None` = en cuanto se pueda).
    pub poll_at: Option<Instant>,
    scroll: f32,
    target: f32,
    last: Option<Instant>,
    moving: bool,
    /// Zonas del último dibujo, para el ratón (lo que se ve es lo que se pulsa).
    rects: Vec<(Hit, [f32; 4])>,
    area: [f32; 4],
}

impl Default for ChangesPanel {
    fn default() -> Self {
        Self {
            open: false,
            ratio: 0.42,
            root: None,
            expanded: HashSet::new(),
            diffs: HashMap::new(),
            asked: HashMap::new(),
            hover: None,
            dragging: false,
            fresh: true,
            poll_at: None,
            scroll: 0.0,
            target: 0.0,
            last: None,
            moving: false,
            rects: Vec::new(),
            area: [0.0; 4],
        }
    }
}

fn inside(r: &[f32; 4], x: f32, y: f32) -> bool {
    x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3]
}

/// Color de la insignia de cada tipo de cambio.
fn kind_color(kind: Kind, theme: &Theme) -> Rgb {
    match kind {
        Kind::Modified => theme.palette[3],
        Kind::Added | Kind::Untracked => theme.palette[2],
        Kind::Deleted | Kind::Conflict => theme.palette[1],
        Kind::Renamed => theme.palette[4],
    }
}

impl ChangesPanel {
    /// Ancho del panel con `avail` píxeles entre las columnas de los lados.
    pub fn width(&self, avail: f32, s: f32) -> f32 {
        let min = (280.0 * s).min(avail * 0.5);
        let max = (avail - 240.0 * s).max(min);
        (self.ratio * avail).clamp(min, max).round()
    }

    /// Arrastre del borde izquierdo hasta `x`; `right` es el borde derecho del panel y `avail`,
    /// el ancho que se reparte con los terminales.
    pub fn drag_to(&mut self, x: f32, right: f32, avail: f32) {
        self.ratio = ((right - x) / avail.max(1.0)).clamp(0.15, 0.85);
    }

    /// Cambia el repositorio que se muestra (empieza plegado y arriba).
    pub fn set_root(&mut self, root: Option<PathBuf>) {
        if root == self.root {
            return;
        }
        self.root = root;
        self.expanded.clear();
        self.diffs.clear();
        self.asked.clear();
        self.hover = None;
        self.fresh = true;
        self.poll_at = None;
        self.scroll = 0.0;
        self.target = 0.0;
    }

    /// Llegó el estado del repositorio que se muestra: quita lo que ya no está y devuelve los
    /// diffs que hay que pedir (los desplegados que han cambiado desde que se leyeron).
    pub fn on_status(&mut self, status: &Status) -> Vec<(String, Kind)> {
        let present: HashSet<&str> = status.files.iter().map(|f| f.path.as_str()).collect();
        self.expanded.retain(|p| present.contains(p.as_str()));
        self.diffs.retain(|p, _| present.contains(p.as_str()));
        self.asked.retain(|p, _| present.contains(p.as_str()));
        if std::mem::take(&mut self.fresh)
            && status.files.len() <= AUTO_FILES
            && status.added + status.removed <= AUTO_LINES
        {
            self.expanded = status.files.iter().map(|f| f.path.clone()).collect();
        }
        let mut wanted = Vec::new();
        for f in status
            .files
            .iter()
            .filter(|f| self.expanded.contains(&f.path))
        {
            let version = (f.modified, f.added, f.removed);
            if self.asked.insert(f.path.clone(), version) != Some(version) {
                wanted.push((f.path.clone(), f.kind));
            }
        }
        wanted
    }

    /// Llegó un diff. Devuelve `true` si cambió lo que se ve.
    pub fn on_diff(
        &mut self,
        root: &std::path::Path,
        path: &str,
        diff: Result<FileDiff, String>,
    ) -> bool {
        if self.root.as_deref() != Some(root) || !self.expanded.contains(path) {
            return false;
        }
        if self.diffs.get(path) == Some(&diff) {
            return false;
        }
        self.diffs.insert(path.to_string(), diff);
        true
    }

    /// Despliega o pliega el archivo `i`. Devuelve el diff que hay que pedir.
    pub fn toggle(&mut self, i: usize, status: &Status) -> Option<(String, Kind)> {
        let f = status.files.get(i)?;
        if self.expanded.remove(&f.path) {
            self.asked.remove(&f.path);
            return None;
        }
        self.expanded.insert(f.path.clone());
        self.asked
            .insert(f.path.clone(), (f.modified, f.added, f.removed));
        Some((f.path.clone(), f.kind))
    }

    /// "Desplegar todo" o, si ya está todo desplegado, "Plegar todo".
    pub fn toggle_all(&mut self, status: &Status) -> Vec<(String, Kind)> {
        if self.all_open(status) {
            self.expanded.clear();
            self.asked.clear();
            return Vec::new();
        }
        let mut wanted = Vec::new();
        for f in status.files.iter().take(MAX_OPEN) {
            if self.expanded.insert(f.path.clone()) {
                self.asked
                    .insert(f.path.clone(), (f.modified, f.added, f.removed));
                wanted.push((f.path.clone(), f.kind));
            }
        }
        wanted
    }

    fn all_open(&self, status: &Status) -> bool {
        status
            .files
            .iter()
            .take(MAX_OPEN)
            .all(|f| self.expanded.contains(&f.path))
    }

    pub fn diff(&self, path: &str) -> Option<&FileDiff> {
        self.diffs.get(path)?.as_ref().ok()
    }

    pub fn wheel(&mut self, px: f32) {
        // Hacia arriba (`px` > 0) la lista baja.
        self.target -= px;
    }

    /// ¿Se está moviendo la lista? (hace falta otro fotograma).
    pub fn animating(&self) -> bool {
        self.open && (self.target - self.scroll).abs() >= 0.5
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        inside(&self.area, x, y)
    }

    /// Zona bajo el ratón, según el último dibujo.
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.rects
            .iter()
            .rev()
            .find(|(_, r)| inside(r, x, y))
            .map(|(h, _)| *h)
            .or_else(|| self.contains(x, y).then_some(Hit::Panel))
    }

    /// `rect`: (x, y, ancho, alto). `bottom_free`: el panel llega al borde de abajo de la ventana
    /// (lo que asoma por abajo no tapa nada).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        p: &mut Painter,
        theme: &Theme,
        status: Option<&Result<Status, String>>,
        rect: (f32, f32, f32, f32),
        s: f32,
        bottom_free: bool,
    ) {
        let (x, y, w, h) = rect;
        self.rects.clear();
        self.area = [x, y, w, h];
        let lh = p.ui_metrics().height;
        let m = p.metrics();
        let (cw, ch) = (m.width, m.height);
        let hair = s.round().max(1.0);
        let pad = (14.0 * s).round();
        let header_h = (44.0 * s).round();
        let sum_h = (32.0 * s).round();
        let row_h = (lh + 16.0 * s).round();
        let dpad = (6.0 * s).round();
        let fg = theme.foreground;
        let dimc = dim(theme);
        let (green, red) = (theme.palette[2], theme.palette[1]);
        p.rect(x, y, w, h, theme.tab_bar, 1.0);

        let body_y = y + header_h + sum_h;
        let body_h = (h - header_h - sum_h).max(0.0);
        let bottom = if bottom_free { f32::INFINITY } else { y + h };
        let ok = status.and_then(|r| r.as_ref().ok());
        let files = ok.map_or(&[][..], |st| &st.files[..]);

        // Dónde va cada archivo (en coordenadas de la lista) y el alto de su diff desplegado.
        let mut pos = Vec::with_capacity(files.len());
        let mut cy = 0.0;
        for f in files {
            let bh = if self.expanded.contains(&f.path) {
                let lines = match self.diffs.get(&f.path) {
                    Some(Ok(d)) if !d.binary && !d.lines.is_empty() => {
                        d.lines.len() + usize::from(d.cut)
                    }
                    _ => 1,
                };
                lines as f32 * ch + 2.0 * dpad
            } else {
                0.0
            };
            pos.push((cy, bh));
            cy += row_h + bh;
        }
        let more = ok.map_or(0, |st| st.more);
        let total = cy + if more > 0 { row_h } else { 0.0 } + 10.0 * s;
        let max = (total - body_h).max(0.0);

        // Desplazamiento suave hacia `target`.
        self.target = self.target.clamp(0.0, max);
        let now = Instant::now();
        let dt = match self.last.filter(|_| self.moving) {
            Some(t) => now.saturating_duration_since(t).as_secs_f32().min(0.1),
            None => 1.0 / 60.0,
        };
        self.last = Some(now);
        self.scroll += (self.target - self.scroll) * (1.0 - (-dt * SCROLL_SPEED).exp());
        if (self.target - self.scroll).abs() < 0.5 {
            self.scroll = self.target;
        }
        self.scroll = self.scroll.clamp(0.0, max);
        self.moving = self.animating();
        let off = body_y - self.scroll.round();
        let shown =
            |top: f32, hh: f32| top + hh > body_y && top < y + h && top + hh <= bottom + 0.5;
        // Solo cuenta para el ratón lo que se ve de la lista.
        let mut list_rects: Vec<(Hit, [f32; 4])> = Vec::new();
        let mut push = |hit: Hit, r: [f32; 4]| {
            let top = r[1].max(body_y);
            let end = (r[1] + r[3]).min(y + h);
            if end > top {
                list_rects.push((hit, [r[0], top, r[2], end - top]));
            }
        };

        for (i, f) in files.iter().enumerate() {
            let (ly, bh) = pos[i];
            let ry = off + ly;
            if ry > y + h {
                break;
            }
            if ry + row_h + bh < body_y {
                continue;
            }
            let open = self.expanded.contains(&f.path);
            let deleted = f.kind == Kind::Deleted;
            if shown(ry, row_h) {
                let hovered = matches!(self.hover, Some(Hit::File(j) | Hit::Open(j)) if j == i);
                if hovered {
                    p.rect(x + hair, ry, w - hair, row_h, fg, 0.05);
                }
                if i > 0 {
                    p.rect(x + hair, ry, w - hair, hair, fg, 0.06);
                }
                push(Hit::File(i), [x + hair, ry, w - hair, row_h]);
                let mid = ry + row_h / 2.0;
                chevron(p, x + pad + 3.0 * s, mid, 3.5 * s, open, dimc, s);
                // Insignia con la letra del cambio.
                let kc = kind_color(f.kind, theme);
                let bsz = (lh * 0.95).round();
                let bx = x + pad + 14.0 * s;
                let by = (ry + (row_h - bsz) / 2.0).round();
                p.rounded(bx, by, bsz, bsz, kc, 0.16, 4.0 * s);
                let letter = f.kind.letter();
                let lw = p.ui_measure(letter);
                p.ui_text_in(bx + (bsz - lw) / 2.0, by, bsz, letter, kc, bsz);
                // Líneas añadidas y quitadas, a la derecha.
                let mut rx = x + w - pad;
                if f.binary {
                    let t = "binario";
                    let tw = p.ui_measure(t);
                    rx -= tw;
                    p.ui_text_in(rx, ry, row_h, t, dimc, tw + 1.0);
                } else {
                    for (n, sign, c) in [(f.removed, "−", red), (f.added, "+", green)] {
                        if n > 0 {
                            let t = format!("{sign}{n}");
                            let tw = p.ui_measure(&t);
                            rx -= tw;
                            p.ui_text_in(rx, ry, row_h, &t, c, tw + 1.0);
                            rx -= 7.0 * s;
                        }
                    }
                }
                // "Abrir" con el ratón encima (un archivo borrado no se puede abrir).
                if hovered && !deleted {
                    let t = "Abrir";
                    let tw = p.ui_measure(t);
                    let (bw, bh2) = ((tw + 18.0 * s).round(), (lh + 8.0 * s).round());
                    let bx0 = (rx - 4.0 * s - bw).round();
                    let by0 = (ry + (row_h - bh2) / 2.0).round();
                    let over = self.hover == Some(Hit::Open(i));
                    p.rounded(
                        bx0,
                        by0,
                        bw,
                        bh2,
                        fg,
                        if over { 0.16 } else { 0.08 },
                        5.0 * s,
                    );
                    p.ui_text_in(bx0 + 9.0 * s, by0, bh2, t, fg, tw + 1.0);
                    push(Hit::Open(i), [bx0, by0, bw, bh2]);
                    rx = bx0;
                }
                // Nombre y, más tenue, su carpeta.
                let tx = bx + bsz + 9.0 * s;
                let (folder, name) = match f.path.rfind('/') {
                    Some(k) => (&f.path[..k], &f.path[k + 1..]),
                    None => ("", f.path.as_str()),
                };
                let avail = (rx - 10.0 * s - tx).max(0.0);
                let nc = if deleted { dimc } else { fg };
                let nw = p.ui_text_in(tx, ry, row_h, name, nc, avail);
                if deleted {
                    p.rect(tx, (mid).round(), nw, hair, dimc, 1.0);
                }
                if !folder.is_empty() && avail - nw > 30.0 * s {
                    let fx = tx + nw + 8.0 * s;
                    p.ui_text_in(fx, ry, row_h, folder, dimc, avail - nw - 8.0 * s);
                }
            }
            if bh <= 0.0 {
                continue;
            }
            // Diff desplegado, sobre el fondo del terminal: solo lo que se ve de la lista. Desde
            // `by`, con un diff largo desplazado, el fondo llegaba por encima de la ventana y
            // tapaba los botones de la barra (la cabecera del panel solo tapa el panel).
            let by = ry + row_h;
            let vis_top = by.max(body_y);
            let vis_end = (by + bh).min(bottom).min(y + h);
            if vis_end > vis_top {
                p.rect(
                    x + hair,
                    vis_top,
                    w - hair,
                    vis_end - vis_top,
                    theme.background,
                    1.0,
                );
            }
            let msg = |p: &mut Painter, text: &str| {
                if shown(by + dpad, ch) {
                    p.ui_text_in(x + pad, by + dpad, ch, text, dimc, w - 2.0 * pad);
                }
            };
            let d = match self.diffs.get(&f.path) {
                None => {
                    msg(p, "Leyendo…");
                    continue;
                }
                Some(Err(e)) => {
                    msg(p, e);
                    continue;
                }
                Some(Ok(d)) if d.binary => {
                    msg(p, "Archivo binario: no se muestra su contenido");
                    continue;
                }
                Some(Ok(d)) if d.lines.is_empty() => {
                    msg(p, "Sin cambios en el texto (permisos o tipo de archivo)");
                    continue;
                }
                Some(Ok(d)) => d,
            };
            let top_line = by + dpad;
            let biggest = d.lines.iter().map(|l| l.old.max(l.new)).max().unwrap_or(1);
            let digits = biggest.to_string().len().max(2);
            let lx = x + (10.0 * s).round();
            let num_w = digits as f32 * cw;
            let sign_x = lx + num_w + 0.7 * cw;
            let text_x = sign_x + 1.5 * cw;
            let text_w = (x + w - 8.0 * s - text_x).max(0.0);
            let first = ((body_y - top_line) / ch).floor().max(0.0) as usize;
            let rows = d.lines.len() + usize::from(d.cut);
            let last = (((y + h - top_line) / ch).ceil().max(0.0) as usize).min(rows);
            for j in first..last {
                let ly = top_line + j as f32 * ch;
                if !shown(ly, ch) {
                    continue;
                }
                let Some(l) = d.lines.get(j) else {
                    p.ui_text_in(
                        lx,
                        ly,
                        ch,
                        "… diff recortado: ábrelo para verlo entero",
                        dimc,
                        w,
                    );
                    continue;
                };
                let (sign, tint) = match l.kind {
                    LineKind::Hunk => {
                        p.rect(x + hair, ly, w - hair, ch, fg, 0.045);
                        p.text(lx, ly, &l.text, dimc, x + w - 8.0 * s - lx);
                        continue;
                    }
                    LineKind::Added => ("+", Some(green)),
                    LineKind::Removed => ("-", Some(red)),
                    LineKind::Context => (" ", None),
                };
                if let Some(c) = tint {
                    p.rect(x + hair, ly, w - hair, ch, c, 0.13);
                }
                if !deleted {
                    if self.hover == Some(Hit::Line(i, j)) {
                        p.rect(x + hair, ly, w - hair, ch, fg, 0.08);
                    }
                    push(Hit::Line(i, j), [x + hair, ly, w - hair, ch]);
                }
                let num = if l.kind == LineKind::Removed {
                    l.old
                } else {
                    l.new
                };
                let ns = num.to_string();
                let nx = lx + num_w - ns.len() as f32 * cw;
                p.text(nx, ly, &ns, dimc, ns.len() as f32 * cw + 1.0);
                if let Some(c) = tint {
                    p.text(sign_x, ly, sign, c, cw + 1.0);
                }
                let tc = if tint.is_some() {
                    fg
                } else {
                    fg.lerp(theme.background, 0.25)
                };
                p.text(text_x, ly, &l.text, tc, text_w);
            }
        }
        if more > 0 {
            let ry = off + cy;
            if shown(ry, row_h) {
                let t = format!("… y {more} archivos más");
                p.ui_text_in(x + pad, ry, row_h, &t, dimc, w - 2.0 * pad);
            }
        }

        // Estado sin lista: sin repositorio, leyendo, error o sin cambios.
        if files.is_empty() {
            let (title, hint): (&str, String) = match (&self.root, status) {
                (None, _) => (
                    "Sin repositorio de git",
                    "La carpeta de esta pestaña no está en un repositorio.".into(),
                ),
                (Some(_), None) => ("Leyendo los cambios…", String::new()),
                (Some(_), Some(Err(e))) => ("No se pudieron leer los cambios", e.clone()),
                (Some(_), Some(Ok(st))) => (
                    "No hay cambios",
                    match &st.branch {
                        Some(b) => format!("Todo está confirmado en {b}."),
                        None => "Todo está confirmado.".into(),
                    },
                ),
            };
            let ty = body_y + 28.0 * s;
            let tw = p.ui_measure(title).min(w - 2.0 * pad);
            p.ui_text(x + (w - tw) / 2.0, ty, title, fg, w - 2.0 * pad);
            if !hint.is_empty() {
                let hw = p.ui_measure(&hint).min(w - 2.0 * pad);
                p.ui_text(
                    x + (w - hw) / 2.0,
                    ty + lh + 8.0 * s,
                    &hint,
                    dimc,
                    w - 2.0 * pad,
                );
            }
        }

        // Barra de desplazamiento.
        if max > 0.0 && body_h > 0.0 {
            let th = (body_h * body_h / total).max(24.0 * s).min(body_h);
            let ty = body_y + (body_h - th) * (self.scroll / max);
            p.rounded(x + w - 6.0 * s, ty, 3.0 * s, th, fg, 0.18, 1.5 * s);
        }

        // Cabecera y resumen, encima: tapan lo que se desplazó hacia arriba.
        p.out.append(p.fg);
        p.rect(x, y, w, header_h + sum_h, theme.tab_bar, 1.0);
        p.rect(x, y, hair, h, fg, 0.10);
        let (iw, ih) = ((11.0 * s).round(), (14.0 * s).round());
        crate::ui::branch(
            p,
            x + pad,
            (y + (header_h - ih) / 2.0).round(),
            iw,
            ih,
            (1.3 * s).max(1.0),
            theme.palette[5],
        );
        let (cx, ccy, r) = (x + w - 22.0 * s, y + header_h / 2.0, 12.0 * s);
        let tx = x + pad + 22.0 * s;
        let tw = p.ui_text_in(tx, y, header_h, "Cambios", fg, w * 0.5);
        if let Some(b) = ok.and_then(|st| st.branch.as_deref()) {
            let bx = tx + tw + 10.0 * s;
            p.ui_text_in(bx, y, header_h, b, dimc, (cx - r - 8.0 * s - bx).max(0.0));
        }
        if self.hover == Some(Hit::Close) {
            p.circle(cx, ccy, r, fg, 0.12);
        }
        cross(p, cx, ccy, 3.5 * s, 1.4 * s, dimc);
        p.rect(x + hair, y + header_h - hair, w - hair, hair, fg, 0.08);

        let sy = y + header_h;
        if let Some(st) = ok.filter(|st| !st.files.is_empty()) {
            let n = st.files.len() + st.more;
            let t = if n == 1 {
                "1 archivo".to_string()
            } else {
                format!("{n} archivos")
            };
            let mut sx = x + pad;
            sx += p.ui_text_in(sx, sy, sum_h, &t, dimc, w * 0.5) + 10.0 * s;
            for (n, sign, c) in [(st.added, "+", green), (st.removed, "−", red)] {
                if n > 0 {
                    let t = format!("{sign}{n}");
                    sx += p.ui_text_in(sx, sy, sum_h, &t, c, w * 0.3) + 7.0 * s;
                }
            }
            let label = if self.all_open(st) {
                "Plegar todo"
            } else {
                "Desplegar todo"
            };
            let lw = p.ui_measure(label);
            let (bw, bh) = ((lw + 16.0 * s).round(), (lh + 8.0 * s).round());
            let bx = (x + w - pad - bw + 8.0 * s).round();
            let by = (sy + (sum_h - bh) / 2.0).round();
            if bx > sx {
                let over = self.hover == Some(Hit::ToggleAll);
                if over {
                    p.rounded(bx, by, bw, bh, fg, 0.08, 5.0 * s);
                }
                p.ui_text_in(bx + 8.0 * s, by, bh, label, theme.accent, lw + 1.0);
                self.rects.push((Hit::ToggleAll, [bx, by, bw, bh]));
            }
        }
        p.rect(x + hair, sy + sum_h - hair, w - hair, hair, fg, 0.08);
        self.rects.extend(list_rects);
        self.rects
            .push((Hit::Close, [cx - r, ccy - r, 2.0 * r, 2.0 * r]));
        // El borde, para arrastrarlo (por delante de todo).
        self.rects
            .push((Hit::Divider, [x - 4.0 * s, y, 8.0 * s, h]));
    }
}

/// Flecha de desplegar: ▸ plegado, ▾ desplegado.
fn chevron(p: &mut Painter, cx: f32, cy: f32, k: f32, open: bool, c: Rgb, s: f32) {
    let t = (1.4 * s).max(1.0);
    if open {
        p.line(cx - k, cy - k * 0.5, cx, cy + k * 0.5, t, c, 1.0);
        p.line(cx, cy + k * 0.5, cx + k, cy - k * 0.5, t, c, 1.0);
    } else {
        p.line(cx - k * 0.5, cy - k, cx + k * 0.5, cy, t, c, 1.0);
        p.line(cx + k * 0.5, cy, cx - k * 0.5, cy + k, t, c, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::FileChange;

    fn status(n: usize, lines: u32) -> Status {
        let files: Vec<FileChange> = (0..n)
            .map(|i| FileChange {
                path: format!("f{i}.rs"),
                kind: Kind::Modified,
                added: lines,
                removed: 0,
                binary: false,
                modified: None,
            })
            .collect();
        Status {
            branch: Some("main".into()),
            added: lines * n as u32,
            removed: 0,
            files,
            more: 0,
        }
    }

    #[test]
    fn desplegar_y_plegar() {
        let mut c = ChangesPanel::default();
        c.set_root(Some("/r".into()));
        // Pocos cambios: al llegar el estado se despliega todo y se piden sus diffs.
        let st = status(2, 10);
        assert_eq!(c.on_status(&st).len(), 2);
        assert!(c.all_open(&st));
        // Lo desplegado que no ha cambiado no se vuelve a pedir; lo que cambia, sí.
        assert!(c.on_status(&st).is_empty());
        let mut touched = st.clone();
        touched.files[1].modified = Some(SystemTime::UNIX_EPOCH);
        assert_eq!(
            c.on_status(&touched),
            [("f1.rs".to_string(), Kind::Modified)]
        );
        // Ya no es nuevo: aunque cambie, no se vuelve a desplegar lo plegado.
        assert_eq!(c.toggle(0, &st), None);
        assert!(c.on_status(&touched).is_empty());
        assert_eq!(c.toggle_all(&st).len(), 1);
        assert!(c.toggle_all(&st).is_empty());
        assert!(!c.all_open(&st));
        // Muchos cambios: empieza plegado.
        c.set_root(Some("/otro".into()));
        assert!(c.on_status(&status(20, 10)).is_empty());
        // Un archivo que ya no está deja de estar desplegado.
        let big = status(3, 1000);
        c.toggle(2, &big);
        assert_eq!(c.on_status(&status(2, 1000)).len(), 0);
    }

    #[test]
    fn ancho_y_arrastre() {
        let mut c = ChangesPanel::default();
        let w = c.width(1000.0, 1.0);
        assert_eq!(w, 420.0);
        c.drag_to(300.0, 1000.0, 1000.0);
        assert_eq!(c.width(1000.0, 1.0), 700.0);
        // El terminal se queda con 240 px como poco.
        c.drag_to(0.0, 1000.0, 1000.0);
        assert_eq!(c.width(1000.0, 1.0), 760.0);
    }
}
