//! Garbo — terminal GPU ligera con pestañas.

mod app;
mod atlas;
mod blocks;
mod colr;
mod config;
mod events;
mod font;
mod input;
mod keybindings;
mod palette;
mod panel;
mod ports;
mod pty_loop;
mod renderer;
mod sessions;
mod settings;
mod subagents;
mod complete;
mod suggest;
mod clipboard;
mod terminal;
mod ui;
mod update;
mod view;

use winit::event_loop::EventLoop;

/// Identificador de la aplicación (Wayland app_id, .desktop, icono).
pub const APP_ID: &str = "io.github.garbanhh.Garbo";

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,garbo=info")).init();

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
