# Drawing and 2D animation

WaltCut's drawing layer is a transparent, frame-sized canvas over the video tracks. It stores
pressure-sampled strokes in cels; each cel stays visible until the next cel starts. This lets you
hold a pose for several frames or place a new cel on every frame.

## Draw on a sequence

1. Put the playhead where the drawing should begin and choose **Graphics and Titles ▸ New Layer ▸
   New Drawing**. The drawing is placed on a video track above the picture.
2. Select the **Brush** tool (`Shift+P`). Draw directly in the Program monitor. The options bar
   offers **Pen**, **Pencil**, **Marker**, and **Eraser**, a colour and size, pressure input, cel
   mode, and onion-skin controls.
3. Move the playhead to another drawing frame. Choose **New Cel** for a blank pose or **Duplicate**
   to copy the held pose, then draw the changes. With **Draw on held cel**, strokes instead extend
   the cel already visible on that frame.
4. Enable **Onion skin** to show the neighbouring cels: the previous cel in red and the next in
   green.
5. To fill a gap between two cels, choose **In-between**. It interpolates matching strokes by
   resampling their points; adjust the spacing in the options bar (two frames by default). This is
   a straight stroke interpolation, not a semantic or AI-generated pose.

Drawing clips are ordinary video clips for timeline placement, so clip motion and opacity can be
used to position or fade a drawing.

## Brushes and input

- **Pen** uses pressure for width; **Pencil** uses pressure for width and opacity; **Marker** is
  broad and translucent; **Eraser** removes strokes in the current cel.
- A mouse uses full pressure. Pen pressure is used when the platform supplies it; the options bar
  shows the current input and pressure when available.
- The lower barrel button erases while drawing when the tablet driver maps it to the secondary
  mouse button.
- **Input samples.** On macOS every pointer event of a pen becomes one sample with its own
  position, pressure, monotonic event time and phase (down, move, up). A stroke is built from that
  ordered stream, so several positions with different pressures inside one UI frame keep their
  own pressure, the stroke ends exactly where the pen came up, and replaying the same samples
  gives the same stroke however they were batched into frames. Without a pen queue (a mouse, a
  headless run) the stroke is built from the UI's pointer events in the same way, at full pressure.
  Timestamps stay with the input samples (diagnostics and replay); a saved stroke keeps position
  and pressure.
