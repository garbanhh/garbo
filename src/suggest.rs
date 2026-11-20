//! Sugerencias de comandos (texto en gris tras el cursor).
//!
//! Mientras se escribe en el prompt se muestra en gris el resto de un comando ya usado que empieza
//! igual; → (o Ctrl+F) lo acepta entero y Ctrl+→ palabra a palabra, escribiéndolo en el shell. Si
//! el historial no tiene nada, se sugiere el único candidato del completado que encaje ("cd Doc" ->
//! "umentos/"). El menú de completado (Tab) está en `complete.rs`.
//!
//! Lo que se está escribiendo se lee de la propia pantalla: desde la marca OSC 133;B (fin del
//! prompt) hasta el cursor. Por eso solo funciona con la integración de shell activa.
//!
//! Historial: el del shell (bash: `$HISTFILE` o `~/.bash_history`; zsh: `~/.zsh_history`; fish:
//! `~/.local/share/fish/fish_history`), releído si cambia, más el historial propio de Garbo
//! (`~/.local/share/garbo/history.tsv`), que guarda cada comando ejecutado con la carpeta donde se
//! ejecutó (los shells no la guardan).
//!
//! Para que lo sugerido tenga sentido:
//!   - primero lo ejecutado en esta misma carpeta; después, lo de cualquier sitio;
//!   - un `cd` a una ruta relativa solo se sugiere donde se hizo: `cd Proyectos` ejecutado en
//!     ~/Documentos no se ofrece en ~ aunque exista ~/Proyectos (otra carpeta con el mismo nombre);
//!   - solo líneas cuya primera palabra se puede ejecutar (programa del PATH, orden interna del
//!     shell, o alias/función de sus archivos de arranque): fuera erratas y fallos;
//!   - `cd X` solo si X existe desde el directorio actual;
//!   - hacen falta al menos 2 caracteres escritos;
//!   - los comandos que acaban en "orden no encontrada" (126/127) no se guardan;
//!   - nada mientras readline reescribe la línea (búsqueda con Ctrl+R).

use crate::complete::Kind;
use crate::shell::Shell;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// Entradas como máximo (las más recientes).
const MAX_ENTRIES: usize = 20_000;
/// Registros del historial propio: al cargarlo con más de estos, se queda con la mitad.
const MAX_RECORDS: usize = 20_000;
/// Carpetas recordadas por comando.
const MAX_DIRS: usize = 8;
/// Cada cuánto, como mucho, se mira si los archivos de historial cambiaron.
const RELOAD_EVERY: Duration = Duration::from_secs(5);
/// Cada cuánto se rehace la lista de órdenes ejecutables.
const KNOWN_EVERY: Duration = Duration::from_secs(60);
/// Cuánto vale una sugerencia calculada para la misma entrada (el dibujo la pide en cada fotograma).
const CACHE_FOR: Duration = Duration::from_secs(1);
/// Caracteres escritos necesarios para sugerir.
const MIN_PREFIX: usize = 2;

/// Órdenes internas y palabras clave de zsh (`${(k)builtins} ${(k)reswords}`, zsh 5.9).
const ZSH_BUILTINS: &[&str] = &[
    "!",
    ".",
    ":",
    "[",
    "[[",
    "{",
    "}",
    "alias",
    "autoload",
    "bg",
    "bindkey",
    "break",
    "builtin",
    "bye",
    "case",
    "cd",
    "chdir",
    "command",
    "compadd",
    "comparguments",
    "compcall",
    "compctl",
    "compdescribe",
    "compfiles",
    "compgroups",
    "compquote",
    "compset",
    "comptags",
    "comptry",
    "compvalues",
    "continue",
    "coproc",
    "declare",
    "dirs",
    "disable",
    "disown",
    "do",
    "done",
    "echo",
    "echotc",
    "echoti",
    "elif",
    "else",
    "emulate",
    "enable",
    "end",
    "esac",
    "eval",
    "exec",
    "exit",
    "export",
    "false",
    "fc",
    "fg",
    "fi",
    "float",
    "for",
    "foreach",
    "function",
    "functions",
    "getln",
    "getopts",
    "hash",
    "history",
    "if",
    "integer",
    "jobs",
    "kill",
    "let",
    "limit",
    "local",
    "log",
    "logout",
    "nocorrect",
    "noglob",
    "popd",
    "print",
    "printf",
    "private",
    "pushd",
    "pushln",
    "pwd",
    "r",
    "read",
    "readonly",
    "rehash",
    "repeat",
    "return",
    "sched",
    "select",
    "set",
    "setopt",
    "shift",
    "source",
    "suspend",
    "test",
    "then",
    "time",
    "times",
    "trap",
    "true",
    "ttyctl",
    "type",
    "typeset",
    "ulimit",
    "umask",
    "unalias",
    "unfunction",
    "unhash",
    "unlimit",
    "unset",
    "unsetopt",
    "until",
    "vared",
    "wait",
    "whence",
    "where",
    "which",
    "while",
    "zcompile",
    "zformat",
    "zle",
    "zmodload",
    "zparseopts",
    "zregexparse",
    "zstyle",
];

