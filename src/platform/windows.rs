//! Windows: no hay `/proc` ni grupos de procesos en la consola. El árbol de procesos sale de una
//! instantánea de ToolHelp (compartida por todas las pestañas unos instantes), la línea de órdenes
//! de `NtQueryInformationProcess` y la carpeta y el entorno, de los parámetros del proceso en su
//! memoria (PEB), que es lo mismo que leen el Explorador de procesos y similares.

use alacritty_terminal::tty;
use std::ffi::{c_void, OsStr, OsString};
use std::mem::{size_of, zeroed, MaybeUninit};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, FILETIME, HANDLE, INVALID_HANDLE_VALUE, UNICODE_STRING,
};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, IsWow64Process, OpenProcess, PROCESS_BASIC_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};
use windows_sys::Win32::UI::Shell::{CommandLineToArgvW, ShellExecuteW};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Cuánto vale una instantánea de procesos. Hacerla cuesta del orden de un milisegundo y varias
/// pestañas preguntan seguido (como mucho cada 700 ms cada una, y al teclear): así se hace una sola.
const TABLE_TTL: Duration = Duration::from_millis(250);
/// `PROCESSINFOCLASS` de la línea de órdenes (Windows 8.1 o posterior).
const PROCESS_COMMAND_LINE_INFORMATION: i32 = 60;

struct Proc {
    pid: u32,
    ppid: u32,
    /// Nombre del ejecutable ("node.exe").
    exe: String,
}

static TABLE: Mutex<Option<(Instant, Vec<Proc>)>> = Mutex::new(None);

fn with_table<R>(f: impl FnOnce(&[Proc]) -> R) -> R {
    let mut table = TABLE.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    if table
        .as_ref()
        .is_none_or(|(at, _)| now.duration_since(*at) >= TABLE_TTL)
    {
        *table = Some((now, snapshot()));
    }
    f(&table.as_ref().expect("instantánea recién hecha").1)
}

fn snapshot() -> Vec<Proc> {
    let mut out = Vec::with_capacity(256);
    // SAFETY: la instantánea se recorre con una entrada inicializada con su tamaño y se cierra.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = zeroed();
        e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            out.push(Proc {
                pid: e.th32ProcessID,
                ppid: e.th32ParentProcessID,
                exe: from_wide_nul(&e.szExeFile),
            });
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    out
}

fn from_wide_nul(s: &[u16]) -> String {
    let len = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..len])
}

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// Proceso abierto, que se cierra al soltarse.
struct Handle(HANDLE);

impl Handle {
    fn open(pid: u32, access: u32) -> Option<Self> {
        if pid == 0 {
            return None;
        }
        // SAFETY: OpenProcess no tiene requisitos; un nulo indica que no se pudo abrir.
        let h = unsafe { OpenProcess(access, 0, pid) };
        (!h.is_null()).then_some(Self(h))
    }

    /// Momento de creación (intervalos de 100 ns), para descartar padres con el pid reutilizado.
    fn created(&self) -> Option<u64> {
        let mut t: [FILETIME; 4] = unsafe { zeroed() };
        let [c, e, k, u] = &mut t;
        // SAFETY: cuatro FILETIME válidos y un proceso abierto con permiso de consulta.
        let ok = unsafe { GetProcessTimes(self.0, c, e, k, u) } != 0;
        ok.then(|| (u64::from(t[0].dwHighDateTime) << 32) | u64::from(t[0].dwLowDateTime))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: el handle es nuestro y se cierra una vez.
        unsafe { CloseHandle(self.0) };
    }
}

/// Anfitriones de la consola, que cuelgan de quien la crea y no son lo que se ejecuta.
fn is_console_host(exe: &str) -> bool {
    exe.eq_ignore_ascii_case("conhost.exe") || exe.eq_ignore_ascii_case("OpenConsole.exe")
}

/// Hijo más reciente de `parent`. Un proceso cuyo padre murió y dejó su pid a otro seguiría
/// apuntando a él: solo cuentan los nacidos después que el padre.
fn newest_child(procs: &[Proc], parent: u32) -> Option<u32> {
    let born = Handle::open(parent, PROCESS_QUERY_LIMITED_INFORMATION)?.created()?;
    procs
        .iter()
        .filter(|p| p.ppid == parent && p.pid != parent && !is_console_host(&p.exe))
        .filter_map(|p| {
            let t = Handle::open(p.pid, PROCESS_QUERY_LIMITED_INFORMATION)?.created()?;
            (t >= born).then_some((t, p.pid))
        })
        .max()
        .map(|(_, pid)| pid)
}

