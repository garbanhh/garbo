//! Puertos en escucha de la máquina, para el panel lateral.
//! En Linux se leen de /proc/net/tcp{,6}; en Windows, de `GetExtendedTcpTable`, que ya da el
//! proceso dueño. Para los puertos de tus procesos averigua el programa y prueba si responden
//! HTTP (servidores web). El hilo solo vive mientras la sección está abierta.

use crate::terminal::UserEvent;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use winit::event_loop::EventLoopProxy;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortInfo {
    pub port: u16,
    pub web: bool,
    /// Programa que escucha (solo para procesos del usuario).
    pub process: Option<String>,
}

static ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn start(proxy: EventLoopProxy<UserEvent>) {
    if ACTIVE.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("puertos".into())
        .spawn(move || {
            let mut cache: HashMap<(u16, u64), bool> = HashMap::new();
            while ACTIVE.load(Ordering::Relaxed) {
                let list = scan(&mut cache);
                if proxy.send_event(UserEvent::Ports(list)).is_err() {
                    break;
                }
                for _ in 0..30 {
                    if !ACTIVE.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        });
}

pub fn stop() {
    ACTIVE.store(false, Ordering::SeqCst);
}

/// Un socket en escucha.
struct Socket {
    port: u16,
    /// Identifica al dueño para la caché de "¿responde HTTP?" (el inode del socket en Linux, el
    /// proceso en Windows): si otro programa ocupa el puerto, se vuelve a probar.
    key: u64,
    v6: bool,
    /// Es de un proceso del usuario (los demás ni se prueban ni se etiquetan).
    mine: bool,
    process: Option<String>,
}

fn scan(cache: &mut HashMap<(u16, u64), bool>) -> Vec<PortInfo> {
    let mut by_port: HashMap<u16, PortInfo> = HashMap::new();
    for s in sockets() {
        let web = s.mine
            && *cache
                .entry((s.port, s.key))
                .or_insert_with(|| probe_http(s.port, s.v6));
        let e = by_port.entry(s.port).or_insert(PortInfo {
            port: s.port,
            web: false,
            process: None,
        });
        e.web |= web;
        if e.process.is_none() {
            e.process = s.process;
        }
    }
    let mut v: Vec<PortInfo> = by_port.into_values().collect();
    v.sort_by_key(|p| (!p.web, p.port));
    v
}

/// "node · vite", "python3 · http.server", "postgres"
fn process_label(pid: i32) -> Option<String> {
    let comm = crate::platform::process_name(pid)?;
    let interpreters = [
        "node", "python", "python3", "py", "bun", "deno", "ruby", "java", "php", "perl",
    ];
    if interpreters.contains(&comm.to_lowercase().as_str()) {
        let script = crate::platform::process_args(pid)
            .unwrap_or_default()
            .into_iter()
            .skip(1)
            .find(|a| !a.starts_with('-') && !a.is_empty())
            .map(|a| crate::platform::base_name(&a).to_string());
        if let Some(s) = script {
            return Some(format!("{comm} · {s}"));
        }
    }
    Some(comm)
}

#[cfg(target_os = "linux")]
struct Listen {
    port: u16,
    inode: u64,
    uid: u32,
    v6: bool,
}

#[cfg(target_os = "linux")]
fn parse_proc_net(text: &str, v6: bool, out: &mut Vec<Listen>) {
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 || f[3] != "0A" {
            continue; // 0A = LISTEN
        }
        let Some(port) = f[1]
            .rsplit(':')
            .next()
            .and_then(|p| u16::from_str_radix(p, 16).ok())
        else {
            continue;
        };
        let (Ok(uid), Ok(inode)) = (f[7].parse(), f[9].parse()) else {
            continue;
        };
        out.push(Listen {
            port,
            inode,
            uid,
            v6,
        });
    }
}

#[cfg(target_os = "linux")]
fn sockets() -> Vec<Socket> {
    // SAFETY: getuid no tiene efectos secundarios.
    let me = unsafe { libc::getuid() };
    let mut socks = Vec::new();
    for (path, v6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        if let Ok(t) = std::fs::read_to_string(path) {
            parse_proc_net(&t, v6, &mut socks);
        }
    }
    let mine: Vec<u64> = socks
        .iter()
        .filter(|s| s.uid == me)
        .map(|s| s.inode)
        .collect();
    let owners = socket_owners(&mine, me);
    socks
        .into_iter()
        .map(|s| Socket {
            port: s.port,
            key: s.inode,
            v6: s.v6,
            mine: s.uid == me,
            process: owners.get(&s.inode).cloned(),
        })
        .collect()
}

/// inode de socket -> nombre del programa, mirando /proc/<pid>/fd de tus procesos.
#[cfg(target_os = "linux")]
fn socket_owners(inodes: &[u64], me: u32) -> HashMap<u64, String> {
    use std::os::unix::fs::MetadataExt;
    let mut out = HashMap::new();
    if inodes.is_empty() {
        return out;
    }
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return out;
    };
    for p in procs.flatten() {
        let name = p.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<i32>().ok()) else {
            continue;
        };
        if p.metadata().map(|m| m.uid()).ok() != Some(me) {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(p.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(link) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let l = link.to_string_lossy();
            if let Some(n) = l
                .strip_prefix("socket:[")
                .and_then(|r| r.strip_suffix(']'))
                .and_then(|n| n.parse().ok())
            {
                if inodes.contains(&n) {
                    if let Some(label) = process_label(pid) {
                        out.entry(n).or_insert(label);
                    }
                }
            }
        }
        if out.len() == inodes.len() {
            break;
        }
    }
    out
}

