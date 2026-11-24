//! Pestañas: en qué borde de la ventana van, su ancho, dónde va el nombre, el nombre que se
//! desplaza cuando no cabe, cambiarles el nombre y el nombre de las pestañas con un agente.
//!
//! De momento solo en Linux (`ENABLED`), como la forma (`style.rs`): en Windows la barra va
//! siempre arriba, con el ancho, el nombre y los títulos de siempre, y no se pueden renombrar.

use crate::style::options;
use serde::Deserialize;
use std::time::{Duration, Instant};

/// Opciones de pestañas disponibles en este sistema.
pub const ENABLED: bool = cfg!(target_os = "linux");

options! {
    /// Borde de la ventana en el que van las pestañas.
    Position {
        Top = "top", "Arriba";
        Bottom = "bottom", "Abajo";
        Left = "left", "Izquierda";
        Right = "right", "Derecha";
    }
}

options! {
    /// Ancho de las pestañas (o de la columna de pestañas, a los lados).
    Width {
        Narrow = "narrow", "Estrecho";
        Normal = "normal", "Normal";
        Wide = "wide", "Ancho";
        Max = "max", "Máximo";
    }
}

options! {
    /// Dónde va el nombre dentro de la pestaña.
    Align {
        Left = "left", "Izquierda";
        Center = "center", "Centrado";
    }
}

options! {
    /// Animación de una pestaña (y de su fila en el panel de agentes) mientras el agente trabaja.
    Working {
        /// Tres puntos que se encienden por turnos (la de siempre).
        Dots = "dots", "Puntos";
        /// Tres puntos que saltan uno tras otro.
        Wave = "wave", "Onda";
        /// Un punto con un anillo que se abre.
        Pulse = "pulse", "Pulso";
        /// Ocho puntos en círculo con una estela que gira.
        Spin = "spin", "Giro";
        /// Una barrita que va y viene.
        Bar = "bar", "Barra";
        /// Un punto quieto: sin animación (y sin fotogramas mientras trabaja).
        Still = "still", "Fijo";
    }
}

impl Working {
    /// Cada cuánto necesita un fotograma (`None`: no se mueve).
    pub fn frame(self) -> Option<Duration> {
        match self {
            Working::Dots => Some(Duration::from_millis(140)),
            Working::Still => None,
            _ => Some(Duration::from_millis(80)),
        }
    }
}

/// Momento de la animación "trabajando" para dibujarla: milisegundos desde un origen fijo y el
/// estilo elegido.
#[derive(Clone, Copy, Debug)]
pub struct Anim {
    pub ms: u64,
    pub kind: Working,
}

impl Position {
    /// A un lado: las pestañas van en una columna.
    pub fn vertical(self) -> bool {
        matches!(self, Position::Left | Position::Right)
    }
}

impl Width {
    /// Ancho máximo de cada pestaña en una barra horizontal (píxeles lógicos; `None`: reparten
    /// todo el sitio).
    pub fn max_tab(self) -> Option<f32> {
        match self {
            Width::Narrow => Some(150.0),
            Width::Normal => Some(260.0),
            Width::Wide => Some(360.0),
            Width::Max => None,
        }
    }

    /// Ancho de la columna de pestañas a un lado (píxeles lógicos).
    pub fn column(self) -> f32 {
        match self {
            Width::Narrow => 170.0,
            Width::Normal => 220.0,
            Width::Wide => 290.0,
            Width::Max => 380.0,
        }
    }
}

/// Sección `[tabs]` de config.toml. Un valor que no se reconoce se ignora (con un aviso en el
/// registro) en vez de invalidar todo el archivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(from = "RawTabs")]
pub struct TabsConfig {
    pub position: Position,
    pub width: Width,
    pub align: Align,
    /// Animación mientras trabaja un agente.
    pub working: Working,
}

impl Default for TabsConfig {
    /// Lo de siempre: arriba, ancho normal y el nombre a la izquierda.
    fn default() -> Self {
        Self {
            position: Position::Top,
            width: Width::Normal,
            align: Align::Left,
            working: Working::Dots,
        }
    }
}

