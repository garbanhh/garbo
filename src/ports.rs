//! Puertos en escucha de la máquina, para el panel lateral.
//! Lee /proc/net/tcp{,6}; para los puertos de tus procesos averigua el programa y prueba si
//! responden HTTP (servidores web). El hilo solo vive mientras la sección está abierta.

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

struct Listen {
    port: u16,
    inode: u64,
    uid: u32,
    v6: bool,
}

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

fn scan(cache: &mut HashMap<(u16, u64), bool>) -> Vec<PortInfo> {
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

    let mut by_port: HashMap<u16, PortInfo> = HashMap::new();
    for s in &socks {
        let process = owners.get(&s.inode).cloned();
        let web = s.uid == me
            && *cache
                .entry((s.port, s.inode))
                .or_insert_with(|| probe_http(s.port, s.v6));
        let e = by_port.entry(s.port).or_insert(PortInfo {
            port: s.port,
            web: false,
            process: None,
        });
        e.web |= web;
        if e.process.is_none() {
            e.process = process;
        }
    }
    let mut v: Vec<PortInfo> = by_port.into_values().collect();
    v.sort_by_key(|p| (!p.web, p.port));
    v
}

/// inode de socket -> nombre del programa, mirando /proc/<pid>/fd de tus procesos.
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
        let Some(pid) = name
            .to_str()
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
        else {
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
                    out.entry(n).or_insert_with(|| process_label(pid));
                }
            }
        }
        if out.len() == inodes.len() {
            break;
        }
    }
    out
}

/// "node · vite", "python3 · http.server", "postgres"
fn process_label(pid: &str) -> String {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .unwrap_or_default()
        .trim()
        .to_string();
    let interpreters = [
        "node", "python", "python3", "bun", "deno", "ruby", "java", "php", "perl",
    ];
    if interpreters.contains(&comm.as_str()) {
        let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        let script = cmd
            .split(|&b| b == 0)
            .skip(1)
            .map(|a| String::from_utf8_lossy(a).to_string())
            .find(|a| !a.starts_with('-') && !a.is_empty())
            .map(|a| a.rsplit('/').next().unwrap_or_default().to_string());
        if let Some(s) = script {
            return format!("{comm} · {s}");
        }
    }
    comm
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

    #[test]
    fn lee_proc_net_tcp() {
        let t = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 12345 1 0\n   1: 0100007F:1F91 0100007F:9C40 01 00000000:00000000 00:00000000 00000000  1000        0 999 1 0\n";
        let mut v = Vec::new();
        parse_proc_net(t, false, &mut v);
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].port, v[0].inode, v[0].uid), (8080, 12345, 1000));
    }
}
