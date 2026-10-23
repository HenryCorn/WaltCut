# Architecture

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** trivial (status line, revision history and roadmap link added; content checked against the crate list)

FilmCraft is a Cargo workspace of small crates with strictly enforced layering. The engine is
headless: every feature can be reached without a window, and the egui UI is one client among the
CLI, the JSON control channel and the MCP server.

Design principles:

1. **Engine-first.** Project-changing actions go through `Session::execute(id, params)`.
2. **Everything is a command.** Stable id, label, menu path, shortcut, parameter doc, `enabled()`
   predicate with a human-readable reason, and `run()`.
3. **Exact time.** Integer ticks, rational frame rates. No `f64` seconds in edit math.
4. **Copy-on-write snapshots.** The project is an `Arc<Project>`. Undo is a stack of snapshots, and
   background readers (playback, export) hold a snapshot without locking.
5. **CPU reference, GPU fast path.** The CPU compositor is the oracle. The GPU path is tested
   against it.
6. **Pure Rust, clean-room.** Codecs and containers are written from public specifications
   (see [AGENTS.md](../AGENTS.md)).

## 1. Layers

```text
 L6  apps/waltcut · apps/waltcut-cli · apps/waltcut-web
 L5  ui-egui · automation · platform
 L4  engine
 L3  render · gpu · export · golden (test-only)
 L2  edit · codecs
 L1  frame · media · project · audio-dsp · text
 L0  foundation: time · geom · color · bitstream · testkit (dev-dependency only)
     codecs/containers: isobmff · matroska · mpegts · h264 · h264enc · hevc · vp9 · av1 · mpeg2v · prores · aac · mpa · opus
```

Crates are named `filmcraft-<dir>` (`crates/time` is `waltcut-time`). The apps are `filmcraft`
and `filmcraft-cli`.

