# WaltCut

WaltCut is a fork of [FilmCraft](https://github.com/storytold/filmcraft), refocused on drawing and
2D frame-by-frame animation over video. It keeps WaltCut's Rust editing engine and adds drawing
cels, pressure-aware brushes, onion skinning, in-between generation, and AI Restyle.

The general editing engine and much of the technical documentation come from WaltCut. This
repository's WaltCut-specific animation workflow is described in the [animation guide](docs/animation.md).
Upstream attribution and licensing are recorded in [NOTICE](NOTICE) and the
[MIT](LICENSE-MIT) / [Apache-2.0](LICENSE-APACHE) license files.

## Animation workflow

1. Open a project and sequence, or create a drawing above the footage from **Graphics and Titles ▸
   New Layer ▸ New Drawing**.
2. Choose the **Brush** tool (`Shift+P`) and draw in the Program monitor. Choose Pen, Pencil,
   Marker, or Eraser; use the cel mode to draw on the held cel, start a blank cel, or duplicate the
   held cel.
3. Step through the sequence, make new cels for changed poses, and use onion skins to see the
   previous and next cels. **In-between** interpolates strokes toward the next cel.
4. Optionally open **Graphics and Titles ▸ AI Restyle…** to restyle the frame at the playhead, a
   selected clip, or a sequence In/Out range. See the guide for provider and privacy details.

See [docs/animation.md](docs/animation.md) for the full workflow and current limitations.

### Audio reduction draft (#23)

**DRAFT IMPLEMENTATION — native macOS acceptance pending.** The stacked audio cuts retain
synchronized mono/stereo-source playback to the stereo soundtrack, waveforms,
scrubbing, clip Audio Gain and track mute. Intrinsic Volume remains audible, including keyframes:
its level adds to Audio Gain in dB; disabling/bypassing Volume does not bypass Audio Gain.
Advanced audio processing, transition commands and their catalogue/editors are deleted, not
disabled behind controls. Export loudness/limiting, surround/output-layout conversion and
Source-monitor mixdown are also deleted. Actual playback callback and written MOV/float-PCM
sample acceptance remains in `audio_timing`; WAV is an import fixture, not a user export.
#23 is not complete, accepted or merged.

### Export simplification draft (#25)

**IMPLEMENTATION DRAFTED — native macOS acceptance pending.** The only media delivery outputs are:

- **H.264 + AAC MP4**
- **ProRes 4444 MOV with alpha** and the stereo PCM soundtrack
- **PNG sequence with alpha**
- **PNG/JPEG still frames**

Export and Quick Export share destination, format, range, Start, progress/status and Cancel.
There is one owned export worker, not a queue or backlog. Delivery presets, advanced encoding
choices, hardware encoding, overlays/watermarks/burn-ins and other output formats are deleted;
removed commands and parameters fail explicitly. Existing movies/stills require explicit Replace,
PNG sequences require unused names, and failed/cancelled encoding preserves the previous destination
and project. See [the export contract](crates/export/README.md).

All 39 workspace crates and retained import/hardware-decoding paths remain. Native retained-import
fixture decoding, every retained output round trip (including frame count and alpha), actual
callback/MOV sample tests, a real iPhone HEVC Main10 HLG MOV + AAC manual check, full native CI and
the control-channel animation-flow screenshot are **pending**. No real iPhone file has been checked.
#25 is drafted, not complete, accepted or merged; no issue close-out or freeze is implied.

## Build from source

This fork retains the upstream Cargo package names for now:

```sh
cargo run --release -p waltcut
cargo run --release -p waltcut-cli -- commands
```

See [docs/contributing.md](docs/contributing.md) for Rust setup and build details, and
[AGENTS.md](AGENTS.md) for project-wide engineering and asset rules.

## Upstream documentation

The remaining architecture, command, and editing references describe the inherited WaltCut
engine and are retained for contributors:

- [Architecture](docs/architecture.md)
- [Agent and control-channel guide](docs/agents.md)
- [Graphics and titles](docs/graphics.md)
- [Roadmap](ROADMAP.md)
