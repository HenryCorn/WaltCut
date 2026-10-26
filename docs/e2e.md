# End-to-end animation-flow check

`tools/e2e-animation-flow.py` drives the real desktop app through the [control
channel](control-protocol.md) over the whole retained workflow and checks the results. It is the
#27 acceptance run and a good smoke test after large changes.

```sh
cargo run --release -p waltcut -- --control 9876      # one terminal
python3 tools/e2e-animation-flow.py --out /tmp/waltcut-e2e
```

It needs `ffmpeg` and `ffprobe` (to make synthetic footage and to check the exports) and uses only
the Python standard library. Use `WALTCUT_DATA_DIR=<dir>` for the app to keep the run away from your
own preferences and recovery files.

| Step | What it does | What it checks |
|---|---|---|
| Project | `file.newProject` | – |
| Import | an MP4, a WAV and a 6-frame PNG sequence | no import errors, 6 frames in the sequence |
| Timeline | sequence from the clip, place the image sequence and audio, trim, razor split | three clips on V1 |
| Drawing | `drawing.new`, then Pen, Pencil, Marker and Eraser strokes | – (screenshots) |
| Cels | new, duplicate and held cels, `drawing.tween`, retime frame 4 to 5, retime three cels on twos and undo, Next Cel and a pose review that restores the playhead, onion skin | cels at frames 0, 2, 5, 8; the twos run starts 0, 2, 4, 7; onion skin on |
| Onion skin | two cels before and one after with fading opacity; a pinned pose and the loop start with nothing else | ghosts at frames 0, 2, 8; opacities 0.25 and 0.5; then exactly the loop and pin ghosts |
| Lasso | lasso everything, preview a transform, apply it, undo | a pending transform leaves the exported still unchanged; applying changes it; one undo restores it |
| Smoothing | turn smoothing on, draw a stroke through the pointer, turn it off, undo | the stroke is added with smoothing on; the setting reads back; one undo removes the stroke (jitter, corners, pen-up and Escape are covered by `drawing_smooth.rs` and `smoothing_ui.rs`) |
| Rope stabilizer | `drawing.smoothing {mode: rope, length: 40}`, a huge length refused whole, draw a stroke through the pointer, turn it off, undo | the mode and length read back; the bad length changes nothing; the stroke is added; one undo removes it (string length, corners, pen-up end, tap, batching and Escape are covered by `drawing_smooth.rs` and `smoothing_ui.rs`) |
| Pressure curves | set the Soft preset and a custom width curve, read them back, reset | the curves read back as set; a bad curve is refused; reset restores the brush's own response (the stored stroke, the mouse and the pen path are covered by `pressure_curve_ui.rs`) |
| Fill | draw a closed square on a new cel, `drawing.fill` inside it, recolour it with `drawing.fillEdit`, a point outside the canvas, a pointer click in Fill mode, undo | the fill covers only the inside and shows in the exported still; recolouring keeps the region and changes the still; the outside point is refused; the click adds a fill; undo steps give back the outline alone (concave shapes, holes, transparency, edges, order, save and load, locked tracks and preview-vs-export are covered by `drawing_fill_tests.rs`, `drawing_fill.rs` and `fill_ui.rs`) |
| Reference fill | line art with a 4 pixel opening on its own clip, `drawing.fillPreview` without and with `gap`, `drawing.fill` from the reference with gap and grow, undo, Preview mode click then Cancel, then Apply, undo | no radius leaks over the canvas, radius 2 closes it; the fill records `reference`, `gap`, `grow` and shows in the exported still; the line art clip is unchanged; undo restores the still; a preview changes nothing until Apply (gap sizes at and beyond the limit, hostile values, locked and missing references, save and load and preview-vs-export are covered by `drawing_fill.rs`, `drawing_fill_tests.rs` and `fill_ui.rs`) |
| Light table | pin a cel, shift and turn the pinned pose, drag it with the Shift pose hold, draw against it, Escape inside a hold, Reset, undo | the exported still is the same with the pose shifted; the drag moves the pose and draws nothing; the stroke is drawn and the alignment survives it; Escape gives back the hold's changes; Reset clears the pose; one undo removes the stroke |
| Linked exposures | link a cel to two later frames, add a duplicate, read the lane and options bar, draw on one exposure, export stills, make one independent, delete one, undo, a refused link | the group has three exposures and the duplicate none; the lane label and the `program.brush.linked` badge say three share the artwork; the stroke reaches all three and not the duplicate, and the exported still of another linked exposure changes while the duplicate's does not; the detached exposure stops following; undo gives back the original cels; a link from a frame with no cel is refused (hold vs duplicate vs link, fills, lasso, locked tracks, save and load, cache sharing and preview-vs-export are covered by `drawing_link_tests.rs`, `cel_link_ui.rs` and `fixtures.rs`) |
| Reference image | place a picture under the drawing, adjust it, hide it, undo | one reference at 50 %; a Reference track is added; the exported still differs with it and equals the plain one when it is hidden; three undo steps remove it |
| Guide clip | mark the drawing clip as a guide, export a frame with and without `includeGuides`, unmark | the default still is the same as the unguided one's; with `includeGuides` it differs |
| Drawing view | rotate 90°, mirror, draw a stroke through the turned view, pan hold, reset | a stroke drawn through the turned view is added; the pan hold switches on; reset restores 0°, no mirror (where the stroke lands is covered by `canvas_view_ui.rs`) |
| Preview | play, stop, three frame steps | the playhead moved three frames |
| Export | MP4 (H.264), MOV (ProRes 4444), PNG sequence | each decodes; the MOV has alpha; one PNG per frame |

Screenshots of each stage go to `<out>/shots` (`--no-shots` skips them). The exit status is 0 when
every check passes.

**Not covered:** AI Restyle. Its mock provider is a seam in the engine tests
(`restyler_override`), not reachable from the running app, and a real provider needs a key; see
[animation.md](animation.md). Also not covered: a real iPhone HEVC Main10 HLG file, which has to be
checked by hand.
