//! Fuentes: descubrimiento con fontdb (una sola vez al arrancar) y rasterizado con swash.
//! Los archivos se mapean en memoria (mmap) y los fallbacks se abren solo cuando hacen falta,
//! así una fuente de emoji de 10 MB no cuesta nada hasta que aparece un emoji.

use memmap2::Mmap;
use rustc_hash::FxHashMap;
use std::fs::File;
use std::path::PathBuf;
use swash::scale::{image::Content, Render, ScaleContext, Source, StrikeWith};
use swash::zeno::Format;
use swash::{CacheKey, FontRef};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Style {
    Regular = 0,
    Bold = 1,
    Italic = 2,
    BoldItalic = 3,
}

struct LoadedFont {
    data: Mmap,
    index: u32,
    offset: u32,
    key: CacheKey,
    /// Fuente de emoji a color (COLR, CBDT o sbix).
    color: bool,
}

impl LoadedFont {
    fn open(path: &PathBuf, index: u32) -> Option<Self> {
        let file = File::open(path).ok()?;
        // SAFETY: los archivos de fuentes del sistema no se modifican mientras se usan.
        let data = unsafe { Mmap::map(&file).ok()? };
        let f = FontRef::from_index(&data, index as usize)?;
        let (offset, key) = (f.offset, f.key);
        let color = [b"COLR", b"CBDT", b"sbix"]
            .iter()
            .any(|t| f.table(swash::tag_from_bytes(t)).is_some());
        Some(Self {
            data,
            index,
            offset,
            key,
            color,
        })
    }

    fn as_ref(&self) -> FontRef<'_> {
        FontRef {
            data: &self.data,
            offset: self.offset,
            key: self.key,
        }
    }
}

/// Slot de fuente: ruta resuelta + carga perezosa.
struct Slot {
    path: PathBuf,
    index: u32,
    font: Option<Option<LoadedFont>>, // None = no intentado; Some(None) = falló
}

impl Slot {
    fn get(&mut self) -> Option<&LoadedFont> {
        if self.font.is_none() {
            self.font = Some(LoadedFont::open(&self.path, self.index));
        }
        self.font.as_ref().unwrap().as_ref()
    }
}

/// Un glifo rasterizado listo para subir al atlas.
pub struct RasterGlyph {
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    pub is_color: bool,
    /// RGBA8 (para máscaras: blanco con alfa = cobertura).
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub struct CellMetrics {
    pub width: f32,
    pub height: f32,
    /// Distancia desde el borde superior de la celda a la línea base.
    pub baseline: f32,
    pub underline_pos: f32,
    pub stroke: f32,
}

pub struct FontSet {
    /// 0..4 = estilos de la familia principal, 4.. = fallbacks.
    slots: Vec<Slot>,
    ctx: ScaleContext,
    pub size_px: f32,
    pub metrics: CellMetrics,
    line_height: f32,
    lookup: FxHashMap<(char, Style, bool), (u16, u16)>,
}

impl FontSet {
    /// Índice de las fuentes del sistema. Se crea al arrancar (o al cambiar de fuente) y se
    /// suelta en cuanto se resuelven las rutas.
    pub fn system_db() -> fontdb::Database {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db
    }

    /// `mono = false` para la fuente proporcional de la interfaz (pestañas, paleta, ajustes).
    pub fn from_db(
        db: &fontdb::Database,
        family: &str,
        fallbacks: &[String],
        size_px: f32,
        line_height: f32,
        mono: bool,
    ) -> Self {
        let resolve = |families: &[fontdb::Family], weight, style| -> Option<(PathBuf, u32)> {
            let id = db.query(&fontdb::Query {
                families,
                weight,
                stretch: fontdb::Stretch::Normal,
                style,
            })?;
            match db.face_source(id)? {
                (fontdb::Source::File(p), i) | (fontdb::Source::SharedFile(p, _), i) => {
                    Some((p, i))
                }
                _ => None,
            }
        };

        // fontdb no conoce la monoespaciada del sistema (por defecto asume "Courier New"):
        // probamos la familia pedida, luego los fallbacks monoespaciados y por último cualquiera.
        let has_mono = |name: &str| {
            db.faces().any(|f| {
                f.monospaced && f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
            })
        };
        let exists = |name: &str| {
            db.faces()
                .any(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)))
        };
        let chosen: String = if exists(family) {
            family.to_string()
        } else {
            let alt = fallbacks
                .iter()
                .find(|n| if mono { has_mono(n) } else { exists(n) })
                .cloned()
                .or_else(|| {
                    db.faces()
                        .find(|f| f.monospaced == mono)
                        .and_then(|f| f.families.first().map(|(n, _)| n.clone()))
                })
                .expect("No se encontró ninguna fuente utilizable en el sistema");
            log::warn!("Fuente '{family}' no encontrada; usando '{alt}'");
            alt
        };
        let fams = [fontdb::Family::Name(&chosen)];
        let mut slots = Vec::new();
        let combos = [
            (fontdb::Weight::NORMAL, fontdb::Style::Normal),
            (fontdb::Weight::BOLD, fontdb::Style::Normal),
            (fontdb::Weight::NORMAL, fontdb::Style::Italic),
            (fontdb::Weight::BOLD, fontdb::Style::Italic),
        ];
        for (w, s) in combos {
            let (path, index) = resolve(&fams, w, s)
                .or_else(|| slots.first().map(|s: &Slot| (s.path.clone(), s.index)))
                .expect("No se encontró ninguna fuente monoespaciada en el sistema");
            slots.push(Slot {
                path,
                index,
                font: None,
            });
        }
        log::info!("Fuente: {chosen} ({})", slots[0].path.display());
        for name in fallbacks
            .iter()
            .filter(|n| !n.eq_ignore_ascii_case(&chosen))
        {
            let q = [fontdb::Family::Name(name.as_str())];
            // Solo aceptamos coincidencia exacta de familia para fallbacks.
            let exact = db.query(&fontdb::Query {
                families: &q,
                ..Default::default()
            });
            if exact.is_some() {
                if let Some((path, index)) =
                    resolve(&q, fontdb::Weight::NORMAL, fontdb::Style::Normal)
                {
                    slots.push(Slot {
                        path,
                        index,
                        font: None,
                    });
                }
            }
        }

