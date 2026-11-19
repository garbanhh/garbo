use std::ffi::{CString, OsStr};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// Directorio privado y único, eliminado también si la operación falla.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new_in(parent: &Path) -> io::Result<Self> {
        let template = parent.join(".garbo-XXXXXX");
        let mut template = CString::new(template.as_os_str().as_bytes())?.into_bytes_with_nul();
        // SAFETY: el búfer es modificable y termina en los seis marcadores y un NUL.
        if unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) }.is_null() {
            return Err(io::Error::last_os_error());
        }
        template.pop();
        Ok(Self(PathBuf::from(OsStr::from_bytes(&template))))
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
