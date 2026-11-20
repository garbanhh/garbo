//! Configuración de usuario: `~/.config/garbo/config.toml`.
//! Todo tiene valores por defecto razonables; el archivo es opcional.

use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub font: FontConfig,
    pub window: WindowConfig,
    pub shell: ShellConfig,
    pub terminal: TerminalConfig,
    /// Nombre de tema incorporado (ver `Theme::builtin`).
    pub theme: String,
    /// Sobrescrituras de colores sobre el tema elegido.
    pub colors: ColorOverrides,
    /// acción -> combinación, p.ej. `new_tab = "ctrl+shift+t"`.
    pub keybindings: HashMap<String, String>,
    /// Consultar en GitHub si hay versión nueva (al arrancar y cada 24 h).
    pub check_updates: bool,
    /// Avisos del escritorio (agente que termina).
    pub notifications: NotifyConfig,
}

/// Cuándo avisar de que un agente terminó.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyWhen {
    /// Si no se está viendo su pestaña (ventana sin foco u otra pestaña).
    #[default]
    Unfocused,
    Always,
    Never,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NotifyConfig {
    pub agents: NotifyWhen,
    /// Solo si el agente trabajó al menos estos segundos.
    pub min_work_secs: u64,
    /// Con sonido (el de "completado" del tema de sonidos).
    pub sound: bool,
    /// Avisar cuando un agente espera tu permiso o tu respuesta (si lo informa por OSC 7501).
    pub blocked: bool,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            agents: NotifyWhen::Unfocused,
            min_work_secs: 5,
            sound: true,
            blocked: true,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font: FontConfig::default(),
            window: WindowConfig::default(),
            shell: ShellConfig::default(),
            terminal: TerminalConfig::default(),
            theme: "garbo-black".into(),
            colors: ColorOverrides::default(),
            keybindings: HashMap::new(),
            check_updates: true,
            notifications: NotifyConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
    pub line_height: f32,
    pub fallback: Vec<String>,
    /// Fuente de la interfaz (pestañas, paneles, paleta, ajustes). Vacía = la del terminal.
    pub ui_family: String,
    pub ui_size: f32,
}

impl Default for FontConfig {
    #[cfg(not(windows))]
    fn default() -> Self {
        Self {
            // Mismos valores por defecto que GNOME Terminal.
            family: "Adwaita Mono".into(),
            size: 11.0,
            line_height: 1.0,
            fallback: strings(&[
                "JetBrains Mono",
                "Noto Sans Mono",
                "DejaVu Sans Mono",
                "Liberation Mono",
                "Symbols Nerd Font Mono",
                "Noto Sans Symbols 2",
                "Noto Sans Mono CJK SC",
                "Noto Color Emoji",
                "Noto Emoji",
                "Symbola",
            ]),
            ui_family: String::new(),
            ui_size: 10.5,
        }
    }

    /// La misma que en Linux, Adwaita Mono, que va dentro del ejecutable (ver `font::EMBEDDED`),
    /// un poco más pequeña: en Windows la escala suele ser 1 y 11 puntos se ven grandes.
    /// Símbolos, CJK y emoji con las fuentes de Windows.
    #[cfg(windows)]
    fn default() -> Self {
        Self {
            family: "Adwaita Mono".into(),
            size: 10.0,
            line_height: 1.0,
            // Adwaita Mono también aquí: si la familia elegida no está (p. ej. "Cascadia Mono",
            // que Ajustes guardaba hasta la 0.11.0), se usa la incluida y no Consolas.
            fallback: strings(&[
                "Adwaita Mono",
                "Cascadia Mono",
                "Consolas",
                "JetBrains Mono",
                "Symbols Nerd Font Mono",
                "Segoe UI Symbol",
                "MS Gothic",
                "Microsoft YaHei",
                "Malgun Gothic",
                "Segoe UI Emoji",
                "Segoe UI Historic",
            ]),
            ui_family: String::new(),
            ui_size: 9.5,
        }
    }
}

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

