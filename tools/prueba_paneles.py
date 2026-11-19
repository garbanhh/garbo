#!/usr/bin/env python3
"""Comprueba el repintado de un panel sin foco mediante los PTY y una ventana X11."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import tty


def terminal(work):
    tty.setraw(0)
    try:
        (work / "primer-panel").touch(exist_ok=False)
    except FileExistsError:
        print(
            "\033]0;tienda-web\007"
            "\033[1mcargo test\033[0m\r\n\r\n"
            "running 3 tests\r\n"
            "test carrito::aplica_descuento ... \033[32mok\033[0m\r\n"
            "test carrito::conserva_productos ... \033[32mok\033[0m\r\n"
            "test pago::rechaza_tarjeta ... \033[32mok\033[0m\r\n\r\n"
            "\033[32mtest result: ok\033[0m. 3 passed; 0 failed\r\n",
            flush=True,
        )
        (work / "segundo-panel").touch()
        if os.read(0, 1) == b"x":
            (work / "foco-confirmado").touch()
    else:
        print(
            "\033[1mAgente · tienda-web\033[0m\r\n\r\n"
            "❯ Revisa el cálculo de descuentos del carrito.\r\n\r\n"
            "  Encontré el caso que fallaba al quitar un\r\n"
            "  producto. Ajusté el cálculo y añadí una prueba.\r\n",
            flush=True,
        )
        while not (work / "continuar").exists():
            time.sleep(0.02)
        print("\r\n\033[38;2;0;255;140m  ✓ Cambios listos para revisar\033[0m\r\n", flush=True)
    while os.read(0, 4096):
        pass


def wait_for(description, condition, app):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline and app.poll() is None:
        if result := condition():
            return result
        time.sleep(0.05)
    raise RuntimeError(f"No se pudo verificar: {description}")


def frame(path):
    try:
        data = path.read_bytes()
        magic, size, depth, pixels = data.split(b"\n", 3)
        width, height = map(int, size.split())
    except (FileNotFoundError, ValueError):
        return None
    if magic == b"P6" and depth == b"255" and len(pixels) == width * height * 3:
        return width, height, pixels, data
    return None


def has_marker(capture):
    width, height, pixels, _ = capture
    # Busca el verde turquesa del marcador, incluso con la atenuación del panel sin foco.
    for row in range(height):
        start = 3 * row * width
        left = pixels[start : start + 3 * (width // 2)]
        if any(r < 40 and g > 125 and 60 < b < g for r, g, b in zip(left[::3], left[1::3], left[2::3])):
            return True
    return False


def check(binary, capture_path):
    with tempfile.TemporaryDirectory(prefix="garbo-paneles-") as directory:
        work = Path(directory)
        config = work / "config" / "garbo"
        config.mkdir(parents=True)
        (config / "config.toml").write_text(
            'check_updates = false\n[window]\ncolumns = 138\nlines = 15\n'
            '[terminal]\ncursor_blink = false\nautosuggest = false\n'
            f'[shell]\nprogram = {json.dumps(sys.executable)}\n'
            f'args = {json.dumps([str(Path(__file__).resolve()), "--terminal", directory])}\n',
            encoding="utf-8",
        )
        screenshot = work / "frame.ppm"
        runtime = work / "runtime"
        runtime.mkdir(mode=0o700)
        env = os.environ.copy()
        env.pop("WAYLAND_DISPLAY", None)
        env.pop("WAYLAND_SOCKET", None)
        env.update(
            HOME=directory,
            XDG_CONFIG_HOME=str(config.parent),
            XDG_CACHE_HOME=str(work / "cache"),
            XDG_DATA_HOME=str(work / "data"),
            XDG_RUNTIME_DIR=str(runtime),
            GARBO_SCREENSHOT=str(screenshot),
        )
        project = work / "tienda-web"
        project.mkdir()
        with subprocess.Popen([binary], cwd=project, env=env) as app:
            try:
                wait_for("el primer panel y su fotograma", lambda: (work / "primer-panel").exists() and frame(screenshot), app)
                window = subprocess.check_output(
                    ["xdotool", "search", "--onlyvisible", "--pid", str(app.pid)], text=True
                ).splitlines()[0]
                subprocess.run(["xdotool", "windowfocus", "--sync", window, "key", "ctrl+shift+d"], check=True)
                wait_for("el segundo PTY está listo", (work / "segundo-panel").exists, app)
                subprocess.run(["xdotool", "key", "x"], check=True)
                wait_for("el segundo panel recibe la entrada", (work / "foco-confirmado").exists, app)
                # Deja terminar el repintado del foco y del teclado antes de liberar la salida.
                time.sleep(0.5)
                before = wait_for("el fotograma con los dos paneles", lambda: frame(screenshot), app)
                if has_marker(before):
                    raise RuntimeError("El marcador ya estaba en el fotograma inicial")
                (work / "continuar").touch()

                def updated():
                    capture = frame(screenshot)
                    return capture if capture and has_marker(capture) else None

                after = wait_for("el primer panel repinta su salida sin recibir el foco", updated, app)
                if capture_path:
                    capture_path.write_bytes(after[3])
            finally:
                app.terminate()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", nargs="?", type=Path, default=Path("target/debug/garbo"))
    parser.add_argument("--capture", type=Path, help="Ruta opcional del fotograma PPM")
    parser.add_argument("--terminal", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.terminal:
        terminal(args.terminal)
    else:
        check(args.binary.resolve(), args.capture)
        print("El panel visible sin foco se actualizó.")
