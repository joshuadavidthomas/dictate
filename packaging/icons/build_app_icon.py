"""Build locked v1: Bowl-up-more Control, with Blender 3.4.1.

Run: blender --background --factory-startup --python packaging/icons/build_app_icon.py
Icon Composer bake: blender --background --factory-startup --python packaging/icons/build_app_icon.py -- --icon-composer
Dark bake: blender --background --factory-startup --python packaging/icons/build_app_icon.py -- --icon-composer --dark
Standalone: no input assets, external textures, or local helper modules.
The default writes dictate.blend and the 2048px dictate-2048.png; run
export_icons.py next. The alternate mode writes a full-bleed 1024px bake whose
ceramic surface extends beyond the canvas so Apple's mask is its only squircle.

Preserves the exact selected modeling sequence: C+16 X placement, Bottom-up
third pill, Half top-shortening of pills 1–3, then their 32-camera-pixel lift.
No later whole-glyph centering or horizontal shift is applied.
"""

import math
import struct
import sys
from pathlib import Path

import bpy
import bmesh

ROOT = Path(__file__).resolve().parent
ARGS = sys.argv[sys.argv.index('--') + 1:] if '--' in sys.argv else []
ICON_COMPOSER = '--icon-composer' in ARGS
DARK = '--dark' in ARGS
if DARK and not ICON_COMPOSER:
    raise ValueError('--dark requires --icon-composer')

# Tile-relative coordinates span the 8.44-unit tile; camera-canvas coordinates
# span the 10-unit orthographic frame. Both references use a 1024-pixel grid,
# independent of the final 2048 render. Do not interchange these two scales.
TILE_PIXEL = 8.44 / 1024
C16_RIGHT_CAMERA_PX = 16
PILL3_SOURCE_BOTTOM_TILE_PX = 780.43
PILL3_BOTTOM_TILE_PX = 748.12
HALF_TOP_SHORTEN_TILE_PX = 69.595
BOWL_UP_CAMERA_PX = 32


def material(name, hex_color, roughness, specular, grain=False):
    srgb = tuple(int(hex_color[i:i + 2], 16) / 255 for i in (0, 2, 4))
    linear = tuple(c / 12.92 if c < 0.04045 else ((c + 0.055) / 1.055) ** 2.4 for c in srgb)
    mat = bpy.data.materials.new(name)
    mat.diffuse_color = (*linear, 1)
    mat.use_nodes = True
    nodes, links = mat.node_tree.nodes, mat.node_tree.links
    shader = nodes['Principled BSDF']
    shader.inputs['Base Color'].default_value = (*linear, 1)
    shader.inputs['Roughness'].default_value = roughness
    shader.inputs['Specular'].default_value = specular
    shader.inputs['Clearcoat'].default_value = 0
    if grain:
        coordinates = nodes.new('ShaderNodeTexCoord')
        noise = nodes.new('ShaderNodeTexNoise')
        noise.name = 'Fine satin grain (not color mottling)'
        noise.inputs['Scale'].default_value = 145
        noise.inputs['Detail'].default_value = 2
        links.new(coordinates.outputs['Generated'], noise.inputs['Vector'])
        bump = nodes.new('ShaderNodeBump')
        bump.inputs['Strength'].default_value = 0.16
        bump.inputs['Distance'].default_value = 0.006
        links.new(noise.outputs['Fac'], bump.inputs['Height'])
        links.new(bump.outputs['Normal'], shader.inputs['Normal'])
        variation = nodes.new('ShaderNodeMapRange')
        variation.inputs['To Min'].default_value = roughness - 0.035
        variation.inputs['To Max'].default_value = roughness + 0.035
        links.new(noise.outputs['Fac'], variation.inputs['Value'])
        links.new(variation.outputs['Result'], shader.inputs['Roughness'])
    return mat


def extrude(name, outline, bottom, top, mat, collection):
    if sum(a[0] * b[1] - b[0] * a[1]
           for a, b in zip(outline, outline[1:] + outline[:1])) < 0:
        outline = list(reversed(outline))
    count = len(outline)
    vertices = [(x, y, z) for z in (bottom, top) for x, y in outline]
    faces = [tuple(reversed(range(count))), tuple(range(count, count * 2))]
    faces += [(i, (i + 1) % count, (i + 1) % count + count, i + count)
              for i in range(count)]
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(vertices, [], faces)
    mesh.update()
    obj = bpy.data.objects.new(name, mesh)
    collection.objects.link(obj)
    if mat:
        mesh.materials.append(mat)
    return obj


