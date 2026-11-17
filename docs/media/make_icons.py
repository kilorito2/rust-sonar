"""Genera los iconos SVG del README a partir de la fuente Phosphor de la app (MIT).

Uso: python docs/media/make_icons.py
Salen en assets/readme/icons/<nombre>.svg, en el turquesa de la app (se ve en tema claro y oscuro).
"""

from pathlib import Path

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parents[2]
FONT = ROOT / "assets" / "fonts" / "Phosphor.ttf"
OUT = ROOT / "assets" / "readme" / "icons"
COLOR = "#2DD4BF"

# nombre del archivo -> código del icono en la fuente (ver phosphoricons.com)
ICONS = {
    "sparkle": 0xE6A2,
    "sliders": 0xE434,
    "equalizer": 0xEBBC,
    "microphone": 0xE326,
    "desktop": 0xE560,
    "chart-bar": 0xE150,
    "image": 0xE2CA,
    "translate": 0xE4A2,
    "flow-arrow": 0xE6EC,
    "rocket": 0xE3FE,
    "plugs": 0xEB5A,
    "tree": 0xE67C,
    "warning": 0xE4E0,
    "info": 0xE2CE,
    "scales": 0xE750,
    "heart": 0xE2A8,
    "play": 0xE3D2,
    "package": 0xE390,
}


def main() -> None:
    font = TTFont(FONT)
    glyphs = font.getGlyphSet()
    cmap = font.getBestCmap()
    em = font["head"].unitsPerEm
    ascent = font["hhea"].ascent
    OUT.mkdir(parents=True, exist_ok=True)
    for name, code in ICONS.items():
        glyph = glyphs[cmap[code]]
        pen = SVGPathPen(glyphs)
        # La fuente tiene la y hacia arriba; el SVG, hacia abajo.
        glyph.draw(TransformPen(pen, (1, 0, 0, -1, 0, ascent)))
        svg = (
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {glyph.width} {em}" width="24" height="24">'
            f'<path fill="{COLOR}" d="{pen.getCommands()}"/></svg>\n'
        )
        (OUT / f"{name}.svg").write_text(svg, encoding="utf-8")
    print(f"{len(ICONS)} iconos en {OUT}")


if __name__ == "__main__":
    main()
