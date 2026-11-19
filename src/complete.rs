//! Completado al estilo Warp: Tab abre un menú con lo que puede ir en la palabra que se está
//! escribiendo, según el contexto:
//!   - primera palabra: órdenes (programas del PATH, órdenes internas, alias y funciones);
//!   - tras `cd`, `pushd` o `rmdir`: solo carpetas;
//!   - `git`: subórdenes, alias de git, ramas y remotos;
//!   - `cargo`, `npm`/`pnpm`/`yarn`/`bun`, `dnf`, `systemctl`: subórdenes (y scripts de package.json);
//!   - `make`: objetivos del Makefile; `ssh`/`scp`/`rsync`: equipos de `~/.ssh/config`;
//!   - lo demás: archivos y carpetas.
//!
//! La búsqueda no distingue mayúsculas ni tildes; desde dos caracteres, si nada empieza igual ni
//! contiene lo escrito, es difusa (letras en orden: "dent" -> "dashboard_entrenamiento"). Si Garbo no tiene nada que ofrecer (opciones `-x`, paquetes de dnf…), Tab llega a bash,
//! que completa como siempre.
//!
//! Lo elegido sustituye a la palabra escribiéndolo en el shell como si se tecleara: si amplía lo
//! escrito solo se manda lo que falta; si no, se borra la palabra (retroceso) y se escribe entera.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Filas visibles del menú.
pub const MENU_ROWS: usize = 8;
/// Candidatos como máximo.
const MAX_ITEMS: usize = 400;
/// Entradas leídas como máximo de una carpeta (carpetas enormes).
const MAX_DIR_ENTRIES: usize = 5000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Dir,
    File,
    Exec,
    Command,
    Alias,
    Builtin,
    Sub,
    Branch,
    RemoteBranch,
    Remote,
    Script,
    Target,
    Host,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Dir => "carpeta",
            Kind::File => "archivo",
            Kind::Exec => "ejecutable",
            Kind::Command => "orden",
            Kind::Alias => "alias",
            Kind::Builtin => "interna",
            Kind::Sub => "suborden",
            Kind::Branch => "rama",
            Kind::RemoteBranch => "rama remota",
            Kind::Remote => "remoto",
            Kind::Script => "script",
            Kind::Target => "objetivo",
            Kind::Host => "equipo",
        }
    }

    /// Color ANSI con el que se marca en el menú (azul = carpeta, verde = ejecutable…).
    pub fn ansi(self) -> usize {
        match self {
            Kind::Dir | Kind::Host => 4,
            Kind::File => 7,
            Kind::Exec | Kind::Script => 2,
            Kind::Command | Kind::Target => 3,
            Kind::Alias | Kind::Branch | Kind::RemoteBranch => 5,
            Kind::Builtin | Kind::Sub => 6,
            Kind::Remote => 1,
        }
    }

    /// Orden entre tipos cuando la coincidencia es igual de buena: carpetas primero, alias y ramas
    /// remotas al final.
    fn group(self) -> u8 {
        match self {
            Kind::Dir => 0,
            Kind::Alias | Kind::RemoteBranch => 2,
            _ => 1,
        }
    }
}

/// Candidato antes de compararlo con lo escrito.
struct Cand {
    /// Con qué se compara lo escrito (el nombre, sin la carpeta).
    key: String,
    label: String,
    insert: String,
    kind: Kind,
    detail: Cow<'static, str>,
    /// Bytes iniciales de lo escrito que no cuentan para comparar (`carpeta/`, `usuario@`).
    skip: usize,
}

#[derive(Clone, Debug)]
pub struct Item {
    /// Lo que se ve (sin escapar; las carpetas con "/" final).
    pub label: String,
    /// Lo que sustituye a la palabra (escapado para bash, con "/" o espacio final).
    pub insert: String,
    pub kind: Kind,
    /// Descripción a la derecha (tipo, o qué hace la suborden).
    pub detail: Cow<'static, str>,
    /// Posiciones (en caracteres de `label`) que coinciden con lo escrito.
    pub hits: Vec<usize>,
    /// 3 = empieza igual, 2 = igual sin mayúsculas ni tildes, 1 = lo contiene, 0 = letras en orden.
    pub tier: u8,
    score: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Completions {
    /// Dónde empieza la palabra (en caracteres de la entrada).
    pub start: usize,
    /// La palabra tal cual está escrita (con comillas y escapes).
    pub word: String,
    pub items: Vec<Item>,
}

impl Completions {
    /// Bytes que escriben el candidato `i` en lugar de la palabra.
    pub fn accept_bytes(&self, i: usize) -> Option<Vec<u8>> {
        replace(&self.word, &self.items.get(i)?.insert)
    }

    /// Resto de la palabra si un único candidato empieza exactamente por lo escrito (para la
    /// sugerencia en gris).
    pub fn ghost(&self) -> Option<String> {
        if self.word.is_empty() {
            return None;
        }
        let mut exact = self.items.iter().filter(|i| i.tier == 3);
        let only = exact.next()?;
        if exact.next().is_some() {
            return None;
        }
        let rest = only.insert.strip_prefix(self.word.as_str())?;
        (!rest.trim().is_empty() && !has_control(rest)).then(|| rest.to_string())
    }

    /// Si todos los candidatos empiezan por lo escrito y comparten algo más, lo que falta hasta ahí
    /// (como hace bash con el primer Tab).
    pub fn common_extension(&self) -> Option<Vec<u8>> {
        let first = &self.items.first()?.insert;
        if self.items.iter().any(|i| i.tier != 3) {
            return None;
        }
        let mut n = first.len();
        for it in &self.items[1..] {
            n = n.min(common_prefix(first, &it.insert));
        }
        let mut lcp = &first[..n];
        // No dejar un escape a medias ("Mi\").
        if lcp.chars().rev().take_while(|&c| c == '\\').count() % 2 == 1 {
            lcp = &lcp[..lcp.len() - 1];
        }
        let rest = lcp.strip_prefix(self.word.as_str())?;
        (!rest.is_empty() && !has_control(rest)).then(|| rest.as_bytes().to_vec())
    }