impl TabsConfig {
    /// Lo que se usa en este sistema (donde no está `ENABLED`, lo de siempre).
    pub fn effective(&self) -> Self {
        if ENABLED {
            *self
        } else {
            Self::default()
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawTabs {
    position: Option<String>,
    width: Option<String>,
    align: Option<String>,
    working: Option<String>,
}

fn lenient<T>(key: &str, v: Option<String>, parse: fn(&str) -> Option<T>, default: T) -> T {
    let Some(v) = v else { return default };
    parse(&v).unwrap_or_else(|| {
        log::warn!("[tabs] {key} = \"{v}\" no es un valor válido: se usa el de siempre");
        default
    })
}

impl From<RawTabs> for TabsConfig {
    fn from(r: RawTabs) -> Self {
        let d = Self::default();
        Self {
            position: lenient("position", r.position, Position::parse, d.position),
            width: lenient("width", r.width, Width::parse, d.width),
            align: lenient("align", r.align, Align::parse, d.align),
            working: lenient("working", r.working, Working::parse, d.working),
        }
    }
}

// --- Nombre que no cabe: se desplaza con el ratón encima ---------------------------------------

/// Velocidad del desplazamiento (píxeles lógicos por segundo) y de la vuelta al principio.
const SPEED: f32 = 45.0;
const BACK_SPEED: f32 = 220.0;
/// Esperas: antes de empezar (pasar por encima no lo mueve), al llegar al final, al volver al
/// principio y tras mover la rueda.
const START_DELAY: Duration = Duration::from_millis(450);
const END_PAUSE: Duration = Duration::from_millis(1300);
const HOME_PAUSE: Duration = Duration::from_millis(900);
const WHEEL_PAUSE: Duration = Duration::from_millis(1500);
/// Lo que avanza el nombre por cada paso de la rueda (píxeles lógicos).
pub const WHEEL_STEP: f32 = 48.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Quieto hasta ese momento; después, hacia delante (`true`) o de vuelta.
    Wait(Instant, bool),
    Forward,
    Back,
}

/// Desplazamiento del nombre de la pestaña que está bajo el ratón, si no cabe. Solo pide
/// fotogramas mientras se mueve: en reposo (o con el ratón fuera) no cuesta nada.
#[derive(Debug)]
pub struct Marquee {
    /// Pestaña de la barra bajo el ratón.
    target: Option<usize>,
    /// Cuánto se ha desplazado el nombre (píxeles físicos).
    pos: f32,
    /// Lo que no cabe (píxeles físicos): hasta dónde se puede desplazar. Lo pone el dibujo.
    max: f32,
    phase: Phase,
    last: Option<Instant>,
}

impl Default for Marquee {
    fn default() -> Self {
        Self {
            target: None,
            pos: 0.0,
            max: 0.0,
            phase: Phase::Wait(Instant::now(), true),
            last: None,
        }
    }
}

impl Marquee {
    /// El ratón pasa a estar sobre la pestaña `target` (`None`: fuera de las pestañas).
    pub fn hover(&mut self, target: Option<usize>, now: Instant) {
        if target != self.target {
            *self = Self {
                target,
                phase: Phase::Wait(now + START_DELAY, true),
                ..Self::default()
            };
        }
    }

    pub fn target(&self) -> Option<usize> {
        self.target
    }

    /// Rueda sobre la pestaña: `px` (físicos) > 0 avanza. Devuelve si se movió algo.
    pub fn wheel(&mut self, px: f32, now: Instant) -> bool {
        if self.target.is_none() || self.max <= 0.0 {
            return false;
        }
        let before = self.pos;
        self.pos = (self.pos + px).clamp(0.0, self.max);
        let at_end = self.pos >= self.max;
        self.phase = Phase::Wait(now + WHEEL_PAUSE, !at_end);
        self.last = None;
        self.pos != before
    }

    /// Avanza hasta `now` sabiendo lo que no cabe (`max`, físicos). Devuelve el desplazamiento.
    pub fn step(&mut self, max: f32, scale: f32, now: Instant) -> f32 {
        self.max = max.max(0.0);
        if self.max <= 0.0 {
            self.pos = 0.0;
            return 0.0;
        }
        let dt = self
            .last
            .map_or(0.0, |l| now.saturating_duration_since(l).as_secs_f32())
            .min(0.1);
        self.last = Some(now);
        match self.phase {
            Phase::Wait(until, forward) => {
                if now >= until {
                    self.phase = if forward && self.pos < self.max {
                        Phase::Forward
                    } else {
                        Phase::Back
                    };
                }
            }
            Phase::Forward => {
                self.pos += SPEED * scale * dt;
                if self.pos >= self.max {
                    self.pos = self.max;
                    self.phase = Phase::Wait(now + END_PAUSE, false);
                }
            }
            Phase::Back => {
                self.pos -= BACK_SPEED * scale * dt;
                if self.pos <= 0.0 {
                    self.pos = 0.0;
                    self.phase = Phase::Wait(now + HOME_PAUSE, true);
                }
            }
        }
        self.pos = self.pos.clamp(0.0, self.max);
        self.pos
    }

    /// Cuándo hace falta el siguiente fotograma (`None`: no hay nada que mover).
    pub fn next_frame(&self, now: Instant) -> Option<Instant> {
        if self.target.is_none() || self.max <= 0.0 {
            return None;
        }
        Some(match self.phase {
            Phase::Wait(until, _) => until.max(now),
            Phase::Forward | Phase::Back => now + Duration::from_millis(16),
        })
    }
}

// --- Cambiar el nombre --------------------------------------------------------------------------

/// Largo máximo de un nombre puesto a mano (caracteres).
pub const MAX_NAME: usize = 80;

/// Campo de texto sobre la pestaña a la que se le cambia el nombre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    /// Pestaña de la barra (índice del grupo).
    pub group: usize,
    pub text: String,
    /// Posición del cursor, en caracteres.
    pub cursor: usize,
    /// Todo seleccionado (al empezar): lo que se escriba lo sustituye.
    pub all: bool,
}

impl Rename {
    pub fn new(group: usize, current: &str) -> Self {
        let text: String = current.chars().take(MAX_NAME).collect();
        Self {
            group,
            cursor: text.chars().count(),
            text,
            all: true,
        }
    }

