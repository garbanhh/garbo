//! Traducción de teclado y ratón a secuencias que entiende el programa del PTY (xterm).

use alacritty_terminal::term::TermMode;
use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};

fn mod_code(m: ModifiersState) -> u8 {
    1 + m.shift_key() as u8 + 2 * m.alt_key() as u8 + 4 * m.control_key() as u8
}

/// `CSI 1;m X` o `SS3 X` / `CSI X` según modo y modificadores.
pub(crate) fn cursor_key(letter: char, m: ModifiersState, app: bool) -> Vec<u8> {
    let code = mod_code(m);
    if code > 1 {
        format!("\x1b[1;{code}{letter}").into_bytes()
    } else if app {
        format!("\x1bO{letter}").into_bytes()
    } else {
        format!("\x1b[{letter}").into_bytes()
    }
}

fn tilde_key(n: u8, m: ModifiersState) -> Vec<u8> {
    let code = mod_code(m);
    if code > 1 {
        format!("\x1b[{n};{code}~").into_bytes()
    } else {
        format!("\x1b[{n}~").into_bytes()
    }
}

fn ss3_fkey(letter: char, m: ModifiersState) -> Vec<u8> {
    let code = mod_code(m);
    if code > 1 {
        format!("\x1b[1;{code}{letter}").into_bytes()
    } else {
        format!("\x1bO{letter}").into_bytes()
    }
}

pub fn key_to_bytes(ev: &KeyEvent, m: ModifiersState, mode: TermMode) -> Option<Vec<u8>> {
    let app = mode.contains(TermMode::APP_CURSOR);
    let alt = |mut v: Vec<u8>| {
        if m.alt_key() {
            v.insert(0, 0x1b);
        }
        v
    };
    if let Key::Named(named) = &ev.logical_key {
        let bytes = match named {
            NamedKey::Enter => alt(vec![b'\r']),
            NamedKey::Backspace => alt(vec![if m.control_key() { 0x08 } else { 0x7f }]),
            NamedKey::Tab if m.shift_key() => b"\x1b[Z".to_vec(),
            NamedKey::Tab => alt(vec![b'\t']),
            NamedKey::Escape => vec![0x1b],
            NamedKey::Space if m.control_key() => vec![0],
            NamedKey::Space => alt(vec![b' ']),
            NamedKey::ArrowUp => cursor_key('A', m, app),
            NamedKey::ArrowDown => cursor_key('B', m, app),
            NamedKey::ArrowRight => cursor_key('C', m, app),
            NamedKey::ArrowLeft => cursor_key('D', m, app),
            NamedKey::Home => cursor_key('H', m, app),
            NamedKey::End => cursor_key('F', m, app),
            NamedKey::Insert => tilde_key(2, m),
            NamedKey::Delete => tilde_key(3, m),
            NamedKey::PageUp => tilde_key(5, m),
            NamedKey::PageDown => tilde_key(6, m),
            NamedKey::F1 => ss3_fkey('P', m),
            NamedKey::F2 => ss3_fkey('Q', m),
            NamedKey::F3 => ss3_fkey('R', m),
            NamedKey::F4 => ss3_fkey('S', m),
            NamedKey::F5 => tilde_key(15, m),
            NamedKey::F6 => tilde_key(17, m),
            NamedKey::F7 => tilde_key(18, m),
            NamedKey::F8 => tilde_key(19, m),
            NamedKey::F9 => tilde_key(20, m),
            NamedKey::F10 => tilde_key(21, m),
            NamedKey::F11 => tilde_key(23, m),
            NamedKey::F12 => tilde_key(24, m),
            _ => return ev.text.as_ref().map(|t| alt(t.as_bytes().to_vec())),
        };
        return Some(bytes);
    }

    if m.control_key() {
        if let Key::Character(s) = &ev.logical_key {
            let mut chars = s.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                let b = match c.to_ascii_lowercase() {
                    c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
                    '@' | '2' | ' ' => Some(0),
                    '[' | '3' => Some(0x1b),
                    '\\' | '4' => Some(0x1c),
                    ']' | '5' => Some(0x1d),
                    '^' | '6' => Some(0x1e),
                    '_' | '-' | '/' | '7' => Some(0x1f),
                    '?' | '8' => Some(0x7f),
                    _ => None,
                };
                if let Some(b) = b {
                    return Some(alt(vec![b]));
                }
            }
        }
    }

    ev.text
        .as_ref()
        .filter(|t| !t.is_empty())
        .map(|t| alt(t.as_bytes().to_vec()))
}