/// Órdenes internas de fish (`builtin -n`, fish 4.6). Sus funciones de serie (`alias`, `funced`,
/// `fish_add_path`…) salen de las carpetas de funciones.
const FISH_BUILTINS: &[&str] = &[
    "!",
    ".",
    ":",
    "[",
    "_",
    "abbr",
    "and",
    "argparse",
    "begin",
    "bg",
    "bind",
    "block",
    "break",
    "breakpoint",
    "builtin",
    "case",
    "cd",
    "command",
    "commandline",
    "complete",
    "contains",
    "continue",
    "count",
    "disown",
    "echo",
    "else",
    "emit",
    "end",
    "eval",
    "exec",
    "exit",
    "false",
    "fg",
    "fish_indent",
    "fish_key_reader",
    "for",
    "function",
    "functions",
    "history",
    "if",
    "jobs",
    "math",
    "not",
    "or",
    "path",
    "printf",
    "pwd",
    "random",
    "read",
    "realpath",
    "return",
    "set",
    "set_color",
    "source",
    "status",
    "string",
    "switch",
    "test",
    "time",
    "true",
    "type",
    "ulimit",
    "wait",
    "while",
];

/// Órdenes internas y palabras clave de bash.
const BUILTINS: &[&str] = &[
    ".", ":", "[", "[[", "alias", "bg", "bind", "break", "builtin", "case", "cd", "command",
    "compgen", "complete", "continue", "declare", "dirs", "disown", "echo", "enable", "eval",
    "exec", "exit", "export", "false", "fc", "fg", "for", "function", "getopts", "hash", "help",
    "history", "if", "jobs", "kill", "let", "local", "logout", "mapfile", "popd", "printf",
    "pushd", "pwd", "read", "readonly", "return", "select", "set", "shift", "shopt", "source",
    "suspend", "test", "time", "times", "trap", "true", "type", "typeset", "ulimit", "umask",
    "unalias", "unset", "until", "wait", "while",
];

/// Un comando del historial y las carpetas donde se ejecutó en Garbo (la más reciente primero).
struct Entry {
    cmd: String,
    dirs: Vec<PathBuf>,
}

/// Comando ejecutado en Garbo (historial propio).
struct Record {
    cmd: String,
    dir: Option<PathBuf>,
}

#[derive(Default)]
struct History {
    /// De más antiguo a más reciente, sin repetidos.
    entries: Vec<Entry>,
    /// Comandos del historial del shell.
    shell: Vec<String>,
    /// Historial propio, de más antiguo a más reciente.
    garbo: Vec<Record>,
    /// Archivo del historial del shell leído y su fecha.
    shell_file: Option<(PathBuf, Option<SystemTime>)>,
    garbo_mtime: Option<SystemTime>,
    garbo_loaded: bool,
    checked: Option<Instant>,
    /// Cambia con cada modificación (invalida la caché de sugerencias).
    generation: u64,
}

static HISTORY: Mutex<Option<History>> = Mutex::new(None);

/// Archivo de historial del shell en uso.
fn history_file(shell: Shell) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    match shell {
        Shell::Bash | Shell::Other => Some(
            std::env::var_os("HISTFILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".bash_history")),
        ),
        // `HISTFILE` lo pone cada .zshrc (sin exportarlo): los nombres habituales, el más reciente.
        Shell::Zsh => {
            let dir = std::env::var_os("ZDOTDIR").map_or(home, PathBuf::from);
            [".zsh_history", ".zhistory", ".histfile"]
                .iter()
                .map(|f| dir.join(f))
                .filter_map(|p| Some((mtime(&p)?, p)))
                .max()
                .map(|(_, p)| p)
        }
        Shell::Fish => Some(dirs::data_dir()?.join("fish").join("fish_history")),
    }
}

/// Comandos de un archivo de historial, del más antiguo al más reciente. Los de varias líneas se
/// descartan (no se sugieren).
fn parse_shell_history(shell: Shell, bytes: &[u8]) -> Vec<String> {
    match shell {
        Shell::Bash | Shell::Other => String::from_utf8_lossy(bytes)
            .lines()
            .filter(|l| !is_timestamp(l))
            .map(str::to_string)
            .collect(),
        Shell::Zsh => parse_zsh_history(bytes),
        Shell::Fish => parse_fish_history(&String::from_utf8_lossy(bytes)),
    }
}

/// zsh: una línea por comando, con `: inicio:duración;` delante si usa EXTENDED_HISTORY; una
/// barra al final de la línea sigue el comando en la siguiente. Algunos bytes van "metafied":
/// 0x83 y luego el byte con el bit 0x20 cambiado.
fn parse_zsh_history(bytes: &[u8]) -> Vec<String> {
    let mut raw = Vec::with_capacity(bytes.len());
    let mut it = bytes.iter();
    while let Some(&b) = it.next() {
        match b {
            0x83 => raw.extend(it.next().map(|n| n ^ 0x20)),
            _ => raw.push(b),
        }
    }
    let text = String::from_utf8_lossy(&raw);
    let mut out = Vec::new();
    let mut continued = false;
    for line in text.lines() {
        let multi = continued;
        continued = line.ends_with('\\');
        if multi || continued {
            continue;
        }
        let extended = line.strip_prefix(": ").and_then(|r| r.split_once(';'));
        let cmd = match extended {
            Some((when, cmd)) if when.bytes().all(|b| b.is_ascii_digit() || b == b':') => cmd,
            _ => line,
        };
        out.push(cmd.to_string());
    }
    out
}

