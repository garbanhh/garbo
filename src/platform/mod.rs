//! Lo que cambia entre sistemas: qué se ejecuta en primer plano en cada pestaña, la carpeta, los
//! argumentos y el entorno de un proceso, y abrir enlaces, carpetas y archivos. Linux lo lee del
//! PTY (`tcgetpgrp`) y de `/proc`; Windows, de la API de Win32 (la consola no tiene grupos de
//! procesos: ver `windows.rs`).

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

use std::path::Path;

/// ¿Separa `c` componentes de una ruta? En Windows valen `\` y `/`.
pub fn is_separator(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

/// Último componente de una ruta o de una orden (`/usr/bin/zsh` -> `zsh`). En Windows quita
/// también la extensión `.exe` (`C:\Program Files\PowerShell\7\pwsh.exe` -> `pwsh`).
pub fn base_name(path: &str) -> &str {
    let name = path.rsplit(is_separator).next().unwrap_or(path);
    if cfg!(windows) {
        if let Some(i) = name.len().checked_sub(4) {
            if name.is_char_boundary(i) && name[i..].eq_ignore_ascii_case(".exe") {
                return &name[..i];
            }
        }
    }
    name
}

/// Ruta abreviada con `~` para la carpeta personal (`~/Documentos`; en Windows `~\Documents`).
pub fn tilde(path: &Path) -> String {
    let full = path.to_string_lossy().into_owned();
    let Some(home) = dirs::home_dir().filter(|h| !h.as_os_str().is_empty()) else {
        return full;
    };
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        Err(_) => full,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nombre_base() {
        assert_eq!(base_name("/usr/bin/zsh"), "zsh");
        assert_eq!(base_name("fish"), "fish");
        assert_eq!(base_name(""), "");
        if cfg!(windows) {
            assert_eq!(base_name(r"C:\Program Files\PowerShell\7\pwsh.exe"), "pwsh");
            assert_eq!(base_name("node.EXE"), "node");
            assert_eq!(base_name("C:/msys64/usr/bin/bash.exe"), "bash");
        }
    }

    #[test]
    fn carpeta_personal_abreviada() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(tilde(&home), "~");
        let sub = home.join("Documentos").join("Proyectos");
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(tilde(&sub), format!("~{sep}Documentos{sep}Proyectos"));
        // Un prefijo que no es un componente entero no se abrevia.
        let other = format!("{}x", home.display());
        assert_eq!(tilde(Path::new(&other)), other);
    }
}