| Crate | Layer | Purpose |
|---|---|---|
| `time` | L0 | `Tick`, `FrameRate`, `TimeRange`, timecode parse/format (NDF/DF, frames, feet+frames, samples) |
| `geom` | L0 | `Vec2`, `Rect`, `Affine`, Motion-transform composition |
| `color` | L0 | colour spaces, transfer functions, YUV↔RGB matrices, LUTs |
| `bitstream` | L0 | bit reader/writer, Exp-Golomb, emulation prevention |
| `isobmff` | L0 | MP4/MOV demux and mux |
| `matroska` | L0 | MKV/WebM demux |
| `mpegts` | L0 | MPEG-2 Systems demux (H.222.0): transport streams (188 / 192-byte BDAV / 204), program streams (MPEG-2 / MPEG-1, VOB, MOD), PSI, PES, PTS / PCR, access-unit index built on open ([README](../crates/mpegts/README.md)) |
| `h264`, `h264enc` | L0 | H.264 decoder; H.264 encoder |
| `hevc` | L0 | H.265 Main/Main 10 decoder |
| `prores` | L0 | ProRes decoder and encoder, including 4444 with alpha |
| `av1` | L0 | AV1 decoder (Main profile; bit-exact with libdav1d; see its README for the stage table) |
| `mpeg2v` | L0 | MPEG-2 video (Main / 4:2:2 profile; frame and field pictures, dual prime) and MPEG-1 video decoder ([README](../crates/mpeg2v/README.md)) |
| `aac` | L0 | AAC-LC decoder and encoder |
| `mpa` | L0 | MPEG-1/2 Audio Layer I and II (MP1 / MP2) decoder ([README](../crates/mpa/README.md)) |
| `testkit` | L0 | test-only helpers, used only as a dev-dependency: ffmpeg/ffprobe discovery, fixture dirs, golden images ([testing.md](testing.md)) |
| `frame` | L1 | `VideoFrame` (planar YUV / RGBA8 / linear RGBA f32, colour metadata), `AudioBuffer` |
| `media` | L1 | `MediaSource` trait, probing/openers, frame cache, generators, stills, image sequences, WAV / Broadcast WAV |
| `project` | L1 | document model, effect definitions, keyframes |
| `audio-dsp` | L1 | dependency-free output-rate resampling and pitch-preserving shuttle helpers used by live playback |
| `text` | L1 | text engine: font database (bundled OFL fonts + system fonts), shaping (harfrust), bidi, line breaking, paragraph layout, glyph/path rasteriser, strokes ([crates/text/README.md](../crates/text/README.md)) |
| `edit` | L2 | pure edit algebra (insert, overwrite, razor, ripple, roll, slip, slide, rate stretch…) |
| `codecs` | L2 | container + codec hub: MP4/MOV, MKV/WebM, AVI and MPEG TS / PS sources, GOP-aware seeking, decoder registry, audio decoding |
| `render` | L3 | sequence evaluation, CPU compositor, retained Motion/Opacity/blend, graphic layers/keyframes and minimal mono/stereo audio mix |
| `gpu` | L3 | wgpu compositor (WGSL) |
| `golden` | L3 | test-only: golden-image tests of the CPU renderer and GPU-vs-CPU agreement; empty library, dev-dependencies only |
| `export` | L3 | render → encode → mux pipeline, progress/cancel |
| `engine` | L4 | `Session`, command registry, undo history, media pool, jobs |
| `ui-egui` | L5 | the egui frontend: docking, panels, timeline, monitors, playback, control-channel handlers |
| `automation` | L5 | MCP server (`rmcp`, stdio), headless or bridged to the running app |
| `platform` | L5 | OS media FFI only: hardware video decoding (VideoToolbox H.264 / HEVC on macOS; Media Foundation / Direct3D 11 H.264 / HEVC / VP9 / AV1 on Windows; VA-API H.264 / HEVC on Linux, with our own parsers and DPBs driving the stateless hardware; a no-op elsewhere) behind `codecs::VideoDecoder`, with transparent fallback to our decoders, and hardware H.264 encoding (VideoToolbox, opt-in) and H.265 encoding (VideoToolbox, the only H.265 encoder; the format exists only where a hardware encoder does) and NVIDIA NVENC H.264 (Windows and 64-bit Linux, opt-in) and H.265 (Windows, FilmCraft's only H.265 encoder there) encoding behind `export::VideoEncoder`; H.264 declines to the built-in encoder for what the hardware does not take. The one crate allowed `unsafe` ([ADR 0001](adr/0001-platform-ffi.md), [README](../crates/platform/README.md)) |
| `filmcraft` | L6 | desktop binary: eframe/wgpu window, cpal audio output, file dialogs, native macOS menu, TCP control server |
| `filmcraft-cli` | L6 | headless CLI: `exec`, `run`, `inspect`, `describe`, `commands`, `import`, `export`, `render`, `probe`, `mcp`; `--bridge` targets the running app |

### What `cargo xtask layers` enforces

The table of layers lives in `xtask/src/main.rs` (`LAYERS`). It also reserves names for planned
crates. The check reads `cargo metadata` and looks at normal and build dependencies (dev-dependencies
are exempt):

| Rule | Detail |
|---|---|
| Every crate has a layer | A new crate fails the check until it is added to `LAYERS`. |
| Only downward edges | A crate may not depend on a crate in a higher layer. |
| Same-layer edges are listed | From L1 up, a same-layer edge must be in `SAME_LAYER`: `media→frame`, `project→media`, `project→frame`, `gpu→render`, `export→render`, `cli→filmcraft`, plus a few reserved for planned crates. |
| L0 codecs stay standalone | L0 crates other than `time`, `geom`, `color`, `bitstream`, `testkit` may depend on no workspace crate except `waltcut-bitstream`. External crates such as `thiserror` and `rayon` are allowed. |
| No UI/OS crates below L5 | `egui`, `eframe`, `egui-wgpu`, `winit`, `rfd`, `cpal`, `muda` are allowed only in L5 and L6. |

`unsafe_code = "forbid"` applies workspace-wide, except in `platform`
(`deny`, allowed only on its FFI modules; [ADR 0001](adr/0001-platform-ffi.md)).

## 2. Time base

All time is `waltcut_time::Tick(i64)` at `TICKS_PER_SECOND = 254_016_000_000`.

That number divides evenly into the frame duration of every broadcast rate (23.976, 24, 25, 29.97,
30, 48, 50, 59.94, 60, 120…) and the sample duration of every common audio rate (8 kHz to 192 kHz,
including the 44.1 kHz family). So frame and sample positions are exact integers, edits never drift
at 29.97, and audio and video line up to the sample.

| Type | Use |
|---|---|
| `Tick` | timeline and media positions and durations |
| `FrameRate { num, den }` | `frame_duration()`, `tick_of(frame)`, `frame_at(tick)`, `snap(tick)` |
| `TimeRange` | half-open `start + duration` |
| Timecode | display only (SMPTE NDF/DF, frames, feet+frames, samples); `parse_timecode` and `format_time` |

Commands take time as `time` (ticks), `frame`, `seconds` or `timecode`; the engine converts once at
the boundary.

## 3. Data model (`waltcut-project`)

```text
Project
├─ root: Bin                         tree of bins
├─ items: map ItemId → ProjectItem   flat
│    kind: Media(MediaClip) | Sequence(Sequence) | Subclip{..} | Graphic{..}
└─ next_id

Sequence
├─ settings: frame rate, size, sample rate, …
├─ video_tracks / audio_tracks: Vec<Track>
├─ markers, mark_in / mark_out
Track
├─ locked, sync lock, targeting, mute/visibility
├─ items: Vec<TrackItem>             sorted, never overlapping
└─ audio track state: channel layout, mute, enabled
TrackItem (a clip instance)
├─ item: ItemId, start (timeline ticks), source_in (media ticks), duration, speed
├─ link group, label, enabled
└─ effects: Vec<EffectInstance>      intrinsic Motion/Opacity/Volume… first, then standard effects
EffectInstance
└─ effect id, enabled, params: id → constant value or keyframe track
```

- **Graphic clips** ([graphics.md](graphics.md)) reference a `Graphic` canvas item; their text and
  shape layers are hidden `graphic_text` / `graphic_shape` effect instances on the track item.
- Everything is plain serde data. `Sequence::check()` validates the invariants (no overlaps, unique
  ids), and the engine runs it after every sequence edit.
- Timeline positions are sequence ticks; `source_in` and keyframes are in media time, so trims and
  splits never move keyframes.
- **Effect definitions are data.** `project::effect::effect_defs()` lists every video effect,
  audio effect and transition with its parameter schema. The Effects panel tree, the Effect
  Controls rows and the parameter docs agents see are all generated from it.
- **Project files** (`.fcproj`) are the project serialised as JSON. Saves are atomic: write a
  temporary sibling file, then rename it over the target.

## 4. Command system (`waltcut-engine`)

```rust
pub struct CommandSpec {
    pub id: &'static str,                 // "sequence.addEdit"
    pub label: &'static str,              // "Add Edit"
    pub menu: &'static [&'static str],    // ["Sequence"]; empty = not in menus
    pub shortcut: Option<&'static str>,   // "Cmd+K" (Cmd = ⌘ on macOS, Ctrl elsewhere)
    pub params: &'static str,             // r#"{"time":ticks?}"#, shown to agents
    pub enabled: fn(&Session) -> Result<(), String>,  // Err carries the reason
    pub run: fn(&mut Session, &Value) -> Result<Value>,
    pub journal: bool,                    // false for read-only queries
}
```

- All commands are in `crates/engine/src/commands.rs` (`cmd!` for actions, `query!` for read-only
  queries such as `project.inspect`, `sequence.inspect`, `effects.list`, `jobs.list`). Ids follow
  the menu structure: `file.*`, `edit.*`, `clip.*`, `sequence.*`, `markers.*`, `timeline.*`,
  `effects.*`…
- `Session::execute(id, params)` finds the spec, checks `enabled`, runs it and appends it to the
  journal.
- **Undo.** Edits go through `Session::edit(label, |project, state| …)` or
  `Session::edit_sequence(label, |seq, ctx, state| …)`. These clone the project, apply the closure,
  and on success push the old `Arc<Project>` with the label onto the undo stack (200 entries).
  On error nothing changes. `edit.undo` and `edit.redo` swap snapshots. Thanks to structural sharing
  a snapshot costs little.
- **Editor state** (`EditorState`: active sequence, playheads, selection, targeting, edit points…)
  is serde, so agents can read it with `state.inspect`.
- **Events** (`ProjectChanged`, `Toast`, `OpenSequence`, `OpenSource`) are drained by frontends
  each frame.
- **Event log** (`Session::log`, `engine::panels`): every failed top-level command (an error; a
  disabled one is a warning), messages and error toasts, auto-save errors, and background jobs
  starting, finishing, failing or being cancelled. Repeats of the newest entry bump its count. The
  Events panel shows it; `events.list {level?, since?}` and `events.clear` reach it headless.
- **UI-only commands** (tools, playback, zoom, panels, workspaces) live in
  `crates/ui-egui/src/menus.rs` (`UI_COMMANDS`). The menu bar is built from the engine registry plus
  this table, and `menus::invoke` is the single entry point for menus, shortcuts and the control
  channel.

## 5. Media, render and playback pipeline

```text
file ──► codecs (MP4/MOV, MKV, audio)        demux + decode, GOP-aware seek
          │   decoder registry: h264, hevc, vp9, av1, prores, dnx, mjpeg (+ any registered first:
          │   platform's VideoToolbox (macOS) or Media Foundation (Windows) H.264 / HEVC, falling back to ours)
          ▼
        media::MediaSource ──► frame cache (byte-budgeted LRU, shared)
          ▼
        render::render_sequence(project, seq, t, scale)        CPU reference
          per track bottom→top: map timeline t → media t (speed), fetch frame,
          standard effects → Motion → Opacity/blend, transitions, composite
          in linear-light premultiplied f32
          │
          └─ render::plan::plan_frame → gpu::GpuCompositor     GPU path
               layers = decoded YUV/RGBA frames + matrix + opacity + blend mode;
               anything the shaders don't cover is pre-rendered on the CPU
          ▼
        ui-egui frames.rs worker pool ──► monitors (program/source), thumbnails, prefetch
```

- **Sources.** `media::MediaSource` yields `video_frame(FrameRequest)` and
  `audio(start, frames, rate)` in media time. Sources are `Send + Sync` and shared by monitors,
  thumbnails, playback and export. The engine's `MediaPool` creates one per project item, lazily,
  through registered openers (`codecs::openers()`: MP4/MOV, MKV/WebM, audio files).
- **Pixel aspect ratio.** Demuxers report it in `VideoStreamInfo::par` (MP4/MOV `pasp`, Matroska
  display size, MPEG sequence header / VUI); Interpret Footage's
  `Interpretation::par` overrides it (`clip.interpretFootage {pixelAspect}`), and
  `SequenceSettings::par` is the sequence's. Both go through `project::checked_par` (a zero term
  or more than 8:1 either way = square). `render::motion_matrix` works in square display units,
  so a 1440 x 1080 clip with 4:3 pixels fills a 1920 x 1080 square-pixel sequence at 100 %
  (Scale to Frame Size, Set to Frame Size and Fit / Fill use `Project::conformed_source_size`);
  graphics and drawings are in sequence pixels. New Sequence From Clip copies the clip's
  ratio, and the Source and Program monitors draw the picture at its display aspect. The
  renderer does not read decoded frames' own `VideoFrame::par`.