/// fish: `- cmd: orden` seguido de `when:` y `paths:` con sangría; `\\` y `\n` escapados.
fn parse_fish_history(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("- cmd: "))
        .filter_map(|c| {
            let mut out = String::with_capacity(c.len());
            let mut chars = c.chars();
            while let Some(ch) = chars.next() {
                if ch != '\\' {
                    out.push(ch);
                } else if chars.next() == Some('\\') {
                    out.push('\\');
                } else {
                    return None; // \n: varias líneas
                }
            }
            Some(out)
        })
        .collect()
}

/// Historial propio de Garbo (`GARBO_HISTORY` para usar otro archivo).
#[cfg(not(test))]
fn garbo_history_file() -> Option<PathBuf> {
    std::env::var_os("GARBO_HISTORY")
        .map(PathBuf::from)
        .or_else(|| dirs::data_dir().map(|d| d.join("garbo").join("history.tsv")))
}

/// En las pruebas, nunca el historial real: las que aplican marcas de bloques anotan comandos.
#[cfg(test)]
fn garbo_history_file() -> Option<PathBuf> {
    Some(history_path_for_tests())
}

#[cfg(test)]
pub fn history_path_for_tests() -> PathBuf {
    std::env::temp_dir().join(format!("garbo-pruebas-{}.tsv", std::process::id()))
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Línea del historial propio: `segundos \t código \t carpeta \t comando`.
fn record_line(cmd: &str, code: Option<i32>, dir: Option<&Path>) -> String {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let code = code.map(|c| c.to_string()).unwrap_or_default();
    let dir = dir.and_then(Path::to_str).unwrap_or("");
    format!("{secs}\t{code}\t{dir}\t{cmd}\n")
}

fn parse_records(text: &str) -> Vec<Record> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.splitn(4, '\t');
            let (_secs, _code, dir, cmd) = (f.next()?, f.next()?, f.next()?, f.next()?);
            valid(cmd).then(|| Record {
                cmd: cmd.to_string(),
                dir: (!dir.is_empty()).then(|| PathBuf::from(dir)),
            })
        })
        .collect()
}

/// Lee el historial propio; si ha crecido demasiado, lo reescribe con la mitad más reciente.
fn load_records(path: &Path) -> Vec<Record> {
    let text = std::fs::read(path)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let recs = parse_records(&text);
    if recs.len() <= MAX_RECORDS {
        return recs;
    }
    let lines: Vec<&str> = text.lines().collect();
    let keep = lines[lines.len().saturating_sub(MAX_RECORDS / 2)..].join("\n") + "\n";
    let tmp = path.with_extension("tsv.tmp");
    if std::fs::write(&tmp, &keep).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
    parse_records(&keep)
}

impl History {
    fn refresh(&mut self) {
        let now = Instant::now();
        if self
            .checked
            .is_some_and(|t| now.duration_since(t) < RELOAD_EVERY)
        {
            return;
        }
        self.checked = Some(now);
        let mut changed = false;
        let shell = crate::shell::current();
        if let Some(path) = history_file(shell) {
            let m = mtime(&path);
            let seen = self
                .shell_file
                .as_ref()
                .is_some_and(|(p, t)| *p == path && *t == m);
            if !seen {
                self.shell = std::fs::read(&path)
                    .map(|b| parse_shell_history(shell, &b))
                    .unwrap_or_default();
                self.shell_file = Some((path, m));
                changed = true;
            }
        }
        if let Some(path) = garbo_history_file() {
            let m = mtime(&path);
            if !self.garbo_loaded || (m.is_some() && m != self.garbo_mtime) {
                self.garbo_loaded = true;
                self.garbo_mtime = m;
                self.garbo = load_records(&path);
                changed = true;
            }
        }
        if changed {
            self.rebuild();
        }
    }

    /// Une los dos historiales: el del shell primero y el propio después (es el más reciente: bash
    /// y zsh suelen escribir su archivo al salir).
    fn rebuild(&mut self) {
        let mut dirs: HashMap<&str, Vec<PathBuf>> = HashMap::new();
        for r in self.garbo.iter().rev() {
            if let Some(d) = &r.dir {
                let v = dirs.entry(r.cmd.as_str()).or_default();
                if v.len() < MAX_DIRS && !v.contains(d) {
                    v.push(d.clone());
                }
            }
        }
        let cmds = dedup(
            self.shell
                .iter()
                .cloned()
                .chain(self.garbo.iter().map(|r| r.cmd.clone())),
        );
        self.entries = cmds
            .into_iter()
            .map(|cmd| {
                let dirs = dirs.get(cmd.as_str()).cloned().unwrap_or_default();
                Entry { cmd, dirs }
            })
            .collect();
        self.generation += 1;
    }

