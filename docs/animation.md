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
     pan, `Alt+T` shift the pinned pose), `Cmd+Alt` + an arrow is pose review. A reference placed
     under the drawing is moved with its own controls, not with a plain drag on the picture, so it
     cannot compete with pan.
   - While the picture is turned or mirrored, safe margins, rulers, guides, the Graphics and
     Transform handles and the eyedropper are not drawn, because they assume an upright picture;
     reset the view to use them. `ui.inspect` reports `ui.program.canvas` (`rotation`,
     `mirror`, `panHeld`).
7. **Light table.** Slide and turn the *pinned* comparison pose to check proportions against the
   cel you are drawing (for a character that travels across the shot, line the pose up with where
   it now stands). Pin a pose first (**Pin Pose for Onion Skin**), then tick **Light table ▸
   Shift pose with pen** in the Brush options bar (`Alt+T`, held, or `drawing.lightTableShift`). While it is on, a drag on the
   picture moves the pose with the pen, **Shift**-drag turns it about the middle of the canvas, and
   nothing is drawn; let go, untick it or choose another tool and the pose stays where you put it
   while you draw. The same menu sets **X**, **Y** and **Rotate °** exactly;
   a **Pose shifted** (or **Shifting pose**, with **Done**) badge, a dashed outline of where the pose now is and a note under the options
   bar show that the ghost is not where it was drawn, and **Reset pose** (`Alt+Shift+T`,
   `drawing.lightTableReset`) puts it back.
   - It is a view of the pose, like the onion skin: the source cel, its exposure, the clip, the
     undo history, the project's changed mark and the saved file are untouched, and no export, still
     or AI Restyle input includes it (tests compare the project, the undo history, the revision and
     the rendered frame before and after). It is not saved and WaltCut starts unshifted.
   - The shift belongs to the pinned cel. It is moved with the pen through the same mapping as a
     stroke (view rotation and mirror, zoom, Motion), so the pose follows the pen in a turned view,
     and the current cel stays drawable and lands where the pen is. A pinned cel that is also the
     previous or next cel is the ghost that moves; when it is the cel shown (or hidden behind a
     nearer cel with the same strokes, or Onion skin is off) there is nothing to shift on screen and
     the note says so.
   - **Exits.** Letting go of the key, `{active: false}`, unticking or choosing another tool ends
     the hold and *keeps* the alignment. **Escape** or the window losing focus *during* a hold
     abandons the hold: the alignment goes back to what it was when the hold began (a drag in
     progress is dropped with it). **Reset**, unpinning, pinning another cel, switching to a
     different drawing clip and deleting the pinned cel *discard* the alignment. Moving the
     playhead, drawing, undo and turning Onion skin off keep it.
   - `drawing.lightTable {dx?, dy?, rotation?, reset?}` (canvas pixels within ±1 000 000, degrees
     wrapped into (-180, 180]; all values are checked before any is applied; with no parameter it
     reports `{pinned, pose, dx, dy, rotation, shifted, hold, showing}`), `drawing.lightTableShift
     {active?}` and `drawing.lightTableReset`. A shift with no pinned pose, a bad number or an
     unknown parameter is refused and changes nothing. `ui.inspect` reports `ui.brush.light_table`.
     Automation ids: `program.brush.light.menu`, and in it `.shift`, `.dx`, `.dy`, `.rotation`,
     `.reset`; while shifting or shifted `.badge`, `.done`, `.resetBadge`.
8. To fill a gap between two cels, choose **In-between**. It interpolates matching strokes by
   resampling their points; adjust the spacing in the options bar (two frames by default). This is
   a straight stroke interpolation, not a semantic or AI-generated pose.

## Lasso-select and transform strokes

Correct a misplaced or oversized part of a cel without erasing and redrawing it. With the Brush
tool, turn on **Lasso** in the options bar (`drawing.lassoMode`, `Alt+L`, Graphics and Titles ▸
Drawing). While it is on nothing is drawn:

- A drag that starts outside the selection draws a **lasso**; letting go selects the *paint
  strokes of the cel shown* with **more than half of their points** inside it
  (`drawing.selectStrokes {lasso}`, canvas pixels; `{clear: true}` drops it). Eraser strokes are
  never lassoed directly.
