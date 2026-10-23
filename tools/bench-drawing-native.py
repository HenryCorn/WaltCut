#!/usr/bin/env python3
"""Native drawing baselines (#70), driven through the control channel of the real desktop app.

    WALTCUT_DATA_DIR=$(mktemp -d) cargo run --release -p waltcut -- --control 9876   # one terminal
    python3 tools/bench-drawing-native.py --workload medium --trials 5                # another

Complements `cargo xtask bench --sections drawing` (headless: rasterisation, pen-up, scrub, play,
UI frame time) with what only the running app has: audio timing and dropped frames during real
playback (`perf.stats`), the app's own UI frame time while a synthetic pointer drag draws, the
round trip of a stroke command, and the app's resident memory (`ps`). It builds the same
reproducible workloads (fixed seed) over a generated reference video with audio.

A synthetic drag is not a pen: pen-to-photon latency needs a tablet and a camera and is NOT measured.
Needs ffmpeg. Standard library only. Prints a Markdown table and writes <out>/native-<workload>.json.
"""
import argparse
import json
import math
import os
import random
import socket
import subprocess
import sys
import time

WORKLOADS = {
    "small": dict(cels=12, strokes=20, points=40, hold=2),
    "medium": dict(cels=48, strokes=150, points=120, hold=2),
    "stress": dict(cels=96, strokes=1500, points=200, hold=1),
}
BRUSHES = ["Pen", "Pencil", "Marker"]

parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
parser.add_argument("--port", type=int, default=9876)
parser.add_argument("--workload", choices=WORKLOADS, default="medium")
parser.add_argument("--project", help="open this project (saved by the headless bench with WALTCUT_BENCH_SAVE=<dir>) instead of building the workload")
parser.add_argument("--trials", type=int, default=5, help="pen-up and drag trials")
parser.add_argument("--play-seconds", type=float, default=8.0)
parser.add_argument("--out", default=os.path.join(os.environ.get("TMPDIR", "/tmp"), "waltcut-bench-drawing"))
args = parser.parse_args()
os.makedirs(args.out, exist_ok=True)

sock = socket.create_connection(("127.0.0.1", args.port), timeout=300)
reader = sock.makefile("r")
_id = 0


def call(method, params=None):
    global _id
    _id += 1
    msg = {"id": _id, "method": method}
    if params is not None:
        msg["params"] = params
    sock.sendall((json.dumps(msg) + "\n").encode())
    reply = json.loads(reader.readline())
    if not reply.get("ok"):
        sys.exit(f"FAIL {method}: {reply}")
    return reply["result"]


def cmd(command, params=None):
    return call("engine.execute", {"command": command, "params": params or {}})


def app_pid():
    out = subprocess.run(["pgrep", "-n", "-f", f"waltcut.* --control {args.port}"], capture_output=True, text=True).stdout.split()
    return int(out[0]) if out else None


def rss_mb():
    pid = app_pid()
    if not pid:
        return None
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    return int(out) / 1024 if out else None


def pct(v, p):
    if not v:
        return None
    v = sorted(v)
    return v[min(len(v) - 1, int(round(p * (len(v) - 1))))]


def stroke(rng, w, h, n, i):
    brush = "Eraser" if i % 10 == 9 else BRUSHES[i % 3]
    x, y, a = rng.random() * w, rng.random() * h, rng.random() * math.tau
    pts = []
    for k in range(n):
        a += (rng.random() - 0.5) * 0.3
        x = min(max(x + math.cos(a) * 3, 0), w)
        y = min(max(y + math.sin(a) * 3, 0), h)
        pts.append([round(x, 2), round(y, 2), round(max(0.05, math.sin(math.pi * k / max(n, 1))), 3)])
    return {"brush": brush, "color": [rng.random(), rng.random(), rng.random(), 1.0], "size": 4 + rng.random() * 20, "points": pts}


