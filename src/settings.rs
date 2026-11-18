//! Panel de Ajustes: cambios en vivo que se guardan solos en config.toml
//! (respetando los comentarios y el resto del archivo).

use crate::config::{Config, CursorStyle, Rgb, Theme};
use crate::ui::{cross, dim, surface};
use crate::view::Painter;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Theme,
    FontFamily,
    FontSize,
    LineHeight,
    Padding,
    Opacity,
    CursorStyle,
    CursorBlink,
    CopyOnSelect,
    Scrollback,
    Blocks,
    ShellIntegration,
    Decorations,
    EditFile,
    CheckUpdates,
    Version,
}

/// Qué tiene que hacer la app tras un cambio.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change {
    None,
    Theme,
    Fonts,
    FontSize,
    LineHeight,
    Layout,
    TermOptions,
    Redraw,
    Restart,
    OpenFile,
    Updates,
}

const SECTIONS: &[(&str, &[(Field, &str)])] = &[
    (
        "APARIENCIA",
        &[
            (Field::Theme, "Tema"),
            (Field::FontFamily, "Fuente"),
            (Field::FontSize, "Tamaño de fuente"),
            (Field::LineHeight, "Interlineado"),
            (Field::Padding, "Margen interior"),
            (Field::Opacity, "Opacidad"),
        ],
    ),
    (
        "TERMINAL",
        &[
            (Field::CursorStyle, "Cursor"),
            (Field::CursorBlink, "Cursor parpadeante"),
            (Field::CopyOnSelect, "Copiar al seleccionar"),
            (Field::Scrollback, "Historial"),
        ],
    ),
    (
        "BLOQUES",
        &[(Field::Blocks, "Bloques de comandos"), (Field::ShellIntegration, "Integración con bash")],
    ),
    (
        "VENTANA",
        &[(Field::Decorations, "Barra de título del sistema"), (Field::EditFile, "Editar config.toml")],
    ),
    (
        "ACTUALIZACIONES",
        &[(Field::CheckUpdates, "Buscar actualizaciones"), (Field::Version, "Versión instalada")],
    ),
];

enum Item {
    Section(&'static str),
    Row,
}

fn items() -> Vec<Item> {
    let mut v = Vec::new();
    for (title, rows) in SECTIONS {
        v.push(Item::Section(title));
        v.extend(rows.iter().map(|_| Item::Row));
    }
    v
}

fn fields() -> Vec<Field> {
    SECTIONS.iter().flat_map(|(_, rows)| rows.iter().map(|r| r.0)).collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Close,
    Outside,
    Row(usize),
    Dec(usize),
    Inc(usize),
    Panel,
}

pub struct Settings {
    pub selected: usize,
    scroll: f32,
    fonts: Vec<String>,
    pub restart_note: bool,
    pub hover: Option<Hit>,
}

/// Geometría del panel (compartida por el dibujo y la detección de clics).
struct Layout {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    header_h: f32,
    body_y: f32,
    body_h: f32,
    close: (f32, f32, f32),
    /// (índice de fila, y, alto) solo de las filas visibles
    rows: Vec<(usize, f32, f32)>,
    /// (y, título) de las cabeceras de sección visibles
    sections: Vec<(f32, &'static str)>,
    control_w: f32,
    /// (inicio, tamaño) de la barra de scroll relativa al cuerpo, si el contenido no cabe.
    scrollbar: Option<(f32, f32)>,
}

impl Settings {
    pub fn new() -> Self {
        Self {
            selected: 0,
            scroll: 0.0,
            fonts: crate::font::FontSet::monospace_families(),
            restart_note: false,
            hover: None,
        }
    }

    fn field(&self, i: usize) -> Field {
        fields()[i]
    }

    pub fn move_sel(&mut self, d: i32) {
        let n = fields().len() as i32;
        self.selected = (self.selected as i32 + d).clamp(0, n - 1) as usize;
    }