- The selection is highlighted in blue. A drag that starts inside its box moves the **pending
  transform**; **X**, **Y**, **Rotate °** (clockwise) and **Scale ×** (uniform, the brush size
  scales with it) in the options bar set it exactly (`drawing.selectionTransform {dx, dy,
  rotation, scale}`), about the selection's centre. The result is previewed in orange over the
  picture; no artwork changes yet.
- **Apply** (button, `Alt+Enter`, `drawing.selectionApply`) is one undo step and changes only that
  cel's strokes (clip Motion is untouched); **Cancel** drops the pending transform; **Escape**
  drops the lasso being drawn, then the pending transform, then the selection. The selection is
  also dropped when the project changes under it, and none of this state is saved.

**Erasers.** An eraser stroke removes paint from the strokes before it where it passes, so the
rules keep it from revealing or removing anything by surprise:

- An eraser that comes after a selected stroke and overlaps it **belongs to the selection** and
  moves with it (the erased gap travels with the paint).
- An eraser that overlaps both selected and unselected paint is **ambiguous**: the selection says
  so and the transform is refused until those strokes are selected too (or you erase again).
- A transform is refused when, after it, a moved eraser would overlap unselected paint before it,
  or moved paint would land under an unselected eraser after it. Stroke order never changes, and
  overlap is judged by stroke bounds including the brush radius, so a refusal can be stricter than
  the pixels need but never looser.
- Locked tracks, a missing cel, an invalid selection, non-finite or extreme numbers (scale outside
  0.01–100, coordinates beyond ±1 000 000 px, a brush size outside 0.05–1000) and bad parameters
  are refused with the reason and change nothing.

Scripts use the engine commands directly: `drawing.lassoSelect {lasso}` (read only: the strokes
hit, the erasers that belong to them, `conflict`, centre) and `drawing.transformStrokes {strokes |
lasso, dx, dy, rotation, scale, pivot, dryRun}`, which the UI's Apply calls. Not in this version:
point editing, deformation, editing several cels at once, recolouring and thickness changes.

## Fill a region

Colour a closed shape without drawing the colour in by hand: with the Brush tool, turn on **Fill**
in the options bar (`drawing.fillMode {enabled?, tolerance?}`, `Alt+F`, Graphics and Titles ▸
Drawing) and **click** inside the shape. A click fills the contiguous region around it on the cel
shown with the brush colour, as **one undo step**; a drag in Fill mode draws nothing, and a click
on the options bar is not a click on the picture. The command the click runs is `drawing.fill
{point | seed, tolerance?, color?, cel?}` (`point` is canvas coordinates, `seed` a whole pixel;
`cel` is `hold`, `new` or `copy` as for strokes, and the region is found in the paint of the cel
the fill is added to).

- **Tolerance** (slider, `drawing.fillMode {tolerance}`, 0–255, default 32) is how different a
  pixel may be and still join the region; 0 is an exact match. It is saved with the interface
  state; the mode is not (a session starts drawing). Fill and Lasso exclude each other.
- **Recolour last** and **Remove last** (shown in Fill mode) recolour the brush colour onto, or
  remove, the newest fill of the cel shown: `drawing.fillEdit` and `drawing.fillRemove`, below.
- Automation ids: `program.brush.fill`, `program.brush.fill.tolerance`,
  `program.brush.fill.recolor`, `program.brush.fill.remove`; `ui.inspect` reports
  `ui.brush.fill` and `ui.brush.fill_tolerance`.

### Decision: a fill is a stored operation in the cel's paint order (#68)

A cel is an ordered list of operations, and an eraser is already one of them. A fill is another:
a `Stroke` with `brush: Fill` whose `fill` field holds the **result**, and no points. The other
option, baking the result into ordinary strokes, was rejected: a stroke is a round-capped polyline,
so a concave region with holes would need a stroke per scan line (a few fills would pass the
5 000-stroke cap), its edges would show seams, and the seed, tolerance and colour would be lost, so
nothing could be edited afterwards. Storing only the seed and recomputing at render time was
rejected too: it would run a flood fill per fill per frame, and a fill would change by itself when
a line under it is edited or the rasteriser changes, which breaks "preview and export agree" over
time. Storing the result makes rendering cheap and exact and keeps the fill what it was when you
made it.

