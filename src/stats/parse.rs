//! Lectura de los archivos de sesión de cada agente.
//!
//! - **Claude Code**: una línea por bloque de cada mensaje; el uso de tokens (`message.usage`) se
//!   repite en todas las líneas de un mensaje y se cuenta una vez por `message.id`.
//! - **Codex**: eventos `token_count` con el total acumulado de la sesión; se cuenta la diferencia
//!   con el anterior. El modelo sale de `turn_context`.
//! - **Gemini CLI**: un mensaje `gemini` con sus `tokens` por respuesta (JSONL, o un JSON con
//!   `messages` en las versiones antiguas). Se relee entero al cambiar.
//!
//! Solo se analizan las líneas que pueden interesar (se buscan antes unas pocas palabras clave en
//! los bytes) y solo los campos necesarios: el resto, como la salida de las herramientas, se salta
//! sin copiarlo.
//!
//! **Tiempo trabajando**: la suma de los huecos entre eventos seguidos de una sesión, salvo el que
//! acaba en un mensaje del usuario (eso es lo que tardó en escribir) y los de más de
//! [`IDLE_MS`] (el agente esperaba un permiso o la sesión estaba parada).

use super::{FileStats, Usage};
use serde::de::{Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};

/// Un hueco mayor entre dos eventos no cuenta como trabajo.
pub const IDLE_MS: i64 = 10 * 60 * 1000;

/// Antes de 2024 no había ninguno de estos agentes: una fecha anterior es un error (y estiraría
/// "Todo" décadas).
const MIN_TS: i64 = 1_704_067_200_000;

/// Hora de un evento, si es razonable.
fn when(s: &str) -> Option<i64> {
    parse_ts(s).filter(|&t| t >= MIN_TS)
}

/// Mensajes de Claude Code recordados para no contar dos veces el mismo uso.
const RECENT_IDS: usize = 8;

/// Lee lo nuevo de `f` (hasta `size` bytes).
pub fn update(f: &mut FileStats, size: u64) {
    use super::AgentKind::*;
    let whole = f.agent == Gemini || size < f.state.offset;
    if whole {
        f.reset();
    }
    let Ok(mut file) = File::open(&f.path) else {
        return;
    };
    let mut acc = Acc::take(f);
    if f.agent == Gemini {
        let mut bytes = Vec::new();
        if file.read_to_end(&mut bytes).is_ok() {
            gemini(
                &bytes,
                f.path.extension().is_some_and(|e| e == "json"),
                f,
                &mut acc,
            );
        }
    } else if file.seek(SeekFrom::Start(f.state.offset)).is_ok() {
        let mut reader = BufReader::with_capacity(64 * 1024, file.take(size - f.state.offset));
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    // Una línea a medio escribir se lee la próxima vez, entera.
                    if line.last() != Some(&b'\n') {
                        break;
                    }
                    f.state.offset += n as u64;
                    match f.agent {
                        Claude => claude(&line, f.main, &mut f.state, &mut acc),
                        Codex => codex(&line, f, &mut acc),
                        Gemini => {}
                    }
                }
            }
        }
    }
    acc.finish(f);
}

// ---------------------------------------------------------------------------------------------
// Acumulador por horas
// ---------------------------------------------------------------------------------------------

/// Lo leído de un archivo, por horas, mientras se lee.
pub(super) struct Acc {
    usage: HashMap<(u32, u16), Usage>,
    activity: HashMap<u32, (u32, u32)>,
    runs: Vec<(u32, u32)>,
    models: Vec<String>,
}

impl Acc {
    fn take(f: &mut FileStats) -> Self {
        Self {
            usage: f.usage.drain(..).map(|(h, m, u)| ((h, m), u)).collect(),
            activity: f.activity.drain(..).map(|(h, p, w)| (h, (p, w))).collect(),
            runs: std::mem::take(&mut f.runs),
            models: std::mem::take(&mut f.models),
        }
    }

