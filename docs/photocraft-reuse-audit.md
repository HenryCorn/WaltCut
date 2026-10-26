# PhotoCraft reuse audit for the drawing foundations (#74)

Research for epic #53. Nothing here authorises a dependency, a copy of source or a code change: any
reuse needs its own reviewed implementation issue (#74 delivery contract).

**Verdict: no reuse now.** Every drawing foundation the epic asked for (#54 to #71) already exists
in WaltCut as stroke data plus a small CPU rasteriser. The PhotoCraft components solve a different
problem (a raster painting engine over tiled pixel surfaces), so extracting them would add a second
pixel model and a Photoshop-parity settings surface that WaltCut's guardrails exclude. One idea is
worth an independent re-implementation: a pulled-string stabilizer mode (follow-up issue linked from
#74).

## Pinned evidence

* Repository: `storytold/photocraft`, `main` at `59cb4c49bd236df04d8e37d4764253f29ca6600a`
  (committed 2026-10-25). Cloned read-only into a scratch directory; nothing in `../photocraft` or in
  WaltCut's tree was touched. No sibling checkout existed at `../photocraft` on the audit machine.
* WaltCut: `origin/main` at the time of the audit (after #71).
* Observed by running it: `cargo tree -p photocraft-paint -e normal` and `cargo test -p
  photocraft-paint` (129 passed, 0 failed, 1.42 s of test time; the build was about 12 s wall on an
  Apple-silicon laptop).
* Observed by reading: the files named below.
* Documentation claims, not observed: the README calls PhotoCraft "early alpha"; `docs/brush-parity.md`
  (its own words) puts brush breadth at about 90 % and "ready for real work" at about 55 %, with
  feel (latency, pressure response, smoothing) "to be checked on real tablets". I did not run
  PhotoCraft's app or any tablet.

## Licence and provenance (verified at the pin)

| Item | Finding |
|---|---|
| Declared licence | Workspace `Cargo.toml`: `license = "MIT OR Apache-2.0"`. `LICENSE-MIT` ("Copyright (c) 2026 ArtCraft Team and the PhotoCraft contributors") and `LICENSE-APACHE` (standard Apache-2.0 text) are both present. GitHub's detector reports only `Apache-2.0`. Compatible with WaltCut's `MIT OR Apache-2.0`. |
| Branding | `NOTICE`: the ArtCraft name and logos are trademarks outside the licence. WaltCut removed ArtCraft branding in #26, so no extract may carry names, logos or `docs/brand/` material. |
| Contributors | 6301 commits, 540 author identities (top: 914, 781, 293, 169 commits). I found no CLA or DCO file and did not check whether every contributor licensed their work under the stated terms. **Not verified.** |
| Clean-room stance | PhotoCraft's `AGENTS.md` says it was written by studying Photoshop "for behaviour and look only" and states a goal of 1:1 Photoshop parity. I cannot verify that claim. The brush model is explicitly Photoshop-shaped (`crates/paint/src/lib.rs` header; `Smoothing` is "Photoshop's Smoothing %"). Copying it would import a parity-driven design whose provenance WaltCut cannot independently confirm. Adobe assets are the hard line in WaltCut `AGENTS.md` section 1; PhotoCraft also ships an ABR (Photoshop brush) reader in `crates/psd/src/abr.rs`, which WaltCut must not take. |
| Third-party material | `NOTICE` lists fonts, Lucide icons, a word list and libfacedetection weights. None are in the paint, raster or tablet crates; none would come with an extract. |
| Dependency policy | No `deny.toml` or cargo-deny config in the tree. |

## Dependency, safety and layering boundary

`photocraft-paint` (about 9 400 lines) pulls 18 crates in total: `photocraft-{paint,raster,color,cms,geom}`
(first party), `serde` (+ derive, `syn` 3, `quote`, `proc-macro2`) and `rayon` (+ crossbeam, `either`).
WaltCut already depends on `serde` and `rayon`, so the new third-party surface is nil; the new
first-party surface is four crates (`raster` 2 188 lines, `color` 1 558, `geom` 2 000, `cms`) that
WaltCut would have to vendor or pin by git revision.

* `unsafe`: `paint` and `raster` are `#![forbid(unsafe_code)]`; the workspace forbids it. The tablet
  crate allows `unsafe` in `macos.rs` and `windows.rs` (three AppKit calls on macOS).
* Never-crash (WaltCut `AGENTS.md` section 0): the crates deny clippy `unwrap/expect/panic`, but the
  lint does not cover `assert!`, `assert_eq!` or indexing. A crude scan of non-test code (text before
  the first `#[cfg(test)]`) found 5 such sites in `raster/src/lib.rs` and 1 in `paint/src/tile.rs`
  (`GrayTile::from_f32` uses `assert_eq!` on input-derived sizes and `GrayTile::get` indexes
  unchecked). Indexing was not counted. Any extract would need a hostile-input pass and new
  regression tests; this is real review cost, not zero.
* Layering (`cargo xtask layers`): the paint/raster/color/geom/cms crates have no UI dependency, so
  they would sit at a low layer without breaking the rule. WaltCut's `waltcut-render` works on one
  `Image` (premultiplied linear RGBA `f32`, `crates/render/src/image.rs`); PhotoCraft's `StrokeRenderer`
  composites into a tiled `Surface` in a `PixelFormat` (any colour mode, 8/16/32 bit). Bridging the
  two means a conversion at every cel raster or adopting `Surface` in the renderer.

## What WaltCut already has (so it is not rebuilt)

| Area | WaltCut today (file) |
|---|---|
| Stroke model | `crates/project/src/drawing.rs`: `Stroke` = brush, colour, size, pressure-sampled points, optional stored `response` curves and `Arc<Fill>`; caps `MAX_STROKE_POINTS` 20 000, `MAX_CEL_STROKES` 5 000 |
| Rasteriser | `crates/render/src/drawing.rs`: round-capped segments, per-stroke max coverage (no dark joints), eraser removes earlier paint, premultiplied linear output |
| Pressure curves | `Curve` (2 to 8 points, piecewise linear, presets) in `drawing.rs`; `crates/ui-egui/src/panels/pressure_curve.rs` |
| Stabilizer | `crates/ui-egui/src/panels/drawing_smooth.rs`: adaptive moving average, catch-up at pen-up; measured 8 ns per sample; a rope stabilizer is listed as "not in this version" in `docs/animation.md` |
| Pen input | `crates/platform/src/pen.rs` (AppKit monitor, bounded sample queue) and `panels/drawing_input.rs` (thinning, per-event pressure and time) |
| Selection | `crates/project/src/drawing_select.rs`, `crates/engine/src/stroke_select.rs`: lasso over whole strokes with ownership and landing rules |
| Fill | `crates/render/src/drawing_fill.rs`, `crates/project/src/drawing_fill.rs`, `crates/engine/src/drawing_fill.rs`: deterministic 4-connected flood, tolerance, reference, gap closure, edge growth, stored as a mask in the cel |
| Undo | Command per edit; cels are `Arc<Vec<Stroke>>`, so a snapshot is a pointer (measured undo of a stroke: 0.00 ms, `docs/performance.md`) |
| Cache | `crates/render/src/drawing_cache.rs`: identity-keyed raster cache with prefix extension, 256 MiB budget (#71) |
| Time | Cels, holds, links, onion skins, retime: all in `drawing.rs` and the timeline |

Measured baseline from `docs/performance.md` (#70): rasterising one 1080p cel costs 2.0 ms (small),
40.8 ms (medium) and 635 ms (stress) on the development machine. Those numbers are the actual case
for any raster work; they predate #71's cache, whose off/on figures are in the same file.

## Candidate matrix

PhotoCraft file paths are relative to the pinned commit. Effort is a rough engineering estimate for
a reviewed, tested integration in WaltCut, not a measurement.

| Candidate | PhotoCraft has | WaltCut has | Risk | Effort | Recommendation |
|---|---|---|---|---|---|
| Brush dynamics (dab engine) | `crates/paint/src/{brush,dynamics,render,rng}.rs`: computed and sampled tips, spacing, scatter, texture, dual brush, colour dynamics, build-up, seeded jitter keyed by dab index, incremental path walker; 129 tests pass | Four fixed brushes with a capsule-chain rasteriser; stroke list is the stored form | Photoshop-parity model and unverifiable provenance; the settings surface (`BrushSettings`, 828 lines) is exactly the "huge brush catalogue" the epic excludes; Surface/PixelFormat mismatch; panic sites above | Extract 3 to 5 weeks incl. hardening and a stored-format decision (stroke list vs dabs) | **Skip.** Not needed for the pen, pencil, marker and eraser workflow. If textured brushes are ever wanted, reimplement a minimal dab walker from the public idea (spacing along a path), keeping `Stroke` as the stored form. |
| Stabilizer | `Smoothing` (`brush.rs`) plus `dynamics.rs`: exponential smoothing, pulled string up to 100 px (`MAX_STRING_PX`), catch-up, timestamps, zoom adjustment | Adaptive moving average with pen-up catch-up; no rope mode | Low: the algorithm is a standard lazy-mouse; tied to `StrokePoint` f64 and `BrushSettings` | Reimplement about 2 to 3 days with WaltCut's tests | **Reimplement** the pulled-string mode only (follow-up issue). Do not extract the file. |
| Pressure curve | `curve.rs` (173 lines): monotone cubic (Fritsch-Carlson), up to 16 points, `Copy`, no allocation | `Curve` of 2 to 8 points, piecewise linear, validated, stored per stroke | Low, but swapping changes the rendering of already stored strokes (`response` is saved with each stroke), so it would need a schema bump | 1 day | **Skip.** Piecewise-linear is documented behaviour and part of saved projects. |
| Selection | `crates/algo/src/selection*.rs`: pixel masks (marquee, lasso built straight into tiles, magic wand, colour range), about 2 400 lines | Whole-stroke lasso with transform rules; no pixel selection | Different model: PhotoCraft selects pixels, WaltCut selects strokes. `algo` pulls the whole algorithm crate. | n/a | **Skip.** A pixel mask has no meaning on a stroke list. |
| Fill | `algo/src/selection.rs` `magic_wand` / `wand_region` (flood by tolerance on pixels); paint-bucket in `engine/src/paint_cmds.rs` | Deterministic flood with reference layer, gap closure, edge growth, stored as an editable operation (#68, #69) | Re-implementing would regress #69's behaviour; PhotoCraft's fill writes pixels into a layer, not a stored operation | n/a | **Skip.** WaltCut is ahead on the animation-relevant parts. |
| Layers, blend modes | `crates/doc`, `crates/compose` (21 000 lines): PSD-grade layers, masks, adjustments, styles | Drawing clip placed on a track; blend and opacity from the timeline | Wholesale document model import is forbidden by #74 | n/a | **Skip.** |
| Colour management | `crates/cms` (ICC v2/v4), `crates/color` | sRGB straight alpha strokes, premultiplied linear compositing; colour helpers in `crates/color` | Adds ICC machinery to a macOS screen/export pipeline that does not need it | n/a | **Skip.** |
| History and tiled buffers | `crates/raster` (copy-on-write 256x256 tiles, `Arc` shared) and `crates/ops` `History` (document snapshots under a pixel budget) | Command undo with `Arc` strokes; raster cache stores whole cropped cels | `docs/animation.md` (#71) deliberately left dirty rectangles and tiles out because no measurement asked for them. The stress case (635 ms per cel) is a rasteriser cost, not an undo cost | 2 to 4 weeks | **No action now.** Revisit only if a profile shows a raster cache miss dominating stress playback; then compare an independent tiled cache against the 256 MiB cache. |
| Tablet input | `crates/tablet`: AppKit monitor, Windows hook, X11 XInput2, pure mapping modules, tilt and rotation | `crates/platform/src/pen.rs`: AppKit monitor, bounded queue, per-event time and pressure; macOS only | Low; same technique. WaltCut's is already integrated with its UI and tests. Windows and X11 code is out of scope (macOS only, #29) | n/a | **Skip.** Tilt and rotation could be read from the same `NSEvent` if ever needed; that is a few lines, not a reuse. |
| Rope/lazy-mouse, symmetry | `paint/src/symmetry.rs` (143 lines, four axis modes) | None | Low | 1 to 2 days | **Skip** for now; symmetry is not in the epic. |

## Extraction versus a small independent implementation

For the one item that merits action, compare the options:

* **Extract `dynamics.rs` plus `brush.rs`** (about 1 700 lines for the walker and settings): needs
  `rng.rs`, `curve.rs`, `tile.rs`, `geom` and the `StrokePoint` f64 type; brings dozens of Photoshop-named
  settings; and ties WaltCut to PhotoCraft's serde model.
* **Independent pulled-string mode**: a small amount of filter state (an estimate, not measured) added to the existing smoothing
  stage in `panels/drawing_smooth.rs`, using WaltCut's canvas-pixel samples and its existing
  determinism tests (same result however samples are batched). No new crate, no new serde field in
  stored projects (smoothing is a view setting).

The second is smaller, safer and keeps one provenance story. That is the recommendation.

## What I could not verify

* Whether each of the 540 contributors licensed their work under MIT OR Apache-2.0; no CLA or DCO
  was found, and I did not audit authorship per file.
* PhotoCraft's clean-room claim about Photoshop; I only read its stated policy.
* Pen feel, latency and tablet behaviour of either project (no tablet or camera rig; #5 still owns
  that evidence).
* PhotoCraft rasteriser speed against WaltCut's: I did not benchmark `StrokeRenderer`, so there is no
  evidence either way on whether it would be faster than the 40.8 ms and 635 ms cel rasters above.
* Indexing-panic exposure in the PhotoCraft crates (only asserts, unwrap, expect and panic were
  scanned), and the full transitive tree on non-macOS targets (checked on this host only).
* `cargo xtask layers` placement of any new crate; no extract was attempted.