- **Offline media.** The pool caches each item's source per reference (path, offline
  flag), so relinking and undo take effect at once. Media that can't be opened renders the offline
  slate (`render::offline`) instead of failing. See [project-files.md](project-files.md#media-offline-and-relinking).
- **Seeking.** `codecs::Mp4Source` seeks to the preceding sync sample and decodes forward, caching
  every frame of the GOP. Sequential playback reuses the decoder. Decoders implement
  `codecs::VideoDecoder`. `register_video_decoder` puts a factory in front of the built-in ones, so a
  hardware decoder can take precedence. The GOP cache never holds its lock while decoding.
- **Hardware decoding.** The apps (desktop, CLI / headless MCP, bench) call
  `waltcut_platform::register()` at startup, which on macOS registers a VideoToolbox factory for
  `avcC` / `hvcC` streams (8 / 10-bit, 4:2:0 and 4:2:2; elsewhere it does nothing), and on Windows a
  Media Foundation one (below). The factory
  declines (our decoder is used) when Settings ▸ Playback ▸ Hardware decoding is Off
  (`codecs::hw::set_hardware_decoding`, applied by the engine whenever preferences change), for
  formats it does not take, and when the OS cannot create a *hardware* session for the stream.
  Its decoder is a `platform::HybridDecoder`: VideoToolbox decodes asynchronously (two access
  units in flight), a reorder buffer of the stream's own depth (`max_num_reorder_frames` /
  `sps_max_num_reorder_pics`, from `codecs::hw::NalStreamInfo`) restores presentation order, and
  the decoded biplanar `CVPixelBuffer` is copied into planar `Yuv8` / `Yuv16`. Colour, pixel
  aspect, random-access and disposable answers come from the same helpers as the software
  decoders (`video::vui_color`, `sar_par`, `h264_disposable`, `hevc_disposable`), so the two are
  interchangeable (bit-exact in the agreement tests). If the hardware fails mid-stream (decode error,
  session lost to a GPU change or sleep, changed in-band parameter sets), the hybrid builds
  `codecs::software_video_decoder` (built-in factories only), replays the samples since the last
  restart point and continues in software for that instance; it is logged and counted. A source
  already open keeps its decoder when the setting changes, until it is reopened. `perf.stats`
  `decode.hardware` reports hardware vs software frames, sessions, declines and fallbacks, and the
  registered `backend`.
  On Windows the decoder (`platform::media_foundation`) is a Direct3D-aware decoder MFT driven at
  the level of single access units (no Source Reader: the demuxer above already delivers the
  samples): the `avcC` / `hvcC` sample becomes Annex B, the MFT is given the process's Direct3D 11
  video device through an `IMFDXGIDeviceManager` so it decodes with DXVA, and its NV12 (8-bit) /
  P010 (10-bit) texture is read back through a staging texture (the one GPU to CPU copy) into
  planar `Yuv8` / `Yuv16`. H.264 Baseline / Main / High, HEVC Main / Main 10, VP9 profiles 0 / 2 and AV1 main
  (8- and 10-bit), 4:2:0, progressive (VP9 / AV1 samples go in as they are, not as Annex B);
  everything else, and any stream the GPU's DXVA decoder does not list, is declined. A decoder that
  would hand back system-memory pictures (Microsoft's decoder MFTs do that when DXVA is not
  available) fails the stream, so the hybrid continues with our decoder and Windows' software
  decoding is never used in its place. Zero-copy (the texture straight to wgpu) is not done yet.
  Decoders run slices on rayon, so an export worker waiting inside a decode can pick up another
  frame of the same source. A request that finds the shared decoder busy decodes with a private
  decoder.