    /// ¿Merece abrirse el menú solo, mientras se escribe? (opción "Menú al escribir").
    pub fn opens_as_you_type(&self) -> bool {
        !self.word.is_empty()
            && !self.items.is_empty()
            && !(self.items.len() == 1 && self.items[0].insert.trim_end() == self.word)
    }
}

/// Longitud en bytes (en límite de carácter) del prefijo común.
fn common_prefix(a: &str, b: &str) -> usize {
    a.char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map_or(a.len().min(b.len()), |((i, _), _)| i)
}

/// ¿Tiene caracteres de control? Escritos en el PTY, un salto de línea equivale a Intro y un ESC
/// dispara atajos de readline: un archivo llamado "x\nrm -rf ~" ejecutaría la segunda línea.
fn has_control(s: &str) -> bool {
    s.chars().any(char::is_control)
}

/// Bytes para cambiar `word` por `insert` en la línea de readline. Nada si `insert` lleva
/// caracteres de control (los únicos que se mandan son los retrocesos propios).
fn replace(word: &str, insert: &str) -> Option<Vec<u8>> {
    if has_control(insert) {
        return None;
    }
    if let Some(rest) = insert.strip_prefix(word) {
        return Some(rest.as_bytes().to_vec());
    }
    let mut v = vec![0x7f; word.chars().count()];
    v.extend_from_slice(insert.as_bytes());
    Some(v)
}

// ---------- Lectura de la línea ----------

#[derive(Default, Debug)]
struct Lexed {
    /// Palabras ya terminadas de la orden simple actual (sin comillas ni escapes).
    words: Vec<String>,
    /// La palabra actual: dónde empieza (en caracteres), tal cual y su valor.
    start: usize,
    raw: String,
    value: String,
    /// La palabra actual sigue a una redirección (`>`, `<`): es un archivo.
    redirect: bool,
    /// Tiene variables o sustituciones (`$`, `` ` ``): no se completa.
    dynamic: bool,
}

struct Lexer {
    out: Lexed,
    in_word: bool,
    word_redirect: bool,
    redirect: bool,
}

impl Lexer {
    fn begin(&mut self, i: usize) {
        if !self.in_word {
            self.in_word = true;
            self.out.start = i;
            self.word_redirect = self.redirect;
            self.redirect = false;
        }
    }

    fn finish(&mut self) {
        if !self.in_word {
            return;
        }
        let v = std::mem::take(&mut self.out.value);
        if !self.word_redirect {
            self.out.words.push(v);
        }
        self.out.raw.clear();
        self.out.dynamic = false;
        self.in_word = false;
        self.word_redirect = false;
    }

    /// `;`, `|`, `&`, `(`…: empieza otra orden.
    fn boundary(&mut self) {
        self.finish();
        self.out.words.clear();
        self.redirect = false;
    }
}

fn lex(input: &str) -> Lexed {
    let chars: Vec<char> = input.chars().collect();
    let mut lx = Lexer {
        out: Lexed::default(),
        in_word: false,
        word_redirect: false,
        redirect: false,
    };
    let (mut sq, mut dq, mut esc) = (false, false, false);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if esc {
            esc = false;
            lx.out.raw.push(c);
            lx.out.value.push(c);
        } else if sq {
            lx.out.raw.push(c);
            if c == '\'' {
                sq = false;
            } else {
                lx.out.value.push(c);
            }
        } else if dq {
            lx.out.raw.push(c);
            match c {
                '"' => dq = false,
                '\\' => esc = true,
                '$' | '`' => {
                    lx.out.dynamic = true;
                    lx.out.value.push(c);
                }
                _ => lx.out.value.push(c),
            }
        } else {
            match c {
                ' ' | '\t' => lx.finish(),
                ';' | '|' | '&' | '(' | ')' | '`' => lx.boundary(),
                '<' | '>' => {
                    lx.finish();
                    lx.redirect = true;
                }
                '$' if chars.get(i + 1) == Some(&'(') => {
                    lx.boundary();
                    i += 2;
                    continue;
                }
                _ => {
                    lx.begin(i);
                    lx.out.raw.push(c);
                    match c {
                        '\\' => esc = true,
                        '\'' => sq = true,
                        '"' => dq = true,
                        '$' => {
                            lx.out.dynamic = true;
                            lx.out.value.push(c);
                        }
                        _ => lx.out.value.push(c),
                    }
                }
            }
        }
        i += 1;
    }
    if !lx.in_word {
        lx.out.start = chars.len();
        lx.out.raw.clear();
        lx.out.value.clear();
        lx.out.dynamic = false;
        lx.word_redirect = lx.redirect;
    }
    lx.out.redirect = lx.word_redirect;
    lx.out
}

fn is_assignment(w: &str) -> bool {
    w.split_once('=').is_some_and(|(n, _)| {
        !n.is_empty()
            && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !n.starts_with(|c: char| c.is_ascii_digit())
    })
}

/// Órdenes que ejecutan la palabra siguiente.
const WRAPPERS: &[&str] = &[
    "sudo", "doas", "pkexec", "time", "nohup", "exec", "command", "builtin", "nice", "env",
    "watch", "xargs", "strace", "ltrace", "stdbuf", "timeout", "unbuffer",
];

/// La orden de la línea y sus argumentos ya escritos, saltando asignaciones y envoltorios
/// (`sudo -u x`, `env A=1`, `time`…). `None` = la palabra actual es la orden.
fn split_command(words: &[String]) -> Option<(&str, &[String])> {
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        if is_assignment(w) {
            i += 1;
            continue;
        }
        if WRAPPERS.contains(&w) {
            i += 1;
            while let Some(a) = words.get(i) {
                if matches!(w, "sudo" | "doas")
                    && matches!(
                        a.as_str(),
                        "-u" | "-g" | "-C" | "-D" | "-h" | "-p" | "-r" | "-t" | "-U"
                    )
                {
                    i += 2;
                } else if a.starts_with('-') || (w == "env" && is_assignment(a)) {
                    i += 1;
                } else {
                    break;
                }
            }
            if w == "timeout" && i < words.len() {
                i += 1; // la duración
            }
            continue;
        }
        return Some((w, &words[i + 1..]));
    }
    None
}

// ---------- Candidatos ----------