def bevel(obj, width):
    modifier = obj.modifiers.new('Soft machined edges', 'BEVEL')
    modifier.width = width
    modifier.segments = 6
    modifier.limit_method = 'ANGLE'
    for face in obj.data.polygons:
        face.use_smooth = True
    obj.data.use_auto_smooth = True
    normals = obj.modifiers.new('Face weighted normals', 'WEIGHTED_NORMAL')
    normals.keep_sharp = True


def capsule(x, y, width, height):
    radius = width / 2
    straight = height / 2 - radius
    return [(x + radius * math.cos(angle), y + center + radius * math.sin(angle))
            for center, start in ((straight, 0), (-straight, math.pi))
            for angle in (start + math.pi * i / 48 for i in range(49))]


def subtract(obj, cutter):
    bpy.context.view_layer.objects.active = obj
    modifier = obj.modifiers.new('Machined recess', 'BOOLEAN')
    modifier.operation = 'DIFFERENCE'
    modifier.solver = 'FAST'
    modifier.object = cutter
    bpy.ops.object.modifier_apply(modifier=modifier.name)
    mesh = cutter.data
    bpy.data.objects.remove(cutter, do_unlink=True)
    bpy.data.meshes.remove(mesh)


def light(scene, name, location, energy, color):
    data = bpy.data.lights.new(name, 'AREA')
    data.energy = energy
    data.shape = 'DISK'
    data.size = 7
    data.color = color
    obj = bpy.data.objects.new(name, data)
    scene.collection.objects.link(obj)
    obj.location = location
    obj.rotation_euler = (-obj.location).to_track_quat('-Z', 'Y').to_euler()


def strip_volatile_render_metadata(path):
    """Remove Blender timing text while preserving encoded image chunks."""
    volatile = {
        b'Date',
        b'RenderTime',
        b'cycles.ViewLayer.render_time',
        b'cycles.ViewLayer.total_time',
    }
    source = path.read_bytes()
    signature = b'\x89PNG\r\n\x1a\n'
    if not source.startswith(signature):
        raise ValueError(f'not a PNG: {path}')
    output = bytearray(signature)
    offset = len(signature)
    while offset < len(source):
        length = struct.unpack('>I', source[offset:offset + 4])[0]
        end = offset + 12 + length
        chunk_type = source[offset + 4:offset + 8]
        chunk_data = source[offset + 8:offset + 8 + length]
        keyword = chunk_data.split(b'\0', 1)[0] if chunk_type == b'tEXt' else None
        if keyword not in volatile:
            output.extend(source[offset:end])
        offset = end
    if offset != len(source):
        raise ValueError(f'invalid PNG chunk lengths: {path}')
    path.write_bytes(output)


# Start with a new scene rather than inheriting startup lighting or settings.
scene = bpy.data.scenes.new('Dictate · App icon')
bpy.context.window.scene = scene
for old in list(bpy.data.scenes):
    if old != scene:
        bpy.data.scenes.remove(old)
for obj in list(bpy.data.objects):
    bpy.data.objects.remove(obj, do_unlink=True)
for text in list(bpy.data.texts):
    bpy.data.texts.remove(text)
ivory = material(('Warm charcoal · porcelain' if DARK else 'Ivory · porcelain'),
                 ('2B211B' if DARK else 'F3EADA'), 0.58, 0.12, grain=True)
orange = material('Burnt orange · enamel', 'FF8A25', 0.56, 0.26)
camera_data = bpy.data.cameras.new('Front orthographic · no perspective')
camera = bpy.data.objects.new('Front orthographic · no perspective', camera_data)
scene.collection.objects.link(camera)
camera.location = (0, 0, 14)
camera_data.type = 'ORTHO'
camera_data.ortho_scale = 10
scene.camera = camera
light(scene, 'Key · broad upper left', (-3.8, 5.5, 7), 360, (1, 0.97, 0.93))
light(scene, 'Fill · cool right', (4, 1, 6), 200, (0.95, 0.97, 1))
world = bpy.data.worlds.new('Neutral studio · transparent film')
world.use_nodes = True
world.node_tree.nodes['Background'].inputs[0].default_value = (0.7, 0.7, 0.7, 1)
world.node_tree.nodes['Background'].inputs[1].default_value = 0.40
scene.world = world
collection = bpy.data.collections.new('Full cream face')
scene.collection.children.link(collection)
if ICON_COMPOSER:
    # Icon Composer applies the platform's outer mask. Extend the ceramic face
    # beyond its 8.44-unit camera frame so this bake has no second silhouette,
    # transparent corners, outside bevel, or shadow inside that system mask.
    # Keep the production body's dense perimeter topology outside the frame;
    # a four-vertex slab makes FAST boolean triangulation produce long shading
    # seams from the recesses to its corners.
    outline = []
    for i in range(256):
        angle = 2 * math.pi * i / 256
        outline.append(tuple(6 * math.copysign(abs(v) ** (2 / 4.5), v)
                             for v in (math.cos(angle), math.sin(angle))))