- **Hardware encoding.** On Windows and 64-bit Linux, `platform::nvenc::export::factory` is registered with
  `waltcut_export::register_encoder` (`register()` does this once), in front of the software H.264
  encoder. It takes an export only when Export ▸ Hardware encoding (`ExportSettings.hardwareEncoding`,
  `HardwareEncoding::Auto`) is Auto, which is off by default: hardware streams differ from ours, and
  exports are otherwise byte-identical from run to run. NVENC (`platform::nvenc`, the driver's
  `nvEncodeAPI64.dll` / `libnvidia-encode.so.1` loaded at run time) then takes RGBA frames, converted to 4:2:0 by the software
  encoder's own conversion, and produces the H.264 samples and `avcC` of an MP4 / MOV. It declines
  (the software encoder runs, counted in `export.hardware.declined`) two-pass VBR, HDR, MXF,
  interlaced output, sizes outside NVENC's limits, and systems without an NVIDIA GPU or driver.
  A failure during an export ends it with an error: the software encoder cannot take over a hardware
  stream. The same NVENC session also encodes H.265 (HEVC Main 8-bit 4:2:0 SDR, or Main 10 HDR: PQ / HLG,
  BT.2020, limited range, from the float R'G'B' pictures through `rgbf_to_yuv420_10` into P010 input
  buffers, with the HDR10 static metadata as SEI on every IDR for PQ; the `hvcC` is built from the SPS
  the encoder wrote). An HDR sequence exports as HDR H.265 only where a registered probe says so
  (`waltcut_export::register_hdr_probe`, `hdr_available`; NVENC with 10-bit support), otherwise it is
  tone-mapped to SDR as on macOS: `register()` hands `platform::nvenc::hevc_available` to
  `waltcut_export::register_format_probe`, so the H.265 format exists where an HEVC encoder does,
  and choosing it is the opt-in (the Hardware encoding toggle governs H.264 only). With no software
  H.265 encoder, what NVENC declines is counted and ends the export with an error that says why.
- **Compositor.** `render` is the reference for monitors, thumbnails and export. `render::plan`
  turns a frame into GPU layers, each with its opacity and blend mode. All 27 blend modes run on the
  GPU (`waltcut-gpu`): Normal and Dissolve with fixed-function "over" blending, the others by
  copying the accumulator under the layer into a backdrop texture and compositing in the fragment
  shader with the CPU reference's formulas. Intrinsic Motion, Opacity, blend and colour-management
  transforms remain on the GPU fast path. Graphic/drawing layers, non-plain nested sequences,
  log conversion and frame interpolation can be rasterised on the CPU and handed to the GPU as
  images. Broad video effects, grading and video transitions are deleted in the #24 stacked draft;
  there are no effect compute stages or transition shaders. `FILMCRAFT_CPU_COMPOSITE=1` forces
  the CPU compositor in the desktop app. Native macOS CPU/GPU checks are pending, not claimed here.
