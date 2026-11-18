//! Renderizado de emoji a color COLRv1 (p.ej. Noto Color Emoji en Fedora), que swash no soporta.
//! skrifa recorre el grafo de pintura del glifo y tiny-skia lo rasteriza en CPU.
//! Solo se ejecuta la primera vez que aparece cada emoji: el resultado queda en el atlas.

use skrifa::color::{Brush, ColorGlyphFormat, ColorPainter, ColorStop, CompositeMode, Extend, Transform};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::types::BoundingBox;
use skrifa::{FontRef, GlyphId, MetadataProvider};
use tiny_skia as sk;

/// Glifo COLRv1 rasterizado: RGBA no premultiplicado.
pub struct ColorBitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn has_colrv1(font: &FontRef, gid: u16) -> bool {
    font.color_glyphs().get_with_format(GlyphId::new(gid as u32), ColorGlyphFormat::ColrV1).is_some()
}

/// Rasteriza el glifo para que quepa en `max_w` x `max_h` píxeles.
pub fn render(font: &FontRef, gid: u16, max_w: f32, max_h: f32) -> Option<ColorBitmap> {
    let glyph = font
        .color_glyphs()
        .get_with_format(GlyphId::new(gid as u32), ColorGlyphFormat::ColrV1)?;
    let upem = font.metrics(Size::unscaled(), LocationRef::default()).units_per_em as f32;
    let bbox = glyph.bounding_box(LocationRef::default(), Size::unscaled()).unwrap_or(BoundingBox {
        x_min: 0.0,
        y_min: -0.2 * upem,
        x_max: upem,
        y_max: 0.8 * upem,
    });
    let (bw, bh) = (bbox.x_max - bbox.x_min, bbox.y_max - bbox.y_min);
    if bw <= 0.0 || bh <= 0.0 {
        return None;
    }
    let s = (max_w / bw).min(max_h / bh);
    let (w, h) = ((bw * s).ceil().max(1.0) as u32, (bh * s).ceil().max(1.0) as u32);

    // Unidades de fuente (Y hacia arriba) -> píxeles (Y hacia abajo).
    let base = sk::Transform::from_row(s, 0.0, 0.0, -s, -bbox.x_min * s, bbox.y_max * s);
    let palette: Vec<sk::Color> = font
        .color_palettes()
        .get(0)
        .map(|p| p.colors().iter().map(|c| sk::Color::from_rgba8(c.red, c.green, c.blue, c.alpha)).collect())
        .unwrap_or_default();

    let mut p = Painter {
        font,
        w,
        h,
        transforms: vec![base],
        clips: Vec::new(),
        layers: vec![sk::Pixmap::new(w, h)?],
        modes: Vec::new(),
        palette,
    };
    glyph.paint(LocationRef::default(), &mut p).ok()?;
    let pix = p.layers.pop()?;

    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for px in pix.pixels() {
        let c = px.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Some(ColorBitmap { width: w, height: h, rgba })
}

struct Painter<'a> {
    font: &'a FontRef<'a>,
    w: u32,
    h: u32,
    transforms: Vec<sk::Transform>,
    clips: Vec<sk::Mask>,
    layers: Vec<sk::Pixmap>,
    /// Modo de composición de cada capa abierta (paralelo a `layers[1..]`).
    modes: Vec<CompositeMode>,
    palette: Vec<sk::Color>,
}

/// Convierte el contorno de un glifo en un `Path` de tiny-skia (en unidades de fuente).
struct PathPen(sk::PathBuilder);

impl OutlinePen for PathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y);
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.quad_to(cx0, cy0, x, y);
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.cubic_to(cx0, cy0, cx1, cy1, x, y);
    }
    fn close(&mut self) {
        self.0.close();
    }
}

impl Painter<'_> {
    fn transform(&self) -> sk::Transform {
        *self.transforms.last().unwrap()
    }

    fn color(&self, palette_index: u16, alpha: f32) -> sk::Color {
        // 0xFFFF = "color del texto": en emoji casi no se usa.
        let mut c = self.palette.get(palette_index as usize).copied().unwrap_or(sk::Color::BLACK);
        c.apply_opacity(alpha);
        c
    }

    fn stops(&self, stops: &[ColorStop]) -> Vec<sk::GradientStop> {
        stops.iter().map(|s| sk::GradientStop::new(s.offset, self.color(s.palette_index, s.alpha))).collect()
    }

    fn push_mask(&mut self, path: Option<sk::Path>) {
        let t = self.transform();
        let mask = match (path, self.clips.last()) {
            (Some(path), Some(prev)) => {
                let mut m = prev.clone();
                m.intersect_path(&path, sk::FillRule::Winding, true, t);
                m
            }
            (Some(path), None) => {
                let mut m = sk::Mask::new(self.w, self.h).unwrap();
                m.fill_path(&path, sk::FillRule::Winding, true, t);
                m
            }
            // Contorno vacío: recorta todo.
            (None, _) => sk::Mask::new(self.w, self.h).unwrap(),
        };
        self.clips.push(mask);
    }
}

fn spread(e: Extend) -> sk::SpreadMode {
    match e {
        Extend::Repeat => sk::SpreadMode::Repeat,
        Extend::Reflect => sk::SpreadMode::Reflect,
        _ => sk::SpreadMode::Pad,
    }
}

