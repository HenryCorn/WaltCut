# Attribution

Every non-code asset in FilmCraft, with its author, source and licence. Each file listed here also has
a `<file>.attribution` sidecar with full details. The rules are in [AGENTS.md](AGENTS.md) §1: no Adobe
iconography, images or other Adobe assets; open licences only; every asset attributed.
`cargo xtask assets` checks that this index and the sidecars cover every asset in the repository.

## Asset files

| Asset | Author | Source | Licence |
|---|---|---|---|
| `assets/fonts/Inter-Regular.ttf` | The Inter Project Authors | https://github.com/rsms/inter | OFL-1.1 (`assets/fonts/OFL-Inter.txt`) |
| `assets/fonts/Inter-Medium.ttf` | The Inter Project Authors | https://github.com/rsms/inter | OFL-1.1 (`assets/fonts/OFL-Inter.txt`) |
| `assets/fonts/Inter-SemiBold.ttf` | The Inter Project Authors | https://github.com/rsms/inter | OFL-1.1 (`assets/fonts/OFL-Inter.txt`) |
| `assets/fonts/Inter-Bold.ttf` | The Inter Project Authors | https://github.com/rsms/inter | OFL-1.1 (`assets/fonts/OFL-Inter.txt`) |
| `assets/fonts/Inter-Italic.ttf` | The Inter Project Authors | https://github.com/rsms/inter | OFL-1.1 (`assets/fonts/OFL-Inter.txt`) |
| `assets/fonts/NotoSerif-Regular.ttf` | The Noto Project Authors | https://github.com/notofonts/latin-greek-cyrillic | OFL-1.1 (`assets/fonts/OFL-NotoSerif.txt`) |
| `assets/fonts/JetBrainsMono-Regular.ttf` | The JetBrains Mono Project Authors | https://github.com/JetBrains/JetBrainsMono | OFL-1.1 (`assets/fonts/OFL-JetBrainsMono.txt`) |
| `docs/images/filmcraft-hero.png` | FilmCraft contributors | Original work: FilmCraft screenshot; NASA Apollo 11 film and TV footage (images.nasa.gov; US Government work, public domain; NASA does not endorse this project), subtitles from the NASA Apollo 11 air-to-ground transcript (public domain), CC0 Chopin (Musopen) | MIT OR Apache-2.0; footage and transcript public domain |
| `docs/images/filmcraft-keyframes.png` | FilmCraft contributors | Original work: FilmCraft screenshot; frames from Night of the Living Dead (1968, public domain) | MIT OR Apache-2.0; film frames public domain |
| `docs/images/filmcraft-export.png` | FilmCraft contributors | Original work: FilmCraft screenshot; frame from Night of the Living Dead (1968, public domain) | MIT OR Apache-2.0; film frames public domain |
| `docs/images/filmcraft-assembly.png` | FilmCraft contributors | Original work: FilmCraft screenshot; frames from Carnival of Souls (1962), Night of the Living Dead (1968), thumbnails of all demo media | MIT OR Apache-2.0; film frames public domain |
| `docs/images/filmcraft-timeline.png` | FilmCraft contributors | Original work: FilmCraft UI crop; thumbnails from Night of the Living Dead (1968) and Carnival of Souls (1962), CC0 Chopin waveform | MIT OR Apache-2.0; film frames public domain |
| `assets/app-icon/filmcraft.svg` | Brandon Thomas (FilmCraft owner) | Original work: FilmCraft app icon (owl with ear tufts), drawn by the owner in ArtCraft and vectorised; canonical vector | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `packaging/macos/dmg/background.svg` | @XusBadia | Original work: FilmCraft macOS DMG window background, derived from the FilmCraft app icon (`assets/app-icon/filmcraft.svg`); rendered to `background.tiff` by `packaging/macos/dmg/generate.py` | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) |
| `packaging/macos/dmg/background.tiff` | @XusBadia | Rendered from `packaging/macos/dmg/background.svg` by `packaging/macos/dmg/generate.py` | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) |
| `assets/app-icon/filmcraft-small.svg` | Brandon Thomas (FilmCraft owner) | Original work: FilmCraft app icon (owl with ear tufts), drawn by the owner in ArtCraft and vectorised; lighter vector | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `assets/app-icon/filmcraft-1024.png` | Brandon Thomas (FilmCraft owner) | Original work: FilmCraft app icon (owl with ear tufts), drawn by the owner in ArtCraft and vectorised; 1024 px render | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `assets/app-icon/filmcraft-macos-512.png` | Brandon Thomas (FilmCraft owner) | Original work: FilmCraft app icon (owl with ear tufts), drawn by the owner in ArtCraft and vectorised; macOS runtime Dock icon (512 px) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `assets/app-icon/filmcraft.icns` | Brandon Thomas (FilmCraft owner) | Original work: FilmCraft app icon (owl with ear tufts), drawn by the owner in ArtCraft and vectorised; macOS .icns | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |

## Golden test images

Reference frames for the renderer golden tests (`crates/golden`), rendered by FilmCraft itself from
procedural demo footage (`crates/media/src/generators.rs`); no external media. Regenerate with
`FILMCRAFT_BLESS=1 cargo test -p waltcut-golden`.