- **CPU compositor shortcuts.** The CPU compositor, which is what export runs, works on
  premultiplied linear `f32` images, 33 MB at 1080p, so what it does not allocate, convert or mix
  is time saved. Three shortcuts, each of which gives the general path's bits (compared bit for
  bit, with every blend mode, in `render/src/region_tests.rs`): the canvas stays unallocated
  until a layer needs it, and an opaque bottom layer (an 8-bit or 16-bit Y'CbCr frame without an
  alpha plane, opacity 100 %, Normal) simply becomes the canvas; a plain media frame with an alpha
  plane (a ProRes 4444 banner) is converted and mixed only inside the rectangle where its alpha is
  not zero (`VideoFrame::alpha_region`, `to_linear_f32_region`, `blend::composite_at`), the rest
  being transparent anyway; and working images are recycled through `waltcut_frame::pool`
  (`take_f32_overwritten` / `recycle_f32`: the buffer comes back with its old contents, so there is
  no zero-fill) by the export pipeline once a frame is converted to 8 bits; a standalone export's
  pipeline frees the idle float images (`pool::trim_f32`) when it goes away, so a finished export
  leaves none behind (parts of a batch, such as render-preview segments, keep them for the next part). A clip
  with effects, frame blending, colour management, or a picture that is not placed
  one to one on the output takes the general path.
