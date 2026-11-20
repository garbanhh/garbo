//! Pegar imágenes (capturas de pantalla) desde el portapapeles.
//!
//! El portapapeles de la app (copypasta) solo maneja texto. Cuando al pegar no hay texto pero sí
//! una imagen (lo normal tras ImprPant en GNOME), la imagen se guarda como archivo y se pega su
//! ruta: los agentes de código (Claude Code y otros) la reconocen y la adjuntan como imagen.
//!
//! La imagen se lee con `wl-paste` (Wayland) o `xclip` (X11), siempre con un tiempo límite, en un
//! hilo aparte. Las capturas se guardan en `~/.cache/garbo/capturas` y se borran al día siguiente.
//!
//! En Windows se lee con la API del portapapeles: el formato "PNG" tal cual, o el mapa de bits
//! (`CF_DIBV5`/`CF_DIB`) convertido a PNG. Los archivos copiados en el Explorador (`CF_HDROP`) se
//! pegan como rutas.

use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::process::Command;
use std::time::{Duration, SystemTime};

/// Antigüedad a partir de la cual se borran las capturas guardadas.
const KEEP: Duration = Duration::from_secs(24 * 3600);
#[cfg(not(windows))]
type ReadImage = fn(&str) -> Option<Vec<u8>>;

#[cfg(not(windows))]
fn run(args: &[&str]) -> Option<Vec<u8>> {
    // `timeout`: si el dueño del portapapeles no responde, no nos quedamos colgados.
    let out = Command::new("timeout").arg("3").args(args).output().ok()?;
    out.status.success().then_some(out.stdout)
}

/// Tipos MIME disponibles en el portapapeles y la orden para leer uno de ellos.
#[cfg(not(windows))]
fn backend() -> Option<(Vec<String>, ReadImage)> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    if wayland {
        if let Some(t) = run(&["wl-paste", "--list-types"]) {
            let read: ReadImage = |mime| run(&["wl-paste", "--no-newline", "--type", mime]);
            return Some((lines(&t), read));
        }
    }
    let t = run(&["xclip", "-selection", "clipboard", "-t", "TARGETS", "-o"])?;
    let read: ReadImage = |mime| run(&["xclip", "-selection", "clipboard", "-t", mime, "-o"]);
    Some((lines(&t), read))
}

