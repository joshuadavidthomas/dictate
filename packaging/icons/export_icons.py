"""Export PNG sizes and a PNG-backed macOS ICNS from the Blender master.

Requires Python 3 and ImageMagick 7 (magick). Does not install anything.
"""

from pathlib import Path
import struct
import subprocess


ROOT = Path(__file__).resolve().parent
SIZES = (16, 24, 32, 48, 64, 96, 128, 256, 512, 1024)
# ICNS representations retain distinct logical 1x/2x sizes, even when two
# representations currently share the same raster. Tuple: type, points, scale.
REPRESENTATIONS = (
    (b"icp4", 16, 1), (b"ic11", 16, 2),
    (b"icp5", 32, 1), (b"ic12", 32, 2),
    (b"ic07", 128, 1), (b"ic13", 128, 2),
    (b"ic08", 256, 1), (b"ic14", 256, 2),
    (b"ic09", 512, 1), (b"ic10", 512, 2),
)


def export():
    master = ROOT / "dictate-2048.png"
    for size in SIZES:
        subprocess.run([
            "magick", str(master), "-filter", "Lanczos",
            "-resize", f"{size}x{size}", "-depth", "8",
            "-define", "png:color-type=6", "-define", "png:exclude-chunk=time,date",
            str(ROOT / f"dictate-{size}.png"),
        ], check=True)
    # Modern ICNS stores PNG payloads in length-prefixed, big-endian chunks.
    chunks = []
    for tag, points, scale in REPRESENTATIONS:
        png = (ROOT / f"dictate-{points * scale}.png").read_bytes()
        chunks.append(tag + struct.pack(">I", len(png) + 8) + png)
    payload = b"".join(chunks)
    (ROOT / "dictate.icns").write_bytes(b"icns" + struct.pack(">I", len(payload) + 8) + payload)
    print(f"Exported {len(SIZES)} RGBA PNG sizes and {len(chunks)} ICNS representations")


if __name__ == "__main__":
    export()
