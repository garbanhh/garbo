//! Actualizaciones: consulta la última release en GitHub y, si el usuario lo pide, descarga
//! el RPM, verifica su SHA-256 y su firma e instala con `pkexec dnf`.
//! Usa `curl` (ya está en Fedora): la app no lleva ninguna librería de red ni TLS.
//! Coste: una consulta al arrancar y otra cada 24 h; el resto del tiempo el hilo duerme.

use crate::terminal::UserEvent;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;
use std::{fs, io};
use winit::event_loop::EventLoopProxy;

pub const REPO: &str = "garbanhh/garbo";
const USER_AGENT: &str = "garbo-updater";
const RPM_PUBLIC_KEY: Option<&str> = option_env!("GARBO_RPM_PUBLIC_KEY");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub rpm_url: Option<String>,
    pub sha_url: Option<String>,
    pub page: String,
}

impl Release {
    pub fn button_label(&self) -> String {
        let action = if can_install(self) {
            "Actualizar a"
        } else {
            "Ver versión"
        };
        format!("{action} {}", self.version)
    }

    fn install_urls(&self) -> Option<(&str, &str)> {
        let https = |url: &&str| url.starts_with("https://");
        Some((
            self.rpm_url.as_deref().filter(https)?,
            self.sha_url.as_deref().filter(https)?,
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateState {
    Available(Release),
    Downloading(String),
    Installing(String),
    Installed(String),
    Failed(String),
}

/// Estado de la búsqueda manual ("Buscar actualizaciones" en Ajustes).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CheckState {
    #[default]
    Idle,
    Checking,
    UpToDate,
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
    let _ = std::thread::Builder::new()
        .name("actualizaciones".into())
        .spawn(move || loop {
            if let Some(rel) = ENABLED.load(Ordering::Relaxed).then(check).flatten() {
                if proxy
                    .send_event(UserEvent::Update(UpdateState::Available(rel)))
                    .is_err()
                {
                    return;
                }
            }
            std::thread::sleep(Duration::from_secs(24 * 3600));
        });
}

/// Búsqueda inmediata pedida por el usuario (aunque la automática esté desactivada).
pub fn check_now(proxy: EventLoopProxy<UserEvent>) {
    let _ = std::thread::Builder::new()
        .name("buscar-actualización".into())
        .spawn(move || {
            let state = match check_result() {
                Ok(Some(rel)) => {
                    let _ = proxy.send_event(UserEvent::Update(UpdateState::Available(rel)));
                    CheckState::Idle
                }
                Ok(None) => CheckState::UpToDate,
                Err(e) => CheckState::Failed(e),
            };
            let _ = proxy.send_event(UserEvent::UpdateCheck(state));
        });
}

fn curl() -> Command {
    let mut cmd = Command::new("curl");
    cmd.args([
        "-q",
        "-fsSL",
        "--globoff",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "-A",
        USER_AGENT,
    ]);
    cmd
}

fn curl_bytes(args: &[&str]) -> Option<Vec<u8>> {
    let out = curl().args(args).output().ok()?;
    out.status.success().then_some(out.stdout)
}

/// Última release si es más nueva que la versión en ejecución.
pub fn check() -> Option<Release> {
    check_result().ok().flatten()
}

/// Como `check`, distinguiendo "al día" (`Ok(None)`) de "no se pudo consultar" (`Err`).
pub fn check_result() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = curl_bytes(&[
        "--max-time",
        "15",
        "-H",
        "Accept: application/vnd.github+json",
        "--",
        &url,
    ])
    .ok_or("sin conexión con GitHub")?;
    let rel = parse_release(&body).ok_or("respuesta no válida")?;
    Ok(is_newer(&rel.version, env!("CARGO_PKG_VERSION")).then_some(rel))
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
            pred(name)
                .then(|| a["browser_download_url"].as_str().map(str::to_owned))
                .flatten()
        })
    };
    let rpm_name = assets
        .iter()
        .filter_map(|a| a["name"].as_str())
        .find(|n| n.ends_with(&suffix))
        .map(str::to_owned);
    let rpm_url = rpm_name.as_ref().and_then(|n| find(&|name| name == n));
    let sha_url = rpm_name
        .as_ref()
        .and_then(|n| find(&|name| name == format!("{n}.sha256")));
    Some(Release {
        version,
        rpm_url,
        sha_url,
        page,
    })
}

fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let mut it = s
        .trim_start_matches('v')
        .split(['.', '-', '+'])
        .map(|p| p.parse::<u64>());
    Some((
        it.next()?.ok()?,
        it.next()?.ok()?,
        it.next().and_then(Result::ok).unwrap_or(0),
    ))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Se puede instalar automáticamente (Fedora/RPM con dnf y pkexec).
pub fn can_install(rel: &Release) -> bool {
    static TOOLS_PRESENT: OnceLock<bool> = OnceLock::new();
    trusted_key().is_some()
        && rel.install_urls().is_some()
        && *TOOLS_PRESENT.get_or_init(|| {
            ["curl", "dnf", "pkexec", "rpmkeys", "sha256sum"]
                .into_iter()
                .all(which)
        })
}

fn trusted_key() -> Option<&'static str> {
    RPM_PUBLIC_KEY.filter(|key| !key.trim().is_empty())
}

fn which(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
}

