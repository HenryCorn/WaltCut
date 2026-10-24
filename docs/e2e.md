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
| Cels | new, duplicate and held cels, `drawing.tween`, retime frame 4 to 5, retime three cels on twos and undo, onion skin | cels at frames 0, 2, 5, 8; the twos run starts 0, 2, 4, 7; onion skin on |
| Preview | play, stop, three frame steps | the playhead moved three frames |
| Export | MP4 (H.264), MOV (ProRes 4444), PNG sequence | each decodes; the MOV has alpha; one PNG per frame |

Screenshots of each stage go to `<out>/shots` (`--no-shots` skips them). The exit status is 0 when
every check passes.

**Not covered:** AI Restyle. Its mock provider is a seam in the engine tests
(`restyler_override`), not reachable from the running app, and a real provider needs a key; see
[animation.md](animation.md). Also not covered: a real iPhone HEVC Main10 HLG file, which has to be
checked by hand.