| Asset | Author | Source | Licence |
|---|---|---|---|
| `crates/golden/goldens/blend_modes.png` | FilmCraft contributors | Original work: golden reference rendered by FilmCraft (crates/golden/tests/golden.rs, scene `blend_modes`) from procedurally generated media | MIT OR Apache-2.0 |
| `crates/golden/goldens/graphic_shapes.png` | FilmCraft contributors | Original work: golden reference rendered by FilmCraft (crates/golden/tests/golden.rs, scene `graphic_shapes`) from procedurally generated media and bundled OFL fonts | MIT OR Apache-2.0 |
| `crates/golden/goldens/graphic_rich_text.png` | FilmCraft contributors | Original work: golden reference rendered by FilmCraft (crates/golden/tests/golden.rs, scene `graphic_rich_text`) from procedurally generated media and bundled OFL fonts | MIT OR Apache-2.0 |
| `crates/golden/goldens/log_to_rec709.png` | FilmCraft contributors | Original work: golden reference rendered by FilmCraft (crates/golden/tests/golden.rs, scene `log_to_rec709`) from procedurally generated media re-encoded as S-Log3 | MIT OR Apache-2.0 |
| `crates/golden/goldens/hdr_tone_map.png` | FilmCraft contributors | Original work: golden reference rendered by FilmCraft (crates/golden/tests/golden.rs, scene `hdr_tone_map`) from procedurally generated media re-encoded as Rec. 2100 PQ | MIT OR Apache-2.0 |
| `crates/golden/goldens/graphic_title.png` | FilmCraft contributors | Original work: golden reference rendered by FilmCraft (crates/golden/tests/golden.rs, scene `graphic_title`) from procedurally generated media and bundled OFL fonts | MIT OR Apache-2.0 |
| `crates/golden/goldens/transform_opacity.png` | FilmCraft contributors | Original work: golden reference rendered by FilmCraft (crates/golden/tests/golden.rs, scene `transform_opacity`) from procedurally generated media | MIT OR Apache-2.0 |

## Third-party media shown in screenshots

The README screenshots show frames from these works. The media files themselves are not in the
repository; each screenshot's sidecar lists exactly which appear.

| Work | Author | Status | Source |
|---|---|---|---|
| Night of the Living Dead (1968) | George A. Romero / Image Ten | US public domain (no copyright notice) | https://archive.org/details/night-of-the-living-dead-1968-english |
| Carnival of Souls (1962) | Herk Harvey / Harcourt Productions | US public domain | https://archive.org/details/CarnivalOfSoulsVideoQualityUpgrade |
| Charade (1963) | Stanley Donen / Universal | US public domain (defective notice) | https://archive.org/details/charade-1963-cary-grant-audrey-hepburn-comedy-mystery-romance-thriller-full-movie |
| Earth Views from the ISS (NHQ_2020_1221) | NASA | US Government work, public domain (no NASA endorsement implied) | https://images.nasa.gov/details/NHQ_2020_1221_Earth%20Views |
| Chopin: Nocturne Op. 48 No. 1, Ballade No. 1 | Musopen | CC0-1.0 | https://archive.org/details/musopen-chopin |

## Assets drawn or generated in code

These are original work by FilmCraft contributors under the project licence (MIT OR Apache-2.0). None
is traced or derived from Adobe artwork.

| Asset | Where |
|---|---|
| UI icons (tools, transport, panels, header) | `crates/ui-egui/src/icons.rs`: vector paths on a 16×16 grid |
| Source and Program range braces and trim hover cues | `crates/ui-egui/src/panels/monitor.rs` and `source_range.rs`: original generic vector shapes by FilmCraft contributors, MIT OR Apache-2.0; no third-party artwork |
| Align / distribute / paragraph-alignment glyphs in the graphics panel | `crates/ui-egui/src/panels/graphics.rs` (`align_glyph`): bars and lines drawn with the egui painter |
| Demo footage (ocean sunset, aurora, city night, dunes, forest, plasma), bars and tone, counting leader, colour matte | `crates/media/src/generators.rs` |
| Colour palette and layout metrics | `crates/ui-egui/src/theme.rs`: colour values and sizes only; no artwork |

## Optional build input: craft-fonts

These fonts are **not files in this repository**. Builds made with the optional build input
`CRAFT_FONTS_DIR` (all official releases; see [docs/contributing.md](docs/contributing.md#building-with-craft-fonts))
embed them from [storytold/craft-fonts](https://github.com/storytold/craft-fonts), where each file,
its author, pinned upstream source and licence text are listed in its
[ATTRIBUTION.md](https://github.com/storytold/craft-fonts/blob/main/ATTRIBUTION.md). Release packages
carry each font's licence as `OFL-<family>.txt`.

| Font | Use in FilmCraft | Licence |
|---|---|---|
| BIZ UDPGothic Regular, Bold (Morisawa) | Japanese UI text; Japanese fallback for sans titles | OFL-1.1 |
| Shippori Mincho Regular (FONTDASU) | Japanese fallback for serif titles | OFL-1.1 |
| BIZ UDMincho Regular (Morisawa) | Japanese fallback for serif titles | OFL-1.1 |
