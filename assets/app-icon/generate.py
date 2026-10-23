#!/usr/bin/env python3
"""Draws the WaltCut app icon: a bouncing ball with onion-skin ghosts, the classic animation
timing chart, on an ink-blue rounded square. Standard library only.

    python3 generate.py waltcut-1024.png 1024            # the full-bleed tile
    python3 generate.py waltcut-macos-1024.png 1024 --macos   # inside Apple's icon grid

The geometry is the same as waltcut.svg (a 1024 x 1024 canvas). Original work for this project.
"""
import math
import struct
import sys
import zlib

INK = (0x14, 0x1B, 0x2E)  # background
PAPER = (0xF4, 0xEF, 0xE6)  # ghosts and ground line
ORANGE = (0xFF, 0x8A, 0x3D)  # the current frame
HIGHLIGHT = (0xFF, 0xC2, 0x8F)

RADIUS = 224.0  # corner radius of the rounded square (macOS icon grid)
BALL_R = 62.0
GROUND_Y, GROUND_X0, GROUND_X1, GROUND_W = 804.0, 170.0, 854.0, 14.0
TOP_Y = 250.0  # ball centre at the top of the bounce
BOTTOM_Y = GROUND_Y - BALL_R - 9.0  # ball centre when it touches down
HALF_SPAN = 342.0
N_FRAMES = 9
# (x, alpha) of each frame: ghosts fade in toward the current frame, which lands on the right
FRAMES = []
for _i in range(N_FRAMES):
    _x = 512.0 - HALF_SPAN + 2.0 * HALF_SPAN * _i / (N_FRAMES - 1)
    FRAMES.append((_x, 1.0 if _i == N_FRAMES - 1 else 0.10 + 0.055 * _i))


def ball_y(x):
    t = (x - 512.0) / HALF_SPAN
    return TOP_Y + (BOTTOM_Y - TOP_Y) * t * t


def sd_round_rect(px, py, half, r):
    qx, qy = abs(px - 512.0) - (half - r), abs(py - 512.0) - (half - r)
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    return outside + min(max(qx, qy), 0.0) - r


def sd_circle(px, py, cx, cy, r):
    return math.hypot(px - cx, py - cy) - r


def sd_capsule(px, py, x0, x1, y, w):
    cx = min(max(px, x0), x1)
    return math.hypot(px - cx, py - y) - w / 2.0


def cover(d, scale):
    """Anti-aliased coverage from a signed distance in canvas units."""
    return min(max(0.5 - d * scale, 0.0), 1.0)


def blend(dst, rgb, a):
    # dst is premultiplied RGBA floats; `a` is the source's alpha
    inv = 1.0 - a
    return (rgb[0] * a + dst[0] * inv, rgb[1] * a + dst[1] * inv, rgb[2] * a + dst[2] * inv, a + dst[3] * inv)


def pixel(px, py, scale):
    ring = cover(sd_round_rect(px, py, 512.0, RADIUS), scale)
    if ring <= 0.0:
        return (0.0, 0.0, 0.0, 0.0)
    c = blend((0.0, 0.0, 0.0, 0.0), INK, 1.0)
    c = blend(c, PAPER, 0.55 * cover(sd_capsule(px, py, GROUND_X0, GROUND_X1, GROUND_Y, GROUND_W), scale))
    for x, alpha in FRAMES:
        y = ball_y(x)
        colour = ORANGE if alpha >= 1.0 else PAPER
        c = blend(c, colour, alpha * cover(sd_circle(px, py, x, y, BALL_R), scale))
    # a small highlight on the current ball
    x, y = FRAMES[-1][0], ball_y(FRAMES[-1][0])
    c = blend(c, HIGHLIGHT, 0.9 * cover(sd_circle(px, py, x - 21.0, y - 22.0, 16.0), scale))
    return tuple(v * ring for v in c)


GRID = 824.0 / 1024.0  # Apple's icon grid: the tile is 824 of 1024 px, centred in a transparent margin


def render(size, macos=False):
    scale = size / 1024.0
    k = GRID if macos else 1.0
    rows = []
    for j in range(size):
        py = ((j + 0.5) / scale - 512.0) / k + 512.0
        row = bytearray([0])
        for i in range(size):
            r, g, b, a = pixel(((i + 0.5) / scale - 512.0) / k + 512.0, py, scale * k)
            if a <= 0.0:
                row += b"\x00\x00\x00\x00"
            else:
                row += bytes((round(min(r / a, 255.0)), round(min(g / a, 255.0)), round(min(b / a, 255.0)), round(a * 255.0)))
        rows.append(bytes(row))
    return rows


def write_png(path, size, rows):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b""))


if __name__ == "__main__":
    out, size = sys.argv[1], int(sys.argv[2])
    write_png(out, size, render(size, macos="--macos" in sys.argv))
