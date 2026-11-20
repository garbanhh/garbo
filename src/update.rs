//! Actualizaciones: consulta la última release en GitHub y, si el usuario lo pide, descarga el
//! paquete de su sistema (RPM en Fedora, `.deb` en Debian y Ubuntu), verifica su SHA-256 y su firma
//! con la clave de Garbo incorporada al binario e instala con `pkexec dnf` o `pkexec apt-get`.
//! Usa `curl`: la app no lleva ninguna librería de red ni TLS.
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
/// Clave pública de Garbo (la misma firma los RPM y los `.deb`; el nombre es anterior a los `.deb`).
const PUBLIC_KEY: Option<&str> = option_env!("GARBO_RPM_PUBLIC_KEY");

/// Formato de paquete del sistema.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Rpm,
    Deb,
}

impl Format {
    /// Final del nombre del paquete en la release (`garbo-0.9.1-1.x86_64.rpm`,
    /// `garbo_0.9.1-1_amd64.deb`).
    fn suffix(self) -> String {
        match self {
            Format::Rpm => format!(".{}.rpm", std::env::consts::ARCH),
            Format::Deb => format!("_{}.deb", deb_arch()),
        }
    }

    /// Programas que hacen falta para descargar, verificar e instalar.
    fn tools(self) -> [&'static str; 5] {
        match self {
            Format::Rpm => ["curl", "dnf", "pkexec", "rpmkeys", "sha256sum"],
            Format::Deb => ["curl", "apt-get", "pkexec", "gpgv", "sha256sum"],
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Format::Rpm => "garbo.rpm",
            Format::Deb => "garbo.deb",
        }
    }
}

/// Arquitectura con el nombre de Debian.
fn deb_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