    fn push(&mut self, cmd: &str, dir: Option<PathBuf>) {
        let mut dirs = match self.entries.iter().position(|e| e.cmd == cmd) {
            Some(i) => self.entries.remove(i).dirs,
            None => Vec::new(),
        };
        if let Some(d) = &dir {
            dirs.retain(|x| x != d);
            dirs.insert(0, d.clone());
            dirs.truncate(MAX_DIRS);
        }
        self.entries.push(Entry {
            cmd: cmd.to_string(),
            dirs,
        });
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
        self.garbo.push(Record {
            cmd: cmd.to_string(),
            dir,
        });
        self.generation += 1;
    }

    /// Resto del comando más reciente que empieza por `prefix` y pasa el filtro `ok`: primero los
    /// ejecutados en `cwd`; después cualquiera salvo los `cd` relativos (se hicieron en otro sitio).
    fn suggest(
        &self,
        prefix: &str,
        cwd: Option<&Path>,
        ok: impl Fn(&str) -> bool,
    ) -> Option<String> {
        let candidates = || {
            self.entries.iter().rev().filter(|e| {
                e.cmd.len() > prefix.len()
                    && e.cmd.starts_with(prefix)
                    && !e.cmd[prefix.len()..].trim().is_empty()
            })
        };
        let here = cwd
            .and_then(|cwd| candidates().find(|e| e.dirs.iter().any(|d| d == cwd) && ok(&e.cmd)));
        here.or_else(|| candidates().find(|e| !relative_cd(&e.cmd) && ok(&e.cmd)))
            .map(|e| e.cmd[prefix.len()..].to_string())
    }
}

/// ¿Cambia el comando a una carpeta relativa (`cd Proyectos`)? Su destino depende de dónde se
/// ejecutó. `cd`, `cd -`, `cd ..`, `cd ~/x`, `cd /x` y con variables valen en cualquier sitio.
fn relative_cd(cmd: &str) -> bool {
    let toks: Vec<&str> = cmd.split_whitespace().collect();
    toks.windows(2).any(|w| {
        let t = w[1].trim_end_matches([';', '&']).trim_matches(['\'', '"']);
        matches!(w[0], "cd" | "pushd")
            && !(t.is_empty()
                || t.starts_with(['-', '/', '~'])
                || t.contains('$')
                || t.split('/').all(|c| c.is_empty() || c == "." || c == ".."))
    })
}

struct Known {
    names: HashMap<String, Kind>,
    built: Instant,
}

static KNOWN: Mutex<Option<Known>> = Mutex::new(None);

/// Programas del PATH (y carpetas habituales del usuario), órdenes internas, y alias y funciones de
/// los archivos de arranque del shell en uso.
fn build_known() -> HashMap<String, Kind> {
    let shell = crate::shell::current();
    let builtins = match shell {
        Shell::Zsh => ZSH_BUILTINS,
        Shell::Fish => FISH_BUILTINS,
        Shell::Bash | Shell::Other => BUILTINS,
    };
    let mut names: HashMap<String, Kind> = builtins
        .iter()
        .map(|s| (s.to_string(), Kind::Builtin))
        .collect();
    let home = dirs::home_dir().unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend([
        home.join(".local/bin"),
        home.join("bin"),
        home.join(".cargo/bin"),
    ]);
    for d in dirs {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                names
                    .entry(e.file_name().to_string_lossy().into_owned())
                    .or_insert(Kind::Command);
            }
        }
    }
    let (files, rc_dirs): (Vec<PathBuf>, Vec<PathBuf>) = match shell {
        Shell::Zsh => {
            let z = std::env::var_os("ZDOTDIR").map_or(home.clone(), PathBuf::from);
            (
                [
                    ".zshrc",
                    ".zshenv",
                    ".zprofile",
                    ".zlogin",
                    ".zsh_aliases",
                    ".aliases",
                ]
                .iter()
                .map(|f| z.join(f))
                .chain(["/etc/zshrc", "/etc/zshenv", "/etc/zprofile"].map(PathBuf::from))
                .collect(),
                vec![z.join(".zshrc.d"), home.join(".oh-my-zsh/custom")],
            )
        }
        Shell::Fish => {
            let conf = dirs::config_dir().unwrap_or_default().join("fish");
            // Cada archivo de las carpetas de funciones define la función de su nombre.
            for d in [
                conf.join("functions"),
                PathBuf::from("/etc/fish/functions"),
                PathBuf::from("/usr/share/fish/functions"),
                PathBuf::from("/usr/share/fish/vendor_functions.d"),
            ] {
                for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                    let p = e.path();
                    if let (Some(stem), Some("fish")) = (
                        p.file_stem().and_then(|s| s.to_str()),
                        p.extension().and_then(|s| s.to_str()),
                    ) {
                        names.entry(stem.to_string()).or_insert(Kind::Alias);
                    }
                }
            }
            (
                vec![conf.join("config.fish")],
                vec![conf.join("conf.d"), PathBuf::from("/etc/fish/conf.d")],
            )
        }
        Shell::Bash | Shell::Other => (
            [".bashrc", ".bash_aliases", ".bash_profile", ".profile"]
                .iter()
                .map(|f| home.join(f))
                .chain([PathBuf::from("/etc/bashrc")])
                .collect(),
            vec![home.join(".bashrc.d"), PathBuf::from("/etc/profile.d")],
        ),
    };
    let mut rc = files;
    for dir in rc_dirs {
        if let Ok(rd) = std::fs::read_dir(dir) {
            rc.extend(rd.flatten().map(|e| e.path()));
        }
    }
    for f in rc {
        if let Ok(text) = std::fs::read_to_string(f) {
            for n in text.lines().filter_map(defined_name) {
                names.insert(n, Kind::Alias);
            }
        }
    }
    names
}

