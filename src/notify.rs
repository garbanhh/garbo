//! Avisos del escritorio cuando un agente termina (con `notify-send`, de libnotify).
//!
//! Cada pestaña lleva un `Tracker`. Cuando su agente deja de trabajar se espera `CONFIRM` (por si
//! solo era una pausa) y, si sigue parado, trabajó al menos lo configurado y el usuario no lo está
//! viendo (o se eligió "Siempre"), se envía el aviso. Al pulsarlo, el escritorio entrega un permiso
//! de activación (`--activation-token-fd`), con el que Garbo se pone delante (ver `activation.rs`)
//! en esa pestaña. Nada de esto despierta la app en reposo: solo hay temporizador con un aviso
//! pendiente.
//!
//! `notify-send -A` se queda esperando a que se pulse o se cierre el aviso. Al mirar la pestaña (o
//! al salir de Garbo) se termina ese proceso: GNOME 51 solo deja retirar un aviso a la conexión que
//! lo creó (`CloseNotification` desde otra responde "Invalid notification ID"), y sin su proceso
//! el aviso ya no lleva a ninguna pestaña.

use crate::config::{NotifyConfig, NotifyWhen};
use crate::terminal::UserEvent;
use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use winit::event_loop::EventLoopProxy;

/// Silencio que confirma que el agente terminó (no solo que se paró a pensar).
pub const CONFIRM: Duration = Duration::from_millis(2500);
const APP_ID: &str = "io.github.garbanhh.Garbo";

#[derive(Default, Debug)]
pub struct Tracker {
    /// Desde cuándo trabaja el agente.
    working_since: Option<Instant>,
    /// Aviso pendiente: cuándo enviarlo, cuándo empezó a trabajar y cuándo paró.
    pending: Option<(Instant, Instant, Instant)>,
    /// Proceso `notify-send` del aviso mostrado (para retirarlo al mirar la pestaña).
    pub shown: Option<u32>,
}

impl Tracker {
    /// Sigue el estado de trabajo. Devuelve cuánto trabajó si el aviso toca ahora.
    pub fn update(&mut self, working: bool, min_work: Duration, now: Instant) -> Option<Duration> {
        if working {
            if self.working_since.is_none() {
                // Si vuelve a trabajar antes de confirmar, es la misma tarea.
                self.working_since = Some(self.pending.take().map_or(now, |(_, start, _)| start));
            }
            return None;
        }
        if let Some(start) = self.working_since.take() {
            self.pending = Some((now + CONFIRM, start, now));
        }
        match self.pending {
            Some((at, start, stopped)) if now >= at => {
                self.pending = None;
                // Los `WORK_WINDOW` de silencio que `sessions` cuenta como trabajo no se suman.
                let worked = stopped.duration_since(start).saturating_sub(crate::sessions::WORK_WINDOW);
                (worked >= min_work).then_some(worked)
            }
            _ => None,
        }
    }

    /// Cuándo hay que despertar para confirmar un aviso pendiente.
    pub fn next_wake(&self) -> Option<Instant> {
        self.pending.map(|(at, ..)| at)
    }
}

/// ¿Se avisa con esta configuración? `viewing` = el usuario está mirando la pestaña.
pub fn wanted(cfg: &NotifyConfig, viewing: bool) -> bool {
    match cfg.agents {
        NotifyWhen::Never => false,
        NotifyWhen::Always => true,
        NotifyWhen::Unfocused => !viewing,
    }
}

pub struct Notice {
    pub tab: u64,
    pub title: String,
    pub body: String,
    pub sound: bool,
}

/// Procesos `notify-send` sin recoger. Se recogen con este cerrojo tomado y solo se terminan los que
/// siguen aquí: su pid no puede haberse reutilizado para otro proceso.
static LIVE: Mutex<Option<HashSet<u32>>> = Mutex::new(None);

/// Texto del aviso: "Claude Code ha terminado" / "Rediseñar los ajustes\nGRBTER · trabajó 2 min".
pub fn notice_text(agent: &str, task: &str, dir: &str, worked: Duration) -> (String, String) {
    let mut lines = Vec::new();
    if !task.is_empty() && task != agent && task != dir {
        lines.push(task.to_string());
    }
    let took = format!("trabajó {}", crate::blocks::format_duration(worked));
    lines.push(if dir.is_empty() { took } else { format!("{dir} · {took}") });
    (format!("{agent} ha terminado"), lines.join("\n"))
}

/// El cuerpo admite marcas (negrita…) en GNOME: lo que venga del agente se escapa.
fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_control() && c != '\n' { ' ' } else { c }).collect()
}


