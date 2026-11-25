//! Forma de la interfaz: cómo se dibujan las pestañas, la etiqueta del prompt (y el hueco bajo la
//! ruta), los bloques, los botones de ventana, el alto de la barra, el fondo y el radio de las
//! esquinas. Cada tema trae la
//! suya (`Theme::style`) y la sección `[style]` de config.toml la retoca opción a opción.
//!
//! Igual en Linux y en Windows. Lo único que cambia es la forma de siempre (`Style::default()`):
//! en Windows, los botones de ventana son los del sistema ("Planos").

use serde::Deserialize;

/// Enumeración de opciones con su clave en config.toml y su nombre en la interfaz.
macro_rules! options {
    ($(#[$m:meta])* $name:ident { $($(#[$vm:meta])* $var:ident = $key:literal, $label:literal;)+ }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($(#[$vm])* $var),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$var),+];

            /// Valor en config.toml.
            pub fn key(self) -> &'static str {
                match self {
                    $($name::$var => $key),+
                }
            }

            /// Nombre en la interfaz.
            pub fn label(self) -> &'static str {
                match self {
                    $($name::$var => $label),+
                }
            }

            pub fn parse(s: &str) -> Option<Self> {
                let s = s.trim();
                Self::ALL.iter().copied().find(|v| v.key().eq_ignore_ascii_case(s))
            }
        }
    };
}
pub(crate) use options;

options! {
    /// Pestañas de la barra.
    Tabs {
        /// A toda altura, la activa en un tono más claro (la de siempre).
        Flat = "flat", "Planas";
        Underline = "underline", "Subrayado";
        /// La activa se une al terminal con una línea de color encima.
        TopLine = "topline", "Línea arriba";
        Pill = "pill", "Píldora";
        Outline = "outline", "Contorno";
        Floating = "floating", "Flotantes";
        /// La activa se une al terminal como la solapa de una carpeta.
        Folder = "folder", "Carpeta";
        Brackets = "brackets", "Corchetes";
        Segmented = "segmented", "Segmentos";
        Dot = "dot", "Punto";
    }
}

options! {
    /// Etiqueta de la carpeta del prompt de Garbo.
    Prompt {
        /// Etiqueta con borde fino (la de siempre).
        Border = "border", "Con borde";
        Soft = "soft", "Relleno suave";
        Solid = "solid", "Sólido";
        /// Relleno con una franja de color a la izquierda.
        Tab = "tab", "Lengüeta";
        Underline = "underline", "Subrayado";
        Brackets = "brackets", "Corchetes";
        /// Sin etiqueta: las carpetas de la ruta en gris y la última en color.
        Dimmed = "dimmed", "Ruta atenuada";
        Plain = "plain", "Solo texto";
    }
}

options! {
    /// Marcas de los bloques de comandos.
    Blocks {
        /// Barra de estado a la izquierda y línea entre bloques (la de siempre).
        Classic = "classic", "Barra y línea";
        Bar = "bar", "Línea lateral";
        Dot = "dot", "Punto de estado";
        Card = "card", "Tarjeta";
        /// Tarjeta con la carpeta en una etiqueta y el resultado en una píldora; la orden que se
        /// escribe, en una tarjeta del color de acento (la de la interfaz Flotante).
        Chips = "chips", "Etiquetas";
        Outline = "outline", "Contorno";
        Header = "header", "Cabecera";
        Separator = "separator", "Separador";
        Dashed = "dashed", "Discontinua";
        /// Resultado y duración a la derecha de la línea de la carpeta.
        Status = "status", "Estado a la derecha";
        Errors = "errors", "Solo errores";
        /// Los bloques ya ejecutados, atenuados.
        Fade = "fade", "Atenuar anteriores";
        Plain = "none", "Solo la ruta";
    }
}

options! {
    /// Botones de ventana (minimizar, maximizar, cerrar) de la barra integrada.
    Buttons {
        /// Círculos con iconos finos (los de siempre en Linux).
        Round = "round", "Redondos";
        /// Círculos con los colores del tema.
        Color = "color", "De colores";
        /// Rectángulos a toda altura pegados al borde (los de siempre en Windows).
        Flat = "flat", "Planos";
        Icons = "icons", "Solo iconos";
    }
}

options! {
    /// Alto de la barra de pestañas.
    BarSize {
        Compact = "compact", "Compacta";
        Normal = "normal", "Normal";
        Tall = "tall", "Alta";
    }
}

options! {
    /// Fondo del terminal.
    Background {
        Solid = "solid", "Liso";
        /// Un tinte del color de acento arriba que se desvanece hacia abajo.
        Gradient = "gradient", "Degradado";
    }
}

