//! Logos de los agentes de código: en las pestañas, el panel de agentes y las estadísticas.
//!
//! Los trazados son los de LobeHub Icons (`@lobehub/icons-static-svg` 1.106.0, licencia MIT), en
//! su rejilla de 24 × 24, salvo el de Gemini CLI: su pixel art de 12 × 10 celdas, sacado de una
//! imagen que dio el propietario (los colores, llevados a los niveles de la paleta de 256 colores
//! del terminal, de la que salen). Los logos son marcas de sus dueños: aquí solo dicen qué agente
//! se ejecuta. Se rasterizan la primera vez que se piden a un tamaño y quedan en el atlas de
//! glifos (ver `GlyphCache::get_logo`): después, dibujarlos cuesta lo que un glifo.
//!
//! Los logos negros (Copilot, Cursor, Goose, OpenCode) se guardan como máscara y se pintan del
//! color del texto, así se ven con cualquier tema. Los de color, con el suyo: Claude Code y Amp el
//! de su versión a color; Codex, la forma de un color con el degradado de su versión a color (que
//! lleva además una baldosa de fondo que a 14 px pesaba demasiado), y Qwen, el de Simple Icons
//! (`#6950EF`), que se ve mejor sobre fondo oscuro que el de LobeHub.
//!
//! El pixel art (Claude Code, OpenCode y Gemini CLI) se dibuja nítido: bordes en píxeles enteros
//! y sin suavizado. Escalado a 14 px, cada cuadrado caía entre dos píxeles y se emborronaba.

use crate::font::RasterGlyph;
use tiny_skia as sk;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Logo {
    ClaudeCode,
    Codex,
    GeminiCli,
    Copilot,
    Cursor,
    Qwen,
    Goose,
    OpenCode,
    Amp,
}

/// Con qué se rellena un logo de trazados.
enum Fill {
    /// Máscara: se tiñe al dibujar con el color del texto.
    Text,
    Solid([u8; 3]),
    /// Degradado lineal de (x1, y1) a (x2, y2) en la rejilla de 24, con sus paradas.
    Linear([f32; 4], &'static [(f32, [u8; 3])]),
}

enum Art {
    /// Trazados SVG en la rejilla de 24. `crisp`: solo rectas horizontales y verticales (pixel
    /// art), que se llevan a píxeles enteros y se pintan sin suavizado.
    Paths {
        paths: &'static [&'static str],
        evenodd: bool,
        fill: Fill,
        crisp: bool,
    },
    /// Pixel art por celdas: cada carácter de una fila es un color de `colors` ('.' = vacío).
    Cells {
        rows: &'static [&'static str],
        colors: &'static [(char, [u8; 3])],
    },
}

impl Logo {
    pub const ALL: [Logo; 9] = [
        Logo::ClaudeCode,
        Logo::Codex,
        Logo::GeminiCli,
        Logo::Copilot,
        Logo::Cursor,
        Logo::Qwen,
        Logo::Goose,
        Logo::OpenCode,
        Logo::Amp,
    ];

    /// ¿Lleva sus propios colores? (si no, es una máscara del color del texto).
    pub fn is_color(self) -> bool {
        !matches!(
            self.art(),
            Art::Paths {
                fill: Fill::Text,
                ..
            }
        )
    }

    fn art(self) -> Art {
        let paths = |paths, evenodd, fill, crisp| Art::Paths {
            paths,
            evenodd,
            fill,
            crisp,
        };
        match self {
            Logo::ClaudeCode => paths(CLAUDE_CODE, true, Fill::Solid([0xD9, 0x77, 0x57]), true),
            // El degradado de `codex-color` (de y = 3 a 21 sobre su baldosa), estirado a la forma.
            Logo::Codex => paths(
                CODEX,
                true,
                Fill::Linear(
                    [12.0, 0.0, 12.0, 24.0],
                    &[
                        (0.0, [0xB1, 0xA7, 0xFF]),
                        (0.5, [0x7A, 0x9D, 0xFF]),
                        (1.0, [0x39, 0x41, 0xFF]),
                    ],
                ),
                false,
            ),
            Logo::GeminiCli => Art::Cells {
                rows: GEMINI_CLI,
                colors: GEMINI_CLI_COLORS,
            },
            Logo::Copilot => paths(COPILOT, true, Fill::Text, false),
            Logo::Cursor => paths(CURSOR, true, Fill::Text, false),
            Logo::Qwen => paths(QWEN, false, Fill::Solid([0x69, 0x50, 0xEF]), false),
            Logo::Goose => paths(GOOSE, true, Fill::Text, false),
            Logo::OpenCode => paths(OPENCODE, true, Fill::Text, true),
            Logo::Amp => paths(AMP, false, Fill::Solid([0xF3, 0x4E, 0x3F]), false),
        }
    }