/// Envía el aviso en segundo plano. Avisa a la app del proceso (`NoticeShown`) y de si se pulsa
/// (`NoticeClicked`, con el permiso de activación si el escritorio lo da).
pub fn send(n: Notice, proxy: EventLoopProxy<UserEvent>) {
    let mut cmd = Command::new("notify-send");
    cmd.args(["-a", "Garbo", "-i", APP_ID, "-h"]).arg(format!("string:desktop-entry:{APP_ID}"));
    // El permiso de activación sale por stdout justo antes del nombre de la acción.
    cmd.args(["-A", "default=Abrir", "--activation-token-fd=1", "-h"]);
    cmd.arg(if n.sound { "string:sound-name:complete" } else { "boolean:suppress-sound:true" });
    cmd.arg("--").arg(clean(&n.title)).arg(escape_markup(&clean(&n.body)));
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("No se pudo enviar el aviso (¿falta notify-send, del paquete libnotify?): {e}");
            return;
        }
    };
    let pid = child.id();
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).insert(pid);
    log::info!("Aviso (notify-send {pid}): {}", n.title);
    let _ = proxy.send_event(UserEvent::NoticeShown(n.tab, pid));
    std::thread::spawn(move || {
        if let Some(out) = child.stdout.take() {
            let mut token = None;
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(token) = clicked(&line, &mut token) {
                    let _ = proxy.send_event(UserEvent::NoticeClicked(n.tab, token));
                }
            }
        }
        let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(l) = live.as_mut() {
            l.remove(&pid);
        }
        let _ = child.wait();
    });
}

/// Lee una línea de `notify-send`: el permiso de activación se guarda hasta que llega la acción
/// `default` (aviso pulsado), que devuelve el permiso (o nada si el escritorio no lo dio).
fn clicked(line: &str, token: &mut Option<String>) -> Option<Option<String>> {
    let line = line.trim();
    if line == "default" {
        return Some(token.take());
    }
    if crate::activation::valid(line) {
        *token = Some(line.to_string());
    }
    None
}

/// Retira el aviso de un `notify-send` (si sigue en marcha).
pub fn close(pid: u32) {
    let live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    if live.as_ref().is_some_and(|l| l.contains(&pid)) {
        // SAFETY: kill solo manda una señal; el pid es de un hijo nuestro aún sin recoger.
        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avisa_tras_confirmar_y_con_trabajo_suficiente() {
        let t0 = Instant::now();
        let s = |ms: u64| t0 + Duration::from_millis(ms);
        let min = Duration::from_secs(5);
        let mut t = Tracker::default();
        assert_eq!(t.update(true, min, s(0)), None);
        assert_eq!(t.update(true, min, s(9_000)), None);
        // Para a los 10 s (los 1,5 s de silencio ya pasaron): queda pendiente de confirmar.
        assert_eq!(t.update(false, min, s(10_000)), None);
        assert_eq!(t.next_wake(), Some(s(10_000) + CONFIRM));
        assert_eq!(t.update(false, min, s(11_000)), None, "aún sin confirmar");
        assert_eq!(t.update(false, min, s(12_600)), Some(Duration::from_millis(8_500)));
        assert_eq!(t.next_wake(), None, "solo una vez");
        assert_eq!(t.update(false, min, s(20_000)), None);

        // Una pausa corta no avisa y cuenta como la misma tarea.
        let mut t = Tracker::default();
        t.update(true, min, s(0));
        t.update(false, min, s(4_000));
        t.update(true, min, s(5_000));
        t.update(false, min, s(8_000));
        assert_eq!(t.update(false, min, s(10_600)), Some(Duration::from_millis(6_500)));

        // Poco trabajo: sin aviso.
        let mut t = Tracker::default();
        t.update(true, min, s(0));
        t.update(false, min, s(3_000));
        assert_eq!(t.update(false, min, s(6_000)), None);
    }

    #[test]
    fn texto_y_politica() {
        let (title, body) = notice_text("Claude Code", "Rediseñar los ajustes", "GRBTER", Duration::from_secs(125));
        assert_eq!(title, "Claude Code ha terminado");
        assert_eq!(body, "Rediseñar los ajustes\nGRBTER · trabajó 2 min 05 s");
        let (_, body) = notice_text("Aider", "Aider", "src", Duration::from_secs(7));
        assert_eq!(body, "src · trabajó 7,0 s", "sin repetir el nombre del agente");
        assert_eq!(escape_markup("a <b> & c"), "a &lt;b&gt; &amp; c");
        assert_eq!(clean("x\u{1b}[31my"), "x [31my");
        let mut cfg = NotifyConfig::default();
        assert!(wanted(&cfg, false) && !wanted(&cfg, true), "por defecto, solo si no se está viendo");
        cfg.agents = NotifyWhen::Always;
        assert!(wanted(&cfg, true));
        cfg.agents = NotifyWhen::Never;
        assert!(!wanted(&cfg, false));
    }

    #[test]
    fn permiso_de_activacion_antes_del_clic() {
        let mut token = None;
        assert_eq!(clicked("2b1f6c0e-9a7d-4c1e-8f3a-5d6e7f8a9b0c", &mut token), None);
        assert_eq!(clicked("default", &mut token), Some(Some("2b1f6c0e-9a7d-4c1e-8f3a-5d6e7f8a9b0c".into())));
        assert_eq!(clicked("default", &mut token), Some(None), "sin permiso: se pulsa igual");
        assert_eq!(clicked("con espacios raros", &mut token), None);
        assert_eq!(token, None);
    }
}
