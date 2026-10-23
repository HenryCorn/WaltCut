# Agents

WaltCut is built to be driven by AI agents, and largely built by them. Part 1 covers using
WaltCut from an agent. Part 2 covers developing WaltCut as an agent.

# Part 1: driving WaltCut

## 1. MCP server

`waltcut-cli mcp` serves MCP on stdio. It has two modes:

| Mode | Command | What it drives |
|---|---|---|
| Headless | `waltcut-cli mcp --demo` or `--project p.waltcut` (neither = empty project) | an in-process engine session, no window |
| Bridge | `waltcut-cli mcp --bridge 127.0.0.1:9876` | the running desktop app started with `waltcut --control 9876` |

Headless MCP accepts only `--demo` or `--project`, and bridge MCP accepts `--bridge`.
Other options are rejected (exit 2):
there are no folder-confinement flags such as `--automation-read-root`, so the server can read
and write wherever the user can.

**Claude Code.** The repository's `.mcp.json` registers both servers (`waltcut` = bridge,
`waltcut-headless` = demo). Both point at `target/release/waltcut-cli`, so build it first:

```sh
cargo build --release -p waltcut-cli -p waltcut
./target/release/waltcut --control 9876 &      # only for the bridge server
claude                                           # approve the project MCP servers when asked
```

To register the server by hand, or for another MCP client, use the absolute binary path:

```sh
claude mcp add waltcut-headless -- /abs/path/waltcut/target/release/waltcut-cli mcp --demo
```

```json
{ "mcpServers": { "waltcut": { "command": "/abs/path/waltcut-cli", "args": ["mcp", "--bridge", "127.0.0.1:9876"] } } }
```

### From an installed release

The release packages ship `waltcut-cli` alongside the desktop app, so no build is needed:

| Install | CLI |
|---|---|
| macOS | the separate `waltcut-cli-<version>-macos-<arch>.zip` release asset (the `.app` holds only the desktop app) |

```sh
# macOS, with the CLI unzipped onto PATH
claude mcp add waltcut -- waltcut-cli mcp
```

### Tools

| Tool | Modes | Purpose |
|---|---|---|
| `command_list` | both | every command: id, label, menu, shortcut, params, enabled now (`filter`, `enabled_only`) |
| `command_run` | both | run a command `{id, params}`; edits are undoable |
| `command_batch` | both | run several commands in order `{steps: [{id, params}], stop_on_error}` → `{completed, failed, results}` |
| `doc_inspect` | both | the project tree and the active sequence in one call (`project_inspect` + `sequence_inspect`) |
| `render_preview` | both | same as `render_frame` |
| `project_inspect` | both | bins and items with ids, types, durations; active sequence |
| `sequence_inspect` | both | the active sequence: tracks, clips (`start` / `end` and `sourceIn` / `sourceOut` in ticks, frames, `speed`, `reverse`: the clip plays its `sourceIn` to `sourceOut` stretch backward, `gainDb`), effects, transitions, markers (name and `comment`), playhead, selection |
| `media_import` | both | import files by absolute path (`paths`, an array; optional `bin`, `image_sequence`) |
| `render_frame` | both | PNG of the program frame at `seconds` (headless renders; bridge screenshots the Program monitor). Read-only: the playhead and selection are left as they were |
| `ui_inspect` | bridge | UI state: tool, workspace, panels, zoom, playback, fps |
| `ui_elements` | bridge | on-screen interactive elements with id, label and rect (`prefix` filter) |
| `ui_click` | bridge | click by `id` or `x`,`y`; `button`, `count`, `modifiers` |
| `ui_drag` | bridge | press–move–release between points or element ids |
| `ui_key` | bridge | key or shortcut: `Space`, `Cmd+K`, `Shift+Delete` |
| `ui_type` | bridge | type text into the focused field |
| `ui_screenshot` | bridge | PNG of the window or one `panel` |
| `ui_control` | bridge | call any control-channel method directly (`ui.set`, `ui.scroll`, `ui.timeline.locate`, …) |