/// Candidatos para lo escrito hasta el cursor. `None` si no se sabe completar esa palabra (tiene
/// variables, o está vacía en la posición de orden).
pub fn complete(input: &str, cwd: Option<&Path>) -> Option<Completions> {
    let lx = lex(input);
    if lx.dynamic {
        return None;
    }
    let q = lx.value.as_str();
    let mut cands = Vec::new();
    if lx.redirect {
        paths(q, cwd, Filter::All, &mut cands);
    } else {
        match split_command(&lx.words) {
            None if q.contains('/') || q.starts_with('.') || q.starts_with('~') => {
                paths(q, cwd, Filter::Exec, &mut cands)
            }
            None if q.is_empty() => return None,
            None => commands(&mut cands),
            Some(_) if q.starts_with('-') => {}
            Some((cmd, args)) => arguments(cmd, args, q, cwd, &mut cands),
        }
    }
    Some(Completions {
        start: lx.start,
        word: lx.raw,
        items: rank(q, cands),
    })
}

fn arguments(cmd: &str, args: &[String], q: &str, cwd: Option<&Path>, out: &mut Vec<Cand>) {
    let name = cmd.rsplit('/').next().unwrap_or(cmd);
    let positional = args.iter().filter(|a| !a.starts_with('-')).count();
    match name {
        "cd" | "pushd" | "rmdir" => paths(q, cwd, Filter::Dirs, out),
        "git" => git(args, q, cwd, out),
        "cargo" if positional == 0 => cargo(out),
        "cargo" => {}
        "npm" | "pnpm" | "yarn" | "bun" => js(name, args, cwd, out),
        "make" | "gmake" => make_targets(cwd, out),
        "ssh" | "mosh" | "sftp" => hosts(q, " ", out),
        "scp" | "rsync" => {
            hosts(q, ":", out);
            paths(q, cwd, Filter::All, out);
        }
        "dnf" | "dnf5" | "yum" if positional == 0 => list(DNF, out),
        "systemctl" if positional == 0 => list(SYSTEMCTL, out),
        "dnf" | "dnf5" | "yum" | "systemctl" => {}
        "which" | "type" | "man" | "whereis" | "whatis" => commands(out),
        _ => paths(q, cwd, Filter::All, out),
    }
}

