//! Shells con integración (bash, zsh y fish): cuál es, cómo se le inyecta el script que emite las
//! marcas de los bloques (`shell/`) y qué cambia entre ellos para las sugerencias y los atajos.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shell {
    #[default]
    Bash,
    Zsh,
    Fish,
    /// Sin integración (sh, nu, xonsh…).
    Other,
}

/// Lo que hay que añadir al arranque del shell para cargar la integración.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Injection {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl Shell {
    /// Por el nombre del programa (`/usr/bin/zsh` -> zsh).
    pub fn from_program(program: &str) -> Shell {
        match program.rsplit('/').next().unwrap_or(program) {
            "bash" => Shell::Bash,
            "zsh" => Shell::Zsh,
            "fish" => Shell::Fish,
            _ => Shell::Other,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Shell::Bash => "bash",
            Shell::Zsh => "zsh",
            Shell::Fish => "fish",
            Shell::Other => "shell",
        }
    }

    /// Argumentos y entorno para cargar la integración. `zdotdir`: el ZDOTDIR que tendría zsh sin
    /// Garbo (de la configuración o del entorno), que la integración le devuelve al arrancar.
    pub fn injection(self, zdotdir: Option<&str>) -> Option<Injection> {
        match self {
            Shell::Bash => {
                let rc = write_script("garbo.bash", include_str!("../shell/garbo.bash"))?;
                Some(Injection {
                    args: vec!["--rcfile".into(), path_str(&rc)],
                    env: Vec::new(),
                })
            }
            Shell::Zsh => {
                write_script("garbo.zsh", include_str!("../shell/garbo.zsh"))?;
                let env = write_script("zsh/.zshenv", include_str!("../shell/zsh/.zshenv"))?;
                let mut vars = vec![("ZDOTDIR".into(), path_str(env.parent()?))];
                if let Some(z) = zdotdir {
                    vars.push(("GARBO_ZSH_ZDOTDIR".into(), z.into()));
                }
                Some(Injection {
                    args: Vec::new(),
                    env: vars,
                })
            }
            Shell::Fish => {
                let script = write_script("garbo.fish", include_str!("../shell/garbo.fish"))?;
                Some(Injection {
                    args: vec![
                        "--init-command".into(),
                        format!("source {}", fish_quote(&path_str(&script))),
                    ],
                    env: Vec::new(),
                })
            }
            Shell::Other => None,
        }
    }
}

/// Shell de la última pestaña abierta: de él salen el historial y los alias que usan las
/// sugerencias (son comunes a todas las pestañas).
static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_current(shell: Shell) {
    CURRENT.store(shell as u8, Ordering::Relaxed);
}