/// `cmd /c …`: lo lanza PowerShell para los `.cmd` (como los que instala npm: `claude.cmd`). Lo
/// que importa es lo que ejecuta.
fn is_wrapper(procs: &[Proc], pid: u32) -> bool {
    procs
        .iter()
        .find(|p| p.pid == pid)
        .is_some_and(|p| p.exe.eq_ignore_ascii_case("cmd.exe"))
        && process_args(pid as i32).is_some_and(|args| {
            args.iter()
                .skip(1)
                .any(|a| a.eq_ignore_ascii_case("/c") || a.eq_ignore_ascii_case("/k"))
        })
}

/// Lo que hace falta para saber qué se ejecuta en primer plano en una pestaña: su shell.
pub struct Probe {
    shell: u32,
}

impl Probe {
    pub fn new(pty: &tty::Pty) -> Self {
        Self {
            shell: child_pid(pty),
        }
    }

    /// Lo que se ejecuta en primer plano. La consola de Windows no tiene grupos de procesos: se
    /// toma el hijo más reciente del shell (lo último que lanzó) y, si es un envoltorio
    /// (`cmd /c`), lo que este lanzó. Sin hijos, el propio shell, como `tcgetpgrp` en Linux con el
    /// shell esperando.
    pub fn foreground(&self) -> Option<i32> {
        if self.shell == 0 {
            return None;
        }
        with_table(|procs| {
            let mut pid = self.shell;
            if let Some(child) = newest_child(procs, pid) {
                pid = child;
                for _ in 0..3 {
                    if !is_wrapper(procs, pid) {
                        break;
                    }
                    match newest_child(procs, pid) {
                        Some(c) => pid = c,
                        None => break,
                    }
                }
            }
            Some(pid as i32)
        })
    }
}

/// Proceso del shell de un PTY recién creado.
pub fn child_pid(pty: &tty::Pty) -> u32 {
    pty.child_watcher().pid().map_or(0, |p| p.get())
}

/// Nombre del ejecutable del proceso, sin `.exe` ("node").
pub fn process_name(pid: i32) -> Option<String> {
    let exe = with_table(|procs| {
        procs
            .iter()
            .find(|p| p.pid == pid as u32)
            .map(|p| p.exe.clone())
    })?;
    Some(super::base_name(&exe).to_string())
}

/// Línea de órdenes del proceso tal cual (UTF-16).
fn command_line(pid: u32) -> Option<Vec<u16>> {
    let h = Handle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let mut len = 0u32;
    // Primero el tamaño: la llamada falla con STATUS_INFO_LENGTH_MISMATCH y lo devuelve.
    // SAFETY: con búfer nulo y longitud 0 solo escribe `len`.
    unsafe {
        NtQueryInformationProcess(
            h.0,
            PROCESS_COMMAND_LINE_INFORMATION,
            null_mut(),
            0,
            &mut len,
        )
    };
    if len < size_of::<UNICODE_STRING>() as u32 || len > 1 << 20 {
        return None;
    }
    // Alineado a 8 bytes: empieza por un UNICODE_STRING con un puntero a su propio texto.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: el búfer tiene al menos `len` bytes.
    let status = unsafe {
        NtQueryInformationProcess(
            h.0,
            PROCESS_COMMAND_LINE_INFORMATION,
            buf.as_mut_ptr().cast(),
            len,
            &mut len,
        )
    };
    if status < 0 {
        return None;
    }
    // SAFETY: tras una llamada correcta el búfer empieza por un UNICODE_STRING cuyo texto está
    // dentro del mismo búfer.
    let text = unsafe {
        let us = &*(buf.as_ptr() as *const UNICODE_STRING);
        if us.Buffer.is_null() {
            return None;
        }
        std::slice::from_raw_parts(us.Buffer, usize::from(us.Length) / 2).to_vec()
    };
    Some(text)
}

/// Parte una línea de órdenes como lo hacen los programas de C (`CommandLineToArgvW`).
fn split_command_line(line: &[u16]) -> Vec<String> {
    // Con la línea vacía, CommandLineToArgvW devolvería la ruta de Garbo.
    if line.iter().all(|&c| c == b' ' as u16 || c == b'\t' as u16) {
        return Vec::new();
    }
    let mut z = line.to_vec();
    z.push(0);
    let mut n = 0i32;
    // SAFETY: cadena terminada en NUL; el resultado se libera con LocalFree.
    unsafe {
        let argv = CommandLineToArgvW(z.as_ptr(), &mut n);
        if argv.is_null() {
            return Vec::new();
        }
        let args = (0..n.max(0) as usize)
            .map(|i| {
                let p = *argv.add(i);
                let len = (0..).take_while(|&j| *p.add(j) != 0).count();
                String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
            })
            .collect();
        LocalFree(argv.cast());
        args
    }
}

