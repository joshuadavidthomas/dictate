"""Read-only Blender cross-check: --background --python platform/verify_scene.py.

Checks nominal well bounds, not the inset bevel's shaded face boundary.
Never saves or renders a Blender scene.
"""

from pathlib import Path
import xml.etree.ElementTree as ET

import bpy


ROOT = Path(__file__).resolve().parent
bpy.ops.wm.open_mainfile(filepath=str(ROOT.parent / "dictate.blend"))
scene = bpy.context.scene
assert scene.camera.data.ortho_scale == 10
assert scene.camera.location[:] == (0, 0, 14)
ns = {"s": "http://www.w3.org/2000/svg"}
glyph = ET.parse(ROOT / "dictate-glyph-1024.svg").getroot()
rects = glyph.findall("s:g/s:rect", ns)
assert len(rects) == 4
max_delta = 0
for index, rect in enumerate(rects, 1):
    obj = bpy.data.objects[f"Orange well {index}"]
    vertices = [obj.matrix_world @ v.co for v in obj.data.vertices]
    x0, x1 = min(v.x for v in vertices), max(v.x for v in vertices)
    y0, y1 = min(v.y for v in vertices), max(v.y for v in vertices)
    actual = (512 + x0 * 102.4, 512 - y1 * 102.4,
              (x1 - x0) * 102.4, (y1 - y0) * 102.4)
    expected = tuple(float(rect.attrib[key]) for key in ("x", "y", "width", "height"))
    delta = max(abs(a - b) for a, b in zip(actual, expected))
    assert delta < 0.0001, (index, actual, expected)
    max_delta = max(max_delta, delta)
    # Reverse the camera-to-tile coordinate mapping, retaining C+16 placement
    # and bowl lift; compare independently in both coordinate systems.
    actual_tile = (512 + x0 * 1024 / 8.44, 512 - y1 * 1024 / 8.44,
                   (x1 - x0) * 1024 / 8.44, (y1 - y0) * 1024 / 8.44)
    expected_tile = (512 + (expected[0] - 512) / .844,
                     512 + (expected[1] - 512) / .844,
                     expected[2] / .844, expected[3] / .844)
    assert max(abs(a - b) for a, b in zip(actual_tile, expected_tile)) < 0.00012
    assert float(rect.attrib["rx"]) == expected[2] / 2
    print(f"Pill {index} canvas x/top/w/h: {expected}; tile: {expected_tile}")

body = bpy.data.objects['Cream body · subtle outside edge']
vertices = [body.matrix_world @ v.co for v in body.data.vertices]
assert max(abs(abs(value) - 4.22) for value in (
    min(v.x for v in vertices), max(v.x for v in vertices),
    min(v.y for v in vertices), max(v.y for v in vertices))) < 1e-6
print(f"PASS: four nominal well bounds match Blender; max camera-pixel delta {max_delta:.9f}")
print("PASS: tile extent ±4.22 world units = 79.872–944.128 camera pixels")
