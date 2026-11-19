//! Protocolo de estado de programas (OSC 7501, revisión 0.4, propuesta de Mitchell Hashimoto).
//!
//! Un programa pregunta `OSC 7501 ; ? ST` y, si la terminal contesta, informa de su estado:
//! `OSC 7501 ; state=working:app=claude-code:msg=<base64> ST`. Estados: `idle`, `working`,
//! `blocked` (te espera; `kind` = permission | question | auth), `done`, `error` y `clear` (borra).
//!
//! Visto con Claude Code 2.1.328 (lo admite desde la 2.1.295): `idle` al arrancar; `working`, a
//! veces con `msg` ("Writing hola.txt"); `done` al acabar el turno; `blocked` + `permission` con
//! "trust this folder" en el diálogo de confianza; `clear` al salir.
//!
//! Garbo usa el registro principal y, si algún otro (con `id`) está bloqueado, también lo muestra.
//! El texto de los programas nunca se devuelve al PTY: solo se contesta a la consulta.

use std::time::Instant;

/// Respuesta a la consulta (lo que Garbo entiende). La especificación la escribe con espacios
/// alrededor de ":"; se manda sin ellos, que es lo que permite su gramática.
pub const REPLY: &str = "?:states=idle,working,done,blocked,error:kinds=permission,question,auth";
/// Mayor informe que se acepta (límites de la especificación).
pub const MAX_REPORT: usize = 4096;
const MAX_RECORDS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Working,
    Blocked,
    Done,
    Error,
}

/// Qué espera del usuario un programa bloqueado.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Aprobar algo (un comando, una herramienta).
    Permission,
    /// Contestar una pregunta.
    Question,
    /// Iniciar sesión o dar una credencial.
    Auth,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub state: State,
    pub kind: Option<Kind>,
    pub app: Option<String>,
    /// Una línea: qué hace, qué espera o qué terminó (ya sin caracteres de control).
    pub msg: Option<String>,
    pub progress: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Report {
    Query,
    /// `id` vacío = registro principal.
    Set { id: String, record: Record },
    /// `None` = todos.
    Clear { id: Option<String> },
}

/// Interpreta lo que va tras `7501;`. `None` = se descarta entero (según la especificación).
pub fn parse(payload: &[u8]) -> Option<Report> {
    if payload.len() > MAX_REPORT {
        return None;
    }
    let text = std::str::from_utf8(payload).ok()?;
    if text.trim() == "?" {
        return Some(Report::Query);
    }
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    for pair in text.split(':') {
        let Some((k, v)) = pair.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        let key_ok = !k.is_empty() && k.len() <= 16 && k.bytes().all(|b| b.is_ascii_lowercase());
        let value_ok = v.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.,+/=-".contains(&b));
        if key_ok && value_ok {
            pairs.retain(|(pk, _)| *pk != k); // la última gana
            pairs.push((k, v));
        }
    }
    let get = |key: &str| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
    let id = match get("id") {
        Some(id) => Some(valid_id(id).then(|| id.to_string())?),
        None => None,
    };
    let state = match get("state")? {
        "idle" => State::Idle,
        "working" => State::Working,
        "blocked" => State::Blocked,
        "done" => State::Done,
        "error" => State::Error,
        "clear" => return Some(Report::Clear { id }),
        _ => return None,
    };
    let kind = match (state, get("kind")) {
        (State::Blocked, Some("permission")) => Some(Kind::Permission),
        (State::Blocked, Some("question")) => Some(Kind::Question),
        (State::Blocked, Some("auth")) => Some(Kind::Auth),
        _ => None,
    };
    let progress = match state {
        State::Working | State::Blocked => get("progress").and_then(|p| p.parse::<u8>().ok()).filter(|p| *p <= 100),
        _ => None,
    };
    let app = get("app").filter(|a| valid_segment(a)).map(str::to_string);
    let msg = match get("msg") {
        Some(m) => Some(decode_text(m, 2732, 2048)?),
        None => None,
    };
    if let Some(t) = get("title") {
        decode_text(t, 256, 192)?; // no se usa, pero uno inválido descarta el informe
    }
    Some(Report::Set { id: id.unwrap_or_default(), record: Record { state, kind, app, msg, progress } })
}

fn valid_segment(s: &str) -> bool {
    (1..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
}

fn valid_id(id: &str) -> bool {
    id.len() <= 128 && id.split('/').count() <= 8 && id.split('/').all(valid_segment)
}

/// Base64 (con o sin relleno) → texto de una línea. Con caracteres de control, nada.
fn decode_text(b64: &str, max_encoded: usize, max_decoded: usize) -> Option<String> {
    if b64.len() > max_encoded {
        return None;
    }
    let bytes = base64(b64)?;
    if bytes.len() > max_decoded {
        return None;
    }
    let s = String::from_utf8(bytes).ok()?;
    if s.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(s)
}

fn base64(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in s.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    (s.len() % 4 != 1).then_some(out)
}

/// Texto de un programa listo para enseñarlo fuera de la rejilla (avisos, panel): sin cambios de
/// dirección del texto ni caracteres invisibles, que podrían disfrazarlo.
pub fn display(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'))
        .collect()
}

/// Lo que se sabe de una pestaña: sus registros, de quien los manda.
#[derive(Default, Debug)]
pub struct Records {
    recs: Vec<(String, Record, Instant)>,
}

/// Resumen para el panel y los avisos.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub state: State,
    pub kind: Option<Kind>,
    pub msg: Option<String>,
}