/// Órdenes que se pueden ejecutar (nombre -> tipo), rehechas como mucho cada minuto.
pub fn with_known<R>(f: impl FnOnce(&HashMap<String, Kind>) -> R) -> R {
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    if known
        .as_ref()
        .is_none_or(|k| k.built.elapsed() >= KNOWN_EVERY)
    {
        *known = Some(Known {
            names: build_known(),
            built: Instant::now(),
        });
    }
    f(&known.as_ref().unwrap().names)
}

/// Nombre que define una línea de un archivo de arranque: `alias x=…` (bash, zsh), `alias x …`
/// (fish), `abbr -a x …` (fish), `function x`, `x() {`.
fn defined_name(line: &str) -> Option<String> {
    let l = line.trim();
    let ident = |s: &str| {
        let n: String = s
            .chars()
            .take_while(|c| c.is_alphanumeric() || "_-.:".contains(*c))
            .collect();
        (!n.is_empty() && !n.starts_with('-')).then_some(n)
    };
    // Tras la palabra clave, el primer argumento que no es una opción (`alias -g`, `abbr --add`).
    let first_arg = |rest: &str| {
        rest.split_whitespace()
            .find(|w| !w.starts_with('-'))
            .and_then(|w| ident(w.split('=').next()?))
    };
    if let Some(rest) = l.strip_prefix("alias ") {
        return first_arg(rest);
    }
    if let Some(rest) = l.strip_prefix("abbr ") {
        // `abbr -a x …`; `abbr -e x` borra y `abbr --list` no define nada.
        let w: Vec<&str> = rest.split_whitespace().collect();
        if w.iter()
            .any(|o| matches!(*o, "-e" | "--erase" | "-l" | "--list" | "-s" | "--show"))
        {
            return None;
        }
        return first_arg(rest);
    }
    if let Some(rest) = l.strip_prefix("function ") {
        return ident(rest.trim());
    }
    let name = ident(l)?;
    l[name.len()..]
        .trim_start()
        .starts_with("()")
        .then_some(name)
}

/// Primera palabra que se ejecuta (saltando asignaciones `VAR=valor` y `(`, `{`, `!`).
fn first_word(cmd: &str) -> Option<&str> {
    cmd.split_whitespace()
        .map(|w| w.trim_start_matches(['(', '{', '!']))
        .find(|w| {
            let assign = w.split_once('=').is_some_and(|(n, _)| {
                !n.is_empty()
                    && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && !n.starts_with(|c: char| c.is_ascii_digit())
            });
            !w.is_empty() && !assign
        })
}

fn expand(p: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let p = p.trim_matches(['\'', '"']);
    if let Some(rest) = p.strip_prefix("~") {
        return Some(dirs::home_dir()?.join(rest.trim_start_matches('/')));
    }
    let path = Path::new(p);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        cwd.map(|c| c.join(path))
    }
}

/// ¿Tiene sentido sugerir este comando?
fn runnable(cmd: &str, known: &HashMap<String, Kind>, cwd: Option<&Path>) -> bool {
    let Some(w) = first_word(cmd) else {
        return false;
    };
    if w.contains('/') {
        return expand(w, cwd).is_none_or(|p| p.exists());
    }
    if !known.contains_key(w) {
        return false;
    }
    if w == "cd" {
        // El destino tiene que existir desde aquí (sin destino, "-" o con variables: vale).
        let target = cmd.split_whitespace().skip_while(|t| *t != "cd").nth(1);
        return match target.map(|t| t.trim_end_matches([';', '&'])) {
            None | Some("-") | Some("") => true,
            Some(t) if t.contains('$') => true,
            Some(t) => expand(t, cwd).is_none_or(|p| p.is_dir()),
        };
    }
    true
}

/// Las marcas de tiempo de bash (`HISTTIMEFORMAT`) son líneas `#1700000000`.
fn is_timestamp(l: &str) -> bool {
    l.len() > 1 && l.starts_with('#') && l[1..].bytes().all(|b| b.is_ascii_digit())
}

fn valid(cmd: &str) -> bool {
    !cmd.trim().is_empty() && !cmd.chars().any(char::is_control)
}

/// Quita repetidos conservando la aparición más reciente, y limita el tamaño.
fn dedup(items: impl DoubleEndedIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out: Vec<String> = items
        .rev()
        .filter(|e| valid(e) && seen.insert(e.clone()))
        .take(MAX_ENTRIES)
        .collect();
    out.reverse();
    out
}

