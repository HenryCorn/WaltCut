# Performance

`cargo xtask bench` measures the whole app headlessly and writes `target/bench/bench-<label>.json`
and `.md` (options and sections: [testing.md](testing.md) §5). Agents read live counters with
`perf.stats` ([control-protocol.md](control-protocol.md)).

## Interactive builds and redraw work

For interactive editing, build with `cargo build --release -p waltcut` and
launch `target/release/waltcut`. The development profile optimizes dependencies and codec hot
loops, but still builds the editor at optimization level 1; it is not the release performance
baseline. Keep the same build profile when comparing code changes.

Timeline, Effect Controls, Markers and mixer drawing borrow a sequence from a shared
immutable project snapshot instead of deep-copying its clips and effects on every redraw. Commands
can still replace the session's project during the pass; the snapshot stays valid until drawing
finishes.
**Read the numbers with the load average.** These results come from a shared Apple M4 Pro
(14 cores, 48 GB, macOS 15 / Darwin 24.6) with parallel agent builds running: load average
**75–200** during every run below. Wall-clock columns (fps, shown/dropped frames, seek latency)
swing by 2–5× between back-to-back runs at that load and are only comparable within one run. CPU
time per frame, samples decoded and skipped, and peak RSS are much less sensitive, and are what
the before/after comparison relies on. Each milestone below alternated base / after runs (2 rounds
each); the most load-independent figure is the decoder's own cycle count (`proc_pid_rusage`
instructions and cycles of a process, all threads summed).

## Drawing workload baselines (#70)

Measured 2026-10-23 on an Apple M4 Pro (12 cores, 24 GB, macOS 15 / Darwin 24.6) with the
unmodified `origin/main` (commit `7940273c`) release build. **The machine was shared**: the load
average was 4–10 during the headless runs (see the `load` column of the JSON), so wall-clock
figures are upper bounds and only comparable with runs made under similar load; the min–max over
three repeats is shown to make that visible. Not measured: anything on a real pen tablet (#5), and
pen-to-photon latency, which needs a camera.

**Workloads** (fixed seed, nothing committed; `crates/ui-egui/examples/bench/drawing.rs`): a
1920×1080 drawing clip over a 20 s 1080p H.264 reference clip, strokes cycling Pen / Pencil /
Marker with every tenth an Eraser:

| name | unique cels | strokes per cel | samples per stroke | held for | samples in total |
|---|---|---|---|---|---|
| small | 12 | 20 | 40 | 2 frames | 9 600 |
| medium | 48 | 150 | 120 | 2 frames | 864 000 |
| stress | 96 | 1 500 | 200 | 1 frame | 28 800 000 |

### Headless suite

```sh
cargo xtask bench --sections drawing --only small --repeat 3 --label drawing-small   # also medium, stress
cargo xtask bench --sections drawing --only stress/play --repeat 1                    # one phase in its own process
WALTCUT_BENCH_SAVE=<dir> cargo run --release -p waltcut-ui-egui --example bench -- --section drawing --only small/raster   # also saves the workload as a project
```

Median (min–max) of three repeats:

| measurement | small | medium | stress |
|---|---|---|---|
| Rasterise one 1080p cel on the CPU, warm (ms) | 2.0 (2.0–2.0) | 40.8 (40.8–40.9) | 635 (627–642) |
| … first call (ms) | 2.3 (2.3–2.9) | 40.6 (40.1–45.7) | 627 (626–638) |
| Pen-up: `drawing.addStroke` of 150 samples (ms) | 0.03 (0.03–0.03) | 1.1 (1.0–1.2) | 31.5 (28.9–33.4) |
| Pen-up until the exact frame is ready (ms) | 12.1 (10.5–12.2) | 54.2 (54.2–58.5) | 683 (680–684) |
| Undo of that stroke (ms) | 0.00 (0.00–0.00) | 0.00 (0.00–0.00) | 0.00 (0.00–0.01) |
| Random seek until the exact frame is shown, p50 (ms) | 103 (98–105) | 151 (146–155) | 726 (721–733) |
| … p95 (ms) | 160 (159–177) | 208 (207–213) | 788 (788–793) |
| Playhead drag stops until its frame is shown, p50 (ms) | 39.1 (37.8–39.2) | 64.2 (55.4–71.5) | 790 (789–803) |
| Frames shown during a 24-frame playhead drag (%) | 0.00 (0.00–0.00) | 3.6 (3.1–3.6) | 3.1 (3.1–3.1) |
| Playback at full resolution: frames dropped (%) | 0.00 (0.00–0.52) | 0.00 (0.00–0.52) | 95.8 (95.3–97.4) |
| … frame job service time, p50 (ms) | 7.5 (7.3–7.6) | 49.5 (49.1–49.6) | 680 (678–683) |
| … p95 (ms) | 14.5 (13.8–14.6) | 56.6 (55.3–57.3) | 700 (698–702) |
| UI frame (update + tessellation), Brush tool, onion skin on (ms) | 0.60 (0.60–0.60) | 4.2 (3.9–4.3) | 87.4 (84.2–87.8) |
| … onion skin off (ms) | 0.49 (0.49–0.49) | 1.0 (0.9–1.2) | 36.2 (30.6–36.4) |
| … while a 600-sample stroke is drawn (ms) | 0.62 (0.62–0.62) | 4.2 (4.1–4.2) | 86.6 (86.5–88.3) |
| … p95 (ms) | 0.66 (0.65–0.69) | 5.0 (4.8–5.2) | 92.2 (89.8–94.3) |

| peak RSS (MB), phase in its own process | small | medium | stress |
|---|---|---|---|
| cel rasterisation only | 76 | 90 | 472 |
| 20 pen-up trials (5 for stress) | 1105 | 1301 | 1631 |
| scrubbing | 1676 | 1804 | 2238 |
| playback | 1825 | 1828 | 1534 |
| Brush tool UI frames | 310 | 745 | 2607 |

How the rows are measured: *rasterise* is `waltcut_render::drawing::render_strokes` on one
cel; *pen-up* is `drawing.addStroke` (parse, edit, undo snapshot), then the time until the frame
server returns the exact frame at the new revision; *scrub* and *playback* are the frame
scheduler of the existing `scrub` and `playback` sections with the drawing clip in the
sequence; the *UI frame* is the whole app under `egui_kittest` (layout, the egui painter that
draws the onion skins and the live stroke, tessellation; the wgpu draw is not included) at a
cel with a neighbour on each side.

### Native app

`tools/bench-drawing-native.py --project <saved workload>` drives the running desktop app over the
control channel (a fresh app per workload, hardware decoding on, the Program monitor at 1/2
resolution as the app chose it):

| measurement | small | medium | stress |
|---|---|---|---|
| Open the project (`file.open`), s | 0.0 | 0.6 | 42.0 |
| App RSS after opening (MB) | 130 | 212 | 1312 |
| `drawing.addStroke` round trip over the control channel, p50 (ms) | 0.7 | 8.2 | 145.7 |
| … max (ms) | 2 | 138 | 4571 |
| App UI frame time at the end of a synthetic 120-step drag (ms) | 2.1 | 6.4 | 110.6 |
| Playback 8 s at 1/2 resolution: frames shown / dropped | 200 / 0 | 200 / 0 | 64 / 127 |
| … audio callbacks / underruns / minimum lead (ms) | 717 / 0 / 167 | 716 / 0 / 167 | 691 / 0 / 167 |
| … frame render time p50 / p95 (ms) | 2.0 / 9.7 | 17.9 / 32.3 | 249.4 / 457.0 |
| … CPU seconds used while playing | 10.7 | 13.8 | 57.8 |
| … peak app RSS (MB) | 2577 | 2563 | 5849 |

A synthetic drag sends one pointer event per UI frame, so the frame time at its end is the cost of
a frame with a live stroke; it is not pen latency.

### Where the time and memory go

* **Cel rasterisation dominates frames with drawing.** A frame's service time is the cel's
  rasterisation (≈ 2 µs per stroke sample here: 2 ms, 41 ms and 635 ms per cel) plus a few ms of
  decode and composition; the reference video's decode is 0–8 ms per frame job (hardware
  decoding) in every row. Playback keeps up with *small* and *medium* (the prefetching frame workers hide the
  41 ms) and drops 96 % of the frames with *stress*, whose frames take 680 ms.