/// Descarga, verifica e instala en segundo plano, informando de cada paso.
pub fn start_install(rel: Release, proxy: EventLoopProxy<UserEvent>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("instalar-actualización".into())
        .spawn(move || {
            let v = rel.version.clone();
            let send = |s: UpdateState| {
                let _ = proxy.send_event(UserEvent::Update(s));
            };
            match install(&rel, &send) {
                Ok(()) => send(UpdateState::Installed(v)),
                Err(e) => {
                    log::warn!("Actualización fallida: {e}");
                    send(UpdateState::Failed(e));
                }
            }
        })?;
    Ok(())
}

fn install(rel: &Release, send: &dyn Fn(UpdateState)) -> Result<(), String> {
    let (url, sha_url) = rel
        .install_urls()
        .ok_or("la versión necesita un RPM y su suma SHA-256 por HTTPS")?;
    let key = trusted_key().ok_or("esta compilación no tiene una clave de firma de Garbo")?;
    let expected =
        curl_bytes(&["--max-time", "30", "--", sha_url]).ok_or("no se pudo descargar la suma")?;
    let expected = parse_sha256(&expected).ok_or("la suma SHA-256 publicada no es válida")?;

    let dir = dirs::cache_dir()
        .ok_or("sin carpeta de caché")?
        .join("garbo")
        .join("updates");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dir = fs::canonicalize(dir).map_err(|e| e.to_string())?;
    remove_stale_downloads(&dir);
    let staged = crate::temp_dir::TempDir::new_in(&dir).map_err(|e| e.to_string())?;
    let path = staged.path().join("garbo.rpm");
    let download = curl()
        .args(["--max-time", "600", "-o"])
        .arg(&path)
        .args(["--", url])
        .status()
        .map_err(|e| e.to_string())?;
    if !download.success() {
        return Err("no se pudo descargar el RPM".into());
    }
    if sha256(&path)? != expected {
        return Err("la descarga no coincide con su suma de verificación".into());
    }
    verify_signature(staged.path(), &path, key)?;

    send(UpdateState::Installing(rel.version.clone()));
    let status = Command::new("pkexec")
        .args(["dnf", "install", "-y"])
        .arg(&path)
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("instalación cancelada o fallida".into())
    }
}

/// Descargas a medias de intentos anteriores (Garbo se cerró antes de terminar): fuera las de
/// hace más de una hora, para no pisar otra instalación en curso.
fn remove_stale_downloads(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry.file_name().to_string_lossy().starts_with(".garbo-")
            && entry.file_type().is_ok_and(|t| t.is_dir())
            && entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(3600));
        if stale {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

fn verify_signature(dir: &Path, rpm: &Path, key: &str) -> Result<(), String> {
    let key_path = dir.join("garbo.asc");
    fs::write(&key_path, key).map_err(|e| e.to_string())?;
    let db = dir.join("rpmdb");
    fs::create_dir(&db).map_err(|e| e.to_string())?;
    // Solo la clave incorporada a esta compilación entra en el llavero temporal.
    let rpmkeys = || {
        let mut cmd = Command::new("rpmkeys");
        cmd.args(["--noplugins", "--define", "_keyring rpmdb", "--dbpath"])
            .arg(&db);
        cmd
    };
    let imported = rpmkeys()
        .arg("--import")
        .arg(key_path)
        .output()
        .map_err(|e| e.to_string())?;
    if !imported.status.success() {
        return Err("no se pudo cargar la clave de firma de Garbo".into());
    }
    let verified = rpmkeys()
        .args(["--define", "_pkgverify_level all", "--checksig"])
        .arg(rpm)
        .output()
        .map_err(|e| e.to_string())?;
    if !verified.status.success() {
        return Err("la firma del RPM no es válida para la clave de Garbo".into());
    }
    Ok(())
}

fn sha256(path: &Path) -> Result<String, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let out = Command::new("sha256sum")
        .stdin(file)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("no se pudo calcular la suma SHA-256 del RPM".into());
    }
    parse_sha256(&out.stdout).ok_or_else(|| "sha256sum devolvió una suma no válida".into())
}

fn parse_sha256(bytes: &[u8]) -> Option<String> {
    let sum = std::str::from_utf8(bytes)
        .ok()?
        .split_ascii_whitespace()
        .next()?;
    (sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| sum.to_ascii_lowercase())
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
        assert!(r.install_urls().is_some());
    }

    #[test]
    fn instala_solo_con_rpm_y_suma_por_https() {
        let mut release = Release {
            version: "0.9.0".into(),
            rpm_url: Some("https://example.org/garbo.rpm".into()),
            sha_url: None,
            page: "https://example.org/garbo".into(),
        };
        assert!(release.install_urls().is_none());
        assert!(!can_install(&release));
        release.sha_url = Some("http://example.org/garbo.rpm.sha256".into());
        assert!(release.install_urls().is_none());
        release.sha_url = Some("https://example.org/garbo.rpm.sha256".into());
        assert!(release.install_urls().is_some());
    }

    #[test]
    fn valida_el_formato_de_la_suma() {
        let sum = "AB".repeat(32);
        let line = format!("{sum}  garbo.rpm\n");
        assert_eq!(parse_sha256(line.as_bytes()), Some(sum.to_lowercase()));
        assert_eq!(parse_sha256(&sum.as_bytes()[..63]), None);
        assert_eq!(parse_sha256("g".repeat(64).as_bytes()), None);
        assert_eq!(parse_sha256(b""), None);
    }
}