/// Argumentos con los que se lanzó el proceso, empezando por el programa.
pub fn process_args(pid: i32) -> Option<Vec<String>> {
    let line = command_line(pid as u32)?;
    Some(split_command_line(&line))
}

/// Proceso abierto para leer su memoria, si es de 64 bits como Garbo (los parámetros de un
/// proceso de 32 bits tienen otra disposición).
fn open_for_read(pid: i32) -> Option<Handle> {
    let h = Handle::open(
        pid as u32,
        PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
    )?;
    let mut wow64 = 0;
    // SAFETY: proceso abierto con permiso de consulta.
    if unsafe { IsWow64Process(h.0, &mut wow64) } == 0 || wow64 != 0 {
        return None;
    }
    Some(h)
}

fn read<T: Copy>(h: &Handle, addr: usize) -> Option<T> {
    let mut v = MaybeUninit::<T>::uninit();
    let mut got = 0usize;
    // SAFETY: se escriben como mucho size_of::<T>() bytes y solo se usa si se leyeron todos.
    let ok = unsafe {
        ReadProcessMemory(
            h.0,
            addr as *const c_void,
            v.as_mut_ptr().cast(),
            size_of::<T>(),
            &mut got,
        )
    } != 0;
    (ok && got == size_of::<T>()).then(|| unsafe { v.assume_init() })
}

fn read_wide(h: &Handle, addr: usize, chars: usize) -> Option<Vec<u16>> {
    let mut v = vec![0u16; chars];
    let mut got = 0usize;
    // SAFETY: el búfer tiene `chars` caracteres de 2 bytes.
    let ok = unsafe {
        ReadProcessMemory(
            h.0,
            addr as *const c_void,
            v.as_mut_ptr().cast(),
            chars * 2,
            &mut got,
        )
    } != 0;
    (ok && got == chars * 2).then_some(v)
}

/// Dirección de `RTL_USER_PROCESS_PARAMETERS` del proceso (PEB + 0x20 en 64 bits).
fn process_parameters(h: &Handle) -> Option<usize> {
    // SAFETY: estructura del tamaño indicado; la función solo escribe en ella.
    let mut pbi: PROCESS_BASIC_INFORMATION = unsafe { zeroed() };
    let mut len = 0u32;
    let status = unsafe {
        NtQueryInformationProcess(
            h.0,
            ProcessBasicInformation,
            (&mut pbi as *mut PROCESS_BASIC_INFORMATION).cast(),
            size_of::<PROCESS_BASIC_INFORMATION>() as u32,
            &mut len,
        )
    };
    if status < 0 || pbi.PebBaseAddress.is_null() {
        return None;
    }
    read::<usize>(h, pbi.PebBaseAddress as usize + 0x20).filter(|&p| p != 0)
}

/// `UNICODE_STRING` de otro proceso de 64 bits.
#[derive(Clone, Copy)]
#[repr(C)]
struct RemoteString {
    length: u16,
    _max: u16,
    _pad: u32,
    buffer: u64,
}

// Desplazamientos en `RTL_USER_PROCESS_PARAMETERS` (64 bits).
const CURRENT_DIRECTORY: usize = 0x38;
const ENVIRONMENT: usize = 0x80;
const ENVIRONMENT_SIZE: usize = 0x3F0;

/// Carpeta de trabajo del proceso. Ojo: PowerShell no la cambia con `cd` (`Set-Location`); para
/// él vale la que informa la integración (OSC 7, ver `blocks.rs`).
pub fn process_cwd(pid: i32) -> Option<PathBuf> {
    let h = open_for_read(pid)?;
    let params = process_parameters(&h)?;
    let s: RemoteString = read(&h, params + CURRENT_DIRECTORY)?;
    if s.buffer == 0 || s.length == 0 || s.length > 32767 * 2 {
        return None;
    }
    let raw = read_wide(&h, s.buffer as usize, usize::from(s.length) / 2)?;
    let mut path = OsString::from_wide(&raw).into_string().ok()?;
    // "C:\Users\ana\" -> "C:\Users\ana" (pero "C:\" se queda).
    if path.len() > 3 && path.ends_with('\\') {
        path.pop();
    }
    Some(PathBuf::from(path))
}

