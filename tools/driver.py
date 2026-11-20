#!/usr/bin/env python3
# Uso: tools/driver.py <bash|zsh|fish> "comando 1" "comando 2" ...
#      (o, como antes, tools/driver.py shell/garbo.bash "comando 1" ...)
# Se usa como [shell] program de una config de prueba para verificar los bloques de comandos.
# Usuario simulado: el shell real (con la integración de Garbo de este repositorio, shell/) en un
# PTY interno, teclea comandos y reenvía la salida tal cual a Garbo.
import os, pty, sys, time, select, signal, fcntl, termios, tty
here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "shell")
which = sys.argv[1]; cmds = sys.argv[2:]
pid, fd = pty.fork()
if pid == 0:
    if which == "zsh":
        if "ZDOTDIR" in os.environ:
            os.environ["GARBO_ZSH_ZDOTDIR"] = os.environ["ZDOTDIR"]
        os.environ["ZDOTDIR"] = os.path.join(here, "zsh")
        os.execvp("zsh", ["zsh", "-i"])
    if which == "fish":
        os.execvp("fish", ["fish", "-i", "--init-command", "source " + os.path.join(here, "garbo.fish")])
    rc = which if which.endswith(".bash") else os.path.join(here, "garbo.bash")
    os.environ["PS1"] = r"\u@\h:\w$ "
    os.execvp("bash", ["bash", "--rcfile", rc, "-i"])
def sync_size(*_):
    try: fcntl.ioctl(fd, termios.TIOCSWINSZ, fcntl.ioctl(1, termios.TIOCGWINSZ, b"\0" * 8))
    except Exception: pass
signal.signal(signal.SIGWINCH, sync_size)
try: tty.setraw(0)
except Exception: pass
def pump(t):
    end = time.time() + t
    while time.time() < end:
        try: r, _, _ = select.select([fd], [], [], 0.05)
        except InterruptedError: continue
        if fd in r:
            try: os.write(1, os.read(fd, 65536))
            except OSError: sys.exit(0)
time.sleep(0.4); sync_size(); pump(0.8)
for c in cmds:
    for ch in c:
        os.write(fd, ch.encode()); pump(0.015)
    os.write(fd, b"\r"); pump(0.9)
while True:
    try: r, _, _ = select.select([fd, 0], [], [])
    except InterruptedError: continue
    if fd in r:
        try: os.write(1, os.read(fd, 65536))
        except OSError: break
    if 0 in r: os.write(fd, os.read(0, 1024))