    /// Índice para la caché del atlas.
    pub fn id(self) -> u8 {
        Logo::ALL.iter().position(|l| *l == self).unwrap_or(0) as u8
    }
}

/// El logo para un cuadrado de `px` × `px` píxeles. Los de color, RGBA sin premultiplicar (como
/// los emoji de `colr`); los del color del texto, blanco con la cobertura en el alfa (como un
/// glifo). El pixel art por celdas puede quedar algo más pequeño o más grande que el cuadrado
/// (celdas de píxeles enteros): quien lo dibuja lo centra.
pub fn rasterize(logo: Logo, px: u32) -> Option<RasterGlyph> {
    let (paths, evenodd, fill, crisp) = match logo.art() {
        Art::Paths {
            paths,
            evenodd,
            fill,
            crisp,
        } => (paths, evenodd, fill, crisp),
        Art::Cells { rows, colors } => return cells(rows, colors, px),
    };
    let mut pm = sk::Pixmap::new(px, px)?;
    // Todo en píxeles: el trazado y el degradado, sin transformación al pintar.
    let k = px as f32 / 24.0;
    let to_px = sk::Transform::from_scale(k, k);
    let mut paint = sk::Paint {
        anti_alias: !crisp,
        ..Default::default()
    };
    let rgb = |c: [u8; 3]| sk::Color::from_rgba8(c[0], c[1], c[2], 255);
    match fill {
        Fill::Text => paint.set_color_rgba8(255, 255, 255, 255),
        Fill::Solid(c) => paint.set_color(rgb(c)),
        Fill::Linear([x1, y1, x2, y2], stops) => {
            paint.shader = sk::LinearGradient::new(
                sk::Point::from_xy(x1 * k, y1 * k),
                sk::Point::from_xy(x2 * k, y2 * k),
                stops
                    .iter()
                    .map(|(at, c)| sk::GradientStop::new(*at, rgb(*c)))
                    .collect(),
                sk::SpreadMode::Pad,
                sk::Transform::identity(),
            )?;
        }
    }
    let rule = if evenodd {
        sk::FillRule::EvenOdd
    } else {
        sk::FillRule::Winding
    };
    for d in paths {
        let mut path = parse_path(d)?.transform(to_px)?;
        if crisp {
            path = snap(&path)?;
        }
        pm.fill_path(&path, &paint, rule, sk::Transform::identity(), None);
    }
    let color = logo.is_color();
    let mut rgba = Vec::with_capacity((px * px * 4) as usize);
    for p in pm.pixels() {
        if color {
            let c = p.demultiply();
            rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
        } else {
            rgba.extend_from_slice(&[255, 255, 255, p.alpha()]);
        }
    }
    Some(RasterGlyph {
        width: px,
        height: px,
        left: 0,
        top: 0,
        is_color: color,
        rgba,
    })
}

/// Un trazado de rectas con los puntos en píxeles enteros (para el pixel art).
fn snap(path: &sk::Path) -> Option<sk::Path> {
    let mut pb = sk::PathBuilder::new();
    for seg in path.segments() {
        match seg {
            sk::PathSegment::MoveTo(p) => pb.move_to(p.x.round(), p.y.round()),
            sk::PathSegment::LineTo(p) => pb.line_to(p.x.round(), p.y.round()),
            sk::PathSegment::Close => pb.close(),
            // Una curva no es pixel art.
            _ => return None,
        }
    }
    pb.finish()
}