options! {
    /// Interfaz de toda la ventana ([[0041 Interfaz Flotante]]): terminal y paneles como tarjetas
    /// que flotan sobre el fondo, o la de siempre. Los temas no la fijan.
    Interface {
        Floating = "floating", "Flotante";
        Classic = "classic", "Clásica";
    }
}

impl Interface {
    /// La de cada sistema si el usuario no elige: Flotante en Linux; en Windows, de momento,
    /// siempre la Clásica.
    pub fn default_here() -> Self {
        if cfg!(windows) {
            Interface::Classic
        } else {
            Interface::Floating
        }
    }
}

/// Radios de esquina que ofrece el editor (píxeles lógicos).
pub const RADII: [u8; 8] = [0, 2, 4, 6, 8, 10, 12, 16];

/// Huecos entre la ruta y el comando que ofrece el editor (píxeles lógicos).
pub const GAPS: [u8; 8] = [0, 2, 4, 6, 8, 10, 12, 16];

/// Hueco máximo que se acepta en config.toml (píxeles lógicos).
const MAX_GAP: f64 = 32.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub tabs: Tabs,
    pub prompt: Prompt,
    pub blocks: Blocks,
    pub buttons: Buttons,
    pub bar: BarSize,
    pub background: Background,
    /// Radio de las esquinas en píxeles lógicos.
    pub radius: u8,
    /// Hueco entre la línea de la ruta del prompt de Garbo y la del comando, en píxeles lógicos.
    pub prompt_gap: u8,
    /// Hueco entre la salida del bloque anterior y la línea que los separa, en píxeles lógicos.
    /// En los estilos de bloque sin línea, se suma a `line_gap`: es el hueco sobre la ruta.
    pub block_gap: u8,
    /// Hueco entre la línea que separa los bloques y la ruta, en píxeles lógicos.
    pub line_gap: u8,
    /// Hueco entre el comando y su salida, en píxeles lógicos.
    pub output_gap: u8,
    /// Interfaz de toda la ventana. No es de los temas: la resuelve `Config::theme`.
    pub interface: Interface,
}

/// Huecos de los bloques en píxeles físicos (ver `view::Rows`).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Gaps {
    /// Sobre la ruta (los dos lados de la línea entre bloques: `block_gap` + `line_gap`).
    pub above: f32,
    /// Lo que de `above` queda bajo la línea entre bloques (`line_gap`).
    pub line: f32,
    /// Bajo la ruta (antes del comando).
    pub path: f32,
    /// Entre el comando y su salida.
    pub output: f32,
    /// La orden que se escribe, abajo del todo mientras la pantalla no esté llena (bloques
    /// "Etiquetas", como la entrada de la interfaz Flotante).
    pub pin: bool,
    /// Margen de la tarjeta de un bloque "Etiquetas" bajo su última fila (0 con otros bloques).
    /// Con `pin`, la orden que se escribe sube eso para que su tarjeta quepa entera.
    pub tail: f32,
}

impl Gaps {
    /// ¿Hay algún hueco? Sin ninguno, las filas van seguidas.
    pub fn any(&self) -> bool {
        self.above >= 0.5 || self.path >= 0.5 || self.output >= 0.5
    }
}

impl Default for Style {
    /// La forma de siempre (la de Garbo hasta la 0.12). Los botones de ventana, los del sistema:
    /// redondos como los de GNOME en Linux y rectángulos a toda altura en Windows.
    fn default() -> Self {
        Self {
            tabs: Tabs::Flat,
            prompt: Prompt::Border,
            blocks: Blocks::Classic,
            buttons: if cfg!(windows) {
                Buttons::Flat
            } else {
                Buttons::Round
            },
            bar: BarSize::Normal,
            background: Background::Solid,
            radius: 6,
            prompt_gap: 0,
            block_gap: 0,
            line_gap: 0,
            output_gap: 0,
            interface: Interface::Classic,
        }
    }
}

impl Style {
    /// La forma por defecto con la interfaz Flotante (la de los temas que no traen forma propia):
    /// pestañas en píldora, botones redondos y esquinas de 10 px.
    pub fn floating() -> Self {
        Self {
            tabs: Tabs::Pill,
            prompt: Prompt::Plain,
            buttons: Buttons::Round,
            blocks: Blocks::Chips,
            radius: 10,
            block_gap: 16,
            prompt_gap: 6,
            output_gap: 6,
            interface: Interface::Floating,
            ..Self::default()
        }
    }

    /// Radio de esquina en píxeles físicos.
    pub fn r(&self, scale: f32) -> f32 {
        (self.radius as f32 * scale).round()
    }