/// Variable `key` del entorno del proceso (sin distinguir mayúsculas; vacía cuenta como no
/// puesta).
pub fn process_env(pid: i32, key: &str) -> Option<OsString> {
    let h = open_for_read(pid)?;
    let params = process_parameters(&h)?;
    let addr: usize = read(&h, params + ENVIRONMENT)?;
    let size: usize = read(&h, params + ENVIRONMENT_SIZE)?;
    if addr == 0 || size == 0 || size > 1 << 20 {
        return None;
    }
    let block = read_wide(&h, addr, size / 2)?;
    let key: Vec<u16> = key.encode_utf16().collect();
    block
        .split(|&c| c == 0)
        .take_while(|kv| !kv.is_empty())
        .find_map(|kv| {
            // Las variables "=C:" de las unidades empiezan por '=': se buscan desde el segundo.
            let eq = kv.iter().skip(1).position(|&c| c == b'=' as u16)? + 1;
            let (k, v) = (&kv[..eq], &kv[eq + 1..]);
            let same = k.len() == key.len()
                && String::from_utf16_lossy(k)
                    .eq_ignore_ascii_case(&String::from_utf16_lossy(&key));
            (same && !v.is_empty()).then(|| OsString::from_wide(v))
        })
}

/// Archivos abiertos por el proceso: Windows no los enseña sin recorrer los handles de todo el
/// sistema, así que no se usan (los subagentes se buscan por la carpeta y los argumentos).
pub fn open_files(_pid: i32) -> Vec<PathBuf> {
    Vec::new()
}

/// Hijos directos del proceso.
pub fn children(pid: i32) -> Vec<i32> {
    with_table(|procs| {
        procs
            .iter()
            .filter(|p| p.ppid == pid as u32 && p.pid != pid as u32 && !is_console_host(&p.exe))
            .map(|p| p.pid as i32)
            .collect()
    })
}

/// Las DLL que se cargan por nombre (`conpty.dll`, que busca alacritty_terminal para usar la
/// ConPTY de Windows Terminal) solo se buscan en la carpeta de Garbo y en la del sistema, nunca en
/// la carpeta actual ni en el `PATH`: así nadie puede colar una con ese nombre. Al empezar `main`.
pub fn harden_dll_search() {
    use windows_sys::Win32::System::LibraryLoader::{
        SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
    };
    // SAFETY: solo cambia el orden de búsqueda de las DLL de este proceso.
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS) };
}

/// Identificador de la app para Windows (agrupar sus ventanas en la barra de tareas y sus avisos,
/// ver `toast.rs`). Antes de crear la ventana.
pub fn set_app_user_model_id(id: &str) {
    use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    let id = wide(id);
    // SAFETY: cadena terminada en NUL que vive durante la llamada.
    unsafe { SetCurrentProcessExplicitAppUserModelID(id.as_ptr()) };
}

/// Escribe un valor de texto en `HKEY_CURRENT_USER\<key>` (creando la clave si no existe).
pub fn set_user_registry(key: &str, name: &str, value: &str) -> bool {
    use windows_sys::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    let (key, name, value) = (wide(key), wide(name), wide(value));
    // SAFETY: cadenas terminadas en NUL; el tamaño incluye el NUL final, en bytes.
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        ) == 0
    }
}

/// Devuelve al sistema las páginas de memoria que Garbo no está usando. Al arrancar se tocan
/// muchas que no vuelven a hacer falta (inicializar el controlador de la GPU, compilar los
/// shaders, buscar las fuentes): ~47 de 54 MB con una NVIDIA. Pasan a la lista de espera de
/// Windows, libres para el resto del sistema; si alguna vuelve a hacer falta se recupera de ahí
/// sin leer del disco. Lo que se usa al dibujar vuelve solo (5–8 MB). Ver docs/Rendimiento.md.
pub fn trim_memory() {
    use windows_sys::Win32::System::ProcessStatus::K32EmptyWorkingSet;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: el pseudohandle del propio proceso siempre es válido.
    unsafe { K32EmptyWorkingSet(GetCurrentProcess()) };
}

/// Extensiones ejecutables (`PATHEXT`, con un valor por defecto si no está puesta).
fn path_exts() -> Vec<String> {
    std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .filter(|e| e.starts_with('.') && e.len() > 1)
        .map(|e| e.to_ascii_lowercase())
        .collect()
}

fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase()))
        .unwrap_or_default()
}

