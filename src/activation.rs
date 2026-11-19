//! Traer la ventana al frente al pulsar un aviso.
//!
//! En Wayland una ventana no puede ponerse delante por su cuenta (`focus_window` de winit no hace
//! nada): necesita un permiso de activación (*xdg-activation*) de quien recibió el clic. Al pulsar
//! un aviso con acción `default`, GNOME (y KDE) entregan ese permiso con la señal `ActivationToken`;
//! `notify-send --activation-token-fd` lo escribe y Garbo lo usa aquí con la superficie de su
//! ventana. winit solo lo admite al crear una ventana, así que se habla con Wayland directamente,
//! sobre la misma conexión de winit (como el portapapeles).

use std::cell::RefCell;
use std::mem::ManuallyDrop;
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use winit::window::Window;

/// Pone la ventana delante: con el permiso del aviso si lo hay (Wayland) y, si no, como se pueda.
pub fn raise(window: &Window, token: Option<&str>) {
    if let Some(token) = token.filter(|t| valid(t)) {
        match activate(window, token) {
            Ok(true) => return,
            Ok(false) => {}
            Err(e) => log::warn!("No se pudo traer la ventana al frente con el permiso del aviso: {e}"),
        }
    }
    window.focus_window();
}

/// Un permiso es texto ASCII visible (GNOME: un UUID; KDE: otro identificador).
pub fn valid(token: &str) -> bool {
    !token.is_empty() && token.len() <= 1024 && token.bytes().all(|b| b.is_ascii_graphic())
}

struct State;

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<XdgActivationV1, ()> for State {
    fn event(_: &mut Self, _: &XdgActivationV1, _: <XdgActivationV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

/// Conexión propia sobre el display de winit y el `xdg_activation_v1` del compositor. Se crea al
/// primer uso y se conserva (la cola debe vivir mientras vivan sus objetos) hasta `release`.
struct Wayland {
    conn: Connection,
    _queue: EventQueue<State>,
    activation: XdgActivationV1,
}

thread_local! {
    /// `ManuallyDrop`: soltarla al acabar el hilo principal, cuando winit ya cerró el display,
    /// rompería Garbo al salir (SIGSEGV en `wl_proxy_destroy`). Se suelta en `release`.
    static WAYLAND: RefCell<Option<ManuallyDrop<Wayland>>> = const { RefCell::new(None) };
}

/// Suelta la conexión. Hay que llamarla antes de cerrar la ventana (en `exiting`), con el display
/// de winit aún abierto.
pub fn release() {
    if let Some(mut wl) = WAYLAND.with_borrow_mut(Option::take) {
        wl.activation.destroy();
        let _ = wl.conn.flush();
        // SAFETY: se saca del hilo local y no se vuelve a usar.
        unsafe { ManuallyDrop::drop(&mut wl) };
    }
}

/// `Ok(false)`: no es Wayland o el compositor no admite activación.
fn activate(window: &Window, token: &str) -> Result<bool, String> {
    let (Ok(display), Ok(handle)) = (window.display_handle(), window.window_handle()) else {
        return Ok(false);
    };
    let (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(handle)) = (display.as_raw(), handle.as_raw()) else {
        return Ok(false);
    };
    WAYLAND.with_borrow_mut(|wl| {
        if wl.is_none() {
            // SAFETY: el display es el de winit y vive tanto como la ventana (dueña de la app).
            let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
            let conn = Connection::from_backend(backend);
            let (globals, queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
            let Ok(activation) = globals.bind::<XdgActivationV1, _, _>(&queue.handle(), 1..=1, ()) else {
                return Ok(false);
            };
            *wl = Some(ManuallyDrop::new(Wayland { conn, _queue: queue, activation }));
        }
        let wl = wl.as_ref().expect("recién creada");
        // SAFETY: la superficie es la de la ventana de winit, viva mientras lo esté la ventana.
        let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), handle.surface.as_ptr().cast()) }
            .map_err(|e| e.to_string())?;
        let surface = WlSurface::from_id(&wl.conn, id).map_err(|e| e.to_string())?;
        wl.activation.activate(token.to_string(), &surface);
        wl.conn.flush().map_err(|e| e.to_string())?;
        Ok(true)
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn permisos_validos() {
        assert!(super::valid("1e0c0b7e-6f1d-4a49-9c7e-1f2a3b4c5d6e"));
        assert!(super::valid("kwin-12_TIME345"));
        assert!(!super::valid(""));
        assert!(!super::valid("con espacio"));
        assert!(!super::valid("salto\n"));
        assert!(!super::valid(&"x".repeat(2000)));
    }
}
