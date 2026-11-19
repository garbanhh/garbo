//! Ctrl+clic en lo que hay bajo el ratón: enlaces (OSC 8 o una URL en el texto) y rutas de
//! archivo, con su línea si la llevan (`src/main.rs:42:5`, `main.c:7:`, `File "x.py", line 3`).
//!
//! Las URL se abren con `xdg-open`; los archivos, en el editor de `$VISUAL` o `$EDITOR`: los de
//! ventana (VS Code, Zed, GNOME Text Editor…) por su cuenta y los de terminal (nano, vim…) en una
//! pestaña nueva, escribiendo la orden en su shell. Las carpetas, en el gestor de archivos. Con
//! Ctrl pulsado, lo que se abriría se subraya.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Url(String),
    File {
        path: PathBuf,
        line: Option<u32>,
        col: Option<u32>,
    },
}

/// Lo encontrado y las celdas que ocupa (para subrayarlo).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub target: Target,
    pub start: Point,
    pub end: Point,
}

const SCHEMES: [&str; 5] = ["https://", "http://", "ftp://", "file://", "mailto:"];

/// Qué hay en `p` (celda de la rejilla). `cwd`: carpeta para las rutas relativas.
pub fn find<T>(term: &Term<T>, p: Point, cwd: Option<&Path>) -> Option<Found> {
    if let Some(found) = hyperlink(term, p) {
        return Some(found);
    }
    let (chars, at) = logical_line(term, p)?;
    let text: Vec<char> = chars.iter().map(|(c, _)| *c).collect();
    let span = |(a, b): (usize, usize)| (chars[a].1, chars[b - 1].1);
    if let Some((range, url)) = url_at(&text, at) {
        let (start, end) = span(range);
        return Some(Found {
            target: Target::Url(url),
            start,
            end,
        });
    }
    let (range, target) = path_at(&text, at, cwd)?;
    let (start, end) = span(range);
    Some(Found { target, start, end })
}

/// Enlace OSC 8 (`ls --hyperlink`, compiladores, agentes), salvo los internos de Garbo.
fn hyperlink<T>(term: &Term<T>, p: Point) -> Option<Found> {
    let link = term.grid()[p].hyperlink()?;
    let uri = link.uri();
    if uri == crate::blocks::LINK_URI || !SCHEMES.iter().any(|s| uri.starts_with(s)) || !clean(uri)
    {
        return None;
    }
    // Celdas contiguas con el mismo enlace, en esa fila.
    let row = &term.grid()[p.line];
    let same = |c: usize| row[Column(c)].hyperlink().as_ref() == Some(&link);
    let mut a = p.column.0;
    while a > 0 && same(a - 1) {
        a -= 1;
    }
    let mut b = p.column.0;
    while b + 1 < term.columns() && same(b + 1) {
        b += 1;
    }
    let target = match uri.strip_prefix("file://") {
        // file://equipo/ruta: la ruta empieza en la primera "/" tras el equipo. La manda el
        // programa: tiene que existir y no llevar caracteres de control ("%0A", "%15"…), que
        // acabarían tecleados en el shell al abrirla en un editor de terminal.
        Some(rest) => {
            let path = percent_decode(&rest[rest.find('/')?..]);
            if !clean(&path) || !Path::new(&path).is_absolute() || !Path::new(&path).exists() {
                return None;
            }
            Target::File {
                path: PathBuf::from(path),
                line: None,
                col: None,
            }
        }
        None => Target::Url(uri.to_string()),
    };
    Some(Found {
        target,
        start: Point::new(p.line, Column(a)),
        end: Point::new(p.line, Column(b)),
    })
}

