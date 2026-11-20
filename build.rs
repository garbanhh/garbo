//! En Windows, el icono y los datos de versión de garbo.exe: rellena la plantilla
//! `packaging/windows/garbo.rc` y la compila como recurso. En el resto de sistemas no hace nada.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resource();
    }
}

#[cfg(windows)]
fn windows_resource() {
    use std::path::PathBuf;
    println!("cargo:rerun-if-changed=packaging/windows/garbo.rc");
    println!("cargo:rerun-if-changed=packaging/io.github.garbanhh.Garbo.ico");
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    // "0.10.2" -> 0,10,2,0 para FILEVERSION.
    let mut parts: Vec<String> = version
        .split(['.', '-'])
        .take(3)
        .map(|p| p.parse::<u16>().unwrap_or(0).to_string())
        .collect();
    parts.resize(4, "0".into());
    let icon = root.join("packaging").join("io.github.garbanhh.Garbo.ico");
    let template = std::fs::read_to_string(root.join("packaging/windows/garbo.rc"))
        .expect("plantilla del recurso");
    let rc = template
        // En las cadenas de un .rc la barra invertida se dobla.
        .replace("@ICON@", &icon.to_string_lossy().replace('\\', "\\\\"))
        .replace("@VERSION_NUM@", &parts.join(","))
        .replace("@VERSION_STR@", &version);
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("garbo.rc");
    std::fs::write(&out, rc).expect("escribir el recurso");
    // Sin el recurso Garbo funciona igual (sin icono propio): solo se avisa.
    if let Err(e) = embed_resource::compile(&out, embed_resource::NONE).manifest_optional() {
        println!("cargo:warning=No se pudo compilar el recurso de Windows: {e}");
    }
}