pub fn current() -> Shell {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Shell::Zsh,
        2 => Shell::Fish,
        3 => Shell::Other,
        _ => Shell::Bash,
    }
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Entre comillas simples de fish (dentro solo se escapan `\` y `'`).
fn fish_quote(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Escribe un script de `shell/` en la carpeta de Garbo (`$XDG_RUNTIME_DIR/garbo/<versión>`) si no
/// está ya igual, y devuelve su ruta. Se escribe aparte y se renombra: otra instancia de Garbo puede estar
/// leyéndolo.
fn write_script(rel: &str, content: &str) -> Option<PathBuf> {
    // Una carpeta por versión: otra versión de Garbo abierta a la vez no pisa los scripts.
    let dir = dirs::runtime_dir()
        .or_else(dirs::cache_dir)?
        .join("garbo")
        .join(env!("CARGO_PKG_VERSION"));
    let path = dir.join(rel);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(content) {
        std::fs::create_dir_all(path.parent()?).ok()?;
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_file_name(format!(
            "{}.{}-{n}.tmp",
            path.file_name()?.to_string_lossy(),
            std::process::id()
        ));
        std::fs::write(&tmp, content).ok()?;
        std::fs::rename(&tmp, &path).ok()?;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconoce_el_shell_por_el_programa() {
        assert_eq!(Shell::from_program("/usr/bin/zsh"), Shell::Zsh);
        assert_eq!(Shell::from_program("fish"), Shell::Fish);
        assert_eq!(Shell::from_program("/bin/bash"), Shell::Bash);
        assert_eq!(Shell::from_program("/usr/bin/nu"), Shell::Other);
    }

    /// Contesta lo que el terminal manda de vuelta al shell (consultas de fish), como la UI.
    #[derive(Clone)]
    struct Responder(std::sync::Arc<std::sync::Mutex<Option<crate::pty_loop::PtySender>>>);

    impl alacritty_terminal::event::EventListener for Responder {
        fn send_event(&self, event: alacritty_terminal::event::Event) {
            use alacritty_terminal::event::Event;
            let reply = match event {
                Event::PtyWrite(s) => s,
                Event::ColorRequest(_, f) => {
                    f(alacritty_terminal::vte::ansi::Rgb { r: 0, g: 0, b: 0 })
                }
                _ => return,
            };
            if let Some(tx) = self.0.lock().unwrap().as_ref() {
                tx.send(alacritty_terminal::event_loop::Msg::Input(
                    reply.into_bytes().into(),
                ));
            }
        }
    }

    fn installed(program: &str) -> bool {
        std::env::var_os("PATH")
            .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
    }

    /// Arranca `shell` con la integración (HOME temporal con `rc` como archivo de arranque),
    /// teclea y comprueba los bloques, lo escrito con el prompt derecho a la vista y el comando
    /// que se guarda en el historial.
    fn prueba_real(shell: Shell, rc_name: &str, rc: &str, garbo_prompt: bool) {
        use crate::blocks::BlockState;
        use alacritty_terminal::event::WindowSize;
        use alacritty_terminal::event_loop::Msg;
        use alacritty_terminal::sync::FairMutex;
        use alacritty_terminal::term::{test::TermSize, Term};
        use alacritty_terminal::tty;
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        if !installed(shell.name()) {
            eprintln!("{} no está instalado: se omite", shell.name());
            return;
        }
        let home = std::env::temp_dir().join(format!(
            "garbo-prueba-{}-{}-{}",
            shell.name(),
            garbo_prompt,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(rc_name).parent().unwrap()).unwrap();
        std::fs::write(home.join(rc_name), rc).unwrap();
        let home_s = home.to_string_lossy().into_owned();

        let inj = shell.injection(Some(&home_s)).unwrap();
        let mut env: std::collections::HashMap<String, String> = inj.env.into_iter().collect();
        for (k, v) in [
            ("HOME", home_s.clone()),
            ("XDG_CONFIG_HOME", format!("{home_s}/.config")),
            ("XDG_DATA_HOME", format!("{home_s}/.local/share")),
            ("TERM", "xterm-256color".into()),
            ("GARBO_AUTOSUGGEST", "1".into()),
            ("GARBO_PROMPT", if garbo_prompt { "1" } else { "" }.into()),
        ] {
            env.insert(k.into(), v);
        }
        let responder = Responder(Default::default());
        let term = Arc::new(FairMutex::new(Term::new(
            Default::default(),
            &TermSize::new(80, 24),
            responder.clone(),
        )));
        let ws = WindowSize {
            num_lines: 24,
            num_cols: 80,
            cell_width: 10,
            cell_height: 20,
        };
        let opts = tty::Options {
            shell: Some(tty::Shell::new(shell.name().into(), inj.args)),
            working_directory: Some(home.clone()),
            drain_on_exit: false,
            env,
        };
        let pty = tty::new(&opts, ws, 0).unwrap();
        let blocks = Arc::new(Mutex::new(BlockState::new(pty.child().id())));
        let lp =
            crate::pty_loop::PtyLoop::new(term.clone(), responder.clone(), pty, blocks.clone())
                .unwrap();
        let tx = lp.sender();
        *responder.0.lock().unwrap() = Some(tx.clone());
        lp.spawn();

        let wait = |what: &str, ok: &dyn Fn() -> bool| {
            let end = Instant::now() + Duration::from_secs(10);
            while !ok() {
                if Instant::now() > end {
                    let t = term.lock();
                    let screen: Vec<String> = (0..24)
                        .map(|l| {
                            let row = &t.grid()[alacritty_terminal::index::Line(l)];
                            row[..]
                                .iter()
                                .map(|c| c.c)
                                .collect::<String>()
                                .trim_end()
                                .to_string()
                        })
                        .collect();
                    panic!(
                        "{}: no llegó «{what}». Bloques: {:?}\n{}",
                        shell.name(),
                        blocks.lock().unwrap().summary(),
                        screen.join("\n")
                    );
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        let typed = || {
            let t = term.lock();
            let start = blocks.lock().unwrap().input_start;
            crate::terminal::input_for(&t, start)
        };
        let send = |s: &str| tx.send(Msg::Input(s.as_bytes().to_vec().into()));

        wait("el prompt", &|| typed().as_deref() == Some(""));
        if garbo_prompt {
            // Con el prompt de Garbo, lo escrito va debajo de la carpeta, desde la primera columna.
            let start = blocks.lock().unwrap().input_start;
            assert_eq!(start.map(|s| s.1), Some(0), "{}", shell.name());
        }
        // Lo escrito se lee aunque el prompt derecho esté detrás del cursor.
        send("echo garbo-");
        wait("lo escrito", &|| typed().as_deref() == Some("echo garbo-"));
        let tag = format!("{}{}", shell.name(), garbo_prompt as u8);
        send(&format!("{tag}\r"));
        wait("el segundo prompt", &|| {
            blocks.lock().unwrap().summary().len() >= 2 && typed().as_deref() == Some("")
        });
        send("false\r");
        wait("el código de false", &|| {
            blocks
                .lock()
                .unwrap()
                .summary()
                .iter()
                .any(|b| b.2 == Some(1))
        });
        wait("el tercer prompt", &|| typed().as_deref() == Some(""));
        let summary = blocks.lock().unwrap().summary();
        tx.send(Msg::Shutdown);

        // Un bloque por prompt: los repintados no abren otro.
        assert_eq!(
            summary,
            [
                (garbo_prompt, true, Some(0)),
                (garbo_prompt, true, Some(1)),
                (garbo_prompt, false, None)
            ],
            "{}",
            shell.name()
        );
        // En el historial, el comando tal cual: sin el prompt derecho.
        let hist = std::fs::read_to_string(crate::suggest::history_path_for_tests()).unwrap();
        assert!(
            hist.lines()
                .any(|l| l.ends_with(&format!("\techo garbo-{tag}"))),
            "{}: {hist}",
            shell.name()
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn zsh_real_con_prompt_derecho() {
        let rc = "PS1='%n %~ %# '\nRPROMPT='[derecha]'\n";
        prueba_real(Shell::Zsh, ".zshrc", rc, false);
        prueba_real(Shell::Zsh, ".zshrc", rc, true);
    }

    #[test]
    fn fish_real_con_prompt_derecho() {
        let rc = "function fish_right_prompt; echo -n '[derecha]'; end\n";
        prueba_real(Shell::Fish, ".config/fish/config.fish", rc, false);
        prueba_real(Shell::Fish, ".config/fish/config.fish", rc, true);
    }

    #[test]
    fn bash_real() {
        prueba_real(Shell::Bash, ".bashrc", "PS1='\\u \\w \\$ '\n", false);
    }

    #[test]
    fn comillas_de_fish() {
        assert_eq!(fish_quote("/run/a b/garbo.fish"), "'/run/a b/garbo.fish'");
        assert_eq!(fish_quote(r"o'k\x"), r"'o\'k\\x'");
    }
}
