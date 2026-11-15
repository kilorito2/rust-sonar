"""Builds the README media in assets/readme/ from raw window captures.

    python docs/media/make_media.py <captures_dir>

<captures_dir> must contain the output of the capture script: mezcla.png, juego.png, micro.png,
ajustes.png, frames_mezcla/, frames_juego/, frames_micro/ (English UI) and es/mezcla.png (Spanish UI).
Needs Pillow and ffmpeg on PATH. Generates:

- mix.png, channel.png, mic.png, settings.png, mix-es.png   resized screenshots
- demo.gif / demo.mp4                                       short UI tour with live meters
- benchmark.gif / benchmark.mp4                             animated resource comparison
"""
import math
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "assets" / "readme"
FONTS = ROOT / "assets" / "fonts"

BG = (0x0D, 0x0F, 0x13)
SURFACE = (0x16, 0x1A, 0x21)
SURFACE_2 = (0x1C, 0x21, 0x2A)
BORDER = (0x24, 0x2A, 0x34)
TEXT = (0xE6, 0xE9, 0xEF)
TEXT_2 = (0x9A, 0xA3, 0xB2)
TEXT_3 = (0x67, 0x70, 0x80)
ACCENT = (0x2D, 0xD4, 0xBF)
GG = (0xF9, 0x73, 0x16)  # neutral orange for the other product (no brand assets are used)


def font(weight: int, size: int) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(str(FONTS / f"latin-{weight}-normal.ttf"), size)


def ffmpeg(*args: str) -> None:
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", *args], check=True)


def resize(img: Image.Image, width: int) -> Image.Image:
    h = round(img.height * width / img.width)
    return img.convert("RGB").resize((width, h), Image.LANCZOS)


# ------------------------------------------------------------------------------------------------
# Screenshots
# ------------------------------------------------------------------------------------------------


def screenshots(cap: Path) -> None:
    for src, dst in [
        ("mezcla.png", "mix.png"),
        ("juego.png", "channel.png"),
        ("micro.png", "mic.png"),
        ("ajustes.png", "settings.png"),
        ("es/mezcla.png", "mix-es.png"),
    ]:
        resize(Image.open(cap / src), 1400).save(OUT / dst, optimize=True)


# ------------------------------------------------------------------------------------------------
# UI tour
# ------------------------------------------------------------------------------------------------


def caption(img: Image.Image, text: str) -> Image.Image:
    """Small pill with the section name in the bottom-right corner."""
    img = img.copy()
    d = ImageDraw.Draw(img, "RGBA")
    f = font(600, 30)
    w = d.textlength(text, font=f)
    x1, y1 = img.width - 40, img.height - 36
    x0, y0 = x1 - w - 44, y1 - 58
    d.rounded_rectangle((x0, y0, x1, y1), radius=29, fill=(13, 15, 19, 230), outline=ACCENT, width=2)
    d.text((x0 + 22, y0 + 11), text, font=f, fill=TEXT)
    return img


