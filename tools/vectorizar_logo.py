#!/usr/bin/env python3
"""Vectoriza el logo de Garbo (colores planos sobre fondo oscuro) a SVG.

Uso: tools/vectorizar_logo.py LOGO.png LOGO_CON_NOMBRE.png [--vista DIR]

Escribe:
  packaging/io.github.garbanhh.Garbo.svg  icono de la aplicación (logo sobre una baldosa oscura)
  assets/garbo-banner.svg                 cabecera del README (logo y nombre)
Con --vista, además, PNG de muestra del icono (256, 64, 32, 16 px) y de la cabecera.

Cada color (naranja, blanco, verde) se separa del fondo, se siguen los bordes de sus píxeles y se
simplifican (Ramer-Douglas-Peucker) a polígonos. Los huecos (la A, la R…) salen con evenodd.
"""
import math
import os
import sys

from PIL import Image, ImageChops, ImageDraw

RAIZ = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
COLORES = {  # medidos en las imágenes originales
    "naranja": ((255, 122, 6), "#ff7a06"),
    "blanco": ((254, 253, 254), "#ffffff"),
    "verde": ((105, 252, 9), "#69fc09"),
}
FONDO = (3, 4, 10)
FONDO_HEX = "#05060e"
TOLERANCIA = 0.9  # píxeles de desviación al simplificar


def mascaras(img):
    """Píxel -> color del logo, si su mezcla con el fondo es al menos a medias de ese color."""
    w, h = img.size
    px = img.load()
    dirs = {k: tuple(c[i] - FONDO[i] for i in range(3)) for k, (c, _) in COLORES.items()}
    norm = {k: sum(t * t for t in d) for k, d in dirs.items()}
    out = {k: bytearray(w * h) for k in COLORES}
    for y in range(h):
        for x in range(w):
            p = px[x, y]
            v = (p[0] - FONDO[0], p[1] - FONDO[1], p[2] - FONDO[2])
            if v[0] + v[1] + v[2] < 120:
                continue
            mejor, alfa = None, 0.5
            for k, d in dirs.items():
                a = (v[0] * d[0] + v[1] * d[1] + v[2] * d[2]) / norm[k]
                if a > alfa:
                    resto = sum((v[i] - a * d[i]) ** 2 for i in range(3))
                    if resto < 80 * 80:
                        mejor, alfa = k, a
            if mejor:
                out[mejor][y * w + x] = 1
    return out


def contornos(m, w, h):
    """Bordes de los píxeles marcados, como lazos cerrados de esquinas de píxel."""
    sig = {}

    def dentro(x, y):
        return 0 <= x < w and 0 <= y < h and m[y * w + x]

    def arista(a, b):
        sig.setdefault(a, []).append(b)

    for y in range(h):
        fila = y * w
        for x in range(w):
            if not m[fila + x]:
                continue
            if not dentro(x, y - 1):
                arista((x, y), (x + 1, y))
            if not dentro(x + 1, y):
                arista((x + 1, y), (x + 1, y + 1))
            if not dentro(x, y + 1):
                arista((x + 1, y + 1), (x, y + 1))
            if not dentro(x - 1, y):
                arista((x, y + 1), (x, y))
    lazos = []
    while sig:
        inicio = next(iter(sig))
        lazo, cur = [inicio], inicio
        while True:
            salidas = sig.get(cur)
            if not salidas:
                break
            nxt = salidas.pop()
            if not salidas:
                del sig[cur]
            cur = nxt
            if cur == inicio:
                break
            lazo.append(cur)
        if len(lazo) >= 4:
            lazos.append(lazo)
    return lazos


def area(p):
    return abs(sum(p[i][0] * p[i - 1][1] - p[i - 1][0] * p[i][1] for i in range(len(p)))) / 2


def rdp(pts, eps):
    if len(pts) < 3:
        return pts
    (x1, y1), (x2, y2) = pts[0], pts[-1]
    dx, dy = x2 - x1, y2 - y1
    largo = math.hypot(dx, dy) or 1e-9
    peor, idx = 0.0, 0
    for i in range(1, len(pts) - 1):
        d = abs(dy * (pts[i][0] - x1) - dx * (pts[i][1] - y1)) / largo
        if d > peor:
            peor, idx = d, i
    if peor <= eps:
        return [pts[0], pts[-1]]
    return rdp(pts[: idx + 1], eps)[:-1] + rdp(pts[idx:], eps)


def simplificar(lazo):
    # Puntos medios de las aristas (quita la mitad del escalonado) y luego RDP en dos mitades.
    n = len(lazo)
    pts = [((lazo[i][0] + lazo[(i + 1) % n][0]) / 2, (lazo[i][1] + lazo[(i + 1) % n][1]) / 2) for i in range(n)]
    lejos = max(range(n), key=lambda i: (pts[i][0] - pts[0][0]) ** 2 + (pts[i][1] - pts[0][1]) ** 2)
    a = rdp(pts[: lejos + 1], TOLERANCIA)
    b = rdp(pts[lejos:] + [pts[0]], TOLERANCIA)
    return a[:-1] + b[:-1]