    fn byte(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map_or(self.text.len(), |(b, _)| b)
    }

    fn take_selection(&mut self) -> bool {
        if self.all {
            self.text.clear();
            self.cursor = 0;
            self.all = false;
            return true;
        }
        false
    }

    /// Escribe `s` en el cursor (sin saltos de línea ni caracteres de control).
    pub fn insert(&mut self, s: &str) {
        self.take_selection();
        let clean: String = s
            .chars()
            .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
            .filter(|c| !c.is_control())
            .collect();
        let room = MAX_NAME.saturating_sub(self.text.chars().count());
        let add: String = clean.chars().take(room).collect();
        let at = self.byte(self.cursor);
        self.text.insert_str(at, &add);
        self.cursor += add.chars().count();
    }

    pub fn backspace(&mut self) {
        if self.take_selection() || self.cursor == 0 {
            return;
        }
        let (a, b) = (self.byte(self.cursor - 1), self.byte(self.cursor));
        self.text.replace_range(a..b, "");
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        if self.take_selection() || self.cursor >= self.text.chars().count() {
            return;
        }
        let (a, b) = (self.byte(self.cursor), self.byte(self.cursor + 1));
        self.text.replace_range(a..b, "");
    }

    /// Mueve el cursor `d` caracteres (y quita la selección).
    pub fn step(&mut self, d: i32) {
        let n = self.text.chars().count() as i32;
        self.cursor = if self.all {
            if d < 0 {
                0
            } else {
                n as usize
            }
        } else {
            (self.cursor as i32 + d).clamp(0, n) as usize
        };
        self.all = false;
    }

    pub fn home(&mut self) {
        self.cursor = 0;
        self.all = false;
    }

    pub fn end(&mut self) {
        self.cursor = self.text.chars().count();
        self.all = false;
    }

    pub fn select_all(&mut self) {
        self.all = true;
        self.cursor = self.text.chars().count();
    }

    /// El nombre que queda (`None`: vacío, vuelve el automático).
    pub fn result(&self) -> Option<String> {
        let t = self.text.trim();
        (!t.is_empty()).then(|| t.to_string())
    }

    /// Texto hasta el cursor (para colocarlo al dibujar).
    pub fn before_cursor(&self) -> &str {
        &self.text[..self.byte(self.cursor)]
    }
}

// --- Nombre de las pestañas con un agente --------------------------------------------------------

/// Nombre corto del agente ("Claude Code" -> "Claude", "Gemini CLI" -> "Gemini").
pub fn agent_short(name: &str) -> &str {
    name.split_whitespace().next().unwrap_or(name)
}

/// Largo máximo de lo que se le pidió al agente en el nombre de la pestaña (caracteres). Lo que
/// no cabe en la pestaña se ve pasando el ratón por encima.
const MAX_TASK: usize = 110;
/// Una frase más larga se corta en la primera coma que haya pasados `MIN_CLAUSE` caracteres.
const LONG_SENTENCE: usize = 60;
const MIN_CLAUSE: usize = 25;

/// Respuestas que no dicen qué se está haciendo: con ellas se mantiene la tarea anterior.
const CONFIRMATIONS: &[&str] = &[
    "si",
    "vale",
    "ok",
    "okay",
    "okey",
    "de acuerdo",
    "continua",
    "sigue",
    "seguimos",
    "adelante",
    "dale",
    "hazlo",
    "venga",
    "perfecto",
    "genial",
    "gracias",
    "muchas gracias",
    "bien",
    "no",
    "yes",
    "y",
    "go",
    "go on",
    "continue",
    "thanks",
    "thank you",
];

/// Quita tildes y signos para comparar con `CONFIRMATIONS`.
fn plain(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'á' | 'à' | 'Á' => 'a',
            'é' | 'è' | 'É' => 'e',
            'í' | 'Í' => 'i',
            'ó' | 'Ó' => 'o',
            'ú' | 'ü' | 'Ú' => 'u',
            c => c.to_ascii_lowercase(),
        })
        .filter(|c| c.is_alphanumeric() || *c == ' ')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Lo que se le pidió al agente, para el nombre de la pestaña: la primera frase de su último