- **Frame scheduling.** `crates/ui-egui/src/frames.rs` runs a small pool of worker threads with
  prioritised jobs: the frame on screen first, then playback prefetch, then thumbnails. The UI never
  decodes. It shows the exact frame when it is ready and holds the nearest cached frame meanwhile.
  Play waits for the first frames (`PREROLL_FRAMES`, at most 0.5 s) before starting the clock.
  Every refresh, `schedule_playback` asks for the next frames and drops (or cancels, through
  `waltcut_media::cancel`) jobs for frames the playhead has passed; a new on-screen frame
  replaces the one asked for before (scrubbing); Stop cancels the prefetch. When frames cost more
  than the workers can render in real time (CPU effects), it starts only frames that can still be
  on time and spaces them evenly (`playback_plan`). When decoding is what falls behind, each job
  tells its sources which frames are already late (`waltcut_media::cancel::with_catch_up`: the
  playhead's distance, two seconds for a scrubbed-to frame), and the GOP cache skips
  non-reference pictures of late frames on the way (`VideoDecoder::is_disposable`, H.264
  `nal_ref_idc` 0 and HEVC sub-layer non-reference pictures; the wanted frame decodes exactly as
  before). With Settings ▸ Playback ▸ Draft decoding (off by default), frames played at 1/2
  resolution or lower are requested as draft frames (`FrameKey::draft`,
  `waltcut_media::cancel::with_draft`): the H.264 decoder skips deblocking of non-reference
  pictures (`VideoDecoder::set_draft`), the GOP cache serves those draft frames to draft
  requests only (a paused frame, render or export decodes them again exactly), and draft plans
  hand the GPU box-decimated Y'CbCr planes at the drawn size. `perf.stats` reports the counters.
  GPU plans carry their texels already converted for upload (`waltcut_gpu::prepare`), so the UI
  thread only copies them.
- **Audio clock.** The desktop app passes a cpal output (`apps/waltcut/src/audio.rs`) to the UI as
  `AudioOut`. While playing, the samples played by the sound card drive the playhead and video follows.
  Without an audio device, playback falls back to the wall clock. `PlaybackMeter` counts timeline
  frames: shown when the exact picture was on screen while due, dropped otherwise (including frames
  passed over without a refresh, not while the window is hidden). `cargo xtask bench-playback`
  measures the whole path headlessly ([testing.md](testing.md) §5).
  Sequence audio uses the minimal stereo renderer described in §5.1; output-rate resampling and
  scrub/shuttle DSP remain.

### 5.1 Retained soundtrack path (#23 draft)

```text
decoded mono/stereo source pair → source-time mapping → Audio Gain + intrinsic Volume (dB)
    → sum enabled clips on enabled/unmuted tracks → stereo soundtrack
    → stereo soundtrack → device-rate resampling / physical output mapping, or stereo export
```

- **Model** (`project::mixer`). Every audio track, submix and the Mix has a `MixerStrip`. Static
  values stay in `Track::volume_db`, `pan`, `muted` and the send/effect parameters; automation is
  keyframes in sequence ticks: lanes `volume`, `pan`, `mute` (hold), `send.<i>.level` in
  `MixerStrip::lanes`, and insert parameters (`fx.<slot>.<param>`) in the effect's own keyframes.
  Up to 5 inserts (`EffectInstance::post_fader` picks the side) and 5 sends per strip. Submixes feed
  the Mix or a submix after them (no feedback). All fields have serde defaults, so older projects
  load unchanged.
- **Graph** (`render::mixer::mix_graph`). Lanes are evaluated per sample; effect parameters update
  on an absolute 64-sample grid, so the output does not depend on how callers cut the timeline into
  requests (export batches and device callbacks give identical samples). Inserts that report
  latency delay their strip; each route into a bus gets a compensation delay and the graph is read
  ahead by its total latency. Graph state (DSP, delay lines) is cached per structure and continued by
  sequential readers; other requests start fresh with a pre-roll (effect tails, ≤ 3 s). Tracks run
  in parallel (rayon), buses in order. Mono tracks pan with the −3 dB constant-power law; stereo
  tracks and sends use balance. Solo keeps soloed and solo-safe strips plus everything feeding them
  or fed by them. 24 tracks × (EQ + Dynamics + Studio Reverb) + a compressed submix renders at
  ~6× realtime on one core (release).
- **Automation modes** . Off ignores lanes; Read plays them; Latch records from
  the first touch and holds the last value until playback stops; Touch records while held and ramps
  back to the existing automation over the **automatch time** (Preferences ▸ Audio, 1 s); Write
  records every control from playback start (then switches to Touch unless "Switch to Touch after
  Write" is off).
- **Recording** (`engine::mixer`). Playback start runs `mixer.recordStart`, stop runs
  `mixer.recordStop`. Fader and knob drags send `mixer.touch` (value, playhead) while held and
  `mixer.release` when let go. Held values go to `render::mixer::LiveMix`, which the playing mix
  reads, so moves are heard at once. At stop each gesture stream is thinned (linear keyframe
  thinning, optional minimum time interval) and written over its time range as one undo step,
  with boundary keyframes that keep the automation outside the range unchanged.
- **Live state.** `PreviewStore::live` (`LiveMix`) also carries per-strip meter peaks posted by the
  mix (one per channel; Track Mixer and Audio Meters read them) and the newest project snapshot,
  which the audio callback uses, so edits made
  during playback are heard.
- **Audio Clip Mixer automation.** Each audio track has a Clip Mixer mode (`clipMixer.setMode`,
  session state `EditorState::clip_mixer_modes`, Read by default). During a pass the Clip Mixer's
  fader and pan knob are gestures on the track's `clip.volume` / `clip.pan` lanes
  (`clipMixer.touch` / `clipMixer.release`): the clip under the playhead plays the held value at
  once (`render::audio::track_input_live`), and Latch / Touch / Write follow the track rules. At
  `mixer.recordStop` each stream is thinned and written as **clip keyframes** (Volume `level`,
  Panner `balance`, in media time) into every clip it covers, with boundary keyframes that keep the
  clip's values outside the range; one undo step for the whole pass.
- **Track Mixer view.** The panel menu (`mixer.menu`) has Show/Hide Tracks (`UiState::mixer_hidden`).
  Input capture, record arming and recording meters have been removed in the #23 draft train.

`engine::previews` owns the typed live project snapshot consumed by a playing callback. Gain,
Volume/keyframe, source-map and mute edits invalidate audio-preview hashes. Waveforms and
scrub/shuttle retain the same gain semantics. Device-rate resampling and pitch correction are
live DSP dependencies, not audio-effect catalogue entries.

### 5.1.1 Fixed stereo soundtrack

Mono sources (or one selected source channel) are duplicated to left/right; a multichannel source
defaults to its first stereo pair. The Source monitor follows its selected mono/stereo source pair.
There is no multichannel source mixdown preference, shared layout/conversion DSP, export channel
selector, surround track format, or surround output path. Exports write the retained stereo mix
without loudness measurement or true-peak limiting. Removed output-layout and loudness settings
are rejected by export commands rather than ignored.

### 5.2 Audio reduction boundary (#23, draft implementation)

Essential Sound has no panel, UI state, command registry, audio-type model, owned effects or
preferences. Its automatic loudness matching and ducking detector/keyframe generator are deleted,
including the feature-only DSP ducking module and render-time mute branch. Track items and effect
instances reject unknown fields, including removed Essential Sound metadata, rather than silently
rendering a different sound. No legacy-project compatibility path is provided.

The narrower orphan-processing cut removes advanced audio definitions, transition data and
commands, catalogue/preset folders, graphical editors, channel-volume/panner controls, render FX
and crossfade modules, and unreachable DSP processors. Removed IDs and old fields fail explicitly,
including in preset imports; there is no compatibility shell. Only intrinsic Volume remains as
an audio effect, alongside shared Motion/Opacity/keyframe/graphic data.

`crates/ui-egui/tests/audio_timing.rs` compares actual callback samples and decoded float WAV
samples for mono/stereo timing, source In, live gain/mute, additive Volume, bypass/disable,
channel selection and keyframes across irregular blocks. Engine/project/format/UI tests cover
removed commands/IDs/fields and hostile source maps. This is **DRAFT IMPLEMENTATION — native macOS acceptance pending**:
native macOS CI, actual sample-test execution and animation-flow visual acceptance remain
pending. #23 is not complete, accepted or merged.

### 5.3 Clip compositing

Motion, Opacity and blend operate on whole media, drawing and graphic clips. Their ordinary
parameters keep the shared keyframe engine and Effect Controls graphs. Drawing erasers and text
raster coverage are artwork primitives, not effect masks.

The masks/tracking slice of #24 removes mask data, commands, background jobs, UI overlays and CPU/
GPU coverage passes. Effects run directly on the layer; opacity and blending still run after
Motion. Unknown effect-instance fields are rejected to avoid silently rendering old masked data
as a different picture. The ordinary stabilizer and optical-flow effects retain internal image
analysis in `render::motion_analysis`; this is not project tracking state or a command surface.

### 5.4 Colour management

```text
frame (Y'CbCr/RGB + metadata) ─► source colour space: Interpret Footage override, else VUI/colr/MKV Colour
  ─► decode table (sRGB/BT.709, PQ, HLG scene light, camera log) ─► HLG OOTF ─► 3×3 to working gamut
  ─► BT.2390 tone map (HDR/log into an SDR sequence, Auto Tone Map Media) ─► gamut compression
  ─► effects + compositing in working linear (1.0 = reference white = SDR white = 203 cd/m²)
  ─► monitors: working → SDR BT.709 (tone map from HDR, gamut map from BT.2020)
  ─► HDR export: working → PQ/HLG BT.2020 R'G'B' → Y'CbCr (BT.2020 NCL) + VUI/colr/mdcv/clli/SEI
```

- **Model.** `SequenceSettings::color` (`ColorPipeline`: working space Rec. 709 / Rec. 2100 PQ /
  Rec. 2100 HLG, wide gamut, auto tone map) and `Interpretation::color_space` (per media item;
  `None` = from metadata). Commands: `sequence.colorSettings`, `clip.interpretFootage`,
  `color.spaces`, `media.colorInfo`.
- **Maths** in `waltcut-color` (`transform`, `log`, `spaces`; formulas and sources in
  [crates/color/README.md](../crates/color/README.md)); the renderer side is `render::colorman`.
- **Fast path.** A Rec. 709 sequence without wide gamut and media whose metadata says Rec. 709 /
  sRGB decodes exactly as before, and its layers stay on the GPU. Log, HDR or wide-gamut media,
  and every layer of an HDR/wide-gamut sequence, are converted on the CPU (the GPU path draws them
  as pre-rendered images, like layers with effects).
- **Outputs.** `RenderOptions::working_output` returns working-space pixels (HDR exports);
  otherwise the top-level render is converted for an SDR monitor. The other colour
  effects work on display-encoded values clamped to 0..1 (they clip HDR highlights above
  reference white).
- **HDR metadata.** `VideoStreamInfo::hdr` (`HdrMetadata`: ST 2086 mastering luminance, MaxCLL,
  MaxFALL) is read from `mdcv` / `clli` (MP4 / MOV) and Matroska `MasteringMetadata` / `MaxCLL` /
  `MaxFALL`. PQ media tone mapped into an SDR working space uses it as the BT.2390 source peak
  (MaxCLL, else the mastering peak; 1000 cd/m² without metadata), so a 4000-nit master keeps its
  highlight detail instead of clipping at 1000. `media.colorInfo` reports it (`hdrMetadata`,
  `toneMapPeakNits`). The field is optional and serde-defaulted (no schema change).
- **HDR export.** H.264 and ProRes exports of a PQ/HLG sequence encode BT.2020 PQ/HLG (ProRes
  10-bit; our H.264 encoder is 8-bit, so H.264 HDR is 8-bit) and signal it in the VUI / ProRes
  frame header, `colr`, and for PQ `mdcv` (BT.2020 / D65, 1000 / 0.0001 cd/m²) and `clli`
  (MaxCLL/MaxFALL 0 = unknown, they are not measured) plus the matching H.264 SEI. `sdr: true`
  exports the tone-mapped SDR picture instead; render previews always do.
- **Monitors / display colour management.** The monitors are SDR (sRGB-encoded RGBA8 textures):
  HDR sequences are shown tone mapped. macOS EDR (extended-range `CAMetalLayer` output) is not
  wired up: there is no `platform` crate yet and eframe/wgpu do not expose an EDR surface, so HDR
  values above SDR white are never sent to the display.

## 6. Export jobs (`waltcut-export`)

```text
file.exportMedia {path, preset?, settings?, format?, range?, …}     export.quick {preset?, path?}
  → engine::export_tools: preset ⊕ settings JSON ⊕ flat overrides → ExportSettings; range resolved
  → a Job {id, label, progress, result} on a background thread (export.queue.* runs them in order)
  → export: per output frame render (output rate) → fit into the output frame → overlays → limiter
            → encode in order → mux; audio mixed and written as stereo, with no loudness pass
  → jobs.list shows progress; jobs.cancel sets the shared cancel flag
```

| Format | Encoder | Container |
|---|---|---|
| `h264` | `waltcut-h264enc` (profile, level, CBR / VBR 1-pass / VBR 2-pass, keyframe distance) + `waltcut-aac` | MP4 or QuickTime (`isobmff`, the Multiplexer setting) |
| `prores` | `waltcut-prores` (Proxy / LT / 422 / HQ / 4444 with alpha) | MOV |
| `mjpeg` | built in | MOV |
| `aac` | `waltcut-aac` (AAC-LC, the audio settings' bitrate) | audio only: MPEG-4 audio `.m4a` (`isobmff`, priming in an edit list), or raw ADTS when the path ends in `.aac` |
| `png`, `tiff`, `bmp`, `jpg` | `image` | numbered stills `<name>000.<ext>`, `<name>001.<ext>` … |
| `tga`, `dpx`, `exr` | built in (`raster.rs`: uncompressed Targa 24/32-bit; DPX 10-bit RGB, SMPTE ST 268; OpenEXR half-float linear RGB(A), uncompressed scan lines) | numbered stills, as above |
| `gif`, `wav`, `aiff` | built in / `image` | GIF / stereo RIFF WAVE (`WAVE_FORMAT_IEEE_FLOAT` + `fact` for float) / stereo AIFF (16-, 24- or 32-bit PCM; 32-bit float as AIFF-C `fl32`) |

- **Reproducible.** The same project and settings give the same file on every machine. Frames render
  in batches of one per core, but the muxer gets them in fixed groups of 16 output frames
  (`INTERLEAVE` in `crates/export/src/job.rs`), each followed by its audio, and H.264 uses one slice
  per four macroblock rows instead of one per core.
- **Settings.** `ExportSettings` (serde, camelCase, every field optional) holds Video (frame size or
  Match Source, frame rate, Scale to Fit / Fill / Stretch, pixel aspect, field order — progressive
  only, profile / level, bitrate encoding, target / maximum / adaptive bitrate, keyframe distance,
  maximum render quality), Audio (codec, sample rate, AAC bitrate, PCM sample size: `bits` 16, 24 or 32
  and `float` for 32-bit float PCM in WAV, AIFF and QuickTime — `supports_sample_format` says
  which a codec and container store, and Export mode greys out the rest),
  Multiplexer (container choice), Effects (image / name / timecode overlays, video limiter),
  Metadata (`udta` `©nam`, `©ART`, `©cpy`, `©des`,
  `©cmt`). `settings.summary()` / `estimate_bytes()` feed Export mode's Summary.
- **Presets.** `export::presets::builtin_presets()` are our own definitions (Match Source adaptive
  H.264 at 0.2 / 0.1 / 0.05 bits per pixel, 1080p / 2160p delivery, vertical 1080×1920, ProRes,
  image sequences, GIF, WAV / AIFF). User presets and favourites persist in
  `<data dir>/export-presets.json` (`export.presets.*`).
- **Queue.** `export.queue.add` snapshots the project and the resolved settings (several sequences
  or ranges add several items); `export.queue.start` encodes ready items one after another as
  ordinary jobs, advanced by `Session::poll_persistence` / `pump_jobs` (or synchronously with
  `wait`). Cancel, retry, reorder, remove and clear work per item. The queue is session state, not
  part of the project file.

Video encoders implement `export::VideoEncoder`. Codec crates plug in with `register_encoder` and
`register_audio_encoder`.

## 7. Automation surfaces

All of these dispatch the same command ids.

| Surface | Where | Scope |
|---|---|---|
| UI | `ui-egui` menus, shortcuts, panels | `menus::invoke` → engine or UI command |
| CLI | `filmcraft-cli` | `exec <id> key=value…`, `run script.jsonl` (one `{"id","params"}` per line), `inspect`, `import`, `export`, `render`, `probe`; `--save`, `--bridge` |
| Control channel | `filmcraft --control <port>` | JSON lines on loopback TCP: engine commands plus synthetic input, inspection and screenshots of the live UI |
| MCP | `filmcraft-cli mcp` | stdio MCP server: headless in-process session, or `--bridge` to the control channel |

- **Automation ids.** Every interactive widget calls `app.auto.add(id, rect, label)` each frame
  (`crates/ui-egui/src/automation.rs`). Agents click by id, e.g. `tools.Razor`,
  `timeline.clip.<id>`, `effects.item.gaussian_blur`, `panel.Timeline`.
- **UI state** that is not project data (tool, workspace, dock layout, zoom, scroll, monitor
  settings) is in `crates/ui-egui/src/state.rs` as serde structs, so `ui.inspect` and `ui.set` can
  read and write it.

- **Panels with engine data** (M8.9 / M12.6): Metadata (`metadata.get` / `metadata.set`, one undo step per edit, stored in
  `ProjectItem::metadata`), Events (`events.list` / `events.clear`), Progress (`jobs.list` /
  `jobs.cancel`), Project Notes (`project.notes` / `project.setNotes`, plain text in
  `Project::notes`, schema v15; one typing session is one undo step). The Timecode panel is a frontend-only view. Their settings are
  `UiState::panels` (`ui.set {"panels": {...}}`).
- **Project panel** (M12.7). View settings (List / Icon / Freeform, thumbnail
  and font size, Preview Area, Hover Scrub), the List view's columns, widths and sort, and ten view
  presets are preferences (`Preferences::project_panel`; `project.view.*`, `project.columns.*`,
  `project.sort`, `project.viewPreset.*`). Freeform positions, Clip Size, stacks and saved
  arrangements are item metadata (`Freeform Position`, `Freeform Stack`,
  `Freeform Arrangement: <name>`; `project.freeform.*`), so they are undoable and saved with the
  project without a schema change. Bins opened in a tab or a window, the inline
  rename, dialogs and the hover-scrubbed card are `UiState::project_panel`
  (`ui.set {"projectPanel": …}`).

Protocol reference: [control-protocol.md](control-protocol.md). Agent guide: [agents.md](agents.md).

## 8. Not built yet

The layer table reserves names for crates that don't exist yet: `riff`, `mjpeg`,
`keyframe`, `effects`, `audio` and `playback`. (`platform` exists but holds only OS media FFI;
other OS integration stays in `apps/waltcut`.)
Until they exist, that work lives elsewhere: keyframes and effect definitions in `project`, effects
and the audio mix in `render`, playback in `ui-egui`, and OS integration (cpal, rfd,
native menus) in `apps/waltcut`. [roadmap.md](roadmap.md) has the milestone status and [gaps.md](gaps.md) what is missing.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | trivial | Status line and revision history added (craftrules progress-docs standard); milestone link points at docs/roadmap.md |
