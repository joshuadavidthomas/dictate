"""Generate locked-v1 vector/web assets with Python 3 and ImageMagick 7.

Run from any directory: python3 packaging/icons/platform/generate.py
Only writes the six deliverables beside this script; never modifies Blender
sources, production renders, or studies. Geometry is expressed at 1024 pixels.
"""

from decimal import Decimal as D
import math
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parent
CREAM = "#F3EADA"
ORANGE = "#FF8A25"
MONO = "#FFF8EE"
# Original tile-relative left, top, width, height. Tile width is 8.44 world
# units, versus the orthographic camera's 10: tile pixels scale by 0.844.
SOURCE = (
    ("139.18", "447.38", "129.24", "268.43"),
    ("318.14", "313.17", "129.24", "536.85"),
    ("497.09", "382.76", "129.24", "397.67"),
    ("676.04", "164.04", "149.13", "685.98"),
)
TILE_TO_CAMERA = D("0.844")
C16_X = D(16)  # Camera-canvas pixels, not tile-relative pixels.
HALF_TOP = D("69.595")  # Tile-relative pixels.
BOWL_UP = D(32)  # Camera-canvas pixels; Blender +Y maps to SVG -Y.


def rectangles():
    """Yield (tile bounds, camera bounds), each (left, top, width, height).

    Tile bounds include the bowl lift converted back to tile units. Decimal
    preserves the intended source values independently of Blender float32.
    """
    for index, source in enumerate(SOURCE):
        x, top, width, height = map(D, source)
        bottom = top + height
        if index == 2:
            bottom = D("748.12")
        if index < 3:
            top += HALF_TOP
        height = bottom - top
        camera_x = D(512) + (x - D(512)) * TILE_TO_CAMERA + C16_X
        camera_y = D(512) + (top - D(512)) * TILE_TO_CAMERA
        if index < 3:
            camera_y -= BOWL_UP
        tile_x = x + C16_X / TILE_TO_CAMERA
        tile_y = top - (BOWL_UP / TILE_TO_CAMERA if index < 3 else D(0))
        yield ((tile_x, tile_y, width, height),
               (camera_x, camera_y, width * TILE_TO_CAMERA, height * TILE_TO_CAMERA))


def glyph(*, fill=ORANGE, full_bleed=False, width_scale=D(1)):
    result = [f'  <g fill="{fill}">']
    for tile_bounds, camera_bounds in rectangles():
        x, y, width, height = tile_bounds if full_bleed else camera_bounds
        inset = width * (D(1) - width_scale) / 2
        x += inset
        width *= width_scale
        result.append(f'    <rect x="{x:f}" y="{y:f}" width="{width:f}" '
                      f'height="{height:f}" rx="{width / 2:f}"/>')
    return "\n".join([*result, "  </g>"])


def tile():
    # Exact 256-sample superellipse profile used by the Blender extrusion:
    # x/y = 4.22 * sign(cos/sin(t)) * abs(cos/sin(t)) ** (2/4.5).
    # Orthographic world-to-canvas mapping is 102.4 px/unit, Y inverted.
    points = []
    for i in range(256):
        angle = 2 * math.pi * i / 256
        x, y = (4.22 * math.copysign(abs(v) ** (2 / 4.5), v)
                for v in (math.cos(angle), math.sin(angle)))
        points.append(f'{512 + x * 102.4:.6f},{512 - y * 102.4:.6f}')
    lines = [" ".join(points[i:i + 4]) for i in range(0, len(points), 4)]
    return f'  <polygon fill="{CREAM}" points="\n    ' + "\n    ".join(lines) + '"/>'


def svg(contents):
    return ('<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" '
            'viewBox="0 0 1024 1024">\n'
            '  <title>Dictate</title>\n' + contents + '\n</svg>\n')


def rasterize(source, destination, size):
    # Pin ImageMagick's built-in SVG renderer rather than a machine-dependent
    # external delegate. Render the SVG at its 1024 viewBox, then Lanczos downscale.
    subprocess.run([
        "magick", "-background", "none", f"MSVG:{source}",
        "-filter", "Lanczos", "-resize", f"{size}x{size}", "-depth", "8",
        "-define", "png:color-type=6", "-define", "png:exclude-chunk=time,date",
        str(destination),
    ], check=True)


def main():
    (ROOT / "dictate-glyph-1024.svg").write_text(svg(glyph()))
    (ROOT / "dictate-glyph-icon-composer-1024.svg").write_text(
        svg(glyph(fill=MONO, full_bleed=True, width_scale=D("0.94"))))
    full = svg(tile() + "\n" + glyph())
    (ROOT / "dev.joshthomas.dictate.svg").write_text(full)
    (ROOT / "favicon.svg").write_text(full)
    rasterize(ROOT / "favicon.svg", ROOT / "favicon-32.png", 32)
    subprocess.run([
        "magick", str(ROOT.parent / "dictate-2048.png"),
        "-filter", "Lanczos", "-resize", "180x180", "-depth", "8",
        "-define", "png:color-type=6", "-define", "png:exclude-chunk=time,date",
        str(ROOT / "apple-touch-icon.png"),
    ], check=True)
    print("Generated four SVGs, 32px SVG fallback, and 180px Blender-derived RGBA touch icon")


if __name__ == "__main__":
    main()
