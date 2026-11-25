//! Garbo — terminal GPU ligera con pestañas.

// Sin consola propia en Windows (la ventana es la terminal); con `cargo test` sí.
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
mod activation;
mod app;
mod atlas;
mod blocks;
mod changes;
mod clipboard;
mod colr;
mod complete;
mod config;
mod edit;
mod events;
mod font;
mod git;
mod input;
mod keybindings;
mod links;
mod logos;
mod look;
mod notify;
mod palette;
mod panel;
mod panes;
mod platform;
mod ports;
mod pty_loop;
mod renderer;
mod search;
mod sessions;
mod settings;
mod shell;
mod stats;
mod status;
mod style;
mod subagents;
mod suggest;
mod tabs;
mod temp_dir;
mod terminal;
#[cfg(windows)]
mod toast;
mod ui;
mod update;
mod view;

/// Traer la ventana al frente al pulsar un aviso. Solo Wayland necesita un permiso de activación
/// (`activation.rs`); en el resto basta con pedírselo a la ventana.
#[cfg(not(target_os = "linux"))]
mod activation {
    /// Minimizada, `focus_window` no hace nada: primero se restaura.
    pub fn raise(window: &winit::window::Window, _token: Option<&str>) {
        if window.is_minimized() == Some(true) {
            window.set_minimized(false);
        }
        window.focus_window();
    }

    pub fn release() {}

    #[allow(dead_code)]
    pub fn valid(token: &str) -> bool {
        !token.is_empty() && token.len() <= 1024 && token.bytes().all(|b| b.is_ascii_graphic())
    }
}

use winit::event_loop::EventLoop;

/// Identificador de la aplicación (Wayland app_id, .desktop, icono).
pub const APP_ID: &str = "io.github.garbanhh.Garbo";

fn main() {
    #[cfg(windows)]
    {
        platform::harden_dll_search();
        platform::set_app_user_model_id(APP_ID);
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,garbo=info"))
        .init();

    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("garbo {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if std::env::args().any(|a| a == "--print-config-path") {
        if let Some(p) = config::Config::path() {
            println!("{}", p.display());
        }
        return;
    }

    // Debe hacerse antes de crear cualquier hilo.
    renderer::limit_vulkan_drivers();

    let cfg = config::Config::load();
    let event_loop = EventLoop::<terminal::UserEvent>::with_user_event()
        .build()
        .expect("no se pudo iniciar el bucle de eventos");
    let mut app = app::App::new(cfg, event_loop.create_proxy());
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("garbo: {e}");
        std::process::exit(1);
    }
}