    pub fn value_text(&self, f: Field, cfg: &Config) -> String {
        match f {
            Field::Theme => cfg.theme.clone(),
            Field::FontFamily => cfg.font.family.clone(),
            Field::FontSize => format!("{} pt", trim_num(cfg.font.size)),
            Field::LineHeight => trim_num(cfg.font.line_height),
            Field::Padding => format!("{} px", cfg.window.padding.round()),
            Field::Opacity => format!("{} %", (cfg.window.opacity * 100.0).round()),
            Field::CursorStyle => match cfg.terminal.cursor_style {
                CursorStyle::Block => "Bloque".into(),
                CursorStyle::Beam => "Barra".into(),
                CursorStyle::Underline => "Subrayado".into(),
            },
            Field::Scrollback => format!("{} líneas", cfg.terminal.scrollback),
            Field::EditFile => "Abrir".into(),
            Field::Version => env!("CARGO_PKG_VERSION").into(),
            _ => String::new(),
        }
    }

    fn toggle_value(f: Field, cfg: &Config) -> Option<bool> {
        Some(match f {
            Field::CursorBlink => cfg.terminal.cursor_blink,
            Field::CopyOnSelect => cfg.terminal.copy_on_select,
            Field::Blocks => cfg.terminal.blocks,
            Field::ShellIntegration => cfg.terminal.shell_integration,
            Field::Decorations => cfg.window.decorations,
            Field::CheckUpdates => cfg.check_updates,
            _ => return None,
        })
    }

    /// Cambia el valor de la fila `i` en `dir` pasos (-1/+1; 0 = activar).
    pub fn adjust(&mut self, i: usize, dir: i32, cfg: &mut Config) -> Change {
        let f = self.field(i);
        let d = if dir == 0 { 1 } else { dir };
        let cycle = |list: &[String], cur: &str| -> String {
            let n = list.len() as i32;
            let pos = list.iter().position(|x| x.eq_ignore_ascii_case(cur)).map_or(0, |p| p as i32);
            list[(pos + d).rem_euclid(n.max(1)) as usize].clone()
        };
        match f {
            Field::Theme => {
                let names: Vec<String> = Theme::NAMES.iter().map(|s| s.to_string()).collect();
                cfg.theme = cycle(&names, &cfg.theme);
                Change::Theme
            }
            Field::FontFamily => {
                if self.fonts.is_empty() {
                    return Change::None;
                }
                cfg.font.family = cycle(&self.fonts, &cfg.font.family);
                Change::Fonts
            }
            Field::FontSize => {
                cfg.font.size = (cfg.font.size + 0.5 * d as f32).clamp(6.0, 32.0);
                Change::FontSize
            }
            Field::LineHeight => {
                cfg.font.line_height = ((cfg.font.line_height + 0.05 * d as f32) * 100.0).round() / 100.0;
                cfg.font.line_height = cfg.font.line_height.clamp(0.8, 2.0);
                Change::LineHeight
            }
            Field::Padding => {
                cfg.window.padding = (cfg.window.padding + 2.0 * d as f32).clamp(0.0, 40.0);
                Change::Layout
            }
            Field::Opacity => {
                cfg.window.opacity = ((cfg.window.opacity + 0.05 * d as f32) * 100.0).round() / 100.0;
                cfg.window.opacity = cfg.window.opacity.clamp(0.3, 1.0);
                Change::Redraw
            }
            Field::CursorStyle => {
                let order = [CursorStyle::Block, CursorStyle::Beam, CursorStyle::Underline];
                let pos = order.iter().position(|c| *c == cfg.terminal.cursor_style).unwrap_or(0) as i32;
                cfg.terminal.cursor_style = order[(pos + d).rem_euclid(3) as usize];
                Change::TermOptions
            }
            Field::Scrollback => {
                let v = cfg.terminal.scrollback as i64 + 1000 * d as i64;
                cfg.terminal.scrollback = v.clamp(1000, 100_000) as usize;
                Change::TermOptions
            }
            Field::CursorBlink => {
                cfg.terminal.cursor_blink = !cfg.terminal.cursor_blink;
                Change::TermOptions
            }
            Field::CopyOnSelect => {
                cfg.terminal.copy_on_select = !cfg.terminal.copy_on_select;
                Change::Redraw
            }
            Field::Blocks => {
                cfg.terminal.blocks = !cfg.terminal.blocks;
                Change::Redraw
            }
            Field::ShellIntegration => {
                cfg.terminal.shell_integration = !cfg.terminal.shell_integration;
                Change::Redraw
            }
            Field::Decorations => {
                cfg.window.decorations = !cfg.window.decorations;
                self.restart_note = true;
                Change::Restart
            }
            Field::EditFile => Change::OpenFile,
            Field::CheckUpdates => {
                cfg.check_updates = !cfg.check_updates;
                Change::Updates
            }
            Field::Version => Change::None,
        }
    }