/// Anota un comando ejecutado en `dir` con su código de salida, en memoria y en el historial
/// propio. No se guardan los que empiezan por espacio (convención de bash) ni los que no se
/// pudieron ejecutar (126/127: no existe o no es ejecutable).
pub fn record(cmd: &str, code: Option<i32>, dir: Option<&Path>) {
    let cmd = cmd.trim_end();
    if cmd.starts_with(' ')
        || !valid(cmd)
        || cmd.chars().count() > 500
        || matches!(code, Some(126 | 127))
    {
        return;
    }
    // Una carpeta con tabuladores o saltos de línea rompería el formato: se guarda sin ella.
    let dir = dir.filter(|d| d.to_str().is_some_and(|s| !s.contains(['\t', '\n'])));
    let path = garbo_history_file();
    if let Some(path) = &path {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = f.write_all(record_line(cmd, code, dir).as_bytes());
        }
    }
    // Sin releer los archivos: esto se llama desde el hilo del PTY con el terminal bloqueado.
    let mut h = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    let h = h.get_or_insert_with(History::default);
    h.push(cmd, dir.map(Path::to_path_buf));
    if h.garbo_loaded {
        // Lo acabamos de escribir nosotros: no hace falta releerlo.
        h.garbo_mtime = path.as_deref().and_then(mtime);
    }
}

struct Cached {
    input: String,
    cwd: Option<PathBuf>,
    generation: u64,
    at: Instant,
    result: Option<String>,
}

static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

/// Resto del comando sugerido para lo escrito hasta ahora (`cwd` = directorio de la pestaña).
pub fn suggest(input: &str, cwd: Option<&Path>) -> Option<String> {
    if input.trim().chars().count() < MIN_PREFIX {
        return None;
    }
    let generation = {
        let mut h = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
        let h = h.get_or_insert_with(History::default);
        h.refresh();
        h.generation
    };
    if let Some(c) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if c.input == input
            && c.cwd.as_deref() == cwd
            && c.generation == generation
            && c.at.elapsed() < CACHE_FOR
        {
            return c.result.clone();
        }
    }
    // Orden de bloqueo: KNOWN y luego HISTORY (nunca al revés).
    let from_history = with_known(|names| {
        let h = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
        h.as_ref()?
            .suggest(input, cwd, |cmd| runnable(cmd, names, cwd))
    });
    let result = from_history.or_else(|| crate::complete::complete(input, cwd)?.ghost());
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Cached {
        input: input.to_string(),
        cwd: cwd.map(Path::to_path_buf),
        generation,
        at: Instant::now(),
        result: result.clone(),
    });
    result
}

/// Primera palabra de una sugerencia (Ctrl+→): los espacios iniciales y hasta el siguiente
/// espacio, o hasta una "/" incluida (" --resume" -> " --resume"; "umentos/Proyectos" -> "umentos/").
pub fn next_word(rest: &str) -> &str {
    let lead = rest.len() - rest.trim_start().len();
    let body = &rest[lead..];
    let end = body.find(|c: char| c.is_whitespace()).unwrap_or(body.len());
    let end = body[..end].find('/').map_or(end, |i| i + 1);
    &rest[..lead + end]
}

/// Posición absoluta (línea contando el historial, columna) del cursor: estable aunque la
/// pantalla se desplace mientras se escribe.
pub fn cursor_mark<T>(term: &Term<T>) -> (i64, usize) {
    let p = term.grid().cursor.point;
    (p.line.0 as i64 + term.history_size() as i64, p.column.0)
}

fn cell_char(c: char) -> char {
    if c == '\0' {
        ' '
    } else {
        c
    }
}

/// Texto escrito desde `start` hasta el cursor. `None` si el cursor no está al final de lo escrito
/// o la entrada ocupa varias líneas lógicas.
pub fn current_input<T>(term: &Term<T>, start: (i64, usize)) -> Option<String> {
    let cur = term.grid().cursor.point;
    let start_line = Line((start.0 - term.history_size() as i64) as i32);
    if start_line > cur.line || start_line < term.topmost_line() {
        return None;
    }
    let cols = term.columns();
    if !prompt_before(term, start_line, start.1) {
        return None;
    }
    let mut s = String::new();
    let mut l = start_line;
    loop {
        let row = &term.grid()[l];
        let from = if l == start_line { start.1 } else { 0 };
        let to = if l == cur.line { cur.column.0 } else { cols };
        for c in from..to.min(cols) {
            let cell = &row[Column(c)];
            if !cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                s.push(cell_char(cell.c));
            }
        }
        if l == cur.line {
            break;
        }
        if !row[Column(cols - 1)].flags.contains(Flags::WRAPLINE) {
            return None;
        }
        l += 1;
    }
    // Detrás del cursor no puede haber nada escrito (el prompt derecho no cuenta).
    let row = &term.grid()[cur.line];
    let written = |c: usize| {
        let cell = &row[Column(c)];
        cell_char(cell.c) != ' ' && !crate::blocks::is_right_prompt(cell)
    };
    if (cur.column.0..cols).any(written) || readline_mode(&s) {
        return None;
    }
    Some(s)
}