/// Escapa un nombre para bash (`Mi carpeta` -> `Mi\ carpeta`).
fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if " \t'\"\\$`&;|()<>*?[]!#{}".contains(c) || (i == 0 && c == '~') {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

fn expand(p: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    if p == "~" || p.starts_with("~/") {
        return Some(dirs::home_dir()?.join(p[1..].trim_start_matches('/')));
    }
    let path = Path::new(p);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        cwd.map(|c| c.join(path))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Dirs,
    /// Carpetas y ejecutables (para la primera palabra: `./script`).
    Exec,
}

fn paths(q: &str, cwd: Option<&Path>, filter: Filter, out: &mut Vec<Cand>) {
    use std::os::unix::fs::PermissionsExt;
    if q.starts_with('~') && !q.contains('/') {
        let label = "~/".to_string();
        out.push(Cand {
            key: "~".into(),
            insert: label.clone(),
            label,
            kind: Kind::Dir,
            detail: "carpeta personal".into(),
            skip: 0,
        });
        return;
    }
    let cut = q.rfind('/').map_or(0, |i| i + 1);
    let (dir_part, base) = q.split_at(cut);
    let Some(dir) = (if dir_part.is_empty() {
        cwd.map(Path::to_path_buf)
    } else {
        expand(dir_part, cwd)
    }) else {
        return;
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    let prefix = match dir_part.strip_prefix("~/") {
        Some(rest) => format!("~/{}", escape(rest)),
        None => escape(dir_part),
    };
    let hidden = base.starts_with('.');
    for e in rd.flatten().take(MAX_DIR_ENTRIES) {
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') && !hidden {
            continue;
        }
        // Los enlaces cuentan como aquello a lo que apuntan.
        let Ok(meta) = std::fs::metadata(e.path()) else {
            continue;
        };
        let is_dir = meta.is_dir();
        let exec = !is_dir && meta.permissions().mode() & 0o111 != 0;
        match filter {
            Filter::Dirs if !is_dir => continue,
            Filter::Exec if !is_dir && !exec => continue,
            _ => {}
        }
        let kind = if is_dir {
            Kind::Dir
        } else if exec {
            Kind::Exec
        } else {
            Kind::File
        };
        let (label, end) = if is_dir {
            (format!("{name}/"), "/")
        } else {
            (name.clone(), " ")
        };
        out.push(Cand {
            insert: format!("{prefix}{}{end}", escape(&name)),
            key: name,
            label,
            kind,
            detail: kind.name().into(),
            skip: cut,
        });
    }
}

fn commands(out: &mut Vec<Cand>) {
    crate::suggest::with_known(|known| {
        out.extend(known.iter().map(|(name, &kind)| Cand {
            key: name.clone(),
            label: name.clone(),
            insert: format!("{} ", escape(name)),
            kind,
            detail: kind.name().into(),
            skip: 0,
        }));
    });
}

fn list(items: &[(&'static str, &'static str)], out: &mut Vec<Cand>) {
    out.extend(items.iter().map(|&(k, d)| Cand {
        key: k.into(),
        label: k.into(),
        insert: format!("{k} "),
        kind: Kind::Sub,
        detail: d.into(),
        skip: 0,
    }));
}

fn simple(name: &str, kind: Kind, detail: Cow<'static, str>) -> Cand {
    Cand {
        key: name.into(),
        label: name.into(),
        insert: format!("{} ", escape(name)),
        kind,
        detail,
        skip: 0,
    }
}

// ---------- git ----------

const GIT: &[(&str, &str)] = &[
    ("add", "Añade cambios al índice"),
    ("bisect", "Busca el commit que introdujo un fallo"),
    ("blame", "Quién cambió cada línea"),
    ("branch", "Lista, crea o borra ramas"),
    ("checkout", "Cambia de rama o restaura archivos"),
    ("cherry-pick", "Aplica los cambios de otros commits"),
    ("clean", "Borra archivos sin seguimiento"),
    ("clone", "Clona un repositorio"),
    ("commit", "Graba los cambios"),
    ("config", "Lee o cambia la configuración"),
    ("diff", "Muestra diferencias"),
    ("fetch", "Descarga de un remoto"),
    ("grep", "Busca texto en los archivos"),
    ("init", "Crea un repositorio"),
    ("log", "Historial de commits"),
    ("merge", "Une otra rama"),
    ("mv", "Mueve o renombra"),
    ("pull", "Descarga e integra cambios"),
    ("push", "Sube cambios a un remoto"),
    ("rebase", "Reaplica commits sobre otra base"),
    ("reflog", "Historial de referencias"),
    ("remote", "Gestiona los remotos"),
    ("reset", "Mueve HEAD a otro estado"),
    ("restore", "Restaura archivos"),
    ("revert", "Revierte commits"),
    ("rm", "Borra archivos del índice"),
    ("show", "Muestra un commit u objeto"),
    ("stash", "Aparta los cambios"),
    ("status", "Estado del árbol de trabajo"),
    ("switch", "Cambia de rama"),
    ("tag", "Crea, lista o borra etiquetas"),
    ("worktree", "Gestiona árboles de trabajo"),
];
const GIT_REMOTE: &[(&str, &str)] = &[
    ("add", "Añade un remoto"),
    ("remove", "Quita un remoto"),
    ("rename", "Renombra un remoto"),
    ("set-url", "Cambia la URL"),
    ("show", "Información de un remoto"),
];
const GIT_STASH: &[(&str, &str)] = &[
    ("list", "Lista lo apartado"),
    ("show", "Muestra lo apartado"),
    ("pop", "Recupera y borra"),
    ("apply", "Recupera sin borrar"),
    ("drop", "Borra lo apartado"),
    ("push", "Aparta los cambios"),
    ("clear", "Borra todo lo apartado"),
];

fn git(args: &[String], q: &str, cwd: Option<&Path>, out: &mut Vec<Cand>) {
    // Subórden = primera palabra que no es opción (saltando el valor de -C, -c…).
    let mut i = 0;
    let mut dir = cwd.map(Path::to_path_buf);
    while let Some(a) = args.get(i).filter(|a| a.starts_with('-')) {
        if matches!(
            a.as_str(),
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace"
        ) {
            if a == "-C" {
                dir = args.get(i + 1).and_then(|d| expand(d, cwd));
            }
            i += 1;
        }
        i += 1;
    }
    let Some(sub) = args.get(i) else {
        list(GIT, out);
        out.extend(
            git_aliases()
                .into_iter()
                .map(|(n, v)| simple(&n, Kind::Alias, v.into())),
        );
        return;
    };
    let positional = args[i + 1..].iter().filter(|a| !a.starts_with('-')).count();
    let repo = dir.as_deref().and_then(find_git_dir);
    let repo = repo.as_deref();
    match sub.as_str() {
        "checkout" => {
            branches(repo, true, out);
            paths(q, cwd, Filter::All, out);
        }
        "switch" => branches(repo, true, out),
        "merge" | "rebase" | "branch" | "cherry-pick" | "revert" => branches(repo, false, out),
        "diff" | "log" | "show" | "reset" => {
            branches(repo, false, out);
            paths(q, cwd, Filter::All, out);
        }
        "push" | "pull" | "fetch" if positional == 0 => remotes(repo, out),
        "push" | "pull" | "fetch" => branches(repo, false, out),
        "remote" if positional == 0 => list(GIT_REMOTE, out),
        "remote" => remotes(repo, out),
        "stash" if positional == 0 => list(GIT_STASH, out),
        "stash" | "clone" => {}
        _ => paths(q, cwd, Filter::All, out),
    }
}

/// Carpeta `.git` del repositorio que contiene `dir` (también en *worktrees*: archivo `.git`).
fn find_git_dir(dir: &Path) -> Option<PathBuf> {
    for d in dir.ancestors().take(32) {
        let g = d.join(".git");
        if g.is_dir() {
            return Some(g);
        }
        if let Ok(text) = std::fs::read_to_string(&g) {
            let p = text.strip_prefix("gitdir:")?.trim();
            return Some(d.join(p));
        }
    }
    None
}

/// Rama actual del repositorio que contiene `dir` (o el commit abreviado si no hay rama), con una
/// caché de 2 s: el dibujo la pide en cada fotograma.
pub fn git_branch(dir: &Path) -> Option<String> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(PathBuf, Instant, Option<String>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((d, at, b)) = cache.as_ref() {
        if d == dir && at.elapsed() < Duration::from_secs(2) {
            return b.clone();
        }
    }
    let head = find_git_dir(dir).and_then(|g| std::fs::read_to_string(g.join("HEAD")).ok());
    let b = head.and_then(|h| {
        let h = h.trim();
        let name = match h.strip_prefix("ref: refs/heads/") {
            Some(b) => b.to_string(),
            None => h.get(..7)?.to_string(),
        };
        (!name.is_empty() && !has_control(&name)).then_some(name)
    });
    *cache = Some((dir.to_path_buf(), Instant::now(), b.clone()));
    b
}

/// Donde viven las referencias compartidas (en un *worktree*, la carpeta del repositorio principal).
fn common_dir(git: &Path) -> PathBuf {
    match std::fs::read_to_string(git.join("commondir")) {
        Ok(c) => git.join(c.trim()),
        Err(_) => git.to_path_buf(),
    }
}

/// Referencias bajo `refs/<kind>/` (archivos sueltos y `packed-refs`), sin el prefijo.
fn refs(common: &Path, kind: &str) -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let Ok(name) = e.file_name().into_string() else {
                continue;
            };
            let full = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                if depth < 8 {
                    walk(&e.path(), &full, depth + 1, out);
                }
            } else {
                out.push(full);
            }
        }
    }
    let mut out = Vec::new();
    walk(&common.join("refs").join(kind), "", 0, &mut out);
    let packed = format!("refs/{kind}/");
    if let Ok(text) = std::fs::read_to_string(common.join("packed-refs")) {
        out.extend(
            text.lines()
                .filter_map(|l| l.split_once(' ')?.1.strip_prefix(&packed))
                .map(str::to_string),
        );
    }
    out.sort();
    out.dedup();
    out
}

