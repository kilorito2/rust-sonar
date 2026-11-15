"""Genera assets/rust_sonar.ico con el mismo dibujo que `app_icon()` de main.rs
(anillo turquesa con tres barras), en los tamaños que usa Windows.

Uso: python installer/make_icon.py
"""
import math
from pathlib import Path

from PIL import Image

TEAL = (0x2D, 0xD4, 0xBF)
BARS = [(-11.0, 10.0), (0.0, 18.0), (11.0, 13.0)]  # (x, media altura) en la rejilla de 64 px


def clamp(v, lo, hi):
    return max(lo, min(hi, v))


def draw(n: int) -> Image.Image:
    img = Image.new("RGBA", (n, n))
    px = img.load()
    k = n / 64.0  # las medidas originales están pensadas para 64 px
    c = (n - 1) / 2.0
    aa = max(1.0, k)  # ancho del antialiasing en píxeles reales
    for y in range(n):
        for x in range(n):
            dx, dy = (x - c) / k, (y - c) / k
            d = math.hypot(dx, dy)
            ring = max(0.0, 1.0 - clamp((abs(d - 28.5) - 2.5) * k / aa, 0.0, 1.0))
            disc = clamp((31.5 - d) * k / aa, 0.0, 1.0) * 0.18
            bar = 0.0
            for bx, h in BARS:
                cov = (1.0 - clamp((abs(dx - bx) - 2.5) * k / aa, 0.0, 1.0)) * (1.0 - clamp((abs(dy) - h) * k / aa, 0.0, 1.0))
                bar = max(bar, cov)
            a = max(ring, bar, disc)
            px[x, y] = (*TEAL, int(a * 255))
    return img


def main():
    out = Path(__file__).resolve().parent.parent / "assets" / "rust_sonar.ico"
    sizes = [16, 24, 32, 48, 64, 128, 256]
    big = draw(256)
    big.save(out, format="ICO", sizes=[(s, s) for s in sizes], append_images=[draw(s) for s in sizes[:-1]])
    print(f"icono: {out}")


if __name__ == "__main__":
    main()