    fn finish(self, f: &mut FileStats) {
        f.usage = self
            .usage
            .into_iter()
            .map(|((h, m), u)| (h, m, u))
            .collect();
        f.usage.sort_unstable_by_key(|r| (r.0, r.1));
        f.activity = self
            .activity
            .into_iter()
            .map(|(h, (p, w))| (h, p, w))
            .collect();
        f.activity.sort_unstable_by_key(|r| r.0);
        // Tramos ordenados y sin solapes: un archivo cuenta una vez por minuto.
        let mut runs = self.runs;
        runs.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(runs.len());
        for (start, len) in runs {
            match merged.last_mut() {
                Some(last) if start <= last.0 + last.1 => {
                    last.1 = last.1.max(start + len - last.0);
                }
                _ => merged.push((start, len)),
            }
        }
        f.runs = merged;
        f.models = self.models;
    }

    fn model(&mut self, name: &str) -> u16 {
        match self.models.iter().position(|m| m == name) {
            Some(i) => i as u16,
            None => {
                self.models.push(name.to_string());
                (self.models.len() - 1) as u16
            }
        }
    }

    fn add_usage(&mut self, ts: i64, model: u16, u: &Usage) {
        self.usage.entry((hour(ts), model)).or_default().add(u);
    }

    fn prompt(&mut self, ts: i64) {
        self.activity.entry(hour(ts)).or_default().0 += 1;
    }

    /// Trabajo de `from` a `to` (ms), repartido entre las horas que abarca.
    fn work(&mut self, from: i64, to: i64) {
        let mut t = from;
        while t < to {
            let end = ((t.div_euclid(3_600_000) + 1) * 3_600_000).min(to);
            // Redondeando: truncar perdería medio segundo por evento, miles por sesión.
            let secs = ((end - t + 500) / 1000) as u32;
            if secs > 0 {
                self.activity.entry(hour(t)).or_default().1 += secs;
            }
            t = end;
        }
        let (m0, m1) = (minute(from), minute(to));
        match self.runs.last_mut() {
            Some(last) if m0 >= last.0 && m0 <= last.0 + last.1 => {
                last.1 = last.1.max(m1 + 1 - last.0);
            }
            _ => self.runs.push((m0, m1 + 1 - m0)),
        }
    }
}

fn hour(ts: i64) -> u32 {
    ts.div_euclid(3_600_000).max(0) as u32
}

fn minute(ts: i64) -> u32 {
    ts.div_euclid(60_000).max(0) as u32
}

/// Qué es un evento para medir el trabajo.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ev {
    /// Lo hace el agente: el hueco desde el anterior es trabajo.
    Work,
    /// Lo escribe el usuario: el hueco desde el anterior no cuenta.
    User,
    /// Termina un turno: el hueco hasta el siguiente no cuenta.
    End,
}

fn event(st: &mut super::ParseState, acc: &mut Acc, ts: i64, ev: Ev) {
    if let Some(prev) = st.last_ts {
        let gap = ts - prev;
        if ev != Ev::User && !st.idle && gap > 0 && gap <= IDLE_MS {
            acc.work(prev, ts);
        }
        if ts < prev {
            // Fuera de orden: no se cuenta y no se retrocede.
            return;
        }
    }
    st.last_ts = Some(ts);
    st.idle = ev == Ev::End;
}

// ---------------------------------------------------------------------------------------------
// Fechas
// ---------------------------------------------------------------------------------------------

/// "2026-11-18T19:58:38.140Z" (o con "+02:00") → ms desde 1970.
pub fn parse_ts(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r)?.parse::<i64>().ok() };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let mut i = 19;
    let mut ms = 0;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let frac = &s[start..i];
        let digits = &frac[..frac.len().min(3)];
        ms = digits.parse::<i64>().unwrap_or(0) * 10_i64.pow(3 - digits.len() as u32);
    }
    let offset = match b.get(i) {
        Some(b'+') | Some(b'-') => {
            let sign = if b[i] == b'-' { -1 } else { 1 };
            let oh = num(i + 1..i + 3)?;
            let om = num(i + 4..i + 6).or_else(|| num(i + 3..i + 5)).unwrap_or(0);
            sign * (oh * 3600 + om * 60)
        }
        _ => 0,
    };
    let days = super::days_from_civil(y, mo as u32, d as u32);
    Some((days * 86_400 + h * 3600 + mi * 60 + sec - offset) * 1000 + ms)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    memchr::memmem::find(hay, needle).is_some()
}

