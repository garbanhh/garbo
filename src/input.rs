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
pub fn win32_key(ev: &KeyEvent) -> Option<Vec<u8>> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, MapVirtualKeyW, MAPVK_VSC_TO_VK_EX, VK_CAPITAL, VK_LCONTROL, VK_LMENU,
        VK_NUMLOCK, VK_RCONTROL, VK_RMENU, VK_SCROLL, VK_SHIFT,
    };
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
    use winit::platform::scancode::PhysicalKeyExtScancode;
    let scancode = ev.physical_key.to_scancode().unwrap_or(0);
    // SAFETY: MapVirtualKeyW solo consulta la distribución de teclado activa.
    let mapped = if scancode == 0 {
        0
    } else {
        unsafe { MapVirtualKeyW(scancode, MAPVK_VSC_TO_VK_EX) }
    };
    let text = ev.text_with_all_modifiers().unwrap_or_default();
    let vk = win32_vk(mapped, scancode, text);
    if vk == 0 && text.is_empty() {
        return None;
    }
    // Los modificadores, del estado del teclado en el momento de esta tecla y como los da
    // Windows: Alt y Ctrl de cada lado (AltGr es Ctrl izquierdo + Alt derecho; winit no los
    // marca) y las luces de Bloq Num, Bloq Despl y Bloq Mayús.
    // SAFETY: GetKeyState solo consulta el estado del teclado de este hilo.
    let key = |vk: u16| unsafe { GetKeyState(i32::from(vk)) };
    let mut state = 0u32;
    for (vk, flag) in [
        (VK_RMENU, 0x01),    // RIGHT_ALT_PRESSED
        (VK_LMENU, 0x02),    // LEFT_ALT_PRESSED
        (VK_RCONTROL, 0x04), // RIGHT_CTRL_PRESSED
        (VK_LCONTROL, 0x08), // LEFT_CTRL_PRESSED
        (VK_SHIFT, 0x10),    // SHIFT_PRESSED
    ] {
        if key(vk) < 0 {
            state |= flag;
        }
    }
    for (vk, flag) in [(VK_NUMLOCK, 0x20), (VK_SCROLL, 0x40), (VK_CAPITAL, 0x80)] {
        if key(vk) & 1 != 0 {
            state |= flag;
        }
    }
    if scancode & 0xE000 != 0 {
        state |= 0x100; // ENHANCED_KEY (flechas, Insert, Supr, Inicio, Fin…)
    }
    Some(win32_sequence(vk, scancode & 0xFF, text, state))
}

/// Tecla virtual que daría Windows a un programa de consola. `mapped` es la del código de
/// exploración (`MapVirtualKeyW`), que se diferencia en dos casos:
/// - Shift, Ctrl y Alt de cada lado (`VK_LSHIFT`, `VK_RMENU`…): Windows da la genérica
///   (`VK_SHIFT`, `VK_MENU`…), la única que .NET reconoce como modificador. Con las otras,
///   PowerShell escribía "@" (un carácter nulo) al pulsar Shift o Alt.
/// - El teclado numérico con Bloq Num: los mismos códigos que Inicio, Fin, las flechas…, pero
///   con un número es `VK_NUMPAD0`–`9` (y la coma, `VK_DECIMAL`). Como Fin, el 1 no se escribía.
#[cfg(any(windows, test))]
pub fn win32_vk(mapped: u32, scancode: u32, text: &str) -> u32 {
    match mapped {
        0xA0 | 0xA1 => return 0x10, // VK_LSHIFT, VK_RSHIFT -> VK_SHIFT
        0xA2 | 0xA3 => return 0x11, // VK_LCONTROL, VK_RCONTROL -> VK_CONTROL
        0xA4 | 0xA5 => return 0x12, // VK_LMENU, VK_RMENU -> VK_MENU
        _ => {}
    }
    let numpad = matches!(scancode, 0x47..=0x49 | 0x4B..=0x4D | 0x4F..=0x53);
    if numpad {
        match text.chars().next() {
            Some(c @ '0'..='9') => return 0x60 + (c as u32 - '0' as u32), // VK_NUMPAD0–9
            Some(_) if scancode == 0x53 => return 0x6E,                   // VK_DECIMAL
            _ => {}
        }
    }
    mapped
}

/// Texto pegado en el prompt de PowerShell. PSReadLine no pide el pegado entre corchetes: cada
/// salto de línea llegaría como Intro y se ejecutaría línea a línea. Van como Shift+Intro (nueva
/// línea sin ejecutar, lo mismo que hace su Ctrl+V) y sin el salto final: queda escrito, como al
/// pegar en bash. Sin ESC: ConPTY leería una secuencia pegada como teclas (un Intro, por ejemplo).
#[cfg(windows)]
pub fn powershell_paste(text: &str) -> Vec<u8> {
    let shift_enter = win32_sequence(0x0D, 0x1C, "\r", 0x10);
    let text = text.replace("\r\n", "\n").replace(['\r'], "\n");
    let mut out = Vec::with_capacity(text.len() + 16);
    for (i, line) in text.trim_end_matches('\n').split('\n').enumerate() {
        if i > 0 {
            out.extend_from_slice(&shift_enter);
        }
        out.extend(line.bytes().filter(|&b| b != 0x1b));
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teclas_virtuales_de_windows() {
        // Shift, Ctrl y Alt de cada lado: la genérica.
        assert_eq!(win32_vk(0xA0, 0x2A, ""), 0x10);
        assert_eq!(win32_vk(0xA1, 0x36, ""), 0x10);
        assert_eq!(win32_vk(0xA2, 0x1D, ""), 0x11);
        assert_eq!(win32_vk(0xA3, 0xE01D, ""), 0x11);
        assert_eq!(win32_vk(0xA4, 0x38, ""), 0x12);
        assert_eq!(win32_vk(0xA5, 0xE038, ""), 0x12);
        // Teclado numérico: con Bloq Num, números; sin él, Fin, flechas…
        assert_eq!(win32_vk(0x23, 0x4F, "1"), 0x61);
        assert_eq!(win32_vk(0x2D, 0x52, "0"), 0x60);
        assert_eq!(win32_vk(0x21, 0x49, "9"), 0x69);
        assert_eq!(win32_vk(0x2E, 0x53, ","), 0x6E);
        assert_eq!(win32_vk(0x23, 0x4F, ""), 0x23);
        // Fin y las flechas de verdad (códigos extendidos) no cambian.
        assert_eq!(win32_vk(0x23, 0xE04F, ""), 0x23);
        assert_eq!(win32_vk(0x26, 0xE048, ""), 0x26);
        // Ni el - y el + del teclado numérico, ni las letras.
        assert_eq!(win32_vk(0x6D, 0x4A, "-"), 0x6D);
        assert_eq!(win32_vk(0x6B, 0x4E, "+"), 0x6B);
        assert_eq!(win32_vk(0x41, 0x1E, "a"), 0x41);
    }
}