/// Sin caracteres de control.
fn clean(s: &str) -> bool {
    !s.chars().any(char::is_control)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match (
            b[i],
            b.get(i + 1).copied().and_then(hex),
            b.get(i + 2).copied().and_then(hex),
        ) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (c, ..) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// La línea lógica que contiene `p` (unida a través de las filas partidas por el ancho), con la
/// celda de cada carácter, y el índice del carácter de `p`.
fn logical_line<T>(term: &Term<T>, p: Point) -> Option<(Vec<(char, Point)>, usize)> {
    let grid = term.grid();
    let cols = term.columns();
    let wraps = |l: Line| grid[l][Column(cols - 1)].flags.contains(Flags::WRAPLINE);
    let mut first = p.line;
    while first > term.topmost_line() && wraps(first - 1) {
        first -= 1;
    }
    let mut last = p.line;
    while last < term.bottommost_line() && wraps(last) {
        last += 1;
    }
    let mut chars = Vec::new();
    let mut at = None;
    for l in first.0..=last.0 {
        for c in 0..cols {
            let pt = Point::new(Line(l), Column(c));
            let cell = &grid[pt];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                if pt == p {
                    at = chars.len().checked_sub(1);
                }
                continue;
            }
            if pt == p {
                at = Some(chars.len());
            }
            chars.push((if cell.c == '\0' { ' ' } else { cell.c }, pt));
        }
    }
    Some((chars, at?))
}

/// URL que contiene la posición `at`: desde su esquema hasta un espacio o un carácter que no
/// puede ir en una URL, sin la puntuación final (ni el paréntesis de cierre si no se abrió).
fn url_at(text: &[char], at: usize) -> Option<((usize, usize), String)> {
    let s: String = text.iter().collect();
    let idx: Vec<usize> = s.char_indices().map(|(i, _)| i).collect();
    for scheme in SCHEMES {
        let mut from = 0;
        while let Some(off) = s[from..].find(scheme) {
            let start_b = from + off;
            from = start_b + scheme.len();
            let start = idx.iter().position(|&i| i == start_b)?;
            let mut end = start;
            while end < text.len()
                && !text[end].is_whitespace()
                && !"\"'<>`{}|\\^".contains(text[end])
            {
                end += 1;
            }
            // Puntuación final y cierres sin abrir.
            while let Some(&last) = text[start..end].last() {
                let opens = |o: char| text[start..end].iter().filter(|&&c| c == o).count();
                let unbalanced = (last == ')' && opens('(') < opens(')'))
                    || (last == ']' && opens('[') < opens(']'));
                if ".,;:!?".contains(last) || unbalanced {
                    end -= 1;
                } else {
                    break;
                }
            }
            if end > start + scheme.len() && (start..end).contains(&at) {
                return Some(((start, end), text[start..end].iter().collect()));
            }
        }
    }
    None
}

fn delimiter(c: char) -> bool {
    c.is_whitespace() || "\"'`<>()[]{}|,=;".contains(c)
}

/// Ruta de archivo que existe y contiene `at`, con su línea y columna (`ruta:línea:columna`).
fn path_at(text: &[char], at: usize, cwd: Option<&Path>) -> Option<((usize, usize), Target)> {
    if delimiter(text[at]) {
        return None;
    }
    let mut start = at;
    while start > 0 && !delimiter(text[start - 1]) {
        start -= 1;
    }
    let mut end = at + 1;
    while end < text.len() && !delimiter(text[end]) {
        end += 1;
    }
    // Puntuación que no es de la ruta ("main.c:7:", "x.rs.").
    while end > start && ".:".contains(text[end - 1]) {
        end -= 1;
    }
    let token: String = text[start..end].iter().collect();
    let parts: Vec<&str> = token.split(':').collect();
    // El trozo más largo que es una ruta existente; detrás, línea y columna si son números.
    for k in (1..=parts.len()).rev() {
        let candidate = parts[..k].join(":");
        let Some(path) = resolve(&candidate, cwd) else {
            continue;
        };
        let num = |i: usize| parts.get(i).and_then(|p| p.parse::<u32>().ok());
        let mut line = num(k);
        let col = line.and(num(k + 1));
        // Python: File "x.py", line 42
        if line.is_none() {
            let rest: String = text[end..].iter().take(24).collect();
            line = rest
                .strip_prefix("\", line ")
                .and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok());
        }
        let used = candidate.chars().count();
        let cut = match (line, col) {
            (Some(l), Some(c)) if num(k).is_some() => {
                used + 2 + l.to_string().len() + c.to_string().len()
            }
            (Some(l), None) if num(k).is_some() => used + 1 + l.to_string().len(),
            _ => used,
        };
        return Some(((start, start + cut), Target::File { path, line, col }));
    }
    None
}

