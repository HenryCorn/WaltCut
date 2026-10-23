#!/usr/bin/env python3
"""End-to-end check of the retained animation workflow, driven through the control channel.

    cargo run --release -p waltcut -- --control 9876      # in one terminal
    python3 tools/e2e-animation-flow.py --out /tmp/waltcut-e2e

Steps: new project; import footage, an image sequence and audio; arrange on the timeline (trim,
split); draw with pen, pencil, marker and eraser; new, duplicate and held cels, in-between and
onion skin; preview and frame-step; export an H.264 MP4, a ProRes 4444 MOV and a PNG sequence and
check them with ffprobe. Screenshots of each stage go to <out>/shots. Needs ffmpeg and ffprobe.

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
cels = {c["frame"]: c["strokes"] for c in cmd("drawing.inspect", {"clip": drawing})["cels"]}
check(0 in cels and 4 in cels and 8 in cels and between["cels"] == [2], f"cels at frames {sorted(cels)} and an in-between at {between['cels']}")
state = call("ui.inspect")["result"]["ui"]["brush"]
check(state["onion"] is True, "onion skin is on")
call("ui.set", {"tool": "Brush"}, quiet=True)
shot("07-cels-onion-skin")

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
