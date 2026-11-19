//! Atlas de glifos: empaquetado por estanterías (shelf packing) en una textura RGBA.

use crate::font::{FontSet, RasterGlyph, Style};
use std::collections::HashMap;

pub const ATLAS_SIZE: u32 = 1024;

#[derive(Clone, Copy, Debug)]
pub struct AtlasGlyph {
    /// Rectángulo UV normalizado (x, y, w, h).
    pub uv: [f32; 4],
    pub width: f32,
    pub height: f32,
    pub left: f32,
    pub top: f32,
    pub is_color: bool,
}

struct Packer {
    x: u32,
    y: u32,
    row_h: u32,
}

impl Packer {
    fn new() -> Self {
        Self { x: 1, y: 1, row_h: 0 }
    }

    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w + 2 > ATLAS_SIZE || h + 2 > ATLAS_SIZE {
            return None;
        }
        if self.x + w + 1 > ATLAS_SIZE {
            self.x = 1;
            self.y += self.row_h + 1;
            self.row_h = 0;
        }
        if self.y + h + 1 > ATLAS_SIZE {
            return None;
        }
        let pos = (self.x, self.y);
        self.x += w + 1;
        self.row_h = self.row_h.max(h);
        Some(pos)
    }
}

/// Variantes del texto de la interfaz: mismo juego de fuentes a otra escala o grosor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiVariant {
    Normal,
    /// Títulos: negrita y algo más grande.
    Title,
    /// Texto secundario denso (vista previa de una pestaña).
    Small,
}

impl UiVariant {
    pub fn scale(self) -> f32 {
        match self {
            UiVariant::Normal => 1.0,
            UiVariant::Title => 1.22,
            UiVariant::Small => 0.88,
        }
    }

    fn style(self) -> Style {
        if self == UiVariant::Title { Style::Bold } else { Style::Regular }
    }

    fn id(self) -> u8 {
        match self {
            UiVariant::Normal => 1,
            UiVariant::Title => 2,
            UiVariant::Small => 3,
        }
    }
}

/// Caché de glifos: char -> glifo en atlas. Sube a la GPU solo la primera vez.
pub struct GlyphCache {
    /// Fuente monoespaciada del terminal.
    pub fonts: FontSet,
    /// Fuente proporcional de la interfaz (pestañas, paleta, ajustes).
    pub ui: FontSet,
    packer: Packer,
    /// (juego de fuentes 0=terminal 1..=3=interfaz por variante, slot, glifo, ancho doble)
    map: HashMap<(u8, u16, u16, bool), Option<AtlasGlyph>>,
    /// Avances de la fuente de interfaz por carácter y variante.
    ui_adv: HashMap<(char, u8), f32>,
    /// Se activa cuando el atlas se llenó y se vació: el frame debe repetirse.
    pub overflowed: bool,
}

impl GlyphCache {
    pub fn new(fonts: FontSet, ui: FontSet) -> Self {
        Self { fonts, ui, packer: Packer::new(), map: HashMap::new(), ui_adv: HashMap::new(), overflowed: false }
    }

    pub fn clear(&mut self) {
        self.packer = Packer::new();
        self.map.clear();
        self.ui_adv.clear();
    }

    /// Glifo de la rejilla del terminal.
    pub fn get(
        &mut self,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        ch: char,
        style: Style,
        wide: bool,
        emoji: bool,
    ) -> Option<AtlasGlyph> {
        let (slot, gid) = self.fonts.resolve(ch, style, emoji);
        let key = (0, slot, gid, wide);
        if let Some(g) = self.map.get(&key) {
            return *g;
        }
        let max_w = self.fonts.metrics.width * if wide { 2.0 } else { 1.0 };
        let raster = rasterize_with_fallback(&mut self.fonts, ch, slot, gid, max_w);
        let glyph = raster.and_then(|r| self.upload(queue, texture, r));
        self.map.insert(key, glyph);
        glyph
    }

    /// Glifo de la interfaz y su avance horizontal.
    pub fn get_ui(
        &mut self,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        ch: char,
        v: UiVariant,
    ) -> (Option<AtlasGlyph>, f32) {
        let emoji = ch as u32 >= 0x1F000;
        let (slot, gid) = self.ui.resolve(ch, v.style(), emoji);
        let adv = self.ui_advance(ch, v);
        let key = (v.id(), slot, gid, false);
        if let Some(g) = self.map.get(&key) {
            return (*g, adv);
        }
        // Mismas fuentes a otro tamaño: se cambia el tamaño solo durante el rasterizado.
        let base = self.ui.size_px;
        self.ui.size_px = base * v.scale();
        let max_w = self.ui.size_px * 1.3;
        let raster = rasterize_with_fallback(&mut self.ui, ch, slot, gid, max_w);
        self.ui.size_px = base;
        let glyph = raster.and_then(|r| self.upload(queue, texture, r));
        self.map.insert(key, glyph);
        (glyph, adv)
    }

    pub fn ui_advance(&mut self, ch: char, v: UiVariant) -> f32 {
        if let Some(&a) = self.ui_adv.get(&(ch, v.id())) {
            return a;
        }
        let emoji = ch as u32 >= 0x1F000;
        let (slot, gid) = self.ui.resolve(ch, v.style(), emoji);
        let a = if emoji { self.ui.size_px * 1.3 } else { self.ui.advance(slot, gid) } * v.scale();
        self.ui_adv.insert((ch, v.id()), a);
        a
    }

    fn upload(&mut self, queue: &wgpu::Queue, texture: &wgpu::Texture, r: RasterGlyph) -> Option<AtlasGlyph> {
        let pos = match self.packer.alloc(r.width, r.height) {
            Some(p) => p,
            None => {
                // Atlas lleno: se vacía y se reconstruye en el siguiente frame.
                self.clear();
                self.overflowed = true;
                self.packer.alloc(r.width, r.height)?
            }
        };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: pos.0, y: pos.1, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &r.rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(r.width * 4), rows_per_image: Some(r.height) },
            wgpu::Extent3d { width: r.width, height: r.height, depth_or_array_layers: 1 },
        );
        let s = ATLAS_SIZE as f32;
        Some(AtlasGlyph {
            uv: [pos.0 as f32 / s, pos.1 as f32 / s, r.width as f32 / s, r.height as f32 / s],
            width: r.width as f32,
            height: r.height as f32,
            left: r.left as f32,
            top: r.top as f32,
            is_color: r.is_color,
        })
    }
}

/// Si la fuente elegida declara el glifo pero no sabe dibujarlo, prueba las siguientes.
fn rasterize_with_fallback(fonts: &mut FontSet, ch: char, slot: u16, gid: u16, max_w: f32) -> Option<RasterGlyph> {
    let mut raster = fonts.rasterize(slot, gid, max_w);
    let mut cur = slot;
    while raster.is_none() && !ch.is_whitespace() && gid != 0 {
        let (s, g) = fonts.next_fallback(ch, cur)?;
        cur = s;
        raster = fonts.rasterize(s, g, max_w);
    }
    raster
}