def demo(cap: Path, tmp: Path) -> None:
    sections = [
        ("frames_mezcla", "Mix: every channel at a glance"),
        ("frames_juego", "Per-channel EQ, smart volume, spatial audio"),
        ("frames_micro", "AI noise suppression for your mic"),
    ]
    frames: list[Image.Image] = []
    for folder, label in sections:
        seq = [caption(Image.open(p).convert("RGB"), label) for p in sorted((cap / folder).glob("*.png"))]
        if frames:  # crossfade from the previous section
            last = frames[-1]
            for i in range(1, 7):
                frames.append(Image.blend(last, seq[0], i / 7))
        frames.extend(seq)
    settings = caption(Image.open(cap / "ajustes.png").convert("RGB"), "One-click setup of the virtual devices")
    last = frames[-1]
    for i in range(1, 7):
        frames.append(Image.blend(last, settings, i / 7))
    frames.extend([settings] * 24)

    d = tmp / "demo"
    d.mkdir()
    for i, f in enumerate(frames):
        f.save(d / f"{i:04d}.png")
    ffmpeg("-framerate", "12", "-i", str(d / "%04d.png"), "-vf", "scale=1280:-2:flags=lanczos",
           "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "20", "-movflags", "+faststart", str(OUT / "demo.mp4"))
    ffmpeg("-framerate", "12", "-i", str(d / "%04d.png"), "-vf",
           "scale=960:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=none",
           str(OUT / "demo.gif"))


# ------------------------------------------------------------------------------------------------
# Benchmark video
# ------------------------------------------------------------------------------------------------

# Measured on the same PC (Ryzen 5 4600G, 12 threads), 45 s per scenario.
# CPU = process tree + its share of audiodg, as % of the whole machine. Memory = private bytes.
SCENARIOS = [
    # label,                         rust cpu, gg cpu, rust MB, gg MB
    ("Window open, no audio", 0.15, 0.28, 98.4, 881.0),
    ("In the tray, no audio", 0.01, 0.06, 4.8, 275.0),
    ("In the tray, playing audio", 0.26, 0.90, 5.2, 273.0),
]
W, H = 1280, 720
FPS = 30


def ease(t: float) -> float:
    t = min(1.0, max(0.0, t))
    return 1 - (1 - t) ** 3


def bar_panel(d: ImageDraw.ImageDraw, x: int, y: int, w: int, title: str, unit: str, idx_r: int, idx_g: int,
              vmax: float, t: float, fmt) -> None:
    d.rounded_rectangle((x, y, x + w, y + 452), radius=22, fill=SURFACE, outline=BORDER, width=2)
    d.text((x + 28, y + 24), title, font=font(600, 26), fill=TEXT)
    d.text((x + 28, y + 60), unit, font=font(400, 18), fill=TEXT_3)
    track_x = x + 28
    track_w = w - 56 - 110
    for i, sc in enumerate(SCENARIOS):
        gy = y + 108 + i * 112
        d.text((track_x, gy), sc[0], font=font(500, 19), fill=TEXT_2)
        k = ease((t - 0.6 - i * 0.35) / 1.6)
        for j, (val, color, name) in enumerate([(sc[idx_r], ACCENT, "Rust Sonar"), (sc[idx_g], GG, "SteelSeries GG")]):
            by = gy + 32 + j * 30
            d.rounded_rectangle((track_x, by, track_x + track_w, by + 20), radius=10, fill=SURFACE_2)
            bw = max(8, track_w * (val / vmax) * k)
            d.rounded_rectangle((track_x, by, track_x + bw, by + 20), radius=10, fill=color)
            d.text((track_x + track_w + 14, by - 3), fmt(val * k), font=font(600, 20), fill=color if k > 0 else TEXT_3)


def benchmark(tmp: Path) -> None:
    d_out = tmp / "bench"
    d_out.mkdir()
    total = int(FPS * 8.5)
    for n in range(total):
        t = n / FPS
        img = Image.new("RGB", (W, H), BG)
        d = ImageDraw.Draw(img, "RGBA")
        a = ease(t / 0.5)
        d.text((48, 30), "Rust Sonar vs SteelSeries GG", font=font(600, 40), fill=tuple(int(c * a) for c in TEXT))
        d.text((48, 82), "Same PC · Ryzen 5 4600G · 45 s per scenario · lower is better", font=font(400, 20),
               fill=tuple(int(c * a) for c in TEXT_3))
        # legend
        lx = W - 470
        for color, name in [(ACCENT, "Rust Sonar"), (GG, "SteelSeries GG")]:
            d.rounded_rectangle((lx, 44, lx + 18, 62), radius=5, fill=color)
            d.text((lx + 28, 40), name, font=font(500, 20), fill=TEXT_2)
            lx += 180
        bar_panel(d, 48, 136, 578, "CPU usage", "% of the whole machine, incl. audiodg", 1, 2, 0.95, t,
                  lambda v: f"{v:.2f} %")
        bar_panel(d, 654, 136, 578, "Memory", "private MB", 3, 4, 900, t, lambda v: f"{v:,.0f} MB" if v >= 10 else f"{v:.1f} MB")
        # bottom strip with the headline ratios
        k = ease((t - 3.6) / 0.8)
        if k > 0:
            stats = [("57×", "less RAM in the tray"), ("3.5×", "less CPU playing audio"), ("56×", "less disk (24 MB)"), ("1 vs 5", "background processes")]
            sw = (W - 96 - 3 * 18) / 4
            for i, (big, small) in enumerate(stats):
                sx = 48 + i * (sw + 18)
                sy = 610 + (1 - k) * 30
                d.rounded_rectangle((sx, sy, sx + sw, sy + 84), radius=18, fill=(0x2D, 0xD4, 0xBF, int(28 * k)),
                                    outline=(0x2D, 0xD4, 0xBF, int(160 * k)), width=2)
                d.text((sx + 22, sy + 6), big, font=font(600, 34), fill=(0x2D, 0xD4, 0xBF, int(255 * k)))
                d.text((sx + 22, sy + 52), small, font=font(500, 17), fill=(0xE6, 0xE9, 0xEF, int(255 * k)))
        img.save(d_out / f"{n:04d}.png")
    ffmpeg("-framerate", str(FPS), "-i", str(d_out / "%04d.png"), "-c:v", "libx264", "-pix_fmt", "yuv420p",
           "-crf", "18", "-movflags", "+faststart", str(OUT / "benchmark.mp4"))
    ffmpeg("-framerate", str(FPS), "-i", str(d_out / "%04d.png"), "-vf",
           "fps=15,scale=960:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=64[p];[b][p]paletteuse=dither=none",
           "-loop", "0", str(OUT / "benchmark.gif"))


def main() -> None:
    cap = Path(sys.argv[1])
    OUT.mkdir(parents=True, exist_ok=True)
    tmp = Path(tempfile.mkdtemp())
    try:
        screenshots(cap)
        demo(cap, tmp)
        benchmark(tmp)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    for p in sorted(OUT.iterdir()):
        print(f"{p.name:16} {p.stat().st_size / 1024:8.0f} KB")


if __name__ == "__main__":
    main()