    fn layout(&mut self, width: f32, height: f32, top: f32, s: f32) -> Layout {
        let w = (560.0 * s).min(width - 32.0 * s).max(260.0 * s).round();
        let header_h = (56.0 * s).round();
        let footer_h = (40.0 * s).round();
        let row_h = (40.0 * s).round();
        let sec_h = (34.0 * s).round();
        let content: f32 = SECTIONS.iter().map(|(_, r)| sec_h + r.len() as f32 * row_h).sum::<f32>() + 8.0 * s;
        let max_h = (height - top - 48.0 * s).max(200.0 * s);
        let h = (header_h + content + footer_h).min(max_h).round();
        let x = ((width - w) / 2.0).round();
        let y = (top + ((height - top - h) / 2.0).max(16.0 * s)).round();
        let body_y = y + header_h;
        let body_h = h - header_h - footer_h;

        // Scroll: mantener visible la fila seleccionada.
        let mut cy = 0.0;
        let mut sel_y = (0.0, 0.0);
        let mut ri = 0;
        for it in items() {
            match it {
                Item::Section(_) => cy += sec_h,
                Item::Row => {
                    if ri == self.selected {
                        sel_y = (cy, cy + row_h);
                    }
                    cy += row_h;
                    ri += 1;
                }
            }
        }
        let max_scroll = (cy + 8.0 * s - body_h).max(0.0);
        if sel_y.0 < self.scroll {
            self.scroll = sel_y.0;
        }
        if sel_y.1 > self.scroll + body_h {
            self.scroll = sel_y.1 - body_h;
        }
        self.scroll = self.scroll.clamp(0.0, max_scroll);

        let mut rows = Vec::new();
        let mut sections = Vec::new();
        let mut cy = body_y - self.scroll;
        let mut ri = 0;
        for it in items() {
            let ih = if matches!(it, Item::Section(_)) { sec_h } else { row_h };
            let visible = cy >= body_y - 0.5 && cy + ih <= body_y + body_h + 0.5;
            match it {
                Item::Section(t) => {
                    if visible {
                        sections.push((cy, t));
                    }
                }
                Item::Row => {
                    if visible {
                        rows.push((ri, cy, row_h));
                    }
                    ri += 1;
                }
            }
            cy += ih;
        }
        let close = (x + w - 30.0 * s, y + header_h / 2.0, 14.0 * s);
        let total = max_scroll + body_h;
        let scrollbar = (max_scroll > 0.0).then(|| {
            let size = (body_h * body_h / total).max(24.0 * s);
            (self.scroll / max_scroll * (body_h - size), size)
        });
        Layout { x, y, w, h, header_h, body_y, body_h, close, rows, sections, control_w: (190.0 * s).round(), scrollbar }
    }

