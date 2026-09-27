# Dictate icon

Dictate uses one icon across Linux and macOS: a full cream rounded square with
four bright-orange recessed pills. The asymmetric waveform forms a secondary
lowercase-d silhouette through its raised three-pill bowl and tall right stem.
This Bowl-up-more Control design is the locked v1 identity. It is real Blender
geometry, built without downloaded textures or embedded raster sources.
Blender 3.4.1 was used for the approved scene.

`dictate.blend` is the editable source scene. `build_app_icon.py` is its
standalone, deterministic generator: it starts from Blender's factory state,
creates the geometry, materials, lights, camera, world, and render settings,
then writes both the scene and transparent 2048 px master. It does not read a
`.blend` input or import local helper modules. The builder is also embedded in
the generated scene.

The geometry uses a cream 4.22-half-width superellipse footprint, a subtle
0.045 outside bevel, and four orange beds recessed by 0.55. V1 retains the
C+16 horizontal placement, shortens the first three pills from their tops,
raises their bowl 32 camera-canvas pixels at 1024 px, and leaves the stem fixed.
The orange uses sRGB `FF8A25` converted to linear color, without emission or
extra lights.

## Rebuild and export

From the repository root:

```sh
blender --background --factory-startup --python packaging/icons/build_app_icon.py
blender --background --factory-startup --python packaging/icons/build_app_icon.py -- --icon-composer
blender --background --factory-startup --python packaging/icons/build_app_icon.py -- --icon-composer --dark
python3 packaging/icons/export_icons.py
```

Rendering requires Blender. Export requires Python 3 and ImageMagick 7.
The default Blender command overwrites `dictate.blend` and the 2048 px master;
the two Icon Composer commands write the full-bleed light and dark raster
layers directly into `dictate.icon/Assets` without changing the scene file.
Those modes remove Blender's volatile date and render-timing PNG text chunks
while preserving its encoded image data. Exporting overwrites the platform
PNGs and handcrafted ICNS file. Icon Composer itself owns `dictate.icon`
metadata.

## Exports and packaging

- `dictate-2048.png`: transparent RGBA app-icon master.
- `dictate-{size}.png`: Linux icon sizes from 16 through 1024 px.
- `dictate.icns`: ten PNG-backed macOS representations covering 16, 32, 128,
  256, and 512 pt at both 1× and 2×. This is the fallback for macOS builds
  without the required Xcode 26 toolchain.
- `dictate.icon`: layered macOS app-icon source authored with Icon Composer.
  Its Default and Dark appearances use the full-bleed Blender light and dark
  renders with Liquid Glass disabled; Mono uses the flat vector glyph. The
  appearance-specific layers switch through supported Color opacity variants.
  Xcode 26 builds compile it with `actool` for a macOS 14.0 deployment target
  and package both the generated `Assets.car` and `dictate.icns`.
- `platform/dev.joshthomas.dictate.svg`: flat vector companion retained for
  web and vector-reference uses; it is not installed as the Linux app icon.
- `platform/favicon.svg` and `platform/favicon-32.png`: flat web favicon.
- `platform/apple-touch-icon.png`: 180 px Blender-rendered touch icon.
- `platform/dictate-glyph-1024.svg`: transparent orange glyph layer for
  general-purpose layered formats.
- `platform/dictate-mark.svg`: tightly cropped orange mark for in-app and
  interface use without compensating for app-icon canvas padding.
- `platform/dictate-glyph-icon-composer-1024.svg`: full-bleed warm near-white
  Mono layer aligned to the Blender renders for Clear and Tinted appearances.

Linux installation places PNGs under
`~/.local/share/icons/hicolor/{size}x{size}/apps/dev.joshthomas.dictate.png` and
removes the former same-name scalable SVG so icon-theme lookup cannot select a
flat rendition instead. Stable and development desktop entries share that icon
name. On macOS, `tools/build-macos-app.sh` prefers `dictate.icon` when Xcode 26
or newer is available. It merges the icon name and file keys from `actool`'s
partial plist into the app's `Info.plist`. Older toolchains instead copy the
handcrafted `dictate.icns`; the fallback never replaces generated output on the
Icon Composer path.

See `platform/README.md` for exact vector geometry, colors, regeneration, and
cross-checking against the Blender scene.

The baked icon's perimeter is an optical continuous-corner approximation, not
an official Apple mask. Its PNG and handcrafted ICNS files are finished raster
assets, not Icon Composer layers. Do not apply another baked squircle inside
the system mask.
