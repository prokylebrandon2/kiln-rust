#!/usr/bin/env python3
"""Regenerates res/drawable/ic_d_*.xml: folder with a glyph cut out, glyph centred on the folder body.
Glyphs are drawn around (0,0) in absolute SVG path commands; the script renders each one, measures the
real bounding box and moves it so its centre lands on the middle of the folder body."""
import math, re, subprocess, sys, os
from PIL import Image

OUT = sys.argv[1] if len(sys.argv) > 1 else "."
BODY = "M10,4H4C2.9,4 2,4.9 2,6v12c0,1.1 0.9,2 2,2h16c1.1,0 2,-0.9 2,-2V8c0,-1.1 -0.9,-2 -2,-2h-8l-2,-2z"
CX, CY = 12.0, 13.2          # centre of the folder body (below the tab)

def rect(x, y, w, h): return f"M{x},{y}h{w}v{h}h{-w}z"
def circle(cx, cy, r): return f"M{cx-r},{cy}A{r},{r} 0 1 0 {cx+r},{cy}A{r},{r} 0 1 0 {cx-r},{cy}Z"
def star(r1, r2, n=5):
    pts = []
    for i in range(n * 2):
        a = -math.pi / 2 + i * math.pi / n
        r = r1 if i % 2 == 0 else r2
        pts.append((r * math.cos(a), r * math.sin(a)))
    return "M" + "L".join(f"{x:.2f},{y:.2f}" for x, y in pts) + "Z"

G = {
 "download": " ".join([
    "M-0.8,-4L0.8,-4L0.8,-1L2.2,-2.4L3.4,-1.2L0,2.2L-3.4,-1.2L-2.2,-2.4L-0.8,-1Z",
    rect(-3.6, 3.0, 7.2, 1.2)]),
 "music": ("M-2.2,-3.6L4.1,-4.6L4.1,2A1.9,1.5 0 1 1 0.3,2A1.9,1.5 0 0 1 2.8,0.58L2.8,-3.1L-0.9,-2.51"
           "L-0.9,3A1.9,1.5 0 1 1 -4.7,3A1.9,1.5 0 0 1 -2.2,1.58Z"),
 "documents": " ".join([rect(-3.8, -3.4, 7.6, 1.4), rect(-3.8, -0.7, 7.6, 1.4), rect(-3.8, 2, 5, 1.4)]),
 "bell": ("M0,-3.6C-2.1,-3.6 -3.2,-1.9 -3.2,0L-3.2,1.4L-4.1,2.3L-4.1,2.9L4.1,2.9L4.1,2.3L3.2,1.4L3.2,0C3.2,-1.9 2.1,-3.6 0,-3.6Z "
          "M-1.1,3.4L1.1,3.4A1.1,1.1 0 0 1 -1.1,3.4Z"),
 "camera": " ".join(["M-4.2,-2.4L-2,-2.4L-1.4,-3.8L1.4,-3.8L2,-2.4L4.2,-2.4L4.2,4L-4.2,4Z",
                    circle(0, 0.9, 2.4), circle(0, 0.9, 1.1)]),
 "code": ("M-1.3,-3.2L-4.3,0L-1.3,3.2L-0.3,2.2L-2.3,0L-0.3,-2.2Z "
          "M1.3,-3.2L4.3,0L1.3,3.2L0.3,2.2L2.3,0L0.3,-2.2Z"),
 "heart": ("M0,3.8L-0.9,3C-3.6,0.6 -4.2,-0.4 -4.2,-1.8C-4.2,-3 -3.2,-4 -2,-4C-1.2,-4 -0.4,-3.6 0,-3"
           "C0.4,-3.6 1.2,-4 2,-4C3.2,-4 4.2,-3 4.2,-1.8C4.2,-0.4 3.6,0.6 0.9,3Z"),
 "lock": " ".join([rect(-3.2, -0.4, 6.4, 4.8),
                    "M-2.2,-0.4L-2.2,-1.6A2.2,2.2 0 0 1 2.2,-1.6L2.2,-0.4L1,-0.4L1,-1.6A1,1 0 0 0 -1,-1.6L-1,-0.4Z"]),
 "movies": "M-2.4,-4L3.6,0L-2.4,4Z",
 "pictures": " ".join(["M-4.2,3.4L-1.2,-0.8L0.9,1.8L2.2,0.2L4.2,3.4Z", circle(2.3, -2.4, 1.2)]),
 "star": star(4.4, 1.9),
 "zip": " ".join([rect(-1.9, -4, 1.6, 1.2), rect(-1.9, -1.8, 1.6, 1.2), rect(-1.9, 0.4, 1.6, 1.2),
                 rect(0.3, -2.9, 1.6, 1.2), rect(0.3, -0.7, 1.6, 1.2), rect(0.3, 1.5, 1.6, 1.2),
                 rect(-1.9, 3.0, 3.8, 1.5)]),
 "android": " ".join(["M-4.2,2.4A4.2,4.2 0 0 1 4.2,2.4Z", circle(-1.7, 0.3, 0.65), circle(1.7, 0.3, 0.65)]),
}