* **Onion skins are the UI cost.** The egui painter draws every stroke of both neighbouring cels
  as segments on every frame: with onion skin on, a UI frame costs 0.6 / 4.2 / 87 ms, and 0.5 / 1.0
  / 36 ms with it off (*stress*: 538 000 shapes per frame). A live stroke adds nothing
  measurable (600 samples = 1 200 shapes). Drawing with the *stress* project in the native app
  runs at ≈ 110 ms per frame.
* **The off-onion cost of *stress* (36 ms) is a copy.** `panels/drawing.rs::target` clones the whole
  `Drawing` (every cel's strokes) on every UI frame while the Brush tool is active, to read one cel;
  at 28.8 M samples that is ≈ 350 MB per frame. The clone is avoidable (the item holds an
  `Arc<Drawing>`).
* **Pen-up costs a copy of the drawing too.** `drawing.addStroke` takes 0.03 / 1.1 / 31.5 ms because
  the edit makes the item's `Arc<Drawing>` unique while the undo snapshot still shares it, which
  clones every cel. It grows with the whole drawing, not with the cel or the stroke. Undo is a
  pointer swap (< 0.01 ms). After the commit, the exact frame arrives 10 / 53 / 650 ms later, which is
  one cel rasterisation; until then the live stroke is gone and the old picture is shown.
* **Memory.** The frame-plan cache sits at its 1 GiB budget (`frames.cache.planMB` 1073 for 69
  plans, ≈ 15.5 MB each) in every workload after a few seconds of playback, so the app's RSS is
  2.5 GB for *small* and *medium* against 0.5–0.7 GB for the reference clip alone in the headless
  suite. *Stress* adds the drawing itself (a 28.8 M-sample drawing is ≈ 350 MB in memory,
  1.2 GB as a project file, 42 s to open) and its rasterised canvases: 5.8 GB at the peak.
* **Audio.** 0 underruns and a 167 ms minimum lead in all three workloads, even with 66 % of the
  video frames dropped: drawing load does not disturb the audio mixer.

### Attainable budgets and proposal

Budgets that the numbers support on this machine (they are workload-specific, not universal):
live drawing needs a UI frame under ≈ 8 ms (120 Hz) and a pen-up under ≈ 16 ms; playback at 24 fps
needs a cel under ≈ 40 ms for the prefetch to hold. *Small* meets all of them with room to spare. *Medium* meets the UI budget (4.2 ms with onion skin on) and
the pen-up budget (1.1 ms); its 41 ms cel is at the playback limit and plays without drops only
because six frame workers prefetch. *Stress* misses every budget by 10×–80×.

Focused proposal, in order of benefit per effort (each needs its own issue; none is started here):

1. Hold an `Arc<Drawing>` instead of cloning it every UI frame in the Brush tool (removes the
   36 ms floor of *stress*), and avoid the whole-drawing clone on `drawing.addStroke` (a persistent
   cel list, or `Arc` per cel, so an edit clones one cel).
2. Cache each cel's raster (the epic's #71) and draw onion skins from it instead of re-painting
   every stroke; invalidate by cel.
3. Keep the committed stroke visible between pen-up and the arrival of the new frame (#55).
4. Bound the frame-plan cache by the size of its layers, not 1 GiB of 15 MB plans, if lower
   memory matters.

## Memory of clips on a timeline (MEM1–MEM3)

Measured on the desktop release build with `footprint`, `heap` and `malloc_history`
(`MallocStackLogging=lite`), 2026-10-05: twelve H.264 clips dropped on a timeline took the process
to a 12 GB footprint, 10.8 GB of it live heap in three piles.

1. **Media files read whole (2.9 GB), MEM1.** The desktop host had no `Services::reader`, so
   `MediaPool::open_file` fell back to `read_file` and every clip's bytes stayed in memory for as
   long as its source was open. `FsServices::reader` now returns a
   `waltcut_media::reader::FileReader`: the file stays open and containers read their index and
   then single samples with positional reads, as the web host already did through `BlobReader`
   ([web.md](web.md)). Formats decoded in one go (stills, WAV, MPEG elementary streams) are still
   read whole through the reader.
2. **A decoder per clip, for good (3.5 GB), MEM2.** Every source kept its decoder once it had
   decoded a frame, with its reference pictures and the pictures its frame threads have in flight:
   about 290 MB per clip here (`h264` `PicState`, `Frame::make_row`). The GOP caches now share a
   pool (`crates/codecs/src/gop.rs`): beyond `MAX_LIVE_DECODERS` (4) the least recently used
   caches that have been idle for 3 s drop their decoder and make a new one when asked again,
   which costs what a seek costs. A cache in use (a layer of the frame being composited, the next
   clip being prefetched) always keeps its decoder, so a composite of more than four clips does
   not restart decoders on every frame. `perf.stats` reports `decode.liveDecoders`.
3. **A frame budget per clip, none overall (3.8 GB), MEM3.** Each GOP cache keeps up to 384 MB
   of decoded frames (at least 64 frames), which every clip on the timeline filled and kept. The
   pool now counts the frames of all caches against `FRAME_BUDGET` (1 GiB): over it, caches idle
   for 3 s give up frames, least recently used cache first, earliest frames first. Caches in use
   keep their own budget, because a frame evicted before it is shown costs a re-decode from the
   keyframe. `perf.stats` reports `decode.cacheMB` and `decode.cacheBudgetMB`; before, this
   memory appeared in no counter (`frames.cache` covers the frame server's rendered images and
   GPU plans only).

4. **Freed planes the allocator keeps (2.5 GB), MEM4.** With MEM1–MEM3 the same twelve clips took
   5.1 GB, only 2.5 GB of it live: every decoded picture allocated three planes and every
   eviction freed them, and the system allocator kept the freed pages (`footprint`: 1.7 GB
   reclaimable). `waltcut_frame::pool` now keeps the planes of evicted frames (up to 192 MiB per
   sample type; a frame something else still holds is left alone) and the H.264 decoder and the
   VideoToolbox path decode into them, so steady-state decoding allocates no planes.
   `perf.stats` reports `decode.planePoolMB` and `decode.planesReused`.

The same session showed that the desktop app never used hardware decoding: `register()` was an
argument of a `log::info!` that no logger evaluated (fixed under HW1; `decode.hardware.sessions`
was 0 with every frame decoded in software).

What remains per live decoder is unchanged: the H.264 decoder publishes each macroblock row of a
picture as its own allocations (`Frame::make_row`, five per row), which is where the 290 MB per
decoder comes from.

## Results (CPU compositor: layer rectangles, an opaque bottom layer and recycled images, before → after)

The CPU compositor is what export runs. In a profile of a 9:29 export it took about a third of the
busy CPU with the built-in H.264 encoder (the encoder took the other two thirds), and a faster
encoder leaves it as the largest cost of an export. It works on premultiplied linear `f32` images,
33 MB each at 1080p, and every layer was one of them: a banner that is transparent except for its
bottom 100 rows still cost a full conversion and a full mix, and the bottom layer paid for a
zero-filled canvas and a pass that mixes it over that canvas.

Apple M1 (8 cores: 4 performance + 4 efficiency), 2026-10-06, a machine that was not idle, so the
two builds ran alternately in the same session and the median is reported.

### Per frame, synthetic 1080p sources

`cargo run --release -p waltcut-render --example frame_bench`: an 8-bit 4:2:0 camera picture and a
10-bit 4:4:4 banner with alpha only in its bottom 100 rows; `render_sequence` followed by the
conversion to 8-bit RGBA, milliseconds per frame (median of 40 frames, 5 alternating rounds):

| scenario | before | after |
|---|---|---|
| camera only | 8.9 | 6.5 |
| camera + banner | 16.2 | 7.8 |

Both builds give the same picture (same checksum).

### A real export

30 s of a 1080p25 camera clip with a ProRes 4444 banner, exported with the built-in encoder
(deterministic, byte for byte): 46.4 s before, 36.7 s after. **The two exports are the same file**
(same bytes, same MD5), so every frame of the real pipeline (camera decode, ProRes decode,
compositing, conversion) matches. The encoder is most of that time; with a faster one the
compositor's savings are a larger part of the export.

What is left in a frame without overlays is the Y'CbCr → linear float conversion of the camera
picture (4.5 ms in the synthetic case) and the conversion to 8 bits (2 ms).

## Retained GPU export composition

Export ▸ GPU Rendering composites the exported frames on the GPU (`waltcut-gpu`'s off-screen
compositor, a pool of up to four renderers) instead of the CPU reference renderer, which stays the
fallback for anything the GPU stage does not run. Linux, Ryzen 7 9700X (16 threads) + Radeon
RX 7900 (Mesa 26.2.2), 2026-10-08 / 09, same build, `cargo xtask bench --sections export --only
<scene> --hw off|auto`: 191 frames to H.264 + AAC with the built-in encoder. `h264` is one 1080p
clip; `h264_fx` the same clip on V1 with Proc Amp, Gaussian Blur and Vignette, plus two
picture-in-picture copies (50 % with Brightness & Contrast, 33 % at 70 % opacity with Sharpen).
Every Auto row rendered all its frames on the GPU, without fallbacks.

| scene | Off: s / CPU ms per frame | **Auto**: s / CPU ms per frame |
|---|---|---|
| `h264`, 1080p (load 12–33) | 5.96, 6.08 / 292 | 6.07, 6.36 / 281 |
| `h264_fx`, 1080p (load 12–33) | 13.00, 12.67 / 737 | **5.63, 5.00 / 238** |
| `h264_fx`, 4K sequence and clip (load 2–19) | 38.74, 39.34 / 2955 | **11.99 / 775** |

On a plain clip the export is decoding- and encoding-bound and both take the same time; with
layers and effects the GPU is 2.4× (1080p) to 3.2× (4K) faster, with a third to a quarter of the
CPU. GPU rendering is nevertheless **Off by default** (opt-in per export) until it has been
measured on Windows and macOS: the GPU result matches the CPU within the compositor's agreement
tolerance, not bit for bit, and Off keeps exports byte-reproducible on every machine. (The 4K row used the same scene with the sequence, clip and positions scaled to 4K.)

### Memory after an export

The recycled images stay on the shelves of `waltcut_frame::pool` (up to 320 MiB of them, and
192 MiB of each plane type), and nothing else asks for images of an export's size, so a finished
export left them idle. A standalone export's pipeline now frees the idle float images when it goes
away, however the export ended (done, failed, cancelled, dropped half way). The plane shelves are
kept (playback recycles decoded frames through them), and so are the float images after one part
of a batch (a render-preview segment, a proxy), which the next part reuses. A process that exported a synthetic 3 s 1080p24 matte as PNG frames three times
(Apple M1, `footprint`, 10 s after the last export) rested at **575 MB** with the pool holding
332 MB idle and at **258 MB** with the trim. The memory does not come back at once: macOS returns
freed pages over a few seconds (892 MB right after the export in both cases, 726 MB after 2 s).

## Results (HW1: VideoToolbox hardware decoding, Off → Auto)

Same commit, Settings ▸ Playback ▸ Hardware decoding switched with the bench flag `--hw off` /
`--hw auto` (`cargo xtask bench --sections decode --only dec_h --repeat 3 --hw …`, then
`--sections playback --only h264` and `--only hevc-2160`), 2026-10-05, Apple M4 Pro, load average
**150–190** throughout (shared with other agent builds). The decoded pictures are identical either
way (bit-exact agreement tests, `crates/platform/tests/videotoolbox.rs`); only who decodes changes.

### Decode (every frame through the media stack)

| codec | size | CPU ms/frame Off → **Auto** | fps Off → **Auto** (best of 3) |
|---|---|---|---|
| H.264 | 1080p | 31.8 → **1.2** | 153 → **389** |
| H.264 | 2160p | 119.3 → **4.2** | 35 → **107** |
| HEVC | 1080p | 22.2 → **1.1** | 185 → **708** |
| HEVC | 2160p | 85.7 → **3.7** | 49 → **217** |

CPU time per frame is what is left on the CPU: the sample copy into a `CMSampleBuffer`, copying
the decoded biplanar picture out into planar Y'CbCr (chroma deinterleaved; 10-bit shifted down),
and the GOP cache. An earlier synchronous version (one picture decoding at a time) reached only
155 / 51 / 449 / 149 fps; asynchronous decompression with two access units in flight overlaps
decoding with the copy-out.

### Program-monitor playback (8 s, GPU path)

| case | shown/dropped Off → **Auto** | CPU ms/frame Off → **Auto** |
|---|---|---|
| H.264 1080p Full | 192/0 → **192/0** | 38.1 → **3.8** |
| H.264 2160p Full | 104/88 → **192/0** | 119.8 → **9.2** |
| H.264 2160p 1/2 | 1/191 → **192/0** | 95.5 → **9.0** |
| H.264 2160p 1/4 | 0/192 → **192/0** | 94.3 → **8.8** |
| HEVC 2160p Full | 175/17 → **192/0** | 105.6 → **9.1** |
| HEVC 2160p 1/2 | 190/2 → **192/0** | 97.4 → **8.8** |
| HEVC 2160p 1/2 draft | 190/2 → **191/1** | 107.0 → **22.3** |

At this load the software decoders drop most 4K H.264 frames; with hardware decoding every case
plays in real time at a tenth of the CPU. Draft playback costs more CPU than full-quality
playback with hardware decoding: the hardware ignores draft mode (`set_draft` is a no-op) and
the draft plan's box decimation of the planes, cheap next to software decoding, is now the main
CPU cost. Zero-copy upload of the decoded `CVPixelBuffer` into wgpu (no copy-out at all) is the
next step (issue #30).

## Export: overlapped rendering and encoding

The retained H.264/ProRes stepped exporter renders batch k+1 while encoding/muxing/mixing batch k,
using `rayon::in_place_scope`. Encoders stay on the calling thread. Fixed 16-frame audio/video
interleave and H.264 slice layout preserve CPU output bytes across core counts, step sizes and
overlap settings; `determinism_tests.rs` checks those retained paths.

Prefetched incomplete media is discarded and retried; cancellation drops prefetched frames and a
render-ahead panic becomes an export error. Two in-flight payload batches are capped by
`IN_FLIGHT_BUDGET` (512 MiB), reducing the batch size as needed. HDR payload accounting includes
both RGB floats and RGBA/alpha (16 bytes/pixel). This is a payload budget, not a whole-process RSS
promise; decoding, compositor working images and encoders also use memory.

Frames go in as planar Y'CbCr and out as compressed frames (NV12 fill, `VTCompressionSessionEncodeFrame`,
flush); quality is the luma PSNR of our software decoder's output against the input.

| target | B-frames | real bitrate | luma PSNR | frames/s |
|---|---|---|---|---|
| 4 Mb/s | off | 3982 kb/s | 41.66 dB | 202 |
| 8 Mb/s | off | 7987 kb/s | 43.44 dB | 202 |
| 16 Mb/s | off | 15972 kb/s | 45.55 dB | 202 |
| 4 Mb/s | on | 4220 kb/s | 41.37 dB | 183 |
| 8 Mb/s | on | 8268 kb/s | 43.41 dB | 197 |
| 16 Mb/s | on | 16388 kb/s | 45.66 dB | 199 |

B-frames buy nothing measurable here (±0.3 dB) and overshoot the target by up to 5 %, so the hardware
encoder runs without them: frames come out in presentation order and the muxer needs no composition
offsets or edit list.

### Whole exports (1080p25 camera clip with ProRes 4444 overlays with alpha, AAC, loudness normalised to −14 LUFS)

A 9:29 timeline (569 s, 14 227 frames): the camera clip, an intro, six banners, three subscribe bars
and an outro.

| encoder | wall time | frames/s | vs real time | file |
|---|---|---|---|---|
| built-in (`waltcut-h264enc`) | 793 s | 17.9 | 1.4× slower | 1445 MB, 20.0 Mb/s |
| hardware (VideoToolbox) | **311 s** | **45.7** | 1.8× faster | 1417 MB, 19.6 Mb/s |

2.5× faster. The first 134 s of the same timeline, where the overlays are densest (intro, a banner, a
subscribe bar and the outro in 134 s):

| encoder | wall time | frames/s | vs real time | file |
|---|---|---|---|---|
| built-in (`waltcut-h264enc`) | 395 s | 8.5 | 2.9× slower | 340 MB, 20.0 Mb/s |
| hardware (VideoToolbox) | **84 s** | **39.9** | 1.6× faster | 312 MB, 18.3 Mb/s |

4.7× faster there. The gain depends on what else an export spends its time on: with the encoder
off the CPU, what is left is decoding the camera clip, the CPU compositor (every layer, alpha
included) and the RGBA → Y'CbCr conversion.

The pairs of files have the same frame count and duration, decode in ffmpeg without a message, and
compare at SSIM 0.990 / PSNR 46.7 dB over the 134 s (lowest frame: 45.1 dB) and SSIM 0.989–0.990 /
PSNR 46.3–46.6 dB at six 3-second spots of the full export. Audio is identical.

Where the built-in export's time goes (`sample` on the process during a 20 s export, inclusive CPU
time in the call tree; the figures overlap because work-stealing mixes the tasks): about two thirds
of the busy CPU is in the encoder, about a third in compositing and layer decoding. Moving the encoder to
the media engine removes most of the first and lets the next batch render while the previous one
encodes (the exporter now does that: see "Export: rendering the next batch while the current one is
encoded" below). What is left is the
CPU compositor, the next target of #30 ("GPU export").

## Export: rendering the next batch while the current one is encoded

Until now a step of the stepped exporter (`waltcut_export::Exporter`) rendered a batch of frames in
parallel and only then encoded, muxed and mixed the audio of that batch on one thread, so the wall
time was the sum of the stages. Where the sum was large (1080p30, 300 frames, `perf.stats`
`export.stages`) the stages were: software H.264 render 4257 ms, encode 17845 (RGB to YUV 766 of it),
audio 462, mux 17, finish 142 = 22.96 s; NVENC H.264 setup 465, render 3316, encode 1113 (convert 794),
audio 471, mux 20, finish 24 = 5.69 s.

Now a step renders batch k+1 on the rayon pool while the calling thread encodes, muxes and mixes the
audio of batch k (`rayon::in_place_scope`), and the next step starts from the frames already rendered.
Wall time is about `max(render, encode + audio + mux)` instead of the sum. What did not change:

- **The file.** Batches are cut and encoded in the same order and the audio / mux interleave still
  follows the 16-frame groups, so the output is byte-identical with or without overlap, on any core count
  and with any step size, including one frame per step (tests in `determinism_tests.rs`: H.264, ProRes,
  Motion-JPEG, two-pass).
- **Pending media.** A prefetched batch whose sources were not ready is dropped and rendered again by a
  later step; nothing is encoded from an incomplete batch. The web app (no threads) never overlaps: it runs
  the sequential render-then-encode as before.
- **Cancel and errors.** Cancelling drops the prefetched frames; a panic while rendering ahead becomes an
  export error. The encoder stays on the thread that called `step`.
- **Memory.** Two batches are in flight (the one being encoded and the one being rendered), so up to
  `2 * batch * frame_bytes` of finished frames are held, and the batch is lowered until that fits in
  `IN_FLIGHT_BUDGET` (512 MiB; at least one frame): 16 frames of 1080p (8 MB), 8 + 8 frames of 4K SDR
  (33 MB), 2 + 2 frames of 4K HDR (100 MB, 3840 x 2160 x 3 f32), 1 of 8K HDR. A batch never grows, and
  frames are freed as soon as they are encoded. A first try with a 1 GiB budget (16 + 16 frames of 4K SDR,
  5 + 5 of 4K HDR) raised the peak RSS of the 4K SDR exports by 80-700 MB over the old code; 512 MiB
  brings it under the old peak (table below). `Exporter::set_overlap(false)` gives the old behaviour.
- **Known web issue, not changed here.** `AudioOut::pull` moves its position before the pending check, so
  when a web export retries a batch after a pending audio source, that group of audio is lost. It is
  independent of the overlap (the web never overlaps); TODO: fix separately.

`perf.stats` `export.stages`: `renderMs` and `encodeMs` (and audio / mux) now run at the same time, so
their sum can exceed the export's wall time. `waitMs` is the time the encoding side spent waiting for the
next batch to finish rendering after it was done encoding: large means render-bound, near zero means
encode-bound.

Measured (Xeon E5-2680 v4 14C/28T, RTX 5060, driver 617.42, Windows 11, release CLI, 300 frames of
30 fps, mean of 2 alternated rounds, export wall time, peak RSS of the CLI process; "before" is the same
harness on the commit without the overlap, "now" the 512 MiB budget; the 1080p rows were measured with a
1 GiB budget, which gives the same batches at that size):

| Case | wall before | wall now | speedup | `waitMs` now | peak RSS before / now |
|---|---|---|---|---|---|
| 1080p SW H.264 + AAC | 22.60 s | 20.66 s | 1.09x | 0 | 1279 / 1427 MB (+148) |
| 1080p NVENC H.264 | 5.56 s | 4.99 s | 1.11x | 49 ms | 1318 / 1441 MB (+123) |
| 1080p ProRes 422 HQ | 16.97 s | 15.32 s | 1.11x | 0 | 1234 / 1392 MB (+158) |
| 2160p SW H.264 | 60.79 s | 54.12 s | 1.12x | 0 | 4030 / 3113 MB (-917) |
| 2160p NVENC H.264 | 18.08 s | 15.64 s | 1.16x | 678 ms | 4177 / 3250 MB (-927) |
| 2160p ProRes 422 HQ | 60.92 s | 55.71 s | 1.09x | 0 | 3675 / 3140 MB (-535) |
| 2160p HDR PQ SW H.264 | 82.79 s | 77.62 s | 1.07x | 6758 ms | 5284 / 2560 MB (-2724) |
| 2160p HDR PQ ProRes 422 HQ | 89.14 s | 83.95 s | 1.06x | 215 ms | 5183 / 2565 MB (-2618) |
| 1080p effects NVENC H.264 | 28.70 s | 26.82 s | 1.07x | 13116 ms | 2044 / 2130 MB (+86) |
| 1080p effects SW H.264 | 39.60 s | 34.35 s | 1.15x | 0 | 2017 / 2084 MB (+67) |

With the first 1 GiB budget the 2160p rows were 55.42 / 16.00 / 56.75 s at 4404 / 4254 / 4376 MB peak and
the HDR rows 76.69 / 84.39 s at 3504 / 3532 MB: the same speed within 2 % (the HDR SW row 1.2 % slower
at 512 MiB), but 4K SDR used up to 700 MB more than the old code. Halving the budget costs no time and
makes every 4K export smaller than before.

The gain is 6-16 %, well under the `max(render, encode + audio + mux)` bound (1.06-1.86x). The reason is
that the encode side is not one core: the software H.264 and ProRes encoders run their slices on the same
rayon pool, and the RGB to YUV conversion is parallel too, so render and encode compete for the same
cores and overlapping them only fills idle gaps (the serial parts: entropy coding tails, muxing, audio,
the encoder's hand-off). Only NVENC, whose encode is light on the CPU, comes close to the bound in the
1080p case (4.99 s against 4.00 s). The effects case on NVENC is render-bound (`waitMs` 13 s of 27 s: the
encoder waits for the renderer), as expected. 1080p exports use 70-160 MB more (the second batch of 16
frames); 4K exports use less than before because the batch is smaller (8 for SDR, 2 for HDR instead of 16).

## Results (HW3: VideoToolbox hardware H.265 (HEVC) encoding, against hardware H.264)

Apple M1 (8 cores, 16 GB), 2026-10-07, single runs on an otherwise idle machine, release build. Both
encoders are the M1's media engine. H.265 is `Format::Hevc` (Main, 8-bit 4:2:0, one-pass VBR, no
B-frames); H.264 is the opt-in hardware encoder of HW2 (High).

### Speed

The raw encoders (ffmpeg's `hevc_videotoolbox` and `h264_videotoolbox` on a 1080p25 test pattern, 500
frames): 166 and 189 frames/s. H.265 is the codec that compresses further, not the faster one to encode.

The 9:29 timeline of HW2 (camera clip, ProRes 4444 overlays, AAC, loudness normalised to −14 LUFS), 20 Mb/s
H.264 and 10 Mb/s H.265: **204 s and 195 s** (CPU 1042 s and 1029 s), files of 1417 MB and 719 MB. The
same time: the same H.264 export took 193 s and 200 s on other runs. The export is bound by the CPU
compositor and the decoders, not by the encoder, so a different hardware codec does not change it.

### Quality per bit

30 s with a banner (15–45 s of that timeline), exported by WaltCut as ProRes 422 HQ (the reference)
and by each encoder at several bitrates; ffmpeg's `psnr` and `ssim` of the decoded 4:2:0 pictures against
the reference. Average PSNR over the three planes, and SSIM (higher is better):

| Mb/s | H.264 | H.265 | difference | H.264 Mb/s for H.265's PSNR |
|---|---|---|---|---|
| 20 | 46.88 dB, 0.9902 | 46.91 dB, 0.9903 | +0.03 dB | beyond the range measured |
| 10 | 44.96 dB, 0.9863 | 45.09 dB, 0.9863 | +0.13 dB | 10.5 |
| 6 | 43.72 dB, 0.9833 | 44.31 dB, 0.9843 | +0.59 dB | 7.7 |
| 4 | 42.91 dB, 0.9807 | 43.64 dB, 0.9825 | +0.73 dB | 5.8 |
| 3 | 42.37 dB, 0.9786 | 43.05 dB, 0.9807 | +0.68 dB | 4.3 |

At 10 Mb/s and above the two give the same picture. At 3–6 Mb/s H.265 reaches the same PSNR with 22–31 %
less bitrate. Its worst frame is better at every bitrate (lowest per-frame PSNR 45.9 against 42.1 dB at
20 Mb/s, 43.1 against 41.5 dB at 10, 39.5 against 37.8 dB at 3).

So there is no case for a lower default bitrate: the two formats start at the same 20 Mb/s. H.265
pays off where the file size matters, at 3–6 Mb/s; that 9:29 timeline is 428 MB at 6 Mb/s.

## Results (GPU1: blend modes on the GPU compositor, #30, before → after)

Before = this change with the old whole-frame CPU fallback for non-Normal blend modes put back
in `render::plan` (same binary otherwise); after = GPU1. New `stack3-blend` scenario: `stack3`
(three 1080p23.976 H.264 clips, V2/V3 scaled, positioned, rotated, 70–85 % opacity) with V2 in
Screen and V3 in Overlay. `cargo xtask bench-playback --scenario stack3-blend,stack3 --res
full,half`, GPU path, 8 s plays, two alternating rounds each on 2026-10-05 at load average
33–63 (1-minute; 5-minute 67–81).

| case | shown/dropped before (2 runs) | after (2 runs) | CPU ms/frame before → after | worker svc p50 before → after |
|---|---|---|---|---|
| stack3-blend Full | 19/173, 138/54 | **191/1, 192/0** | 168, 214 → **86, 87** | 258, 219 → 1.2, 1.2 ms |
| stack3-blend 1/2 | 192/0, 192/0 | 191/1, 192/0 | 158, 151 → **89, 88** | 66, 99 → 1.1, 1.0 ms |
| stack3 Full (control, Normal) | 192/0, 192/0 | 191/1, 192/0 | 88, 88 → 91, 88 | 1.3, 1.6 → 0.9, 1.5 ms |
| stack3 1/2 (control, Normal) | 191/1, 192/0 | 192/0, 192/0 | 82, 90 → 90, 89 | |

A stack with blend modes now costs what the same stack in Normal does: the frame workers only
decode, and the UI-thread present (upload and draw, including the backdrop copies for the two
blended layers) stays at 9–10 ms p50, as for `stack3`. Before, every frame was composited on the
CPU (≈ 2 cores more at 23.976 fps) and Full resolution could not keep up at this load.

## Results (#847: AV1 in-loop filters in parallel row bands, before → after)

The AV1 decoder ran deblocking, CDEF and loop restoration on one thread per frame. A single-tile
stream (what SVT-AV1 and most encoders write by default) has no other parallelism inside a
frame, and a frame's references are waited for as whole frames, so this serial tail sat on the
critical path of every frame that references it. The three filters now run in bands of 64 luma
rows on the decoder's rayon pool, one join per filter per frame (the loop filter recomputes the
8 rows above each band rather than splitting into two passes, because every join costs a worker
wake-up, which is slow on a busy machine). Output is bit-exact with the single-threaded path and
with libdav1d (all oracle and conformance tests, plus new 4:2:2 / 4:4:4 libaom fixtures); with
one thread the code path is the old one.

Linux, AMD Ryzen 7 7800X3D (8 cores / 16 threads), 2026-10-11, shared with other agents' builds;
base = `2ff3f5e`, after = this change, 16 decoder threads, SVT-AV1 preset 8 fixtures from
`crates/av1/tests/perf.rs` (`perf_1080p_lowdelay` and `perf_2160p_gop` are new there).

`av1dec` example (`AV1_LOOPS=2 AV1_THREADS=16`), 8 rounds alternating base / after per fixture,
load average 11–27; speed-up = base wall time / after wall time of the adjacent pair:

| fixture | fps base → after (median) | paired speed-up, median (range) | after faster | CPU ms / frame (all threads) |
|---|---|---|---|---|
| `perf_1080p_lowdelay` (each frame refs the last) | 31.0 → **39.9** | 1.28 (0.85–1.48) | 6 / 8 | 44.2 → 55.0 |
| `perf_1080p_gop` (crf 30) | 74.8 → **97.7** | 1.30 (1.06–1.41) | 8 / 8 | 30.2 → 35.9 |
| `perf_1080p_gop_hq` (crf 18) | 32.0 → **37.5** | 1.16 (1.04–1.29) | 8 / 8 | 66.9 → 77.1 |
| `perf_2160p_gop` (one tile) | 23.0 → **25.3** | 1.14 (1.08–1.41) | 8 / 8 | 103.1 → 115.5 |
| `perf_1080p_intra` (10 key frames) | 121.3 → 138.1 | 1.03 (0.92–1.21) | 4 / 8 | 40.8 → 45.5 |

Per-stage wall time on the frame worker (`tests/perf.rs`, 16 threads, ms per frame, median of 2
alternating rounds at load average 8–11):

| fixture | 16 threads fps | deblock | CDEF | restoration |
|---|---|---|---|---|
| `perf_1080p_lowdelay` | 31.8 → **48.7** | 2.03 → 0.63 | 5.16 → 1.62 | 5.81 → 1.40 |
| `perf_2160p_gop` | 24.0 → **31.9** | 2.78 → 0.81 | 7.38 → 3.74 | 7.84 → 2.70 |
| `perf_1080p_gop_hq` | 34.8 → 33.6 | 1.10 → 0.55 | 6.53 → 3.08 | 4.94 → 1.65 |

One thread is unchanged (`perf_1080p_lowdelay`, 8 pairs: 2.19–2.20 s both). The extra CPU time
is mostly rayon workers spinning briefly before they sleep after each join, plus the loop
filter's recomputed margin rows. Key-frame-only streams gain little: their frames are
independent, so frame threads already keep every core busy. What remains is the tile decode
itself (entropy decoding and reconstruction of a single tile, 65–80 % of a frame), which only
row-level frame threading (waiting for reference rows instead of whole frames) can overlap.

## Results (VP9 loop filter wavefront, #980, before → after)

The VP9 post stage of a frame (band assembly, loop filter, publication) ran on one thread, and
later frames wait per band for their references, so the loop filter (about 30 % of the decode
profile) set the pace of every inter frame: with 16 threads the tile columns spent most of their
time waiting for the previous frame's bands. The loop filter now runs as a wavefront over
superblock rows (row r two superblocks behind row r - 1, which is what the raster order of 8.8.1
allows), the post thread and helpers on the decoder's pool each taking a row. No lock is held
while a superblock is filtered: rows exchange copies of the 8 rows the filter reaches across a
band boundary. Output is bit-identical (the conformance tests compare 1, 2, 3, 5 and 16 threads
with ffmpeg).

AMD Ryzen 7 7800X3D (8 cores / 16 threads), shared with other agents' builds, 2026-10-11.
`vp9dec` example, 16 threads, runs alternating base / after; paired speed-up = base wall time /
after wall time per pair:

| fixture | load | paired speed-up, median (range) | after faster | CPU ms/frame |
|---|---|---|---|---|
| `bench_1080p` (4 tile columns) | 15–18 | 1.39 (1.20–2.09) | 6/6 | 13.5 → 15.3 |
| `bench_4k` (8 tile columns) | 15–18 | 1.52 (1.49–1.59) | 6/6 | 48.0 → 57.8 |
| 1080p30 realtime, noisy (6 Mb/s) | 15–18 | 1.56 (1.27–1.72) | 6/6 | 15.3 → 20.8 |
| `bench_1080p` | 25–33 | 1.17 (0.91–1.65) | 4/6 | 15.7 → 17.9 |
| `bench_4k` | 25–33 | 1.29 (0.99–2.03) | 5/6 | 56.9 → 63.1 |
| 1080p30 realtime, noisy | 25–33 | 1.53 (1.40–1.94) | 6/6 | 22.5 → 25.4 |

With one thread the rows run one after the other: 31.12 → 31.28 G user instructions for the noisy
1080p clip (+0.5 %, `perf stat`). The extra CPU with threads is mostly rayon workers spinning
before they sleep. With the machine oversubscribed (load above 16) the wavefront gains less, as
every row waits for the one above.

## Results (M4.10: 4K VP9, AV1 and HEVC, before → after)

Baseline = commit `a814ff6` (decoders unchanged since M4.8); after = M4.10. Interleaved runs on
2026-10-03 at load average 170-320 (two base and two after rounds; the second bench round of
each overlapped another bench run, so its wall-clock columns are extra pessimistic).

### Decoder work per frame (load-independent)

Codec examples (`vp9dec`, `av1dec`, `hevcdec`) on the `cargo xtask bench` decode clips, CPU
cycles counted by the kernel (`/usr/bin/time -l`, all threads summed), mean of the two rounds;
output bit-exact with ffmpeg / libdav1d at every thread count:

| stream | 1 thread before → after | 14 threads before → after | wall ms / frame, 14 threads, before → after |
|---|---|---|---|
| VP9 1080p | 68.0 → **52.0** (−24 %) | 100.7 → **57.9** (−43 %) | 95, 82 → 18, 17 |
| VP9 2160p | 273.6 → **205.8** (−25 %) | 391.7 → **219.2** (−44 %) | 146, 118 → 38, 36 |
| HEVC 1080p | 90.9 → **67.3** (−26 %) | 100.2 → **75.1** (−25 %) | 17, 10 → 5, 13 |
| HEVC 2160p | 362.6 → **262.3** (−28 %) | 398.3 → **284.8** (−28 %) | 24, 55 → 26, 25 |
| AV1 1080p | 60.5 → **48.3** (−20 %) | 63.6 → **51.9** (−18 %) | 26, 8 → 8, 7 |
| AV1 2160p | 212.5 → **172.4** (−19 %) | 220.0 → **177.1** (−20 %) | 158, 39 → 20, 32 |

Mcycles per frame. Draft mode (1 thread): HEVC 2160p 232.5 (−11 % on top), AV1 2160p 167.9
(−3 %: SVT-AV1's top-layer frames filter little); the VP9 clips have no non-reference frames.

### Decode (every frame through the media stack)

| codec | size | CPU ms/frame before (2 rounds) | after (2 rounds) | fps before → after |
|---|---|---|---|---|
| VP9 | 1080p | 33.6, 30.8 | **23.5, 21.7** | 14, 17 → 67, 56 |
| VP9 | 2160p | 120.2, 117.7 | **68.4, 67.9** | 9.1, 7.8 → **34, 27** |
| HEVC | 1080p | 31.2, 29.3 | **23.9, 22.4** | 50, 58 → 138, 145 |
| HEVC | 2160p | 117.9, 111.8 | **86.9, 82.1** | 13, 14 → **30, 41** |
| AV1 | 1080p | 18.5, 18.9 | **15.1, 15.7** | 53, 44 → 122, 69 |
| AV1 | 2160p | 64.8, 66.7 | **48.4, 51.3** | 16, 8.9 → **25, 25** |

### Program-monitor playback, 2160p (8 s, GPU path; new `vp9-2160` / `av1-2160` scenarios)

| case | before: shown/dropped (4 runs) | after (4 runs) | CPU ms/frame before → after |
|---|---|---|---|
| VP9 Full | 6/186, 53/139, 0/192, 0/192 | **192/0, 192/0, 189/3, 192/0** | 23-49 → 75-83 |
| VP9 1/2 | 6/186, 58/134, 2/190, 52/140 | **192/0, 191/1, 192/0, 192/0** | 34-70 → 78-80 |
| HEVC Full | 6/186, 1/191, 0/192, 9/183 | **192/0, 192/0, 192/0, 192/0** | 74-124 → 91-97 |
| HEVC 1/2 | 0/192, 9/183, 0/192, 19/173 | **192/0, 192/0, 192/0, 185/7** | 64-118 → 94-97 |
| HEVC 1/2 draft | 5/187, 88/104, 97/95, 138/54 | **188/4, 192/0, 187/5, 189/3** | 135-148 → 96-104 |
| AV1 Full | 4/188, 19/173, 0/192, 36/156 | 52/140, 0/192, **178/14**, 0/192 | 31-47 → 38-56 |
| AV1 1/2 | 32/160, 44/148, 1/191, 38/154 | 2/190, **150/42, 184/8**, 50/142 | 30-50 → 41-72 |
| AV1 1/2 draft | 1/191, 24/168, 1/191, 27/165 | **192/0, 187/5, 184/8, 192/0** | 25-51 → 68-70 |

CPU per *displayed* frame rises where the decoder now keeps up: before, most frames were never
decoded (dropped jobs cost nothing). At load ~180-220, 4K VP9 and HEVC now play in real time at
Full and 1/2; AV1 does at 1/2 with draft decoding and sometimes at 1/2 / Full: its frames
decode concurrently only once their references are complete (whole-frame dependencies) and the
clip has a single tile, so a 4K frame waits for its references' deblocking, CDEF and output
copy.

## What M4.10 changed

1. **VP9 frame threading** (the decoder had none: tile columns, then a loop-filter wavefront,
   then assembly and output, each a barrier — 14 threads used 40-50 % more cycles than one and
   barely ran faster). Frames are now published one superblock row at a time once loop
   filtered; each frame's post stage (band assembly from the tile strips, loop filter in raster
   order, publication, output picture) runs on its own thread while the next frames' tile
   columns decode, their motion compensation waiting per band for exactly the reference rows it
   reads. Backward-adaptive streams only wait for the previous frame's symbol counts.
2. **VP9 kernels**: the loop filter's lane masks were `bool`s with `clamp` and branchy selects,
   which kept it scalar; 0 / -1 integer masks, bitwise selects and min / max make it NEON. The
   inverse transform's column pass runs the generated butterflies on 4 / 8 columns at once;
   8-bit sub-pixel filters accumulate in 16-bit lanes (every VP9 filter sum fits modulo 2^16).
3. **HEVC**: motion compensation zeroed two 10 KB windows per block (11 % of the time in
   `memset`) and filtered with dynamic-width scalar sums; it now reuses scratch windows and
   filters taps-outer over fixed widths. SAO (20 % of the time) selects edge / band offsets per
   lane instead of a table lookup per sample. Runs of bypass bins decode with one division;
   output clipping uses min / max. Frame threading with per-CTB-row waits was already in place.
4. **AV1**: fixed-width sub-pixel filter passes; frame and tile-region planes recycled through a
   bounded pool instead of a fresh `calloc` / `munmap` per frame (~10 % of the time); palette
   mode-info arrays (40 bytes per 4x4) only with screen content tools; branch-free CDF update.
5. **Draft mode** for all three through `VideoDecoder::set_draft` (Settings ▸ Playback ▸ Draft
   decoding, reduced-resolution playback only): VP9 frames that refresh no reference slot skip
   the loop filter; HEVC sub-layer non-reference pictures of the top sub-layer skip deblocking
   and SAO; AV1 shown frames that refresh no slot skip deblocking, CDEF and loop restoration.
   Tested per codec (unflagged pictures bit-exact, single- and multi-threaded) and through the
   media stack (`crates/codecs/tests/draft_decoders.rs`: draft frames reach draft requests only;
   an export after draft playback is bit-exact with ffmpeg).

Not done: AV1 row-level frame threading (references are waited for as whole frames) and its
per-edge deblocking; VP9 and HEVC coefficient / CABAC parsing are now the largest single costs
(~27 %); HEVC reference windows are still copied per block (~11 %).

## Results (M4.9: 4K H.264, before → after)

Baseline = commit `3d14099` (M3.12, unchanged decoder); after = M4.9. Interleaved runs on
2026-10-02/03, load average 92–178 (the playback rows say which load each run saw).

### Decoder work per frame (load-independent)

`h264dec` example, one decoder, CPU cycles and instructions counted by the kernel (all threads),
output bit-exact with ffmpeg:

| stream | Mcycles / frame before | after | after, draft mode |
|---|---|---|---|
| 2160p (`a2160.mp4`, 170 Mbit/s, first 48 frames) | 505 | **390** (−23 %) | **356** (−30 %) |
| 1080p (`dec_h264_1080.mp4`, 120 frames) | 132 | **100** (−24 %) | **90** (−32 %) |

The cycle count is the same with 1, 4 or 14 frame threads (no spinning or contention overhead):
at ~4.5 GHz a 4K frame is ~85 ms of one core, so 23.976 fps needs about two cores.

### Decode (every frame through the media stack)

| codec | size | CPU ms/frame before (2 rounds) | after (2 rounds) |
|---|---|---|---|
| H.264 | 1080p | 35.8, 39.5 | **33.0, 29.4** |
| H.264 | 2160p | 138.9, 152.1 | **125.7, 112.3** |

(A run at load ~190 before any change gave 41.6 / 160.2 ms; an intermediate build 32.2 / 118.3.)

### Cold seeks on the 4K fixture (12 random targets, GOP 250, fresh source per seek)

| mode | CPU ms per seek before | after | wall p50 ms before → after |
|---|---|---|---|
| full decode from the keyframe | 7529, 7418 | **5531, 5268** (−27 %) | 2757, 1848 → 924, 739 |
| keep 2 s (scrub) | 7488, 7373 | **5442, 5397** | 2408, 1995 → 872, 795 |
| late (playback catch-up) | 5597, 5487 | **4079, 3949** | 2014, 1248 → 735, 526 |

Samples decoded per seek are identical (~48, ~34 in catch-up): the same pictures, decoded with
less work. The cold seek is a chain of reference pictures, so it shows the per-picture cost
directly.

### Program-monitor playback, 2160p (8 s, GPU path)

| case | before: shown/dropped (load) | after: shown/dropped (load) | CPU ms/frame before → after |
|---|---|---|---|
| Full | 192/0 (92), 9/183 (167) | 113/79 (171), **192/0** (139) | 159 (no skips), 133 (17 skipped) → 131, 130 |
| 1/2 | 12/180 (95), 5/187 (169) | 173/19 (172), **192/0** (124) | 143, 127 → 133, 132 |
| 1/2 draft | – | 183/9 (175), **192/0** (119) | – → 144, 134 |
| 1/4 | – | 186/6 (177), **192/0** (111) | – → 134, 125 |
| 1/4 draft | – | **192/0** (178), 190/2 (101) | – → 132, 131 |
| 1080p Full | 192/0, 192/0 | 192/0, 192/0 | 41, 48 → 39, 36 |

Before, the 4K clip kept up in one run (Full at load 92; 1/2 dropped 94 % at load 95) and dropped
95–97 % at load ~168. After, every 4K case plays 192/0 at load 100–140; at load 171–178 Full drops
41 %, 1/2 10 % (5 % with draft decoding), 1/4 3 % (none with draft decoding). CPU per
displayed frame (whole process: decode, frame workers, compositing) drops ~15 % where no frames
were skipped; draft decoding at 1/2 and 1/4 is within run-to-run noise in this column (its saving
is the ~9 % of decoder cycles above, plus a quarter / sixteenth of the texture upload), while the
frame jobs wait for the decoder less (decode ms/job 212 → 67 at 1/2, 98 → 44 at 1/4 in the busy
round). On an idle machine the 4K clip needs about two cores.

## What M4.9 changed

1. **Deblocking** (25–30 % of 4K decode time before): the edge filter runs on all 16 luma / 8
   chroma lines of an edge at once with per-lane masks and min/max clipping (no `clamp`, whose
   bounds assert blocked vectorisation), so it compiles to NEON; edges whose alpha or beta index
   is 0 are skipped. Bit-exact with the per-line filter (random-edge unit test) and ffmpeg.
2. **CABAC**: `decode_decision` / `decode_bypass` select instead of branching on the bin value
   (the MPS/LPS outcome is close to random at high bit rates), renormalisation has no branch on
   the range, and context / state indexing has no bounds checks.
3. **Inverse transforms**: the 4x4 / 8x8 column pass and the add/clip run across all columns.
4. **Frame threading** was already in place (M2): pictures decode concurrently and wait per
   macroblock row for the reference rows their motion vectors reach, with deblocking pipelined row
   by row inside each picture's job. The conformance suite now decodes every fixture with 1, 3 and
   all threads (bit-exact each time). A separate deblocking thread was not added: a picture's rows
   are published about two rows after they are reconstructed, so a reference chain advances a few
   rows behind its predecessor and the pool already has more pictures in flight than cores
   (crates/h264/README.md, "Threading model").
5. **Draft decoding** (Settings ▸ Playback ▸ Draft decoding, off by default; `perf.stats`
   `playback.draftDecode`, `decode.draftFrames`, `decode.h264Threads`): while the Program monitor
   plays at 1/2 or 1/4, H.264 non-reference pictures skip deblocking (no other picture can change),
   and draft plans hand the GPU box-decimated Y'CbCr planes at the drawn size. Draft frames carry
   their own frame-cache keys and the GOP cache serves them to draft requests only, so pausing,
   rendering and exporting always decode exact pictures (tested through the media stack against
   ffmpeg after draft playback). The CPU renderer already converted reduced-resolution frames
   straight to the decimated size (`to_linear_f32_decimated`).

Not done: CABAC residual decoding (now 41 % of the work, ~19 cycles per bin) is inherently serial;
the remaining deblocking cost is mostly the vertical-edge transposes; motion compensation and the
per-row / per-picture copies (4 %) are unchanged.

## Results (M4.8, before → after)

Baseline = commit `457aca1` (the harness on unchanged code); after = M4.8. Runs alternated base /
after (2 rounds each, 2026-10-02).

### Decode (every frame through the media stack: container, GOP cache, decoder, conversion)

CPU ms per frame (both rounds; lower is better). fps is the best of the run at that moment's load.

| codec | size | CPU ms/frame before | after | fps before → after (same-round pairs) |
|---|---|---|---|---|
| H.264 | 1080p | 43.4 / 43.7 | 48.2 / 46.1 | 101, 105 → 51, 34 (load 99 → 127) |
| H.264 | 2160p | 169 / 166 | 171 / 178 | 17, 31 → 16, 9 |
| **HEVC** | 1080p | 80.5 / 76.4 | **31.4 / 34.5** | 25, 58 → 69, 41 |
| **HEVC** | 2160p | 325 / 314 | **122 / 136** | 8.2, 12 → 17, 11 |
| VP9 | 1080p / 2160p | 39.7 / 137.6 | 36.5 / 136.8 | 18 / 12 → 18 / 7.8 |
| AV1 | 1080p / 2160p | 20.2 / 78.2 | 19.7 / 72.6 | 106 / 23 → 50 / 9.4 |
| ProRes 422 HQ | 1080p / 2160p | 23.6 / 89.0 | 23.2 / 86.7 | 59 / 28 → 34 / 15 |

On a quieter moment earlier in the session (load ~70) the same binaries decoded H.264 at 100 fps
(1080p) / 24 fps (2160p), AV1 131 / 44 fps and ProRes 202 / 49 fps.

### Seeking (cold seek through the media stack, 12 random targets, modes interleaved, load ~150)

| fixture (GOP 250) | full decode from keyframe | skip non-reference > 2 s before (scrub) | skip all late non-reference (playback catch-up) |
|---|---|---|---|
| H.264 1080p | 1405 ms, 3.3 s CPU | 1276 ms, 3.1 s CPU | **789 ms, 2.3 s CPU** |
| H.264 2160p | 3394 ms, 8.5 s CPU | 2828 ms, 8.4 s CPU | **2274 ms, 6.3 s CPU** |
| HEVC 2160p | 4091 ms, 17.8 s CPU | 3058 ms, 12.3 s CPU | **2512 ms, 9.8 s CPU** |

The target frame is identical in every mode (only pictures nothing references are left out).

### Program-monitor playback (8 s, GPU path)

| scenario | shown/dropped before | after | CPU ms/frame before → after | non-ref samples skipped |
|---|---|---|---|---|
| H.264 1080p | 192/0, 192/0 | 191/1, 191/1 | 55 → 55 | 0 (keeps up) |
| 3 × 1080p stacked | 192/0, 190/2 | 188/4, 185/7 | 120 → 128 | 0 |
| H.264 2160p Full | 22/170, 192/0 | 0/192, 0/192 | 183 → **87** | 39–52 |
| H.264 2160p Half | 180/12, 192/0 | 0/192, 0/192 | 198 → **102** | 46–49 |
| HEVC 2160p Full | 0/192, 0/192 | 0/192, 0/192 | 164 → **90** | 88 |
| HEVC 2160p Half | 0/192, 0/192 | 0/192, 0/192 | 160 → **97** | 33–92 |

Shown/dropped at 2160p is decided by the load at the moment (base round 2 happened to play 192/0
at a quiet minute; a later three-way run at load 175–195 gave 0–21 shown for every binary, and
143/49 for HEVC once). CPU per displayed second halved at 2160p: frames that are already late are
no longer fully decoded, and HEVC decodes 2.5× cheaper.

### Scrubbing (24 random seeks + 8 playhead drags through the monitor's request path)

| scenario | seek p50 / p95 ms before | after | drag-stop p50 ms before → after |
|---|---|---|---|
| H.264 1080p | 357 / 940, 582 / 1535 | 840 / 1567, 770 / 1389 | 27, 62 → 281, 103 |
| H.264 2160p | 1063 / 2377, 1499 / 3028 | 2445 / 5357, 2390 / 4562 | 288, 513 → 560, 834 |
| HEVC 2160p | 130 / 4100 (9 timeouts), 318 / 1397 | 2466 / 6441, 2476 / 7596 (0 timeouts) | 9, 3872 → 462, 1893 |

These runs coincided with load rising from 99 to 170 for "after". A later interleaved three-way
run (base / after with catch-up disabled / after) at load 80–200 gave H.264 1080p seek p50 345 /
712 / 572 ms then 379 / 389 / 461 ms, and H.264 2160p 913 / 1312 / 2105 then 1946 / 931 / 840 ms:
within the noise. The cold-seek table above is the load-independent comparison.

### Timeline UI (1000 clips on 20 tracks, 1920×1080 window, `egui_kittest`)

| view | update p50 / p95 ms before | after | tessellate p50 ms | wgpu render p50 ms (incl. readback) | vertices |
|---|---|---|---|---|---|
| fit (all 1000 clips) | 3.1 / 4.9, 3.4 / 5.7 | 2.9 / 4.5, 2.9 / 3.3 | 0.3 | 15 | 35.7 k |
| zoomed, scrolling | 2.8 / 3.8, 2.8 / 4.8 | 2.9 / 4.9, 2.6 / 6.8 | 0.2 | 15 | 29.2 k |

The timeline is not a bottleneck: the whole app frame (all panels) costs ~3 ms with 1000 clips
visible; it already culls clips outside the view. The waveform display gain no longer rescans the
whole source's peaks for every clip every frame (cached per peak list).

### Project save / open (5 sequences × 1000 clips, 3.7 MB)

save 24 / 68 → 36 / 21 ms, open 26 / 82 → 21 / 19 ms (p50 of 3; noise).

### Peak RSS per section (MB)

decode 2642 / 2925 → 2506 / 2064; playback 5246 / 7903 → 3379 / 3121; scrub 4205 / 5022 →
5390 / 4183; timeline 626 / 650 → 660 / 675; export 2367 / 2286 → 2606 / 2619; project 114 / 136 →
124 / 133.

## What M4.8 changed

1. **HEVC inverse transform** (largest hotspot: >50 % of HEVC decode CPU). Sums contiguous basis
   rows scaled by non-zero coefficients, size as a const generic so loops vectorise; bit-exact
   (exact integer sums), tested against the direct matrix product on random blocks and by the
   ffmpeg conformance fixtures. **SAO edge offset** gets a check-free inner loop. HEVC CPU per
   frame −60 %.
2. **No redundant plane copies**: H.264 / HEVC / VP9 pictures are moved into `VideoFrame`s instead
   of copied (12 MB per 2160p frame; HEVC 10-bit also lost a per-sample widening pass).
3. **Catch-up decoding**: frame jobs carry how far before their frame the playhead is
   (`waltcut_media::cancel::with_catch_up`); the GOP cache skips non-reference samples of late
   frames (`VideoDecoder::is_disposable`: H.264 `nal_ref_idc` 0, HEVC sub-layer non-reference
   pictures of the top temporal layer). Scrub requests keep the last 2 s fully decoded so dragging
   back stays cached. The wanted frame is decoded exactly as before; a later request for a skipped
   frame re-seeks. ~50 % of the fixtures' pictures are non-reference.
4. **`perf.stats`** (engine query + UI/control method): decode counters (cache hit rate, seeks,
   samples decoded / skipped, decoder ms), playback shown / dropped, frame-worker decode / render
   ms (p50 / p95), request hit rate, cache use, UI fps, process CPU.
5. **Timeline**: waveform source peak cached.

Not feasible / not done: reduced-resolution decode for H.264 / HEVC (inter prediction needs
full-resolution references, so it cannot be bit-exact; ½/¼ playback already decimates after
decode); AV1 internals untouched (measured only); export encoders not optimised.
