//! Atajos de teclado configurables: `accion = "ctrl+shift+t, ctrl+n"`.

use std::collections::HashMap;
use winit::keyboard::{Key, ModifiersState, NamedKey};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    MoveTabLeft,
    MoveTabRight,
    GotoTab(usize),
    Copy,
    Paste,
    FontBigger,
    FontSmaller,
    FontReset,
    ScrollPageUp,
    ScrollPageDown,
    ScrollTop,
    ScrollBottom,
    ClearScrollback,
    ReloadConfig,
    CommandPalette,
    NextTheme,
    PrevBlock,
    NextBlock,
    CopyBlockOutput,
    CopyBlockCommand,
    CopyBlock,
    Settings,
}

impl Action {
    /// (nombre en config, descripción para la paleta, combinaciones por defecto)
    pub fn all() -> Vec<(Action, &'static str, &'static str, &'static str)> {
        use Action::*;
        let mut v = vec![
            (NewTab, "new_tab", "Nueva pestaña", "ctrl+shift+t"),
            (CloseTab, "close_tab", "Cerrar pestaña", "ctrl+shift+w"),
            (NextTab, "next_tab", "Pestaña siguiente", "ctrl+tab, ctrl+pagedown"),
            (PrevTab, "prev_tab", "Pestaña anterior", "ctrl+shift+tab, ctrl+pageup"),
            (MoveTabLeft, "move_tab_left", "Mover pestaña a la izquierda", "ctrl+shift+pageup"),
            (MoveTabRight, "move_tab_right", "Mover pestaña a la derecha", "ctrl+shift+pagedown"),
            (Copy, "copy", "Copiar selección", "ctrl+shift+c"),
            (Paste, "paste", "Pegar", "ctrl+shift+v, shift+insert"),
            (FontBigger, "font_bigger", "Aumentar fuente", "ctrl+plus, ctrl+equal"),
            (FontSmaller, "font_smaller", "Reducir fuente", "ctrl+minus"),
            (FontReset, "font_reset", "Tamaño de fuente original", "ctrl+0"),
            (ScrollPageUp, "scroll_page_up", "Subir una página", "shift+pageup"),
            (ScrollPageDown, "scroll_page_down", "Bajar una página", "shift+pagedown"),
            (ScrollTop, "scroll_top", "Ir al principio del historial", "shift+home"),
            (ScrollBottom, "scroll_bottom", "Ir al final", "shift+end"),
            (ClearScrollback, "clear_scrollback", "Limpiar historial", "ctrl+shift+k"),
            (ReloadConfig, "reload_config", "Recargar configuración", "ctrl+shift+r"),
            (CommandPalette, "command_palette", "Paleta de comandos", "ctrl+shift+p"),
            (NextTheme, "next_theme", "Cambiar al siguiente tema", "ctrl+shift+y"),
            (PrevBlock, "prev_block", "Bloque anterior", "ctrl+shift+up"),
            (NextBlock, "next_block", "Bloque siguiente", "ctrl+shift+down"),
            (CopyBlockOutput, "copy_block_output", "Copiar salida del bloque", "ctrl+shift+o"),
            (CopyBlockCommand, "copy_block_command", "Copiar comando del bloque", ""),
            (CopyBlock, "copy_block", "Copiar bloque entero", ""),
            (Settings, "settings", "Ajustes", "ctrl+comma"),
        ];
        const GOTO: [&str; 9] = [
            "goto_tab_1", "goto_tab_2", "goto_tab_3", "goto_tab_4", "goto_tab_5",
            "goto_tab_6", "goto_tab_7", "goto_tab_8", "goto_tab_9",
        ];
        const GOTO_KEYS: [&str; 9] = ["alt+1", "alt+2", "alt+3", "alt+4", "alt+5", "alt+6", "alt+7", "alt+8", "alt+9"];
        for i in 0..9 {
            v.push((GotoTab(i), GOTO[i], "Ir a pestaña", GOTO_KEYS[i]));
        }
        v
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct Combo {
    mods: ModifiersState,
    key: String,
}

pub struct Keybindings {
    map: HashMap<Combo, Action>,
    /// Para mostrar el atajo en la paleta de comandos.
    pub labels: HashMap<String, String>,
}

fn parse_combo(s: &str) -> Option<Combo> {
    let mut mods = ModifiersState::empty();
    let mut key = None;
    for part in s.split('+').map(|p| p.trim().to_lowercase()) {
        match part.as_str() {
            "ctrl" | "control" => mods |= ModifiersState::CONTROL,
            "shift" => mods |= ModifiersState::SHIFT,
            "alt" | "option" => mods |= ModifiersState::ALT,
            "super" | "cmd" | "meta" | "win" => mods |= ModifiersState::SUPER,
            "" => key = Some("plus".to_string()), // "ctrl++"
            k => key = Some(normalize_name(k)),
        }
    }
    Some(Combo { mods, key: key? })
}

fn normalize_name(k: &str) -> String {
    match k {
        "+" => "plus",
        "-" => "minus",
        "=" => "equal",
        "," => "comma",
        "." => "period",
        "pgup" => "pageup",
        "pgdn" => "pagedown",
        "esc" => "escape",
        "return" => "enter",
        "ins" => "insert",
        "del" => "delete",
        other => other,
    }
    .to_string()
}

/// Nombre normalizado de la tecla pulsada (independiente de Shift para letras).
pub fn key_name(key: &Key) -> Option<String> {
    Some(match key {
        Key::Character(s) => normalize_name(&s.to_lowercase()),
        Key::Named(n) => match n {
            NamedKey::Tab => "tab".into(),
            NamedKey::PageUp => "pageup".into(),
            NamedKey::PageDown => "pagedown".into(),
            NamedKey::Home => "home".into(),
            NamedKey::End => "end".into(),
            NamedKey::Insert => "insert".into(),
            NamedKey::Delete => "delete".into(),
            NamedKey::Enter => "enter".into(),
            NamedKey::Escape => "escape".into(),
            NamedKey::Space => "space".into(),
            NamedKey::Backspace => "backspace".into(),
            NamedKey::ArrowUp => "up".into(),
            NamedKey::ArrowDown => "down".into(),
            NamedKey::ArrowLeft => "left".into(),
            NamedKey::ArrowRight => "right".into(),
            NamedKey::F1 => "f1".into(),
            NamedKey::F2 => "f2".into(),
            NamedKey::F3 => "f3".into(),
            NamedKey::F4 => "f4".into(),
            NamedKey::F5 => "f5".into(),
            NamedKey::F6 => "f6".into(),
            NamedKey::F7 => "f7".into(),
            NamedKey::F8 => "f8".into(),
            NamedKey::F9 => "f9".into(),
            NamedKey::F10 => "f10".into(),
            NamedKey::F11 => "f11".into(),
            NamedKey::F12 => "f12".into(),
            _ => return None,
        },
        _ => return None,
    })
}

impl Keybindings {
    pub fn new(user: &HashMap<String, String>) -> Self {
        let mut map = HashMap::new();
        let mut labels = HashMap::new();
        for (action, name, _, default) in Action::all() {
            let spec = user.get(name).map(String::as_str).unwrap_or(default);
            if spec.trim().eq_ignore_ascii_case("none") {
                continue;
            }
            labels.insert(name.to_string(), spec.split(',').next().unwrap_or("").trim().to_string());
            for combo in spec.split(',').filter_map(parse_combo) {
                map.insert(combo, action);
            }
        }
        for name in user.keys() {
            if !Action::all().iter().any(|(_, n, _, _)| n == name) {
                log::warn!("Acción de atajo desconocida: {name}");
            }
        }
        Self { map, labels }
    }

    pub fn lookup(&self, key: &Key, mods: ModifiersState) -> Option<Action> {
        let name = key_name(key)?;
        let is_symbol = matches!(key, Key::Character(s) if !s.chars().all(char::is_alphabetic));
        if let Some(a) = self.map.get(&Combo { mods, key: name.clone() }) {
            return Some(*a);
        }
        // En muchos teclados '+' o '?' requieren Shift: "ctrl+plus" debe funcionar igual.
        if is_symbol && mods.shift_key() {
            return self.map.get(&Combo { mods: mods - ModifiersState::SHIFT, key: name }).copied();
        }
        None
    }
}