else:
    outline = []
    for i in range(256):
        angle = 2 * math.pi * i / 256
        outline.append(tuple(4.22 * math.copysign(abs(v) ** (2 / 4.5), v)
                             for v in (math.cos(angle), math.sin(angle))))
body = extrude('Cream body · subtle outside edge', outline,
                    0.0, 1.02, ivory, collection)
bevel(body, 0.045)
body.modifiers[0].segments = 8
body.modifiers[0].use_clamp_overlap = False
bpy.context.view_layer.objects.active = body
bpy.ops.object.modifier_apply(modifier=body.modifiers[0].name)
body.modifiers.clear()
scale = 1.35
# Initial C rectangles [left, top, width, height], in tile-relative pixels.
# Machine this source topology first, then reproduce the selected cap edits
# below. Cutting the final silhouettes directly would change boolean topology
# and float storage, rather than reproduce the locked study scene exactly.
rects = [(139.18, 447.38, 129.24, 268.43),
         (318.14, 313.17, 129.24, 536.85),
         (497.09, 382.76, 129.24, 397.67),
         (676.04, 164.04, 149.13, 685.98)]
bars = [((x + w/2 - 512)*TILE_PIXEL, (512 - y - h/2)*TILE_PIXEL,
         w*TILE_PIXEL, h*TILE_PIXEL) for x, y, w, h in rects]
for i, (x, y, width, height) in enumerate(bars, 1):
    pocket = capsule(x, y, width, height)
    cutter = extrude('Pocket cutter', pocket, -0.1, 1.5, None, collection)
    subtract(body, cutter)
    bed = extrude(f'Orange well {i}', pocket, 0.0, 0.47, orange, collection)
    bevel(bed, 0.065 * scale)
bevel(body, 0.085 * scale)
body.modifiers[0].limit_method = 'WEIGHT'
body.modifiers[0].use_clamp_overlap = False
body.modifiers[0].harden_normals = True
for edge in body.data.edges:
    edge.bevel_weight = 0
    a, b = (body.data.vertices[i].co for i in edge.vertices)
    if abs(a.z - 1.02) < .001 and abs(b.z - 1.02) < .001:
        for x, y, width, height in bars:
            radius = width / 2
            def on_rim(v):
                dy = max(abs(v.y - y) - (height / 2 - radius), 0)
                return abs(math.hypot(v.x - x, dy) - radius) < .002
            if on_rim(a) and on_rim(b):
                edge.bevel_weight = 1
if ICON_COMPOSER:
    # The uninterrupted ceramic face is planar. Keep only that face flat so
    # evaluated boolean diagonals cannot affect its shading; recess bevels and
    # walls retain the production scene's smooth weighted normals.
    for face in body.data.polygons:
        if all(abs(body.data.vertices[index].co.z - 1.02) < .001
               for index in face.vertices):
            face.use_smooth = False
meshes = [obj for obj in scene.objects if obj.type == 'MESH']
assert len(meshes) == 5
# C+16: 16 camera-canvas pixels at 1024, independent of export resolution.
# Well vertices are inside this region; all tile exterior vertices are outside.
camera_pixel = camera_data.ortho_scale / 1024
for obj in meshes:
    for vertex in obj.data.vertices:
        if obj.name.startswith('Orange well') or max(abs(vertex.co.x), abs(vertex.co.y)) < 3.4:
            vertex.co.x += C16_RIGHT_CAMERA_PX * camera_pixel
    obj.data.update()

# Bottom-up third pill: keep top 382.76; move bottom 780.43 to 748.12.
# Move the lower semicircle, not a scale transform, so its radius stays exact.
bed = bpy.data.objects['Orange well 3']
low = [min(v.co[i] for v in bed.data.vertices) for i in range(2)]
high = [max(v.co[i] for v in bed.data.vertices) for i in range(2)]
midline = (low[1] + high[1]) / 2
for obj in (body, bed):
    for vertex in obj.data.vertices:
        x, y = vertex.co[:2]
        if low[0]-1e-5 <= x <= high[0]+1e-5 and low[1]-1e-5 <= y <= high[1]+1e-5:
            delta = 0 if y > midline else PILL3_SOURCE_BOTTOM_TILE_PX - PILL3_BOTTOM_TILE_PX
            vertex.co.y += delta * TILE_PIXEL
    obj.data.update()

