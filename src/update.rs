//! Actualizaciones: consulta la última release en GitHub y, si el usuario lo pide, descarga
//! el RPM, comprueba su SHA-256 e instala con `pkexec dnf` (GNOME pide la contraseña).
//! Usa `curl` (ya está en Fedora): la app no lleva ninguna librería de red ni TLS.
//! Coste: una consulta al arrancar y otra cada 24 h; el resto del tiempo el hilo duerme.

use crate::terminal::UserEvent;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use winit::event_loop::EventLoopProxy;

pub const REPO: &str = "garbanhh/garbo";
const USER_AGENT: &str = "garbo-updater";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub rpm_url: Option<String>,
    pub sha_url: Option<String>,
    pub page: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateState {
    Available(Release),
    Downloading(String),
    Installing(String),
    Installed(String),
    Failed(String),
}

static CHECKER_STARTED: AtomicBool = AtomicBool::new(false);
/// Se puede desactivar desde Ajustes sin parar el hilo (simplemente deja de consultar).
pub static ENABLED: AtomicBool = AtomicBool::new(true);

/// Lanza (una sola vez) el hilo que busca actualizaciones.
pub fn start_checker(proxy: EventLoopProxy<UserEvent>) {
    if CHECKER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new().name("actualizaciones".into()).spawn(move || loop {
        if let Some(rel) = ENABLED.load(Ordering::Relaxed).then(check).flatten() {
            if proxy.send_event(UserEvent::Update(UpdateState::Available(rel))).is_err() {
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(24 * 3600));
    });
}

fn curl(args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("curl").args(["-fsSL", "-A", USER_AGENT]).args(args).output().ok()?;
    out.status.success().then_some(out.stdout)
}

/// Última release si es más nueva que la versión en ejecución.
pub fn check() -> Option<Release> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = curl(&["--max-time", "15", "-H", "Accept: application/vnd.github+json", &url])?;
    let rel = parse_release(&body)?;
    is_newer(&rel.version, env!("CARGO_PKG_VERSION")).then_some(rel)
}

pub fn parse_release(body: &[u8]) -> Option<Release> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let tag = v["tag_name"].as_str()?;
    let version = tag.trim_start_matches('v').to_string();
    let page = v["html_url"].as_str().unwrap_or_default().to_string();
    let suffix = format!(".{}.rpm", std::env::consts::ARCH);
    let assets = v["assets"].as_array().cloned().unwrap_or_default();
    let find = |pred: &dyn Fn(&str) -> bool| {
        assets.iter().find_map(|a| {
            let name = a["name"].as_str()?;
            pred(name).then(|| a["browser_download_url"].as_str().map(str::to_owned)).flatten()
        })
    };
    let rpm_name = assets
        .iter()
        .filter_map(|a| a["name"].as_str())
        .find(|n| n.ends_with(&suffix))
        .map(str::to_owned);
    let rpm_url = rpm_name.as_ref().and_then(|n| find(&|name| name == n));
    let sha_url = rpm_name.as_ref().and_then(|n| find(&|name| name == format!("{n}.sha256")));
    Some(Release { version, rpm_url, sha_url, page })
}

fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let mut it = s.trim_start_matches('v').split(['.', '-', '+']).map(|p| p.parse::<u64>());
    Some((it.next()?.ok()?, it.next()?.ok()?, it.next().and_then(Result::ok).unwrap_or(0)))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Se puede instalar automáticamente (Fedora/RPM con dnf y pkexec).
pub fn can_install(rel: &Release) -> bool {
    rel.rpm_url.is_some() && which("dnf") && which("pkexec")
}

fn which(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
}

/// Descarga, verifica e instala en segundo plano, informando de cada paso.
pub fn start_install(rel: Release, proxy: EventLoopProxy<UserEvent>) {
    let _ = std::thread::Builder::new().name("instalar-actualización".into()).spawn(move || {
        let v = rel.version.clone();
        let send = |s: UpdateState| {
            let _ = proxy.send_event(UserEvent::Update(s));
        };
        send(UpdateState::Downloading(v.clone()));
        match install(&rel, &send) {
            Ok(()) => send(UpdateState::Installed(v)),
            Err(e) => {
                log::warn!("Actualización fallida: {e}");
                send(UpdateState::Failed(e));
            }
        }
    });
}

fn install(rel: &Release, send: &dyn Fn(UpdateState)) -> Result<(), String> {
    let url = rel.rpm_url.as_deref().ok_or("la release no trae RPM")?;
    let dir = dirs::cache_dir().ok_or("sin carpeta de caché")?.join("garbo").join("updates");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = url.rsplit('/').next().unwrap_or("garbo.rpm");
    let path = dir.join(name);
    let p = path.to_string_lossy().to_string();
    curl(&["--max-time", "600", "-o", &p, url]).ok_or("no se pudo descargar")?;

    if let Some(sha_url) = &rel.sha_url {
        let expected = curl(&["--max-time", "30", sha_url]).ok_or("no se pudo descargar la suma")?;
        let expected = String::from_utf8_lossy(&expected).split_whitespace().next().unwrap_or("").to_lowercase();
        if expected.is_empty() || sha256(&path)? != expected {
            let _ = std::fs::remove_file(&path);
            return Err("la descarga no coincide con su suma de verificación".into());
        }
    }

    send(UpdateState::Installing(rel.version.clone()));
    let status = Command::new("pkexec")
        .args(["dnf", "install", "-y", &p])
        .status()
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&path);
    if status.success() {
        Ok(())
    } else {
        Err("instalación cancelada o fallida".into())
    }
}

fn sha256(path: &Path) -> Result<String, String> {
    let out = Command::new("sha256sum").arg(path).output().map_err(|e| e.to_string())?;
    let s = String::from_utf8_lossy(&out.stdout);
    Ok(s.split_whitespace().next().unwrap_or("").to_lowercase())
}

/// Ruta del binario para relanzar tras actualizar (el RPM instala en /usr/bin/garbo).
pub fn restart_path() -> PathBuf {
    let installed = PathBuf::from("/usr/bin/garbo");
    if installed.is_file() {
        return installed;
    }
    std::env::current_exe()
        .map(|p| PathBuf::from(p.to_string_lossy().trim_end_matches(" (deleted)")))
        .unwrap_or(installed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compara_versiones() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("basura", "0.1.0"));
    }

    #[test]
    fn lee_release_de_github() {
        let arch = std::env::consts::ARCH;
        let json = format!(
            r#"{{"tag_name":"v0.3.0","html_url":"https://github.com/garbanhh/garbo/releases/tag/v0.3.0",
            "assets":[
              {{"name":"garbo-0.3.0-1.{arch}.rpm","browser_download_url":"https://x/garbo.rpm"}},
              {{"name":"garbo-0.3.0-1.{arch}.rpm.sha256","browser_download_url":"https://x/garbo.rpm.sha256"}},
              {{"name":"otro.zip","browser_download_url":"https://x/otro.zip"}}
            ]}}"#
        );
        let r = parse_release(json.as_bytes()).unwrap();
        assert_eq!(r.version, "0.3.0");
        assert_eq!(r.rpm_url.as_deref(), Some("https://x/garbo.rpm"));
        assert_eq!(r.sha_url.as_deref(), Some("https://x/garbo.rpm.sha256"));
        assert!(r.page.ends_with("v0.3.0"));
    }
}