Typical loop: `project_inspect` / `sequence_inspect` → get ids → `command_run` → `render_frame` or
`ui_screenshot` → look at the result → `edit.undo` if it's wrong.

### Conventions

The server follows a few conventions that make it predictable for agents. They are not
WaltCut-specific: the sibling craft apps' MCP servers can follow the same ones, so an agent that has
driven one app can drive the others.

- **Core tools.** `command_list`, `command_run`, `command_batch`, `doc_inspect` and `render_preview`
  work in both modes. The WaltCut tools above stay as they are.
- **Titles and annotations.** Every tool has a `title` and the four MCP hints (`readOnlyHint`,
  `destructiveHint`, `idempotentHint`, `openWorldHint`), so a client can let the read-only tools
  (lists, inspects, renders, screenshots) run without asking and ask before edits. When you add a
  tool, set them in its `#[tool(...)]` attribute; a test checks every listed tool.
- **Strict arguments.** An unknown tool argument is a JSON-RPC `-32602` error that names it and the
  accepted ones (`unknown argument "filtr" for command_list; expected: enabled_only, filter`), not a
  silently ignored key. `command_run.params` goes to the command unchanged.
- **Errors.** A failing command is an `isError` result with the engine's message. A line that is not
  JSON gets a `-32700` parse error (id `null`) and the server keeps serving. A command that panics is
  reported as `internal error: …`, and the session stays usable.
- **Resources.** `waltcut://document` (the project tree and active sequence, as `doc_inspect`) and
  `waltcut://commands` (the command catalog, as `command_list`), both `application/json`. For MCP
  2026-07-28 clients such as current Claude Code, list and read results carry the `ttlMs` and
  `cacheScope` hints that revision requires.

### Long exports: progress and cancellation

`command_run {"id": "file.exportMedia", "params": {…, "wait": true}}` (or `export.quick`) blocks until
the file is written. It uses the MCP progress and cancellation utilities, so a client can show
progress and stop it:

- The export runs as the same background job `jobs.list` shows. The session is locked only for short
  polls, so other requests (`ping`, `doc_inspect`, `jobs.list`, …) are answered while it encodes.
- Waited media/quick exports and export jobs in `jobs.list` carry
  `offlineMedia`: the offline items the export drew the placeholder for (see §2b), `[]`
  when none. The export itself still succeeds; deciding what to do about it is up to the caller.
- Every job in `jobs.list` carries `etaSeconds`: the
  time left at the job's speed over the last 15 seconds, `null` until a second of progress has been
  measured (a seek) and once it is done. The app shows it as `37% · 2:05 left` in the
  status bar and Progress diagnostics.
- With `"_meta": {"progressToken": T}` on the `tools/call`, the server sends
  `notifications/progress` `{progressToken: T, progress: <frames done>, total: <frames>, message:
  <job status>}` at most every 100 ms while frames advance. Without a token it sends none.
- `notifications/cancelled` for the request stops the encode at the next batch (`jobs.cancel`),
  waits for worker completion and discards its owned staging/partial sequence files, without
  deleting a pre-existing destination or another writer's replacement. The MCP utility sends no
  response for the cancelled request. An inability to confirm cancellation is an error, never a
  successful result.
- Without `wait` the command returns `{job, path, poll: "jobs.list", cancel: "jobs.cancel"}` at
  once: poll `jobs.list` for the job's `progress`, `etaSeconds`, `finished` and `result`, and stop it
  with `jobs.cancel`. A second export start fails while this worker is active; there is no queue or
  backlog. Finished history is bounded and the session cancels/joins its worker on shutdown.
  `file.exportFrame` finishes synchronously and
  ignore the token.
- Bridge mode works the same way: the export runs as an app job (the app stays responsive and shows
  "Exporting… NN%" in its status bar) and the call returns when it is written, however long it
  takes. `waltcut-cli --bridge … exec file.exportMedia … wait=true` blocks the same way.

### Retained output contract (#25 draft)

Media outputs are only **H.264 + AAC MP4**, **ProRes 4444 MOV with alpha** (stereo float PCM), and
**PNG sequence with alpha**; `file.exportFrame` writes **PNG/JPEG**. Export and Quick Export share
the simple destination/format/range controls. Unknown arguments, removed formats, queue/preset
commands and advanced delivery settings are errors, not ignored compatibility options.

