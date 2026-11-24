//! Paneles divididos: cada pestaña de la barra (`Group`) reparte su espacio entre uno o varios
//! terminales con un árbol de divisiones. Cada hoja es el `id` de un terminal (`terminal::Tab`).
//!
//! El resto de Garbo sigue trabajando con "el terminal activo" (`State::active`): es el panel con
//! el foco de la pestaña activa.

/// Rectángulo en píxeles: x, y, ancho, alto.
pub type Rect = [f32; 4];

/// Separación entre paneles (en píxeles, ya escalada): ahí va la línea divisoria.
pub fn gap(scale: f32) -> f32 {
    (9.0 * scale).round()
}

#[derive(Clone, Debug, PartialEq)]
pub enum Layout {
    Leaf(u64),
    /// `side`: uno al lado del otro (si no, uno encima del otro). `ratio`: parte del primero.
    Split {
        side: bool,
        ratio: f32,
        a: Box<Layout>,
        b: Box<Layout>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Línea divisoria: su rectángulo (la separación), el área que reparte y dónde está en el árbol.
#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    pub rect: Rect,
    pub area: Rect,
    pub side: bool,
    /// Camino desde la raíz (`false` = primer hijo).
    pub path: Vec<bool>,
}

pub struct Group {
    pub layout: Layout,
    /// Terminal con el foco.
    pub focus: u64,
    /// Nombre puesto a mano (`None`: el automático, lo que se hace en el panel con el foco).
    pub name: Option<String>,
}

impl Group {
    pub fn new(id: u64) -> Self {
        Self {
            layout: Layout::Leaf(id),
            focus: id,
            name: None,
        }
    }
}

fn split_rect(r: Rect, side: bool, ratio: f32, gap: f32) -> (Rect, Rect, Rect) {
    let [x, y, w, h] = r;
    if side {
        let wa = ((w - gap) * ratio).round();
        (
            [x, y, wa, h],
            [x + wa + gap, y, w - wa - gap, h],
            [x + wa, y, gap, h],
        )
    } else {
        let ha = ((h - gap) * ratio).round();
        (
            [x, y, w, ha],
            [x, y + ha + gap, w, h - ha - gap],
            [x, y + ha, w, gap],
        )
    }
}

impl Layout {
    pub fn leaves(&self) -> Vec<u64> {
        match self {
            Layout::Leaf(id) => vec![*id],
            Layout::Split { a, b, .. } => {
                let mut v = a.leaves();
                v.extend(b.leaves());
                v
            }
        }
    }