/// Tecla como evento de teclado de Windows (`win32-input-mode`, el que pide ConPTY al empezar):
/// `ESC [ Vk ; Sc ; Uc ; Kd ; Cs ; Rc _`, con la tecla virtual, el código de exploración, el
/// carácter (UTF-16), pulsada, los modificadores y las repeticiones. Así el programa recibe la
/// tecla exacta (PSReadLine distingue Shift+Intro, Ctrl+Retroceso, Ctrl+Z…), como en Windows
/// Terminal. Un carácter fuera del plano básico va en dos eventos, uno por mitad.
#[cfg(windows)]
pub fn win32_key(ev: &KeyEvent, m: ModifiersState) -> Option<Vec<u8>> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW, MAPVK_VSC_TO_VK_EX};
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
    use winit::platform::scancode::PhysicalKeyExtScancode;
    let scancode = ev.physical_key.to_scancode().unwrap_or(0);
    // SAFETY: MapVirtualKeyW solo consulta la distribución de teclado activa.
    let vk = if scancode == 0 {
        0
    } else {
        unsafe { MapVirtualKeyW(scancode, MAPVK_VSC_TO_VK_EX) }
    };
    let text = ev.text_with_all_modifiers().unwrap_or_default();
    if vk == 0 && text.is_empty() {
        return None;
    }
    // Modificadores como los da Windows (sin distinguir izquierda y derecha: la izquierda).
    let mut state = 0u32;
    if m.alt_key() {
        state |= 0x02; // LEFT_ALT_PRESSED
    }
    if m.control_key() {
        state |= 0x08; // LEFT_CTRL_PRESSED
    }
    if m.shift_key() {
        state |= 0x10; // SHIFT_PRESSED
    }
    if scancode & 0xE000 != 0 {
        state |= 0x100; // ENHANCED_KEY (flechas, Insert, Supr, Inicio, Fin…)
    }
    Some(win32_sequence(vk, scancode & 0xFF, text, state))
}

/// `ESC [ Vk ; Sc ; Uc ; 1 ; Cs ; 1 _` para una tecla pulsada (y uno más por cada mitad de un
/// carácter fuera del plano básico).
#[cfg(windows)]
pub fn win32_sequence(vk: u32, sc: u32, text: &str, state: u32) -> Vec<u8> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let first = units.first().copied().unwrap_or(0);
    let mut out = format!("\x1b[{vk};{sc};{first};1;{state};1_").into_bytes();
    for unit in units.iter().skip(1) {
        out.extend(format!("\x1b[0;0;{unit};1;{state};1_").into_bytes());
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseButtonCode {
    Left = 0,
    Middle = 1,
    Right = 2,
    WheelUp = 64,
    WheelDown = 65,
}

/// Informe de ratón para apps que lo piden (vim, htop, tmux...). `col`/`line` en base 0.
pub fn mouse_report(
    button: MouseButtonCode,
    pressed: bool,
    motion: bool,
    col: usize,
    line: usize,
    m: ModifiersState,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let mut code = button as u8;
    if motion {
        code += 32;
    }
    code += 4 * m.shift_key() as u8 + 8 * m.alt_key() as u8 + 16 * m.control_key() as u8;
    if mode.contains(TermMode::SGR_MOUSE) {
        let suffix = if pressed { 'M' } else { 'm' };
        Some(format!("\x1b[<{code};{};{}{suffix}", col + 1, line + 1).into_bytes())
    } else {
        if !pressed {
            code = 3 + (code & !3);
        }
        if col > 222 || line > 222 {
            return None;
        }
        Some(vec![
            0x1b,
            b'[',
            b'M',
            32 + code,
            33 + col as u8,
            33 + line as u8,
        ])
    }
}