fn blend(mode: CompositeMode) -> sk::BlendMode {
    use sk::BlendMode as B;
    use CompositeMode as C;
    match mode {
        C::Clear => B::Clear,
        C::Src => B::Source,
        C::Dest => B::Destination,
        C::DestOver => B::DestinationOver,
        C::SrcIn => B::SourceIn,
        C::DestIn => B::DestinationIn,
        C::SrcOut => B::SourceOut,
        C::DestOut => B::DestinationOut,
        C::SrcAtop => B::SourceAtop,
        C::DestAtop => B::DestinationAtop,
        C::Xor => B::Xor,
        C::Plus => B::Plus,
        C::Screen => B::Screen,
        C::Overlay => B::Overlay,
        C::Darken => B::Darken,
        C::Lighten => B::Lighten,
        C::ColorDodge => B::ColorDodge,
        C::ColorBurn => B::ColorBurn,
        C::HardLight => B::HardLight,
        C::SoftLight => B::SoftLight,
        C::Difference => B::Difference,
        C::Exclusion => B::Exclusion,
        C::Multiply => B::Multiply,
        C::HslHue => B::Hue,
        C::HslSaturation => B::Saturation,
        C::HslColor => B::Color,
        C::HslLuminosity => B::Luminosity,
        _ => B::SourceOver,
    }
}

impl ColorPainter for Painter<'_> {
    fn push_transform(&mut self, t: Transform) {
        let m = sk::Transform::from_row(t.xx, t.yx, t.xy, t.yy, t.dx, t.dy);
        let cur = self.transform().pre_concat(m);
        self.transforms.push(cur);
    }

    fn pop_transform(&mut self) {
        if self.transforms.len() > 1 {
            self.transforms.pop();
        }
    }

    fn push_clip_glyph(&mut self, glyph_id: GlyphId) {
        let mut pen = PathPen(sk::PathBuilder::new());
        if let Some(outline) = self.font.outline_glyphs().get(glyph_id) {
            let _ = outline.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut pen);
        }
        let path = pen.0.finish();
        self.push_mask(path);
    }

    fn push_clip_box(&mut self, b: BoundingBox<f32>) {
        let path = sk::Rect::from_ltrb(b.x_min, b.y_min, b.x_max, b.y_max).map(sk::PathBuilder::from_rect);
        self.push_mask(path);
    }

    fn pop_clip(&mut self) {
        self.clips.pop();
    }

    fn fill(&mut self, brush: Brush<'_>) {
        let t = self.transform();
        let shader = match brush {
            Brush::Solid { palette_index, alpha } => sk::Shader::SolidColor(self.color(palette_index, alpha)),
            Brush::LinearGradient { p0, p1, color_stops, extend } => {
                if color_stops.len() == 1 {
                    sk::Shader::SolidColor(self.color(color_stops[0].palette_index, color_stops[0].alpha))
                } else {
                    match sk::LinearGradient::new(
                        sk::Point::from_xy(p0.x, p0.y),
                        sk::Point::from_xy(p1.x, p1.y),
                        self.stops(color_stops),
                        spread(extend),
                        t,
                    ) {
                        Some(s) => s,
                        None => return,
                    }
                }
            }
            Brush::RadialGradient { c0, r0, c1, r1, color_stops, extend } => {
                // tiny-skia solo admite radio inicial 0: con círculos concéntricos se reajustan las paradas.
                let mut stops = self.stops(color_stops);
                let (start, end, radius) = if r0.abs() < 1e-3 {
                    (c0, c1, r1)
                } else if r1 > 0.0 {
                    let k = (r0 / r1).clamp(0.0, 1.0);
                    stops = color_stops
                        .iter()
                        .map(|s| sk::GradientStop::new(k + s.offset * (1.0 - k), self.color(s.palette_index, s.alpha)))
                        .collect();
                    (c1, c1, r1)
                } else {
                    (c0, c0, r0.abs())
                };
                match sk::RadialGradient::new(
                    sk::Point::from_xy(start.x, start.y),
                    sk::Point::from_xy(end.x, end.y),
                    radius,
                    stops,
                    spread(extend),
                    t,
                ) {
                    Some(s) => s,
                    None => sk::Shader::SolidColor(self.color(color_stops[0].palette_index, color_stops[0].alpha)),
                }
            }
            Brush::SweepGradient { color_stops, .. } => {
                // tiny-skia no tiene gradiente cónico: se usa el color central (muy raro en emoji).
                let s = &color_stops[color_stops.len() / 2];
                sk::Shader::SolidColor(self.color(s.palette_index, s.alpha))
            }
        };
        let paint = sk::Paint { shader, anti_alias: true, ..Default::default() };
        let rect = sk::Rect::from_xywh(0.0, 0.0, self.w as f32, self.h as f32).unwrap();
        let mask = self.clips.last();
        if let Some(layer) = self.layers.last_mut() {
            layer.fill_rect(rect, &paint, sk::Transform::identity(), mask);
        }
    }

    fn push_layer(&mut self, mode: CompositeMode) {
        if let Some(pix) = sk::Pixmap::new(self.w, self.h) {
            self.layers.push(pix);
            self.modes.push(mode);
        }
    }

    fn pop_layer(&mut self) {
        if self.layers.len() < 2 {
            return;
        }
        let top = self.layers.pop().unwrap();
        let mode = self.modes.pop().unwrap_or(CompositeMode::SrcOver);
        let paint = sk::PixmapPaint { blend_mode: blend(mode), ..Default::default() };
        if let Some(below) = self.layers.last_mut() {
            below.draw_pixmap(0, 0, top.as_ref(), &paint, sk::Transform::identity(), None);
        }
    }
}