impl Records {
    pub fn apply(&mut self, report: Report) {
        match report {
            Report::Query => {}
            Report::Clear { id: None } => self.recs.clear(),
            Report::Clear { id: Some(id) } => {
                let below = format!("{id}/");
                self.recs.retain(|(r, ..)| *r != id && !r.starts_with(&below));
            }
            Report::Set { id, record } => {
                self.recs.retain(|(r, ..)| *r != id);
                if self.recs.len() >= MAX_RECORDS {
                    // Fuera el que lleva más tiempo sin actualizarse.
                    if let Some(i) = self.recs.iter().enumerate().min_by_key(|(_, r)| r.2).map(|(i, _)| i) {
                        self.recs.remove(i);
                    }
                }
                self.recs.push((id, record, Instant::now()));
            }
        }
    }

    /// El programa salió o volvió el prompt del shell: lo que estaba en marcha ya no lo está.
    /// "Terminado" y "error" se quedan hasta que se vean.
    pub fn drop_live(&mut self) {
        self.recs.retain(|(_, r, _)| !matches!(r.state, State::Working | State::Blocked));
    }

    /// Estado de la pestaña: si algo espera al usuario, eso; si no, el registro principal (o el
    /// primero que trabaje).
    pub fn summary(&self) -> Option<Summary> {
        let pick = self
            .recs
            .iter()
            .find(|(_, r, _)| r.state == State::Blocked)
            .or_else(|| self.recs.iter().find(|(id, ..)| id.is_empty()))
            .or_else(|| self.recs.iter().find(|(_, r, _)| r.state == State::Working))
            .or_else(|| self.recs.last())?;
        let r = &pick.1;
        Some(Summary { state: r.state, kind: r.kind, msg: r.msg.as_deref().map(display) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(state: State) -> Report {
        Report::Set { id: String::new(), record: Record { state, kind: None, app: None, msg: None, progress: None } }
    }

    #[test]
    fn informes_de_claude_code() {
        // Lo que manda Claude Code 2.1.328 (capturado en un PTY).
        assert_eq!(parse(b"?"), Some(Report::Query));
        let r = parse(b"state=blocked:app=claude-code:kind=permission:msg=dHJ1c3QgdGhpcyBmb2xkZXI=").unwrap();
        let Report::Set { id, record } = r else { panic!() };
        assert_eq!(id, "");
        assert_eq!(record.state, State::Blocked);
        assert_eq!(record.kind, Some(Kind::Permission));
        assert_eq!(record.app.as_deref(), Some("claude-code"));
        assert_eq!(record.msg.as_deref(), Some("trust this folder"));
        let r = parse(b"state=working:app=claude-code:msg=V3JpdGluZyBob2xhLnR4dA").unwrap(); // sin relleno
        assert!(matches!(r, Report::Set { record: Record { state: State::Working, ref msg, .. }, .. } if msg.as_deref() == Some("Writing hola.txt")));
        assert_eq!(parse(b"state=clear"), Some(Report::Clear { id: None }));
        assert_eq!(parse(b"state=clear:id=build"), Some(Report::Clear { id: Some("build".into()) }));
    }

    #[test]
    fn reglas_de_la_especificacion() {
        assert_eq!(parse(b"app=x"), None, "sin estado no vale");
        assert_eq!(parse(b"state=dormido"), None, "estado desconocido");
        assert_eq!(parse(b"state=working:id=a b"), Some(set(State::Working)), "par con un byte no permitido: se salta");
        assert_eq!(parse(b"state=working:id=a/../b/" ), None, "id mal formado: todo fuera");
        // Un mensaje con salto de línea (en base64: "a\nb") descarta el informe.
        assert_eq!(parse(b"state=working:msg=YQpi"), None);
        assert_eq!(parse(b"state=working:msg=ab-c"), None, "base64 inválido");
        assert_eq!(parse(b"state=working:msg=a"), None, "base64 cortado");
        assert_eq!(parse(b"state=working:msg=!!"), Some(set(State::Working)), "par con bytes no permitidos: se salta");
        // kind solo con blocked; progreso fuera de rango = ausente; claves desconocidas, ignoradas.
        let Some(Report::Set { record, .. }) = parse(b" state = done : kind=permission : progress=150 : zz=1 ") else { panic!() };
        assert_eq!((record.state, record.kind, record.progress), (State::Done, None, None));
        let Some(Report::Set { record, .. }) = parse(b"state=working:progress=40:state=blocked") else { panic!() };
        assert_eq!((record.state, record.progress), (State::Blocked, Some(40)), "la última gana");
        assert_eq!(parse(&vec![b'a'; MAX_REPORT + 1]), None);
        assert_eq!(display("ok\u{202E}txt.exe\u{200B}"), "oktxt.exe");
    }

    #[test]
    fn registros_y_resumen() {
        let mut r = Records::default();
        assert_eq!(r.summary(), None);
        r.apply(set(State::Working));
        r.apply(Report::Set {
            id: "sub/uno".into(),
            record: Record { state: State::Blocked, kind: Some(Kind::Question), app: None, msg: None, progress: None },
        });
        assert_eq!(r.summary().map(|s| (s.state, s.kind)), Some((State::Blocked, Some(Kind::Question))), "lo que espera al usuario manda");
        r.apply(Report::Clear { id: Some("sub".into()) });
        assert_eq!(r.summary().map(|s| s.state), Some(State::Working), "clear borra también lo de debajo");
        r.apply(set(State::Done));
        r.drop_live();
        assert_eq!(r.summary().map(|s| s.state), Some(State::Done), "terminado sobrevive al prompt");
        r.apply(set(State::Working));
        r.drop_live();
        assert_eq!(r.summary(), None);
        for i in 0..100 {
            r.apply(Report::Set { id: format!("r{i}"), record: Record { state: State::Idle, kind: None, app: None, msg: None, progress: None } });
        }
        assert_eq!(r.recs.len(), MAX_RECORDS);
    }
}