#[cfg(not(windows))]
fn lines(b: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(b)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Formato preferido entre los ofrecidos, con su extensión.
#[cfg(any(not(windows), test))]
pub fn pick_image(types: &[String]) -> Option<(&str, &'static str)> {
    for (mime, ext) in [
        ("image/png", "png"),
        ("image/jpeg", "jpg"),
        ("image/webp", "webp"),
        ("image/gif", "gif"),
    ] {
        if let Some(t) = types.iter().find(|t| t.eq_ignore_ascii_case(mime)) {
            return Some((t.as_str(), ext));
        }
    }
    None
}

/// Si el portapapeles tiene una imagen, la guarda y devuelve la ruta del archivo (en Windows,
/// también las rutas de los archivos copiados en el Explorador).
#[cfg(not(windows))]
pub fn save_image() -> Result<Vec<PathBuf>, String> {
    let (types, read) =
        backend().ok_or("no se pudo leer el portapapeles (falta wl-paste o xclip)")?;
    let (mime, ext) = pick_image(&types).ok_or("el portapapeles no tiene texto ni imagen")?;
    let data = read(mime)
        .filter(|d| !d.is_empty())
        .ok_or("no se pudo leer la imagen")?;
    Ok(vec![store(&data, ext)?])
}

/// Guarda una captura en la carpeta de capturas (borrando las de más de un día).
fn store(data: &[u8], ext: &str) -> Result<PathBuf, String> {
    let dir = dirs::cache_dir()
        .ok_or("sin carpeta de caché")?
        .join("garbo")
        .join("capturas");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    prune(&dir);
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = dir.join(format!("captura-{stamp}.{ext}"));
    std::fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(windows)]
pub fn save_image() -> Result<Vec<PathBuf>, String> {
    match windows::read()? {
        windows::Content::Files(files) => Ok(files),
        windows::Content::Png(data) => Ok(vec![store(&data, "png")?]),
    }
}

/// Portapapeles de Windows.
#[cfg(windows)]
mod windows {
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
        RegisterClipboardFormatW,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows_sys::Win32::UI::Shell::{DragQueryFileW, HDROP};

    const CF_DIB: u32 = 8;
    const CF_HDROP: u32 = 15;
    const CF_DIBV5: u32 = 17;

    pub enum Content {
        Files(Vec<PathBuf>),
        Png(Vec<u8>),
    }

    /// Portapapeles abierto, que se cierra al soltarse.
    struct Open;

    impl Open {
        fn new() -> Result<Self, String> {
            // Otro programa puede tenerlo abierto un instante: se reintenta.
            for _ in 0..10 {
                // SAFETY: sin ventana dueña; se cierra en `drop`.
                if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                    return Ok(Open);
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            Err("no se pudo abrir el portapapeles".into())
        }
    }

    impl Drop for Open {
        fn drop(&mut self) {
            // SAFETY: abierto en `new`.
            unsafe { CloseClipboard() };
        }
    }

    /// Copia de los datos de un formato del portapapeles (abierto).
    fn data(format: u32) -> Option<Vec<u8>> {
        // SAFETY: el portapapeles está abierto; el bloque se bloquea solo mientras se copia.
        unsafe {
            if IsClipboardFormatAvailable(format) == 0 {
                return None;
            }
            let h: HANDLE = GetClipboardData(format);
            if h.is_null() {
                return None;
            }
            let p = GlobalLock(h as HGLOBAL) as *const u8;
            if p.is_null() {
                return None;
            }
            let v = std::slice::from_raw_parts(p, GlobalSize(h as HGLOBAL)).to_vec();
            GlobalUnlock(h as HGLOBAL);
            Some(v)
        }
    }

    fn files() -> Option<Vec<PathBuf>> {
        // SAFETY: el portapapeles está abierto; el HDROP es suyo y solo se consulta.
        unsafe {
            if IsClipboardFormatAvailable(CF_HDROP) == 0 {
                return None;
            }
            let h = GetClipboardData(CF_HDROP) as HDROP;
            if h.is_null() {
                return None;
            }
            let n = DragQueryFileW(h, u32::MAX, std::ptr::null_mut(), 0);
            let files: Vec<PathBuf> = (0..n)
                .filter_map(|i| {
                    let len = DragQueryFileW(h, i, std::ptr::null_mut(), 0) as usize;
                    let mut buf = vec![0u16; len + 1];
                    let got = DragQueryFileW(h, i, buf.as_mut_ptr(), buf.len() as u32) as usize;
                    (got > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..got])))
                })
                .collect();
            (!files.is_empty()).then_some(files)
        }
    }

    pub fn read() -> Result<Content, String> {
        let _open = Open::new()?;
        if let Some(files) = files() {
            return Ok(Content::Files(files));
        }
        let png: Vec<u16> = "PNG\0".encode_utf16().collect();
        // SAFETY: cadena terminada en NUL.
        let png_format = unsafe { RegisterClipboardFormatW(png.as_ptr()) };
        if let Some(png) = (png_format != 0).then(|| data(png_format)).flatten() {
            if png.starts_with(b"\x89PNG") {
                return Ok(Content::Png(png));
            }
        }
        let dib = data(CF_DIBV5)
            .or_else(|| data(CF_DIB))
            .ok_or("el portapapeles no tiene texto ni imagen")?;
        super::png::from_dib(&dib)
            .map(Content::Png)
            .ok_or_else(|| "formato de imagen no admitido".into())
    }
}

