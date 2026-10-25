#!/usr/bin/env python3
"""End-to-end check of the retained animation workflow, driven through the control channel.

    cargo run --release -p waltcut -- --control 9876      # in one terminal
    python3 tools/e2e-animation-flow.py --out /tmp/waltcut-e2e

Steps: new project; import footage, an image sequence and audio; arrange on the timeline (trim,
split); draw with pen, pencil, marker and eraser; new, duplicate and held cels, in-between and
onion skin; lasso, reference and guide clips; the drawing view; stroke smoothing; pressure
curves; the light table's pose shift; preview and frame-step; export an H.264 MP4, a ProRes 4444 MOV and a PNG
sequence and check them with ffprobe. Screenshots of each stage go to <out>/shots. Needs ffmpeg
and ffprobe.

AI Restyle is not driven: its mock provider is an engine-test seam, not reachable from the app.
Standard library only. See docs/e2e.md.
"""
import argparse
import glob
import json
import math
import os
import socket
import subprocess
import sys
import time

TICKS_PER_SECOND = 254_016_000_000

parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
parser.add_argument("--port", type=int, default=9876)
parser.add_argument("--out", default=os.path.join(os.environ.get("TMPDIR", "/tmp"), "waltcut-e2e"))
parser.add_argument("--no-shots", action="store_true", help="skip screenshots (headless runs)")
args = parser.parse_args()

MEDIA = os.path.join(args.out, "media")
SHOTS = os.path.join(args.out, "shots")
OUT = os.path.join(args.out, "exports")
for d in (os.path.join(MEDIA, "seq"), SHOTS, OUT):
    os.makedirs(d, exist_ok=True)


def run(*cmd):
    subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)


# media: synthetic, generated here (nothing is committed)
run("ffmpeg", "-y", "-v", "error", "-f", "lavfi", "-i", "testsrc2=s=320x180:r=24:d=2", "-pix_fmt", "yuv420p", "-c:v", "libx264", os.path.join(MEDIA, "clip.mp4"))
run("ffmpeg", "-y", "-v", "error", "-f", "lavfi", "-i", "gradients=s=320x180:r=6:d=1:seed=7", "-frames:v", "6", os.path.join(MEDIA, "seq", "frame_%03d.png"))
run("ffmpeg", "-y", "-v", "error", "-f", "lavfi", "-i", "sine=frequency=440:duration=1.5", os.path.join(MEDIA, "tone.wav"))

sock = socket.create_connection(("127.0.0.1", args.port), timeout=120)
reader = sock.makefile("r")
_id = 0
failures = []


def call(method, params=None, quiet=False):
    global _id
    _id += 1
    msg = {"id": _id, "method": method}
    if params is not None:
        msg["params"] = params
    sock.sendall((json.dumps(msg) + "\n").encode())
    reply = json.loads(reader.readline())
    if not quiet and not reply.get("ok"):
        print(f"FAIL {method} {json.dumps(params)[:200]}: {reply}")
        sys.exit(1)
    return reply


def cmd(command, params=None):
    return call("engine.execute", {"command": command, "params": params or {}})["result"]


def shot(name):
    if args.no_shots:
        return
    time.sleep(0.6)
    reply = call("ui.screenshot", {"path": os.path.join(SHOTS, f"{name}.png")}, quiet=True)
    print(f"  screenshot {name}: {'ok' if reply.get('ok') else reply.get('error')}")


def check(ok, what):
    print(f"  {'ok  ' if ok else 'FAIL'} {what}")
    if not ok:
        failures.append(what)


def arc(cx, cy, r, n=24):
    return [[cx + r * math.cos(2 * math.pi * k / n), cy + r * math.sin(2 * math.pi * k / n), 0.3 + 0.7 * math.sin(math.pi * k / n)] for k in range(n + 1)]


print("== new project")
cmd("file.newProject", {"name": "E2E"})
shot("01-new-project")