/// Pixel art por celdas para un cuadrado de `px`. Las celdas son de píxeles enteros si así el
/// dibujo no cambia más de un 15 % de tamaño; si no, cada borde cae en el píxel más cercano
/// (celdas de uno o dos píxeles, pero nítidas).
fn cells(rows: &[&str], colors: &[(char, [u8; 3])], px: u32) -> Option<RasterGlyph> {
    let (cols, n) = (rows.first()?.chars().count(), rows.len());
    let exact = px as f32 / cols.max(n) as f32;
    let whole = exact.round();
    let cell = if whole >= 1.0 && (whole - exact).abs() <= exact * 0.15 {
        whole
    } else {
        exact
    };
    let edge = |i: usize| (i as f32 * cell).round() as u32;
    let (w, h) = (edge(cols).max(1), edge(n).max(1));
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for (j, row) in rows.iter().enumerate() {
        for (i, ch) in row.chars().enumerate() {
            let Some((_, c)) = colors.iter().find(|(k, _)| *k == ch) else {
                continue;
            };
            for y in edge(j)..edge(j + 1).min(h) {
                for x in edge(i)..edge(i + 1).min(w) {
                    let o = ((y * w + x) * 4) as usize;
                    rgba[o..o + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
                }
            }
        }
    }
    Some(RasterGlyph {
        width: w,
        height: h,
        left: 0,
        top: 0,
        is_color: true,
        rgba,
    })
}

/// Lee los números y banderas de un trazado SVG, con sus abreviaturas (`-.5.5`, `a1 1 0 012 2`).
struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

impl Lexer<'_> {
    fn skip(&mut self) {
        while self
            .s
            .get(self.i)
            .is_some_and(|c| c.is_ascii_whitespace() || *c == b',')
        {
            self.i += 1;
        }
    }

    /// Siguiente orden, si lo que viene es una letra.
    fn command(&mut self) -> Option<u8> {
        self.skip();
        let c = *self.s.get(self.i)?;
        c.is_ascii_alphabetic().then(|| {
            self.i += 1;
            c
        })
    }

    fn at_end(&mut self) -> bool {
        self.skip();
        self.i >= self.s.len()
    }

    fn num(&mut self) -> Option<f32> {
        self.skip();
        let start = self.i;
        let at = |i: usize| self.s.get(i).copied();
        let mut i = self.i;
        if matches!(at(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let mut dot = false;
        while let Some(c) = at(i) {
            if c.is_ascii_digit() {
                i += 1;
            } else if c == b'.' && !dot {
                dot = true;
                i += 1;
            } else {
                break;
            }
        }
        if matches!(at(i), Some(b'e' | b'E')) {
            let mut j = i + 1;
            if matches!(at(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if at(j).is_some_and(|c| c.is_ascii_digit()) {
                while at(j).is_some_and(|c| c.is_ascii_digit()) {
                    j += 1;
                }
                i = j;
            }
        }
        self.i = i;
        std::str::from_utf8(&self.s[start..i]).ok()?.parse().ok()
    }

    /// Bandera de un arco: un solo carácter, 0 o 1.
    fn flag(&mut self) -> Option<bool> {
        self.skip();
        let c = *self.s.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

/// Trazado SVG (`d`) a tiny-skia: M L H V C S Q T A Z, absolutos y relativos.
fn parse_path(d: &str) -> Option<sk::Path> {
    let mut lx = Lexer {
        s: d.as_bytes(),
        i: 0,
    };
    let mut pb = sk::PathBuilder::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    // Último punto de control de una curva (para S y T, que lo reflejan).
    let mut cubic: Option<(f32, f32)> = None;
    let mut quad: Option<(f32, f32)> = None;
    let mut cmd = lx.command()?;
    loop {
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { (x, y) } else { (0.0, 0.0) };
        let (mut next_cubic, mut next_quad) = (None, None);
        match cmd.to_ascii_uppercase() {
            b'M' => {
                (x, y) = (lx.num()? + ox, lx.num()? + oy);
                (sx, sy) = (x, y);
                pb.move_to(x, y);
                // Los pares que siguen a una M son líneas.
                cmd = if rel { b'l' } else { b'L' };
            }
            b'L' => {
                (x, y) = (lx.num()? + ox, lx.num()? + oy);
                pb.line_to(x, y);
            }
            b'H' => {
                x = lx.num()? + ox;
                pb.line_to(x, y);
            }
            b'V' => {
                y = lx.num()? + oy;
                pb.line_to(x, y);
            }
            b'C' => {
                let (x1, y1) = (lx.num()? + ox, lx.num()? + oy);
                let (x2, y2) = (lx.num()? + ox, lx.num()? + oy);
                (x, y) = (lx.num()? + ox, lx.num()? + oy);
                pb.cubic_to(x1, y1, x2, y2, x, y);
                next_cubic = Some((x2, y2));
            }
            b'S' => {
                let (x1, y1) = cubic.map_or((x, y), |(cx, cy)| (2.0 * x - cx, 2.0 * y - cy));
                let (x2, y2) = (lx.num()? + ox, lx.num()? + oy);
                (x, y) = (lx.num()? + ox, lx.num()? + oy);
                pb.cubic_to(x1, y1, x2, y2, x, y);
                next_cubic = Some((x2, y2));
            }
            b'Q' => {
                let (x1, y1) = (lx.num()? + ox, lx.num()? + oy);
                (x, y) = (lx.num()? + ox, lx.num()? + oy);
                pb.quad_to(x1, y1, x, y);
                next_quad = Some((x1, y1));
            }
            b'T' => {
                let (x1, y1) = quad.map_or((x, y), |(qx, qy)| (2.0 * x - qx, 2.0 * y - qy));
                (x, y) = (lx.num()? + ox, lx.num()? + oy);
                pb.quad_to(x1, y1, x, y);
                next_quad = Some((x1, y1));
            }
            b'A' => {
                let (rx, ry, rot) = (lx.num()?, lx.num()?, lx.num()?);
                let (large, sweep) = (lx.flag()?, lx.flag()?);
                let (x2, y2) = (lx.num()? + ox, lx.num()? + oy);
                arc(&mut pb, (x, y), (rx, ry), rot, large, sweep, (x2, y2));
                (x, y) = (x2, y2);
            }
            b'Z' => {
                pb.close();
                (x, y) = (sx, sy);
            }
            _ => return None,
        }
        (cubic, quad) = (next_cubic, next_quad);
        if lx.at_end() {
            break;
        }
        // Sin letra, se repite la orden anterior (salvo tras Z, que no lleva números).
        match lx.command() {
            Some(c) => cmd = c,
            None if cmd.eq_ignore_ascii_case(&b'Z') => return None,
            None => {}
        }
    }
    pb.finish()
}

/// Arco elíptico de SVG como curvas cúbicas (de un cuarto de vuelta como mucho). Ver "Elliptical
/// arc implementation notes" en la especificación de SVG.
fn arc(
    pb: &mut sk::PathBuilder,
    (x1, y1): (f32, f32),
    (rx, ry): (f32, f32),
    rot: f32,
    large: bool,
    sweep: bool,
    (x2, y2): (f32, f32),
) {
    use std::f32::consts::{FRAC_PI_2, TAU};
    if (x1, y1) == (x2, y2) {
        return;
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 {
        pb.line_to(x2, y2);
        return;
    }
    let (sin, cos) = rot.to_radians().sin_cos();
    let (dx, dy) = ((x1 - x2) / 2.0, (y1 - y2) / 2.0);
    let (px, py) = (cos * dx + sin * dy, -sin * dx + cos * dy);
    // Radios demasiado pequeños para llegar: se agrandan lo justo.
    let l = (px / rx).powi(2) + (py / ry).powi(2);
    if l > 1.0 {
        rx *= l.sqrt();
        ry *= l.sqrt();
    }
    let num = (rx * ry).powi(2) - (rx * py).powi(2) - (ry * px).powi(2);
    let den = (rx * py).powi(2) + (ry * px).powi(2);
    let mut k = (num / den).max(0.0).sqrt();
    if large == sweep {
        k = -k;
    }
    let (cpx, cpy) = (k * rx * py / ry, -k * ry * px / rx);
    let (cx, cy) = (
        cos * cpx - sin * cpy + (x1 + x2) / 2.0,
        sin * cpx + cos * cpy + (y1 + y2) / 2.0,
    );
    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    let (ux, uy) = ((px - cpx) / rx, (py - cpy) / ry);
    let (vx, vy) = ((-px - cpx) / rx, (-py - cpy) / ry);
    let start = angle(1.0, 0.0, ux, uy);
    let mut delta = angle(ux, uy, vx, vy);
    if !sweep && delta > 0.0 {
        delta -= TAU;
    } else if sweep && delta < 0.0 {
        delta += TAU;
    }
    let n = (delta.abs() / FRAC_PI_2).ceil().max(1.0);
    let step = delta / n;
    let t = 4.0 / 3.0 * (step / 4.0).tan();
    // Del círculo unidad a la elipse girada en su sitio.
    let at = |ex: f32, ey: f32| {
        (
            cx + cos * rx * ex - sin * ry * ey,
            cy + sin * rx * ex + cos * ry * ey,
        )
    };
    let mut a = start;
    for _ in 0..n as usize {
        let (s1, c1) = a.sin_cos();
        let (s2, c2) = (a + step).sin_cos();
        let p1 = at(c1 - t * s1, s1 + t * c1);
        let p2 = at(c2 + t * s2, s2 - t * c2);
        let p = at(c2, s2);
        pb.cubic_to(p1.0, p1.1, p2.0, p2.1, p.0, p.1);
        a += step;
    }
}

// Trazados de LobeHub Icons (`icons/<nombre>.svg`, 1.106.0), sin tocar; Gemini CLI, aparte.
const CLAUDE_CODE: &[&str] = &[
    "M20.998 10.949H24v3.102h-3v3.028h-1.487V20H18v-2.921h-1.487V20H15v-2.921H9V20H7.488v-2.921H6V20H4.487v-2.921H3V14.05H0V10.95h3V5h17.998v5.949zM6 10.949h1.488V8.102H6v2.847zm10.51 0H18V8.102h-1.49v2.847z",
];
const CODEX: &[&str] = &[
    "M8.086.457a6.105 6.105 0 013.046-.415c1.333.153 2.521.72 3.564 1.7a.117.117 0 00.107.029c1.408-.346 2.762-.224 4.061.366l.063.03.154.076c1.357.703 2.33 1.77 2.918 3.198.278.679.418 1.388.421 2.126a5.655 5.655 0 01-.18 1.631.167.167 0 00.04.155 5.982 5.982 0 011.578 2.891c.385 1.901-.01 3.615-1.183 5.14l-.182.22a6.063 6.063 0 01-2.934 1.851.162.162 0 00-.108.102c-.255.736-.511 1.364-.987 1.992-1.199 1.582-2.962 2.462-4.948 2.451-1.583-.008-2.986-.587-4.21-1.736a.145.145 0 00-.14-.032c-.518.167-1.04.191-1.604.185a5.924 5.924 0 01-2.595-.622 6.058 6.058 0 01-2.146-1.781c-.203-.269-.404-.522-.551-.821a7.74 7.74 0 01-.495-1.283 6.11 6.11 0 01-.017-3.064.166.166 0 00.008-.074.115.115 0 00-.037-.064 5.958 5.958 0 01-1.38-2.202 5.196 5.196 0 01-.333-1.589 6.915 6.915 0 01.188-2.132c.45-1.484 1.309-2.648 2.577-3.493.282-.188.55-.334.802-.438.286-.12.573-.22.861-.304a.129.129 0 00.087-.087A6.016 6.016 0 015.635 2.31C6.315 1.464 7.132.846 8.086.457zm-.804 7.85a.848.848 0 00-1.473.842l1.694 2.965-1.688 2.848a.849.849 0 001.46.864l1.94-3.272a.849.849 0 00.007-.854l-1.94-3.393zm5.446 6.24a.849.849 0 000 1.695h4.848a.849.849 0 000-1.696h-4.848z",
];
/// Gemini CLI: 12 × 10 celdas (ver el comentario del módulo).
const GEMINI_CLI: &[&str] = &[
    ".....ab.....",
    "....cadb....",
    "...efgdbb...",
    "...higdjj...",
    "..iklmnopq..",
    "..hlr..rsp..",
    "..tu....rr..",
    ".tvv....wwr.",
    ".xy......ww.",
    "yy........ww",
];
const GEMINI_CLI_COLORS: &[(char, [u8; 3])] = &[
    ('a', [0xFF, 0x87, 0x00]),
    ('b', [0xFF, 0x5F, 0x5F]),
    ('c', [0xD7, 0xAF, 0x5F]),
    ('d', [0xFF, 0x87, 0x5F]),
    ('e', [0xAF, 0xD7, 0x5F]),
    ('f', [0xAF, 0xAF, 0x5F]),
    ('g', [0xD7, 0x87, 0x5F]),
    ('h', [0x87, 0xD7, 0x5F]),
    ('i', [0x87, 0xAF, 0x5F]),
    ('j', [0xD7, 0x5F, 0x5F]),
    ('k', [0x5F, 0xAF, 0x5F]),
    ('l', [0x5F, 0xAF, 0x87]),
    ('m', [0x5F, 0x87, 0xAF]),
    ('n', [0x87, 0x87, 0xAF]),
    ('o', [0x87, 0x5F, 0xD7]),
    ('p', [0x87, 0x5F, 0xAF]),
    ('q', [0xAF, 0x5F, 0x87]),
    ('r', [0x5F, 0x87, 0xD7]),
    ('s', [0x5F, 0x5F, 0xD7]),
    ('t', [0x5F, 0xD7, 0x87]),
    ('u', [0x5F, 0xAF, 0xAF]),
    ('v', [0x5F, 0xAF, 0xD7]),
    ('w', [0x5F, 0x87, 0xFF]),
    ('x', [0x5F, 0xD7, 0xAF]),
    ('y', [0x5F, 0xAF, 0xFF]),
];
const COPILOT: &[&str] = &[
    "M19.245 5.364c1.322 1.36 1.877 3.216 2.11 5.817.622 0 1.2.135 1.592.654l.73.964c.21.278.323.61.323.955v2.62c0 .339-.173.669-.453.868C20.239 19.602 16.157 21.5 12 21.5c-4.6 0-9.205-2.583-11.547-4.258-.28-.2-.452-.53-.453-.868v-2.62c0-.345.113-.679.321-.956l.73-.963c.392-.517.974-.654 1.593-.654l.029-.297c.25-2.446.81-4.213 2.082-5.52 2.461-2.54 5.71-2.851 7.146-2.864h.198c1.436.013 4.685.323 7.146 2.864zm-7.244 4.328c-.284 0-.613.016-.962.05-.123.447-.305.85-.57 1.108-1.05 1.023-2.316 1.18-2.994 1.18-.638 0-1.306-.13-1.851-.464-.516.165-1.012.403-1.044.996a65.882 65.882 0 00-.063 2.884l-.002.48c-.002.563-.005 1.126-.013 1.69.002.326.204.63.51.765 2.482 1.102 4.83 1.657 6.99 1.657 2.156 0 4.504-.555 6.985-1.657a.854.854 0 00.51-.766c.03-1.682.006-3.372-.076-5.053-.031-.596-.528-.83-1.046-.996-.546.333-1.212.464-1.85.464-.677 0-1.942-.157-2.993-1.18-.266-.258-.447-.661-.57-1.108-.32-.032-.64-.049-.96-.05zm-2.525 4.013c.539 0 .976.426.976.95v1.753c0 .525-.437.95-.976.95a.964.964 0 01-.976-.95v-1.752c0-.525.437-.951.976-.951zm5 0c.539 0 .976.426.976.95v1.753c0 .525-.437.95-.976.95a.964.964 0 01-.976-.95v-1.752c0-.525.437-.951.976-.951zM7.635 5.087c-1.05.102-1.935.438-2.385.906-.975 1.037-.765 3.668-.21 4.224.405.394 1.17.657 1.995.657h.09c.649-.013 1.785-.176 2.73-1.11.435-.41.705-1.433.675-2.47-.03-.834-.27-1.52-.63-1.813-.39-.336-1.275-.482-2.265-.394zm6.465.394c-.36.292-.6.98-.63 1.813-.03 1.037.24 2.06.675 2.47.968.957 2.136 1.104 2.776 1.11h.044c.825 0 1.59-.263 1.995-.657.555-.556.765-3.187-.21-4.224-.45-.468-1.335-.804-2.385-.906-.99-.088-1.875.058-2.265.394zM12 7.615c-.24 0-.525.015-.84.044.03.16.045.336.06.526l-.001.159a2.94 2.94 0 01-.014.25c.225-.022.425-.027.612-.028h.366c.187 0 .387.006.612.028-.015-.146-.015-.277-.015-.409.015-.19.03-.365.06-.526a9.29 9.29 0 00-.84-.044z",
];
const CURSOR: &[&str] = &[
    "M22.106 5.68L12.5.135a.998.998 0 00-.998 0L1.893 5.68a.84.84 0 00-.419.726v11.186c0 .3.16.577.42.727l9.607 5.547a.999.999 0 00.998 0l9.608-5.547a.84.84 0 00.42-.727V6.407a.84.84 0 00-.42-.726zm-.603 1.176L12.228 22.92c-.063.108-.228.064-.228-.061V12.34a.59.59 0 00-.295-.51l-9.11-5.26c-.107-.062-.063-.228.062-.228h18.55c.264 0 .428.286.296.514z",
];
const QWEN: &[&str] = &[
    "M19.713 2.825h-5.907c0-.251-.066-.503-.196-.728L12.819.728A1.46 1.46 0 0 0 11.559 0h-1.58c-.52 0-1 .275-1.26.728l-2.42 4.19a1.45 1.45 0 0 0 0 1.453l.791 1.37c.26.448.74.727 1.26.727h11.36c.52 0 1-.275 1.26-.728l.79-1.37a1.45 1.45 0 0 0 0-1.451l-.79-1.37a1.46 1.46 0 0 0-1.26-.728zM.195 9.905l2.951 5.115a1.46 1.46 0 0 0-.531.531l-.791 1.37a1.45 1.45 0 0 0 0 1.452l.79 1.37c.26.448.74.727 1.26.727h4.84c.52 0 1-.275 1.26-.728l.79-1.37a1.45 1.45 0 0 0 0-1.451L5.086 7.083a1.46 1.46 0 0 0-1.26-.728h-1.58c-.52 0-1 .276-1.26.728l-.79 1.37a1.45 1.45 0 0 0 0 1.452m15.888 13.367 2.951-5.115c.217.125.469.196.728.196h1.582c.52 0 1-.275 1.26-.728l.79-1.37a1.45 1.45 0 0 0 0-1.451l-2.42-4.191a1.46 1.46 0 0 0-1.259-.728h-1.582c-.52 0-1 .276-1.259.728l-5.678 9.838a1.45 1.45 0 0 0 0 1.452l.79 1.37c.26.448.74.727 1.26.727h1.582c.52 0 1-.275 1.26-.728z",
];
const GOOSE: &[&str] = &[
    "M21.595 23.61c1.167-.254 2.405-.944 2.405-.944l-2.167-1.784a12.124 12.124 0 01-2.695-3.131 12.127 12.127 0 00-3.97-4.049l-.794-.462a1.115 1.115 0 01-.488-.815.844.844 0 01.154-.575c.413-.582 2.548-3.115 2.94-3.44.503-.416 1.065-.762 1.586-1.159.074-.056.148-.112.221-.17.003-.002.007-.004.009-.007.167-.131.325-.272.45-.438.453-.524.563-.988.59-1.193-.061-.197-.244-.639-.753-1.148.319.02.705.272 1.056.569.235-.376.481-.773.727-1.171.165-.266-.08-.465-.086-.471h-.001V3.22c-.007-.007-.206-.25-.471-.086-.567.35-1.134.702-1.639 1.021 0 0-.597-.012-1.305.599a2.464 2.464 0 00-.438.45l-.007.009c-.058.072-.114.147-.17.221-.397.521-.743 1.083-1.16 1.587-.323.391-2.857 2.526-3.44 2.94a.842.842 0 01-.574.153 1.115 1.115 0 01-.815-.488l-.462-.794a12.123 12.123 0 00-4.049-3.97 12.133 12.133 0 01-3.13-2.695L1.332 0S.643 1.238.39 2.405c.352.428 1.27 1.49 2.34 2.302C1.58 4.167.73 3.75.06 3.4c-.103.765-.063 1.92.043 2.816.726.317 1.961.806 3.219 1.066-1.006.236-2.11.278-2.961.262.15.554.358 1.119.64 1.688.119.263.25.52.39.77.452.125 2.222.383 3.164.171l-2.51.897a27.776 27.776 0 002.544 2.726c2.031-1.092 2.494-1.241 4.018-2.238-2.467 2.008-3.108 2.828-3.8 3.67l-.483.678c-.25.351-.469.725-.65 1.117-.61 1.31-1.47 4.1-1.47 4.1-.154.486.202.842.674.674 0 0 2.79-.861 4.1-1.47.392-.182.766-.4 1.118-.65l.677-.483c.227-.187.453-.37.701-.586 0 0 1.705 2.02 3.458 3.349l.896-2.511c-.211.942.046 2.712.17 3.163.252.142.509.272.772.392.569.28 1.134.49 1.688.64-.016-.853.026-1.956.261-2.962.26 1.258.75 2.493 1.067 3.219.895.106 2.051.146 2.816.043a73.87 73.87 0 01-1.308-2.67c.811 1.07 1.874 1.988 2.302 2.34h-.001z",
];
const OPENCODE: &[&str] = &["M16 6H8v12h8V6zm4 16H4V2h16v20z"];
const AMP: &[&str] = &[
    "M15.087 23.18L12.03 24l-2.097-7.823-5.738 5.738-2.251-2.251 5.718-5.719-7.769-2.082.82-3.057 11.294 3.08 3.08 11.295z",
    "M19.505 18.762l-3.057.82-2.564-9.573-9.572-2.564.819-3.057 11.295 3.079 3.08 11.295z",
    "M23.893 14.374l-3.057.82-2.565-9.572L8.7 3.057 9.52 0l11.295 3.08 3.079 11.294z",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todos_los_logos_se_leen_y_se_dibujan() {
        for logo in Logo::ALL {
            if let Art::Paths { paths, .. } = logo.art() {
                for d in paths {
                    assert!(parse_path(d).is_some(), "{logo:?}: {d}");
                }
            }
            let r = rasterize(logo, 28).unwrap_or_else(|| panic!("{logo:?}"));
            let (w, h) = (r.width as usize, r.height as usize);
            assert!((20..=32).contains(&w) && h <= 32, "{logo:?}: {w} × {h}");
            assert_eq!(r.rgba.len(), w * h * 4);
            let ink = r.rgba.chunks(4).filter(|p| p[3] > 128).count();
            // Ni vacío ni un cuadrado lleno: algo con forma.
            assert!(ink > w * h / 10 && ink < w * h * 9 / 10, "{logo:?}: {ink}");
            assert_eq!(r.is_color, logo.is_color());
        }
    }

    /// El pixel art sale nítido (cada píxel, lleno o vacío) a cualquier tamaño.
    #[test]
    fn pixel_art_nitido() {
        for logo in [Logo::ClaudeCode, Logo::OpenCode, Logo::GeminiCli] {
            for px in [16, 23, 27, 37] {
                let r = rasterize(logo, px).unwrap();
                let solid = r.rgba.chunks(4).all(|p| p[3] == 0 || p[3] == 255);
                assert!(solid, "{logo:?} a {px} px");
            }
        }
        // Gemini CLI para 23 px (14 px lógicos a 1,67): celdas de 2 px.
        let g = rasterize(Logo::GeminiCli, 23).unwrap();
        assert_eq!((g.width, g.height), (24, 20));
        // Claude Code a 23 px: brazos, ojos y patas iguales a los dos lados.
        let c = rasterize(Logo::ClaudeCode, 23).unwrap();
        let w = c.width as usize;
        let a = |x: usize, y: usize| c.rgba[(y * w + x) * 4 + 3];
        for y in 0..c.height as usize {
            for x in 0..w {
                assert_eq!(a(x, y), a(w - 1 - x, y), "({x}, {y})");
            }
        }
    }

    #[test]
    fn abreviaturas_de_los_trazados() {
        // Números pegados (`-.5.5`), órdenes repetidas sin letra y banderas de arco sin espacio.
        let p = parse_path("M0 0l1-.5.5.5h2v2H0zm4 4a2 2 0 014 0z").unwrap();
        let b = p.bounds();
        assert_eq!((b.left(), b.right()), (0.0, 8.0));
        assert!((b.top() + 0.5).abs() < 1e-3 && b.bottom() >= 4.0, "{b:?}");
        // El arco de (4, 4) a (8, 4) con radio 2 pasa por (6, 2): por arriba, con la bandera de
        // sentido a 1 (horario en pantalla).
        let semicirculo = parse_path("M4 4a2 2 0 014 0").unwrap().bounds();
        assert!((semicirculo.top() - 2.0).abs() < 0.01, "{semicirculo:?}");
        assert!(parse_path("M0 0 Z 3").is_none());
        assert!(parse_path("M0 0 X1 1").is_none());
    }
}
