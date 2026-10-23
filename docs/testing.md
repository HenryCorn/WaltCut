# Testing

```sh
cargo test --workspace                          # everything; oracle tests skip without ffmpeg
cargo test -p waltcut-h264                    # one crate
cargo test --release -p waltcut-hevc          # codec tests are much faster in release
cargo xtask ci                                  # all gates (fmt, clippy, tests, layers, assets)
cargo xtask fixtures                            # pre-generate the ffmpeg fixture matrix
WALTCUT_REQUIRE_ORACLES=1 cargo test --workspace   # CI: missing ffmpeg fails instead of skipping
```

Negative command-registry and removed-data rejection tests cover the deleted features; Motion,
Opacity and blend editing is tested on media, drawing, text and shape clips, with whole-layer
animated-opacity compositing coverage. Effect Controls retains ordinary parameter, graph and
opacity-group keyframe tests. `cargo xtask check` is the quick local gate (fmt, an all-targets
check, layers, assets, and tests for the changed crates and their dependents); `cargo xtask ci`
is the full one. The removed-capability deny-list runs inside `cargo xtask layers`.

## 1. Kinds of tests

| Kind | Where | Examples |
|---|---|---|
| Unit | `src/` `#[cfg(test)]` modules, `src/tests.rs` | CABAC engines, VLC tables (prefix-freeness, Kraft sums), colour matrices, frame cache |
| Property (`proptest`) | `time`, `bitstream`, `edit`, `isobmff/tests/roundtrip.rs` | tick↔frame round trips for every rate, DF timecode, edit invariants (no overlaps, durations conserved), mux→demux round trips, interchange export→import |
| Synthetic streams | `h264/src/synth_tests.rs`, `hevc/src/synth_tests.rs` | hand-built bitstreams for features the reference encoders don't emit; expected output known exactly |
| Scripted UI | `crates/ui-egui/tests/scripted.rs` | headless app (egui_kittest) driven over the control channel: razor/undo, insert, apply effect, playback, click by automation id (§4) |
| Monitor view / Graphics menu | `crates/ui-egui/tests/view_ui.rs`, `crates/engine/src/graphics_tests.rs` | View menu items and checkmarks, playback/paused resolution, channel / waveform display modes, magnification and Hand-tool panning, guides dragged from the rulers (move, lock, remove, Add Guide…, templates in the preferences), snapping a graphic to the frame centre and a guide; align to frame / as group / to selection, distribute (centres and gaps), arrange, select next/previous graphic and layer, reset parameters / duration, vertical text, New Layer from file (`WALTCUT_UI_SHOTS=<dir>` writes `view-*.png`) |
| Settings | `crates/engine/src/settings_tests.rs`, `crates/ui-egui/tests/settings_ui.rs` | schema keys and defaults, persistence in the data directory, v1 → v2 migration and repair of bad values, validation, per-category reset; each wired setting (still / transition durations, step many, label defaults and colours, media scaling, timebase, recent projects, media cache policy, output mapping, smart quotes, auto-transcribe); the dialog: every category from its command and Cmd+,, edits by automation id, OK / Cancel / Escape / Reset…, theme, label names in Edit ▸ Label, tooltips, frame cache (`WALTCUT_UI_SHOTS=<dir>` writes `settings-*.png`) |
| Keyframe navigator and Effect Controls ruler | `crates/engine/src/tests.rs` (`add_keyframe_toggles_the_keyframe_at_the_playhead`), `crates/ui-egui/tests/keyframes_ui.rs`, `lane_tests` in `crates/ui-egui/src/panels/effect_controls.rs` | `effects.addKeyframe` adds a keyframe at the playhead or removes the one there (the value falls back to the remaining keyframes; undo, redo, parameters that name nothing); headless: the Properties diamond turns animation on, adds and removes the keyframe at the playhead (Scale 50 % → 100 % again), its ◀ ▶ arrows and Effect Controls' step between keyframes, Crop is applied before its first keyframe; a click on the Effect Controls time ruler and a drag of the playhead's handle move the playhead, and no playhead is drawn off the clip; ruler labels are sequence timecode, and clip times, frame rates and panel widths at their limits don't panic (`WALTCUT_UI_SHOTS=<dir>` writes `keyframes-*.png`) |
| Golden images | `crates/golden/tests/golden.rs` | CPU renders vs committed PNGs; GPU vs CPU on the same scenes (§3) |
| Engine / command | `crates/engine/src/tests.rs` | run commands on the demo project, assert the sequence, undo/redo, disabled cases |
| Render | `crates/render/src/tests.rs` | compositing, opacity, Motion, ½-res vs full, GPU plan vs reference, retained mono/stereo audio mix |
| Project panel (M12.7) | `crates/engine/src/project_panel_tests.rs`, `crates/ui-egui/tests/project_panel_ui.rs` | view settings and their preference round trip; Metadata Display columns (custom fields, Freeform keys hidden), numeric vs text sorting, view presets (save / save as / restore / rename / delete, ten slots, persisted in `preferences.json`); Freeform moves keeping offsets, snapping, Align to Grid, Clip Size, stacks, arrangements, Reset (each one undo step, saved in the project); bin rename / nesting; the Media Browser against a fake filesystem (folders first, hidden and unsupported files left out, file-type filter, back / forward / up, recent directories, Favorites, import of a selection or a folder, Open in Source Monitor, ingest, probe columns). Headless: List view (sort by header, drag a column wider, inline rename), Icon view (hover scrub + I / O set the clip's In / Out, Shift+] / [), Freeform (drag a card, stack from the card menu, options dialog), panel menu, Metadata Display dialog, view presets, bins in place / tab / window, footer buttons, Media Browser navigation, Favorites, import, Shift+O, Edit Columns…, thumbnails, drag to the Timeline (`WALTCUT_UI_SNAPSHOT_DIR=<dir>` writes `project-*.png`, `media-browser-*.png`) |
| Menu long tail (M3.11) | `crates/engine/src/{sequence_extras,project_tools}_tests.rs`, `crates/project/src/find.rs`, `crates/ui-egui/tests/menus_ui.rs` | Normalize Mix Track to a target peak; Simplify Sequence; Find / Find Next and search bins; Automate to Sequence (overlap, transitions, insert, unnumbered markers); Project Settings and scratch disks; menu order and shortcuts against the documented order; the dialogs by automation id |
| Removed capture  | `crates/engine/src/audio_reduction_tests.rs`, existing `paste_media_tests.rs` and scratch-folder tests | recording commands reject hostile parameters without project/preference mutation; pasted images retain trusted scratch-folder selection. Cross-typechecked; execution pending macOS. |
| Removed audio processing  | `engine/src/audio_reduction_tests.rs`, `project/src/effect.rs`, `format/tests/fixtures.rs` (under `crates/`) | Removed effect/transition commands and IDs, insert flags, project transition data, unused sequence Mix parameters and multichannel source maps reject malformed/hostile requests without project/history/state mutation. Current golden project contains only retained fields; pre-reduction fixtures still prove explicit refusal. Native execution pending. |
| Retained timing audio  | `crates/ui-egui/tests/audio_timing.rs` | generated mono/stereo WAV import, source In and timeline offset; actual live callback plus actual decoded written ProRes MOV/float-PCM samples checked for Audio Gain, mute/unmute, additive intrinsic Volume, bypass/disable, selected mono channel, silence floor and sample-accurate keyframes across irregular blocks. Each of 24 mono/stereo × state cases checks every stereo sample, exactly 12,000 audio frames at 48 kHz, six video frames and `ap4h`; no WAV delivery API or model-only replacement. Cross-target typechecked; **native macOS execution pending**, not claimed passing. |
| Oracle | `crates/*/tests/*oracle*.rs`, `conformance.rs` | compare with ffmpeg/ffprobe (§2) |
| Robustness / fuzz | `*/tests/robustness.rs`, `*/tests/fuzz.rs` | seeded mutation and truncation of real and synthetic files; nothing may panic |
| Program monitor while dragging | `crates/ui-egui/tests/perf_ui.rs` | headless: a parameter changed on every UI pass (a value dragged in Effect Controls or Properties) and a backward scrub leave the Program monitor's picture at most one step behind (it shows the frame asked for the pass before while the frame due renders; it used to freeze until the mouse rested); a frame still rendering is kept when the same frame is asked for at a newer revision, so frames slower than a refresh (CPU effects) are shown too; `perf.stats` reports the picture's frame and revision against the playhead and the project |
| Performance | `*/tests/perf.rs` (`#[ignore]`) | §5 |

Commit `*.proptest-regressions` files so failing cases are re-run.

## 2. ffmpeg oracle tests

ffmpeg and ffprobe are **external processes** used to generate fixtures and to check results. They
are never linked or shipped ([AGENTS.md](../AGENTS.md) §2).

- The tools are found by `waltcut-testkit` (a dev-dependency-only crate, `crates/testkit`), in
  this order: `WALTCUT_FFMPEG` / `WALTCUT_FFPROBE` (explicit paths; a wrong path is an error),
  then every directory on `PATH` (`ffmpeg` and `ffmpeg.exe`), then well-known install directories
  (`/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin`, `C:\ffmpeg\bin`, …).
- If a tool is missing the test prints `SKIPPED (<test>): ffmpeg not found …` and passes. Set
  **`WALTCUT_REQUIRE_ORACLES=1`** (CI) to make every such skip a failure. In new tests use
  `let ff = waltcut_testkit::require_ffmpeg!();` (also `require_ffprobe!`, `require_oracles!`).
- Fixtures are generated on first use into `<workspace>/target/fixtures/<crate>/`
  (`waltcut_testkit::fixtures_dir`; independent of `CARGO_TARGET_DIR`, so agents with private
  target dirs share them; `WALTCUT_FIXTURES_DIR` overrides the root) and reused afterwards.
  Generators write to a per-process/thread temporary name (`testkit::temp_path`) and rename it into
  place, so concurrent tests never see half-written files. Delete the directory to regenerate.
  Never commit media.
- Every crate's `tests/common/mod.rs` delegates discovery and fixture paths to testkit, except
  `crates/codecs/src/tests.rs`, which still has its own lookup (the crate was being edited
  concurrently; migrate it when convenient).
- `cargo xtask fixtures [crate…]` pre-generates the whole fixture matrix up front (useful before a
  parallel test run or on a fresh machine). It runs each crate's ignored `generate_fixtures` test —
  the same generators the oracle tests call — for `h264`, `hevc`, `isobmff`, `matroska`,
  `prores`, `mpeg2v` and `codecs` (MPEG TS / PS), and prints one `made` / `cached` / `skipped` line per fixture plus a summary. The
  `aac`, `opus` and `h264enc` oracles generate small per-test signals on demand and are not part of
  the matrix.
- Fixture sources are synthetic: `testsrc2`, `mandelbrot`, SMPTE bars, noise and fades, sine tones.
  H.264 and HEVC fixtures need ffmpeg built with libx264 and libx265. VideoToolbox fixtures are
  generated only on macOS. The hardware decoder agreement tests (`crates/platform/tests/media_foundation.rs`,
  Windows only) make their 640x360, 1080p and 2160p H.264 / HEVC / Main 10 fixtures with ffmpeg and skip
  without a Direct3D 11 video device or the HEVC / VP9 / AV1 codec extensions of the Microsoft Store. The VP9 and
  AV1 fixtures (libvpx-vp9, libaom-av1; 360p, 1080p, 2160p, hidden alt-ref frames, two GOPs) need an ffmpeg with those encoders.

The NVENC tests (`crates/platform/tests/nvenc.rs` and `nvenc_export.rs` for H.264, `nvenc_rgba_input.rs` for
the GPU's RGB → 4:2:0 conversion, `nvenc_hevc.rs`,
`nvenc_hevc_export.rs`, `nvenc_hevc_probe.rs` and `nvenc_hevc_warm.rs` for H.265, `nvenc_hevc_main10.rs` and
`nvenc_hevc_hdr_export.rs` for Main 10 HDR; Windows only) skip, printing `SKIPPED`, without an NVIDIA GPU
with NVENC (H.265: when the HEVC probe says there is no HEVC encoder; Main 10: when the Main 10 probe says
there is no 10-bit encoder; on a machine that has one, a configuration NVENC refuses fails the test). The FFI layout tests in
`crates/platform/src/nvenc/abi_tests.rs` were generated from a C program built with MSVC
(`cl` after `vcvars64.bat`) against NVIDIA's MIT-licensed `nvEncodeAPI.h` (12.1); the header and the
program are not in the repository. To regenerate them, put the header in a scratch directory and
write a program that prints, for every type, field, constant and GUID of `src/nvenc/ffi.rs` that the
asserts name (H.264, HEVC and the HEVC picture parameters with their SEI payload array), `sizeof` / `alignof` / `offsetof`, the enum and macro values cast to
64-bit integers and the GUIDs as 128-bit hex; for the bit-fields of the `flags` words, which
`offsetof` cannot take, zero a structure, set that one field to its maximum and print the 32-bit word.
Compare the numbers with the asserts (a difference is drift, which must be understood before
anything is "fixed" to match) and add asserts for anything new.

### Pass criteria per codec

| Crate | Oracle check | Criterion |
|---|---|---|
| `h264` | decode every fixture single-threaded and frame-threaded; compare with `ffmpeg -f rawvideo -pix_fmt yuv420p` | **bit-exact**, every frame |
| `hevc` | same, `yuv420p` / `yuv420p10le` | **bit-exact** (8- and 10-bit) |
| `h264enc` | `ffmpeg -v warning -err_detect +crccheck+bitstream+buffer+explode` decodes our stream | no output at all (a `corrupt decoded frame` warning fails; a negative control checks dropped and truncated slices are reported), exact frame count, **bit-identical to the encoder's reconstruction**; B-frame order checked with ffprobe; rate-control targets |
| `prores` decode | ffmpeg `prores_ks` / `prores_aw` fixtures, compared at native depth | within **±1 LSB** (different integer IDCT); alpha bit-exact |
| `prores` encode | ffmpeg decodes with `-xerror` | no errors; agrees with our decoder within ±1 |
| `aac` decode | ffmpeg's decode of the same stream | max abs error ~1e-7 (float) |
| `aac` encode | ffmpeg decodes our stream | no errors, no clipping, CBR within ±5% of target; SNR reported |
| `isobmff` | `ffprobe -show_packets` on ffmpeg-made MP4/MOV | packet offsets, sizes, pts/dts, durations, key flags and stream parameters equal; remuxed files decode with `ffmpeg -v error` silent |
| `matroska` | `ffprobe -show_packets` | every packet (stream, size, key flag, pts, duration) equal; seeks land on the latest keyframe ≤ target |
| `audio-dsp` loudness | `tests/loudness_oracle.rs`: signals generated in Rust (997 Hz sine, pink noise at 48/96 kHz, speech-like bursts at 48/44.1 kHz, stereo with silence and sub-gate passages, an fs/4 inter-sample-peak tone) written as float WAV and measured with `ffmpeg -af ebur128=peak=true:metadata=1` | momentary and short-term every 100 ms **±0.1 LU**, integrated **±0.1 LU**, LRA **±0.5 LU**, true peak **±0.2 dB** (against the analytic value when one exists). Measured: ΔM/ΔS ≤ 0.0005 LU, ΔI ≤ 0.007 LU, ΔLRA ≤ 0.04 LU, ΔTP ≤ 0.045 dB against ffmpeg. On the fs/4 tone ffmpeg's own true peak is +0.6 dB high (−0.32 vs the analytic −0.92 dBTP); ours is −0.05 dB |
| `audio-dsp` effects | `src/effects/effect_tests.rs`, `src/design.rs`, registry tests in `src/effects/mod.rs`: generated tones, noise and impulses | every effect: neutral settings = delayed identity (≤ 2·10⁻⁴), bit-exact determinism, latency constant and equal to where an impulse lands, block-size independence, finite / denormal-free tails at extreme settings, > 1× realtime; per effect a level or frequency-response check (measured tone vs analytic `response_db` within 0.1–0.2 dB, Butterworth vs closed form 10⁻⁶ dB, Chebyshev/elliptic ripple and stop-band bounds, LR4 crossover sum flat within 0.02 dB, comb-notch depth, echo positions to the sample, Schroeder RT60 of generated impulses within 25 %, click repair −20 dB) |
| `mxf` (via `codecs/tests/mxf_oracle.rs`) | ffmpeg-written OP1a / OP-Atom / D-10 files decoded by ffmpeg | H.264 (long GOP, B pictures without temporal offsets) **bit-exact** every frame and at 25 random seeks; DNxHR ±2, ProRes ±1; PCM / AES3 **sample-exact**; frame count = ffprobe packets; timecode (25 fps, 29.97 DF); MPEG-2 long GOP, XDCAM HD422 (1080i 4:2:2) and D-10 / IMX (720×608 4:2:2) within ±4 (`mpeg2v` criterion); truncation/mutation never panics |
| `mpeg2v` | ffmpeg-encoded MPEG-1/2 elementary streams (progressive, TFF/BFF interlaced, 4:2:2 intra and long GOP, 1080i, escapes + custom matrices, MPEG-1) and our synthetic streams with field pictures / 16x8 / dual prime / concealment vectors, decoded by ffmpeg (`-xerror`) | per frame max ±4 and PSNR ≥ 58 dB, ≤ 3 % of samples differing (ffmpeg's integer IDCT vs our IEEE 1180-accurate one; measured max ±1-3, 65-75 dB); frame count, display order, picture types, interlacing and field order **exact** (ffprobe); IEEE 1180 procedure passes |
| MPEG TS / PS (via `codecs/tests/mpeg_oracle.rs`) | synthetic ffmpeg TS / BDAV / VOB / MPEG-1 program streams with MPEG-2 + MP2, H.264 + AAC, HEVC + AAC/LATM, LPCM, and AC-3; MPEG-2 in MOV/MP4/MKV; `.m2v` | frame count and PTS exact; retained video decodes and seeks match the oracle; MP2/AAC/LPCM audio checks remain; AC-3 is omitted from otherwise-retained video with an actionable user-visible notice (not decoded) |
| Removed-format imports | empty and truncated `.mxf`, `.ogg`/`.oga`, `.dnx`, `.apv`, `.vp8`, `.dng`, `.ac3`/`.eac3`, and `.flac`; unsupported DNx/APV/VP8/FLAC tracks in retained containers | no panic; actionable format-not-supported error, except AC-3/E-AC-3 alongside retained picture, which imports picture-only and surfaces a conversion notice |
| MP4 audio at another rate (`codecs/tests/mp4_oracle.rs`) | ffmpeg-written AAC in MP4 (48 kHz, edit list skipping the priming) | reads resampled to 44.1 kHz in frame-sized requests equal the native decode interpolated at the same times (difference < 10⁻⁴, peak ≤ the source level) |
| image sequences / BWF (`codecs/tests/media_oracle.rs`) | ffmpeg-written PNG/TIFF/BMP/JPEG stills read by ffmpeg's image2 demuxer; ffmpeg BWF (`-write_bext`) | PNG/TIFF/BMP **exact**, JPEG ±12 (measured 3); missing frames hold the previous one; BWF TimeReference = ffprobe's, start timecode, samples exact |
| `export` | our own demuxer/decoder reads the file back; ffprobe counts frames when present | expected size, duration, colour, audio level; exact frame count |
| Retained delivery  | `export/src/{tests,settings_tests,hdr_tests,determinism_tests}.rs`, engine `export_tests.rs`, CLI integration tests | actual H.264 + AAC MP4, ProRes 4444 MOV and RGBA PNG sequence read back with retained decoders; frame counts exact, alpha checked on every ProRes/PNG frame, range/selected-sequence audio timing preserved; PNG/JPEG stills, HDR colour signalling, bounded memory, deterministic CPU batches/overlap and panic/cancel/error paths. Unknown/removed formats/settings/commands and hostile dimensions/rates/ranges/buffers rejected. Native macOS execution of every retained output round trip remains pending. |
| Transactional output  | `engine/src/export_output.rs` standalone-compatible tests, engine/UI/CLI export tests | no-clobber install, explicit movie/still replacement, unused PNG bases, uppercase PNG names, cancellable preflight, owned rollback, another writer's replacement preserved, cleanup errors surfaced, panic/shutdown/cancel cleanup and no orphan worker. The Unix atomic-replacement test requires macOS. |

Each codec README has the full fixture matrix and the measured results.

## 3. Rendering: CPU reference and GPU agreement

- The CPU compositor in `waltcut-render` is the reference. Its tests check computed values
  (opacity blends, Motion placement, ½-resolution against downsampled full
  resolution) and that `render::plan` matches `render_sequence`.
- `crates/gpu/src/tests.rs` composites the same frame plan (a YUV layer plus a transformed,
  semi-transparent RGBA layer) on the GPU and on the CPU. It requires a 99th-percentile channel
  difference of ≤ 6 and a mean of < 1.5 (8-bit levels). It skips when no GPU adapter is available.
- **Golden images** (`crates/golden`, a test-only crate): `tests/golden.rs` builds small procedural
  projects (demo-generator footage, bars, no media files) at 320×180 and renders one frame of each
  through the CPU compositor: Motion transform + opacity, four blend modes (Multiply, Screen,
  Overlay, Difference), graphic clips (text/shape layers and per-character styles with a pinned
  box), and colour
  management: S-Log3/S-Gamut3.Cine footage interpreted into Rec. 709 (`log_to_rec709`) and
  Rec. 2100 PQ footage tone mapped into Rec. 709 (`hdr_tone_map`; the footage is demo frames
  re-encoded by the test's `Encoded` source). Each frame is compared
  with `crates/golden/goldens/<scene>.png` by `waltcut_testkit::golden`:
  **PSNR ≥ 45 dB, max abs ≤ 12, 99th-percentile per-pixel max channel difference ≤ 2** (8-bit
  sRGB levels; `Tolerance::RENDER`). On failure the actual frame and a ×8 difference image go to
  `<workspace>/target/golden-failures/`. `scenes_are_distinct` guards against blank or duplicate
  scenes. Removed video effects/transitions have no retained goldens, contact sheets or fixtures.
  Negative registry/command/persistence tests reject their IDs. Audio crossfades and their tests are
  removed too; negative audio registry/command/persistence tests remain.
- **Blessing:** `WALTCUT_BLESS=1 cargo test -p waltcut-golden` rewrites the references (and
  writes a `.attribution` sidecar for any new one; it prints the `ATTRIBUTION.md` row to add).
  References are original work rendered by WaltCut, must stay under 50 KB (enforced on bless),
  and every one needs its sidecar and index row (`cargo xtask assets`). Look at a re-blessed PNG
  before committing it.
- The same scenes run through `plan_frame` + `GpuCompositor` when a GPU adapter exists
  (`gpu_matches_cpu_on_golden_scenes`, same p99 ≤ 6 / mean < 1.5 criterion; skipped otherwise).
  This found a real bug: the GPU upload cache was keyed by pixel-buffer address without keeping
  the buffer alive, so a new frame allocated at a freed frame's address was drawn with the stale
  texture (fixed; regression test `upload_cache_keeps_buffers_alive`).

## 4. UI: control channel and screenshots

### Scripted UI tests (headless)

`crates/ui-egui/tests/scripted.rs` runs the real `WaltCutApp` under
[`egui_kittest`](https://docs.rs/egui_kittest) (`build_eframe`): no window, no GPU, no OS event loop.
A small `Driver` opens the demo project, sends requests through the same control channel agents use
(`ControlRequest` → `control::handle`, exactly as the TCP server does) and steps egui frames until
each reply arrives. Synthetic input queued by `ui.click` / `ui.drag` / `ui.key` is moved into the
next frame by the app's own `raw_input_hook`, so clicks by automation id work headless. Covered:

| Test | Asserts |
|---|---|
| `demo_project_opens_headless_and_registers_widgets` | active sequence, > 50 registered widgets, panel and tool ids, 6 clips on V1 (`sequence.inspect`) |
| `razor_then_undo_through_the_control_channel` | `timeline.razor` adds a clip, the timeline can locate it on screen, `edit.undo` via `ui.menu.invoke`, redo |
| `insert_from_source_ripples_the_sequence` | `source.open` + marks + `source.insert` lengthens the sequence |
| `effects_panel_omits_removed_video_catalogue` | Video and audio catalogue/transition folders and entries are absent; only retained presets remain |
| `playback_toggle_and_stop` | `playback.toggle` / `ui.playback stop` and `ui.inspect` playback state |
| `clicking_a_tool_button_by_automation_id` | `ui.click {id: "tools.Razor"}` changes the tool (real egui input path) |
| `unknown_methods_and_commands_fail_cleanly` | errors come back as `{"ok": false}` |

The mixer UI tests were deleted with the mixer/automation slice; they no longer describe shipped
panels or current coverage. The Audio Gain dialog remains, while its normalization options were
removed. No mixer visual acceptance is claimed.
`crates/ui-egui/tests/clip_audio_ui.rs` covers the audio of video clips (#223): the linked audio's
Volume in Effect Controls/Properties and absent channel-volume/panner rows. The timeline line
edits independent static Audio Gain without changing Volume keyframes; drag/undo, click and
right-click remain covered (`WALTCUT_UI_SHOTS=<dir>` writes `volume-*.png`).

The Essential Sound slice of #23 deletes its feature-only engine/UI/ducking tests along with the
implementation. `engine::audio_reduction_tests` now checks that every removed command rejects
empty, malformed and hostile parameters without changing project, preferences, editor state,
undo/redo or jobs; removed preferences reject writes. Project tests check absent metadata and
rejection of removed clip/effect fields. The scripted UI test checks the absent panel, commands
and automation ids without project or layout edits.

The narrower orphan-processing slice removes audio-effect/transition tests and fixture generation
with their implementations, not the shared Motion/Opacity/graphic keyframe tests.
`crates/ui-egui/tests/audio_timing.rs` compares production callback samples and actual written
ProRes MOV stereo float-PCM samples decoded through the retained media stack
with generated mono/stereo input, including timing, source In, gain, live mute/unmute,
additive keyframed Volume, bypass/disable and channel selection. They run natively on macOS in CI.

`crates/ui-egui/tests/export_ui.rs` drives the simple Export and shared Quick Export controls:
destination, three retained media formats, exact ranges, Start, live progress/status and the actual
Cancel widget. It checks absent queue/advanced UI, strict draft refusal without mutation, actual
written frame counts, Replace/Keep behavior and PNG conflicts. `export_menu_ui`, `frame_export_ui`
and `native_dialogs_ui` retain menu routing, PNG/JPEG stills, full-resolution/captured-monitor frames,
alpha/depth, explicit replacement and asynchronous destination picking/late-answer refusal.
Optional snapshots use `WALTCUT_UI_SNAPSHOT_DIR`.

### Export acceptance on macOS

With the external ffmpeg/ffprobe fixture and oracle setup, these cover every retained output:

```sh
cargo test -p waltcut-export
cargo test -p waltcut-engine export
cargo test -p waltcut-ui-egui --test audio_timing --test export_ui --test export_menu_ui --test frame_export_ui --test native_dialogs_ui
cargo test -p waltcut-cli
cargo test -p waltcut-automation export_progress_and_cancel
cargo test -p waltcut-codecs -p waltcut-platform
cargo xtask ci
```

They check retained-import fixture decoding, a round trip of each output (H.264/AAC MP4, alpha
ProRes 4444 MOV, alpha PNG sequence, PNG/JPEG stills), the PQ/HLG straight-colour and partial/zero
alpha ProRes cases, and callback/MOV sample comparisons while a different-rate sequence is active.
The control-channel end-to-end run (import, timeline, drawing, cels, preview, all three exports) is
described in `docs/agents.md`. Still by hand: a real iPhone HEVC Main10 HLG MOV + AAC import,
preview/scrub/timing and each retained output. Synthetic fixtures do not substitute for that check,
and no real iPhone file has been checked.

`crates/ui-egui/tests/color_ui.rs` drives colour features the same way (`color-*.png` snapshots):
the Interpret Footage ▸ Color Management and Sequence Color Management dialogs.

Colour science tests: `waltcut-color` (curve round trips and published reference values per
camera log curve, BT.709/BT.2087 matrices, BT.2390 EETF, gamut mapping, LUT tetrahedral vs a
brute-force barycentric reference, `.cube`/`.3dl` round trips), `waltcut-gpu`
(`gpu_lut_matches_cpu_tetrahedral`: WGSL vs CPU, max |Δ| ≈ 2e-7), `render::colorman`,
`render::color_match`, engine `color_tests.rs`, and `export::tests::hdr_exports_signal_pq_and_hlg`
(PQ/HLG exports read back by our demuxer and by ffprobe: `color_transfer`, `color_primaries`,
`color_space`, mastering display and content light side data).

Not covered headless: `ui.screenshot` (needs a real viewport), the wgpu monitor path (the harness
runs the CPU texture path), audio output, and wall-clock playback advance (kittest frames do not
advance real time). Playing sound is covered without a device: `play_ahead` tests drive the
mixed-ahead ring with a wall-clock device (order, underruns stay in time, mixer panic, stop), and
`cargo run --release -p waltcut-ui-egui --example bench_audio -- <project.waltcut>` plays a real
project's sequences through a simulated 48 kHz device and counts dropouts, mixing in the callback
(`direct`, as before) against mixing ahead (`ahead`). Use `Driver` for new UI regressions: `d.exec(command, params)`,
`d.ok(method, params)`, `d.frames(n)`.

### Interactive checks

For visual work, drive the real app:

```sh
cargo run --release -p waltcut -- --control 9876
```

Then script it over the control channel or MCP: run commands, click by automation id,
`ui.inspect` / `ui.elements` to assert state, and `ui.screenshot {"path": "…"}` to capture the window
or a single panel (`{"panel": "Timeline"}`). Look at every screenshot. Examples are in
[agents.md](agents.md). The headless path (`waltcut-cli run script.jsonl`, MCP `--demo`) covers
engine behaviour without a window.

## 5. Performance

| Benchmark | Command |
|---|---|
| H.264 decode, 1080p, threads sweep | `cargo test --release -p waltcut-h264 --test perf -- --ignored --nocapture` |
| HEVC decode, 1080p / 2160p | `cargo test --release -p waltcut-hevc --test perf -- --ignored --nocapture` |
| ProRes decode/encode | `cargo test --release -p waltcut-prores --test perf -- --ignored --nocapture` |
| Encoder speed / PSNR / bitrate | `cargo run --release -p waltcut-h264enc --example h264enc_synth -- …` |
| Decode a real file through the media stack | `cargo run --release -p waltcut-cli -- bench-decode file.mp4 --frames 120` |
| Coding-tool coverage of the fixtures | `cargo test --release -p waltcut-h264 --test conformance coverage_report -- --ignored --nocapture` (same for `hevc`) |
| **Whole-app suite**: decode fps per codec, playback, scrubbing, timeline UI, export, project save/open, peak RSS | `cargo xtask bench` ([performance.md](performance.md)) |

`cargo xtask bench` (the example `crates/ui-egui/examples/bench/`) runs six sections, each in its
own process under `/usr/bin/time` so peak RSS is per section: `decode` (every frame of 1080p and
2160p H.264, HEVC, VP9, AV1 and ProRes through the media stack), `playback` (the scenarios below),
`scrub` (random seeks and playhead drags: time until the exact frame is on screen), `timeline`
(the real app under `egui_kittest` with a 1000-clip / 20-track sequence: update, tessellation and
wgpu render ms per frame), `export` (H.264 + AAC and ProRes) and `project` (save / open 5000 clips).
Options: `--sections decode,scrub`, `--only <substring>` (fixture or scenario), `--repeat N`,
`--quick`, `--cpu`, `--hw auto|off` (Settings ▸ Playback ▸ Hardware decoding; default `auto`, as the app), `--label NAME` (output `target/bench/bench-<label>.{json,md}`),
`--section NAME --json FILE` (one section in-process, e.g. under a profiler). Fixtures are made
with ffmpeg in `target/fixtures/playback/` (or `$WALTCUT_FIXTURES/playback`). The suite's
`playback` section plays `h264-1080`, `stack3` and `stack3-blend` at Full, `h264-2160` at Full, 1/2 and 1/4 and with
draft decoding (Settings ▸ Playback ▸ Draft decoding) at 1/2 and 1/4, and `hevc-2160`, `vp9-2160`
and `av1-2160` at Full, 1/2 and 1/2 draft; its `draft` column counts the frames decoded in draft
mode.

The perf tests check bit-exactness before they time anything. Results go in the crate README's
performance table, with machine and thread count. Headline numbers go in
[ROADMAP.md](../ROADMAP.md). Measure on an idle machine: parallel agent builds distort timings.

### Playback benchmark

`cargo xtask bench-playback` (the example `crates/ui-egui/examples/bench_playback.rs`) plays
sequences headlessly through the Program monitor's own frame scheduler: the `FrameServer` worker
pool, `schedule_playback` (prefetch order and stale-job dropping), its caches, and the
`PlaybackMeter` that counts shown/dropped frames in the app. On the GPU path it also composites
each plan with `waltcut-gpu`, as the monitor does on the UI thread.

```sh
cargo xtask bench-playback                                   # every scenario, GPU path, Full and Half
cargo xtask bench-playback --scenario stack3 --res full --cpu
cargo xtask bench-playback --json target/bench-playback.json # machine-readable results
```

| Scenario | What plays |
|---|---|
| `h264-1080` | one 1080p23.976 H.264 clip (testsrc2 + grain, ~45 Mbit/s, 250-frame GOP) |
| `stack3` | three 1080p H.264 clips on V1–V3; V2/V3 scaled, positioned, rotated, 70–85 % opacity |
| `stack3-blend` | `stack3` with V2 in Screen and V3 in Overlay (blend modes on the GPU) |
| `h264-2160` | one 2160p23.976 H.264 clip |
| `demo` | the built-in demo project (procedural footage, transitions, effects) |
| `after-preview` | 1080p H.264 + Sharpen/Levels/Tint: a live play, Render Effects In to Out, then two plays of the green segment |
| `seek-storm` | 40 jumps to random frames 150 ms apart (scrubbing), time until the exact frame shows |

Options: `--res full,half,quarter`, `--cpu` (CPU compositor + texture conversion instead of the
GPU path), `--seconds`, `--refresh` (display Hz), `--workers`, `--repeat`, `--json <file>`.
Fixtures are made with ffmpeg in `target/fixtures/playback/` (set `WALTCUT_FIXTURES` to share one
set between worktrees).

Columns: **shown/drop** as counted in the app (a frame is shown when its exact picture was on
screen at a refresh while it was due; frames passed over without a refresh count as dropped);
**ontime** = due frames whose job finished before they were due; **lat** = queue→ready per job,
**svc** = worker time per job; **cpu/j**, **src/j**, **srcC/j** = worker thread CPU, source fetch
(decode) wall and thread CPU per job; **ui** = time on the UI thread to present a frame (GPU upload
and draw, or texture conversion); **cpu ms/f** = process CPU per frame and **cores** = the cores that
needs at the sequence frame rate; **seeks/dec** = decoder restarts and samples decoded; **waste** =
jobs for frames that were never due.

Wall-clock columns (shown/drop, ontime, latencies) depend on machine load, so each row prints the
load average. CPU columns (thread and process CPU time) and the structural counters (seeks,
samples decoded, wasted jobs) do not, and are what to compare between runs on a busy machine.

Results on an M4 Pro (14 cores), GPU path, 8 s plays, median of 2 alternating runs of the M4.6
baseline (commit `5170376`) and the result of M4.6, on a machine shared with parallel agent
builds (**load average 86–207**, so wall-clock columns are pessimistic; CPU ms/frame is not):

| Scenario | shown/dropped before | after | CPU ms/frame before → after | decoder seeks |
|---|---|---|---|---|
| h264-1080 Full / Half | 106/86, 118/74 | **192/0, 192/0** | 80 → 46, 95 → 46 | 3–4 → 1 |
| stack3 (3 × 1080p) Full / Half | 185/7, 174/18 | **192/0, 192/0** | 136 → 109, 117 → 111 | 4 → 3 (one per source) |
| h264-2160 Full / Half | 0/192, 0/192 | 5/187, 19/173 | 110 → 135, 170 → 145 | 4–5 → 1–2 |
| demo Full / Half | 63/129, 124/68 | 97/95, 152/40 | 140 → 80, 93 → 40 | |
| after-preview, 1st play after render (Full) | 80/112 | **190/2** | 119 → 26 | 104 → 0 |
| after-preview, 2nd play (Full) | 145/47 | **192/0** | 76 → 26 | 132 → 0 |
| after-preview, live effects (Half) | 18/174 | 69/123 (frames skipped evenly) | 109 → 95 | |
| seek-storm (40 jumps, 150 ms each) | 0/40 shown | 6/40 | 827 → 666 per jump | |

At load ~25–60 the same final build plays h264-1080, stack3 and h264-2160 at Full with 0 dropped
in 3 of 3 runs (192/0) and render previews 192/0. The 4K fixture needs ~4 cores of decode per
real-time second (≈160 ms CPU per frame at 170 Mbit/s); the demo project's procedural footage
~190 ms per Full-resolution frame.

### Audio mixer throughput

`perf_24_tracks_3_effects_realtime_factor` (24 tracks, 72 inserts and a compressed submix on one
core) must run at least 4x realtime in release builds (1x in debug). On Unix it times the test
thread's CPU time, so it runs with the rest of the suite. Hosts without a per-thread CPU clock
(Windows) fall back to wall time, which depends on load, so there it is ignored by default; run it
explicitly with
`cargo test --release -p waltcut-render --lib perf_24_tracks_3_effects_realtime_factor -- --ignored --nocapture`
and record the machine with the result.