/// Un paquete publicado: el archivo, su suma y, en los `.deb`, su firma aparte (la de los RPM va
/// dentro del propio paquete).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub url: String,
    pub sha_url: String,
    pub sig_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub rpm: Option<Package>,
    pub deb: Option<Package>,
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

    /// El paquete de ese formato, si todo lo necesario para instalarlo llega por HTTPS.
    fn package(&self, format: Format) -> Option<&Package> {
        let pkg = match format {
            Format::Rpm => self.rpm.as_ref(),
            Format::Deb => self.deb.as_ref(),
        }?;
        let https = |url: &str| url.starts_with("https://");
        let signed = match format {
            Format::Rpm => true,
            Format::Deb => pkg.sig_url.as_deref().is_some_and(https),
        };
        (https(&pkg.url) && https(&pkg.sha_url) && signed).then_some(pkg)
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
    let assets = v["assets"].as_array().cloned().unwrap_or_default();
    let url_of = |want: &str| {
        assets.iter().find_map(|a| {
            (a["name"].as_str()? == want)
                .then(|| a["browser_download_url"].as_str().map(str::to_owned))
                .flatten()
        })
    };
    let package = |format: Format| {
        let suffix = format.suffix();
        let name = assets
            .iter()
            .filter_map(|a| a["name"].as_str())
            .find(|n| n.ends_with(&suffix))?;
        Some(Package {
            url: url_of(name)?,
            sha_url: url_of(&format!("{name}.sha256"))?,
            sig_url: url_of(&format!("{name}.sig")),
        })
    };
    Some(Release {
        version,
        rpm: package(Format::Rpm),
        deb: package(Format::Deb),
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

/// Se puede instalar automáticamente: hay clave, el sistema tiene sus herramientas y la release
/// trae su paquete.
pub fn can_install(rel: &Release) -> bool {
    trusted_key().is_some() && system_format().is_some_and(|f| rel.package(f).is_some())
}

/// Formato de paquete de este sistema: `.deb` si Garbo se instaló así; si no, RPM con dnf o, en
/// su defecto, `.deb` con apt. `None` si faltan las herramientas.
fn system_format() -> Option<Format> {
    static FORMAT: OnceLock<Option<Format>> = OnceLock::new();
    *FORMAT.get_or_init(|| {
        let has = |f: Format| f.tools().into_iter().all(which);
        let deb_installed = Path::new("/var/lib/dpkg/info/garbo.list").exists();
        if deb_installed && has(Format::Deb) {
            Some(Format::Deb)
        } else if has(Format::Rpm) {
            Some(Format::Rpm)
        } else {
            has(Format::Deb).then_some(Format::Deb)
        }
    })
}

fn trusted_key() -> Option<&'static str> {
    PUBLIC_KEY.filter(|key| !key.trim().is_empty())
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
    let format = system_format().ok_or("este sistema no tiene dnf ni apt con pkexec")?;
    let pkg = rel.package(format).ok_or(
        "la versión no trae un paquete para este sistema con su suma y su firma por HTTPS",
    )?;
    let key = trusted_key().ok_or("esta compilación no tiene una clave de firma de Garbo")?;
    let expected = curl_bytes(&["--max-time", "30", "--", &pkg.sha_url])
        .ok_or("no se pudo descargar la suma")?;
    let expected = parse_sha256(&expected).ok_or("la suma SHA-256 publicada no es válida")?;

    let dir = dirs::cache_dir()
        .ok_or("sin carpeta de caché")?
        .join("garbo")
        .join("updates");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dir = fs::canonicalize(dir).map_err(|e| e.to_string())?;
    remove_stale_downloads(&dir);
    let staged = crate::temp_dir::TempDir::new_in(&dir).map_err(|e| e.to_string())?;
    let path = staged.path().join(format.file_name());
    download(&pkg.url, &path, 600).ok_or("no se pudo descargar el paquete")?;
    if sha256(&path)? != expected {
        return Err("la descarga no coincide con su suma de verificación".into());
    }
    match (format, &pkg.sig_url) {
        (Format::Rpm, _) => verify_rpm_signature(staged.path(), &path, key)?,
        (Format::Deb, Some(sig_url)) => {
            let sig = staged.path().join("garbo.deb.sig");
            download(sig_url, &sig, 30).ok_or("no se pudo descargar la firma")?;
            verify_deb_signature(staged.path(), &path, &sig, key)?;
        }
        (Format::Deb, None) => return Err("el paquete no trae firma".into()),
    }

    send(UpdateState::Installing(rel.version.clone()));
    // apt-get instala un archivo local si la ruta empieza por "/" (y resuelve sus dependencias).
    // La firma ya la comprobó Garbo con su clave, que no está en la base de RPM del sistema: con
    // `localpkg_gpgcheck=1` en la configuración de dnf, la instalación fallaría (NOKEY).
    let installer: &[&str] = match format {
        Format::Rpm => &["dnf", "install", "-y", "--setopt=localpkg_gpgcheck=0"],
        Format::Deb => &["apt-get", "install", "-y"],
    };
    let status = Command::new("pkexec")
        .args(installer)
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

/// Descarga `url` en `path` (como mucho `secs` segundos).
fn download(url: &str, path: &Path, secs: u32) -> Option<()> {
    let status = curl()
        .args(["--max-time", &secs.to_string(), "-o"])
        .arg(path)
        .args(["--", url])
        .status()
        .ok()?;
    status.success().then_some(())
}

/// Firma aparte del `.deb` con `gpgv` y un llavero que solo tiene la clave de esta compilación.
fn verify_deb_signature(dir: &Path, deb: &Path, sig: &Path, key: &str) -> Result<(), String> {
    let keyring = dir.join("garbo.gpg");
    let key = dearmor(key).ok_or("la clave de firma de Garbo no es válida")?;
    fs::write(&keyring, key).map_err(|e| e.to_string())?;
    let verified = Command::new("gpgv")
        .arg("--keyring")
        .arg(&keyring)
        .arg(sig)
        .arg(deb)
        .output()
        .map_err(|e| e.to_string())?;
    if !verified.status.success() {
        return Err("la firma del paquete no es válida para la clave de Garbo".into());
    }
    Ok(())
}

/// Clave OpenPGP en armadura ASCII a binario (`gpgv` solo lee llaveros binarios). Se quitan la
/// cabecera, las líneas `Clave: valor`, las vacías y la suma `=XXXX` del final.
fn dearmor(armored: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let body: String = armored
        .lines()
        .map(str::trim)
        .skip_while(|l| !l.starts_with("-----BEGIN PGP PUBLIC KEY BLOCK-----"))
        .skip(1)
        .take_while(|l| !l.starts_with("-----END "))
        .filter(|l| !l.is_empty() && !l.contains(':') && !l.starts_with('='))
        .collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()?;
    (!bytes.is_empty()).then_some(bytes)
}

fn verify_rpm_signature(dir: &Path, rpm: &Path, key: &str) -> Result<(), String> {
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
        return Err("no se pudo calcular la suma SHA-256 del paquete".into());
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

/// Ruta del binario para relanzar tras actualizar (los paquetes instalan en /usr/bin/garbo).
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
        let deb = deb_arch();
        let json = format!(
            r#"{{"tag_name":"v0.3.0","html_url":"https://github.com/garbanhh/garbo/releases/tag/v0.3.0",
            "assets":[
              {{"name":"garbo-0.3.0-1.{arch}.rpm","browser_download_url":"https://x/garbo.rpm"}},
              {{"name":"garbo-0.3.0-1.{arch}.rpm.sha256","browser_download_url":"https://x/garbo.rpm.sha256"}},
              {{"name":"garbo_0.3.0-1_{deb}.deb","browser_download_url":"https://x/garbo.deb"}},
              {{"name":"garbo_0.3.0-1_{deb}.deb.sha256","browser_download_url":"https://x/garbo.deb.sha256"}},
              {{"name":"garbo_0.3.0-1_{deb}.deb.sig","browser_download_url":"https://x/garbo.deb.sig"}},
              {{"name":"otro.zip","browser_download_url":"https://x/otro.zip"}}
            ]}}"#
        );
        let r = parse_release(json.as_bytes()).unwrap();
        assert_eq!(r.version, "0.3.0");
        let rpm = r.package(Format::Rpm).unwrap();
        assert_eq!(rpm.url, "https://x/garbo.rpm");
        assert_eq!(rpm.sha_url, "https://x/garbo.rpm.sha256");
        let deb = r.package(Format::Deb).unwrap();
        assert_eq!(deb.url, "https://x/garbo.deb");
        assert_eq!(deb.sig_url.as_deref(), Some("https://x/garbo.deb.sig"));
        assert!(r.page.ends_with("v0.3.0"));
    }

    #[test]
    fn instala_solo_con_suma_y_firma_por_https() {
        let pkg = |sha: &str, sig: Option<&str>| Package {
            url: "https://example.org/garbo".into(),
            sha_url: sha.into(),
            sig_url: sig.map(str::to_owned),
        };
        let mut release = Release {
            version: "0.9.0".into(),
            rpm: Some(pkg("http://example.org/garbo.rpm.sha256", None)),
            deb: Some(pkg("https://example.org/garbo.deb.sha256", None)),
            page: "https://example.org/garbo".into(),
        };
        // Suma por HTTP: no. El .deb sin firma aparte: no (el RPM la lleva dentro).
        assert!(release.package(Format::Rpm).is_none());
        assert!(release.package(Format::Deb).is_none());
        release.rpm = Some(pkg("https://example.org/garbo.rpm.sha256", None));
        assert!(release.package(Format::Rpm).is_some());
        release.deb = Some(pkg(
            "https://example.org/garbo.deb.sha256",
            Some("http://example.org/garbo.deb.sig"),
        ));
        assert!(release.package(Format::Deb).is_none());
        release.deb = Some(pkg(
            "https://example.org/garbo.deb.sha256",
            Some("https://example.org/garbo.deb.sig"),
        ));
        assert!(release.package(Format::Deb).is_some());
    }

    /// Verificación real con `gpgv` de un `.deb` firmado aparte: `GARBO_TEST_DEB` (el paquete,
    /// con su `.sig` al lado) y `GARBO_TEST_KEY` (la clave pública en armadura ASCII). Un byte
    /// cambiado tiene que fallar. Se ejecuta a mano (ver docs/Procesos/Compilar y probar.md).
    #[test]
    #[ignore = "necesita gpgv, un paquete firmado y su clave"]
    fn verifica_la_firma_de_un_deb() {
        let deb = PathBuf::from(std::env::var("GARBO_TEST_DEB").unwrap());
        let key = std::env::var("GARBO_TEST_KEY").unwrap();
        let sig = deb.with_extension("deb.sig");
        let dir = crate::temp_dir::TempDir::new_in(&std::env::temp_dir()).unwrap();
        assert_eq!(verify_deb_signature(dir.path(), &deb, &sig, &key), Ok(()));
        let mut bytes = fs::read(&deb).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 1;
        let bad = dir.path().join("manipulado.deb");
        fs::write(&bad, bytes).unwrap();
        assert!(verify_deb_signature(dir.path(), &bad, &sig, &key).is_err());
    }

    #[test]
    fn quita_la_armadura_de_la_clave() {
        use base64::Engine;
        let bytes: Vec<u8> = (0u8..=200).collect();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let lines: Vec<&str> = b64
            .as_bytes()
            .chunks(64)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect();
        let armored = format!(
            "-----BEGIN PGP PUBLIC KEY BLOCK-----\nComment: Garbo\n\n{}\n=AbCd\n-----END PGP PUBLIC KEY BLOCK-----\n",
            lines.join("\n")
        );
        assert_eq!(dearmor(&armored), Some(bytes));
        assert_eq!(dearmor("no es una clave"), None);
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