/// ¿Sigue estando el prompt justo antes de lo escrito? Readline lo sobrescribe en la búsqueda con
/// Ctrl+R ("(reverse-i-search)…"): entonces lo que hay no es la línea de órdenes. Si lo escrito
/// empieza en la primera columna (prompt de Garbo, o un prompt que llena la línea), el prompt está
/// en la línea de encima.
pub(crate) fn prompt_before<T>(term: &Term<T>, line: Line, col: usize) -> bool {
    let is_prompt = |l: Line, c: usize| {
        matches!(
            crate::blocks::cell_link(&term.grid()[l][Column(c)]),
            Some((b'p', _))
        )
    };
    if col > 0 {
        is_prompt(line, col - 1)
    } else if line > term.topmost_line() {
        (0..term.columns()).any(|c| is_prompt(line - 1, c))
    } else {
        true
    }
}

/// Lo que readline muestra en lugar de la línea durante una búsqueda o un argumento numérico. Con
/// el prompt de Garbo lo escribe desde la primera columna, sin tocar el prompt.
pub(crate) fn readline_mode(text: &str) -> bool {
    [
        "(reverse-i-search)",
        "(i-search)",
        "(failed reverse-i-search)",
        "(failed i-search)",
        "(arg: ",
    ]
    .iter()
    .any(|p| text.starts_with(p))
}

