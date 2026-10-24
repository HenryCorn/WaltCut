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
   the cel already visible on that frame. A cel holds from its start frame up to (but not including)
   the next cel start. The Timeline shows these exposures in the drawing clip; drag a start marker
   to retime its pose, or drag an exposure end to adjust the hold. Empty cels are explicit blank
   poses; clearing a cel keeps its start, while deleting it lets an earlier pose hold again.
   Click an exposure (Shift-click to extend) to pick it, then right-click it, or choose **Graphics and
   Titles ▸ Drawing ▸ Retime Cels on Ones / Twos / Threes**. In the right-click menu, hovering a choice
   marks in the lane where the cels would land (white marks; the grey line is where a cel stands now)
   and clicking applies it; Escape or clicking away changes nothing: the picked run of cels is re-spaced one, two or
   three frames apart from its first cel, every cel keeping its drawing. The last picked cel keeps
   its own hold and the cels after it move with it, so the rest of the animation keeps its timing;
   one undo step restores everything. With nothing picked the whole drawing is retimed. The
   command is `drawing.retimeExposures {frames, on|spacing, ripple?, dryRun?}` (`dryRun` returns the
   resulting cel starts without editing); a refusal (cels that do not
   follow each other, a pose that would land past the clip's end) changes nothing.
4. Enable **Onion skin** to show the neighbouring cels: the previous cel in red and the next in
   green (one of each by default). The **Onion** menu in the Brush options bar, or
   `drawing.onionSkin`, sets how many *distinct* cels show on each side (0–4), the opacity of the
   nearest, how much each farther ghost fades (`falloff`, a fraction of the one before) and how far
   ghosts take their tint rather than their own colours (`tint`, 1 = fully tinted).
   - A ghost is one *cel*, never one frame: a pose held for ten frames is one ghost, and a cel
     whose strokes equal the cel shown (or a nearer ghost) is passed over, since it would only sit
     on top of it. An explicit blank cel is a pose: it takes its place and shows nothing, so an
     older drawing does not stand in for it. Erased areas stay empty.
   - A ghost is never more than 60 % opaque, whatever is set, so it cannot hide the picture or
     the cel being drawn.
   - **Pin Pose for Onion Skin** (`drawing.pinPose`, again to unpin; `drawing.onionSkin {pin}`
     takes a cel's start frame, `"current"` or `null`) keeps one cel in blue wherever the
     playhead goes; the **First cel (loop start)** option (`loop`) shows the first cel in amber,
     to match a loop's end against its start. Neither appears twice: a pinned cel that is
     already a neighbour, the cel shown, or the loop start is shown once. A pin whose cel has
     been moved or deleted shows nothing until it is pinned again.
   - The onion skin is a view setting of the Brush tool, saved with the interface state: it
     adds no undo step, does not mark the project changed, never moves the playhead and is not
     part of any export. `ui.inspect` lists the ghosts being shown (`ghosts`: role, rank,
     cel start frame and opacity). Bad settings (a count over 4, an opacity outside 0–1, a pin on
     a frame with no cel) are refused whole.
5. Compare poses:
   **Previous Cel / Next Cel** (`Cmd+Alt+,` / `Cmd+Alt+.`, Graphics and Titles ▸ Drawing) move the
   playhead to the previous or next *drawing*: a held pose counts once, a cel trimmed off the clip
   does not count, one whose hold reaches into the clip does, and on a reversed clip "next" follows
   the timeline. Blank cels are poses (`skipBlank: true` passes over them). With no neighbour the
   command says so and the playhead stays.
   **Review** (hold `Cmd+Alt+Up` for the previous pose or `Cmd+Alt+Down` for the next, with any tool
   selected) shows that pose for as long as the key is down and puts the playhead and selection back
   on release, on Escape and when the window loses focus. It only moves the playhead: nothing is
   edited and there is no undo step. If a scrub or playback moves the playhead meanwhile, that move
   stands. Switching from one key to the other stays relative to the pose the review began on. The
   same behaviour is `drawing.reviewPrevious` / `drawing.reviewNext` (`{active?, skipBlank?}`, held
   until `drawing.reviewEnd`, `active: false`, Escape or focus loss when sent as a command), and
   `ui.inspect` reports `review`. A tablet's express keys or pen buttons can send the same chords
   through the tablet driver, which is how a pen-driven review loop is set up.
6. **Drawing view.** Turn the picture to draw at a comfortable angle (**View ▸ Drawing View**,
   Brush tool or not). It changes only how the Program monitor *shows* the picture: nothing is
   added to the project, there is no undo step, clip Motion and artwork are untouched, and no
   export changes. A stroke lands where the pen is: the pointer is mapped back through the view,
   the monitor zoom and the clip's Motion. The view is not saved and WaltCut always starts
   unturned.
   - `view.canvasRotateRight` / `view.canvasRotateLeft` (`Alt+.` / `Alt+,`, `{degrees?}`, 15° by
     default) and `view.canvasRotate {degrees}` turn it clockwise-positive, wrapped into
     (-180, 180]; `view.canvasMirror` (`Alt+M`, `{enabled?}`) flips it left to right before
     the rotation; `view.canvasReset` (`Alt+0`) returns to the default: 0°, not mirrored, Fit,
     centred. A turned picture is fitted so all of it stays visible.
   - **Pan While Held** (`Alt+H`, `view.canvasPan {active?}`) pans a magnified picture with a
     pen or mouse drag and never draws: held with the key, it lasts as long as the key is down;
     as a command, until `{active: false}`. It also ends on Escape, when the window loses focus
     and when another tool is chosen. (The Hand tool and a middle-button drag still pan.)
   - **Modifier ownership.** `Alt` + a letter is a held drawing aid (`Alt+E` erase, `Alt+H`
     pan), `Cmd+Alt` + an arrow is pose review. A reference placed under the drawing is moved
     with its own controls, not with a plain drag on the picture, so it cannot compete with pan.
   - While the picture is turned or mirrored, safe margins, rulers, guides, the Graphics and
     Transform handles and the eyedropper are not drawn, because they assume an upright picture;
     reset the view to use them. `ui.inspect` reports `ui.program.canvas` (`rotation`,
     `mirror`, `panHeld`).
7. To fill a gap between two cels, choose **In-between**. It interpolates matching strokes by
   resampling their points; adjust the spacing in the options bar (two frames by default). This is
   a straight stroke interpolation, not a semantic or AI-generated pose.

## Guide clips

A **guide** is a reference or tracing clip (a placed picture, a drawing, a graphic) that you see
while you work but that never ends up in what you deliver. Choose **Clip ▸ Use as Guide**
(`clip.setGuide {clips?, guide?}`; it flips the selected clips when `guide` is absent). A guide
clip is a video clip on an unlocked track; audio clips, locked tracks and missing clips are
refused and nothing changes. It is one undo step, and the Timeline marks the clip with a
**guide** badge. A guide is not the same as *disabled* (drawn nowhere), *muted* or *locked*, and a
split keeps it a guide on both halves.

| Output | Guides |
|---|---|
| Program monitor, render-cache previews, `render_frame` | **shown** |
| Still (`file.exportFrame`), PNG sequence and movie (`file.exportMedia`, `export.quick`) | **left out** unless `includeGuides: true` (the Export page's **Include guide clips** switch, off each time) |
| AI Restyle input (`ai.restyle`) | **left out** unless `includeGuides: true`: the provider is sent the picture a delivery render would give |

This holds inside nested sequences too, and is the same for the app, the CLI and MCP, because
they all run these commands. With no `includeGuides` a guided project exports exactly like the
same project with the guide clips removed (tests compare PNG frames and ProRes movies byte for
byte). The renderer's own default is "no guides" (`RenderOptions::guides`): only the Program
preview asks for them, so a render nobody thought about cannot carry one.

Saved projects keep the flag as `guide: true` on the clip, written only when set: a project
without guides is byte for byte what it was, and the schema version is unchanged. A build that
predates guides refuses a file that has one (unknown field) rather than dropping the flag. Placing
a picture to trace is covered by the reference-image work (#3); this is the policy it builds on.

Drawing clips are ordinary video clips for timeline placement, so clip motion and opacity can be
used to position or fade a drawing.

## Brushes and input

- **Pen** uses pressure for width; **Pencil** uses pressure for width and opacity; **Marker** is
  broad and translucent; **Eraser** removes strokes in the current cel.
- A mouse uses full pressure. Pen pressure is used when the platform supplies it; the options bar
  shows the current input and pressure when available.
- The lower barrel button erases while drawing when the tablet driver maps it to the secondary
  mouse button.
- **Brush size and temporary erase** (Brush tool selected; **Graphics and Titles ▸ Drawing**, and
  the same command ids over the control channel and MCP):
  - `drawing.brushSizeUp` (`Alt+]`) and `drawing.brushSizeDown` (`Alt+[`), parameter `steps`
    (1–100, default 1), step through 1, 2, 3, 4, 5, 6, 8, 10, 12 … 160, 200 px and stop at both
    ends (the reply says `atLimit`). They are listed with their parameters in `ui.menu.list` and
    are disabled when another tool is selected. A tablet driver can map an express key or dial
    to these chords.
  - `drawing.eraseTemporarily` (`Alt+E`): strokes erase while the key is held, whichever brush
    is selected; the selected brush and all its settings are never changed, so letting go
    brings back the same Pen, Pencil or Marker. Over the control channel
    `{"active": true}` erases until `{"active": false}`. The hold also ends when the window
    loses focus (a stroke in progress is abandoned, as with Escape) or another tool is
    chosen. A stroke already begun keeps the mode it began in. A driver that sends the key as a
    quick tap instead of holding it gives no erasing.
  - The keys do not collide with anything in the default map: `[` and `]` remain the
    clip-volume keys, `E` remains Extend Edit to Playhead, and `ui.brush.erase_held` in
    `ui.inspect` shows an active hold.
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
| Draw and manage cels | `drawing.new`, `drawing.newCel`, `drawing.retimeCel`, `drawing.setHold`, `drawing.retimeExposures`, `drawing.previousCel`, `drawing.nextCel`, `drawing.reviewPrevious`, `drawing.reviewNext`, `drawing.addStroke`, `drawing.clearCel`, `drawing.deleteCel`, `drawing.tween`, `clip.setGuide`, `drawing.onionSkin`, `drawing.pinPose`, the Drawing View commands `view.canvasRotate`, `view.canvasRotateRight`, `view.canvasRotateLeft`, `view.canvasMirror`, `view.canvasPan`, `view.canvasReset` |
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