/// ¿Se puede ejecutar el archivo? En Windows lo dice la extensión (`PATHEXT`, más los scripts de
/// PowerShell).
pub fn is_executable(meta: &std::fs::Metadata, path: &Path) -> bool {
    let ext = extension(path);
    meta.is_file() && (ext == ".ps1" || path_exts().contains(&ext))
}

/// Busca un programa en el `PATH` probando las extensiones de `PATHEXT` (`code` -> `code.cmd`).
pub fn find_program(name: &str) -> Option<PathBuf> {
    let exts = path_exts();
    let candidates = |base: PathBuf| {
        let has_ext = exts.contains(&extension(&base));
        let mut v = Vec::new();
        if has_ext {
            v.push(base.clone());
        }
        v.extend(exts.iter().map(|e| {
            let mut s = base.clone().into_os_string();
            s.push(e);
            PathBuf::from(s)
        }));
        v
    };
    if name.contains(['/', '\\']) {
        return candidates(PathBuf::from(name))
            .into_iter()
            .find(|p| p.is_file());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|d| candidates(d.join(name)))
        .find(|p| p.is_file())
}

/// Archivos que el Explorador ejecutaría al abrirlos (programas, scripts, accesos directos,
/// instaladores…). Un enlace del terminal nunca debe lanzarlos: se muestran en su carpeta.
fn runs_when_opened(path: &Path) -> bool {
    const RISKY: &[&str] = &[
        ".appref-ms",
        ".application",
        ".cpl",
        ".diagcab",
        ".gadget",
        ".hta",
        ".inf",
        ".jar",
        ".js",
        ".jse",
        ".library-ms",
        ".lnk",
        ".msc",
        ".msi",
        ".msix",
        ".msixbundle",
        ".msp",
        ".appx",
        ".appxbundle",
        ".pif",
        ".ps1",
        ".psm1",
        ".py",
        ".pyw",
        ".pyz",
        ".reg",
        ".scf",
        ".scr",
        ".search-ms",
        ".settingcontent-ms",
        ".url",
        ".vb",
        ".vbe",
        ".vbs",
        ".ws",
        ".wsc",
        ".wsf",
        ".wsh",
    ];
    let ext = extension(path);
    RISKY.contains(&ext.as_str()) || path_exts().contains(&ext)
}

fn shell_execute(verb: &str, file: &OsStr, params: Option<&OsStr>) -> bool {
    let (verb, file) = (wide(verb), wide(file));
    let params = params.map(wide);
    // SAFETY: cadenas terminadas en NUL que viven durante la llamada.
    let r = unsafe {
        ShellExecuteW(
            null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ref().map_or(null(), |p| p.as_ptr()),
            null(),
            SW_SHOWNORMAL,
        )
    };
    // Según la documentación, un valor mayor que 32 indica éxito.
    r as usize > 32
}

/// Abre una URL (solo web y correo), una carpeta o un archivo con su programa predeterminado. Lo
/// que se ejecutaría al abrirlo (`.exe`, `.bat`, `.py`, accesos directos…) se muestra
/// seleccionado en el Explorador en lugar de lanzarlo.
pub fn open(target: &str) {
    let ok = if target.contains("://") || target.starts_with("mailto:") {
        let lower = target.to_ascii_lowercase();
        if !["http://", "https://", "ftp://", "mailto:"]
            .iter()
            .any(|s| lower.starts_with(s))
        {
            log::warn!("No se abre {target}: esquema no permitido");
            return;
        }
        shell_execute("open", OsStr::new(target), None)
    } else {
        let path = Path::new(target);
        if path.is_file() && runs_when_opened(path) {
            let mut arg = OsString::from("/select,");
            arg.push(path.as_os_str());
            shell_execute("open", OsStr::new("explorer.exe"), Some(&arg))
        } else {
            shell_execute("open", path.as_os_str(), None)
        }
    };
    if !ok {
        log::warn!("No se pudo abrir {target}");
    }
}

/// Editor para Ctrl+clic en un archivo cuando no hay `$VISUAL` ni `$EDITOR`. Abrir un archivo con
/// su programa predeterminado no vale en Windows: un `.py` o un `.js` se ejecutarían. VS Code si
/// está instalado y, si no, el Bloc de notas.
pub fn default_editor() -> Option<String> {
    Some(if find_program("code").is_some() {
        "code".into()
    } else {
        "notepad".into()
    })
}

