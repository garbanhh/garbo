//! Shells con integración (bash, zsh, fish y PowerShell): cuál es, cómo se le inyecta el script que
//! emite las marcas de los bloques (`shell/`) y qué cambia entre ellos para las sugerencias y los
//! atajos.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};

// `PowerShell` acaba como el enum, pero es su nombre.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shell {
    #[default]
    Bash,
    Zsh,
    Fish,
    /// Sin integración (sh, nu, xonsh, cmd…).
    Other,
    /// Windows PowerShell 5.1 (`powershell`) y PowerShell 7 (`pwsh`).
    PowerShell,
}

/// Lo que hay que añadir al arranque del shell para cargar la integración.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Injection {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl Shell {
    /// Por el nombre del programa (`/usr/bin/zsh` -> zsh; en Windows, `C:\…\pwsh.exe` -> PowerShell).
    pub fn from_program(program: &str) -> Shell {
        match crate::platform::base_name(program).to_lowercase().as_str() {
            "bash" => Shell::Bash,
            "zsh" => Shell::Zsh,
            "fish" => Shell::Fish,
            "pwsh" | "powershell" => Shell::PowerShell,
            _ => Shell::Other,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Shell::Bash => "bash",
            Shell::Zsh => "zsh",
            Shell::Fish => "fish",
            Shell::Other => "shell",
            Shell::PowerShell => "powershell",
        }
    }

    /// Argumentos y entorno para cargar la integración. `zdotdir`: el ZDOTDIR que tendría zsh sin
    /// Garbo (de la configuración o del entorno), que la integración le devuelve al arrancar.
    pub fn injection(self, zdotdir: Option<&str>) -> Option<Injection> {
        // Junto a la integración, lo que lleva Garbo al servidor por ssh (Linux, de momento).
        if cfg!(target_os = "linux") && matches!(self, Shell::Bash | Shell::Zsh | Shell::Fish) {
            write_script("ssh.sh", include_str!("../shell/ssh.sh"));
            write_script("remoto.b64", &remote_boot_b64());
        }
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
            Shell::PowerShell => {
                let script = write_script("garbo.ps1", include_str!("../shell/garbo.ps1"))?;
                // Como bloque de código y no con `. archivo.ps1`: la directiva de ejecución de serie
                // de Windows PowerShell (`Restricted`) no deja cargar archivos de script, y así no
                // hace falta cambiarla (ni se cambia para los del usuario).
                let load = format!(
                    ". ([scriptblock]::Create([IO.File]::ReadAllText('{}')))",
                    path_str(&script).replace('\'', "''")
                );
                Some(Injection {
                    args: vec!["-NoLogo".into(), "-NoExit".into(), "-Command".into(), load],
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
        4 => Shell::PowerShell,
        _ => Shell::Bash,
    }
}

/// Arranque del shell del servidor por ssh (`shell/remoto.sh`) con la integración de bash, zsh y
/// fish dentro: lo que `ssh.sh` manda al servidor.
pub fn remote_boot() -> String {
    include_str!("../shell/remoto.sh")
        .replace(
            "@GARBO_BASH@",
            include_str!("../shell/garbo.bash").trim_end(),
        )
        .replace("@GARBO_ZSH@", include_str!("../shell/garbo.zsh").trim_end())
        .replace(
            "@GARBO_ZSHENV@",
            include_str!("../shell/zsh/.zshenv").trim_end(),
        )
        .replace(
            "@GARBO_FISH@",
            include_str!("../shell/garbo.fish").trim_end(),
        )
}

/// `remote_boot` en base64 y en una sola línea: así viaja dentro de la orden de ssh sin comillas
/// que el shell del servidor pudiera leer.
fn remote_boot_b64() -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(remote_boot())
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
            #[cfg(windows)]
            escape_args: true,
        };
        let pty = tty::new(&opts, ws, 0).unwrap();
        let blocks = Arc::new(Mutex::new(BlockState::new(crate::platform::child_pid(
            &pty,
        ))));
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
        // Ctrl+Retroceso borra una palabra como la entiende cada shell, Ctrl+Z lo deshace y
        // Ctrl+Supr borra la de la derecha, con lo que manda Garbo (su atajo o, si no lo toca, la
        // tecla de siempre).
        use winit::keyboard::{Key, ModifiersState, NamedKey};
        let ctx = crate::edit::Context::Prompt(shell);
        let key = |k: &Key, usual: &[u8]| {
            let b = crate::edit::shortcut(k, ModifiersState::CONTROL, ctx).unwrap_or(usual);
            tx.send(Msg::Input(b.to_vec().into()));
        };
        send("\x15echo ~/Documentos/Proyectos");
        wait("la ruta", &|| {
            typed().as_deref() == Some("echo ~/Documentos/Proyectos")
        });
        key(&Key::Named(NamedKey::Backspace), b"\x08");
        // zsh, con su WORDCHARS de serie, cuenta la ruta entera como una palabra.
        let left = if shell == Shell::Zsh {
            "echo "
        } else {
            "echo ~/Documentos/"
        };
        wait("Ctrl+Retroceso", &|| typed().as_deref() == Some(left));
        key(&Key::Character("z".into()), b"\x1a");
        wait("Ctrl+Z", &|| {
            typed().as_deref() == Some("echo ~/Documentos/Proyectos")
        });
        send("\x15echo uno dos\x1b[D\x1b[D\x1b[D");
        key(&Key::Named(NamedKey::Delete), b"\x1b[3;5~");
        wait("Ctrl+Supr", &|| typed().as_deref() == Some("echo uno "));
        send("\x15");
        wait("la línea vacía", &|| typed().as_deref() == Some(""));
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

    /// ssh de punta a punta sin servidor: `ssh.sh` de verdad, con un `ssh` falso que, como el de
    /// verdad, abre su propio terminal y ejecuta la orden con `$SHELL -c` (lo que haría sshd). El
    /// shell "del servidor" arranca con la integración: prompt de Garbo con su shell, lee sus
    /// archivos de inicio de sesión, borra la carpeta temporal, y un agente que se lance allí se
    /// reconoce con la pestaña como remota.
    #[cfg(target_os = "linux")]
    fn prueba_ssh(shell: Shell) {
        use crate::blocks::BlockState;
        use alacritty_terminal::event::WindowSize;
        use alacritty_terminal::event_loop::Msg;
        use alacritty_terminal::sync::FairMutex;
        use alacritty_terminal::term::{test::TermSize, Term};
        use alacritty_terminal::tty;
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        if !installed(shell.name()) || !installed("python3") {
            eprintln!("{} o python3 no están instalados: se omite", shell.name());
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "garbo-prueba-ssh-{}-{}",
            shell.name(),
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let (home, bin, tmp) = (dir.join("home"), dir.join("bin"), dir.join("tmp"));
        for d in [&home.join(".config/fish"), &bin, &tmp] {
            std::fs::create_dir_all(d).unwrap();
        }
        let write = |p: std::path::PathBuf, body: &str, exec: bool| {
            std::fs::write(&p, body).unwrap();
            if exec {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        };
        // Archivos de inicio de sesión "del servidor": dejan una variable que se mira después.
        write(home.join(".bash_profile"), "export DE_INICIO=si\n", false);
        write(home.join(".zprofile"), "export DE_INICIO=si\n", false);
        write(
            home.join(".config/fish/config.fish"),
            "set -gx DE_INICIO si\n",
            false,
        );
        // El `ssh` falso: `-G` responde como el de verdad; si no, la última palabra es la orden.
        write(
            bin.join("ssh"),
            r#"#!/usr/bin/python3 -I
import os, pty, sys, select, signal, fcntl, termios, tty
if '-G' in sys.argv:
    print('hostname servidor'); sys.exit(0)
cmd = sys.argv[-1]
pid, fd = pty.fork()
if pid == 0:
    os.execv(os.environ['SHELL'], [os.environ['SHELL'], '-c', cmd])
def size(*_):
    try: fcntl.ioctl(fd, termios.TIOCSWINSZ, fcntl.ioctl(0, termios.TIOCGWINSZ, b'\0' * 8))
    except OSError: pass
size(); signal.signal(signal.SIGWINCH, size)
tty.setraw(0)
while True:
    r, _, _ = select.select([fd, 0], [], [])
    if fd in r:
        try: os.write(1, os.read(fd, 65536))
        except OSError: break
    if 0 in r: os.write(fd, os.read(0, 4096))
"#,
            true,
        );
        // Un agente "en el servidor".
        write(
            bin.join("claude"),
            "#!/bin/sh\necho 'agente de prueba'\nsleep 30\n",
            true,
        );

        // `ssh.sh` y `remoto.b64` donde los deja la integración.
        let rc = Shell::Bash.injection(None).unwrap().args[1].clone();
        let ssh_sh = std::path::Path::new(&rc).with_file_name("ssh.sh");
        let shell_path = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|d| d.join(shell.name()))
            .find(|p| p.is_file())
            .unwrap();
        let env = [
            ("HOME", path_str(&home)),
            ("PATH", format!("{}:/usr/bin:/bin", path_str(&bin))),
            ("SHELL", path_str(&shell_path)),
            ("TMPDIR", path_str(&tmp)),
            ("TERM", "xterm-256color".into()),
            ("GARBO_SSH", "1".into()),
            ("GARBO_PROMPT", "1".into()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
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
            shell: Some(tty::Shell::new(
                "sh".into(),
                vec![
                    path_str(&ssh_sh),
                    "ssh".into(),
                    "-p".into(),
                    "22".into(),
                    "usuario@servidor".into(),
                ],
            )),
            working_directory: Some(home.clone()),
            drain_on_exit: false,
            env,
        };
        let pty = tty::new(&opts, ws, 0).unwrap();
        let probe = crate::platform::Probe::new(&pty);
        let blocks = Arc::new(Mutex::new(BlockState::new(crate::platform::child_pid(
            &pty,
        ))));
        let lp =
            crate::pty_loop::PtyLoop::new(term.clone(), responder.clone(), pty, blocks.clone())
                .unwrap();
        let tx = lp.sender();
        *responder.0.lock().unwrap() = Some(tx.clone());
        lp.spawn();

        let wait = |what: &str, ok: &dyn Fn() -> bool| {
            let end = Instant::now() + Duration::from_secs(15);
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
                                .into()
                        })
                        .collect();
                    panic!(
                        "ssh con {}: no llegó «{what}». Bloques: {:?}\n{}",
                        shell.name(),
                        blocks.lock().unwrap().summary(),
                        screen.join("\n")
                    );
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        let send = |s: &str| tx.send(Msg::Input(s.as_bytes().to_vec().into()));
        let screen_has = |s: &str| {
            let t = term.lock();
            (0..24).any(|l| {
                let row = &t.grid()[alacritty_terminal::index::Line(l)];
                row[..].iter().map(|c| c.c).collect::<String>().contains(s)
            })
        };

        // El prompt del servidor, con la integración: el de Garbo, y diciendo qué shell es.
        wait("el prompt de Garbo del servidor", &|| {
            let bs = blocks.lock().unwrap();
            bs.input_start.is_some() && bs.prompt_shell == Some(shell)
        });
        assert!(blocks.lock().unwrap().summary().iter().any(|b| b.0));
        // Lo que ve la pestaña: una sesión remota (el `ssh` en primer plano).
        assert!(probe
            .foreground()
            .is_some_and(crate::sessions::is_remote_session));
        // Sus archivos de inicio de sesión, y nada en la carpeta temporal.
        send("echo inicio=$DE_INICIO\r");
        wait("los archivos de inicio de sesión", &|| {
            screen_has("inicio=si")
        });
        assert_eq!(
            std::fs::read_dir(&tmp).unwrap().count(),
            0,
            "carpeta temporal"
        );

        // Un agente lanzado en el servidor: se sabe cuál es por lo que se escribió.
        send("claude\r");
        wait("el agente", &|| screen_has("agente de prueba"));
        let running = blocks.lock().unwrap().running_command().map(str::to_owned);
        assert_eq!(running.as_deref(), Some("claude"));
        let mut session = crate::sessions::Session::default();
        let now = Instant::now();
        session.refresh(Some(&probe), now, true, now, None, running.as_deref());
        assert!(session.remote);
        assert_eq!(session.agent.map(|a| a.name), Some("Claude Code"));

        tx.send(Msg::Shutdown);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ssh_con_bash() {
        prueba_ssh(Shell::Bash);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ssh_con_zsh() {
        prueba_ssh(Shell::Zsh);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ssh_con_fish() {
        prueba_ssh(Shell::Fish);
    }

    /// Lo que lleva Garbo al servidor: los cuatro scripts dentro, sin huecos sin rellenar, y ninguno
    /// que cierre antes de tiempo el `cat <<'__GARBO_FIN__'` que los escribe.
    #[test]
    fn arranque_remoto_completo() {
        let boot = remote_boot();
        assert!(!boot.contains("@GARBO_"), "huecos sin rellenar");
        for script in [
            include_str!("../shell/garbo.bash"),
            include_str!("../shell/garbo.zsh"),
            include_str!("../shell/zsh/.zshenv"),
            include_str!("../shell/garbo.fish"),
        ] {
            assert!(boot.contains(script.trim_end()));
            assert!(!script.lines().any(|l| l == "__GARBO_FIN__"));
        }
        use base64::Engine;
        let back = base64::engine::general_purpose::STANDARD
            .decode(remote_boot_b64())
            .unwrap();
        assert_eq!(String::from_utf8(back).unwrap(), boot);
        // Viaja entre comillas simples: nada que las cierre ni barras que fish leería.
        assert!(!remote_boot_b64().contains(['\'', '\\', '\n']));
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

    /// PowerShell de verdad por ConPTY, con la integración y sin el perfil del usuario: bloques con
    /// su código (programa que falla y error de PowerShell), la carpeta tras `cd` (OSC 9;9) y el
    /// comando guardado. Con y sin el prompt de Garbo.
    #[cfg(windows)]
    fn prueba_powershell(program: &str, garbo_prompt: bool) {
        use crate::blocks::BlockState;
        use alacritty_terminal::event::WindowSize;
        use alacritty_terminal::event_loop::Msg;
        use alacritty_terminal::sync::FairMutex;
        use alacritty_terminal::term::{test::TermSize, Term};
        use alacritty_terminal::tty;
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        let Some(exe) = crate::platform::find_program(program) else {
            eprintln!("{program} no está instalado: se omite");
            return;
        };
        let mut inj = Shell::PowerShell.injection(None).unwrap();
        inj.args.insert(0, "-NoProfile".into());
        let env = [
            ("TERM", "xterm-256color"),
            ("GARBO_AUTOSUGGEST", "1"),
            ("GARBO_PROMPT", if garbo_prompt { "1" } else { "" }),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let responder = Responder(Default::default());
        let term = Arc::new(FairMutex::new(Term::new(
            Default::default(),
            &TermSize::new(100, 30),
            responder.clone(),
        )));
        let ws = WindowSize {
            num_lines: 30,
            num_cols: 100,
            cell_width: 10,
            cell_height: 20,
        };
        let home = dirs::home_dir().unwrap();
        let opts = tty::Options {
            shell: Some(tty::Shell::new(path_str(&exe), inj.args)),
            working_directory: Some(home.clone()),
            drain_on_exit: false,
            env,
            escape_args: true,
        };
        let pty = tty::new(&opts, ws, 0).unwrap();
        let blocks = Arc::new(Mutex::new(BlockState::new(crate::platform::child_pid(
            &pty,
        ))));
        let lp =
            crate::pty_loop::PtyLoop::new(term.clone(), responder.clone(), pty, blocks.clone())
                .unwrap();
        let tx = lp.sender();
        *responder.0.lock().unwrap() = Some(tx.clone());
        lp.spawn();

        let wait = |what: &str, ok: &dyn Fn() -> bool| {
            let end = Instant::now() + Duration::from_secs(20);
            while !ok() {
                if Instant::now() > end {
                    let t = term.lock();
                    let screen: Vec<String> = (0..30)
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
                        "{program}: no llegó «{what}». Bloques: {:?}\n{}",
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
        let codes = || -> Vec<Option<i32>> {
            blocks
                .lock()
                .unwrap()
                .summary()
                .iter()
                .map(|b| b.2)
                .collect()
        };

        wait("el prompt", &|| typed().as_deref() == Some(""));
        // Teclas como las manda `input::win32_key`: Shift, Ctrl y Alt solos (de los dos lados) no
        // escriben nada (escribían "@") y los números del teclado numérico con Bloq Num se escriben
        // (eran Fin, Supr…). AltGr+2 da "@" (Ctrl izquierdo + Alt derecho, como en Windows).
        let key = |mapped: u32, sc: u32, text: &str, state: u32| {
            let vk = crate::input::win32_vk(mapped, sc, text);
            tx.send(Msg::Input(
                crate::input::win32_sequence(vk, sc & 0xFF, text, state).into(),
            ))
        };
        key(0xA0, 0x2A, "", 0x10);
        key(0xA1, 0x36, "", 0x10);
        key(0xA4, 0x38, "", 0x02);
        key(0xA5, 0xE038, "", 0x101);
        key(0xA2, 0x1D, "", 0x08);
        key(0xA3, 0xE01D, "", 0x104);
        key(0x23, 0x4F, "1", 0x20);
        key(0x2E, 0x53, ".", 0x20);
        key(0x22, 0x51, "3", 0x20);
        key(0x32, 0x03, "@", 0x09);
        key(0xBE, 0x34, ".", 0);
        key(0xBD, 0x35, "-", 0);
        wait("los números del teclado numérico", &|| {
            typed().as_deref() == Some("1.3@.-")
        });
        key(0x1B, 0x01, "\x1b", 0);
        wait("la línea vacía", &|| typed().as_deref() == Some(""));
        assert!(
            blocks.lock().unwrap().win32_input,
            "{program}: ConPTY pide win32-input-mode"
        );
        if garbo_prompt {
            let start = blocks.lock().unwrap().input_start;
            assert_eq!(start.map(|s| s.1), Some(0), "{program}: desde la columna 0");
        }
        // Shift+Intro como evento de Windows (lo que manda `input::win32_key`): nueva línea sin
        // ejecutar; Intro ejecuta las dos líneas en un solo bloque.
        let shift_enter = crate::input::win32_sequence(0x0D, 0x1C, "\r", 0x10);
        let cursor_line = || {
            let t = term.lock();
            let row = &t.grid()[t.grid().cursor.point.line];
            row[..]
                .iter()
                .map(|c| c.c)
                .collect::<String>()
                .trim_end()
                .to_string()
        };
        send("echo uno");
        wait("la primera línea", &|| {
            typed().as_deref() == Some("echo uno")
        });
        tx.send(Msg::Input(shift_enter.into()));
        send("echo dos");
        // Con el prompt de Garbo, la segunda línea desde la primera columna; si no, tras ">> ".
        let second = if garbo_prompt {
            "echo dos"
        } else {
            ">> echo dos"
        };
        wait("la segunda línea", &|| cursor_line() == second);
        assert!(
            codes().iter().all(|c| c.is_none()),
            "{program}: Shift+Intro ejecutó el comando: {:?}",
            blocks.lock().unwrap().summary()
        );
        send("\r");
        wait("las dos líneas", &|| codes().contains(&Some(0)));
        wait("el prompt tras las dos líneas", &|| {
            typed().as_deref() == Some("")
        });
        assert_eq!(blocks.lock().unwrap().shell_cwd(), Some(home.clone()));
        send("echo garbo-");
        wait("lo escrito", &|| typed().as_deref() == Some("echo garbo-"));
        let tag = format!("ps{}", garbo_prompt as u8);
        send(&format!("{tag}\r"));
        wait("el código 0", &|| {
            codes().iter().filter(|c| **c == Some(0)).count() >= 2
        });
        wait("el segundo prompt", &|| typed().as_deref() == Some(""));
        send("cmd /c exit 3\r");
        wait("el código de un programa", &|| codes().contains(&Some(3)));
        wait("el tercer prompt", &|| typed().as_deref() == Some(""));
        send("Get-Item C:\\garbo-no-existe\r");
        wait("el código de un error de PowerShell", &|| {
            codes().contains(&Some(1))
        });
        wait("el cuarto prompt", &|| typed().as_deref() == Some(""));
        send("cd C:\\Windows\r");
        wait("la carpeta nueva", &|| {
            blocks.lock().unwrap().shell_cwd() == Some(std::path::PathBuf::from(r"C:\Windows"))
        });
        wait("el quinto prompt", &|| typed().as_deref() == Some(""));
        // Pegar varias líneas (lo que manda Garbo al prompt de PowerShell): quedan escritas sin
        // ejecutarse; Esc las borra.
        tx.send(Msg::Input(
            crate::input::powershell_paste("echo tres\r\necho cuatro\r\n").into(),
        ));
        let fourth = if garbo_prompt {
            "echo cuatro"
        } else {
            ">> echo cuatro"
        };
        wait("lo pegado", &|| cursor_line() == fourth);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            codes().iter().filter(|c| c.is_some()).count(),
            5,
            "{program}: lo pegado se ejecutó: {:?}",
            blocks.lock().unwrap().summary()
        );
        key(0x1B, 0x01, "\x1b", 0);
        wait("la línea vacía tras pegar", &|| {
            typed().as_deref() == Some("")
        });
        let summary = blocks.lock().unwrap().summary();
        tx.send(Msg::Shutdown);

        let ran: Vec<_> = summary.iter().filter(|b| b.1).map(|b| b.2).collect();
        assert_eq!(
            ran,
            [Some(0), Some(0), Some(3), Some(1), Some(0)],
            "{program}: {summary:?}"
        );
        assert!(summary.iter().all(|b| b.0 == garbo_prompt), "{summary:?}");
        let hist = std::fs::read_to_string(crate::suggest::history_path_for_tests()).unwrap();
        let home_s = home.to_string_lossy();
        assert!(
            hist.lines()
                .any(|l| l.contains(&*home_s) && l.ends_with(&format!("\techo garbo-{tag}"))),
            "{program}: {hist}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn powershell_real() {
        prueba_powershell("powershell.exe", false);
        prueba_powershell("powershell.exe", true);
        prueba_powershell("pwsh.exe", false);
    }

    #[test]
    fn comillas_de_fish() {
        assert_eq!(fish_quote("/run/a b/garbo.fish"), "'/run/a b/garbo.fish'");
        assert_eq!(fish_quote(r"o'k\x"), r"'o\'k\\x'");
    }
}