/// Ramas locales y remotas. Con `dwim` (checkout/switch) las remotas sin rama local se ofrecen con
/// su nombre corto, que git convierte en una rama que sigue a la remota.
fn branches(repo: Option<&Path>, dwim: bool, out: &mut Vec<Cand>) {
    let Some(git) = repo else { return };
    let common = common_dir(git);
    let local = refs(&common, "heads");
    let remote: Vec<String> = refs(&common, "remotes")
        .into_iter()
        .filter(|r| !r.ends_with("/HEAD"))
        .collect();
    out.extend(
        local
            .iter()
            .map(|b| simple(b, Kind::Branch, Kind::Branch.name().into())),
    );
    let mut seen: HashSet<String> = local.iter().cloned().collect();
    for r in remote {
        match r.split_once('/') {
            Some((origin, short)) if dwim => {
                if seen.insert(short.to_string()) {
                    out.push(simple(
                        short,
                        Kind::RemoteBranch,
                        format!("rama de {origin}").into(),
                    ));
                }
            }
            _ => out.push(simple(
                &r,
                Kind::RemoteBranch,
                Kind::RemoteBranch.name().into(),
            )),
        }
    }
}

fn remotes(repo: Option<&Path>, out: &mut Vec<Cand>) {
    let Some(git) = repo else { return };
    let Ok(text) = std::fs::read_to_string(common_dir(git).join("config")) else {
        return;
    };
    for l in text.lines() {
        if let Some(name) = l
            .trim()
            .strip_prefix("[remote \"")
            .and_then(|r| r.strip_suffix("\"]"))
        {
            out.push(simple(name, Kind::Remote, Kind::Remote.name().into()));
        }
    }
}

/// Alias de `~/.gitconfig` y `~/.config/git/config` (nombre, a qué equivale).
fn git_aliases() -> Vec<(String, String)> {
    let mut files = Vec::new();
    if let Some(h) = dirs::home_dir() {
        files.push(h.join(".gitconfig"));
    }
    if let Some(c) = dirs::config_dir() {
        files.push(c.join("git/config"));
    }
    let mut out = Vec::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        let mut in_alias = false;
        for l in text.lines().map(str::trim) {
            if l.starts_with('[') {
                in_alias = l.eq_ignore_ascii_case("[alias]");
            } else if in_alias {
                if let Some((n, v)) = l.split_once('=') {
                    out.push((n.trim().to_string(), v.trim().to_string()));
                }
            }
        }
    }
    out
}

// ---------- cargo, npm, make, ssh, dnf, systemctl ----------

const CARGO: &[(&str, &str)] = &[
    ("add", "Añade dependencias"),
    ("bench", "Ejecuta las pruebas de rendimiento"),
    ("build", "Compila el paquete"),
    ("check", "Busca errores sin compilar"),
    ("clean", "Borra target/"),
    ("clippy", "Revisa el código"),
    ("doc", "Genera la documentación"),
    ("fetch", "Descarga las dependencias"),
    ("fix", "Corrige avisos automáticamente"),
    ("fmt", "Formatea el código"),
    ("init", "Crea un paquete en esta carpeta"),
    ("install", "Instala un binario"),
    ("new", "Crea un paquete nuevo"),
    ("publish", "Publica en crates.io"),
    ("remove", "Quita dependencias"),
    ("run", "Compila y ejecuta"),
    ("search", "Busca en crates.io"),
    ("test", "Ejecuta las pruebas"),
    ("tree", "Árbol de dependencias"),
    ("uninstall", "Desinstala un binario"),
    ("update", "Actualiza Cargo.lock"),
];

fn cargo(out: &mut Vec<Cand>) {
    list(CARGO, out);
    // Complementos instalados (`cargo-generate-rpm` -> `cargo generate-rpm`).
    crate::suggest::with_known(|known| {
        for name in known.keys() {
            if let Some(sub) = name
                .strip_prefix("cargo-")
                .filter(|s| !s.is_empty() && !CARGO.iter().any(|c| c.0 == *s))
            {
                out.push(simple(sub, Kind::Sub, "complemento".into()));
            }
        }
    });
}

const JS: &[(&str, &str)] = &[
    ("install", "Instala las dependencias"),
    ("add", "Añade una dependencia"),
    ("remove", "Quita una dependencia"),
    ("run", "Ejecuta un script"),
    ("test", "Ejecuta las pruebas"),
    ("start", "Arranca el proyecto"),
    ("init", "Crea un package.json"),
    ("update", "Actualiza las dependencias"),
    ("exec", "Ejecuta un binario del proyecto"),
    ("publish", "Publica el paquete"),
];

fn js(name: &str, args: &[String], cwd: Option<&Path>, out: &mut Vec<Cand>) {
    let pos: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let scripts = |out: &mut Vec<Cand>| {
        for (n, cmd) in package_scripts(cwd) {
            out.push(simple(&n, Kind::Script, cmd.into()));
        }
    };
    match pos.as_slice() {
        [] => {
            list(JS, out);
            // pnpm, yarn y bun ejecutan los scripts sin `run`.
            if name != "npm" {
                scripts(out);
            }
        }
        ["run" | "run-script"] => scripts(out),
        _ => {}
    }
}

/// Scripts del package.json más cercano (nombre, orden).
fn package_scripts(cwd: Option<&Path>) -> Vec<(String, String)> {
    let Some(cwd) = cwd else { return vec![] };
    for d in cwd.ancestors().take(8) {
        let Ok(text) = std::fs::read_to_string(d.join("package.json")) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return vec![];
        };
        let Some(obj) = v.get("scripts").and_then(|s| s.as_object()) else {
            return vec![];
        };
        return obj
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
            .collect();
    }
    vec![]
}

fn make_targets(cwd: Option<&Path>, out: &mut Vec<Cand>) {
    let Some(cwd) = cwd else { return };
    let Some(text) = ["GNUmakefile", "makefile", "Makefile"]
        .iter()
        .find_map(|f| std::fs::read_to_string(cwd.join(f)).ok())
    else {
        return;
    };
    let mut seen = HashSet::new();
    for l in text.lines() {
        if l.starts_with([' ', '\t', '#', '.']) {
            continue;
        }
        let Some((head, rest)) = l.split_once(':') else {
            continue;
        };
        if rest.starts_with('=') || head.contains(['=', '$', '%']) {
            continue;
        }
        for t in head.split_whitespace() {
            if seen.insert(t.to_string()) {
                out.push(simple(t, Kind::Target, Kind::Target.name().into()));
            }
        }
    }
}