/// Tiempo entre dos clics para que cuenten como doble clic: el de la configuración del ratón de
/// Windows (500 ms de serie).
pub fn double_click_time() -> Duration {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
    // SAFETY: solo lee un ajuste del sistema.
    let ms = unsafe { GetDoubleClickTime() };
    Duration::from_millis(u64::from(ms.clamp(100, 5000)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parte_la_linea_de_ordenes() {
        let line: Vec<u16> = r#""C:\Program Files\nodejs\node.exe" "C:\a b\cli.js" --resume x"#
            .encode_utf16()
            .collect();
        assert_eq!(
            split_command_line(&line),
            [
                r"C:\Program Files\nodejs\node.exe",
                r"C:\a b\cli.js",
                "--resume",
                "x"
            ]
        );
        assert!(split_command_line(&[]).is_empty());
    }

    #[test]
    fn lee_su_propio_proceso() {
        let me = std::process::id() as i32;
        let args = process_args(me).unwrap();
        assert!(!args.is_empty());
        assert_eq!(
            process_cwd(me).unwrap(),
            std::env::current_dir().unwrap(),
            "carpeta leída de la memoria del proceso"
        );
        let path = std::env::var_os("PATH").unwrap();
        assert_eq!(
            process_env(me, "path"),
            Some(path),
            "sin distinguir mayúsculas"
        );
        assert_eq!(process_env(me, "GARBO_NO_EXISTE"), None);
        assert!(process_name(me).is_some());
    }

    #[test]
    fn primer_plano_de_un_shell() {
        // Un cmd que lanza otro proceso que espera: el primer plano es el nieto.
        let mut child = std::process::Command::new("cmd")
            .args(["/d", "/c", "ping -n 30 127.0.0.1 >nul"])
            .spawn()
            .unwrap();
        let shell = child.id();
        let probe = Probe { shell };
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut fg = None;
        while Instant::now() < deadline {
            fg = probe.foreground().filter(|&p| p != shell as i32);
            if fg.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
        let fg = fg.expect("el hijo de cmd en primer plano");
        assert_eq!(
            process_name(fg).map(|n| n.to_lowercase()).as_deref(),
            Some("ping")
        );
    }

    /// Lo que sale por ConPTY de un programa que escribe las marcas de Garbo: tienen que llegar
    /// intactas (la ConPTY de Windows reescribe la pantalla y se come las secuencias que no conoce).
    #[test]
    fn conpty_deja_pasar_las_marcas() {
        use alacritty_terminal::event::WindowSize;
        use alacritty_terminal::tty::EventedReadWrite;
        use std::io::Read;
        let script = "$e=[char]27; $b=[char]7; [Console]::Write(\"$e]133;A$b\" + 'hola' + \"$e]9;9;`\"C:\\`\"$b\" + \"$e]7501;?$e\\\" + \"$e]133;D;0$b\"); Start-Sleep -Milliseconds 300";
        let opts = tty::Options {
            shell: Some(tty::Shell::new(
                "powershell.exe".into(),
                vec!["-NoProfile".into(), "-Command".into(), script.into()],
            )),
            escape_args: true,
            ..Default::default()
        };
        let ws = WindowSize {
            num_lines: 24,
            num_cols: 80,
            cell_width: 8,
            cell_height: 16,
        };
        let mut pty = tty::new(&opts, ws, 0).unwrap();
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            // En Windows el lector de alacritty da 0 (no WouldBlock) mientras no hay datos.
            match pty.reader().read(&mut buf) {
                Ok(0) => std::thread::sleep(Duration::from_millis(20)),
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
            if out.windows(8).any(|w| w == b"133;D;0\x07") {
                break;
            }
        }
        let text = String::from_utf8_lossy(&out);
        eprintln!("ConPTY: {:?}", text);
        assert!(text.contains("\x1b]133;A\x07"), "OSC 133;A");
        assert!(text.contains("\x1b]9;9;\"C:\\\"\x07"), "OSC 9;9");
        assert!(text.contains("\x1b]7501;?\x1b\\"), "OSC 7501");
        assert!(text.contains("\x1b]133;D;0\x07"), "OSC 133;D");
    }

    #[test]
    fn no_lanza_ejecutables() {
        assert!(runs_when_opened(Path::new(r"C:\x\setup.exe")));
        assert!(runs_when_opened(Path::new(r"C:\x\script.PY")));
        assert!(runs_when_opened(Path::new(r"C:\x\acceso.lnk")));
        assert!(!runs_when_opened(Path::new(r"C:\x\notas.txt")));
        assert!(!runs_when_opened(Path::new(r"C:\x\captura.png")));
    }
}