```sh
waltcut-cli --project p.waltcut export movie.mp4 --start 0 --end 0.25
waltcut-cli --project p.waltcut export alpha.mov --format prores --range inOut
waltcut-cli --project p.waltcut export frames.png --format png --start 0 --end 0.25
waltcut-cli --project p.waltcut render --seconds 0.25 --out frame.png
```

Media range bounds must be paired; custom ends are exclusive. An explicit non-custom range cannot
be combined with start/end. Destination extensions must match. Movies/stills require `--overwrite`
for replacement; PNG sequences require unused names even with an overwrite request. CLI `render`
dispatches `file.exportFrame` at full resolution, without a scale override, and also works through
the bridge. Offline output is named in the result and warned about; CLI exits 1 unless
`--allow-offline` accepts placeholders. See [the command reference](control-protocol.md#export).

**IMPLEMENTATION DRAFTED**: native retained-import fixtures, every retained output round trip,
actual callback/MOV samples, real iPhone manual acceptance, full CI and animation-flow screenshot
are pending. No real iPhone file has been checked; #25 is not complete, accepted or merged.

## 2. Control channel

`waltcut --control 9876` (or `WALTCUT_CONTROL_PORT=9876`) listens on `127.0.0.1` only. Send one
JSON request per line and get one JSON reply per line. Full method table:
[control-protocol.md](control-protocol.md).

```jsonc
{"id":1,"method":"engine.commands"}
{"id":2,"method":"engine.execute","params":{"command":"file.openDemoProject"}}
{"id":3,"method":"engine.execute","params":{"command":"playhead.set","params":{"seconds":2}}}
{"id":4,"method":"engine.execute","params":{"command":"sequence.addEdit"}}
{"id":5,"method":"engine.execute","params":{"command":"effects.setParam","params":{"clip":1,"effect":"opacity","param":"opacity","value":75}}}
{"id":6,"method":"ui.elements","params":{"prefix":"tools."}}
{"id":7,"method":"ui.click","params":{"id":"tools.Razor"}}
{"id":8,"method":"ui.key","params":{"key":"Cmd+Z"}}
{"id":9,"method":"ui.screenshot","params":{"path":"/tmp/timeline.png","panel":"Timeline"}}
```

Replies look like `{"id":4,"ok":true,"result":{"cuts":2}}` or `{"id":7,"ok":false,"error":"…"}`.
Minimal client:

```python
import json, socket
s = socket.create_connection(("127.0.0.1", 9876)); f = s.makefile("rw")
def call(method, **params):
    f.write(json.dumps({"id": 1, "method": method, "params": params}) + "\n"); f.flush()
    return json.loads(f.readline())
print(call("engine.execute", command="sequence.inspect"))
```

Notes:

- Time is in ticks (254 016 000 000 per second). Commands also accept `seconds`, `frame` or
  `timecode`.
- A track parameter (`track`, `audioTrack`) is a track id or a name such as `"V1"` / `"A2"`.
  Leaving it out picks the command's default track; naming a track the sequence does not have is
  an "invalid parameters" error, never a different track.
  In `timeline.place`, `track` is where the picture goes and `audioTrack` where the sound goes;
  an audio track given as `track` places the sound alone, on that track.
  The clip lands on the frame grid: its start and its end (`time + duration`) each snap to the
  nearest frame.
- `sequence.inspect` reports `durationFrames` as the number of frames export renders: a sequence
  that ends partway through a frame counts that frame.
- `engine.execute` also runs UI-only commands (`tool.razor`, `playback.toggle`,
  `window.workspace.color`, `window.panel.<name>`).
- A command that works on the selection is disabled ("no clips selected", "select a clip in the
  Project panel") until something is selected, unless the call names its targets under a key the
  command documents (`clips` / `clip` for timeline clips, `items` / `item` for project items):
  `clip.replaceFromBin {"clips":[12],"item":4}` runs with nothing selected and leaves the
  selection alone. Only the selection condition is answered by the named targets; every other
  one (an open sequence, a filled clipboard, the right kind of clip) still applies, and ids of
  nothing do not count. `command_list` / `engine.commands` keep reporting the selection's state.
- Element ids come from the previous frame. If an element is missing right after a layout change,
  the app retries on later frames before giving up.
- `timeline.move` takes linked partners along while Linked Selection is on: moving a picture
  clip moves its sound by the same offset (pass `"linked": false` to move only the listed clips).
  It returns `{"moved": [ids]}`, every clip that moved.
- Without a window, `waltcut-cli run script.jsonl` (lines of `{"id":"…","params":{…}}`) runs the
  same commands headlessly. See §2b for the rest of the CLI.

## 2b. Command-line interface

Every command is also one shell call away. Options go anywhere; output is JSON; exit status is 0 on
success, 1 when a command fails and 2 on a usage error. `waltcut-cli help`, `waltcut-cli --help`
and `waltcut-cli -h` print the reference and exit successfully.
If the reader of stdout closes the pipe early (`waltcut-cli commands | head`), the CLI drops the rest
of its output but still finishes the work, saves included, and the exit status still reports
failures; any other stdout write error is reported and makes the status 1.

```sh
waltcut-cli commands razor                     # find ids (add --json for machine output)
waltcut-cli describe timeline.razor            # one command: menu, shortcut, params, enabled now
waltcut-cli --demo inspect sequence            # project / sequence as JSON
waltcut-cli --project p.waltcut --save exec timeline.razor seconds=3.5
waltcut-cli --project p.waltcut exec effects.apply '{"effect":"Gaussian Blur"}'
waltcut-cli --project p.waltcut exec markers.exportCsv path=review.csv
waltcut-cli --project p.waltcut --save import a.mov b.wav
waltcut-cli --project p.waltcut export out.mp4  # format from the extension; waits for the job
waltcut-cli --project p.waltcut export out --preset "YouTube 1080p Full HD" --start 0 --end 10
waltcut-cli export --list-presets prores       # built-in + user presets (--data-dir for another library)
waltcut-cli --project p.waltcut exec export.queue.add preset="Apple ProRes 422 HQ" path=renders/ start=true wait=true
echo '{"id":"file.newBin","params":{"name":"Selects"}}' | waltcut-cli --project p.waltcut --save run -
waltcut-cli --bridge 127.0.0.1:9876 exec window.workspace.color   # the running app
```

`key=value` values are parsed as JSON when they can be (`3.5`, `true`, `[1,2]`), otherwise taken as
strings; dotted keys nest (`color.r=1`). `run` prints one JSON line per command
(`{"line","id","ok","result"|"error"}`) and stops at the first failure unless `--keep-going`.
`--save` writes back to `--project`; `--save-as path` writes elsewhere.

Media that is offline when it is needed (the file is missing, was made offline, or can't be read)
renders as the "media not found" placeholder. `export` and `render` then name each such item on
stderr (`item 8 a.mp4 (missing): /path/a.mp4`) and exit 1 after writing the output, so a moved or
unmounted media folder doesn't pass as a good render. `--allow-offline` accepts the placeholder
frames: the warning stays and the exit status is 0. The export result lists the same items as
`offlineMedia` (`[{item, name, path, status, error}]`, `status` as in `media.status`: `missing`,
`offline` or `unreadable`), and `[]` when every clip it drew was online; check before exporting with
`exec media.status` or `exec media.findMissing`.

`markers.exportCsv {path}` exports all markers in the active sequence, including colours hidden
in the UI and markers outside In/Out, sorted by start then id. The UTF-8 CSV has a header and CRLF
records; quotes, commas and embedded newlines are escaped. Columns are Sequence, Marker ID, Name,
Comment, Kind, Color, Start Timecode, End Timecode, Duration Frames, Start Ticks, Duration Ticks and
Frame Rate (exact numerator/denominator). Color uses the user's display label from Settings ▸
Labels. Timecodes include the sequence's start timecode and
drop-frame setting; End is exclusive, and a point marker's End equals Start. Duration Frames is
rounded down; tick columns preserve sub-frame timing (254016000000 ticks per second).
An empty sequence writes just the header. User text starting with a spreadsheet formula character
(`=`, `+`, `-`, `@`, including after whitespace) gets a leading apostrophe in the exported cell.
The export leaves the project, dirty flag and undo history untouched, works with locked tracks,
and uses the host's file service (atomic
on desktop, a download on web). It exports sequence markers only, not clip/source markers;
reports are limited to 100000 markers and a conservative 16 MiB output budget. Invalid timing or a
failed write reports an error. The menu opens the normal save dialog; cancelling writes nothing.

## 3. Verifying UI work

For any UI change, look at the result:

1. `cargo run --release -p waltcut -- --control 9876`
2. Drive the feature through the control channel or the MCP bridge: commands, then clicks and drags
   by automation id.
3. Assert on `ui.inspect`, `ui.elements` and `sequence.inspect`.
4. Take `ui.screenshot` (the whole window and the panel you changed) and open the PNG.

Screenshots you commit must follow [AGENTS.md](../AGENTS.md) §1: WaltCut only, openly licensed
media, sidecar and `ATTRIBUTION.md` entry.

# Part 2: developing WaltCut

## 4. Orientation, in this order

1. [AGENTS.md](../AGENTS.md): absolute rules (assets, clean-room, licences).
2. [CLAUDE.md](../CLAUDE.md): working instructions and non-negotiables.
3. [ROADMAP.md](../ROADMAP.md) for the summary, [roadmap.md](roadmap.md) (current focus) and
   [animation.md](animation.md) (the product scope).
4. [architecture.md](architecture.md), then the README and tests of the crate you'll touch.
5. [contributing.md](contributing.md) (how to add things, gates) and [testing.md](testing.md).

## 5. Autonomous work loop

1. **Orient.** Pick the next open task of the epic #14 or the roadmap's current focus. Read the
   relevant architecture section and crate README.
2. **Plan tests first.** Write down the acceptance test before writing code.
3. **Implement and test.**
4. **Verify.** Run all gates (`cargo xtask ci`). For UI work, run the app with `--control`, drive it
   and look at the screenshots (§3).
5. **Record.** Update the crate README (behaviour decisions, test results, limitations) and
   the progress docs when a milestone moves (ROADMAP.md, docs/roadmap.md). Commit as `#<issue>: …`,
   then move on to the next task.

For parallel agents, use one git worktree and one `CARGO_TARGET_DIR` per agent, and keep each crate
with one owner ([contributing.md §5](contributing.md#5-parallel-work-several-agents-or-worktrees)).

## 6. Decision rules

Don't block on choices you can make yourself:

- **Behaviour.** Follow [animation.md](animation.md). Never inspect other applications' internals
  ([AGENTS.md](../AGENTS.md) §2). If behaviour is undocumented, choose what animators most commonly
  expect and record the choice in the crate README.
- **Ask a human only about:**
  - licences outside the allowlist;
  - spending money;
  - publishing or pushing;
  - personal data;
  - destructive git operations;
  - anything AGENTS.md leaves unclear ("when in doubt, leave it out").

## 7. Definition of Done

A feature is done when:

- [ ] its command(s) are registered with label, menu path, shortcut (if any) and a parameter doc;
- [ ] engine tests cover it, including undo/redo and the disabled case;
- [ ] the UI is wired: menu, panel control and shortcut;
- [ ] every new interactive widget has an automation id;
- [ ] you have driven it through the control channel and reviewed a screenshot;
- [ ] all gates pass (`cargo xtask ci`), and new assets have sidecars;
- [ ] the crate README and the progress docs (ROADMAP.md, docs/roadmap.md) are updated where relevant;
- [ ] it is committed with its task id.

### Reporting progress honestly

- say how a number was obtained: *measured* (a test, a diff, a benchmark you ran) or *estimated*;
- count an approximation, a stub or a partly wired setting as such, never as done;
- record known bugs and limitations in the crate README;
- benchmarks state the machine, the load average and before/after on the same build.
