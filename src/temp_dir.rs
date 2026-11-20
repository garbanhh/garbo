use std::io;
use std::path::{Path, PathBuf};

/// Directorio privado y único, eliminado también si la operación falla.
pub struct TempDir(PathBuf);

impl TempDir {
    #[cfg(unix)]
    pub fn new_in(parent: &Path) -> io::Result<Self> {
        use std::ffi::{CString, OsStr};
        use std::os::unix::ffi::OsStrExt;
        let template = parent.join(".garbo-XXXXXX");
        let mut template = CString::new(template.as_os_str().as_bytes())?.into_bytes_with_nul();
        // SAFETY: el búfer es modificable y termina en los seis marcadores y un NUL.
        if unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) }.is_null() {
            return Err(io::Error::last_os_error());
        }
        template.pop();
        Ok(Self(PathBuf::from(OsStr::from_bytes(&template))))
    }

    /// En Windows la carpeta hereda los permisos de su padre (la del usuario); el nombre único
    /// sale del proceso, la hora y un contador, y `create_dir` falla si ya existe.
    #[cfg(windows)]
    pub fn new_in(parent: &Path) -> io::Result<Self> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        for _ in 0..100 {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos());
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = parent.join(format!(".garbo-{}-{nanos:x}-{n}", std::process::id()));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no se pudo crear una carpeta temporal única",
        ))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
