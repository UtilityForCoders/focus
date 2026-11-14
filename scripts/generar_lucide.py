"""Genera src/lucide_datos.rs a partir del paquete npm `lucide-static`.

Cada ícono de Lucide es una lista de formas SVG (path, circle, rect, line, polyline, polygon,
ellipse) dibujadas con el mismo trazo. Aquí se aplanan a una sola cadena de datos de `path`, para
que en Orbyt cada ícono sea una sola geometría de Direct2D y una sola llamada de dibujo.

Uso:
    npm pack lucide-static@<versión>  y extraer el .tgz
    python -I scripts/generar_lucide.py <package>/icon-nodes.json <package>/tags.json src/lucide_datos.rs
"""

import json
import re
import sys


def n(v):
    """Número compacto: sin ceros de más."""
    f = round(float(v), 4)
    return str(int(f)) if f == int(f) else repr(f)


NUM = r"[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?"


def absoluto_al_inicio(d):
    """Un `m` al principio de un path es absoluto (regla de SVG), pero al unir varios paths en uno
    dejaría de serlo y se movería al final del anterior. Se reescribe como `M x y` y, si le siguen
    más pares (que tras un `m` son líneas relativas), se les antepone `l`."""
    m = re.match(r"\s*m\s*(" + NUM + r")[\s,]*(" + NUM + r")(.*)$", d, re.S)
    if not m:
        return d
    x, y, resto = m.groups()
    r = resto.lstrip(" ,\t\n")
    if r and (r[0].isdigit() or r[0] in "-+."):
        resto = "l" + r
    return f"M{x} {y}{resto}"


def a_path(etiqueta, a):
    if etiqueta == "path":
        return absoluto_al_inicio(a["d"])
    if etiqueta == "circle":
        cx, cy, r = float(a["cx"]), float(a["cy"]), float(a["r"])
        return f"M{n(cx - r)} {n(cy)}A{n(r)} {n(r)} 0 1 0 {n(cx + r)} {n(cy)}A{n(r)} {n(r)} 0 1 0 {n(cx - r)} {n(cy)}Z"
    if etiqueta == "ellipse":
        cx, cy, rx, ry = (float(a[k]) for k in ("cx", "cy", "rx", "ry"))
        return f"M{n(cx - rx)} {n(cy)}A{n(rx)} {n(ry)} 0 1 0 {n(cx + rx)} {n(cy)}A{n(rx)} {n(ry)} 0 1 0 {n(cx - rx)} {n(cy)}Z"
    if etiqueta == "line":
        return f"M{n(a['x1'])} {n(a['y1'])}L{n(a['x2'])} {n(a['y2'])}"
    if etiqueta in ("polyline", "polygon"):
        nums = a["points"].replace(",", " ").split()
        pares = [f"{n(nums[i])} {n(nums[i + 1])}" for i in range(0, len(nums) - 1, 2)]
        d = "M" + "L".join(pares)
        return d + "Z" if etiqueta == "polygon" else d
    if etiqueta == "rect":
        x, y = float(a.get("x", 0)), float(a.get("y", 0))
        w, h = float(a["width"]), float(a["height"])
        rx = float(a.get("rx", a.get("ry", 0)))
        ry = float(a.get("ry", rx))
        rx, ry = min(rx, w / 2), min(ry, h / 2)
        if rx == 0 and ry == 0:
            return f"M{n(x)} {n(y)}H{n(x + w)}V{n(y + h)}H{n(x)}Z"
        return (
            f"M{n(x + rx)} {n(y)}H{n(x + w - rx)}A{n(rx)} {n(ry)} 0 0 1 {n(x + w)} {n(y + ry)}"
            f"V{n(y + h - ry)}A{n(rx)} {n(ry)} 0 0 1 {n(x + w - rx)} {n(y + h)}"
            f"H{n(x + rx)}A{n(rx)} {n(ry)} 0 0 1 {n(x)} {n(y + h - ry)}"
            f"V{n(y + ry)}A{n(rx)} {n(ry)} 0 0 1 {n(x + rx)} {n(y)}Z"
        )
    raise ValueError(f"forma SVG no soportada: {etiqueta}")


def cadena_rust(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def main():
    nodos_ruta, tags_ruta, salida = sys.argv[1:4]
    with open(nodos_ruta, encoding="utf-8") as f:
        nodos = json.load(f)
    with open(tags_ruta, encoding="utf-8") as f:
        tags = json.load(f)

    filas = []
    for nombre in sorted(nodos):
        d = "".join(a_path(etq, attrs) for etq, attrs in nodos[nombre])
        palabras = " ".join(t.lower() for t in tags.get(nombre, []))
        filas.append(f"    ({cadena_rust(nombre)}, {cadena_rust(palabras)}, {cadena_rust(d)}),")

    with open(salida, "w", encoding="utf-8", newline="\n") as f:
        f.write("// Generado por scripts/generar_lucide.py a partir de lucide-static. No editar a mano.\n")
        f.write("// Íconos de Lucide (https://lucide.dev), licencia ISC: ver licencias/LUCIDE.txt.\n\n")
        f.write("/// (nombre, palabras clave, datos de path SVG en una caja de 24x24), ordenado por nombre.\n")
        f.write("#[rustfmt::skip]\n")
        f.write("pub static ICONOS: &[(&str, &str, &str)] = &[\n")
        f.write("\n".join(filas))
        f.write("\n];\n")
    print(f"{len(filas)} íconos -> {salida}")


if __name__ == "__main__":
    main()