TOK = re.compile(r"([MLHVCAZmlhvcaz])|(-?\d*\.?\d+(?:e-?\d+)?)")
ARGS = {"M": 2, "L": 2, "H": 1, "V": 1, "C": 6, "A": 7, "Z": 0, "m": 2, "l": 2, "h": 1, "v": 1, "c": 6, "a": 7, "z": 0}

def shift(d, dx, dy):
    """Translate a path. Absolute commands move; relative ones only matter right after a move."""
    out, cmd, nums, first = [], None, [], True
    toks = [(m.group(1), m.group(2)) for m in TOK.finditer(d)]
    i = 0
    res = []
    while i < len(toks):
        c, n = toks[i]
        if c:
            cmd = c; i += 1
            if cmd in "Zz": res.append("Z"); continue
            k = ARGS[cmd]
            first_cmd = cmd
            while i < len(toks) and toks[i][1] is not None:
                vals = []
                for _ in range(k):
                    vals.append(float(toks[i][1])); i += 1
                if cmd in "MLC":
                    vals = [v + (dx if j % 2 == 0 else dy) for j, v in enumerate(vals)]
                elif cmd == "H": vals = [vals[0] + dx]
                elif cmd == "V": vals = [vals[0] + dy]
                elif cmd == "A": vals[5] += dx; vals[6] += dy
                elif cmd == "m" and not res: pass
                elif cmd in "mlhvca": pass
                res.append(cmd.upper() if cmd in "MLCHVA" else cmd)
                res.append(",".join(f"{v:.2f}".rstrip("0").rstrip(".") for v in vals))
                if cmd == "M": cmd = "L"
        else:
            i += 1
    return "".join(p if p in "MLHVCAZmlhvca" else p for p in res)

def bbox(d):
    svg = f'<svg xmlns="http://www.w3.org/2000/svg" width="800" height="800" viewBox="-10 -10 20 20"><rect x="-10" y="-10" width="20" height="20" fill="white"/><path fill="black" fill-rule="evenodd" d="{d}"/></svg>'
    import cairosvg
    cairosvg.svg2png(bytestring=svg.encode(), write_to="/tmp/g.png")
    im = Image.open("/tmp/g.png").convert("L").point(lambda p: 255 if p < 128 else 0)
    x0, y0, x1, y1 = im.getbbox()
    s = 40.0
    return x0 / s - 10, y0 / s - 10, x1 / s - 10, y1 / s - 10

XML = '''<vector xmlns:android="http://schemas.android.com/apk/res/android"
    android:width="24dp" android:height="24dp" android:viewportWidth="24" android:viewportHeight="24"
    android:tint="?android:attr/textColorSecondary">
    <path android:fillColor="#FF000000" android:fillType="evenOdd"
        android:pathData="%s" />
</vector>
'''
if __name__ == "__main__":
    for name, d in G.items():
        x0, y0, x1, y1 = bbox(d)
        dx, dy = CX - (x0 + x1) / 2, CY - (y0 + y1) / 2
        moved = shift(d, dx, dy)
        nx0, ny0, nx1, ny1 = bbox(shift(d, 0, 0))
        open(os.path.join(OUT, f"ic_d_{name}.xml"), "w").write(XML % (BODY + " " + moved))
        print(f"{name:10s} size {x1-x0:4.1f}x{y1-y0:4.1f}  shift {dx:+.2f},{dy:+.2f}")
