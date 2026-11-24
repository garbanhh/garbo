//! Forma de la interfaz: cómo se dibujan las pestañas, la etiqueta del prompt (y el hueco bajo la
//! ruta), los bloques, los botones de ventana, el alto de la barra, el fondo y el radio de las
//! esquinas. Cada tema trae la
//! suya (`Theme::style`) y la sección `[style]` de config.toml la retoca opción a opción.
//!
//! De momento solo en Linux (`ENABLED`): en Windows se dibuja siempre la forma de siempre
//! (`Style::default()`) y el editor de aspecto no aparece. El código es común a los dos sistemas,
//! así que en Windows compila igual; activarlo allí es cambiar `ENABLED` y probarlo.

use serde::Deserialize;

/// Forma configurable y editor de aspecto disponibles en este sistema.
pub const ENABLED: bool = cfg!(target_os = "linux");

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
        /// Rectángulos a toda altura pegados al borde.
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
    /// Hueco sobre la ruta (entre un bloque y el anterior), en píxeles lógicos.
    pub block_gap: u8,
    /// Hueco entre el comando y su salida, en píxeles lógicos.
    pub output_gap: u8,
}

/// Huecos de los bloques en píxeles físicos (ver `view::Rows`).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Gaps {
    /// Sobre la ruta.
    pub above: f32,
    /// Bajo la ruta (antes del comando).
    pub path: f32,
    /// Entre el comando y su salida.
    pub output: f32,
}

impl Gaps {
    /// ¿Hay algún hueco? Sin ninguno, las filas van seguidas.
    pub fn any(&self) -> bool {
        self.above >= 0.5 || self.path >= 0.5 || self.output >= 0.5
    }
}

impl Default for Style {
    /// La forma de siempre (la de Garbo hasta la 0.12).
    fn default() -> Self {
        Self {
            tabs: Tabs::Flat,
            prompt: Prompt::Border,
            blocks: Blocks::Classic,
            buttons: Buttons::Round,
            bar: BarSize::Normal,
            background: Background::Solid,
            radius: 6,
            prompt_gap: 0,
            block_gap: 0,
            output_gap: 0,
        }
    }
}

impl Style {
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
            above: px(self.block_gap),
            path: px(self.prompt_gap),
            output: px(self.output_gap),
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
    pub output_gap: Option<u8>,
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
    output_gap: Option<f64>,
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
            output_gap: gap(r.output_gap),
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
            output_gap: self.output_gap.unwrap_or(base.output_gap),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Las claves y su valor para config.toml (`None` = no escribir y borrar si estaba).
    pub fn entries(&self) -> [(&'static str, Option<toml_edit::Item>); 10] {
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
            ("output_gap", self.output_gap.map(|g| value(g as i64))),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valores_desconocidos_no_invalidan_el_archivo() {
        let o: StyleOverrides = toml::from_str(
            "tabs = \"pill\"\nprompt = \"nada\"\nradius = 99\nbar = \"TALL\"\nprompt_gap = 7.6\nblock_gap = 4\noutput_gap = 40",
        )
        .unwrap();
        assert_eq!(o.tabs, Some(Tabs::Pill));
        assert_eq!(o.prompt, None);
        assert_eq!(o.radius, Some(24));
        assert_eq!(o.bar, Some(BarSize::Tall));
        assert_eq!(o.prompt_gap, Some(8));
        assert_eq!((o.block_gap, o.output_gap), (Some(4), Some(32)));
        let s = o.apply(Style::default());
        assert_eq!(s.tabs, Tabs::Pill);
        assert_eq!(s.prompt, Prompt::Border);
        assert_eq!(s.prompt_gap, 8);
        let lejos: StyleOverrides = toml::from_str("prompt_gap = -3").unwrap();
        assert_eq!(lejos.prompt_gap, Some(0));
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
    }
}