/// mensaje (si es larga, hasta la primera coma), sin las marcas de imágenes o texto pegado
/// ("[Image #3]", "[Pasted text #1 +20 lines]"). `None` si no dice qué se está haciendo (una
/// orden "/…", solo una imagen, "sí", "continúa"…): entonces se mantiene la tarea anterior.
pub fn task_from_prompt(prompt: &str) -> Option<String> {
    // Fuera las marcas entre corchetes que pone el agente al adjuntar.
    let mut text = String::with_capacity(prompt.len());
    let mut rest = prompt;
    while let Some(i) = rest.find('[') {
        text.push_str(&rest[..i]);
        let tail = &rest[i..];
        let marker = ["[Image", "[Pasted text", "[image", "[pasted text"]
            .iter()
            .any(|m| tail.starts_with(m));
        match tail.find(']').filter(|_| marker) {
            Some(end) => rest = &tail[end + 1..],
            None => {
                text.push('[');
                rest = &tail[1..];
            }
        }
    }
    text.push_str(rest);
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if line.starts_with('/') || line.starts_with('!') || line.starts_with('<') {
        return None;
    }
    if CONFIRMATIONS.contains(&plain(&line).as_str()) || plain(&line).is_empty() {
        return None;
    }
    let line = first_sentence(&line);
    let mut out: String = line.chars().take(MAX_TASK).collect();
    if line.chars().count() > MAX_TASK {
        // Hasta la última palabra entera.
        if let Some(cut) = out.rfind(' ').filter(|&c| c > MAX_TASK / 2) {
            out.truncate(cut);
        }
        out = out.trim_end_matches([',', ';', ':', ' ']).to_string();
        out.push('…');
    }
    let mut chars = out.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

/// La primera frase (con su "?" o "!", sin el punto) y, si es larga, hasta la primera coma.
fn first_sentence(line: &str) -> &str {
    let mut end = line.len();
    for (i, c) in line.char_indices() {
        let next = line[i + c.len_utf8()..].chars().next();
        if matches!(c, '.' | '?' | '!' | ';') && next.is_none_or(char::is_whitespace) {
            end = if matches!(c, '?' | '!') {
                i + c.len_utf8()
            } else {
                i
            };
            break;
        }
    }
    let sentence = &line[..end];
    if sentence.chars().count() <= LONG_SENTENCE {
        return sentence;
    }
    sentence
        .char_indices()
        .skip(MIN_CLAUSE)
        .find(|&(i, c)| c == ',' && sentence[i + 1..].starts_with(' '))
        .map_or(sentence, |(i, _)| &sentence[..i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_tolerante() {
        let c: TabsConfig =
            toml::from_str("position = \"LEFT\"\nwidth = \"enorme\"\nalign = \"center\"").unwrap();
        assert_eq!(c.position, Position::Left);
        assert_eq!(c.width, Width::Normal, "valor desconocido: el de siempre");
        assert_eq!(c.align, Align::Center);
        assert_eq!(
            toml::from_str::<TabsConfig>("").unwrap(),
            TabsConfig::default()
        );
    }

    #[test]
    fn el_nombre_se_desplaza_y_vuelve() {
        let t0 = Instant::now();
        let mut m = Marquee::default();
        assert_eq!(m.next_frame(t0), None, "sin pestaña bajo el ratón");
        m.hover(Some(2), t0);
        assert_eq!(m.step(100.0, 1.0, t0), 0.0);
        // Espera antes de empezar: no pide fotogramas hasta entonces.
        assert_eq!(m.next_frame(t0), Some(t0 + START_DELAY));
        let mut t = t0 + START_DELAY;
        m.step(100.0, 1.0, t);
        let mut last = 0.0;
        for _ in 0..200 {
            t += Duration::from_millis(16);
            let p = m.step(100.0, 1.0, t);
            assert!(p >= last || p == 100.0);
            last = p;
            if p == 100.0 {
                break;
            }
        }
        assert_eq!(last, 100.0, "llega al final");
        // Pausa al final y luego vuelve al principio.
        t += END_PAUSE;
        m.step(100.0, 1.0, t);
        for _ in 0..100 {
            t += Duration::from_millis(16);
            if m.step(100.0, 1.0, t) == 0.0 {
                break;
            }
        }
        assert_eq!(m.pos, 0.0);
        // La rueda lo mueve al momento, y no pasa del final.
        assert!(m.wheel(30.0, t));
        assert_eq!(m.pos, 30.0);
        m.wheel(500.0, t);
        assert_eq!(m.pos, 100.0);
        // Otra pestaña: vuelve a empezar.
        m.hover(Some(3), t);
        assert_eq!(m.pos, 0.0);
        // Si cabe, no se mueve ni pide fotogramas.
        assert_eq!(m.step(0.0, 1.0, t + START_DELAY), 0.0);
        assert_eq!(m.next_frame(t), None);
        assert!(!m.wheel(10.0, t));
    }

    #[test]
    fn campo_del_nombre() {
        let mut r = Rename::new(0, "GRBTER");
        assert!(r.all);
        r.insert("Garbo");
        assert_eq!(r.text, "Garbo", "lo seleccionado se sustituye");
        r.step(-2);
        r.insert("–Ñ");
        assert_eq!(r.text, "Gar–Ñbo");
        assert_eq!(r.before_cursor(), "Gar–Ñ");
        r.backspace();
        r.delete();
        assert_eq!(r.text, "Gar–o");
        r.home();
        r.delete();
        r.end();
        r.insert(" dev\nnueva");
        assert_eq!(r.text, "ar–o dev nueva");
        r.select_all();
        r.backspace();
        assert_eq!(r.result(), None, "vacío: vuelve el nombre automático");
        r.insert(&"x".repeat(200));
        assert_eq!(r.text.chars().count(), MAX_NAME);
        assert_eq!(Rename::new(1, "  hola  ").result().as_deref(), Some("hola"));
    }

    #[test]
    fn tarea_del_agente() {
        assert_eq!(agent_short("Claude Code"), "Claude");
        assert_eq!(agent_short("Gemini CLI"), "Gemini");
        assert_eq!(agent_short("Codex"), "Codex");
        assert_eq!(
            task_from_prompt("quiero que se pueda cambiar el nombre a las pestañas\notra línea")
                .as_deref(),
            Some("Quiero que se pueda cambiar el nombre a las pestañas")
        );
        assert_eq!(
            task_from_prompt("[Image #3] arregla esto [Pasted text #1 +20 lines]").as_deref(),
            Some("Arregla esto")
        );
        for nada in [
            "[Image #3]",
            "/clear",
            "sí",
            "Continúa.",
            "vale!",
            "  ",
            "<command>x",
        ] {
            assert_eq!(task_from_prompt(nada), None, "{nada:?}");
        }
        assert_eq!(
            task_from_prompt("revisa [esto] y [aquello]").as_deref(),
            Some("Revisa [esto] y [aquello]")
        );
        assert_eq!(
            task_from_prompt(
                "Quiero que se le pueda cambiar el nombre a las pestañas, que el nombre sea mejor"
            )
            .as_deref(),
            Some("Quiero que se le pueda cambiar el nombre a las pestañas"),
            "frase larga: hasta la primera coma"
        );
        assert_eq!(
            task_from_prompt("lo has documentado todo? y lo demás").as_deref(),
            Some("Lo has documentado todo?")
        );
        assert_eq!(
            task_from_prompt("Arregla el panel. Luego mira lo otro").as_deref(),
            Some("Arregla el panel")
        );
        assert_eq!(
            task_from_prompt("sube la versión 0.13.0 a github").as_deref(),
            Some("Sube la versión 0.13.0 a github"),
            "un punto sin espacio detrás no corta"
        );
        let largo = task_from_prompt(&"palabra ".repeat(40)).unwrap();
        assert!(
            largo.ends_with('…') && largo.chars().count() <= MAX_TASK + 1,
            "{largo}"
        );
        assert!(!largo.contains("palabra…") || largo.ends_with("palabra…"));
    }
}