def vectorizar(ruta):
    img = Image.open(ruta).convert("RGB")
    w, h = img.size
    capas = {}
    for k, m in mascaras(img).items():
        lazos = [simplificar(l) for l in contornos(m, w, h)]
        capas[k] = [l for l in lazos if len(l) >= 3 and area(l) >= 30]
    return capas


def caja(capas):
    xs = [p[0] for ls in capas.values() for l in ls for p in l]
    ys = [p[1] for ls in capas.values() for l in ls for p in l]
    return min(xs), min(ys), max(xs), max(ys)


def ruta_svg(lazos, f):
    partes = []
    for l in lazos:
        q = [f(p) for p in l]
        partes.append("M" + " ".join(f"{x:.1f} {y:.1f}" for x, y in q) + "Z")
    return "".join(partes)


def svg(capas, ancho, alto, f, fondo, titulo):
    """`f`: coordenadas de la imagen -> del SVG. `fondo`: (x, y, w, h, radio) de la baldosa."""
    x, y, w, h, r = fondo
    filas = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{ancho:g}" height="{alto:g}" viewBox="0 0 {ancho:g} {alto:g}">',
        f"<title>{titulo}</title>",
        f'<rect x="{x:g}" y="{y:g}" width="{w:g}" height="{h:g}" rx="{r:g}" fill="{FONDO_HEX}"/>',
    ]
    for k in ("naranja", "blanco", "verde"):
        if capas.get(k):
            filas.append(f'<path fill="{COLORES[k][1]}" fill-rule="evenodd" d="{ruta_svg(capas[k], f)}"/>')
    filas.append("</svg>")
    return "\n".join(filas) + "\n"


def icono(capas):
    # Lienzo de 128 (rejilla de iconos de GNOME): baldosa de 112 con márgenes de 8 y el logo al 78 %.
    x0, y0, x1, y1 = caja(capas)
    lado = max(x1 - x0, y1 - y0)
    s = 112 * 0.78 / lado
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    f = lambda p: (64 + (p[0] - cx) * s, 64 + (p[1] - cy) * s)
    return svg(capas, 128, 128, f, (8, 8, 112, 112, 24), "Garbo"), f


def cabecera(capas):
    x0, y0, x1, y1 = caja(capas)
    alto_logo = y1 - y0
    margen = alto_logo * 0.42
    s = 160 / (alto_logo + 2 * margen)  # 160 de alto
    ancho = (x1 - x0 + 2 * margen * 1.3) * s
    f = lambda p: ((p[0] - x0 + margen * 1.3) * s, (p[1] - y0 + margen) * s)
    return svg(capas, round(ancho), 160, f, (0, 0, round(ancho), 160, 20), "Garbo"), f, round(ancho)


def pintar(capas, f, ancho, alto, fondo, tam):
    """Muestra en PNG (evenodd con XOR de cada lazo), supermuestreada x4."""
    k = 4 * tam / ancho
    big = Image.new("RGBA", (round(ancho * k), round(alto * k)), (0, 0, 0, 0))
    x, y, w, h, r = fondo
    ImageDraw.Draw(big).rounded_rectangle([x * k, y * k, (x + w) * k, (y + h) * k], r * k, fill=FONDO + (255,))
    for nombre in ("naranja", "blanco", "verde"):
        mask = Image.new("1", big.size, 0)
        for l in capas.get(nombre, []):
            capa = Image.new("1", big.size, 0)
            ImageDraw.Draw(capa).polygon([(px * k, py * k) for px, py in map(f, l)], fill=1)
            mask = ImageChops.logical_xor(mask, capa)
        big.paste(COLORES[nombre][0] + (255,), (0, 0), mask)
    return big.resize((tam, round(alto * tam / ancho)), Image.LANCZOS)


def main():
    args = sys.argv[1:]
    vista = None
    if "--vista" in args:
        i = args.index("--vista")
        vista = args[i + 1]
        del args[i : i + 2]
    if len(args) != 2:
        sys.exit(__doc__)
    logo, nombre = vectorizar(args[0]), vectorizar(args[1])
    texto, f = icono(logo)
    with open(os.path.join(RAIZ, "packaging/io.github.garbanhh.Garbo.svg"), "w") as fh:
        fh.write(texto)
    os.makedirs(os.path.join(RAIZ, "assets"), exist_ok=True)
    texto_c, fc, ancho = cabecera(nombre)
    with open(os.path.join(RAIZ, "assets/garbo-banner.svg"), "w") as fh:
        fh.write(texto_c)
    for k, ls in logo.items():
        print(f"logo {k}: {len(ls)} lazos, {sum(len(l) for l in ls)} puntos")
    for k, ls in nombre.items():
        print(f"cabecera {k}: {len(ls)} lazos, {sum(len(l) for l in ls)} puntos")
    if vista:
        os.makedirs(vista, exist_ok=True)
        for tam in (256, 64, 32, 16):
            pintar(logo, f, 128, 128, (8, 8, 112, 112, 24), tam).save(os.path.join(vista, f"icono-{tam}.png"))
        pintar(nombre, fc, ancho, 160, (0, 0, ancho, 160, 20), 900).save(os.path.join(vista, "cabecera.png"))


if __name__ == "__main__":
    main()