    pub fn hit(&mut self, mx: f32, my: f32, width: f32, height: f32, top: f32, s: f32) -> Hit {
        let l = self.layout(width, height, top, s);
        if mx < l.x || mx > l.x + l.w || my < l.y || my > l.y + l.h {
            return Hit::Outside;
        }
        let (cx, cy, r) = l.close;
        if (mx - cx).hypot(my - cy) <= r {
            return Hit::Close;
        }
        let pad = (16.0 * s).round();
        for &(i, ry, rh) in &l.rows {
            if my >= ry && my < ry + rh {
                let right = l.x + l.w - pad;
                let left = right - l.control_w;
                let f = self.field(i);
                if !is_toggle(f) && !matches!(f, Field::EditFile | Field::Version) && mx >= left {
                    return if mx < left + l.control_w / 2.0 { Hit::Dec(i) } else { Hit::Inc(i) };
                }
                return Hit::Row(i);
            }
        }
        Hit::Panel
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, p: &mut Painter, theme: &Theme, cfg: &Config, width: f32, height: f32, top: f32, s: f32) {
        let l = self.layout(width, height, top, s);
        p.rect(0.0, 0.0, width, height, Rgb(0, 0, 0), 0.35);
        let panel = surface(theme, 0.05);
        p.card(l.x, l.y, l.w, l.h, panel, theme.foreground, (14.0 * s).round(), s);
        let dimc = dim(theme);
        let pad = (16.0 * s).round();
        let hair = s.round().max(1.0);

        // Cabecera
        p.ui_text_in(l.x + pad + 4.0 * s, l.y, l.header_h, "Ajustes", theme.foreground, l.w / 2.0);
        let (cx, cy, r) = l.close;
        if self.hover == Some(Hit::Close) {
            p.circle(cx, cy, r, theme.foreground, 0.12);
        }
        cross(p, cx, cy, 4.0 * s, 1.5 * s, theme.foreground);
        p.rect(l.x, l.y + l.header_h - hair, l.w, hair, theme.foreground, 0.08);

        for &(sy, title) in &l.sections {
            p.ui_text(l.x + pad + 4.0 * s, sy + (14.0 * s).round(), title, theme.accent, l.w);
        }

        for &(i, ry, rh) in &l.rows {
            let f = self.field(i);
            let label = SECTIONS.iter().flat_map(|(_, r)| r.iter()).find(|r| r.0 == f).map_or("", |r| r.1);
            let hovered = matches!(self.hover, Some(Hit::Row(j) | Hit::Dec(j) | Hit::Inc(j)) if j == i);
            if i == self.selected || hovered {
                let a = if i == self.selected { 0.14 } else { 0.07 };
                p.rounded(l.x + pad / 2.0, ry + 2.0 * s, l.w - pad, rh - 4.0 * s, theme.accent, a, (8.0 * s).round());
            }
            let mut lw = p.ui_text_in(l.x + pad + 4.0 * s, ry, rh, label, theme.foreground, l.w * 0.45);
            if f == Field::Decorations && self.restart_note {
                lw += p.ui_text_in(l.x + pad + 4.0 * s + lw, ry, rh, "  · al reiniciar", dimc, l.w * 0.3);
            }
            let _ = lw;

            let right = l.x + l.w - pad - 4.0 * s;
            let mid = ry + rh / 2.0;
            if let Some(on) = Self::toggle_value(f, cfg) {
                // Interruptor
                let tw = (38.0 * s).round();
                let th = (22.0 * s).round();
                let tx = right - tw;
                let ty = (mid - th / 2.0).round();
                if on {
                    p.rounded(tx, ty, tw, th, theme.accent, 1.0, th / 2.0);
                } else {
                    p.rounded(tx, ty, tw, th, theme.foreground, 0.18, th / 2.0);
                }
                let kr = th / 2.0 - 3.0 * s;
                let kx = if on { tx + tw - th / 2.0 } else { tx + th / 2.0 };
                p.circle(kx, mid, kr, Rgb(255, 255, 255), 0.95);
            } else if f == Field::Version {
                let t = self.value_text(f, cfg);
                let tw = p.ui_measure(&t);
                p.ui_text_in(right - tw, ry, rh, &t, dimc, tw + 2.0);
            } else if f == Field::EditFile {
                let t = "Abrir";
                let tw = p.ui_measure(t);
                p.ui_text_in(right - tw - 12.0 * s, ry, rh, t, theme.accent, tw + 2.0);
                let ax = right - 4.0 * s;
                p.line(ax - 3.0 * s, mid - 4.0 * s, ax + 1.0 * s, mid, 1.5 * s, theme.accent, 1.0);
                p.line(ax + 1.0 * s, mid, ax - 3.0 * s, mid + 4.0 * s, 1.5 * s, theme.accent, 1.0);
            } else {
                // Selector ‹ valor ›
                let left = right - l.control_w;
                let ph = (28.0 * s).round();
                let py = (mid - ph / 2.0).round();
                p.rounded(left, py, l.control_w, ph, theme.foreground, 0.06, (7.0 * s).round());
                for (dir, hx) in [(-1.0f32, left + 14.0 * s), (1.0, right - 14.0 * s)] {
                    let over = matches!((self.hover, dir < 0.0), (Some(Hit::Dec(j)), true) | (Some(Hit::Inc(j)), false) if j == i);
                    let c = if over { theme.foreground } else { dimc };
                    let k = 3.5 * s;
                    p.line(hx + dir * -k * 0.5, mid - k, hx + dir * k * 0.5, mid, 1.6 * s, c, 1.0);
                    p.line(hx + dir * k * 0.5, mid, hx + dir * -k * 0.5, mid + k, 1.6 * s, c, 1.0);
                }
                let value = self.value_text(f, cfg);
                let inner = l.control_w - 56.0 * s;
                let vw = p.ui_measure(&value).min(inner);
                let mut vx = left + (l.control_w - vw) / 2.0;
                if f == Field::Theme {
                    // Muestras de color del tema
                    if let Some(t) = Theme::builtin(&cfg.theme) {
                        let sw = 5.0 * s;
                        let total = 4.0 * (2.0 * sw + 2.0 * s);
                        let sx0 = left + 28.0 * s;
                        for (k, c) in [t.background, t.accent, t.palette[2], t.palette[1]].iter().enumerate() {
                            let ccx = sx0 + sw + k as f32 * (2.0 * sw + 2.0 * s);
                            p.circle(ccx, mid, sw + hair, theme.foreground, 0.25);
                            p.circle(ccx, mid, sw, *c, 1.0);
                        }
                        vx = vx.max(sx0 + total + 6.0 * s);
                    }
                }
                p.ui_text_in(vx.round(), ry, rh, &value, theme.foreground, right - 28.0 * s - vx);
            }
        }

        if let Some((off, size)) = l.scrollbar {
            let bw = (4.0 * s).round();
            p.rounded(l.x + l.w - bw - 4.0 * s, l.body_y + off + 2.0 * s, bw, size - 4.0 * s, theme.foreground, 0.22, bw / 2.0);
        }

        // Pie
        let fy = l.body_y + l.body_h;
        p.rect(l.x, fy, l.w, hair, theme.foreground, 0.08);
        let path = Config::path().map(|p| p.display().to_string()).unwrap_or_default();
        let home = std::env::var("HOME").unwrap_or_default();
        let short = if !home.is_empty() { path.replacen(&home, "~", 1) } else { path };
        p.ui_text_in(
            l.x + pad + 4.0 * s,
            fy,
            l.y + l.h - fy,
            &format!("Se guarda solo en {short}"),
            dimc,
            l.w - 2.0 * pad,
        );
    }
}

fn is_toggle(f: Field) -> bool {
    matches!(
        f,
        Field::CursorBlink | Field::CopyOnSelect | Field::Blocks | Field::ShellIntegration | Field::Decorations | Field::CheckUpdates
    )
}

fn trim_num(v: f32) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.replace('.', ",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guardar_conserva_comentarios_y_relee() {
        let dir = std::env::temp_dir().join(format!("garbo-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("garbo")).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let path = dir.join("garbo/config.toml");
        std::fs::write(&path, "# mi comentario\n[shell]\nprogram = \"/bin/zsh\"\n").unwrap();

        let mut cfg = Config::load();
        let mut set = Settings { selected: 0, scroll: 0.0, fonts: vec![], restart_note: false, hover: None };
        let i_theme = fields().iter().position(|f| *f == Field::Theme).unwrap();
        let i_size = fields().iter().position(|f| *f == Field::FontSize).unwrap();
        let i_blocks = fields().iter().position(|f| *f == Field::Blocks).unwrap();
        assert_eq!(set.adjust(i_theme, 1, &mut cfg), Change::Theme);
        assert_eq!(cfg.theme, Theme::NAMES[1]);
        set.adjust(i_size, 1, &mut cfg);
        set.adjust(i_blocks, 0, &mut cfg);
        cfg.save().unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# mi comentario"), "{text}");
        assert!(text.contains("program = \"/bin/zsh\""), "{text}");
        let back = Config::load();
        assert_eq!(back.theme, Theme::NAMES[1]);
        assert_eq!(back.font.size, 11.5);
        assert!(!back.terminal.blocks);
        assert_eq!(back.shell.program.as_deref(), Some("/bin/zsh"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn limites_de_valores() {
        let mut cfg = Config::default();
        let mut set = Settings { selected: 0, scroll: 0.0, fonts: vec![], restart_note: false, hover: None };
        let i = fields().iter().position(|f| *f == Field::FontSize).unwrap();
        for _ in 0..100 {
            set.adjust(i, -1, &mut cfg);
        }
        assert_eq!(cfg.font.size, 6.0);
        let i = fields().iter().position(|f| *f == Field::Opacity).unwrap();
        for _ in 0..100 {
            set.adjust(i, 1, &mut cfg);
        }
        assert_eq!(cfg.window.opacity, 1.0);
        set.move_sel(-5);
        assert_eq!(set.selected, 0);
        set.move_sel(1000);
        assert_eq!(set.selected, fields().len() - 1);
    }
}