/// Equipos de `~/.ssh/config` (sin comodines). Admite `usuario@equipo`.
fn hosts(q: &str, end: &str, out: &mut Vec<Cand>) {
    let Some(text) =
        dirs::home_dir().and_then(|h| std::fs::read_to_string(h.join(".ssh/config")).ok())
    else {
        return;
    };
    let at = q.rfind('@').map_or(0, |i| i + 1);
    let user = &q[..at];
    for l in text.lines() {
        let l = l.trim();
        let Some((k, v)) = l.split_once(char::is_whitespace) else {
            continue;
        };
        if !k.eq_ignore_ascii_case("host") {
            continue;
        }
        for h in v
            .split_whitespace()
            .filter(|h| !h.contains(['*', '?', '!']))
        {
            out.push(Cand {
                key: h.into(),
                label: h.into(),
                insert: format!("{}{}{end}", escape(user), escape(h)),
                kind: Kind::Host,
                detail: Kind::Host.name().into(),
                skip: at,
            });
        }
    }
}

const DNF: &[(&str, &str)] = &[
    ("install", "Instala paquetes"),
    ("remove", "Desinstala paquetes"),
    ("upgrade", "Actualiza el sistema o paquetes"),
    ("search", "Busca paquetes"),
    ("info", "Información de un paquete"),
    ("list", "Lista paquetes"),
    ("provides", "Qué paquete da un archivo"),
    ("history", "Historial de operaciones"),
    ("autoremove", "Quita dependencias sin uso"),
    ("reinstall", "Reinstala paquetes"),
    ("downgrade", "Vuelve a una versión anterior"),
    ("check-upgrade", "Comprueba si hay actualizaciones"),
    ("clean", "Limpia la caché"),
    ("makecache", "Rehace la caché"),
    ("repolist", "Lista los repositorios"),
    ("group", "Grupos de paquetes"),
    ("copr", "Repositorios Copr"),
    ("swap", "Cambia un paquete por otro"),
];

const SYSTEMCTL: &[(&str, &str)] = &[
    ("status", "Estado de un servicio"),
    ("start", "Arranca un servicio"),
    ("stop", "Para un servicio"),
    ("restart", "Reinicia un servicio"),
    ("reload", "Recarga la configuración"),
    ("enable", "Activa al arrancar"),
    ("disable", "Desactiva al arrancar"),
    ("is-active", "¿Está en marcha?"),
    ("is-enabled", "¿Arranca solo?"),
    ("list-units", "Unidades cargadas"),
    ("list-unit-files", "Unidades instaladas"),
    ("list-timers", "Temporizadores"),
    ("daemon-reload", "Relee las unidades"),
    ("edit", "Edita una unidad"),
    ("cat", "Muestra una unidad"),
    ("mask", "Bloquea una unidad"),
    ("unmask", "Desbloquea una unidad"),
    ("poweroff", "Apaga el equipo"),
    ("reboot", "Reinicia el equipo"),
    ("suspend", "Suspende el equipo"),
];

// ---------- Comparación y orden ----------

/// Minúscula y sin tilde (para comparar "videos" con "Vídeos").
fn fold(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'á' | 'à' | 'ä' | 'â' | 'ã' => 'a',
        'é' | 'è' | 'ë' | 'ê' => 'e',
        'í' | 'ì' | 'ï' | 'î' => 'i',
        'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
        'ú' | 'ù' | 'ü' | 'û' => 'u',
        'ñ' => 'n',
        'ç' => 'c',
        c => c,
    }
}

/// (nivel, puntuación, posiciones) de `q` en `key`; ver `Item::tier`.
fn fuzzy(q: &str, key: &str) -> Option<(u8, i32, Vec<usize>)> {
    let n = q.chars().count();
    if n == 0 {
        return Some((3, 0, vec![]));
    }
    if key.starts_with(q) {
        return Some((3, 0, (0..n).collect()));
    }
    let k: Vec<char> = key.chars().map(fold).collect();
    let qq: Vec<char> = q.chars().map(fold).collect();
    if k.len() < n {
        return None;
    }
    if k.starts_with(&qq) {
        return Some((2, 0, (0..n).collect()));
    }
    // Con una sola letra, solo lo que empieza por ella (si no, saldría casi todo).
    if n < 2 {
        return None;
    }
    let boundary = |i: usize| i == 0 || matches!(k[i - 1], '-' | '_' | '.' | ' ' | '/');
    let mut found: Option<usize> = None;
    for i in 1..=k.len() - n {
        if k[i..i + n] == qq[..] {
            if boundary(i) {
                found = Some(i);
                break;
            }
            found = found.or(Some(i));
        }
    }
    if let Some(i) = found {
        let score = if boundary(i) { 10 } else { 0 } - i as i32;
        return Some((1, score, (i..i + n).collect()));
    }
    let mut hits: Vec<usize> = Vec::with_capacity(n);
    let mut score = 0;
    let mut ki = 0;
    for &qc in &qq {
        while ki < k.len() && k[ki] != qc {
            ki += 1;
        }
        if ki == k.len() {
            return None;
        }
        if hits.last().is_some_and(|&l| l + 1 == ki) {
            score += 3;
        }
        if boundary(ki) {
            score += 5;
        }
        hits.push(ki);
        ki += 1;
    }
    Some((0, score - k.len() as i32 / 4, hits))
}

fn rank(q: &str, cands: Vec<Cand>) -> Vec<Item> {
    let mut seen = HashSet::new();
    let mut items: Vec<Item> = cands
        .into_iter()
        // Nombres con caracteres de control (archivos, ramas, scripts… de origen ajeno): fuera.
        .filter(|c| !has_control(&c.insert) && !has_control(&c.label))
        .filter(|c| seen.insert(c.insert.clone()))
        .filter_map(|c| {
            let query = q.get(c.skip.min(q.len())..).unwrap_or("");
            let (tier, mut score, hits) = fuzzy(query, &c.key)?;
            // Entre órdenes, las cortas primero ("cd" antes que "cdrecord").
            if matches!(c.kind, Kind::Command | Kind::Alias | Kind::Builtin) {
                score -= c.key.chars().count() as i32;
            }
            // La descripción solo se dibuja, pero sin controles (p. ej. un script de package.json).
            let detail = if has_control(&c.detail) {
                c.detail.replace(char::is_control, " ").into()
            } else {
                c.detail
            };
            Some(Item {
                label: c.label,
                insert: c.insert,
                kind: c.kind,
                detail,
                hits,
                tier,
                score,
            })
        })
        .collect();
    items.sort_by(|a, b| {
        b.tier
            .cmp(&a.tier)
            .then(b.score.cmp(&a.score))
            .then(a.kind.group().cmp(&b.kind.group()))
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });
    // Las coincidencias "letras en orden" solo si no hay ninguna mejor: con cientos de órdenes en
    // el PATH, "carg" no debe ofrecer "canberra-gtk-play".
    if items.first().is_some_and(|i| i.tier > 0) {
        items.retain(|i| i.tier > 0);
    }
    items.truncate(MAX_ITEMS);
    items
}