What is stored (`Stroke.fill`, schema v19, see [project-files.md](project-files.md#fill-operations)):

- `seed` (the clicked canvas pixel), `tolerance` (0–255) and the stroke's `color`, so the fill can be
  recoloured or redone;
- `rect` (`x, y, w, h`, canvas pixels inside the canvas) and `runs`: the result as a run-length
  mask over `rect`, row by row, each run a class (0 outside, 1 filled, 2 **edge**) and a length.

**Bounds.** A canvas of more than 8 847 360 pixels (4096 × 2160) is not filled and no fill is
accepted from a file for it. A fill holds at most 65 536 runs, a cel 262 144 and a drawing
2 097 152 (about 20 MB of project file at the very most); a region too ragged for that is refused
with the number of runs it needed. The work is bounded by the canvas size, and the flood fill checks a cancel
callback and a 5 s time budget as it goes, so a runaway stops with an error and changes nothing.
The app has no Cancel button for it: the fill runs when you click and returns within the budget
(measured times are in the pull request for #68).

### What a fill does

- **What it looks at.** Only the cel's own paint shown at that moment: its strokes, erasers and
  earlier fills as rendered at full size. The video, references and other cels are not looked at
  (filling from a reference layer is #69).
- **Similar pixels.** The pixel under the seed is the target. A pixel is similar when every one of
  its premultiplied linear red, green, blue and alpha values (0–255) is within `tolerance` of the
  target's (default 32). The region is the 4-connected (not diagonal) set of similar pixels around
  the seed, so a one-pixel diagonal line closes a shape.
- **Seeds.** The seed must be inside the canvas: a point outside it is refused, not clamped.
  A seed on an empty (transparent) pixel fills the empty region it is in, up to the canvas edge:
  an open shape fills everything it leaks into, including the whole canvas. A seed on paint fills
  the contiguous paint of that colour (it recolours a line or an earlier fill). A fully
  transparent fill colour is refused (it would change nothing).
- **Edge pixels.** The one-pixel shell around the region (pixels that are not similar but touch
  it: the soft edge of a line) is stored as class 2 and painted *behind* what is already there,
  so the colour reaches the line without a gap and never covers it. Filled pixels (class 1) are
  painted over what is there like any stroke, with the colour's alpha. Wider antialiasing, gap closing and growing the
  fill are #69.
- **Order.** A fill is painted where it stands in the cel, so later strokes paint over it and a
  later eraser removes it (an eraser before it does not). Erasing a fill leaves its stored region
  alone; the paint under an eraser's pass is simply not shown.
- **Editing a cel afterwards.** The result is saved, not live: drawing or erasing under a fill, moving
  strokes with the lasso, or deleting them does not change it. To redo it, use `drawing.fillEdit
  {index, tolerance}` (finds the region again from the same seed against the paint *before* the fill
  in the cel, so it follows corrections made to the lines under it), recolour it with
  `drawing.fillEdit {index, color}` (no new region), or remove it with `drawing.fillRemove
  {index}`. `index` is the fill's position in the cel (`drawing.inspect {strokes: true}` lists
  it). Each is one undo step and changes nothing when refused.
- **Cels.** Duplicate copies fills with the strokes. In-betweens never morph a
  fill: at each place where a cel has a fill, the nearer cel's operation is shown. The lasso never
  selects a fill (it has no points), a stroke selection is refused if a transform would make an
  eraser reach a fill, and Clear Cel removes fills with the strokes. Onion skins show fills.
- **Same pixels.** Preview, stills, PNG sequences and movies all render the cel through
  `waltcut_render::drawing::render_strokes`, so they agree pixel for pixel. A fill samples its mask at
  the pixel centres of the output with bilinear coverage: at the drawing's own size and placement
  that is exact (whole-pixel steps, no blur), under Motion scale or rotation the edge is as sharp
  as one canvas pixel. The flood fill itself is integer maths on 8-bit values over the rasterised
  cel, so the same cel gives the same region on every run of a build.
- **Failures.** A locked track, a missing cel, a seed outside the canvas, an oversized canvas, too
  many runs, a bad colour or tolerance, an unknown parameter, a cancelled or too long fill and a
  cel that already holds 5 000 strokes are refused with the reason. The cel and the undo history
  are untouched.

## Reference images

To trace a picture, use the **Reference** menu in the Brush options bar (or **Graphics and Titles ▸
Drawing ▸ Place Reference Image / Paste Reference Image**, `reference.place`,
`reference.pasteImage`):

- **Place Image…** (the host's file picker) or **Paste Image** (the picture on the clipboard, saved
  the way Paste Media saves it) puts the picture on a video track named **Reference** directly
  below the target drawing, for the drawing clip's whole duration, at **50 % opacity** (the
  scaling follows Preferences ▸ Media). The playhead and selection stay where they were. It is
  **one undo step**, and all or nothing: a locked drawing track or Reference track, a missing or
  unreadable picture or a bad parameter change nothing (no import is left behind). A free,
  unlocked Reference track right below the drawing is reused; otherwise a new one is added there.
  `reference.place {path | item, target?, opacity?, guide?}` also takes a picture already in the
  project (`item`).
- The Reference menu then controls the drawing's reference without leaving the drawing:
  **Opacity**, **Scale** and **Rotation** sliders, **Hide**, **Lock** (locks the Reference track:
  nothing else changes it), **Guide only (not exported)**, four **Nudge** buttons (10 sequence
  pixels each) and **Move with pen**. With Move with pen on, a drag on the Program monitor moves
  the reference instead of drawing (the cursor is a hand, the status text says so, and the
  mapping goes through the view and zoom, so it follows the pen in a turned view); one drag is one
  undo step. It is never remembered: a session starts drawing.
  The same changes are `reference.set {clip?, target?, opacity?, hidden?, locked?, guide?, x?, y?, dx?,
  dy?, scale?, rotation?, merge?}` (all values are checked before any is applied; send
  `merge: "start"` with the first move of a drag and `merge: true` with the rest to make it one undo
  step) and `reference.list {target?}` names the references (identity, track, settings) for scripts.
- **A reference is ordinary artwork.** A visible reference is in the Program monitor and in
  stills, PNG sequences and movies; **Hide** takes it out of both. To see it while keeping it out of
  what you deliver, turn on **Guide only**: that is the guide policy below. Nothing here changes
  how the drawing itself is stored or exported.
- A reference is found by being on a `Reference` track below the drawing and overlapping it; the
  clip, its Motion and Opacity are the usual ones (the Effect Controls edit them too) and are saved
  with the project like any clip.

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
- **Smoothing** (`drawing.smoothing {mode?, strength?, pressure?}`, **Feel ▸ Smooth strokes** in the
  Brush options bar, Graphics and Titles ▸ Drawing ▸ Stroke Smoothing). **Off** (the default) stores the
  captured samples exactly as before. **Smooth** passes each kept sample through an adaptive moving
  average, one at a time and in order (so the result does not depend on how samples were batched
  into frames), in canvas pixels:
  - *Position* strength 0–1 (`alpha = 1 - 0.9 × strength`): small moves are absorbed, and the
    filter follows the pen more closely the faster it moves (the follow rises with the square of
    the pen's step, reaching full at 16 canvas pixels per sample). On a synthetic ±2 px zigzag
    drawn in 3 px steps the wobble falls from 2.0 px to 1.22 px at strength 0.3 and to 0.31 px at
    0.9; a 90° corner drawn in 8 px steps is rounded by 0.4, 2.2 and 7 px at strengths 0.3, 0.6
    and 1 (all measured by the unit tests). *Pressure* has its own strength (`alpha = 1 - 0.8 ×
    pressure`) and never moves the position.
  - The smoothed line **trails the pen** while you draw. That lag is intentional: the menu says so
    and the **Feel** button turns blue while smoothing or a curve is in use.
  - **Pen-up** ends the stroke exactly where the pen lifted: the smoothed point first catches up
    along the filter's own curve (at most 32 extra points), then the true end point is stored, so
    no straight segment to the cursor is added beyond the last quarter pixel. A tap stays one
    point. **Escape** and focus loss drop the stroke before anything is committed, with or without
    smoothing. A mouse is smoothed the same way. The setting is a view of how the *next* strokes
    are made: it adds no undo step, changes no stored stroke and is saved with the interface state.
  - Not in this version: a trailing (rope) stabilizer, and smoothing already drawn strokes. Bad
    values (a mode other than `off` / `smooth`, a strength outside 0–1, unknown parameters) are
    refused whole. The filter costs 8 ns per sample on the development machine (measured); the
    delay between pen and screen was not measured (no camera rig), only the lag above.
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

## Pressure response curves

A pen's pressure becomes line width and opacity through the brush's own response (a Pen thins to a
hairline at a light touch, a Pencil also fades). To tune light touch, taper and opacity, open
**Feel** in the Brush options bar (`drawing.pressureCurve {preset?, width?, opacity?, reset?}`,
Graphics and Titles ▸ Drawing ▸ Pressure Response Curves):

- **Presets** *Linear* (the brush's own response, the default), *Soft* (a light touch already
  leaves a good line), *Firm* (needs a harder press) and *S-curve* (soft at both ends).
- **Width** and **Opacity** have their own curve and editor: five handles at pressure 0, ¼, ½, ¾
  and 1, dragged up or down. A curve is straight lines between 2–8 points from pressure 0 to 1,
  increasing, each within 0–1; the command takes the points as `[[pressure, response], …]` or a
  preset name, and `null` is the brush's own response. A bad curve, a number outside 0–1, an
  unknown preset or key is refused whole and changes nothing.
- A **preview** stroke (pressure rising from nothing to full and back) is drawn with the brush
  colour and the curves as you change them. **Reset** puts both curves back.
- The Marker has constant width and opacity, so curves do nothing to it.

**What is stored.** The curves in use are saved *with each new stroke* (`response` in the project
file, written only when it differs from the brush's own, so a project without curves is byte for
byte what it was). A stroke therefore looks the same after changing the settings, after saving and
reopening, and in every export; changing the settings never alters a stroke already drawn. The
stored pressures are the pen's own; the curve is applied when the stroke is drawn. Schema v18 added
the field, so an older build says "newer schema" instead of drawing it differently. A damaged curve
in a file costs the stroke its response, not the stroke. `drawing.addStroke` takes the same
`response` inside `stroke`; an in-between of two strokes takes the nearer stroke's response.

**Mouse.** A mouse draws at full pressure and never stores a response, so it is the same whatever
the curves are. With Pressure switched off a stroke stores no response either (the platform pen
queue still sends its own pressures; that switch has only ever governed the pointer fallback).
The setting is saved with the interface state and adds no undo step. Hardware tuning on a real
pen is the maintainer's check (#5): the tests drive synthetic pen samples.

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
| Draw and manage cels | `drawing.new`, `drawing.newCel`, `drawing.retimeCel`, `drawing.setHold`, `drawing.retimeExposures`, `drawing.previousCel`, `drawing.nextCel`, `drawing.reviewPrevious`, `drawing.reviewNext`, `drawing.addStroke`, `drawing.clearCel`, `drawing.deleteCel`, `drawing.tween`, `drawing.lassoSelect`, `drawing.transformStrokes`, `drawing.lassoMode`, `drawing.selectStrokes`, `drawing.selectionTransform`, `drawing.selectionApply`, `reference.place`, `reference.pasteImage`, `reference.set`, `reference.list`, `clip.setGuide`, `drawing.onionSkin`, `drawing.pinPose`, `drawing.smoothing`, `drawing.pressureCurve`, `drawing.lightTable`, `drawing.lightTableShift`, `drawing.lightTableReset`, the Drawing View commands `view.canvasRotate`, `view.canvasRotateRight`, `view.canvasRotateLeft`, `view.canvasMirror`, `view.canvasPan`, `view.canvasReset` |
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
