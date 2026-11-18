//! Traducción de teclado y ratón a secuencias que entiende el programa del PTY (xterm).

use alacritty_terminal::term::TermMode;
use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};

fn mod_code(m: ModifiersState) -> u8 {
    1 + m.shift_key() as u8 + 2 * m.alt_key() as u8 + 4 * m.control_key() as u8
}

/// `CSI 1;m X` o `SS3 X` / `CSI X` según modo y modificadores.
fn cursor_key(letter: char, m: ModifiersState, app: bool) -> Vec<u8> {
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

    ev.text.as_ref().filter(|t| !t.is_empty()).map(|t| alt(t.as_bytes().to_vec()))
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
        Some(vec![0x1b, b'[', b'M', 32 + code, 33 + col as u8, 33 + line as u8])
    }
}