/// Comando completo recién aceptado (el cursor ya bajó de línea al pulsar Intro).
pub fn executed_command<T>(term: &Term<T>, start: (i64, usize)) -> Option<String> {
    let cur = term.grid().cursor.point;
    let start_line = Line((start.0 - term.history_size() as i64) as i32);
    if start_line > cur.line || start_line < term.topmost_line() {
        return None;
    }
    let cols = term.columns();
    let last = if cur.column.0 == 0 && cur.line > start_line {
        cur.line - 1
    } else {
        cur.line
    };
    let mut s = String::new();
    let mut l = start_line;
    loop {
        let row = &term.grid()[l];
        let from = if l == start_line { start.1 } else { 0 };
        let mut line = String::new();
        for c in from..cols {
            let cell = &row[Column(c)];
            if crate::blocks::is_right_prompt(cell) {
                line.push(' ');
            } else if !cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                line.push(cell_char(cell.c));
            }
        }
        let wraps = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        if wraps {
            s.push_str(&line);
        } else {
            s.push_str(line.trim_end());
        }
        if l >= last {
            break;
        }
        if !wraps {
            return None; // varias líneas lógicas (heredoc, continuación): no se guarda
        }
        l += 1;
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lee_el_historial_de_zsh() {
        // Con EXTENDED_HISTORY, sin él, un comando de dos líneas y "ñ€" con el byte 0x82 de "€"
        // guardado como 0x83 0xA2 (metafied).
        let mut bytes = b": 1700000000:0;git status\nls -la\n: 1700000001:3;for i in 1 2; do\\\necho $i; done\n: 1700000002:0;echo \xc3\xb1\xe2".to_vec();
        bytes.extend_from_slice(&[0x83, 0x82 ^ 0x20, 0xac, b'\n']);
        bytes.extend_from_slice(b": no es extendido;x\n");
        assert_eq!(
            parse_zsh_history(&bytes),
            ["git status", "ls -la", "echo ñ€", ": no es extendido;x"]
        );
    }

    #[test]
    fn lee_el_historial_de_fish() {
        let text = "- cmd: cargo build\n  when: 1700000000\n- cmd: echo a\\\\b\n  when: 1\n  paths:\n    - b\n- cmd: echo uno\\necho dos\n  when: 2\n";
        assert_eq!(parse_fish_history(text), ["cargo build", "echo a\\b"]);
    }

    #[test]
    fn nombres_definidos_en_los_archivos_de_arranque() {
        let names: Vec<Option<String>> = [
            "alias ll='ls -l'",
            "alias -- gs='git status'",
            "alias -g G='| grep'",
            "alias la 'ls -a'",
            "abbr -a gco git checkout",
            "abbr --add --global ga git add",
            "abbr -e gco",
            "function mkcd --description 'crea y entra'",
            "mkcd() {",
            "export PATH=x",
        ]
        .iter()
        .map(|l| defined_name(l))
        .collect();
        let want = [
            Some("ll"),
            Some("gs"),
            Some("G"),
            Some("la"),
            Some("gco"),
            Some("ga"),
            None,
            Some("mkcd"),
            Some("mkcd"),
            None,
        ];
        assert_eq!(names, want.map(|n| n.map(str::to_string)));
    }

    fn history(bash: &[&str], garbo: &[(&str, Option<&str>)]) -> History {
        let mut h = History {
            shell: bash
                .iter()
                .filter(|l| !is_timestamp(l))
                .map(|s| s.to_string())
                .collect(),
            garbo: garbo
                .iter()
                .map(|(c, d)| Record {
                    cmd: c.to_string(),
                    dir: d.map(PathBuf::from),
                })
                .collect(),
            ..Default::default()
        };
        h.rebuild();
        h
    }

    #[test]
    fn historial_y_sugerencias() {
        let mut h = history(
            &[
                "git status",
                "cargo build",
                "git commit -m x",
                "cargo build",
                "#1700000000",
                "ls",
            ],
            &[],
        );
        let cmds: Vec<&str> = h.entries.iter().map(|e| e.cmd.as_str()).collect();
        assert_eq!(cmds, ["git status", "git commit -m x", "cargo build", "ls"]);
        let all = |_: &str| true;
        assert_eq!(
            h.suggest("git", None, all).as_deref(),
            Some(" commit -m x"),
            "la más reciente gana"
        );
        assert_eq!(h.suggest("git s", None, all).as_deref(), Some("tatus"));
        assert_eq!(h.suggest("ls", None, all), None, "igual no es sugerencia");
        h.push("git status", None);
        assert_eq!(
            h.suggest("git", None, all).as_deref(),
            Some(" status"),
            "ejecutarlo lo vuelve más reciente"
        );
        // Con filtro: se salta lo que no pasa y se ofrece el siguiente más reciente.
        assert_eq!(
            h.suggest("git", None, |c| !c.contains("status")).as_deref(),
            Some(" commit -m x")
        );
        assert!(!valid("   "));
        assert!(!valid("a\u{1b}b"));
    }

    #[test]
    fn primero_lo_ejecutado_en_esta_carpeta() {
        let home = Path::new("/home/u");
        let docs = Path::new("/home/u/Documentos");
        let mut h = history(
            &["cd Documentos", "cd Proyectos", "cd ..", "claude --resume"],
            &[
                ("cd Documentos", Some("/home/u")),
                ("cargo test", Some("/home/u/p")),
                ("cargo build", Some("/home/u/q")),
            ],
        );
        let all = |_: &str| true;
        // El caso de la captura: en ~, "cd Proyectos" (hecho en ~/Documentos) no se sugiere.
        assert_eq!(
            h.suggest("cd ", Some(home), all).as_deref(),
            Some("Documentos"),
            "lo hecho aquí"
        );
        assert_eq!(
            h.suggest("cd P", Some(home), all),
            None,
            "cd relativo hecho en otro sitio"
        );
        assert_eq!(
            h.suggest("cd ", Some(docs), all).as_deref(),
            Some(".."),
            "cd .. vale en cualquier sitio"
        );
        h.push("cd Proyectos", Some(docs.to_path_buf()));
        assert_eq!(
            h.suggest("cd ", Some(docs), all).as_deref(),
            Some("Proyectos")
        );
        assert_eq!(h.suggest("cd P", Some(home), all), None);
        // Fuera de las carpetas conocidas gana el más reciente; dentro, el de esa carpeta.
        assert_eq!(
            h.suggest("cargo ", Some(home), all).as_deref(),
            Some("build")
        );
        assert_eq!(
            h.suggest("cargo ", Some(Path::new("/home/u/p")), all)
                .as_deref(),
            Some("test")
        );
        assert!(relative_cd("cd GRBTER && claude"));
        assert!(!relative_cd("cd ~/x"));
        assert!(!relative_cd("cd ../.."));
        assert!(!relative_cd("cd \"$X\""));
        assert!(!relative_cd("git status"));
    }

    #[test]
    fn historial_propio_en_archivo() {
        let line = record_line(
            "cd Proyectos",
            Some(0),
            Some(Path::new("/home/u/Documentos")),
        );
        assert!(
            line.ends_with("\t0\t/home/u/Documentos\tcd Proyectos\n"),
            "{line}"
        );
        let recs = parse_records(&(line + "basura\n1\t\t\tls -la\n"));
        assert_eq!(recs.len(), 2);
        assert_eq!(
            recs[0].dir.as_deref(),
            Some(Path::new("/home/u/Documentos"))
        );
        assert_eq!(
            (recs[1].cmd.as_str(), recs[1].dir.as_deref()),
            ("ls -la", None)
        );
        assert_eq!(next_word(" --resume"), " --resume");
        assert_eq!(next_word("umentos/Proyectos"), "umentos/");
        assert_eq!(next_word("tatus --short"), "tatus");
    }

    #[test]
    fn solo_comandos_que_se_pueden_ejecutar() {
        let known: HashMap<String, Kind> = ["ls", "git", "cd", "servidor", "sudo"]
            .iter()
            .map(|s| (s.to_string(), Kind::Command))
            .collect();
        let tmp = std::env::temp_dir();
        let cwd = Some(tmp.as_path());
        assert!(runnable("git status", &known, cwd));
        assert!(runnable("servidor", &known, cwd), "alias del usuario");
        assert!(
            runnable("FOO=1 git log", &known, cwd),
            "con asignaciones delante"
        );
        assert!(!runnable("calude", &known, cwd), "errata");
        assert!(!runnable("cd..", &known, cwd), "errata sin espacio");
        assert!(runnable("cd", &known, cwd));
        assert!(runnable("cd -", &known, cwd));
        assert!(runnable("cd /", &known, cwd));
        assert!(
            !runnable("cd /no/existe/garbo", &known, cwd),
            "destino inexistente"
        );
        assert!(runnable("/bin/sh -c x", &known, cwd));
        assert!(!runnable("/no/existe/x", &known, cwd));
        assert_eq!(
            defined_name("alias servidor='ssh x'").as_deref(),
            Some("servidor")
        );
        assert_eq!(defined_name("  function mkcd {").as_deref(), Some("mkcd"));
        assert_eq!(defined_name("mkcd() {").as_deref(), Some("mkcd"));
        assert_eq!(defined_name("export PATH=x"), None);
        assert_eq!(defined_name("echo hola"), None);
    }
}