    pub fn contains(&self, id: u64) -> bool {
        match self {
            Layout::Leaf(l) => *l == id,
            Layout::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    /// Rectángulo de cada terminal dentro de `area`.
    pub fn rects(&self, area: Rect, gap: f32) -> Vec<(u64, Rect)> {
        match self {
            Layout::Leaf(id) => vec![(*id, area)],
            Layout::Split { side, ratio, a, b } => {
                let (ra, rb, _) = split_rect(area, *side, *ratio, gap);
                let mut v = a.rects(ra, gap);
                v.extend(b.rects(rb, gap));
                v
            }
        }
    }

    pub fn dividers(&self, area: Rect, gap: f32) -> Vec<Divider> {
        let mut out = Vec::new();
        self.collect_dividers(area, gap, &mut Vec::new(), &mut out);
        out
    }

    fn collect_dividers(&self, area: Rect, gap: f32, path: &mut Vec<bool>, out: &mut Vec<Divider>) {
        if let Layout::Split { side, ratio, a, b } = self {
            let (ra, rb, rd) = split_rect(area, *side, *ratio, gap);
            out.push(Divider {
                rect: rd,
                area,
                side: *side,
                path: path.clone(),
            });
            path.push(false);
            a.collect_dividers(ra, gap, path, out);
            path.pop();
            path.push(true);
            b.collect_dividers(rb, gap, path, out);
            path.pop();
        }
    }

    /// Parte el panel `target` en dos; el nuevo (`new`) va a la derecha o debajo.
    pub fn split(&mut self, target: u64, new: u64, side: bool) -> bool {
        match self {
            Layout::Leaf(id) if *id == target => {
                *self = Layout::Split {
                    side,
                    ratio: 0.5,
                    a: Box::new(Layout::Leaf(target)),
                    b: Box::new(Layout::Leaf(new)),
                };
                true
            }
            Layout::Leaf(_) => false,
            Layout::Split { a, b, .. } => a.split(target, new, side) || b.split(target, new, side),
        }
    }

    /// Quita el panel `id`; su hermano ocupa su sitio. `None` si no queda nada.
    pub fn remove(self, id: u64) -> Option<Layout> {
        match self {
            Layout::Leaf(l) if l == id => None,
            leaf @ Layout::Leaf(_) => Some(leaf),
            Layout::Split { side, ratio, a, b } => match (a.remove(id), b.remove(id)) {
                (Some(a), Some(b)) => Some(Layout::Split {
                    side,
                    ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            },
        }
    }

    /// Cambia la proporción de la división en `path` (al arrastrar su línea).
    pub fn set_ratio(&mut self, path: &[bool], value: f32) {
        match (self, path.split_first()) {
            (Layout::Split { ratio, .. }, None) => *ratio = value.clamp(0.1, 0.9),
            (Layout::Split { a, b, .. }, Some((first, rest))) => {
                if *first {
                    b.set_ratio(rest, value)
                } else {
                    a.set_ratio(rest, value)
                }
            }
            _ => {}
        }
    }
}

/// El panel vecino de `from` en la dirección `dir` (el más cercano que se solapa con él).
pub fn neighbor(rects: &[(u64, Rect)], from: u64, dir: Dir) -> Option<u64> {
    let [fx, fy, fw, fh] = rects.iter().find(|(id, _)| *id == from)?.1;
    let (fcx, fcy) = (fx + fw / 2.0, fy + fh / 2.0);
    rects
        .iter()
        .filter(|(id, _)| *id != from)
        .filter_map(|(id, [x, y, w, h])| {
            let overlap_v = y < &(fy + fh) && y + h > fy;
            let overlap_h = x < &(fx + fw) && x + w > fx;
            let dist = match dir {
                Dir::Left if overlap_v && x + w <= fx + 1.0 => fx - (x + w),
                Dir::Right if overlap_v && *x >= fx + fw - 1.0 => x - (fx + fw),
                Dir::Up if overlap_h && y + h <= fy + 1.0 => fy - (y + h),
                Dir::Down if overlap_h && *y >= fy + fh - 1.0 => y - (fy + fh),
                _ => return None,
            };
            // A igual distancia, el más alineado con el centro del panel de partida.
            let off = match dir {
                Dir::Left | Dir::Right => (y + h / 2.0 - fcy).abs(),
                Dir::Up | Dir::Down => (x + w / 2.0 - fcx).abs(),
            };
            Some((dist, off, *id))
        })
        .min_by(|a, b| {
            (a.0, a.1)
                .partial_cmp(&(b.0, b.1))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(_, _, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partir_y_quitar() {
        let mut l = Layout::Leaf(1);
        assert!(l.split(1, 2, true));
        assert!(l.split(2, 3, false));
        assert_eq!(l.leaves(), vec![1, 2, 3]);
        // 1 a la izquierda; 2 arriba a la derecha y 3 debajo.
        let r = l.rects([0.0, 0.0, 1009.0, 609.0], 9.0);
        assert_eq!(r[0], (1, [0.0, 0.0, 500.0, 609.0]));
        assert_eq!(r[1], (2, [509.0, 0.0, 500.0, 300.0]));
        assert_eq!(r[2], (3, [509.0, 309.0, 500.0, 300.0]));
        let d = l.dividers([0.0, 0.0, 1009.0, 609.0], 9.0);
        assert_eq!(d.len(), 2);
        assert_eq!(
            (d[0].rect, d[0].side, d[0].path.clone()),
            ([500.0, 0.0, 9.0, 609.0], true, vec![])
        );
        assert_eq!(d[1].path, vec![true]);

        assert_eq!(
            neighbor(&r, 1, Dir::Right),
            Some(2),
            "a igual distancia, el más alto (empate: el primero)"
        );
        assert_eq!(neighbor(&r, 3, Dir::Left), Some(1));
        assert_eq!(neighbor(&r, 3, Dir::Up), Some(2));
        assert_eq!(neighbor(&r, 2, Dir::Down), Some(3));
        assert_eq!(neighbor(&r, 1, Dir::Left), None);

        l.set_ratio(&[true], 0.25);
        let r = l.rects([0.0, 0.0, 1009.0, 609.0], 9.0);
        assert_eq!(r[1].1[3], 150.0);

        let l = l.remove(2).unwrap();
        assert_eq!(l.leaves(), vec![1, 3]);
        assert_eq!(
            l.rects([0.0, 0.0, 1009.0, 609.0], 9.0)[1],
            (3, [509.0, 0.0, 500.0, 609.0]),
            "3 ocupa el sitio de su rama"
        );
        let l = l.remove(1).unwrap();
        assert_eq!(l, Layout::Leaf(3));
        assert_eq!(l.remove(3), None);
    }
}