print("== import footage, image sequence, audio")
imported = cmd("file.import", {"paths": [os.path.join(MEDIA, "clip.mp4"), os.path.join(MEDIA, "tone.wav")]})
video_item, audio_item = imported["items"]
sequence_frames = cmd("file.importImageSequence", {"path": os.path.join(MEDIA, "seq", "frame_001.png")})
check(not imported["errors"] and sequence_frames["imageSequences"][0]["frames"] == 6, "footage, audio and a 6-frame image sequence imported")
frames_item = sequence_frames["items"][0]
shot("02-imported")

print("== arrange: sequence, place, trim, split")
made = cmd("file.newSequenceFromClip", {"items": [video_item]})
cmd("timeline.place", {"item": frames_item, "track": "V1", "seconds": 2.0})
cmd("timeline.place", {"item": audio_item, "track": "A1", "seconds": 0.0})
inspected = cmd("sequence.inspect", {})
video_clip = inspected["video"][0]["items"][0]["clip"]
shot("03-arranged")
cmd("timeline.trim", {"clip": video_clip, "edge": "out", "mode": "regular", "deltaFrames": -6})
cmd("timeline.razor", {"time": 12 * TICKS_PER_SECOND // 24, "track": "V1", "clip": video_clip})
v1 = cmd("sequence.inspect", {})["video"][0]["items"]
check(len(v1) == 3, f"trim and split give three clips on V1 ({[(c['startFrame'], c['endFrame']) for c in v1]})")
shot("04-trimmed-split")

print("== drawing: pen, pencil, marker, eraser")
drawing = cmd("drawing.new", {"seconds": 2.0, "track": 2, "name": "Ball"})["clip"]
for brush, color, size, points in [
    ("Pen", "#1d3557", 6.0, arc(110, 90, 40)),
    ("Pencil", "#457b9d", 3.0, [[60, 150, 0.5], [120, 140, 0.7], [200, 150, 0.6], [260, 142, 0.5]]),
    ("Marker", "#e63946", 14.0, [[200, 40, 0.8], [240, 60, 0.9], [270, 100, 0.8]]),
]:
    cmd("drawing.addStroke", {"clip": drawing, "frame": 0, "stroke": {"brush": brush, "color": color, "size": size, "points": points}})
shot("05-drawn-pen-pencil-marker")
cmd("drawing.addStroke", {"clip": drawing, "frame": 0, "stroke": {"brush": "Eraser", "size": 18.0, "points": [[90, 70, 0.8], [130, 110, 0.8]]}})
shot("06-eraser")

print("== cels: new, duplicate, hold, in-between, onion skin")
cmd("drawing.newCel", {"clip": drawing, "frame": 4})
cmd("drawing.addStroke", {"clip": drawing, "frame": 4, "stroke": {"brush": "Pen", "color": "#1d3557", "size": 6.0, "points": arc(150, 70, 40)}})
cmd("drawing.newCel", {"clip": drawing, "frame": 8, "copy": True})
cmd("drawing.addStroke", {"clip": drawing, "frame": 10, "cel": "hold", "stroke": {"brush": "Pen", "color": "#1d3557", "size": 6.0, "points": [[100, 30, 0.5], [110, 34, 0.5]]}})
between = cmd("drawing.tween", {"clip": drawing, "frame": 0, "step": 2})
cmd("drawing.retimeCel", {"clip": drawing, "from": 4, "to": 5})
cels = {c["frame"]: c["strokes"] for c in cmd("drawing.inspect", {"clip": drawing})["cels"]}
check(0 in cels and 2 in cels and 5 in cels and 8 in cels and 4 not in cels and between["cels"] == [2], f"retimed cels at frames {sorted(cels)} and an in-between at {between['cels']}")
twos = cmd("drawing.retimeExposures", {"clip": drawing, "frames": [0, 2, 5], "on": "twos"})
check(twos["cels"] == [0, 2, 4, 7], f"cels 0, 2 and 5 retimed on twos give starts {twos['cels']}")
cmd("edit.undo", {})
check(sorted(c["frame"] for c in cmd("drawing.inspect", {"clip": drawing})["cels"]) == [0, 2, 5, 8], "undo restores the cels")
nxt = cmd("drawing.nextCel", {"clip": drawing, "frame": 3})
check(nxt["cel"] == 5, f"Next Cel from inside the hold of cel 2 lands on cel {nxt['cel']}")
shown = call("ui.inspect")["result"]["playhead"]
rev = cmd("drawing.reviewPrevious", {})
check(rev["cel"] == 2 and call("ui.inspect")["result"]["review"]["active"], "holding Review Previous shows cel 2")
cmd("drawing.reviewEnd", {})
after = call("ui.inspect")["result"]
check(after["playhead"] == shown and not after["review"]["active"], "ending the review puts the playhead back")
state = call("ui.inspect")["result"]["ui"]["brush"]
check(state["onion"] is True, "onion skin is on")
call("ui.set", {"tool": "Brush"}, quiet=True)
shot("07-cels-onion-skin")

print("== onion skin: several cels, a pinned pose and the loop start")
cmd("playhead.set", {"frame": 5})
cmd("drawing.onionSkin", {"before": 2, "after": 1, "opacity": 0.5, "falloff": 0.5})
ghosts = call("ui.inspect")["result"]["ghosts"]
check(sorted(g["frame"] for g in ghosts) == [0, 2, 8], f"two cels before and one after give ghosts at {[g['frame'] for g in ghosts]}")
check([g["opacity"] for g in ghosts if g["role"] == "before"] == [0.25, 0.5], "the farther ghost fades")
cmd("drawing.onionSkin", {"before": 0, "after": 0, "pin": 8, "loop": True})
cmd("playhead.set", {"frame": 3})
ghosts = call("ui.inspect")["result"]["ghosts"]
check([(g["role"], g["frame"]) for g in ghosts] == [("loop", 0), ("pin", 8)], f"the loop start and the pinned pose are the only ghosts ({[(g['role'], g['frame']) for g in ghosts]})")
shot("07b-onion-skin-pin-and-loop")
cmd("drawing.onionSkin", {"before": 1, "after": 1, "opacity": 0.35, "falloff": 0.6, "pin": None, "loop": False})

print("== drawing view: rotate, mirror, draw, pan hold, reset")
cmd("playhead.set", {"frame": 0})
strokes_before = cmd("drawing.inspect", {"clip": drawing})["cels"][0]["strokes"]
strokes_before = cmd("drawing.inspect", {"clip": drawing})["cels"][0]["strokes"]
cmd("view.canvasRotate", {"degrees": 90})
cmd("view.canvasMirror", {"enabled": True})
canvas = call("ui.inspect")["result"]["ui"]["program"]["canvas"]
check(canvas["rotation"] == 90 and canvas["mirror"] is True, "the view is turned 90° and mirrored")
rect = [e for e in call("ui.elements", {"prefix": "program.brush"})["result"] if e["id"] == "program.brush"][0]["rect"]
x, y, w, h = rect
call("ui.drag", {"from": {"x": x + w * 0.4, "y": y + h * 0.82}, "to": {"x": x + w * 0.6, "y": y + h * 0.92}, "steps": 8})
time.sleep(0.5)
strokes_after = cmd("drawing.inspect", {"clip": drawing})["cels"][0]["strokes"]
check(strokes_after == strokes_before + 1, f"a stroke drawn through the turned view is added")
shot("08a-drawing-view-turned")
cmd("view.canvasPan", {"active": True})
check(call("ui.inspect")["result"]["ui"]["program"]["canvas"]["panHeld"] == "Command", "the pan hold is on")
cmd("view.canvasPan", {"active": False})
cmd("view.canvasReset", {})
canvas = call("ui.inspect")["result"]["ui"]["program"]["canvas"]
check(canvas["rotation"] == 0 and canvas["mirror"] is False and canvas["panHeld"] == "Off", "reset returns to 0°, not mirrored")

print("== guide clip: shown in the preview, left out of exports unless asked")
cmd("playhead.set", {"frame": 0})
def still(name, **extra):
    path = os.path.join(OUT, name)
    cmd("file.exportFrame", {"path": path, "overwrite": True, **extra})
    with open(path, "rb") as f:
        return f.read()
ordinary = still("guide-ordinary.png")
cmd("clip.setGuide", {"clips": [drawing], "guide": True})
guided = still("guide-default.png")
included = still("guide-included.png", includeGuides=True)
check(guided != ordinary, "a guide drawing is left out of the default still")
check(included == ordinary, "includeGuides gives the still of the ordinary clip")
check(call("ui.inspect")["result"]["playhead"] == 0, "the exports did not move the playhead")
time.sleep(1.0)
call("ui.inspect")
shot("08b-guide-clip-badge")
cmd("clip.setGuide", {"clips": [drawing], "guide": False})
check(still("guide-back.png") == ordinary, "an ordinary clip is in the default still again")

print("== reference image: place under the drawing, adjust, hide, undo")
tracks_before = len(cmd("sequence.inspect", {})["video"])
without = still("ref-without.png")
placed = cmd("reference.place", {"item": frames_item, "target": drawing})
refs = cmd("reference.list", {"target": drawing})["references"]
check(len(refs) == 1 and refs[0]["opacity"] == 50 and not refs[0]["hidden"], f"one reference at 50% under the drawing ({[(r['clip'], r['opacity']) for r in refs]})")
check(len(cmd("sequence.inspect", {})["video"]) == tracks_before + 1, "a Reference track was added")
check(still("ref-with.png") != without, "a visible reference is in the exported still")
cmd("reference.set", {"target": drawing, "opacity": 80, "dx": 12, "dy": -6, "scale": 90, "rotation": 4})
moved = cmd("reference.list", {"target": drawing})["references"][0]
check(moved["opacity"] == 80 and moved["scale"] == 90 and moved["rotation"] == 4, "opacity, scale and rotation set")
shot("08c-reference-image")
cmd("reference.set", {"target": drawing, "hidden": True})
check(still("ref-hidden.png") == without, "a hidden reference is not exported")
cmd("edit.undo", {})
cmd("edit.undo", {})
cmd("edit.undo", {})
check(len(cmd("sequence.inspect", {})["video"]) == tracks_before and not cmd("reference.list", {"target": drawing})["references"], "three undo steps (hide, adjust, place) take the reference away")

print("== lasso: select strokes, preview a transform, apply, undo")
cmd("playhead.set", {"frame": 0})
call("ui.set", {"tool": "Brush"}, quiet=True)
lasso_before = still("lasso-before.png")
cmd("drawing.lassoMode", {"enabled": True})
everything = [[-5000, -5000], [5000, -5000], [5000, 5000], [-5000, 5000]]
sel = cmd("drawing.selectStrokes", {"lasso": everything})
check(sel["selected"] >= 3, f"a lasso around the drawing selects its paint strokes ({sel['strokes']}, erasers {sel['erasers']})")
pending = cmd("drawing.selectionTransform", {"dx": 24, "dy": 8, "rotation": 5})
check(pending["refused"] is None and still("lasso-preview.png") == lasso_before, "a pending transform is only a preview: the exported still is unchanged")
shot("08d-lasso-selection")
cmd("drawing.selectionApply", {})
lasso_after = still("lasso-after.png")
check(lasso_after != lasso_before, "applying moved the strokes")
cmd("edit.undo", {})
check(still("lasso-undone.png") == lasso_before, "one undo puts every stroke back")
cmd("drawing.lassoMode", {"enabled": False})

print("== smoothing: a stroke with smoothing on, back off, undo")
cmd("playhead.set", {"frame": 0})
call("ui.set", {"tool": "Brush"}, quiet=True)
time.sleep(0.4)
rect = [e for e in call("ui.elements", {"prefix": "program.brush"})["result"] if e["id"] == "program.brush"][0]["rect"]
x, y, w, h = rect
strokes_at = lambda: next(c["strokes"] for c in cmd("drawing.inspect", {"clip": drawing})["cels"] if c["frame"] == 0)
check(cmd("drawing.smoothing", {})["mode"] == "off", "smoothing starts off")
cmd("drawing.smoothing", {"mode": "smooth", "strength": 0.8, "pressure": 0.3})
state = call("ui.inspect")["result"]["ui"]["brush"]["smoothing"]
check(state["mode"] == "Smooth" and abs(state["strength"] - 0.8) < 1e-6, f"smoothing is on ({state})")
before = strokes_at()
call("ui.drag", {"from": {"x": x + w * 0.2, "y": y + h * 0.8}, "to": {"x": x + w * 0.7, "y": y + h * 0.85}, "steps": 40})
time.sleep(0.4)
check(strokes_at() == before + 1, "a smoothed stroke was drawn")
shot("08h-smoothing")
cmd("drawing.smoothing", {"mode": "off"})
check(call("ui.inspect")["result"]["ui"]["brush"]["smoothing"]["mode"] == "Off", "smoothing is off again")
cmd("edit.undo", {})
check(strokes_at() == before, "one undo takes the stroke away; the setting had no undo step")

print("== pressure curves: presets, a custom width curve, a refused curve, reset")
check(cmd("drawing.pressureCurve", {})["default"] is True, "the brush starts with its own pressure response")
soft = cmd("drawing.pressureCurve", {"preset": "soft"})
check(soft["width"] == soft["opacity"] and not soft["default"], "the Soft preset sets both curves")
custom = cmd("drawing.pressureCurve", {"width": [[0, 0], [0.5, 0.2], [1, 1]], "opacity": None})
check(len(custom["width"]) == 3 and custom["opacity"] is None, f"a custom width curve and the brush's own opacity ({custom})")
r = call("engine.execute", {"command": "drawing.pressureCurve", "params": {"width": [[0, 0], [0.7, 0.5], [0.3, 0.6], [1, 1]]}}, quiet=True)
check(not r.get("ok"), "a curve that goes backwards is refused")
check(call("ui.inspect")["result"]["ui"]["brush"]["response"]["width"] == custom["width"], "the refusal changed nothing")
check(cmd("drawing.pressureCurve", {"reset": True})["default"] is True, "reset restores the brush's own response")

print("== light table: shift the pinned pose, draw against it, Escape, reset")
cmd("playhead.set", {"frame": 0})
call("ui.set", {"tool": "Brush"}, quiet=True)
cmd("drawing.onionSkin", {"enabled": True, "before": 0, "after": 0, "pin": 8, "loop": False})
home = still("light-before.png")
shifted = cmd("drawing.lightTable", {"dx": 30, "dy": -12, "rotation": 8})
check(shifted["shifted"] and shifted["showing"] and (shifted["dx"], shifted["dy"], shifted["rotation"]) == (30, -12, 8), f"the pinned pose is shifted and showing ({shifted})")
time.sleep(0.6)
call("ui.inspect")
check(still("light-shifted.png") == home, "a shifted pose is not in the exported still")
shot("08e-light-table-shifted")
strokes_before = cmd("drawing.inspect", {"clip": drawing})["cels"][0]["strokes"]
cmd("drawing.lightTableShift", {"active": True})
check(call("ui.inspect")["result"]["ui"]["brush"]["light_table"]["hold"] == "Command", "the Shift pose hold is on")
rect = [e for e in call("ui.elements", {"prefix": "program.brush"})["result"] if e["id"] == "program.brush"][0]["rect"]
x, y, w, h = rect
call("ui.drag", {"from": {"x": x + w * 0.45, "y": y + h * 0.78}, "to": {"x": x + w * 0.55, "y": y + h * 0.88}, "steps": 8})
time.sleep(0.4)
table = call("ui.inspect")["result"]["ui"]["brush"]["light_table"]
check(table["shift"]["dx"] > 30 and table["shift"]["dy"] > -12, f"a drag moved the pose with the pen ({table['shift']})")
check(cmd("drawing.inspect", {"clip": drawing})["cels"][0]["strokes"] == strokes_before, "the drag drew nothing")
cmd("drawing.lightTableShift", {"active": False})
kept = call("ui.inspect")["result"]["ui"]["brush"]["light_table"]["shift"]
call("ui.drag", {"from": {"x": x + w * 0.4, "y": y + h * 0.72}, "to": {"x": x + w * 0.6, "y": y + h * 0.8}, "steps": 8})
time.sleep(0.4)
check(cmd("drawing.inspect", {"clip": drawing})["cels"][0]["strokes"] == strokes_before + 1, "the current cel is still drawable against the shifted pose")
check(call("ui.inspect")["result"]["ui"]["brush"]["light_table"]["shift"] == kept, "drawing kept the alignment")
shot("08f-light-table-drawing")
cmd("drawing.lightTableShift", {"active": True})
cmd("drawing.lightTable", {"dx": 99})
call("ui.key", {"key": "Escape"})
back = call("ui.inspect")["result"]["ui"]["brush"]["light_table"]
check(back["hold"] == "Off" and back["shift"] == kept, "Escape gives back what the hold changed")
cmd("drawing.lightTableReset", {})
check(not call("ui.inspect")["result"]["ui"]["brush"]["light_table"]["pose"], "Reset puts the pose back where it was drawn")
time.sleep(0.6)
call("ui.inspect")
shot("08g-light-table-reset")
cmd("edit.undo", {})
check(cmd("drawing.inspect", {"clip": drawing})["cels"][0]["strokes"] == strokes_before, "one undo takes the stroke away; the shift had no undo step")
cmd("drawing.onionSkin", {"before": 1, "after": 1, "pin": None})

print("== preview and frame-step")
cmd("playhead.set", {"frame": 0})
call("ui.playback", {"action": "play"})
time.sleep(1.0)
call("ui.playback", {"action": "stop"})
cmd("playhead.set", {"frame": 0})
for _ in range(3):
    cmd("playhead.stepForward", {})
check(call("ui.inspect")["result"]["playhead"] == 3 * TICKS_PER_SECOND // 24, "three frame steps move the playhead three frames")
shot("08-preview-frame-step")

print("== export MP4, ProRes 4444, PNG sequence")
for stale in glob.glob(os.path.join(OUT, "*")):
    os.remove(stale)
paths = {"h264": os.path.join(OUT, "e2e.mp4"), "prores": os.path.join(OUT, "e2e.mov"), "png": os.path.join(OUT, "e2e_frames.png")}
for fmt, path in paths.items():
    params = {"path": path, "format": fmt, "range": "entire", "wait": True}
    if fmt != "png":
        params["overwrite"] = True
    cmd("file.exportMedia", params)


def probe(path, entries):
    out = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", f"stream={entries}", "-of", "default=nw=1", path], capture_output=True, text=True).stdout
    return dict(line.split("=", 1) for line in out.strip().splitlines() if "=" in line)


mp4 = probe(paths["h264"], "codec_name,pix_fmt")
check(mp4.get("codec_name") == "h264" and subprocess.run(["ffmpeg", "-v", "error", "-i", paths["h264"], "-f", "null", "-"]).returncode == 0, "the MP4 is H.264 and decodes")
mov = probe(paths["prores"], "codec_name,profile,pix_fmt")
check(mov.get("profile") == "4444" and mov.get("pix_fmt", "").startswith("yuva"), f"the MOV is ProRes 4444 with alpha ({mov.get('pix_fmt')})")
check(subprocess.run(["ffmpeg", "-v", "error", "-i", paths["prores"], "-f", "null", "-"]).returncode == 0, "the MOV decodes")
frames = sorted(glob.glob(os.path.join(OUT, "e2e_frames*.png")))
total = cmd("sequence.inspect", {})["durationFrames"]
check(len(frames) == total, f"the PNG sequence has one file per frame ({len(frames)} of {total})")
shot("09-after-export")

call("app.quit", quiet=True)
if failures:
    print(f"\n{len(failures)} check(s) failed")
    sys.exit(1)
print("\nall checks passed; outputs in", OUT)