fn resolve(s: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    if s.is_empty() || s.len() > 4096 {
        return None;
    }
    let path = if let Some(rest) = s.strip_prefix("~/") {
        dirs::home_dir()?.join(rest)
    } else if s.starts_with('/') {
        PathBuf::from(s)
    } else {
        cwd?.join(s)
    };
    path.exists().then_some(path)
}

/// Cómo abrir un archivo.
#[derive(Debug, PartialEq, Eq)]
pub enum Open {
    /// Lanzar el programa (editor con ventana, o `xdg-open`).
    Spawn(Vec<String>),
    /// Escribir esta orden en una pestaña nueva (editor de terminal).
    InTab(String),
}

/// Orden para abrir `path` en la línea `line` con el editor `editor` (de `$VISUAL`/`$EDITOR`).
pub fn open_file(path: &Path, line: Option<u32>, col: Option<u32>, editor: Option<&str>) -> Open {
    let p = path.to_string_lossy().to_string();
    let xdg = || Open::Spawn(vec!["xdg-open".into(), p.clone()]);
    // Lo que va a una pestaña se teclea en su shell: nada con caracteres de control.
    if path.is_dir() || !clean(&p) {
        return xdg();
    }
    let Some(editor) = editor.map(str::trim).filter(|e| !e.is_empty()) else {
        return xdg();
    };
    let mut words: Vec<String> = editor.split_whitespace().map(str::to_string).collect();
    let name = words[0].rsplit('/').next().unwrap_or_default().to_string();
    let at = |sep: &str| match (line, col) {
        (Some(l), Some(c)) => format!("{p}{sep}{l}{sep}{c}"),
        (Some(l), None) => format!("{p}{sep}{l}"),
        _ => p.clone(),
    };
    match name.as_str() {
        // Editores con ventana: por su cuenta (sin esperar a que se cierre el archivo).
        "code" | "codium" | "code-oss" | "cursor" | "windsurf" => {
            words.retain(|w| w != "--wait" && w != "-w");
            words.extend(["-g".into(), at(":")]);
            Open::Spawn(words)
        }
        "zed" | "zeditor" | "subl" | "sublime_text" => {
            words.retain(|w| w != "--wait" && w != "-w");
            words.push(at(":"));
            Open::Spawn(words)
        }
        "gnome-text-editor" | "gedit" | "mousepad" | "pluma" | "xed" | "kwrite" | "kate" => {
            if let Some(l) = line {
                words.push(format!("+{l}"));
            }
            words.push(p);
            Open::Spawn(words)
        }
        // Helix usa ruta:línea:columna; el resto de editores de terminal, +línea.
        "hx" | "helix" => {
            words.push(at(":"));
            Open::InTab(words.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" "))
        }
        _ => {
            if let Some(l) = line {
                words.push(format!("+{l}"));
            }
            words.push(p);
            Open::InTab(words.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" "))
        }
    }
}