// ---------------------------------------------------------------------------------------------
// Claude Code
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct ClaudeLine<'a> {
    #[serde(rename = "type", borrow)]
    kind: Cow<'a, str>,
    #[serde(borrow, default)]
    timestamp: Option<Cow<'a, str>>,
    #[serde(rename = "isMeta", default)]
    meta: Option<bool>,
    /// Mensajes de un subagente dentro del archivo de la sesión (versiones antiguas).
    #[serde(rename = "isSidechain", default)]
    sidechain: Option<bool>,
    /// El resumen que deja `/compact` como si fuera un mensaje del usuario.
    #[serde(rename = "isCompactSummary", default)]
    compact: Option<bool>,
    #[serde(borrow, default)]
    message: Option<ClaudeMessage<'a>>,
}

#[derive(Deserialize)]
struct ClaudeMessage<'a> {
    #[serde(borrow, default)]
    id: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    model: Option<Cow<'a, str>>,
    #[serde(default)]
    usage: Option<ClaudeUsage>,
    #[serde(default)]
    content: UserContent,
}

#[derive(Deserialize)]
struct ClaudeUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
}

/// Qué lleva un mensaje del usuario, sin copiar su contenido.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
enum UserContent {
    #[default]
    Other,
    /// Algo escrito por el usuario.
    Prompt,
    /// Una orden (`/clear`, `!ls`) o su salida: empieza por `<`.
    Command,
    /// Resultado de una herramienta: el agente sigue trabajando.
    ToolResult,
}

#[derive(Deserialize)]
struct Block<'a> {
    #[serde(rename = "type", borrow, default)]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    text: Option<Cow<'a, str>>,
}

impl<'de> Deserialize<'de> for UserContent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = UserContent;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("contenido de un mensaje")
            }

            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Self::Value, E> {
                Ok(if s.trim_start().starts_with('<') {
                    UserContent::Command
                } else {
                    UserContent::Prompt
                })
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let (mut text, mut command, mut tool) = (false, false, false);
                while let Some(b) = seq.next_element::<Block>()? {
                    match b.kind.as_deref() {
                        Some("tool_result") => tool = true,
                        Some("text") => {
                            text = true;
                            command |= b.text.is_some_and(|t| t.trim_start().starts_with('<'));
                        }
                        Some("image") | Some("document") => text = true,
                        _ => {}
                    }
                }
                Ok(match (tool, text, command) {
                    (true, ..) => UserContent::ToolResult,
                    (_, true, false) => UserContent::Prompt,
                    (_, true, true) => UserContent::Command,
                    _ => UserContent::Other,
                })
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(UserContent::Other)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(UserContent::Other)
            }

            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(UserContent::Other)
            }

            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(UserContent::Other)
            }

            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(UserContent::Other)
            }

            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(UserContent::Other)
            }

            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(UserContent::Other)
            }
        }
        d.deserialize_any(V)
    }
}