wl = WORKLOADS[args.workload]
media = os.path.join(args.out, "media")
os.makedirs(media, exist_ok=True)
video = os.path.join(media, "ref1080.mp4")
if not os.path.exists(video):
    subprocess.run(
        ["ffmpeg", "-y", "-v", "error", "-f", "lavfi", "-i", "testsrc2=s=1920x1080:r=24000/1001:d=20,noise=alls=6:allf=t", "-f", "lavfi", "-i", "sine=f=440:d=20",
         "-c:v", "libx264", "-preset", "fast", "-crf", "20", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest", video],
        check=True,
    )

rss0 = rss_mb()
rng = random.Random(0x9E3779B97F4A7C15)
t0 = time.time()
if args.project:
    # the control channel caps a request line, so large workloads are loaded, not streamed
    cmd("file.open", {"path": args.project})
    clip = None
else:
    cmd("file.newProject")
    imp = cmd("file.import", {"paths": [video]})
    cmd("file.newSequenceFromClip", {"items": [imp["items"][0]]})
    dr = cmd("drawing.new", {"seconds": 20.0})
    clip = dr["clip"]
    for c in range(wl["cels"]):
        strokes = [stroke(rng, 1920, 1080, wl["points"], i) for i in range(wl["strokes"])]
        cmd("drawing.setStrokes", {"clip": clip, "frame": c * wl["hold"], "strokes": strokes})
build_s = time.time() - t0
rss_built = rss_mb()
print(f"built {wl['cels']} cels in {build_s:.1f} s; RSS {rss_built or 0:.0f} MB")

res = {"workload": args.workload, **wl, "build_s": build_s, "rss_before_mb": rss0, "rss_built_mb": rss_built}
mid = (wl["cels"] // 2) * wl["hold"]
cmd("playhead.set", {"frame": mid})
call("ui.set", {"tool": "Brush"})

# 1. pen-up: addStroke round trip through the control channel (parse + edit + undo snapshot)
rt = []
for _ in range(args.trials):
    st = stroke(rng, 1920, 1080, 150, 1)
    st["brush"] = "Pen"
    t = time.perf_counter()
    cmd("drawing.addStroke", {"cel": "hold", "stroke": st, **({"clip": clip} if clip else {})})
    rt.append((time.perf_counter() - t) * 1000)
    cmd("edit.undo")
res["addStroke_roundtrip_p50_ms"], res["addStroke_roundtrip_max_ms"] = pct(rt, 0.5), max(rt)

# 2. a synthetic pointer drag across the canvas (below the options bar); the app's UI frame time
#    during it. The control channel sends one pointer event per UI frame, so a 120-step drag is 120
#    frames. Each drag must add exactly one stroke, which is then undone.
def stroke_count():
    return sum(c["strokes"] for c in cmd("drawing.inspect", {})["cels"])


els = call("ui.elements", {"prefix": "program.brush"})
canvas = next((e["rect"] for e in els if e["id"] == "program.brush"), None)
drag_ms, drag_frames_ms, committed = [], [], 0
if canvas:
    x, y, w, h = canvas
    for k in range(args.trials):
        a = (x + w * 0.25, y + h * 0.62 + k * 3)
        b = (x + w * 0.75, y + h * 0.9 - k * 3)
        before = stroke_count()
        f0 = call("perf.stats")["ui"]
        t = time.perf_counter()
        call("ui.drag", {"from": {"x": a[0], "y": a[1]}, "to": {"x": b[0], "y": b[1]}, "steps": 120})
        drag_ms.append((time.perf_counter() - t) * 1000)
        drag_frames_ms.append(call("perf.stats")["ui"]["frameMs"])
        if stroke_count() == before + 1:
            committed += 1
            cmd("edit.undo")
res["drag_120_steps_p50_ms"] = pct(drag_ms, 0.5)
res["drag_ui_frame_ms_p50"] = pct(drag_frames_ms, 0.5)
res["drag_strokes_committed"] = committed
res["drag_trials"] = args.trials
if committed != args.trials:
    print(f"WARNING: only {committed} of {args.trials} drags drew a stroke")

# 3. playback from the start: dropped frames and audio timing from the app's own counters
cmd("playhead.set", {"frame": 0})
call("ui.playback", {"action": "stop"})
time.sleep(0.5)
p0 = call("perf.stats")
call("ui.playback", {"action": "play"})
peak = rss_mb() or 0
t_end = time.time() + args.play_seconds
while time.time() < t_end:
    time.sleep(0.5)
    peak = max(peak, rss_mb() or 0)
p1 = call("perf.stats")
call("ui.playback", {"action": "stop"})
res["playback"] = p1.get("playback")
res["playback_before"] = p0.get("playback")
res["rss_peak_play_mb"] = peak
res["cpu_s_during_play"] = (p1.get("process", {}).get("cpuS", 0) or 0) - (p0.get("process", {}).get("cpuS", 0) or 0)
res["frames"] = p1.get("frames")
res["rss_end_mb"] = rss_mb()

path = os.path.join(args.out, f"native-{args.workload}.json")
with open(path, "w") as f:
    json.dump(res, f, indent=2)
print(json.dumps(res, indent=2)[:3000])
print("wrote", path)
