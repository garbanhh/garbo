//! Pegar imágenes (capturas de pantalla) desde el portapapeles.
//!
//! El portapapeles de la app (copypasta) solo maneja texto. Cuando al pegar no hay texto pero sí
//! una imagen (lo normal tras ImprPant en GNOME), la imagen se guarda como archivo y se pega su
//! ruta: los agentes de código (Claude Code y otros) la reconocen y la adjuntan como imagen.
//!
//! La imagen se lee con `wl-paste` (Wayland) o `xclip` (X11), siempre con un tiempo límite, en un
//! hilo aparte. Las capturas se guardan en `~/.cache/garbo/capturas` y se borran al día siguiente.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

/// Antigüedad a partir de la cual se borran las capturas guardadas.
const KEEP: Duration = Duration::from_secs(24 * 3600);

fn run(args: &[&str]) -> Option<Vec<u8>> {
    // `timeout`: si el dueño del portapapeles no responde, no nos quedamos colgados.
    let out = Command::new("timeout").arg("3").args(args).output().ok()?;
    out.status.success().then_some(out.stdout)
}

/// Tipos MIME disponibles en el portapapeles y la orden para leer uno de ellos.
fn backend() -> Option<(Vec<String>, fn(&str) -> Option<Vec<u8>>)> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    if wayland {
        if let Some(t) = run(&["wl-paste", "--list-types"]) {
            let read: fn(&str) -> Option<Vec<u8>> = |mime| run(&["wl-paste", "--no-newline", "--type", mime]);
            return Some((lines(&t), read));
        }
    }
    let t = run(&["xclip", "-selection", "clipboard", "-t", "TARGETS", "-o"])?;
    let read: fn(&str) -> Option<Vec<u8>> = |mime| run(&["xclip", "-selection", "clipboard", "-t", mime, "-o"]);
    Some((lines(&t), read))
}

fn lines(b: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(b).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

/// Formato preferido entre los ofrecidos, con su extensión.
pub fn pick_image(types: &[String]) -> Option<(&str, &'static str)> {
    for (mime, ext) in [("image/png", "png"), ("image/jpeg", "jpg"), ("image/webp", "webp"), ("image/gif", "gif")] {
        if let Some(t) = types.iter().find(|t| t.eq_ignore_ascii_case(mime)) {
            return Some((t.as_str(), ext));
        }
    }
    None
}

/// Si el portapapeles tiene una imagen, la guarda y devuelve la ruta del archivo.
pub fn save_image() -> Result<PathBuf, String> {
    let (types, read) = backend().ok_or("no se pudo leer el portapapeles (falta wl-paste o xclip)")?;
    let (mime, ext) = pick_image(&types).ok_or("el portapapeles no tiene texto ni imagen")?;
    let data = read(mime).filter(|d| !d.is_empty()).ok_or("no se pudo leer la imagen")?;
    let dir = dirs::cache_dir().ok_or("sin carpeta de caché")?.join("garbo").join("capturas");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    prune(&dir);
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_millis();
    let path = dir.join(format!("captura-{stamp}.{ext}"));
    std::fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(path)
}

fn prune(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| now.duration_since(t).ok());
        if old.is_some_and(|d| d > KEEP) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Ruta lista para pegar en un shell (entre comillas solo si hace falta).
pub fn shell_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+~".contains(c)) {
        s.into_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatos_y_rutas() {
        let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(pick_image(&t(&["text/plain", "image/jpeg", "image/png"])), Some(("image/png", "png")));
        assert_eq!(pick_image(&t(&["image/jpeg"])), Some(("image/jpeg", "jpg")));
        assert_eq!(pick_image(&t(&["text/plain"])), None);
        assert_eq!(shell_path(Path::new("/home/a/.cache/garbo/capturas/captura-1.png")), "/home/a/.cache/garbo/capturas/captura-1.png");
        assert_eq!(shell_path(Path::new("/home/a b/x.png")), "'/home/a b/x.png'");
    }
}

