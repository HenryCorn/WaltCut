# WaltCut app icon

**Motif:** a bouncing ball drawn as nine onion-skin frames: the earlier frames fade, the current
one (orange) lands on a ground line. It is the classic animation timing chart, and the onion-skin
view WaltCut uses when drawing cels.

## Palette

| Colour | Hex | Used for |
|---|---|---|
| Ink | `#141b2e` | the rounded-square tile |
| Paper | `#f4efe6` | the ghost frames and the ground line |
| Orange | `#ff8a3d` | the current frame (with a `#ffc28f` highlight) |

## Geometry

- A 1024-unit canvas, rounded square `rx=224`, full bleed.
- The macOS renders (`waltcut-macos-512.png`, `waltcut.icns`) put the tile in Apple's grid (824 of
  1024 px, transparent margin) so the Dock shows it at the same size as other apps.

## Files

| File | What |
|---|---|
| `waltcut.svg` | canonical artwork |
| `waltcut-1024.png` | 1024 px full-bleed render |
| `waltcut-macos-512.png` | runtime Dock icon on macOS (embedded by `apps/waltcut/src/main.rs`) |
| `waltcut.icns` | macOS bundle icon (`CFBundleIconFile`) |
| `generate.py` | draws the PNGs (standard library only); the SVG uses the same constants |

`packaging/icons.sh` regenerates the derived files. Licence: [LICENSE.txt](LICENSE.txt) (MIT OR Apache-2.0).