    /// Hueco bajo la ruta en píxeles físicos.
    pub fn gap(&self, scale: f32) -> f32 {
        (self.prompt_gap as f32 * scale).round()
    }

    /// Los tres huecos de los bloques en píxeles físicos.
    pub fn gaps(&self, scale: f32) -> Gaps {
        let px = |v: u8| (v as f32 * scale).round();
        Gaps {
            above: px(self.block_gap) + px(self.line_gap),
            line: px(self.line_gap),
            path: px(self.prompt_gap),
            output: px(self.output_gap),
            pin: self.blocks == Blocks::Chips,
            tail: if self.blocks == Blocks::Chips {
                (4.0 * scale).round()
            } else {
                0.0
            },
        }
    }

    /// Diferencia del alto de la barra respecto al normal, en píxeles lógicos.
    pub fn bar_delta(&self) -> f32 {
        match self.bar {
            BarSize::Compact => -6.0,
            BarSize::Normal => 0.0,
            BarSize::Tall => 6.0,
        }
    }
}

/// Sección `[style]`: lo que el usuario cambió sobre la forma del tema. Un valor que no se
/// reconoce se ignora (con un aviso en el registro) en vez de invalidar todo el archivo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(from = "RawStyle")]
pub struct StyleOverrides {
    pub tabs: Option<Tabs>,
    pub prompt: Option<Prompt>,
    pub blocks: Option<Blocks>,
    pub buttons: Option<Buttons>,
    pub bar: Option<BarSize>,
    pub background: Option<Background>,
    pub radius: Option<u8>,
    pub prompt_gap: Option<u8>,
    pub block_gap: Option<u8>,
    pub line_gap: Option<u8>,
    pub output_gap: Option<u8>,
    pub interface: Option<Interface>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawStyle {
    tabs: Option<String>,
    prompt: Option<String>,
    blocks: Option<String>,
    buttons: Option<String>,
    bar: Option<String>,
    background: Option<String>,
    radius: Option<f64>,
    prompt_gap: Option<f64>,
    block_gap: Option<f64>,
    line_gap: Option<f64>,
    output_gap: Option<f64>,
    interface: Option<String>,
}

fn lenient<T>(key: &str, v: Option<String>, parse: fn(&str) -> Option<T>) -> Option<T> {
    let v = v?;
    let parsed = parse(&v);
    if parsed.is_none() {
        log::warn!("[style] {key} = \"{v}\" no es un valor válido: se usa el del tema");
    }
    parsed
}

/// Un hueco leído de config.toml, dentro de lo que se acepta.
fn gap(v: Option<f64>) -> Option<u8> {
    v.map(|v| v.clamp(0.0, MAX_GAP).round() as u8)
}

impl From<RawStyle> for StyleOverrides {
    fn from(r: RawStyle) -> Self {
        Self {
            tabs: lenient("tabs", r.tabs, Tabs::parse),
            prompt: lenient("prompt", r.prompt, Prompt::parse),
            blocks: lenient("blocks", r.blocks, Blocks::parse),
            buttons: lenient("buttons", r.buttons, Buttons::parse),
            bar: lenient("bar", r.bar, BarSize::parse),
            background: lenient("background", r.background, Background::parse),
            radius: r.radius.map(|v| v.clamp(0.0, 24.0).round() as u8),
            prompt_gap: gap(r.prompt_gap),
            block_gap: gap(r.block_gap),
            line_gap: gap(r.line_gap),
            output_gap: gap(r.output_gap),
            interface: lenient("interface", r.interface, Interface::parse),
        }
    }
}

impl StyleOverrides {
    /// La forma del tema con estos cambios encima.
    pub fn apply(&self, base: Style) -> Style {
        Style {
            tabs: self.tabs.unwrap_or(base.tabs),
            prompt: self.prompt.unwrap_or(base.prompt),
            blocks: self.blocks.unwrap_or(base.blocks),
            buttons: self.buttons.unwrap_or(base.buttons),
            bar: self.bar.unwrap_or(base.bar),
            background: self.background.unwrap_or(base.background),
            radius: self.radius.unwrap_or(base.radius),
            prompt_gap: self.prompt_gap.unwrap_or(base.prompt_gap),
            block_gap: self.block_gap.unwrap_or(base.block_gap),
            line_gap: self.line_gap.unwrap_or(base.line_gap),
            output_gap: self.output_gap.unwrap_or(base.output_gap),
            interface: self.interface.unwrap_or(base.interface),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Las claves y su valor para config.toml (`None` = no escribir y borrar si estaba).
    pub fn entries(&self) -> [(&'static str, Option<toml_edit::Item>); 12] {
        use toml_edit::value;
        let s = |v: Option<&'static str>| v.map(value);
        [
            ("tabs", s(self.tabs.map(Tabs::key))),
            ("prompt", s(self.prompt.map(Prompt::key))),
            ("blocks", s(self.blocks.map(Blocks::key))),
            ("buttons", s(self.buttons.map(Buttons::key))),
            ("bar", s(self.bar.map(BarSize::key))),
            ("background", s(self.background.map(Background::key))),
            ("radius", self.radius.map(|r| value(r as i64))),
            ("prompt_gap", self.prompt_gap.map(|g| value(g as i64))),
            ("block_gap", self.block_gap.map(|g| value(g as i64))),
            ("line_gap", self.line_gap.map(|g| value(g as i64))),
            ("output_gap", self.output_gap.map(|g| value(g as i64))),
            ("interface", s(self.interface.map(Interface::key))),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valores_desconocidos_no_invalidan_el_archivo() {
        let o: StyleOverrides = toml::from_str(
            "tabs = \"pill\"\nprompt = \"nada\"\nradius = 99\nbar = \"TALL\"\nprompt_gap = 7.6\nblock_gap = 4\nline_gap = 2.4\noutput_gap = 40",
        )
        .unwrap();
        assert_eq!(o.tabs, Some(Tabs::Pill));
        assert_eq!(o.prompt, None);
        assert_eq!(o.radius, Some(24));
        assert_eq!(o.bar, Some(BarSize::Tall));
        assert_eq!(o.prompt_gap, Some(8));
        assert_eq!((o.block_gap, o.output_gap), (Some(4), Some(32)));
        assert_eq!(o.line_gap, Some(2));
        let s = o.apply(Style::default());
        assert_eq!(s.tabs, Tabs::Pill);
        assert_eq!(s.prompt, Prompt::Border);
        assert_eq!(s.prompt_gap, 8);
        let lejos: StyleOverrides = toml::from_str("prompt_gap = -3").unwrap();
        assert_eq!(lejos.prompt_gap, Some(0));
    }

    /// El hueco sobre la ruta son los dos lados de la línea entre bloques.
    #[test]
    fn la_linea_parte_el_hueco_sobre_la_ruta() {
        let s = Style {
            block_gap: 10,
            line_gap: 4,
            ..Style::default()
        };
        let g = s.gaps(2.0);
        assert_eq!((g.above, g.line), (28.0, 8.0));
        let solo_linea = Style {
            line_gap: 6,
            ..Style::default()
        };
        assert!(solo_linea.gaps(1.0).any());
    }

    #[test]
    fn claves_unicas_y_reconocibles() {
        fn check<T: Copy + PartialEq + std::fmt::Debug>(
            all: &[T],
            key: fn(T) -> &'static str,
            parse: fn(&str) -> Option<T>,
        ) {
            for &v in all {
                assert_eq!(parse(key(v)), Some(v));
                assert_eq!(all.iter().filter(|w| key(**w) == key(v)).count(), 1);
            }
        }
        check(Tabs::ALL, Tabs::key, Tabs::parse);
        check(Prompt::ALL, Prompt::key, Prompt::parse);
        check(Blocks::ALL, Blocks::key, Blocks::parse);
        check(Buttons::ALL, Buttons::key, Buttons::parse);
        check(BarSize::ALL, BarSize::key, BarSize::parse);
        check(Background::ALL, Background::key, Background::parse);
        check(Interface::ALL, Interface::key, Interface::parse);
    }

    #[test]
    fn interfaz_en_style() {
        let o: StyleOverrides = toml::from_str("interface = \"classic\"").unwrap();
        assert_eq!(o.interface, Some(Interface::Classic));
        assert!(o
            .entries()
            .iter()
            .any(|(k, v)| *k == "interface" && v.is_some()));
        let raro: StyleOverrides = toml::from_str("interface = \"isla\"").unwrap();
        assert_eq!(raro.interface, None);
        // La forma de Flotante: la de siempre con píldoras, botones redondos y radio 10.
        let f = Style::floating();
        assert_eq!(f.interface, Interface::Floating);
        assert_eq!(
            (f.tabs, f.buttons, f.radius),
            (Tabs::Pill, Buttons::Round, 10)
        );
        assert_eq!(f.blocks, Blocks::Chips);
        // Hueco entre tarjetas: más que lo que cada tarjeta sobresale de sus filas.
        assert!(f.block_gap >= 12);
        assert_eq!(Style::default().interface, Interface::Classic);
    }
}
