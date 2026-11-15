"""Genera las imágenes del asistente de Inno Setup (desinstalador y modo de respaldo) con la estética
de Rust Sonar: fondo grafito, logo turquesa y una franja con los colores de los canales.

Uso: python installer/make_wizard_images.py
"""
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(__file__).resolve().parent / "img"
BG = (0x0D, 0x0F, 0x13)
BG2 = (0x16, 0x1A, 0x21)
TEAL = (0x2D, 0xD4, 0xBF)
TEXT = (0xE6, 0xE9, 0xEF)
TEXT3 = (0x67, 0x70, 0x80)
CHANNELS = [(0x84, 0xCC, 0x16), (0x38, 0xBD, 0xF8), (0xFB, 0xBF, 0x24), (0xA7, 0x8B, 0xFA), (0xF4, 0x72, 0xB6)]


def font(weight: str, size: int) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(str(ROOT / "assets" / "fonts" / f"latin-{weight}-normal.ttf"), size)


def logo(draw: ImageDraw.ImageDraw, cx: float, cy: float, r: float):
    """Anillo con disco tenue y tres barras, como el icono de la app."""
    w = r * 0.09
    draw.ellipse([cx - r, cy - r, cx + r, cy + r], fill=tuple(int(BG2[i] * 0.4 + TEAL[i] * 0.12) for i in range(3)))
    draw.ellipse([cx - r, cy - r, cx + r, cy + r], outline=TEAL, width=max(2, int(w * 1.6)))
    for bx, h in [(-0.36, 0.33), (0.0, 0.58), (0.36, 0.42)]:
        x = cx + bx * r
        draw.rounded_rectangle([x - w, cy - h * r, x + w, cy + h * r], radius=w, fill=TEAL)


def gradient(w: int, h: int) -> Image.Image:
    img = Image.new("RGB", (w, h), BG)
    px = img.load()
    for y in range(h):
        t = y / max(1, h - 1)
        for x in range(w):
            # Luz turquesa muy suave arriba a la izquierda.
            d = ((x / w) ** 2 + (t * 0.9) ** 2) ** 0.5
            glow = max(0.0, 1.0 - d) ** 2 * 0.10
            px[x, y] = tuple(int(BG[i] + (TEAL[i] - BG[i]) * glow) for i in range(3))
    return img


def side_image(scale: float) -> Image.Image:
    w, h = int(164 * scale), int(314 * scale)
    img = gradient(w, h)
    d = ImageDraw.Draw(img)
    logo(d, w / 2, h * 0.30, w * 0.22)
    title = font("600", int(19 * scale))
    sub = font("400", int(10.5 * scale))
    d.text((w / 2, h * 0.52), "Rust Sonar", font=title, fill=TEXT, anchor="mm")
    d.text((w / 2, h * 0.52 + 20 * scale), "Mezclador de audio", font=sub, fill=TEXT3, anchor="mm")
    # Franja de canales.
    n = len(CHANNELS)
    bw, gap = 16 * scale, 6 * scale
    x0 = w / 2 - (n * bw + (n - 1) * gap) / 2
    base = h * 0.86
    for i, c in enumerate(CHANNELS):
        bh = [26, 40, 32, 22, 34][i] * scale
        x = x0 + i * (bw + gap)
        d.rounded_rectangle([x, base - bh, x + bw, base], radius=int(4 * scale), fill=c)
    return img


def small_image(size: int) -> Image.Image:
    img = Image.new("RGB", (size, size), BG)
    logo(ImageDraw.Draw(img), size / 2, size / 2, size * 0.42)
    return img


def main():
    OUT.mkdir(exist_ok=True)
    for s, tag in [(1.0, "100"), (1.5, "150"), (2.0, "200")]:
        side_image(s).save(OUT / f"wizard-{tag}.bmp")
    for s in (55, 83, 110):
        small_image(s).save(OUT / f"wizard-small-{s}.bmp")
    print(f"imágenes en {OUT}")


if __name__ == "__main__":
    main()