impl FontConfig {
    pub fn ui_fallback(&self) -> Vec<String> {
        let names: &[&str] = if cfg!(windows) {
            &["Segoe UI Variable Text", "Segoe UI"]
        } else {
            &[
                "Cantarell",
                "Inter",
                "Noto Sans",
                "DejaVu Sans",
                "Liberation Sans",
            ]
        };
        let mut v = strings(names);
        v.extend(self.fallback.iter().cloned());
        v
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    pub padding: f32,
    pub opacity: f32,
    /// `false` = barra de título integrada en la barra de pestañas.
    pub decorations: bool,
    pub columns: u16,
    pub lines: u16,
    /// Mostrar la barra de pestañas aunque solo haya una.
    pub always_show_tabs: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            padding: 10.0,
            opacity: 1.0,
            decorations: false,
            columns: 110,
            lines: 32,
            always_show_tabs: true,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    /// Vacío = `$SHELL` del usuario.
    pub program: Option<String>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    Block,
    Beam,
    Underline,
}

/// Qué hace Tab en el prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabKey {
    /// Abre el menú de completado (como Warp); → acepta la sugerencia en gris.
    #[default]
    Menu,
    /// Acepta la sugerencia en gris si la hay; si no, abre el menú.
    Accept,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub scrollback: usize,
    pub cursor_style: CursorStyle,
    pub cursor_blink: bool,
    /// Líneas por paso de rueda del ratón.
    pub scroll_multiplier: f32,
    /// Copiar automáticamente al soltar la selección.
    pub copy_on_select: bool,
    /// Inyectar la integración de shell (bash, zsh o fish) que delimita los bloques de comandos.
    pub shell_integration: bool,
    /// Dibujar los bloques (separadores, estado, duración).
    pub blocks: bool,
    /// Sugerir en gris el resto del comando a partir del historial (se acepta con →). En fish
    /// apaga sus sugerencias propias en las pestañas nuevas.
    pub autosuggest: bool,
    /// Qué hace Tab: abrir el menú de completado o aceptar la sugerencia.
    pub tab_key: TabKey,
    /// Abrir el menú de completado solo, mientras se escribe.
    pub complete_as_you_type: bool,
    /// Prompt de Garbo: la carpeta como etiqueta encima de lo que se escribe y, en los comandos ya
    /// ejecutados, en gris con su duración (con la integración del shell; en pestañas nuevas).
    pub garbo_prompt: bool,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            scrollback: 10_000,
            cursor_style: CursorStyle::Block,
            cursor_blink: false,
            scroll_multiplier: 3.0,
            copy_on_select: false,
            shell_integration: true,
            blocks: true,
            autosuggest: true,
            tab_key: TabKey::Menu,
            complete_as_you_type: false,
            garbo_prompt: true,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ColorOverrides {
    pub foreground: Option<String>,
    pub background: Option<String>,
    pub cursor: Option<String>,
    pub selection: Option<String>,
    pub tab_bar: Option<String>,
    pub tab_active: Option<String>,
    pub accent: Option<String>,
    /// 16 colores ANSI (0-7 normales, 8-15 brillantes).
    pub palette: Option<Vec<String>>,
}

impl Config {
    /// `~/.config/garbo/config.toml` (en Windows, `%APPDATA%\garbo\config.toml`). `GARBO_CONFIG`
    /// apunta a otro archivo: sirve para probar sin tocar el real (en Windows no hay
    /// `XDG_CONFIG_HOME`).
    pub fn path() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("GARBO_CONFIG").filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(p));
        }
        dirs::config_dir().map(|d| d.join("garbo").join("config.toml"))
    }

    /// Carga la config. Si hay un error de sintaxis se informa y se usan los valores por defecto.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        Self::load_at(&path)
    }

    pub(crate) fn load_at(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(cfg) => cfg,
                Err(e) => {
                    log::error!("Error en {}: {e}", path.display());
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    /// Guarda los valores que se pueden cambiar desde Ajustes. Edita el documento con toml_edit
    /// (se conservan comentarios y el resto de claves) y lo sustituye de una vez: se escribe al
    /// lado y se renombra encima. Un archivo de solo lectura no se toca.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "Sin carpeta de configuración")
        })?;
        self.save_at(&path)
    }

    pub(crate) fn save_at(&self, path: &Path) -> io::Result<()> {
        use toml_edit::{value, DocumentMut};
        let (path, permissions, mut doc) = match fs::canonicalize(path) {
            Ok(path) => {
                let text = fs::read_to_string(&path)?;
                // Valida la estructura antes de editar secciones con toml_edit.
                toml::from_str::<Self>(&text)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                let doc = text
                    .parse::<DocumentMut>()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                let permissions = fs::metadata(&path)?.permissions();
                // `chmod 444` para congelarlo: se respeta (renombrar encima lo saltaría).
                if permissions.readonly() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "config.toml es de solo lectura",
                    ));
                }
                (path, Some(permissions), doc)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // Si queda un enlace sin destino, se conserva para que pueda repararse.
                match fs::symlink_metadata(path) {
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        (path.to_owned(), None, DocumentMut::new())
                    }
                    _ => return Err(e),
                }
            }
            Err(e) => return Err(e),
        };
        let r2 = |v: f32| ((v as f64) * 100.0).round() / 100.0;
        doc["theme"] = value(self.theme.clone());
        doc["check_updates"] = value(self.check_updates);
        doc["notifications"]["agents"] = value(match self.notifications.agents {
            NotifyWhen::Unfocused => "unfocused",
            NotifyWhen::Always => "always",
            NotifyWhen::Never => "never",
        });
        doc["notifications"]["min_work_secs"] = value(self.notifications.min_work_secs as i64);
        doc["notifications"]["sound"] = value(self.notifications.sound);
        doc["notifications"]["blocked"] = value(self.notifications.blocked);
        doc["font"]["family"] = value(self.font.family.clone());
        doc["font"]["size"] = value(r2(self.font.size));
        doc["font"]["line_height"] = value(r2(self.font.line_height));
        doc["window"]["padding"] = value(r2(self.window.padding));
        doc["window"]["opacity"] = value(r2(self.window.opacity));
        doc["window"]["decorations"] = value(self.window.decorations);
        let cursor = match self.terminal.cursor_style {
            CursorStyle::Block => "block",
            CursorStyle::Beam => "beam",
            CursorStyle::Underline => "underline",
        };
        doc["terminal"]["cursor_style"] = value(cursor);
        doc["terminal"]["cursor_blink"] = value(self.terminal.cursor_blink);
        doc["terminal"]["copy_on_select"] = value(self.terminal.copy_on_select);
        doc["terminal"]["scrollback"] = value(self.terminal.scrollback as i64);
        doc["terminal"]["blocks"] = value(self.terminal.blocks);
        doc["terminal"]["shell_integration"] = value(self.terminal.shell_integration);
        doc["terminal"]["autosuggest"] = value(self.terminal.autosuggest);
        doc["terminal"]["tab_key"] = value(match self.terminal.tab_key {
            TabKey::Menu => "menu",
            TabKey::Accept => "accept",
        });
        doc["terminal"]["complete_as_you_type"] = value(self.terminal.complete_as_you_type);
        doc["terminal"]["garbo_prompt"] = value(self.terminal.garbo_prompt);
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let dir = crate::temp_dir::TempDir::new_in(parent)?;
        let staged = dir.path().join("config.toml");
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        // Solo para el usuario. En Windows lo da la carpeta (`%APPDATA%`, privada de cada usuario).
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&staged)?;
        file.write_all(doc.to_string().as_bytes())?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        // Sin fsync: se guarda en cada paso de un ajuste, en el hilo de la interfaz. Renombrar
        // encima del archivo ya hace que btrfs y ext4 escriban antes los datos.
        drop(file);
        fs::rename(staged, path)
    }

    pub fn theme(&self) -> Theme {
        let mut t = Theme::builtin(&self.theme).unwrap_or_else(|| {
            log::warn!("Tema '{}' no existe, usando garbo-black", self.theme);
            Theme::builtin("garbo-black").unwrap()
        });
        let c = &self.colors;
        let set = |slot: &mut Rgb, v: &Option<String>| {
            if let Some(rgb) = v.as_deref().and_then(Rgb::parse) {
                *slot = rgb;
            }
        };
        set(&mut t.foreground, &c.foreground);
        set(&mut t.background, &c.background);
        set(&mut t.cursor, &c.cursor);
        set(&mut t.selection, &c.selection);
        set(&mut t.tab_bar, &c.tab_bar);
        set(&mut t.tab_active, &c.tab_active);
        set(&mut t.accent, &c.accent);
        if let Some(p) = &c.palette {
            for (i, s) in p.iter().take(16).enumerate() {
                if let Some(rgb) = Rgb::parse(s) {
                    t.palette[i] = rgb;
                }
            }
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guardar_rechaza_toml_invalido_sin_tocar_el_archivo() {
        let dir = crate::temp_dir::TempDir::new_in(&std::env::temp_dir()).unwrap();
        let path = dir.path().join("config.toml");
        for original in [
            "# comentario\n[shell]\nprogram = \"/bin/zsh\"\n[colors]\nbackground =\n",
            "# comentario\nfont = 12\n[shell]\nprogram = \"/bin/zsh\"\n",
        ] {
            fs::write(&path, original).unwrap();
            let cfg = Config::load_at(&path);
            assert_eq!(
                cfg.save_at(&path).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), original);
        }
    }

    #[test]
    fn guardar_respeta_un_archivo_de_solo_lectura() {
        let dir = crate::temp_dir::TempDir::new_in(&std::env::temp_dir()).unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "theme = \"garbo-black\"\n").unwrap();
        let mut ro = fs::metadata(&path).unwrap().permissions();
        ro.set_readonly(true);
        fs::set_permissions(&path, ro).unwrap();
        let mut cfg = Config::load_at(&path);
        cfg.theme = "otro".into();
        let err = cfg.save_at(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "theme = \"garbo-black\"\n",
            "sin tocar"
        );
        assert!(fs::metadata(&path).unwrap().permissions().readonly());
    }

    // Los enlaces simbólicos de Windows necesitan permisos especiales.
    #[cfg(unix)]
    #[test]
    fn guardar_conserva_el_enlace_al_archivo() {
        let dir = crate::temp_dir::TempDir::new_in(&std::env::temp_dir()).unwrap();
        let path = dir.path().join("config.toml");
        let target = dir.path().join("dotfiles.toml");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        Config::default().save_at(&target).unwrap();
        let cfg = Config {
            theme: "garbo-light".into(),
            ..Config::default()
        };
        cfg.save_at(&path).unwrap();
        assert_eq!(fs::read_link(&path).unwrap(), target);
        assert_eq!(Config::load_at(&target).theme, "garbo-light");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn parse(s: &str) -> Option<Rgb> {
        let s = s.trim().trim_start_matches('#').trim_start_matches("0x");
        if s.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        Some(Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }

    const fn hex(v: u32) -> Rgb {
        Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    /// Color lineal premultiplicado listo para la GPU.
    pub fn to_f32(self, alpha: f32) -> [f32; 4] {
        [
            self.0 as f32 / 255.0,
            self.1 as f32 / 255.0,
            self.2 as f32 / 255.0,
            alpha,
        ]
    }

    pub fn lerp(self, other: Rgb, t: f32) -> Rgb {
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Rgb(m(self.0, other.0), m(self.1, other.1), m(self.2, other.2))
    }
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub selection: Rgb,
    pub tab_bar: Rgb,
    pub tab_active: Rgb,
    pub accent: Rgb,
    pub palette: [Rgb; 16],
}

impl Theme {
    pub const NAMES: &'static [&'static str] = &[
        "garbo-black",
        "garbo-dark",
        "garbo-light",
        "tokyo-night",
        "catppuccin-mocha",
        "gruvbox-dark",
        "nord",
    ];

    /// Nombre para mostrar en la interfaz ("garbo-black" -> "Garbo Black").
    pub fn display_name(name: &str) -> String {
        name.split(['-', '_'])
            .filter(|w| !w.is_empty())
            .map(|w| {
                let mut c = w.chars();
                c.next().map_or(String::new(), |f| {
                    f.to_uppercase().collect::<String>() + c.as_str()
                })
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn builtin(name: &str) -> Option<Theme> {
        let (fg, bg, cursor, sel, bar, active, accent, pal): (
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
            [u32; 16],
        ) = match name {
            "garbo-black" => (
                0xD4D4D4,
                0x000000,
                0xD4D4D4,
                0x264F78,
                0x0A0A0A,
                0x1C1C1C,
                0x7AA2F7,
                [
                    0x1E1E1E, 0xF14C4C, 0x23D18B, 0xE5C07B, 0x3B8EEA, 0xC586C0, 0x29B8DB, 0xCCCCCC,
                    0x5C5C5C, 0xF48771, 0x4EE6A5, 0xF5DA82, 0x6FB3FF, 0xD7A3D1, 0x56D4EC, 0xFFFFFF,
                ],
            ),
            "garbo-dark" => (
                0xD7DAE0,
                0x16181D,
                0x7AA2F7,
                0x2E3A55,
                0x0F1115,
                0x1F232B,
                0x7AA2F7,
                [
                    0x1D2027, 0xF26D78, 0x8FD18B, 0xE6C07B, 0x6FA8FF, 0xC792EA, 0x56C8D8, 0xC8CCD4,
                    0x4B5263, 0xFF8A94, 0xA7E3A1, 0xF2D49B, 0x8FBFFF, 0xDDB0F6, 0x7FE0EC, 0xFFFFFF,
                ],
            ),
            "garbo-light" => (
                0x2A2E36,
                0xFAFAFA,
                0x3B6FE0,
                0xCFDDF7,
                0xE9EAEE,
                0xFAFAFA,
                0x3B6FE0,
                [
                    0x2A2E36, 0xD1393E, 0x3E9A3A, 0xB27A00, 0x3B6FE0, 0x9B4FC8, 0x1C8C9C, 0xD5D7DC,
                    0x6B717D, 0xE5484D, 0x4CB648, 0xC99400, 0x5A88F0, 0xB06ADB, 0x2BA6B8, 0xFFFFFF,
                ],
            ),
            "tokyo-night" => (
                0xC0CAF5,
                0x1A1B26,
                0xC0CAF5,
                0x33467C,
                0x16161E,
                0x24283B,
                0x7AA2F7,
                [
                    0x15161E, 0xF7768E, 0x9ECE6A, 0xE0AF68, 0x7AA2F7, 0xBB9AF7, 0x7DCFFF, 0xA9B1D6,
                    0x414868, 0xF7768E, 0x9ECE6A, 0xE0AF68, 0x7AA2F7, 0xBB9AF7, 0x7DCFFF, 0xC0CAF5,
                ],
            ),
            "catppuccin-mocha" => (
                0xCDD6F4,
                0x1E1E2E,
                0xF5E0DC,
                0x45475A,
                0x181825,
                0x313244,
                0xCBA6F7,
                [
                    0x45475A, 0xF38BA8, 0xA6E3A1, 0xF9E2AF, 0x89B4FA, 0xF5C2E7, 0x94E2D5, 0xBAC2DE,
                    0x585B70, 0xF38BA8, 0xA6E3A1, 0xF9E2AF, 0x89B4FA, 0xF5C2E7, 0x94E2D5, 0xA6ADC8,
                ],
            ),
            "gruvbox-dark" => (
                0xEBDBB2,
                0x282828,
                0xEBDBB2,
                0x504945,
                0x1D2021,
                0x3C3836,
                0xFABD2F,
                [
                    0x282828, 0xCC241D, 0x98971A, 0xD79921, 0x458588, 0xB16286, 0x689D6A, 0xA89984,
                    0x928374, 0xFB4934, 0xB8BB26, 0xFABD2F, 0x83A598, 0xD3869B, 0x8EC07C, 0xEBDBB2,
                ],
            ),
            "nord" => (
                0xD8DEE9,
                0x2E3440,
                0xD8DEE9,
                0x434C5E,
                0x272C36,
                0x3B4252,
                0x88C0D0,
                [
                    0x3B4252, 0xBF616A, 0xA3BE8C, 0xEBCB8B, 0x81A1C1, 0xB48EAD, 0x88C0D0, 0xE5E9F0,
                    0x4C566A, 0xBF616A, 0xA3BE8C, 0xEBCB8B, 0x81A1C1, 0xB48EAD, 0x8FBCBB, 0xECEFF4,
                ],
            ),
            _ => return None,
        };
        Some(Theme {
            foreground: Rgb::hex(fg),
            background: Rgb::hex(bg),
            cursor: Rgb::hex(cursor),
            selection: Rgb::hex(sel),
            tab_bar: Rgb::hex(bar),
            tab_active: Rgb::hex(active),
            accent: Rgb::hex(accent),
            palette: pal.map(Rgb::hex),
        })
    }

    /// Paleta xterm de 256 colores: 16 del tema + cubo 6x6x6 + 24 grises.
    pub fn color256(&self, idx: u8) -> Rgb {
        match idx {
            0..=15 => self.palette[idx as usize],
            16..=231 => {
                let i = idx - 16;
                let c = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
                Rgb(c(i / 36), c((i / 6) % 6), c(i % 6))
            }
            _ => {
                let v = 8 + (idx - 232) * 10;
                Rgb(v, v, v)
            }
        }
    }
}
