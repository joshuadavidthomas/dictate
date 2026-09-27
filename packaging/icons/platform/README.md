# Locked v1 platform assets

Flat vector companions to the approved Bowl-up-more Control Blender icon.
These do not replace or modify the rendered production PNG/ICNS assets.

| File | Purpose |
| --- | --- |
| `dictate-glyph-1024.svg` | Transparent 1024 canvas; four opaque orange pills only |
| `dictate-mark.svg` | Tightly cropped orange mark for in-app and interface use |
| `dictate-glyph-icon-composer-1024.svg` | Full-bleed Mono layer aligned to the Blender light/dark renders |
| `dev.joshthomas.dictate.svg` | Flat web/vector companion; cream tile and orange glyph |
| `favicon.svg` | Self-contained web icon, identical to the flat vector companion |
| `favicon-32.png` | 32 × 32, 8-bit RGBA fallback rasterized from the SVG |
| `apple-touch-icon.png` | 180 × 180, 8-bit RGBA downscale of the approved Blender master |
| `generate.py` | Reproducible generator; Python 3 standard library and ImageMagick 7 |
| `verify_scene.py` | Read-only bounds check against the locked Blender scene |

## Identity colors and silhouette

Solid sRGB cream **#F3EADA**, solid sRGB orange **#FF8A25**. Both are fully
opaque. The canvas outside the tile is transparent. No gradients, lighting,
filters, raster embeds, external dependencies, or provenance metadata occur
in the SVGs. The title is an accessibility label, not drawn lettering.

The tile uses the production model's 256-point superellipse outline:
`x,y = 4.22 × sign(cos/sin(t)) × abs(cos/sin(t))^(2/4.5)`, sampled at
`t = 2πi/256`. Canvas mapping is `(512 + 102.4x, 512 − 102.4y)`.
Its bounds are **79.872–944.128** on both axes. Polygon coordinates are
written to six decimal places. There is no baked outer bevel or shadow.

Pills use analytic SVG semicircular caps (`rx = width/2`), matching the
nominal well profiles rather than tracing shaded pixels or the inner bevel.
Blender approximates those arcs with 48 segments per semicircle; the vector
caps stay circular at any scale.

The general-purpose glyph uses the traditional 10-unit camera canvas below.
The Icon Composer glyph instead uses warm near-white **#FFF8EE** so Clear and
Tinted appearances retain contrast, and uses the tile-space bounds directly
because its Blender raster layers frame the 8.44-unit tile area to the full
1024 canvas. Each of its pill widths is 94% of the matching well, narrowed
around its own center without changing height or placement. This preserves
the rendered icon's optical gaps in flat Mono appearances.

## Exact locked-v1 transforms

The original tile-relative rectangles `(left, top, width, height)` are:

```text
139.18  447.38  129.24  268.43
318.14  313.17  129.24  536.85
497.09  382.76  129.24  397.67
676.04  164.04  149.13  685.98
```

1. Pill 3 bottom becomes **748.12** (Bottom-up).
2. Pills 1–3 tops increase by **69.595 tile pixels** (Half).
3. All pills retain **+16 camera-canvas pixels X** (C+16).
4. Pills 1–3 move **32 camera-canvas pixels upward**; stem is unchanged.
5. No subsequent horizontal or vertical centering correction.

Tile pixels represent `8.44/1024` Blender units. Camera-canvas pixels
represent `10/1024` Blender units. Thus a tile coordinate `t` maps to
`512 + (t − 512) × 0.844`, followed by the camera-space translations.
Do not apply the 32-pixel bowl lift as a tile-relative shift.

Final exact camera-canvas bounds at 1024:

| Pill | Left | Top | Width | Height |
| --- | ---: | ---: | ---: | ---: |
| 1 | 213.33992 | 484.19890 | 109.07856 | 167.81674 |
| 2 | 364.38216 | 370.92566 | 109.07856 | 394.36322 |
| 3 | 515.41596 | 429.65962 | 109.07856 | 249.62566 |
| 4 | 666.44976 | 218.32176 | 125.86572 | 578.96712 |

Final tile-relative bounds, including camera translations, are derived
without rounding by reversing that mapping. For example, pill 1 left is
`139.18 + 16/0.844`, top is `516.975 − 32/0.844`, width is `129.24`,
height is `198.835`. The verifier prints both coordinate systems for all
four pills. Nominal SVG bounds differ from stored Blender float32 bounds
by at most **0.000019 camera pixels** in the locked scene.

## Regenerate and verify

Run from the repository root:

```sh
python3 packaging/icons/platform/generate.py
blender --background --factory-startup --python-exit-code 1 --python packaging/icons/platform/verify_scene.py
```

The generator resolves assets relative to its own location. It writes only
the seven generated assets listed above. The verifier never saves the scene.

Rasterization uses ImageMagick's explicit `MSVG:` built-in renderer at 1024,
then Lanczos downsampling. This avoids an optional external SVG delegate.
The touch icon instead downsamples `../dictate-2048.png` directly, preserving
the approved rendered colors, transparency, and deboss. Transparent corners
are deliberately retained in the requested RGBA asset; final platform
integration may impose its own presentation policy.

These commands can reproduce additional review sizes without changing assets:

```sh
magick -background none MSVG:packaging/icons/platform/favicon.svg -filter Lanczos -resize 64x64 -depth 8 -define png:color-type=6 /tmp/dictate-flat-64.png
```

Generation verified with Python 3, ImageMagick 7.1.2-25, and Blender 3.4.1.
The vector geometry is deterministic; identical raster bytes require the
same ImageMagick version. SVGs are deliberately flat companions, not claims
of pixel equivalence with the illuminated Blender render.
