//! Configuración de usuario: `~/.config/garbo/config.toml`.
//! Todo tiene valores por defecto razonables; el archivo es opcional.

use crate::style::{Background, BarSize, Blocks, Buttons, Interface, Prompt, Style, Tabs};
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
    /// Cambios sobre la forma del tema (pestañas, prompt, bloques…). Ver `style.rs`.
    pub style: crate::style::StyleOverrides,
    /// acción -> combinación, p.ej. `new_tab = "ctrl+shift+t"`.
    pub keybindings: HashMap<String, String>,
    /// Consultar en GitHub si hay versión nueva (al arrancar y cada 24 h).
    pub check_updates: bool,
    /// Avisos del escritorio (agente que termina).
    pub notifications: NotifyConfig,
    /// Dónde van las pestañas, su ancho y dónde va el nombre. Ver `tabs.rs`.
    pub tabs: crate::tabs::TabsConfig,
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
            style: crate::style::StyleOverrides::default(),
            keybindings: HashMap::new(),
            check_updates: true,
            notifications: NotifyConfig::default(),
            tabs: crate::tabs::TabsConfig::default(),
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
    /// Contorno de un bloque (el que se ve sin foco).
    Hollow,
}

impl CursorStyle {
    pub const ALL: [CursorStyle; 4] = [
        CursorStyle::Block,
        CursorStyle::Beam,
        CursorStyle::Underline,
        CursorStyle::Hollow,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CursorStyle::Block => "Bloque",
            CursorStyle::Beam => "Barra",
            CursorStyle::Underline => "Subrayado",
            CursorStyle::Hollow => "Hueco",
        }
    }
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
    /// `ssh` y `mosh` abren el shell del servidor con la integración de Garbo (bloques, prompt,
    /// agentes), sin instalar nada allí. Ver shell/ssh.sh. En pestañas nuevas; de momento, Linux.
    pub ssh_integration: bool,
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
            ssh_integration: true,
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
        // Se escribe lo que ya estaba en el archivo y lo que no es el valor por defecto. Lo demás
        // no se añade: así un valor por defecto nuevo (o el de otro sistema) le llega a quien no
        // lo cambió. Hasta la 0.11.1 se escribía todo al tocar cualquier ajuste y quedaba fijado
        // (en Windows, Consolas a 12 pt).
        let d = Config::default();
        let mut put = |table: &str, key: &str, item: toml_edit::Item, default: bool| {
            let present = if table.is_empty() {
                doc.contains_key(key)
            } else {
                doc.get(table).and_then(|t| t.get(key)).is_some()
            };
            if default && !present {
                return;
            }
            if table.is_empty() {
                doc[key] = item;
            } else {
                // Una sección nueva, como `[sección]` (sin esto, toml_edit la escribe en línea).
                if !doc.contains_key(table) {
                    doc[table] = toml_edit::table();
                }
                doc[table][key] = item;
            }
        };
        let notify = |w: NotifyWhen| match w {
            NotifyWhen::Unfocused => "unfocused",
            NotifyWhen::Always => "always",
            NotifyWhen::Never => "never",
        };
        let cursor = |c: CursorStyle| match c {
            CursorStyle::Block => "block",
            CursorStyle::Beam => "beam",
            CursorStyle::Underline => "underline",
            CursorStyle::Hollow => "hollow",
        };
        let tab_key = |k: TabKey| match k {
            TabKey::Menu => "menu",
            TabKey::Accept => "accept",
        };
        let (n, dn) = (&self.notifications, &d.notifications);
        let (f, df) = (&self.font, &d.font);
        let (w, dw) = (&self.window, &d.window);
        let (t, dt) = (&self.terminal, &d.terminal);
        let (tb, dtb) = (&self.tabs, &d.tabs);
        put("", "theme", value(&self.theme), self.theme == d.theme);
        put(
            "",
            "check_updates",
            value(self.check_updates),
            self.check_updates == d.check_updates,
        );
        put(
            "notifications",
            "agents",
            value(notify(n.agents)),
            n.agents == dn.agents,
        );
        put(
            "notifications",
            "min_work_secs",
            value(n.min_work_secs as i64),
            n.min_work_secs == dn.min_work_secs,
        );
        put(
            "notifications",
            "sound",
            value(n.sound),
            n.sound == dn.sound,
        );
        put(
            "notifications",
            "blocked",
            value(n.blocked),
            n.blocked == dn.blocked,
        );
        put("font", "family", value(&f.family), f.family == df.family);
        put("font", "size", value(r2(f.size)), r2(f.size) == r2(df.size));
        put(
            "font",
            "line_height",
            value(r2(f.line_height)),
            r2(f.line_height) == r2(df.line_height),
        );
        put(
            "window",
            "padding",
            value(r2(w.padding)),
            r2(w.padding) == r2(dw.padding),
        );
        put(
            "window",
            "opacity",
            value(r2(w.opacity)),
            r2(w.opacity) == r2(dw.opacity),
        );
        put(
            "window",
            "decorations",
            value(w.decorations),
            w.decorations == dw.decorations,
        );
        put(
            "terminal",
            "cursor_style",
            value(cursor(t.cursor_style)),
            t.cursor_style == dt.cursor_style,
        );
        put(
            "terminal",
            "cursor_blink",
            value(t.cursor_blink),
            t.cursor_blink == dt.cursor_blink,
        );
        put(
            "terminal",
            "copy_on_select",
            value(t.copy_on_select),
            t.copy_on_select == dt.copy_on_select,
        );
        put(
            "terminal",
            "scrollback",
            value(t.scrollback as i64),
            t.scrollback == dt.scrollback,
        );
        put("terminal", "blocks", value(t.blocks), t.blocks == dt.blocks);
        put(
            "terminal",
            "shell_integration",
            value(t.shell_integration),
            t.shell_integration == dt.shell_integration,
        );
        put(
            "terminal",
            "autosuggest",
            value(t.autosuggest),
            t.autosuggest == dt.autosuggest,
        );
        put(
            "terminal",
            "tab_key",
            value(tab_key(t.tab_key)),
            t.tab_key == dt.tab_key,
        );
        put(
            "terminal",
            "complete_as_you_type",
            value(t.complete_as_you_type),
            t.complete_as_you_type == dt.complete_as_you_type,
        );
        put(
            "terminal",
            "garbo_prompt",
            value(t.garbo_prompt),
            t.garbo_prompt == dt.garbo_prompt,
        );
        put(
            "terminal",
            "ssh_integration",
            value(t.ssh_integration),
            t.ssh_integration == dt.ssh_integration,
        );
        put(
            "tabs",
            "position",
            value(tb.position.key()),
            tb.position == dtb.position,
        );
        put(
            "tabs",
            "width",
            value(tb.width.key()),
            tb.width == dtb.width,
        );
        put(
            "tabs",
            "align",
            value(tb.align.key()),
            tb.align == dtb.align,
        );
        put(
            "tabs",
            "working",
            value(tb.working.key()),
            tb.working == dtb.working,
        );
        if !self.style.is_empty() && !doc.contains_key("style") {
            doc["style"] = toml_edit::table();
        }
        for (key, item) in self.style.entries() {
            match item {
                Some(item) => doc["style"][key] = item,
                None => {
                    if let Some(t) = doc.get_mut("style").and_then(|t| t.as_table_like_mut()) {
                        t.remove(key);
                    }
                }
            }
        }
        if doc
            .get("style")
            .and_then(|t| t.as_table_like())
            .is_some_and(|t| t.is_empty())
        {
            doc.remove("style");
        }
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
        t.style = self.style.apply(self.base_style());
        t.style.interface = self.interface();
        t
    }

    /// Letra de la interfaz (familia y alternativas) si es proporcional; `None` = la del terminal.
    /// `[font] ui_family` manda; si está vacía, la interfaz Flotante usa la proporcional del
    /// sistema y la Clásica, la del terminal. La del sistema: Adwaita Sans (la de GNOME, derivada
    /// de Inter) en Linux y Segoe UI en Windows (la de Segoe UI Variable es una sola fuente
    /// variable, que se dibujaría siempre con su grosor de serie).
    pub fn ui_font(&self) -> Option<(String, Vec<String>)> {
        let f = &self.font;
        if !f.ui_family.trim().is_empty() {
            return Some((f.ui_family.clone(), f.ui_fallback()));
        }
        let system = if cfg!(windows) {
            "Segoe UI"
        } else {
            "Adwaita Sans"
        };
        (self.interface() == Interface::Floating).then(|| (system.to_string(), f.ui_fallback()))
    }

    /// Interfaz de la ventana: la elegida en `[style]` o la de serie (Flotante).
    pub fn interface(&self) -> Interface {
        self.style.interface.unwrap_or(Interface::DEFAULT)
    }

    /// La forma de la que parte `[style]`: la del tema o, con la interfaz Flotante y un tema sin
    /// forma propia, la de Flotante. Su interfaz es la de serie, así lo que se guarde en `[style]`
    /// es solo lo que el usuario cambió.
    pub fn base_style(&self) -> Style {
        let theme = Theme::builtin(&self.theme)
            .map(|t| t.style)
            .unwrap_or_default();
        let base = if self.interface() == Interface::Floating && theme == Style::default() {
            Style::floating()
        } else {
            theme
        };
        Style {
            interface: Interface::DEFAULT,
            ..base
        }
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

    /// Al cambiar un ajuste no se fijan en el archivo los demás valores por defecto (la fuente,
    /// sobre todo); lo que ya estaba escrito sí se actualiza.
    #[test]
    fn guardar_solo_escribe_lo_que_no_es_por_defecto() {
        let dir = crate::temp_dir::TempDir::new_in(&std::env::temp_dir()).unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "# mío\n[font]\nsize = 14.0\n").unwrap();
        let mut cfg = Config::load_at(&path);
        assert_eq!(cfg.font.size, 14.0);
        cfg.theme = "nord".into();
        cfg.font.size = Config::default().font.size;
        cfg.save_at(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# mío"), "{text}");
        assert!(text.contains("theme = \"nord\""), "{text}");
        assert!(!text.contains("family"), "{text}");
        assert!(
            !text.contains("[terminal]") && !text.contains("cursor_style"),
            "{text}"
        );
        let back = Config::load_at(&path);
        assert_eq!(back.theme, "nord");
        assert_eq!(back.font.size, Config::default().font.size);
        // Las secciones nuevas salen como tablas, no en línea.
        let mut cfg = back;
        cfg.terminal.scrollback = 2000;
        cfg.save_at(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[terminal]\nscrollback = 2000"), "{text}");
        // En un archivo nuevo, con todo por defecto, no se escribe nada.
        let fresh = dir.path().join("nuevo.toml");
        Config::default().save_at(&fresh).unwrap();
        assert_eq!(fs::read_to_string(&fresh).unwrap().trim(), "");
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

    /// `[style]` guarda solo lo que se cambió; lo que vuelve a ser del tema se borra (y la sección
    /// entera si queda vacía), sin tocar el resto del archivo.
    #[test]
    fn guardar_la_forma_y_quitar_lo_que_vuelve_al_tema() {
        let dir = crate::temp_dir::TempDir::new_in(&std::env::temp_dir()).unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "# mío\ntheme = \"duna\"\n").unwrap();
        let mut cfg = Config::load_at(&path);
        cfg.style.tabs = Some(Tabs::Pill);
        cfg.style.radius = Some(10);
        cfg.style.prompt_gap = Some(6);
        cfg.style.output_gap = Some(4);
        cfg.style.line_gap = Some(8);
        cfg.save_at(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[style]"), "{text}");
        assert!(text.contains("tabs = \"pill\""), "{text}");
        assert!(text.contains("radius = 10"), "{text}");
        assert!(text.contains("prompt_gap = 6"), "{text}");
        assert!(text.contains("output_gap = 4"), "{text}");
        assert!(text.contains("line_gap = 8"), "{text}");
        let mut back = Config::load_at(&path);
        assert_eq!(back.style.tabs, Some(Tabs::Pill));
        assert_eq!(back.style.radius, Some(10));
        assert_eq!(back.style.prompt_gap, Some(6));
        assert_eq!(back.style.line_gap, Some(8));
        back.style.radius = None;
        back.save_at(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("tabs = \"pill\"") && !text.contains("radius"),
            "{text}"
        );
        back.style = crate::style::StyleOverrides::default();
        back.save_at(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("style"), "{text}");
        assert!(text.contains("# mío"), "{text}");
    }

    #[test]
    fn cada_tema_tiene_colores_y_nombre() {
        for name in ALL_NAMES {
            let t = Theme::builtin(name).unwrap_or_else(|| panic!("{name}"));
            assert_ne!(t.foreground, t.background, "{name}");
            assert!(!Theme::display_name(name).is_empty());
        }
        assert_eq!(Theme::display_name("fosforo"), "Fósforo");
        assert_eq!(
            Theme::builtin("garbo-black").unwrap().style,
            Style::default()
        );
        // Cada tema trae su forma, en los dos sistemas.
        let cfg = Config {
            theme: "bosque".into(),
            ..Config::default()
        };
        assert_eq!(cfg.theme().style.tabs, Tabs::Folder);
        assert_eq!(Theme::names(), ALL_NAMES);
    }

    #[test]
    fn interfaz_flotante_por_defecto() {
        use crate::style::{Buttons, Interface};
        let cfg = Config::default();
        let s = cfg.theme().style;
        // Los temas de siempre estrenan la forma de Flotante, en Linux y en Windows, con los
        // botones de ventana del sistema.
        assert_eq!(s.interface, Interface::Floating);
        let system = Style::default().buttons;
        assert_eq!((s.tabs, s.buttons, s.radius), (Tabs::Pill, system, 10));
        if cfg!(windows) {
            assert_eq!(system, Buttons::Flat);
        }
        // Un tema con forma propia la conserva.
        let bosque = Config {
            theme: "bosque".into(),
            ..Config::default()
        };
        let b = bosque.theme().style;
        assert_eq!((b.tabs, b.interface), (Tabs::Folder, Interface::Floating));
        // Lo que el usuario fija en [style] manda.
        let mio = Config {
            style: toml::from_str("tabs = \"underline\"").unwrap(),
            ..Config::default()
        };
        assert_eq!(mio.theme().style.tabs, Tabs::Underline);
        // Clásica: la forma de siempre, tal cual.
        let clasica = Config {
            style: toml::from_str("interface = \"classic\"").unwrap(),
            ..Config::default()
        };
        assert_eq!(clasica.theme().style, Style::default());
    }

    #[test]
    fn letra_de_la_interfaz() {
        let mut cfg = Config::default();
        // Flotante: la proporcional del sistema (en Linux, la de GNOME primero).
        let (family, fallback) = cfg.ui_font().unwrap();
        if cfg!(windows) {
            assert_eq!(family, "Segoe UI");
        } else {
            assert_eq!(family, "Adwaita Sans");
            assert!(fallback.iter().any(|f| f == "Cantarell"));
        }
        // Clásica: la del terminal, como siempre.
        cfg.style = toml::from_str("interface = \"classic\"").unwrap();
        assert_eq!(cfg.ui_font(), None);
        // La que elija el usuario, con cualquier interfaz.
        cfg.font.ui_family = "Inter".into();
        assert_eq!(cfg.ui_font().map(|f| f.0), Some("Inter".to_string()));
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
    /// Forma de la interfaz (la del tema con los cambios de `[style]`).
    pub style: Style,
}

/// Temas incorporados. Los 7 primeros existen desde siempre (con la forma de siempre); los demás
/// traen además su propia forma.
const ALL_NAMES: &[&str] = &[
    "garbo-black",
    "garbo-dark",
    "garbo-light",
    "tokyo-night",
    "catppuccin-mocha",
    "gruvbox-dark",
    "nord",
    "grafito",
    "abisal",
    "fosforo",
    "bosque",
    "niebla",
    "ciruela",
    "papel",
    "neon",
    "duna",
    "glaciar",
    "tinta",
    "cobalto",
];

impl Theme {
    /// Temas que se ofrecen.
    pub fn names() -> &'static [&'static str] {
        ALL_NAMES
    }

    /// Nombre para mostrar en la interfaz ("garbo-black" -> "Garbo Black").
    pub fn display_name(name: &str) -> String {
        match name {
            "fosforo" => return "Fósforo".into(),
            "neon" => return "Neón".into(),
            _ => {}
        }
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
            "grafito" => (
                0xE4DFD6,
                0x161514,
                0xE8B866,
                0x3B3427,
                0x0F0E0D,
                0x211F1C,
                0xE8B866,
                [
                    0x22201E, 0xE06C5F, 0x9DBB7A, 0xE8B866, 0x7FA3C7, 0xC397B5, 0x7DB8AE, 0xCFC8BC,
                    0x5C5650, 0xF08A7C, 0xB5D292, 0xF2CC88, 0x9CBDE0, 0xD7B0CA, 0x98D0C6, 0xF5F1EA,
                ],
            ),
            "abisal" => (
                0xCFE3F0,
                0x0A1018,
                0x5EE6D0,
                0x173149,
                0x070B11,
                0x14243A,
                0x5EE6D0,
                [
                    0x13202E, 0xFF6B81, 0x5EE6A8, 0xFFD479, 0x5AA8FF, 0xB591FF, 0x5EE6D0, 0xA9C4D6,
                    0x3D566E, 0xFF8DA0, 0x86F2C2, 0xFFE3A3, 0x8CC4FF, 0xCDB3FF, 0x8FF3E3, 0xE8F6FF,
                ],
            ),
            "fosforo" => (
                0xFFB547,
                0x0E0A04,
                0xFFC870,
                0x4A3210,
                0x090602,
                0x1E1508,
                0xFFD08A,
                [
                    0x1A1208, 0xFF6A3D, 0xFFC870, 0xFFD98F, 0xE8A23E, 0xFF9E5E, 0xF2B860, 0xE6A84A,
                    0x6B4A1E, 0xFF8A5E, 0xFFE0A6, 0xFFEAC2, 0xF5BD5E, 0xFFB98A, 0xFFD08A, 0xFFF1D6,
                ],
            ),
            "bosque" => (
                0xD3DDD4,
                0x0F1512,
                0xD9A066,
                0x2A3B31,
                0x090D0B,
                0x18211C,
                0xD9A066,
                [
                    0x1A231E, 0xE07A6A, 0x8FC48A, 0xD9B866, 0x78A8B8, 0xC493B0, 0x7EC0A8, 0xBCC8BE,
                    0x4A5A50, 0xF09484, 0xA8DBA2, 0xEBCB80, 0x96C2D2, 0xD9ADC8, 0x9AD6C0, 0xEEF3EE,
                ],
            ),
            "niebla" => (
                0x24292F,
                0xF4F5F7,
                0x2F5BD3,
                0xCCD8F5,
                0xE4E7EC,
                0xFFFFFF,
                0x2F5BD3,
                [
                    0x24292F, 0xC93A3A, 0x2E8540, 0x9A6700, 0x2F5BD3, 0x8A44B8, 0x127F8C, 0xC9CED6,
                    0x6A737D, 0xE0534F, 0x3BA357, 0xBF8500, 0x4F7AE8, 0xA260CF, 0x1E9AA8, 0xFFFFFF,
                ],
            ),
            "ciruela" => (
                0xE8DCEB,
                0x1A1420,
                0xFF9E7A,
                0x3D2D4A,
                0x130E18,
                0x2A2033,
                0xFF9E7A,
                [
                    0x241B2B, 0xFF6F8E, 0xA8D98A, 0xFFCB7A, 0x8FA8FF, 0xD69BFF, 0x7FD6D0, 0xCFC2D6,
                    0x5A4866, 0xFF8FA8, 0xBFE8A6, 0xFFDCA0, 0xADBFFF, 0xE3B8FF, 0xA0E6E0, 0xF8F0FA,
                ],
            ),
            "papel" => (
                0x2D2B26,
                0xEFEBE2,
                0x29579C,
                0xD3DAE6,
                0xE2DDD1,
                0xEFEBE2,
                0x29579C,
                [
                    0x2D2B26, 0xB4442E, 0x4F7A35, 0x946300, 0x29579C, 0x8A4C8F, 0x2C7A78, 0xCFC8B8,
                    0x77705F, 0xCC5A42, 0x61924A, 0xB07A0C, 0x3E6DB5, 0xA262A6, 0x3A9491, 0xFBF8F1,
                ],
            ),
            "neon" => (
                0xEDE4FF,
                0x120A1F,
                0xFF4FD8,
                0x3A1F5C,
                0x0C0616,
                0x22123A,
                0xFF4FD8,
                [
                    0x1E1233, 0xFF5C8A, 0x3DFFB0, 0xFFE15C, 0x4FB4FF, 0xFF4FD8, 0x3FF0FF, 0xCFC2EA,
                    0x4E3B70, 0xFF85A8, 0x7DFFC9, 0xFFEA8F, 0x85CCFF, 0xFF85E4, 0x85F6FF, 0xFFFFFF,
                ],
            ),
            "duna" => (
                0xF0DCC6,
                0x1C1410,
                0xFF8A3D,
                0x4A2E1E,
                0x140E0B,
                0x2A1E17,
                0xFF8A3D,
                [
                    0x2A1E17, 0xE5533D, 0xA9B665, 0xF2B33D, 0x7DAEA3, 0xD3869B, 0x89B482, 0xDCC7B0,
                    0x6B5345, 0xF2735E, 0xC2CF85, 0xFFCB66, 0x9CC9BE, 0xE5A3B5, 0xA6CCA0, 0xFFF1E2,
                ],
            ),
            "glaciar" => (
                0xDDEEF8,
                0x0C1A26,
                0x9AD8FF,
                0x1F3A52,
                0x091520,
                0x1A3044,
                0x9AD8FF,
                [
                    0x16293A, 0xFF7A8A, 0x7FE0B0, 0xFFD98A, 0x6FB8FF, 0xB9A2FF, 0x7FE3F0, 0xB8CCD8,
                    0x4A6378, 0xFF9AA6, 0xA3EDC8, 0xFFE6AE, 0x9ACDFF, 0xCFC0FF, 0xA6EEF6, 0xF4FBFF,
                ],
            ),
            "tinta" => (
                0xE8E8E8,
                0x0B0B0B,
                0xFFFFFF,
                0x3A3A3A,
                0x0B0B0B,
                0x1C1C1C,
                0xFFFFFF,
                [
                    0x1A1A1A, 0xE06C6C, 0x9CC49C, 0xD8D8D8, 0xBDBDBD, 0xA8A8A8, 0xCFCFCF, 0xD0D0D0,
                    0x555555, 0xF08A8A, 0xB5D8B5, 0xEFEFEF, 0xE0E0E0, 0xC8C8C8, 0xE8E8E8, 0xFFFFFF,
                ],
            ),
            "cobalto" => (
                0xE8EEFF,
                0x0A2472,
                0xFFD23F,
                0x2D50B5,
                0x071A55,
                0x0F2E8A,
                0xFFD23F,
                [
                    0x071A55, 0xFF6B6B, 0x5CE08A, 0xFFD23F, 0x8DBBFF, 0xE08AFF, 0x4FE0E0, 0xC9D4F2,
                    0x4A5E9E, 0xFF8F8F, 0x8AF0AE, 0xFFE27A, 0xA6CAFF, 0xEDB0FF, 0x85F0F0, 0xFFFFFF,
                ],
            ),
            _ => return None,
        };
        let style = Self::builtin_style(name);
        Some(Theme {
            foreground: Rgb::hex(fg),
            background: Rgb::hex(bg),
            cursor: Rgb::hex(cursor),
            selection: Rgb::hex(sel),
            tab_bar: Rgb::hex(bar),
            tab_active: Rgb::hex(active),
            accent: Rgb::hex(accent),
            palette: pal.map(Rgb::hex),
            style,
        })
    }

    /// Forma propia de cada tema; los de siempre, la de siempre. Los temas sin botones de ventana
    /// propios llevan los del sistema (`system`: redondos en Linux, rectangulares en Windows).
    fn builtin_style(name: &str) -> Style {
        let system = Style::default().buttons;
        let s = |tabs, prompt, blocks, buttons, bar, background, radius| Style {
            tabs,
            prompt,
            blocks,
            buttons,
            bar,
            background,
            radius,
            prompt_gap: 0,
            block_gap: 0,
            line_gap: 0,
            output_gap: 0,
            interface: Interface::Classic,
        };
        use Background::{Gradient, Solid};
        use BarSize::{Compact, Normal, Tall};
        match name {
            "grafito" => s(
                Tabs::Underline,
                Prompt::Border,
                Blocks::Bar,
                system,
                Normal,
                Solid,
                4,
            ),
            "abisal" => s(
                Tabs::Pill,
                Prompt::Soft,
                Blocks::Card,
                system,
                Normal,
                Gradient,
                10,
            ),
            "fosforo" => s(
                Tabs::Brackets,
                Prompt::Brackets,
                Blocks::Dashed,
                Buttons::Flat,
                Normal,
                Solid,
                0,
            ),
            "bosque" => s(
                Tabs::Folder,
                Prompt::Solid,
                Blocks::Plain,
                Buttons::Color,
                Normal,
                Solid,
                8,
            ),
            "niebla" => s(
                Tabs::Segmented,
                Prompt::Plain,
                Blocks::Separator,
                Buttons::Icons,
                Tall,
                Solid,
                8,
            ),
            "ciruela" => s(
                Tabs::Pill,
                Prompt::Soft,
                Blocks::Bar,
                Buttons::Color,
                Normal,
                Solid,
                12,
            ),
            "papel" => s(
                Tabs::TopLine,
                Prompt::Dimmed,
                Blocks::Header,
                Buttons::Icons,
                Normal,
                Solid,
                4,
            ),
            "neon" => s(
                Tabs::Outline,
                Prompt::Soft,
                Blocks::Dot,
                Buttons::Color,
                Normal,
                Gradient,
                6,
            ),
            "duna" => s(
                Tabs::Folder,
                Prompt::Tab,
                Blocks::Status,
                system,
                Normal,
                Gradient,
                6,
            ),
            "glaciar" => s(
                Tabs::Floating,
                Prompt::Underline,
                Blocks::Outline,
                system,
                Normal,
                Gradient,
                10,
            ),
            "tinta" => s(
                Tabs::Underline,
                Prompt::Plain,
                Blocks::Fade,
                Buttons::Icons,
                Tall,
                Solid,
                2,
            ),
            "cobalto" => s(
                Tabs::Dot,
                Prompt::Solid,
                Blocks::Errors,
                Buttons::Flat,
                Compact,
                Solid,
                0,
            ),
            _ => Style::default(),
        }
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
