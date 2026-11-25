//! Medidas de la interfaz de la ventana ([[0041 Interfaz Flotante]]): con la Flotante, el
//! terminal y los paneles son tarjetas que flotan sobre el fondo de la ventana, con margen,
//! huecos, esquinas y sombra; con la Clásica todo vale cero y la ventana se reparte como siempre.
//! El dibujo lee estas medidas en vez de preguntar por la interfaz en cada sitio.

use crate::style::{Interface, Style};

/// Medidas de la interfaz en píxeles físicos.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chrome {
    /// ¿Interfaz Flotante?
    pub floating: bool,
    /// Margen entre el borde de la ventana y las tarjetas (salvo arriba: ahí está la barra).
    pub margin: f32,
    /// Hueco entre tarjetas (terminal, paneles, paneles divididos).
    pub gap: f32,
    /// Radio de las tarjetas grandes: terminal y paneles laterales.
    pub radius: f32,
    /// Radio de lo pequeño: menús, filas, campos.
    pub small: f32,
    /// Radio de las hojas: paleta y Ajustes.
    pub sheet: f32,
    /// Cuánto se oscurece el fondo bajo una hoja (0 = nada).
    pub dim: f32,
}

impl Chrome {
    pub fn new(style: &Style, scale: f32) -> Self {
        let px = |v: f32| (v * scale).round();
        // Las esquinas siguen a "Esquinas" (`Style::radius`): las tarjetas, algo más redondas que
        // lo de dentro; con 0, todo en ángulo recto.
        let r = style.radius as f32;
        let more = |k: f32| if r > 0.0 { px(r + k) } else { 0.0 };
        match style.interface {
            Interface::Floating => Self {
                floating: true,
                margin: px(10.0),
                gap: px(10.0),
                radius: more(4.0),
                small: px(r),
                sheet: more(6.0),
                dim: 0.45,
            },
            Interface::Classic => Self::classic(),
        }
    }

    /// Hueco entre paneles divididos: el de las tarjetas con la Flotante; con la Clásica, el de
    /// siempre (donde va la línea divisoria).
    pub fn pane_gap(&self, scale: f32) -> f32 {
        if self.floating {
            self.gap
        } else {
            crate::panes::gap(scale)
        }
    }

    fn classic() -> Self {
        Self {
            floating: false,
            margin: 0.0,
            gap: 0.0,
            radius: 0.0,
            small: 0.0,
            sheet: 0.0,
            dim: 0.0,
        }
    }
}

/// Un rectángulo (x, y, ancho, alto).
pub type Rect = (f32, f32, f32, f32);

/// Lo que ocupan la barra o la zona de las pestañas en cada borde de la ventana.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edges {
    pub top: f32,
    pub left: f32,
    pub right: f32,
    pub bottom: f32,
}

/// Reparto de la ventana: el terminal (todos sus paneles) y, a su derecha, los paneles
/// laterales.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Areas {
    pub area: [f32; 4],
    /// Agentes o puertos, pegado al borde derecho.
    pub panel: Option<Rect>,
    /// Cambios de git, entre el terminal y el panel.
    pub changes: Option<Rect>,
}