fn claude(line: &[u8], main: bool, st: &mut super::ParseState, acc: &mut Acc) {
    // Solo interesan los mensajes; en el texto de un mensaje estas palabras irían escapadas.
    if !contains(line, br#""type":"assistant""#) && !contains(line, br#""type":"user""#) {
        return;
    }
    let Ok(l) = serde_json::from_slice::<ClaudeLine>(line) else {
        return;
    };
    let Some(ts) = l.timestamp.as_deref().and_then(when) else {
        return;
    };
    match l.kind.as_ref() {
        "assistant" => {
            if let Some(m) = l.message {
                let id = m.id.as_deref().unwrap_or("");
                let seen = !id.is_empty() && st.recent_ids.iter().any(|r| r == id);
                if !seen {
                    if !id.is_empty() {
                        if st.recent_ids.len() >= RECENT_IDS {
                            st.recent_ids.remove(0);
                        }
                        st.recent_ids.push(id.to_string());
                    }
                    let model = m.model.as_deref().unwrap_or("");
                    if let Some(u) = m
                        .usage
                        .filter(|_| !model.is_empty() && model != "<synthetic>")
                    {
                        let usage = Usage {
                            input: u.input_tokens.unwrap_or(0),
                            cache_write: u.cache_creation_input_tokens.unwrap_or(0),
                            cache_read: u.cache_read_input_tokens.unwrap_or(0),
                            output: u.output_tokens.unwrap_or(0),
                            calls: 1,
                        };
                        let mi = acc.model(model);
                        acc.add_usage(ts, mi, &usage);
                    }
                }
            }
            event(st, acc, ts, Ev::Work);
        }
        "user" => {
            let content = l.message.map(|m| m.content).unwrap_or_default();
            match content {
                UserContent::ToolResult => event(st, acc, ts, Ev::Work),
                UserContent::Prompt
                    if main
                        && l.meta != Some(true)
                        && l.sidechain != Some(true)
                        && l.compact != Some(true) =>
                {
                    acc.prompt(ts);
                    event(st, acc, ts, Ev::User);
                }
                _ => event(st, acc, ts, Ev::User),
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------
// Codex
// ---------------------------------------------------------------------------------------------

/// Una línea de Codex de la que solo interesa el `payload`.
#[derive(Deserialize)]
struct CodexLine<'a, P> {
    #[serde(rename = "type", borrow)]
    kind: Cow<'a, str>,
    payload: Option<P>,
}

#[derive(Deserialize)]
struct TurnContext {
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
struct TokenCount {
    #[serde(default)]
    info: Option<TokenInfo>,
}

#[derive(Deserialize)]
struct TokenInfo {
    #[serde(default)]
    total_token_usage: Option<CodexUsage>,
    #[serde(default)]
    last_token_usage: Option<CodexUsage>,
}

#[derive(Deserialize, Clone, Copy, Default)]
struct CodexUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

impl CodexUsage {
    fn arr(self) -> [u64; 3] {
        [
            self.input_tokens,
            self.cached_input_tokens,
            self.output_tokens,
        ]
    }
}

#[derive(Deserialize)]
struct TaskStarted<'a> {
    #[serde(borrow, default)]
    turn_attribution: Option<TurnAttribution<'a>>,
}

#[derive(Deserialize)]
struct TurnAttribution<'a> {
    #[serde(borrow, default)]
    turn_trigger: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct SessionMeta {
    #[serde(default)]
    parent_thread_id: Option<IgnoredAny>,
    #[serde(default)]
    source: Option<serde_json::Value>,
}

/// La hora de una línea de Codex: es siempre el primer campo, no hace falta analizarla entera.
fn codex_ts(line: &[u8]) -> Option<i64> {
    let head = &line[..line.len().min(96)];
    let key = br#""timestamp":""#;
    let start = memchr::memmem::find(head, key)? + key.len();
    let end = start + memchr::memchr(b'"', &head[start..])?;
    when(std::str::from_utf8(&head[start..end]).ok()?)
}

fn codex(line: &[u8], f: &mut FileStats, acc: &mut Acc) {
    let Some(ts) = codex_ts(line) else {
        return;
    };
    let st = &mut f.state;
    let has = |k: &[u8]| contains(line, k);
    if has(br#""type":"session_meta""#) {
        if let Ok(l) = serde_json::from_slice::<CodexLine<SessionMeta>>(line) {
            if let Some(p) = l.payload.filter(|_| l.kind == "session_meta") {
                let sub = p
                    .source
                    .as_ref()
                    .is_some_and(|s| s.get("subagent").is_some());
                if p.parent_thread_id.is_some() || sub {
                    f.main = false;
                }
            }
        }
        event(st, acc, ts, Ev::User);
    } else if has(br#""type":"turn_context""#) {
        if let Ok(l) = serde_json::from_slice::<CodexLine<TurnContext>>(line) {
            if let Some(m) = l
                .payload
                .and_then(|p| p.model)
                .filter(|_| l.kind == "turn_context")
            {
                st.model = Some(acc.model(&m));
            }
        }
        event(st, acc, ts, Ev::Work);
    } else if has(br#""type":"token_count""#) {
        if let Ok(l) = serde_json::from_slice::<CodexLine<TokenCount>>(line) {
            if let Some(info) = l.payload.and_then(|p| p.info) {
                let total = info.total_token_usage.map(CodexUsage::arr);
                let last = info.last_token_usage.unwrap_or_default().arr();
                let delta = match (total, st.totals) {
                    (Some(t), Some(p)) if t.iter().zip(p).all(|(t, p)| *t >= p) => {
                        [t[0] - p[0], t[1] - p[1], t[2] - p[2]]
                    }
                    // El total volvió a empezar (o no lo hay): vale el de la última respuesta.
                    (Some(_), Some(_)) | (None, _) => last,
                    (Some(t), None) => t,
                };
                if let Some(t) = total {
                    st.totals = Some(t);
                }
                if delta.iter().any(|&d| d > 0) {
                    let model = match st.model {
                        Some(m) => m,
                        None => acc.model("desconocido"),
                    };
                    let cached = delta[1].min(delta[0]);
                    acc.add_usage(
                        ts,
                        model,
                        &Usage {
                            input: delta[0] - cached,
                            cache_read: cached,
                            cache_write: 0,
                            output: delta[2],
                            calls: 1,
                        },
                    );
                }
            }
        }
        event(st, acc, ts, Ev::Work);
    } else if has(br#""type":"task_started""#) {
        let by_user = serde_json::from_slice::<CodexLine<TaskStarted>>(line)
            .ok()
            .and_then(|l| l.payload)
            .and_then(|p| p.turn_attribution)
            .and_then(|a| a.turn_trigger)
            .is_none_or(|t| t == "user");
        st.task_events = true;
        if by_user && f.main {
            acc.prompt(ts);
        }
        event(st, acc, ts, Ev::User);
    } else if has(br#""type":"task_complete""#) || has(br#""type":"turn_aborted""#) {
        event(st, acc, ts, Ev::End);
    } else if has(br#""type":"user_message""#) {
        // Versiones sin `task_started`: cada mensaje del usuario es una petición.
        if !st.task_events && f.main {
            acc.prompt(ts);
        }
        event(st, acc, ts, Ev::User);
    } else if has(br#""role":"user""#) {
        event(st, acc, ts, Ev::User);
    } else {
        event(st, acc, ts, Ev::Work);
    }
}

// ---------------------------------------------------------------------------------------------
// Gemini CLI
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct GeminiRecord {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    /// Cabecera de un archivo de subagente: `"kind": "subagent"`.
    #[serde(rename = "kind", default)]
    file_kind: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    tokens: Option<GeminiTokens>,
}

#[derive(Deserialize, Default)]
struct GeminiTokens {
    #[serde(default)]
    input: u64,
    #[serde(default)]
    output: u64,
    #[serde(default)]
    cached: u64,
    #[serde(default)]
    thoughts: u64,
    #[serde(default)]
    tool: u64,
}

#[derive(Deserialize)]
struct GeminiFile {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    messages: Vec<GeminiRecord>,
}

fn gemini(bytes: &[u8], json: bool, f: &mut FileStats, acc: &mut Acc) {
    let records: Vec<GeminiRecord> = if json {
        match serde_json::from_slice::<GeminiFile>(bytes) {
            Ok(file) => {
                if file.kind.as_deref() == Some("subagent") {
                    f.main = false;
                }
                file.messages
            }
            Err(_) => return,
        }
    } else {
        bytes
            .split(|&b| b == b'\n')
            .filter_map(|l| serde_json::from_slice::<GeminiRecord>(l).ok())
            .collect()
    };
    if records
        .iter()
        .any(|r| r.file_kind.as_deref() == Some("subagent"))
    {
        f.main = false;
    }
    // Un mensaje puede aparecer varias veces (se reescribe al completarse): una vez cada uno.
    let mut seen: HashSet<String> = HashSet::new();
    let mut counted: HashSet<String> = HashSet::new();
    let st = &mut f.state;
    for r in records {
        let Some(ts) = r.timestamp.as_deref().and_then(when) else {
            continue;
        };
        let first = r.id.as_ref().is_none_or(|id| seen.insert(id.clone()));
        if let Some(t) = r.tokens.as_ref() {
            let new = r.id.as_ref().is_none_or(|id| counted.insert(id.clone()));
            if new && t.input + t.output + t.thoughts + t.tool > 0 {
                let input = t.input + t.tool;
                let cached = t.cached.min(input);
                let model = acc.model(r.model.as_deref().unwrap_or("gemini"));
                acc.add_usage(
                    ts,
                    model,
                    &Usage {
                        input: input - cached,
                        cache_read: cached,
                        cache_write: 0,
                        output: t.output + t.thoughts,
                        calls: 1,
                    },
                );
            }
        }
        if !first {
            continue;
        }
        match r.kind.as_deref() {
            Some("user") => {
                if f.main {
                    acc.prompt(ts);
                }
                event(st, acc, ts, Ev::User);
            }
            Some("gemini") => event(st, acc, ts, Ev::Work),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{AgentKind, FileStats};
    use super::*;
    use std::io::Write;

    #[test]
    fn horas_iso() {
        assert_eq!(parse_ts("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_ts("1970-01-01T00:00:01.5Z"), Some(1500));
        assert_eq!(
            parse_ts("2026-11-18T19:58:38.140Z"),
            Some(1_795_031_918_140)
        );
        assert_eq!(
            parse_ts("2026-11-18T21:58:38.140+02:00"),
            Some(1_795_031_918_140)
        );
        assert_eq!(
            parse_ts("2026-11-18T19:58:38.140123456Z"),
            Some(1_795_031_918_140)
        );
        assert_eq!(parse_ts("ayer"), None);
    }

    fn write(dir: &std::path::Path, name: &str, lines: &[String]) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        path
    }

    fn read(agent: AgentKind, path: std::path::PathBuf, main: bool) -> FileStats {
        let mut f = FileStats {
            path: path.clone(),
            agent,
            main,
            ..Default::default()
        };
        let size = std::fs::metadata(&path).unwrap().len();
        update(&mut f, size);
        f
    }

    fn sum(f: &FileStats) -> Usage {
        let mut u = Usage::default();
        f.usage.iter().for_each(|r| u.add(&r.2));
        u
    }

    fn work(f: &FileStats) -> (u32, u32) {
        f.activity
            .iter()
            .fold((0, 0), |a, r| (a.0 + r.1, a.1 + r.2))
    }

    fn claude_msg(ts: &str, id: &str, block: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"{id}","model":"claude-opus-5-5","content":[{block}],"usage":{{"input_tokens":3,"cache_creation_input_tokens":100,"cache_read_input_tokens":1000,"output_tokens":50}}}}}}"#
        )
    }

    #[test]
    fn claude_cuenta_cada_mensaje_una_vez_y_el_trabajo() {
        let temp = crate::temp_dir::TempDir::new_in(&std::env::temp_dir()).unwrap();
        let lines = vec![
            r#"{"type":"user","timestamp":"2026-11-18T10:00:00Z","message":{"role":"user","content":"Arregla el fallo"}}"#.to_string(),
            r#"{"type":"user","isMeta":true,"timestamp":"2026-11-18T10:00:00Z","message":{"role":"user","content":"Caveat"}}"#.to_string(),
            claude_msg("2026-11-18T10:00:30Z", "m1", r#"{"type":"text","text":"Voy"}"#),
            claude_msg("2026-11-18T10:00:31Z", "m1", r#"{"type":"tool_use","name":"Bash","input":{"command":"ls"}}"#),
            // Resultado enorme de una herramienta: el agente sigue trabajando.
            format!(
                r#"{{"type":"user","timestamp":"2026-11-18T10:02:31Z","message":{{"role":"user","content":[{{"type":"tool_result","content":"{}"}}]}}}}"#,
                "x".repeat(10_000)
            ),
            claude_msg("2026-11-18T10:03:00Z", "m2", r#"{"type":"text","text":"Hecho"}"#),
            // La siguiente petición llega una hora después: eso no es trabajo.
            r#"{"type":"user","timestamp":"2026-11-18T11:03:00Z","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#.to_string(),
            r#"{"type":"user","timestamp":"2026-11-18T11:04:00Z","message":{"role":"user","content":[{"type":"text","text":"Otra cosa"},{"type":"image"}]}}"#.to_string(),
            // Ni el resumen de /compact ni un mensaje con una fecha imposible cuentan.
            r#"{"type":"user","isCompactSummary":true,"timestamp":"2026-11-18T11:04:00Z","message":{"role":"user","content":"This session is being continued"}}"#.to_string(),
            r#"{"type":"user","timestamp":"0001-01-01T00:00:00Z","message":{"role":"user","content":"Viejo"}}"#.to_string(),
            r#"{"type":"assistant","timestamp":"2026-11-18T11:04:10Z","message":{"id":"m3","model":"<synthetic>","content":[],"usage":{"input_tokens":0,"output_tokens":0}}}"#.to_string(),
        ];
        let path = write(temp.path(), "s.jsonl", &lines);
        let f = read(AgentKind::Claude, path.clone(), true);
        let u = sum(&f);
        assert_eq!(u.calls, 2);
        assert_eq!(u.input, 6);
        assert_eq!(u.cache_read, 2000);
        assert_eq!(u.cache_write, 200);
        assert_eq!(u.output, 100);
        // Dos peticiones (ni la meta ni la orden cuentan) y 3 min + 10 s de trabajo.
        assert_eq!(work(&f), (2, 190));
        assert!(f.activity.iter().all(|r| r.0 >= hour(MIN_TS)));
        assert_eq!(f.models, vec!["claude-opus-5-5"]);

        // Un subagente no suma peticiones.
        let sub = read(AgentKind::Claude, path, false);
        assert_eq!(work(&sub).0, 0);
    }

    #[test]
    fn claude_sigue_leyendo_donde_lo_dejo() {
        let temp = crate::temp_dir::TempDir::new_in(&std::env::temp_dir()).unwrap();
        let first = claude_msg("2026-11-18T10:00:00Z", "m1", "");
        let path = write(temp.path(), "s.jsonl", std::slice::from_ref(&first));
        let mut f = read(AgentKind::Claude, path.clone(), true);
        assert_eq!(sum(&f).calls, 1);
        // Se añade la misma respuesta (otra línea del mensaje), una nueva y media línea.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{}", claude_msg("2026-11-18T10:00:01Z", "m1", "")).unwrap();
        writeln!(file, "{}", claude_msg("2026-11-18T10:01:00Z", "m2", "")).unwrap();
        write!(file, r#"{{"type":"assistant","timest"#).unwrap();
        drop(file);
        let size = std::fs::metadata(&path).unwrap().len();
        update(&mut f, size);
        assert_eq!(sum(&f).calls, 2);
        assert_eq!(work(&f).1, 60);
        assert!(f.state.offset < size);
        assert_eq!(f.runs, vec![(29_916_600, 2)]);
    }

    #[test]
    fn codex_cuenta_diferencias_del_total() {
        let temp = crate::temp_dir::TempDir::new_in(&std::env::temp_dir()).unwrap();
        let tc = |ts: &str, total: (u64, u64, u64), last: (u64, u64, u64)| {
            format!(
                r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"output_tokens":{}}},"last_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"output_tokens":{}}}}}}}}}"#,
                total.0, total.1, total.2, last.0, last.1, last.2
            )
        };
        let lines = vec![
            r#"{"timestamp":"2026-11-19T21:47:00.000Z","type":"session_meta","payload":{"id":"x","base_instructions":{"text":"..."},"source":"cli"}}"#.to_string(),
            r#"{"timestamp":"2026-11-19T21:47:01.000Z","type":"event_msg","payload":{"type":"task_started","turn_attribution":{"turn_trigger":"user"}}}"#.to_string(),
            r#"{"timestamp":"2026-11-19T21:47:02.000Z","type":"turn_context","payload":{"model":"gpt-6.2-sol","cwd":"/"}}"#.to_string(),
            tc("2026-11-19T21:47:10.000Z", (1000, 800, 10), (1000, 800, 10)),
            // Repetido: no suma nada.
            tc("2026-11-19T21:47:10.100Z", (1000, 800, 10), (1000, 800, 10)),
            tc("2026-11-19T21:47:30.000Z", (2500, 2000, 30), (1500, 1200, 20)),
            r#"{"timestamp":"2026-11-19T21:47:31.000Z","type":"event_msg","payload":{"type":"task_complete"}}"#.to_string(),
            // Media hora parado antes del siguiente evento: no es trabajo.
            r#"{"timestamp":"2026-11-19T22:17:31.000Z","type":"event_msg","payload":{"type":"thread_settings_applied"}}"#.to_string(),
        ];
        let path = write(temp.path(), "rollout-a.jsonl", &lines);
        let f = read(AgentKind::Codex, path, true);
        let u = sum(&f);
        assert_eq!(u.calls, 2);
        assert_eq!(u.input_total(), 2500);
        assert_eq!(u.cache_read, 2000);
        assert_eq!(u.output, 30);
        assert_eq!(f.models, vec!["gpt-6.2-sol"]);
        assert_eq!(work(&f), (1, 30));
        assert!(f.main);

        let child = write(
            temp.path(),
            "rollout-b.jsonl",
            &[r#"{"timestamp":"2026-11-19T21:47:00.000Z","type":"session_meta","payload":{"id":"y","source":{"subagent":{"thread_spawn":{"parent_thread_id":"x"}}}}}"#.to_string()],
        );
        assert!(!read(AgentKind::Codex, child, true).main);
    }

    #[test]
    fn gemini_jsonl_y_json() {
        let temp = crate::temp_dir::TempDir::new_in(&std::env::temp_dir()).unwrap();
        let lines = vec![
            r#"{"sessionId":"s","projectHash":"p","startTime":"2026-11-19T10:00:00Z"}"#.to_string(),
            r#"{"id":"u1","timestamp":"2026-11-19T10:00:00Z","type":"user","content":"hola"}"#.to_string(),
            r#"{"id":"g1","timestamp":"2026-11-19T10:00:20Z","type":"gemini","content":"","model":"gemini-2.5-pro"}"#.to_string(),
            // El mismo mensaje, reescrito con sus tokens.
            r#"{"id":"g1","timestamp":"2026-11-19T10:00:20Z","type":"gemini","content":"hola","model":"gemini-2.5-pro","tokens":{"input":1000,"output":40,"cached":600,"thoughts":10,"tool":0,"total":1050}}"#.to_string(),
            r#"{"id":"g1","timestamp":"2026-11-19T10:00:20Z","type":"gemini","content":"hola","model":"gemini-2.5-pro","tokens":{"input":1000,"output":40,"cached":600,"thoughts":10,"tool":0,"total":1050}}"#.to_string(),
            r#"{"$set":{"summary":"Saludo"}}"#.to_string(),
        ];
        let f = read(
            AgentKind::Gemini,
            write(temp.path(), "session-a.jsonl", &lines),
            true,
        );
        let u = sum(&f);
        assert_eq!(
            (u.calls, u.input, u.cache_read, u.output),
            (1, 400, 600, 50)
        );
        assert_eq!(work(&f), (1, 20));

        let json = r#"{"sessionId":"s","messages":[{"id":"a","timestamp":"2026-11-19T10:00:00Z","type":"user","content":"x"},{"id":"b","timestamp":"2026-11-19T10:01:00Z","type":"gemini","model":"gemini-2.5-flash","tokens":{"input":10,"output":5,"cached":0}}]}"#;
        let path = temp.path().join("session-b.json");
        std::fs::write(&path, json).unwrap();
        let f = read(AgentKind::Gemini, path, true);
        assert_eq!(sum(&f).input_total(), 10);
        assert_eq!(work(&f), (1, 60));
    }
}