        let mut set = Self {
            slots,
            ctx: ScaleContext::new(),
            size_px,
            metrics: CellMetrics {
                width: 1.0,
                height: 1.0,
                baseline: 1.0,
                underline_pos: 1.0,
                stroke: 1.0,
            },
            line_height,
            lookup: FxHashMap::default(),
        };
        set.metrics = set.compute_metrics(line_height);
        log::info!("Celda: {:?} ({size_px:.1} px)", set.metrics);
        set
    }

    /// Cambia el tamaño en píxeles (zoom o cambio de escala HiDPI) sin re-escanear fuentes.
    pub fn set_size(&mut self, size_px: f32) {
        self.size_px = size_px;
        self.metrics = self.compute_metrics(self.line_height);
    }

    pub fn set_line_height(&mut self, line_height: f32) {
        self.line_height = line_height;
        self.metrics = self.compute_metrics(line_height);
    }

    /// Avance horizontal de un glifo (texto proporcional de la interfaz).
    pub fn advance(&mut self, slot: u16, gid: u16) -> f32 {
        let size = self.size_px;
        match self.slots[slot as usize].get() {
            Some(f) => f.as_ref().glyph_metrics(&[]).scale(size).advance_width(gid),
            None => size * 0.5,
        }
    }

    /// Familias monoespaciadas instaladas (para el selector de Ajustes).
    pub fn monospace_families() -> Vec<String> {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let mut v: Vec<String> = db
            .faces()
            .filter(|f| f.monospaced)
            .filter_map(|f| f.families.first().map(|(n, _)| n.clone()))
            .filter(|n| !n.contains("Emoji"))
            .collect();
        v.sort_by_key(|n| n.to_lowercase());
        v.dedup();
        v
    }

    fn compute_metrics(&mut self, line_height: f32) -> CellMetrics {
        let size = self.size_px;
        let f = self.slots[0]
            .get()
            .expect("fuente principal ilegible")
            .as_ref();
        let m = f.metrics(&[]).scale(size);
        let gm = f.glyph_metrics(&[]).scale(size);
        let gid = f.charmap().map('M');
        let width = gm.advance_width(gid).round().max(1.0);
        // Como VTE/GNOME Terminal: altura = ascendente + descendente (sin el "leading" extra).
        let natural = m.ascent + m.descent.abs();
        let height = (natural * line_height).round().max(1.0);
        let baseline = ((height - (m.ascent + m.descent.abs())) / 2.0 + m.ascent).round();
        let stroke = (size / 14.0).round().max(1.0);
        let underline_pos = (baseline + (m.descent.abs() * 0.5).max(stroke)).min(height - stroke);
        CellMetrics {
            width,
            height,
            baseline,
            underline_pos,
            stroke,
        }
    }

    /// Resuelve un carácter a (slot, glyph_id) probando estilo -> regular -> fallbacks.
    /// Con `emoji`, las fuentes de color van primero (si no, una fuente de símbolos
    /// monocroma anterior en la lista "ganaría" a Noto Color Emoji).
    pub fn resolve(&mut self, ch: char, style: Style, emoji: bool) -> (u16, u16) {
        if let Some(&r) = self.lookup.get(&(ch, style, emoji)) {
            return r;
        }
        let mut found = None;
        if emoji {
            for i in 4..self.slots.len() {
                if let Some(f) = self.slots[i].get().filter(|f| f.color) {
                    let gid = f.as_ref().charmap().map(ch);
                    if gid != 0 {
                        found = Some((i as u16, gid));
                        break;
                    }
                }
            }
        }
        if found.is_none() {
            let mut order = vec![style as usize];
            if style != Style::Regular {
                order.push(0);
            }
            order.extend(4..self.slots.len());
            for i in order {
                if let Some(f) = self.slots[i].get() {
                    let gid = f.as_ref().charmap().map(ch);
                    if gid != 0 {
                        found = Some((i as u16, gid));
                        break;
                    }
                }
            }
        }
        let found = found.unwrap_or((0, 0));
        self.lookup.insert((ch, style, emoji), found);
        found
    }

    /// Siguiente fuente (después de `after`) que tenga el carácter. Se usa cuando una fuente
    /// lo declara pero no puede dibujarlo (p.ej. emoji COLRv1, que swash no soporta).
    pub fn next_fallback(&mut self, ch: char, after: u16) -> Option<(u16, u16)> {
        let start = (after as usize + 1).max(4);
        for i in start..self.slots.len() {
            if let Some(f) = self.slots[i].get() {
                let gid = f.as_ref().charmap().map(ch);
                if gid != 0 {
                    return Some((i as u16, gid));
                }
            }
        }
        None
    }

    /// Rasteriza un glifo. `max_w` limita el ancho (1 o 2 celdas) para emoji en color.
    pub fn rasterize(&mut self, slot: u16, gid: u16, max_w: f32) -> Option<RasterGlyph> {
        let size = self.size_px;
        let m = self.metrics;
        let loaded = self.slots[slot as usize].get()?;
        // Emoji COLRv1 (Noto Color Emoji en Fedora): swash no lo soporta, lo pintamos nosotros.
        if let Ok(sk) = skrifa::FontRef::from_index(&loaded.data, loaded.index) {
            if crate::colr::has_colrv1(&sk, gid) {
                let bmp = crate::colr::render(&sk, gid, max_w, (m.height * 0.95).floor())?;
                let top = (m.baseline - (m.height - bmp.height as f32) / 2.0).round() as i32;
                return Some(RasterGlyph {
                    width: bmp.width,
                    height: bmp.height,
                    left: 0,
                    top,
                    is_color: true,
                    rgba: bmp.rgba,
                });
            }
        }
        let font = loaded.as_ref();
        let mut scaler = self.ctx.builder(font).size(size).hint(true).build();
        let img = Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ])
        .format(Format::Alpha)
        .render(&mut scaler, gid)?;

        let p = img.placement;
        if p.width == 0 || p.height == 0 {
            return None;
        }
        match img.content {
            Content::Color => {
                // Los emoji bitmap vienen grandes (p.ej. 136 px): se escalan a la celda.
                let scale = (max_w / p.width as f32)
                    .min(m.height / p.height as f32)
                    .min(1.0);
                let (w, h) = (
                    ((p.width as f32 * scale).round() as u32).max(1),
                    ((p.height as f32 * scale).round() as u32).max(1),
                );
                let rgba = if scale < 1.0 {
                    downscale(&img.data, p.width, p.height, w, h)
                } else {
                    img.data
                };
                let top = if scale < 1.0 {
                    // centrado vertical en la celda
                    (m.baseline - (m.height - h as f32) / 2.0).round() as i32
                } else {
                    p.top
                };
                Some(RasterGlyph {
                    width: w,
                    height: h,
                    left: 0,
                    top,
                    is_color: true,
                    rgba,
                })
            }
            _ => {
                let mut rgba = Vec::with_capacity(img.data.len() * 4);
                for &a in &img.data {
                    rgba.extend_from_slice(&[255, 255, 255, a]);
                }
                Some(RasterGlyph {
                    width: p.width,
                    height: p.height,
                    left: p.left,
                    top: p.top,
                    is_color: false,
                    rgba,
                })
            }
        }
    }
}

/// Reducción por promedio de área (box filter) RGBA8.
fn downscale(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    let (fx, fy) = (sw as f32 / dw as f32, sh as f32 / dh as f32);
    for y in 0..dh {
        let y0 = (y as f32 * fy) as u32;
        let y1 = (((y + 1) as f32 * fy).ceil() as u32).min(sh).max(y0 + 1);
        for x in 0..dw {
            let x0 = (x as f32 * fx) as u32;
            let x1 = (((x + 1) as f32 * fx).ceil() as u32).min(sw).max(x0 + 1);
            let mut acc = [0u32; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = ((sy * sw + sx) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += src[i + c] as u32;
                    }
                }
            }
            let n = (x1 - x0) * (y1 - y0);
            let o = ((y * dw + x) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (acc[c] / n) as u8;
            }
        }
    }
    out
}