/// Tabla de sockets TCP en escucha con su proceso (`MIB_TCPTABLE_OWNER_PID` o la de IPv6).
#[cfg(windows)]
fn tcp_table(family: u16) -> Option<Vec<u32>> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, TCP_TABLE_OWNER_PID_LISTENER,
    };
    let mut size = 0u32;
    // La tabla puede crecer entre la consulta del tamaño y la lectura: se reintenta.
    for _ in 0..4 {
        let mut buf = vec![0u32; (size as usize).div_ceil(4).max(1)];
        let mut len = (buf.len() * 4) as u32;
        // SAFETY: búfer alineado de `len` bytes; la función escribe como mucho eso.
        let r = unsafe {
            GetExtendedTcpTable(
                buf.as_mut_ptr().cast(),
                &mut len,
                0,
                u32::from(family),
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        match r {
            0 => return Some(buf),
            // ERROR_INSUFFICIENT_BUFFER
            122 => size = len + 1024,
            _ => return None,
        }
    }
    None
}

#[cfg(windows)]
fn sockets() -> Vec<Socket> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    // Los procesos del usuario están en su sesión; los servicios del sistema, en la 0.
    let session = |pid: u32| {
        let mut s = 0u32;
        // SAFETY: solo escribe el número de sesión.
        (unsafe { ProcessIdToSessionId(pid, &mut s) } != 0).then_some(s)
    };
    let me = session(std::process::id());
    let mut out = Vec::new();
    let mut labels: HashMap<u32, Option<String>> = HashMap::new();
    for (family, v6) in [(AF_INET, false), (AF_INET6, true)] {
        let Some(table) = tcp_table(family) else {
            continue;
        };
        let n = table[0] as usize;
        let rows = table[1..].as_ptr().cast::<u8>();
        let row_size = if v6 {
            std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>()
        } else {
            std::mem::size_of::<MIB_TCPROW_OWNER_PID>()
        };
        if (table.len() - 1) * 4 < n * row_size {
            continue;
        }
        for i in 0..n {
            // SAFETY: la tabla tiene `n` filas del tamaño indicado tras el contador.
            let (port, pid) = unsafe {
                let p = rows.add(i * row_size);
                if v6 {
                    let r = p.cast::<MIB_TCP6ROW_OWNER_PID>().read_unaligned();
                    (r.dwLocalPort, r.dwOwningPid)
                } else {
                    let r = p.cast::<MIB_TCPROW_OWNER_PID>().read_unaligned();
                    (r.dwLocalPort, r.dwOwningPid)
                }
            };
            // El puerto va en orden de red en los 16 bits bajos.
            let port = u16::from_be(port as u16);
            let mine = pid > 4 && me.is_some() && session(pid) == me;
            let process = if mine {
                labels
                    .entry(pid)
                    .or_insert_with(|| process_label(pid as i32))
                    .clone()
            } else {
                None
            };
            out.push(Socket {
                port,
                key: u64::from(pid),
                v6,
                mine,
                process,
            });
        }
    }
    out
}

#[cfg(not(any(target_os = "linux", windows)))]
fn sockets() -> Vec<Socket> {
    Vec::new()
}

/// ¿Responde HTTP? Petición HEAD mínima con tiempos de espera cortos.
fn probe_http(port: u16, v6: bool) -> bool {
    let addr: SocketAddr = if v6 {
        ([0u16, 0, 0, 0, 0, 0, 0, 1], port).into()
    } else {
        ([127, 0, 0, 1], port).into()
    };
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(200)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_millis(400)));
    let _ = s.set_write_timeout(Some(Duration::from_millis(200)));
    if s.write_all(b"HEAD / HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut buf = [0u8; 8];
    matches!(s.read(&mut buf), Ok(n) if n >= 5 && buf.starts_with(b"HTTP/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn lee_proc_net_tcp() {
        let t = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 12345 1 0\n   1: 0100007F:1F91 0100007F:9C40 01 00000000:00000000 00:00000000 00000000  1000        0 999 1 0\n";
        let mut v = Vec::new();
        parse_proc_net(t, false, &mut v);
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].port, v[0].inode, v[0].uid), (8080, 12345, 1000));
    }

    /// Un servidor HTTP mínimo de esta prueba aparece como web y con su programa.
    #[test]
    fn encuentra_un_servidor_web_propio() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                let mut buf = [0u8; 256];
                let _ = s.read(&mut buf);
                let _ = s.write_all(b"HTTP/1.0 200 OK\r\n\r\n");
            }
        });
        let found = scan(&mut HashMap::new());
        let me = found
            .iter()
            .find(|p| p.port == port)
            .expect("el puerto de la prueba");
        assert!(me.web, "{me:?}");
        assert!(me.process.is_some(), "{me:?}");
    }
}
