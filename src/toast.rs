//! Avisos del escritorio en Windows: notificaciones del sistema (las del centro de notificaciones),
//! con la API de WinRT (`ToastNotification`). Misma interfaz que los de Linux (`notify.rs`): `send`
//! avisa a la app con `NoticeShown(pestaña, id)` y, al pulsarlo, con `NoticeClicked`; `close(id)`
//! lo retira (al mirar la pestaña o al salir).
//!
//! Una app sin instalar (sin acceso directo en el menú Inicio) solo puede mostrar avisos si su
//! identificador (`APP_ID`) está registrado en el registro del usuario con su nombre y su icono:
//! se registra una vez, al enviar el primero. El clic llega mientras Garbo está abierto.

use crate::notify::Notice;
use crate::terminal::UserEvent;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, Once};
use windows::core::HSTRING;
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use winit::event_loop::EventLoopProxy;

/// Grupo de los avisos de Garbo en el centro de notificaciones (cada uno lleva su id de etiqueta).
const GROUP: &str = "garbo";

/// Avisos mostrados y aún sin retirar.
static SHOWN: Mutex<Option<HashMap<u32, ToastNotification>>> = Mutex::new(None);
static NEXT: AtomicU32 = AtomicU32::new(1);

/// Envía el aviso. Si Windows no lo admite (avisos desactivados para Garbo, sin servicio de
/// notificaciones), solo se anota en el registro.
pub fn send(n: Notice, proxy: EventLoopProxy<UserEvent>) {
    // Al pulsarlo (llega en otro hilo): a la pestaña del aviso.
    let tab = n.tab;
    let clicked = Mutex::new(proxy.clone());
    let on_click = move || {
        if let Ok(p) = clicked.lock() {
            let _ = p.send_event(UserEvent::NoticeClicked(tab, None));
        }
    };
    match show(&n, on_click) {
        Ok(id) => {
            log::info!("Aviso ({id}): {}", n.title);
            let _ = proxy.send_event(UserEvent::NoticeShown(n.tab, id));
        }
        Err(e) => log::warn!("No se pudo mostrar el aviso: {e}"),
    }
}

fn show(n: &Notice, on_click: impl Fn() + Send + 'static) -> windows::core::Result<u32> {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(register_app);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(toast_xml(&n.title, &n.body, n.sound)))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    toast.SetTag(&HSTRING::from(id.to_string()))?;
    toast.SetGroup(&HSTRING::from(GROUP))?;
    toast.Activated(&TypedEventHandler::new(move |_, _| {
        on_click();
        Ok(())
    }))?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(crate::APP_ID))?
        .Show(&toast)?;
    SHOWN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(id, toast);
    Ok(id)
}

/// Retira el aviso de la pantalla y del centro de notificaciones.
pub fn close(id: u32) {
    let toast = SHOWN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .and_then(|m| m.remove(&id));
    let Some(toast) = toast else {
        return;
    };
    let app = HSTRING::from(crate::APP_ID);
    if let Ok(notifier) = ToastNotificationManager::CreateToastNotifierWithId(&app) {
        let _ = notifier.Hide(&toast);
    }
    if let Ok(history) = ToastNotificationManager::History() {
        let _ = history.RemoveGroupedTagWithId(
            &HSTRING::from(id.to_string()),
            &HSTRING::from(GROUP),
            &app,
        );
    }
}

/// XML del aviso: el título y hasta dos líneas de texto (lo que admite la plantilla genérica), sin
/// sonido si así se configuró.
fn toast_xml(title: &str, body: &str, sound: bool) -> String {
    let mut lines = body.lines().map(str::trim).filter(|l| !l.is_empty());
    let mut texts = format!("<text>{}</text>", xml_escape(title));
    if let Some(first) = lines.next() {
        texts += &format!("<text>{}</text>", xml_escape(first));
    }
    let rest: Vec<&str> = lines.collect();
    if !rest.is_empty() {
        texts += &format!("<text>{}</text>", xml_escape(&rest.join(" · ")));
    }
    let audio = if sound {
        ""
    } else {
        r#"<audio silent="true"/>"#
    };
    format!(
        r#"<toast activationType="foreground" launch="garbo"><visual><binding template="ToastGeneric">{texts}</binding></visual>{audio}</toast>"#
    )
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Nombre e icono del identificador de Garbo en el registro del usuario
/// (`HKCU\Software\Classes\AppUserModelId\<APP_ID>`), que Windows usa para mostrar sus avisos.
fn register_app() {
    let icon = dirs::data_local_dir().map(|d| d.join("garbo").join("garbo.png"));
    if let Some(icon) = &icon {
        const PNG: &[u8] = include_bytes!("../packaging/io.github.garbanhh.Garbo.png");
        if std::fs::read(icon).ok().as_deref() != Some(PNG) {
            let _ = std::fs::create_dir_all(icon.parent().unwrap_or(icon));
            let _ = std::fs::write(icon, PNG);
        }
    }
    let key = format!(r"Software\Classes\AppUserModelId\{}", crate::APP_ID);
    let mut values = vec![("DisplayName", "Garbo".to_string())];
    if let Some(icon) = icon {
        values.push(("IconUri", icon.to_string_lossy().into_owned()));
    }
    for (name, value) in values {
        if !crate::platform::set_user_registry(&key, name, &value) {
            log::warn!("No se pudo registrar {name} de los avisos");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_del_aviso() {
        let xml = toast_xml(
            "Claude Code ha terminado",
            "Rediseñar <ajustes>\ngarbo · trabajó 2 min",
            false,
        );
        assert!(xml.contains("<text>Claude Code ha terminado</text>"));
        assert!(xml.contains("<text>Rediseñar &lt;ajustes&gt;</text>"));
        assert!(xml.contains("<text>garbo · trabajó 2 min</text>"));
        assert!(xml.contains(r#"<audio silent="true"/>"#));
        assert!(!toast_xml("a", "", true).contains("audio"));
        // Más de dos líneas: la segunda y siguientes van juntas.
        assert!(toast_xml("a", "uno\ndos\ntres", true).contains("<text>dos · tres</text>"));
    }

    /// Aviso real: Windows lo entrega (está en el historial del centro de notificaciones con su
    /// etiqueta) y `close` lo retira. Ignorada: registra el identificador de Garbo en el registro
    /// del usuario y muestra un aviso. `cargo test aviso_real -- --ignored`
    #[test]
    #[ignore = "muestra un aviso de verdad"]
    fn aviso_real() {
        let n = Notice {
            tab: 0,
            title: "Prueba de Garbo".into(),
            body: "Aviso de prueba de cargo test\nse retira solo".into(),
            sound: false,
        };
        let id = show(&n, || {}).expect("aviso mostrado");
        let in_history = || {
            let app = HSTRING::from(crate::APP_ID);
            let list = ToastNotificationManager::History()
                .and_then(|h| h.GetHistoryWithId(&app))
                .expect("historial");
            (0..list.Size().unwrap()).any(|i| {
                list.GetAt(i)
                    .and_then(|t| t.Tag())
                    .is_ok_and(|tag| tag == id.to_string())
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(1500));
        assert!(in_history(), "Windows no entregó el aviso");
        close(id);
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(!in_history(), "el aviso no se retiró");
    }
}