// ---------- Menú ----------

/// Menú de completado abierto en una pestaña.
pub struct Menu {
    pub tab: u64,
    /// Lo escrito cuando se calcularon los candidatos.
    pub input: String,
    pub comp: Completions,
    pub selected: usize,
    /// Primera fila visible.
    pub top: usize,
    /// Se abrió solo al escribir ("Menú al escribir"): Intro ejecuta la línea y Tab acepta.
    pub auto: bool,
    /// Dónde se dibujó: x, y, ancho, alto, alto de fila y margen (para el ratón).
    pub rect: Option<[f32; 6]>,
    wheel: f32,
}

impl Menu {
    pub fn new(tab: u64, input: String, comp: Completions) -> Self {
        Self {
            tab,
            input,
            comp,
            selected: 0,
            top: 0,
            auto: false,
            rect: None,
            wheel: 0.0,
        }
    }

    /// Marca el menú como abierto solo al escribir (ver `auto`).
    pub fn opened_while_typing(mut self) -> Self {
        self.auto = true;
        self
    }

    pub fn move_sel(&mut self, d: i32) {
        let n = self.comp.items.len() as i32;
        if n > 0 {
            self.selected = (self.selected as i32 + d).rem_euclid(n) as usize;
            self.keep_visible();
        }
    }

    pub fn page(&mut self, d: i32) {
        let n = self.comp.items.len();
        if n > 0 {
            self.selected =
                (self.selected as i32 + d * MENU_ROWS as i32).clamp(0, n as i32 - 1) as usize;
            self.keep_visible();
        }
    }

    fn keep_visible(&mut self) {
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + MENU_ROWS {
            self.top = self.selected + 1 - MENU_ROWS;
        }
    }

    pub fn accept(&self) -> Option<Vec<u8>> {
        self.comp.accept_bytes(self.selected)
    }

    /// Rehace los candidatos si cambió lo escrito. `false` = el menú ya no aplica (se escribió un
    /// espacio, se borró la palabra entera, se movió el cursor…) y hay que cerrarlo.
    pub fn refresh(&mut self, input: &str, cwd: Option<&Path>) -> bool {
        if input == self.input {
            return true;
        }
        let before: String = self.input.chars().take(self.comp.start).collect();
        if !input.starts_with(&before) {
            return false;
        }
        match complete(input, cwd) {
            Some(c) if c.start == self.comp.start && !c.items.is_empty() => {
                self.input = input.to_string();
                self.comp = c;
                self.selected = 0;
                self.top = 0;
                true
            }
            _ => false,
        }
    }

    /// Acumula la rueda; devuelve cuántas filas enteras avanzar (positivo = hacia arriba).
    pub fn wheel(&mut self, steps: f32) -> i32 {
        self.wheel += steps;
        let n = self.wheel.trunc() as i32;
        self.wheel -= n as f32;
        n
    }

