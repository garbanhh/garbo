//! Linux: el proceso en primer plano se pregunta al PTY (`tcgetpgrp`) y el resto se lee de `/proc`.

use alacritty_terminal::tty;
use std::ffi::OsString;
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

/// Lo que hace falta para saber qué se ejecuta en primer plano en una pestaña: una copia del
/// descriptor maestro del PTY.
pub struct Probe {
    master: Option<fs::File>,
}

impl Probe {
    pub fn new(pty: &tty::Pty) -> Self {
        Self {
            master: pty.file().try_clone().ok(),
        }
    }

    /// Grupo de procesos en primer plano (su líder). Con el shell esperando, el propio shell.
    pub fn foreground(&self) -> Option<i32> {
        let fd = self.master.as_ref()?.as_raw_fd();
        // SAFETY: tcgetpgrp solo lee el grupo de primer plano del terminal asociado al descriptor.
        let p = unsafe { libc::tcgetpgrp(fd) };
        (p > 0).then_some(p)
    }
}

/// Proceso del shell de un PTY recién creado.
pub fn child_pid(pty: &tty::Pty) -> u32 {
    pty.child().id()
}

/// Nombre del proceso (`/proc/<pid>/comm`).
pub fn process_name(pid: i32) -> Option<String> {
    let comm = fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(comm.trim().to_string())
}

/// Argumentos con los que se lanzó el proceso, empezando por el programa.
pub fn process_args(pid: i32) -> Option<Vec<String>> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    Some(
        raw.split(|&b| b == 0)
            .filter(|a| !a.is_empty())
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect(),
    )
}

/// Carpeta de trabajo del proceso.
pub fn process_cwd(pid: i32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// Variable `key` del entorno del proceso (vacía cuenta como no puesta).
pub fn process_env(pid: i32, key: &str) -> Option<OsString> {
    let environ = fs::read(format!("/proc/{pid}/environ")).ok()?;
    let prefix = format!("{key}=");
    environ
        .split(|&b| b == 0)
        .find_map(|kv| kv.strip_prefix(prefix.as_bytes()))
        .filter(|v| !v.is_empty())
        .map(|v| OsString::from_vec(v.to_vec()))
}

/// Archivos que tiene abiertos el proceso (solo los del propio usuario se pueden leer).
pub fn open_files(pid: i32) -> Vec<PathBuf> {
    let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Vec::new();
    };
    fds.flatten()
        .filter_map(|fd| fs::read_link(fd.path()).ok())
        .collect()
}

/// Hijos directos del proceso.
pub fn children(pid: i32) -> Vec<i32> {
    fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// Devolver al sistema la memoria que no se usa: en Linux no hace falta (ver `windows.rs`).
pub fn trim_memory() {}

/// Termina un proceso hijo nuestro (p. ej. el `notify-send` de un aviso).
pub fn terminate(pid: u32) {
    // SAFETY: kill solo manda una señal; el pid es de un hijo nuestro aún sin recoger.
    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
}

/// ¿Se puede ejecutar el archivo? (algún bit de ejecución).
pub fn is_executable(meta: &fs::Metadata, _path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.is_file() && meta.permissions().mode() & 0o111 != 0
}

/// Busca un programa en el `PATH`.
pub fn find_program(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        return Path::new(name).is_file().then(|| PathBuf::from(name));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Abre una URL, una carpeta o un archivo con el programa predeterminado del escritorio.
pub fn open(target: &str) {
    match std::process::Command::new("xdg-open")
        .arg(target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        // Se recoge en segundo plano para que no quede como proceso zombi.
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(e) => log::warn!("No se pudo abrir {target}: {e}"),
    }
}

/// Editor para Ctrl+clic en un archivo cuando no hay `$VISUAL` ni `$EDITOR`: en Linux, ninguno
/// (se abre con `xdg-open`, que elige la aplicación del tipo de archivo).
pub fn default_editor() -> Option<String> {
    None
}

/// Tiempo entre dos clics para que cuenten como doble clic.
pub fn double_click_time() -> std::time::Duration {
    std::time::Duration::from_millis(400)
}