/// Para el shell: tal cual si no hay nada raro; si no, entre comillas simples.
fn quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./:+=,@%-".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn urls_en_el_texto() {
        let t = chars("ver https://example.com/a_(b)?q=1. y (https://x.org/y), fin");
        let at = |n: usize| url_at(&t, n).map(|(_, u)| u);
        assert_eq!(
            at(10).as_deref(),
            Some("https://example.com/a_(b)?q=1"),
            "sin el punto final, con el paréntesis equilibrado"
        );
        assert_eq!(
            at(45).as_deref(),
            Some("https://x.org/y"),
            "sin el paréntesis de cierre que no abrió"
        );
        assert_eq!(at(1), None);
    }

    #[test]
    fn rutas_con_linea() {
        let dir = std::env::temp_dir().join(format!("garbo-links-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "").unwrap();
        std::fs::write(dir.join("x.py"), "").unwrap();
        let find = |s: &str, at: usize| path_at(&chars(s), at, Some(&dir));
        let file = |p: &str, line, col| Target::File {
            path: dir.join(p),
            line,
            col,
        };

        let s = "  --> src/main.rs:42:5";
        assert_eq!(
            find(s, 8),
            Some(((6, 22), file("src/main.rs", Some(42), Some(5))))
        );
        assert_eq!(
            find("src/main.rs:7: error: x", 2),
            Some(((0, 13), file("src/main.rs", Some(7), None)))
        );
        assert_eq!(
            find("grep: src/main.rs:12:fn main", 8),
            Some(((6, 20), file("src/main.rs", Some(12), None)))
        );
        assert_eq!(
            find("  File \"x.py\", line 3, in f", 8),
            Some(((8, 12), file("x.py", Some(3), None)))
        );
        assert_eq!(find("src/no-existe.rs:3", 2), None);
        assert_eq!(find("hola mundo", 1), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn enlaces_osc8_con_trampa() {
        use alacritty_terminal::event::VoidListener;
        use alacritty_terminal::term::test::TermSize;
        use alacritty_terminal::term::Config;
        use alacritty_terminal::vte::ansi::{Handler, Hyperlink, Processor};
        let mut t = Term::new(Config::default(), &TermSize::new(40, 3), VoidListener);
        let mut pr: Processor = Processor::new();
        let link = |t: &mut Term<VoidListener>, uri: &str| {
            t.set_hyperlink(Some(Hyperlink {
                id: None,
                uri: uri.into(),
            }))
        };
        link(&mut t, "file:///tmp%15curl%20x%7Csh%0A");
        pr.advance(&mut t, b"abrir");
        link(&mut t, "https://example.com/a");
        pr.advance(&mut t, b" web");
        link(&mut t, "steam://run/1");
        pr.advance(&mut t, b" app");
        t.set_hyperlink(None);
        let at = |c: usize| hyperlink(&t, Point::new(Line(0), Column(c))).map(|f| f.target);
        assert_eq!(at(1), None, "ruta con caracteres de control: fuera");
        assert_eq!(at(6), Some(Target::Url("https://example.com/a".into())));
        assert_eq!(
            at(10),
            None,
            "esquemas que lanzarían otros programas: fuera"
        );
    }

    #[test]
    fn como_abrir() {
        let p = Path::new("/tmp/a b.rs");
        assert_eq!(
            open_file(p, Some(42), None, Some("/usr/bin/nano")),
            Open::InTab("/usr/bin/nano +42 '/tmp/a b.rs'".into())
        );
        assert_eq!(
            open_file(p, Some(4), Some(2), Some("code --wait")),
            Open::Spawn(vec!["code".into(), "-g".into(), "/tmp/a b.rs:4:2".into()])
        );
        assert_eq!(
            open_file(p, Some(4), Some(2), Some("hx")),
            Open::InTab("hx '/tmp/a b.rs:4:2'".into())
        );
        assert_eq!(
            open_file(p, None, None, None),
            Open::Spawn(vec!["xdg-open".into(), "/tmp/a b.rs".into()])
        );
        assert_eq!(
            open_file(p, Some(9), None, Some("gnome-text-editor")),
            Open::Spawn(vec![
                "gnome-text-editor".into(),
                "+9".into(),
                "/tmp/a b.rs".into()
            ])
        );
        assert_eq!(quote("it's"), "'it'\\''s'");
        // Una ruta con caracteres de control nunca se teclea en un shell.
        let raro = Path::new("/tmp/a\x15curl x|sh\n");
        assert_eq!(
            open_file(raro, Some(1), None, Some("nano")),
            Open::Spawn(vec!["xdg-open".into(), raro.to_string_lossy().into()])
        );
        assert_eq!(percent_decode("/home/a%20b/%C3%B1.txt"), "/home/a b/ñ.txt");
    }
}