/// Mapa de bits de Windows (DIB) a PNG.
#[cfg(windows)]
mod png {
    /// PNG RGB de 8 bits a partir de un DIB (`BITMAPINFOHEADER` o `BITMAPV5HEADER` seguido de los
    /// píxeles) de 24 o 32 bits. Las capturas no llevan transparencia: el canal alfa se descarta.
    pub fn from_dib(dib: &[u8]) -> Option<Vec<u8>> {
        let u32_at = |o: usize| Some(u32::from_le_bytes(dib.get(o..o + 4)?.try_into().ok()?));
        let header = u32_at(0)? as usize;
        let width = u32_at(4)? as i32;
        let height = u32_at(8)? as i32;
        let bpp = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?);
        let compression = u32_at(16)?;
        if width <= 0 || height == 0 || !(bpp == 24 || bpp == 32) {
            return None;
        }
        // BI_RGB = 0, BI_BITFIELDS = 3 (con las máscaras tras la cabecera si es la clásica).
        let masks = match compression {
            0 => 0,
            3 if header == 40 => 12,
            3 => 0,
            _ => return None,
        };
        let (w, h) = (width as usize, height.unsigned_abs() as usize);
        let stride = (w * bpp as usize / 8).div_ceil(4) * 4;
        let pixels = dib.get(header + masks..)?;
        if pixels.len() < stride * h {
            return None;
        }
        let step = bpp as usize / 8;
        // Una fila filtrada por línea: byte de filtro (0) y RGB.
        let mut raw = Vec::with_capacity(h * (1 + 3 * w));
        for y in 0..h {
            // Altura positiva: las filas van de abajo arriba.
            let row = if height > 0 { h - 1 - y } else { y };
            let line = &pixels[row * stride..row * stride + w * step];
            raw.push(0);
            for px in line.chunks_exact(step) {
                raw.extend_from_slice(&[px[2], px[1], px[0]]);
            }
        }
        Some(encode_rgb(w as u32, h as u32, &raw))
    }

    /// PNG con las filas ya filtradas (`raw`), comprimidas con deflate.
    pub fn encode_rgb(w: u32, h: u32, raw: &[u8]) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8 bits, RGB, deflate, filtros, sin entrelazar
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &zlib(raw));
        chunk(&mut out, b"IEND", &[]);
        out
    }

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    fn zlib(raw: &[u8]) -> Vec<u8> {
        miniz_oxide::deflate::compress_to_vec_zlib(raw, 6)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// DIB de 2×2 de 24 bits de abajo arriba (filas de 8 bytes con relleno) a PNG: la cabecera,
        /// el CRC de IHDR y los píxeles (rojo, verde / azul, blanco) tras descomprimir.
        #[test]
        fn dib_a_png() {
            let mut dib = Vec::new();
            for v in [40u32, 2, 2] {
                dib.extend_from_slice(&v.to_le_bytes());
            }
            dib.extend_from_slice(&1u16.to_le_bytes());
            dib.extend_from_slice(&24u16.to_le_bytes());
            dib.extend_from_slice(&[0u8; 24]); // compresión (BI_RGB) y el resto
                                               // Fila de abajo primero: azul, blanco; después la de arriba: rojo, verde (BGR).
            dib.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]);
            dib.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
            let png = from_dib(&dib).unwrap();
            assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
            assert_eq!(&png[12..16], b"IHDR");
            let crc = u32::from_be_bytes(png[29..33].try_into().unwrap());
            assert_eq!(crc, crc32(&png[12..29]));
            let idat_len = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
            let raw =
                miniz_oxide::inflate::decompress_to_vec_zlib(&png[41..41 + idat_len]).unwrap();
            assert_eq!(raw, [0, 255, 0, 0, 0, 255, 0, 0, 0, 0, 255, 255, 255, 255]);
            // El CRC de referencia de "IEND" sin datos.
            assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        }
    }
}

fn prune(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok());
        if old.is_some_and(|d| d > KEEP) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Ruta lista para pegar en un shell (entre comillas solo si hace falta; en Windows, como las
/// entiende PowerShell: comillas simples y la de dentro doblada).
pub fn shell_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    let plain = if cfg!(windows) { "/._-+~\\:" } else { "/._-+~" };
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || plain.contains(c))
    {
        s.into_owned()
    } else if cfg!(windows) {
        format!("'{}'", s.replace('\'', "''"))
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
        assert_eq!(
            pick_image(&t(&["text/plain", "image/jpeg", "image/png"])),
            Some(("image/png", "png"))
        );
        assert_eq!(pick_image(&t(&["image/jpeg"])), Some(("image/jpeg", "jpg")));
        assert_eq!(pick_image(&t(&["text/plain"])), None);
        assert_eq!(
            shell_path(Path::new("/home/a/.cache/garbo/capturas/captura-1.png")),
            "/home/a/.cache/garbo/capturas/captura-1.png"
        );
        assert_eq!(
            shell_path(Path::new("/home/a b/x.png")),
            "'/home/a b/x.png'"
        );
        if cfg!(windows) {
            assert_eq!(
                shell_path(Path::new(r"C:\Users\ana\captura-1.png")),
                r"C:\Users\ana\captura-1.png"
            );
            assert_eq!(
                shell_path(Path::new(r"C:\Users\Ana O'Neil\x.png")),
                r"'C:\Users\Ana O''Neil\x.png'"
            );
        }
    }
}