- **Thinning.** A sample is kept when it is at least 0.75 screen points from the last kept one or
  its pressure differs by 0.02 or more; the sample that ends the stroke is always kept. A stroke
  holds at most 20 000 samples; past that the newest sample replaces the last (the end follows
  the pen) and the status bar says so. The pen queue holds 4 096 samples between frames; if it
  overflows the oldest moves are dropped (never the stroke's first and last samples) and the loss
  is counted.
- **What you see is what is drawn.** The live stroke, the stroke between pen-up and the arrival of
  the new frame, and the onion skins are rasterised by the same CPU code as the Program monitor
  and export (`waltcut_render::drawing`) with the same placement, so Pen, Pencil, Marker and
  Eraser have the same coverage and compositing before pen-up, after it and in export (tests
  compare them pixel for pixel, including under Motion scale, position and rotation). An
  overlapping translucent stroke (Marker) has no darker joints. Onion skins are the neighbouring
  cel *as drawn*: areas you erased stay empty. The one difference is the display blend of
  partially covered pixels (antialiased edges, Marker, Pencil): the preview is blended by the UI in
  gamma space and the engine in linear light, at most 20/255 apart on a 60 % Marker over mid grey
  (opaque pixels are identical). While you erase, the preview shows the erased footprint as a
  translucent white (the coverage the eraser will remove) rather than the picture beneath, which
  only the engine knows. A stroke stays visible after pen-up until the engine's frame containing it
  is on screen. The preview ignores the clip's opacity and blend mode.
- **Cancel.** Escape, or the window losing focus, abandons the stroke being drawn; nothing is
  committed and the status bar says so.
- **Diagnostics.** `ui.inspect` reports `ui.brush.pen`: the input source (`Pointer`, `Pen`, or
  `PenFallback` when the pen queue disagreed with the pointer by more than 12 points and was not
  used for that frame), samples received, thinned, queue-dropped and invalid, cancelled and
  truncated strokes, and the point count, duration and sampling rate of the last stroke. These
  numbers describe what WaltCut received from the driver, not how good the tablet is.

## AI Restyle

Open **Graphics and Titles ▸ AI Restyle…** to restyle the frame at the playhead, every Nth frame of
the sequence In/Out range, or every Nth frame of the selected clip. WaltCut renders the composed
frame—including its picture, graphics, and drawings—and sends it to the configured image provider.
Completed images are imported onto a new video track above the sequence.

The implemented provider is fal.ai cloud; it requires a fal.ai API key and a cloud-enabled build.
The image data leaves the machine when sent to the provider, and cloud requests may incur charges.
The Local Stable Diffusion option is not implemented yet. A fixed seed can help keep the look
consistent, but temporal consistency between generated frames is not guaranteed.

## Current scope

Drawings support raster-rendered Pen, Pencil, Marker, and Eraser strokes, held or duplicated cels,
onion skins, and straight in-between interpolation. This is a focused 2D drawing workflow, not a
full vector animation or rigging system.

## What WaltCut keeps

WaltCut is being reduced to the code this workflow needs (epic
[#14](https://github.com/HenryCorn/WaltCut/issues/14)):

> import footage, images and audio → arrange a simple timeline → draw and manage cels → preview →
> export

This section is the allowlist the reduction follows. Anything not listed here is removed from the
repository, not hidden or put behind a flag. The decisions were made in
[#17](https://github.com/HenryCorn/WaltCut/issues/17). Until the reduction is finished, the app
still contains the inherited WaltCut features.

### Workflows

Each workflow lists its main commands. CLI, MCP and the control channel expose only these
workflows' commands once the reduction is finished.

| Workflow | Main commands |
|---|---|
| Create, open, save and recover projects | `file.newProject`, `file.open`, `file.save`, `file.saveAs`, `file.autoSaveNow`, `file.recover`, `edit.undo`, `edit.redo` |
| Import reference media | `file.import`, `file.importImageSequence`, `edit.pasteMedia` |
| Organise media in bins | `file.newBin`, `project.moveToBin`, `project.renameBin` |
| Arrange a simple timeline | `file.newSequence`, `sequence.addEdit` (split), `edit.duplicate`, `edit.clear`, `edit.rippleDelete`, `clip.link`, `clip.audioGain`, track mute |
| Draw and manage cels | `drawing.new`, `drawing.newCel`, `drawing.addStroke`, `drawing.clearCel`, `drawing.deleteCel`, `drawing.tween`, onion skin in the Brush options |
| AI Restyle | `ai.setup`, `ai.status`, `ai.restyle` |
| Preview | `playback.toggle`, frame stepping, Program monitor zoom and resolution |
| Export | `file.exportMedia`, `file.exportFrame`, `export.quick` |

### Media formats

| | Kept |
|---|---|
| Containers | MP4/MOV, MKV/WebM, AVI, MPEG-TS/PS (`.mpg`, `.vob`, `.m2ts`), image sequences |
| Video decoding | H.264, HEVC (including iPhone 10-bit HDR), ProRes, VP9, AV1, MPEG-2 |
| Audio decoding | WAV/PCM, AAC, MP3, Opus, MP1/MP2 |
| Images | PNG, JPEG, WebP, TIFF, GIF, BMP |
| Export | H.264 + AAC in MP4, ProRes 4444 MOV with alpha, PNG sequence with alpha, still frame (PNG/JPEG) |

Removed: MXF and Ogg containers; DNxHD/HR, APV, VP8 and DNG/CinemaDNG video; AC-3/E-AC-3 audio
(MPEG-2 files with Dolby audio import without sound); FLAC export; broadcast and cinema delivery
presets.

### Kept from shared systems

- Keyframes for position, scale, rotation, anchor and opacity.
- Blend modes on drawing and reference clips.
- Timeline waveforms, audio scrubbing, track mute and clip volume, for timing to dialogue and
  music.
- Colour management needed to decode, display and export footage correctly (no grading).
- Text and shape graphic clips, with per-character styles and responsive layout, for titles and
  artwork.

### Removed

- Masks, tracking, adjustment layers, and the broad effects and transitions catalogue.
- Proxies, multicam, clip synchronisation, scene edit detection, professional ingest and the media
  browser.
- Audio post-production: mixers, sends, automation, Essential Sound, voice-over recording, Remix,
  surround and loudness tools.
- Captions, speech-to-text, transcripts and text-to-speech.
- Colour grading, scopes and reference monitoring.
- AAF, OMF, EDL, FCP XML/FCPXML and OTIO interchange.
- Graphics templates (`.fcgt`).
- UI translations: the interface is English only. Titles can still use other scripts.

### Platforms

macOS only, with Apple Silicon release builds. Windows, Linux, FreeBSD and the web version are
removed ([#29](https://github.com/HenryCorn/WaltCut/issues/29)).

### Project files

Projects created before the reduction don't open in later versions
([#18](https://github.com/HenryCorn/WaltCut/issues/18)). Open them in
[v0.0.1-legacy](https://github.com/HenryCorn/WaltCut/releases/tag/v0.0.1-legacy), the last build
from before the reduction. New projects use the `.waltcut` extension
([#2](https://github.com/HenryCorn/WaltCut/issues/2)).

### Size measurements

The size of the codebase before and after the reduction is measured on a GitHub Actions macOS
runner: crate count, direct dependencies, lines of Rust, clean release build time, binary size and
test time. The baseline is recorded in #17 and the final comparison in
[#27](https://github.com/HenryCorn/WaltCut/issues/27).