# Half: lower each bowl pill's upper semicircle by 69.595 tile pixels.
# Intermediate tile-relative top/bottom bounds are:
# p1 516.975–715.81, p2 382.765–850.02, p3 452.355–748.12.
for index in (1, 2, 3):
    bed = bpy.data.objects[f'Orange well {index}']
    low = [min(v.co[i] for v in bed.data.vertices) for i in range(2)]
    high = [max(v.co[i] for v in bed.data.vertices) for i in range(2)]
    midline = (low[1] + high[1]) / 2
    for obj in (body, bed):
        for vertex in obj.data.vertices:
            x, y = vertex.co[:2]
            if low[0]-1e-5 <= x <= high[0]+1e-5 and low[1]-1e-5 <= y <= high[1]+1e-5 and y > midline:
                vertex.co.y -= HALF_TOP_SHORTEN_TILE_PX * TILE_PIXEL
        obj.data.update()

# Bowl-up-more: move all three shortened bowl wells upward by 32 camera
# pixels = 0.3125 units. Stem remains at tile bounds 164.04–850.02.
# This is the locked Control: no Left4/Left8 or whole-glyph Y correction.
bowl_bounds = []
for index in (1, 2, 3):
    bed = bpy.data.objects[f'Orange well {index}']
    bowl_bounds.append(([min(v.co[i] for v in bed.data.vertices) for i in range(2)],
                        [max(v.co[i] for v in bed.data.vertices) for i in range(2)]))
for obj in meshes:
    for vertex in obj.data.vertices:
        x, y = vertex.co[:2]
        for low, high in bowl_bounds:
            if low[0]-1e-5 <= x <= high[0]+1e-5 and low[1]-1e-5 <= y <= high[1]+1e-5:
                vertex.co.y += BOWL_UP_CAMERA_PX * camera_pixel
                break
    obj.data.update()
bpy.context.view_layer.update()
dg = bpy.context.evaluated_depsgraph_get()
for obj in meshes:
    evaluated = obj.evaluated_get(dg)
    bm = bmesh.new()
    bm.from_mesh(evaluated.to_mesh())
    assert all(e.is_manifold for e in bm.edges), obj.name
    bm.free()
    evaluated.to_mesh_clear()
scene['Design'] = 'Locked v1 — Bowl-up-more Control: Bottom-up third pill, Half top-shortening of pills 1–3, then bowl-only lift; cream ceramic tile and orange debossed wells.'
scene['Orange albedo gain'] = 'Replaced by independent sRGB FF8A25 material; no emission.'
scene['Placement'] = 'C+16 horizontal placement retained; pills 1–3 lifted 32 camera-canvas pixels at 1024; stem unchanged; no later whole-glyph correction.'
scene.render.engine = 'CYCLES'
scene.cycles.use_denoising = False
scene.cycles.use_adaptive_sampling = True
scene.cycles.adaptive_threshold = 0.012
scene.render.threads_mode = 'FIXED'
scene.render.threads = 4
scene.render.resolution_x = scene.render.resolution_y = 1024 if ICON_COMPOSER else 2048
scene.render.resolution_percentage = 100
scene.render.film_transparent = True
scene.render.image_settings.file_format = 'PNG'
scene.render.image_settings.color_mode = 'RGBA'
scene.render.image_settings.color_depth = '8'
scene.view_settings.view_transform = 'Standard'
scene.view_settings.look = 'Medium High Contrast'
scene.view_settings.exposure = 0
scene.view_settings.gamma = 1
scene.cycles.samples = 512
if ICON_COMPOSER:
    # Geometry edits above deliberately use the locked 10-unit camera reference.
    # Frame the finished bake to the tile's 8.44-unit design area only after
    # those edits, preserving the approved glyph-to-tile optical scale.
    camera_data.ortho_scale = 8.44
    filename = ('dictate-icon-composer-dark-1024.png' if DARK
                else 'dictate-icon-composer-1024.png')
    scene.render.filepath = str(ROOT / 'dictate.icon' / 'Assets' / filename)
else:
    scene.render.filepath = '//dictate-2048.png'
for screen in bpy.data.screens:
    for area in screen.areas:
        if area.type == 'VIEW_3D':
            area.spaces.active.region_3d.view_perspective = 'CAMERA'
            area.spaces.active.shading.color_type = 'MATERIAL'
            area.spaces.active.overlay.show_overlays = False
if not ICON_COMPOSER:
    bpy.data.texts.load(__file__)
    bpy.ops.wm.save_as_mainfile(filepath=str(ROOT / 'dictate.blend'))
print('PASS: five manifold meshes; outside bevel 0.045; recess depth 0.55; locked v1 Bowl-up-more Control; '
      + (('full-bleed Icon Composer ' + ('dark' if DARK else 'finished') + ' bake')
         if ICON_COMPOSER else 'traditional transparent export'))
bpy.ops.render.render(write_still=True)
if ICON_COMPOSER:
    strip_volatile_render_metadata(Path(scene.render.filepath))