/// Reparte una ventana de `w` × `h` con los paneles de `panel_w` y `changes_w` de ancho (0 =
/// cerrado).
pub fn areas(w: f32, h: f32, e: Edges, panel_w: f32, changes_w: f32, c: &Chrome) -> Areas {
    let (m, g) = (c.margin, c.gap);
    // Arriba, la barra ya separa; sin barra (la del sistema y una sola pestaña), el margen.
    let y = if e.top > 0.0 { e.top } else { m };
    let inner_h = (h - y - e.bottom - m).max(1.0);
    // De derecha a izquierda: panel, cambios y lo que queda para el terminal.
    let mut right = w - e.right - m;
    let panel = (panel_w > 0.0).then(|| {
        right -= panel_w;
        let r = (right, y, panel_w, inner_h);
        right -= g;
        r
    });
    let changes = (changes_w > 0.0).then(|| {
        right -= changes_w;
        let r = (right, y, changes_w, inner_h);
        right -= g;
        r
    });
    let x = e.left + m;
    Areas {
        area: [x, y, (right - x).max(1.0), inner_h],
        panel,
        changes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(t: f32) -> Edges {
        Edges {
            top: t,
            ..Edges::default()
        }
    }

    #[test]
    fn clasica_sin_margenes() {
        let c = Chrome::new(&Style::default(), 2.0);
        assert!(!c.floating);
        assert_eq!((c.margin, c.gap, c.radius, c.dim), (0.0, 0.0, 0.0, 0.0));
        let a = areas(1000.0, 800.0, top(40.0), 300.0, 200.0, &c);
        assert_eq!(a.area, [0.0, 40.0, 500.0, 760.0]);
        assert_eq!(a.changes, Some((500.0, 40.0, 200.0, 760.0)));
        assert_eq!(a.panel, Some((700.0, 40.0, 300.0, 760.0)));
        let solo = areas(1000.0, 800.0, top(40.0), 0.0, 0.0, &c);
        assert_eq!(solo.area, [0.0, 40.0, 1000.0, 760.0]);
        assert_eq!((solo.panel, solo.changes), (None, None));
    }

    #[test]
    fn flotante_con_margen_y_huecos() {
        let c = Chrome::new(&Style::floating(), 1.0);
        assert!(c.floating);
        assert_eq!(
            (c.margin, c.gap, c.radius, c.sheet),
            (10.0, 10.0, 14.0, 16.0)
        );
        // Escala 2: todo el doble.
        assert_eq!(Chrome::new(&Style::floating(), 2.0).margin, 20.0);
        // Sin margen arriba (ahí está la barra); a los lados, abajo y entre tarjetas, sí.
        let a = areas(1000.0, 800.0, top(40.0), 300.0, 200.0, &c);
        assert_eq!(a.panel, Some((690.0, 40.0, 300.0, 750.0)));
        assert_eq!(a.changes, Some((480.0, 40.0, 200.0, 750.0)));
        assert_eq!(a.area, [10.0, 40.0, 460.0, 750.0]);
        // Pestañas en una columna a la izquierda y en una franja abajo.
        let e = Edges {
            top: 40.0,
            left: 120.0,
            right: 0.0,
            bottom: 0.0,
        };
        assert_eq!(
            areas(1000.0, 800.0, e, 0.0, 0.0, &c).area,
            [130.0, 40.0, 860.0, 750.0]
        );
        let e = Edges {
            top: 40.0,
            bottom: 36.0,
            ..Edges::default()
        };
        assert_eq!(
            areas(1000.0, 800.0, e, 0.0, 0.0, &c).area,
            [10.0, 40.0, 980.0, 714.0]
        );
    }

    #[test]
    fn las_esquinas_siguen_al_ajuste() {
        let recto = Style {
            radius: 0,
            ..Style::floating()
        };
        let c = Chrome::new(&recto, 1.0);
        assert_eq!((c.radius, c.small, c.sheet), (0.0, 0.0, 0.0));
        let redondo = Style {
            radius: 16,
            ..Style::floating()
        };
        assert_eq!(Chrome::new(&redondo, 1.0).radius, 20.0);
    }

    #[test]
    fn sin_barra_arriba_tambien_hay_margen() {
        // Barra de título del sistema y una sola pestaña: no hay barra de Garbo arriba.
        let c = Chrome::new(&Style::floating(), 1.0);
        let a = areas(1000.0, 800.0, Edges::default(), 0.0, 0.0, &c);
        assert_eq!(a.area, [10.0, 10.0, 980.0, 780.0]);
        // La Clásica, pegada como siempre.
        let clasica = Chrome::new(&Style::default(), 1.0);
        let a = areas(1000.0, 800.0, Edges::default(), 0.0, 0.0, &clasica);
        assert_eq!(a.area, [0.0, 0.0, 1000.0, 800.0]);
    }

    #[test]
    fn ventana_minima_sin_tamanos_negativos() {
        let c = Chrome::new(&Style::floating(), 1.0);
        let a = areas(30.0, 30.0, top(40.0), 0.0, 0.0, &c);
        assert!(a.area[2] >= 1.0 && a.area[3] >= 1.0);
    }
}