    /// Opción bajo el ratón.
    pub fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        let [rx, ry, w, h, row_h, pad] = self.rect?;
        if x < rx || x >= rx + w || y < ry || y >= ry + h {
            return None;
        }
        let r = ((y - ry - pad) / row_h).floor().max(0.0) as usize;
        let i = self.top + r.min(MENU_ROWS - 1);
        (i < self.comp.items.len()).then_some(i)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.rect
            .is_some_and(|[rx, ry, w, h, ..]| x >= rx && x < rx + w && y >= ry && y < ry + h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lectura_de_la_linea() {
        let l = lex("git checkout ma");
        assert_eq!(l.words, ["git", "checkout"]);
        assert_eq!(
            (l.start, l.raw.as_str(), l.value.as_str()),
            (13, "ma", "ma")
        );
        let l = lex("cd ");
        assert_eq!(l.words, ["cd"]);
        assert_eq!((l.start, l.raw.as_str()), (3, ""));
        let l = lex("ls Mi\\ car");
        assert_eq!((l.raw.as_str(), l.value.as_str()), ("Mi\\ car", "Mi car"));
        let l = lex("cat \"Mis doc");
        assert_eq!((l.start, l.value.as_str()), (4, "Mis doc"));
        let l = lex("make && cd sr");
        assert_eq!(l.words, ["cd"], "otra orden tras &&");
        let l = lex("echo $(git br");
        assert_eq!(l.words, ["git"]);
        let l = lex("sort < da");
        assert!(l.redirect);
        assert_eq!(l.words, ["sort"]);
        assert!(lex("ls $HOME/Do").dynamic);
    }

    #[test]
    fn orden_y_argumentos() {
        let w = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(split_command(&w(&[])), None);
        assert_eq!(split_command(&w(&["sudo"])), None, "tras sudo va una orden");
        assert_eq!(split_command(&w(&["sudo", "-u", "pepe"])), None);
        assert_eq!(
            split_command(&w(&["FOO=1", "env", "A=2", "git"])).map(|c| c.0),
            Some("git")
        );
        let wo = w(&["sudo", "dnf", "install"]);
        let (cmd, args) = split_command(&wo).unwrap();
        assert_eq!((cmd, args.len()), ("dnf", 1));
    }

    #[test]
    fn busqueda_difusa() {
        assert_eq!(fuzzy("Doc", "Documentos").map(|f| f.0), Some(3));
        assert_eq!(
            fuzzy("doc", "Documentos").map(|f| f.0),
            Some(2),
            "sin distinguir mayúsculas"
        );
        assert_eq!(fuzzy("vid", "Vídeos").map(|f| f.0), Some(2), "sin tildes");
        assert_eq!(fuzzy("ter", "GRBTER").map(|f| f.0), Some(1), "lo contiene");
        assert_eq!(
            fuzzy("dent", "dashboard_entrenamiento").map(|f| f.0),
            Some(0),
            "letras en orden"
        );
        assert_eq!(fuzzy("x", "GRBTER"), None);
        assert_eq!(
            fuzzy("e", "GRBTER"),
            None,
            "con una letra solo el principio o el interior seguido"
        );
        let (_, _, hits) = fuzzy("dent", "dashboard_entrenamiento").unwrap();
        assert_eq!(hits, [0, 10, 11, 12]);
        // Letras en orden solo si no hay nada mejor.
        let cand = |k: &str| Cand {
            key: k.into(),
            label: k.into(),
            insert: k.into(),
            kind: Kind::Command,
            detail: "".into(),
            skip: 0,
        };
        let labels = |q: &str| {
            rank(q, vec![cand("cargo"), cand("canberra-gtk-play")])
                .into_iter()
                .map(|i| i.label)
                .collect::<Vec<_>>()
        };
        assert_eq!(labels("carg"), ["cargo"]);
        assert_eq!(labels("cgp"), ["canberra-gtk-play"]);
    }

    #[test]
    fn carpetas_y_archivos() {
        let dir = std::env::temp_dir().join(format!("garbo-complete-{}", std::process::id()));
        for d in ["Documentos", "Descargas", "Mi carpeta", ".oculta"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("notas.txt"), "").unwrap();
        std::fs::write(dir.join("nota\nrm -rf x"), "").unwrap();
        std::fs::write(dir.join("nota\u{1b}[A"), "").unwrap();
        std::fs::create_dir_all(dir.join("Documentos/Proyectos")).unwrap();
        let cwd = Some(dir.as_path());
        let labels = |c: &Completions| c.items.iter().map(|i| i.label.clone()).collect::<Vec<_>>();

        let c = complete("cd ", cwd).unwrap();
        assert_eq!(
            labels(&c),
            ["Descargas/", "Documentos/", "Mi carpeta/"],
            "solo carpetas, sin ocultas"
        );
        let c = complete("cd doc", cwd).unwrap();
        assert_eq!(labels(&c), ["Documentos/"]);
        assert_eq!(
            c.accept_bytes(0).unwrap(),
            b"\x7f\x7f\x7fDocumentos/",
            "se reescribe con sus mayúsculas"
        );
        let c = complete("cd Doc", cwd).unwrap();
        assert_eq!(
            c.accept_bytes(0).unwrap(),
            b"umentos/",
            "si amplía, solo lo que falta"
        );
        assert_eq!(c.ghost().as_deref(), Some("umentos/"));
        let c = complete("cd D", cwd).unwrap();
        assert_eq!(c.ghost(), None, "dos candidatos: sin sugerencia en gris");
        assert_eq!(c.common_extension(), None);
        let c = complete("cd Mi", cwd).unwrap();
        assert_eq!(c.items[0].insert, "Mi\\ carpeta/", "con escapes");
        let c = complete("cd Documentos/P", cwd).unwrap();
        assert_eq!(c.items[0].insert, "Documentos/Proyectos/");
        let c = complete("cd .o", cwd).unwrap();
        assert_eq!(labels(&c), [".oculta/"]);
        let c = complete("cat ", cwd).unwrap();
        assert!(labels(&c).contains(&"notas.txt".to_string()));
        assert_eq!(
            c.items
                .iter()
                .find(|i| i.label == "notas.txt")
                .unwrap()
                .insert,
            "notas.txt "
        );
        assert_eq!(
            complete("ls -", cwd).unwrap().items.len(),
            0,
            "opciones: para bash"
        );
        let c = complete("cat not", cwd).unwrap();
        assert_eq!(
            labels(&c),
            ["notas.txt"],
            "nombres con saltos de línea o ESC: fuera"
        );
        assert_eq!(
            replace("x", "a\nb"),
            None,
            "nunca se escribe un control en el PTY"
        );
        assert!(complete("", cwd).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ramas_de_git() {
        let dir = std::env::temp_dir().join(format!("garbo-git-{}", std::process::id()));
        let g = dir.join(".git");
        std::fs::create_dir_all(g.join("refs/heads/feature")).unwrap();
        std::fs::create_dir_all(g.join("refs/remotes/origin")).unwrap();
        std::fs::write(g.join("refs/heads/main"), "x").unwrap();
        std::fs::write(g.join("refs/heads/feature/menu"), "x").unwrap();
        std::fs::write(g.join("refs/remotes/origin/HEAD"), "x").unwrap();
        std::fs::write(g.join("refs/remotes/origin/main"), "x").unwrap();
        std::fs::write(
            g.join("packed-refs"),
            "# pack\nabc refs/remotes/origin/arreglo\n",
        )
        .unwrap();
        std::fs::write(g.join("config"), "[core]\n[remote \"origin\"]\n\turl = x\n").unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let cwd = Some(dir.join("src"));
        let labels = |q: &str| {
            complete(q, cwd.as_deref())
                .unwrap()
                .items
                .into_iter()
                .map(|i| i.label)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels("git switch "),
            ["feature/menu", "main", "arreglo"],
            "remotas sin local, con nombre corto"
        );
        assert_eq!(
            labels("git merge "),
            ["feature/menu", "main", "origin/arreglo", "origin/main"]
        );
        assert_eq!(labels("git push "), ["origin"]);
        assert!(labels("git ch").starts_with(&["checkout".to_string(), "cherry-pick".to_string()]));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prefijo_comun_y_menu() {
        let item = |l: &str| Item {
            label: l.into(),
            insert: format!("{l}/"),
            kind: Kind::Dir,
            detail: "".into(),
            hits: vec![],
            tier: 3,
            score: 0,
        };
        let c = Completions {
            start: 3,
            word: "pro".into(),
            items: vec![item("proyecto-a"), item("proyecto-b")],
        };
        assert_eq!(c.common_extension().unwrap(), b"yecto-");
        let mut m = Menu::new(1, "cd pro".into(), c);
        m.move_sel(-1);
        assert_eq!(m.selected, 1, "da la vuelta");
        assert!(!m.refresh("ls ", None), "otra palabra: se cierra");
    }
}
