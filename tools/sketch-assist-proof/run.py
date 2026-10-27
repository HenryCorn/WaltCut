#!/usr/bin/env python3
"""Sketch Assist proof kit for issue #77: a cost-capped, confirm-every-upload runner for fal.ai.

Developer tool, Python 3 standard library only. Not product code and not linked into WaltCut.
NOTHING in this file has been run against the real fal.ai service: see docs/sketch-assist-proof.md.

Subcommands: plan, run, report, spent.  Read the safety rules in docs/sketch-assist-proof.md.
"""
import argparse
import base64
import datetime
import hashlib
import html
import json
import math
import os
import re
import struct
import zlib
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from decimal import Decimal, InvalidOperation
from pathlib import Path

# ---------------------------------------------------------------- constants

DEFAULT_BASE_URL = "https://queue.fal.run"  # the real host; FAL_BASE_URL overrides (tests only)
DEFAULT_CAP = Decimal("3.50")  # owner-approved total (#77 owner decisions comment, 2026-10-27)
MAX_CAP = Decimal("5.00")  # the owner's whole budget; anything above is refused
MAX_PX = 4096
MAX_BYTES = 20 * 1024 * 1024
PRICE_DATE = "2026-10-26"  # when the prices below were checked (docs/sketch-assist-backends.md section 2)
LIFECYCLE_SECONDS = 3600  # short retention requested for output/CDN files
DEFAULT_STYLE = "storybook, flat cartoon, ink line, cut-out paper"

PRIVACY_DOCS = {
    "retention": "https://fal.ai/docs/model-apis/faq (generated media is on the fal CDN 'at least 7 days by default'; "
    "URLs are public to anyone with the link until they expire)",
    "lifecycle": "https://fal.ai/docs/model-apis/media-expiration (header X-Fal-Object-Lifecycle-Preference, JSON "
    '{"expiration_duration_seconds": N}; X-Fal-Store-IO: 0 stops inputs/outputs being kept in request history, '
    "default 90 days; that page does not say whether inline data-URI inputs are stored)",
    "privacy": "https://fal.ai/privacy (whether fal trains on inputs was NOT verified: read it before the first upload)",
    "queue": "https://fal.ai/docs/model-apis/model-endpoints/queue (submit, status, result, PUT cancel)",
}


class Refusal(Exception):
    """A safety rule said no. Message is safe to print (already redacted by the caller)."""


# ---------------------------------------------------------------- secrets

_SECRETS = set()


def register_secret(s):
    if s:
        _SECRETS.add(s)


def redact(text):
    text = str(text)
    for s in sorted(_SECRETS, key=len, reverse=True):
        text = text.replace(s, "[REDACTED]")
    text = re.sub(r"(?i)(authorization\s*[:=]\s*)(key\s+)?\S+", r"\1[REDACTED]", text)
    return text


# ---------------------------------------------------------------- images

class Loaded:
    """An explicitly named image file, read once. Holds bytes, size, dimensions, hash."""

    def __init__(self, role, path):
        self.role = role
        p = Path(path)
        if p.is_dir():
            raise Refusal(f"{role}: {path} is a directory; only individual files passed on the command line are read")
        if not p.is_file():
            raise Refusal(f"{role}: {path} is not a file")
        size = p.stat().st_size
        if size > MAX_BYTES:
            raise Refusal(f"{role}: {p.name} is {size} bytes; the limit is {MAX_BYTES} (20 MB)")
        if size == 0:
            raise Refusal(f"{role}: {p.name} is empty")
        data = p.read_bytes()
        self.path = str(p.resolve())
        self.name = p.name
        self.data = data
        self.size = len(data)
        self.sha256 = hashlib.sha256(data).hexdigest()
        self.mime, self.width, self.height = sniff_image(data, p.name, role)
        if self.width > MAX_PX or self.height > MAX_PX:
            raise Refusal(f"{role}: {p.name} is {self.width}x{self.height}; the limit is {MAX_PX} px on a side")
        if self.width < 1 or self.height < 1:
            raise Refusal(f"{role}: {p.name} has an invalid size")

    def data_uri(self):
        return f"data:{self.mime};base64," + base64.b64encode(self.data).decode("ascii")

    def describe(self):
        return f"{self.name} ({self.width}x{self.height} px, {self.mime}, {self.size} bytes, sha256 {self.sha256[:16]}...)"

    def ledger(self):
        return {"role": self.role, "name": self.name, "sha256": self.sha256, "bytes": self.size}


def sniff_image(data, name, role):
    """Return (mime, width, height) for PNG or JPEG, else refuse. Header parsing only; never raises other errors."""
    try:
        if data[:8] == b"\x89PNG\r\n\x1a\n" and data[12:16] == b"IHDR":
            w, h = struct.unpack(">II", data[16:24])
            return "image/png", w, h
        if data[:2] == b"\xff\xd8":
            i = 2
            while i + 9 < len(data):
                if data[i] != 0xFF:
                    i += 1
                    continue
                marker = data[i + 1]
                if marker in (0xD8, 0x01) or 0xD0 <= marker <= 0xD7 or marker == 0xFF:
                    i += 1 if marker == 0xFF else 2
                    continue
                (seglen,) = struct.unpack(">H", data[i + 2 : i + 4])
                if marker in (0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF):
                    h, w = struct.unpack(">HH", data[i + 5 : i + 9])
                    return "image/jpeg", w, h
                i += 2 + max(seglen, 2)
    except (struct.error, IndexError):
        pass
    raise Refusal(f"{role}: {name} is not a readable PNG or JPEG (only those two are accepted)")


class ImageRef:
    """Placeholder in a request body; replaced by a data URI when sending, by a description when displayed."""

    def __init__(self, loaded):
        self.loaded = loaded


def render_body(obj, inline):
    if isinstance(obj, ImageRef):
        return obj.loaded.data_uri() if inline else f"<inline data URI of {obj.loaded.describe()}>"
    if isinstance(obj, dict):
        return {k: render_body(v, inline) for k, v in obj.items()}
    if isinstance(obj, list):
        return [render_body(v, inline) for v in obj]
    return obj


# ---------------------------------------------------------------- recipes and prices

def megapixels_up(w, h):
    return max(1, math.ceil(Decimal(w * h) / Decimal(1_000_000)))


RECIPES = {
    # slug: endpoint, price rule text, estimator, experimental flag, shape note
    "flux-fill": dict(
        endpoint="fal-ai/flux-pro/v1/fill",
        price_text="$0.05 per megapixel, rounded up (fal model page, checked 2026-10-26)",
        est=lambda w, h: Decimal("0.05") * megapixels_up(w, h),
        experimental=False,
        shape="documented: prompt, image_url, mask_url (same dimensions), output_format, safety_tolerance, sync_mode "
        "(https://fal.ai/models/fal-ai/flux-pro/v1/fill/api). Mask polarity (which colour is regenerated) is NOT stated "
        "on that page: check the first result.",
    ),
    "kontext": dict(
        endpoint="fal-ai/flux-pro/kontext",
        price_text="$0.04 per image on its model page, $0.055 per MP on the Fill page (they disagree); the planner uses "
        "the higher figure, rounded up per MP (checked 2026-10-26)",
        est=lambda w, h: Decimal("0.055") * megapixels_up(w, h),
        experimental=False,
        shape="documented: prompt, image_url (one image), guidance_scale, output_format, safety_tolerance, sync_mode "
        "(https://fal.ai/models/fal-ai/flux-pro/kontext/api). One image only, no mask.",
    ),
    "nano-banana": dict(
        endpoint="fal-ai/nano-banana/edit",
        price_text="$0.0398 per image (fal model page, checked 2026-10-26)",
        est=lambda w, h: Decimal("0.0398"),
        experimental=False,
        shape="documented: prompt, image_urls (list), aspect_ratio, output_format, safety_tolerance, sync_mode, "
        "limit_generations (https://fal.ai/models/fal-ai/nano-banana/edit/api). Cheap control, no mask.",
    ),
    "flux-general": dict(
        endpoint="fal-ai/flux-general",
        price_text="$0.075 per megapixel, rounded up (fal model page, checked 2026-10-26)",
        est=lambda w, h: Decimal("0.075") * megapixels_up(w, h),
        experimental=True,
        shape="EXPERIMENTAL: field names come from the schema on https://fal.ai/models/fal-ai/flux-general/api but no "
        "official example combines sketch control, reference image and mask; you must supply Hugging Face repo paths "
        "(--controlnet-union-path etc.) because none are documented as defaults. Off by default.",
    ),
}
ORDER = ["flux-fill", "kontext", "nano-banana", "flux-general"]

SCENARIOS = {
    "single-enhance": "one rough sketch -> one finished image (needs --sketch)",
    "additive-edit": "approved image + new stroke/mask -> same picture with only the addition "
    "(needs --approved, --prompt, and --mask and/or --sketch where --sketch is the approved image with the new stroke drawn on)",
    "two-poses": "pose A approved as the reference, pose B rough sketch -> same dog in pose B (needs --reference, --sketch)",
}


class Call:
    def __init__(self, idx, slug, body, inputs, est, notes, skip=None):
        self.idx = idx
        self.slug = slug
        self.recipe = RECIPES[slug]
        self.endpoint = self.recipe["endpoint"]
        self.body = body
        self.inputs = inputs  # list of Loaded, in order, unique
        self.est = est
        self.notes = notes
        self.skip = skip  # reason string if not runnable
        self.name = f"{idx:02d}-{slug}"

    def headers_extra(self):
        return {
            "X-Fal-Object-Lifecycle-Preference": json.dumps({"expiration_duration_seconds": LIFECYCLE_SECONDS}),
            "X-Fal-Store-IO": "0",
        }


def build_calls(args):
    sc = args.scenario
    L = {}

    def need(role, path):
        if not path:
            raise Refusal(f"scenario {sc} needs --{role}")
        L[role] = Loaded(role, path)
        return L[role]

    style = args.style
    common = {"num_images": 1, "sync_mode": True}
    calls = []

    def add(slug, body, inputs, notes, skip=None, dims=None):
        w, h = dims
        est = RECIPES[slug]["est"](w, h)
        uniq = []
        for i in inputs:
            if all(i is not u for u in uniq):
                uniq.append(i)
        calls.append(Call(len(calls) + 1, slug, body, uniq, est, notes, skip))

    fg_missing = []
    if not args.controlnet_union_path:
        fg_missing.append("--controlnet-union-path")
    fg_skip = ("missing " + ", ".join(fg_missing)) if fg_missing else None

    def flux_general(prompt, control, reference, mask, dims):
        body = dict(
            prompt=prompt,
            image_size={"width": dims[0], "height": dims[1]},
            output_format="png",
            **common,
        )
        if args.controlnet_union_path:
            body["controlnet_unions"] = [
                {
                    "path": args.controlnet_union_path,
                    "controls": [
                        {
                            "control_image_url": ImageRef(control),
                            "control_mode": args.control_mode,
                            "conditioning_scale": args.conditioning_scale,
                        }
                    ],
                }
            ]
        if reference is not None:
            body["reference_image_url"] = ImageRef(reference)
            body["reference_strength"] = args.reference_strength
        if args.ip_adapter_path and args.ip_adapter_encoder_path and reference is not None:
            ip = {
                "path": args.ip_adapter_path,
                "image_encoder_path": args.ip_adapter_encoder_path,
                "image_url": ImageRef(reference),
                "scale": args.ip_scale,
            }
            if mask is not None:
                ip["mask_image_url"] = ImageRef(mask)
            body["ip_adapters"] = [ip]
        return body

    if sc == "single-enhance":
        sk = need("sketch", args.sketch)
        prompt = args.prompt or f"A walking dog, {style}. Follow the rough sketch loosely; clean lines, plain light background."
        d = (sk.width, sk.height)
        add("kontext", dict(prompt=prompt, image_url=ImageRef(sk), output_format="png", safety_tolerance="2", **common),
            [sk], "the sketch is the single image to be edited", dims=d)
        add("nano-banana", dict(prompt=prompt, image_urls=[ImageRef(sk)], output_format="png", aspect_ratio="auto", **common),
            [sk], "cheap control", dims=d)
        add("flux-general", flux_general(prompt, sk, None, None, d), [sk],
            "sketch as control image only", skip=fg_skip, dims=d)
    elif sc == "additive-edit":
        ap = need("approved", args.approved)
        if not args.prompt:
            raise Refusal("additive-edit needs --prompt (what the new stroke is, for example 'a red collar')")
        if not args.mask and not args.sketch:
            raise Refusal("additive-edit needs --mask and/or --sketch")
        mk = Loaded("mask", args.mask) if args.mask else None
        sk = Loaded("sketch", args.sketch) if args.sketch else None
        d = (ap.width, ap.height)
        for other in (mk, sk):
            if other is not None and (other.width, other.height) != d:
                raise Refusal(f"{other.role} is {other.width}x{other.height} but the approved image is {d[0]}x{d[1]}; they must match")
        edit_prompt = (f"Add this to the picture: {args.prompt}. Keep everything else exactly the same: same lines, "
                       "colours, shapes and background. Do not redraw the rest.")
        src = sk or ap
        fill_skip = None if mk else "needs --mask (white = the area to regenerate: verify polarity on the first result)"
        add("flux-fill", dict(prompt=f"{args.prompt}, {style}, matching the rest of the picture",
                              image_url=ImageRef(ap), mask_url=ImageRef(mk if mk else ap),
                              output_format="png", safety_tolerance="2", sync_mode=True, num_images=1),
            [ap] + ([mk] if mk else []), "approved image + mask; unmasked area should stay put", skip=fill_skip, dims=d)
        add("kontext", dict(prompt=edit_prompt, image_url=ImageRef(src), output_format="png", safety_tolerance="2", **common),
            [src], "no mask: shows how much the rest drifts", dims=d)
        add("nano-banana", dict(prompt=edit_prompt, image_urls=[ImageRef(src)], output_format="png", aspect_ratio="auto", **common),
            [src], "cheap control, no mask", dims=d)
        add("flux-general", flux_general(f"{args.prompt}, {style}", src, ap, mk, d), [src, ap] + ([mk] if mk else []),
            "sketch+reference+region in one call", skip=fg_skip, dims=d)
    elif sc == "two-poses":
        rf = need("reference", args.reference)
        sk = need("sketch", args.sketch)
        pose = args.prompt or "walking, with the legs in the pose of the rough sketch"
        d = (sk.width, sk.height)
        add("kontext",
            dict(prompt=f"The same dog, {style}, same colours, markings and line style, now {pose}.",
                 image_url=ImageRef(rf), output_format="png", safety_tolerance="2", **common),
            [rf], "ONE image only: the approved pose; pose B is described in words, the sketch is NOT sent", dims=d)
        add("nano-banana",
            dict(prompt=("The first image is the approved character design. Redraw that exact same dog (same shape, colours, "
                         f"markings, line style, {style}) in the pose shown by the second rough sketch. Keep the style of the first image."),
                 image_urls=[ImageRef(rf), ImageRef(sk)], output_format="png", aspect_ratio="auto", **common),
            [rf, sk], "two images: reference first, rough sketch second", dims=d)
        add("flux-general", flux_general(f"A dog, {style}, {pose}", sk, rf, None, d), [rf, sk],
            "sketch as control, approved pose as reference_image_url", skip=fg_skip, dims=d)
    else:
        raise Refusal(f"unknown scenario {sc}")

    for c in calls:
        if c.recipe["experimental"] and not args.include_experimental and not c.skip:
            c.skip = "EXPERIMENTAL, off by default: add --include-experimental to enable"
        elif c.recipe["experimental"] and c.skip and not args.include_experimental:
            c.skip = "EXPERIMENTAL, off by default (and " + c.skip + ")"
    if args.only:
        wanted = {s.strip() for s in args.only.split(",") if s.strip()}
        unknown = wanted - set(RECIPES)
        if unknown:
            raise Refusal(f"--only: unknown recipe {sorted(unknown)}; choose from {ORDER}")
        for c in calls:
            if c.slug not in wanted:
                c.skip = "not selected by --only"
            elif c.recipe["experimental"] and not args.include_experimental:
                c.skip = "EXPERIMENTAL, off by default: add --include-experimental to enable"
    return calls


# ---------------------------------------------------------------- ledger

class Ledger:
    """JSON lines, append only. A record is written before the network call and a later record with the
    same id updates it, so a crash leaves the spend visible."""

    def __init__(self, path):
        self.path = Path(path)

    def append(self, rec):
        rec = json.loads(redact(json.dumps(rec)))  # defence in depth: never persist a secret
        rec["ts"] = datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with open(self.path, "a", encoding="utf-8") as f:
            f.write(json.dumps(rec, sort_keys=True) + "\n")
            f.flush()
            os.fsync(f.fileno())

    def entries(self):
        merged = {}
        order = []
        if not self.path.exists():
            return []
        for line in self.path.read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                raise Refusal(f"ledger {self.path} has a corrupt line; refusing to run until it is fixed (spend cannot be known)")
            i = rec.get("id")
            if i not in merged:
                merged[i] = {}
                order.append(i)
            merged[i].update(rec)
        return [merged[i] for i in order]

    def spent(self):
        total = Decimal("0")
        for e in self.entries():
            if e.get("state") == "declined":
                continue  # never sent
            try:
                total += Decimal(str(e.get("est_cost_usd", "0")))
            except InvalidOperation:
                raise Refusal("ledger has an unreadable cost; refusing to run")
        return total


# ---------------------------------------------------------------- http

class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *a, **k):
        return None


_OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}), _NoRedirect)


class NetError(Exception):
    pass


def http(method, url, headers, body=None, timeout=60):
    req = urllib.request.Request(url, data=body, method=method, headers=headers)
    try:
        with _OPENER.open(req, timeout=timeout) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        try:
            return e.code, e.read()
        finally:
            e.close()
    except (urllib.error.URLError, OSError, TimeoutError) as e:
        raise NetError(redact(f"{type(e).__name__}: {e}"))


_sleep = time.sleep
_now = time.monotonic


def host_is_fal(url):
    h = (urllib.parse.urlparse(url).hostname or "").lower()
    return h == "fal.run" or h.endswith(".fal.run")


def host_is_loopback(url):
    return (urllib.parse.urlparse(url).hostname or "").lower() in ("127.0.0.1", "localhost", "::1")


# ---------------------------------------------------------------- disclosure and run

class IO:
    def __init__(self, env=None, input_fn=input, out=None, err=None):
        self.env = os.environ if env is None else env
        self.input_fn = input_fn
        self.out = out or sys.stdout
        self.err = err or sys.stderr

    def p(self, s=""):
        print(redact(s), file=self.out)

    def e(self, s=""):
        print(redact(s), file=self.err)


def fmt_usd(d):
    return f"${Decimal(d).quantize(Decimal('0.0001'))}"


def disclosure(call, spent, cap, base_url, live):
    lines = []
    lines.append("=" * 72)
    lines.append(f"CALL {call.name}: {call.endpoint}   {'(EXPERIMENTAL)' if call.recipe['experimental'] else ''}")
    lines.append(f"  sends to : {base_url}/{call.endpoint}   ({'LIVE' if live else 'DRY RUN: nothing is sent'})")
    lines.append(f"  estimate : {fmt_usd(call.est)}   price rule: {call.recipe['price_text']}  [price date {PRICE_DATE}]")
    lines.append(f"  spend    : {fmt_usd(spent)} so far + {fmt_usd(call.est)} = {fmt_usd(spent + call.est)} of cap {fmt_usd(cap)}")
    lines.append("  FILES that will be uploaded (read from disk, sent inline):")
    for i in call.inputs:
        lines.append(f"    - [{i.role}] {i.path}")
        lines.append(f"        {i.width}x{i.height} px, {i.mime}, {i.size} bytes, sha256 {i.sha256}")
    lines.append("  PARAMETERS (exact request body; inline images shown as placeholders):")
    for l in json.dumps(render_body(call.body, False), indent=2).splitlines():
        lines.append("    " + l)
    lines.append("  EXTRA HEADERS: " + json.dumps(call.headers_extra()))
    lines.append("  PRIVACY:")
    lines.append("    - Images are sent INLINE as base64 data URIs in the request body; this tool does not upload them to a URL.")
    lines.append("    - Requested: output files expire after %d s (X-Fal-Object-Lifecycle-Preference), sync_mode true "
                 "(output returned inline), X-Fal-Store-IO: 0 (not kept in request history)." % LIFECYCLE_SECONDS)
    lines.append("    - Without those, fal's media URLs are public to anyone with the link for at least 7 days by default.")
    lines.append("    - NOT verified: whether fal stores inline inputs anywhere, or trains on inputs. " + PRIVACY_DOCS["privacy"])
    lines.append("    - Sources: " + PRIVACY_DOCS["retention"])
    lines.append("               " + PRIVACY_DOCS["lifecycle"])
    lines.append(f"  NOTES: {call.notes}")
    lines.append(f"  SHAPE: {call.recipe['shape']}")
    return "\n".join(lines)


def parse_cap(s):
    try:
        cap = Decimal(str(s))
    except InvalidOperation:
        raise Refusal(f"--cap {s!r} is not a number")
    if not cap.is_finite() or cap <= 0:
        raise Refusal("--cap must be a positive number")
    if cap > MAX_CAP:
        raise Refusal(f"--cap {cap} is above the {MAX_CAP} USD total budget; refused")
    return cap


def base_url_of(io):
    return (io.env.get("FAL_BASE_URL") or DEFAULT_BASE_URL).rstrip("/")


def run_dir_of(args):
    return Path(args.workdir).expanduser() / "runs" / args.run


def check_outputs_free(run_dir, calls):
    for c in calls:
        if c.skip:
            continue
        if run_dir.exists():
            clash = [p.name for p in run_dir.iterdir() if p.name.startswith(c.name + ".") or p.name.startswith(c.name + "-")]
            if clash:
                raise Refusal(f"would overwrite existing outputs in {run_dir}: {', '.join(sorted(clash))}. Use a new --run name.")


def default_run(args):
    if not args.run:
        args.run = f"{args.scenario}-{datetime.datetime.now().strftime('%Y%m%d-%H%M%S')}"
    if not re.fullmatch(r"[A-Za-z0-9._-]{1,80}", args.run) or args.run in (".", ".."):
        raise Refusal("--run may contain only letters, digits, . _ - (max 80)")


def cmd_plan(args, io):
    default_run(args)
    calls = build_calls(args)
    cap = parse_cap(args.cap)
    ledger = Ledger(Path(args.workdir).expanduser() / "ledger.jsonl")
    spent = ledger.spent()
    io.p(f"PLAN for scenario {args.scenario}: {SCENARIOS[args.scenario]}")
    io.p(f"Prices below were checked on {PRICE_DATE} (fal model pages); they are estimates, check your fal dashboard for actual billing.")
    total = Decimal("0")
    for c in calls:
        state = "WILL RUN" if not c.skip else f"SKIPPED: {c.skip}"
        io.p(f"\n[{c.name}] {c.endpoint}  {'(EXPERIMENTAL) ' if c.recipe['experimental'] else ''}{state}")
        io.p(f"  estimate {fmt_usd(c.est)}  ({c.recipe['price_text']})")
        io.p(f"  shape: {c.recipe['shape']}")
        io.p(f"  files: " + "; ".join(i.describe() for i in c.inputs))
        io.p("  parameters: " + json.dumps(render_body(c.body, False), sort_keys=True))
        if not c.skip:
            total += c.est
    io.p(f"\nEstimated total for the calls that will run: {fmt_usd(total)}; already spent (ledger): {fmt_usd(spent)}; cap {fmt_usd(cap)}.")
    if spent + total > cap:
        io.p("WARNING: the whole plan does not fit under the cap; later calls will be refused.")
    return 0


def save_call_record(run_dir, call, rec):
    run_dir.mkdir(parents=True, exist_ok=True)
    path = run_dir / f"{call.name}.call.json"
    tmp = run_dir / f".{call.name}.call.json.tmp"
    tmp.write_text(redact(json.dumps(rec, indent=2, sort_keys=True)), encoding="utf-8")
    os.replace(tmp, path)


def decode_output(url, timeout):
    """Return (bytes, ext). Data URIs are decoded locally; http(s) URLs are fetched WITHOUT the key."""
    if url.startswith("data:"):
        m = re.match(r"data:([\w/+.-]+);base64,(.*)$", url, re.S)
        if not m:
            raise NetError("output data URI is not base64")
        mime = m.group(1)
        data = base64.b64decode(m.group(2), validate=False)
    else:
        status, data = http("GET", url, {}, timeout=timeout)
        if status != 200:
            raise NetError(f"output download returned HTTP {status}")
        mime = "image/png" if data[:4] == b"\x89PNG" else "image/jpeg" if data[:2] == b"\xff\xd8" else "application/octet-stream"
    ext = {"image/png": "png", "image/jpeg": "jpg", "image/webp": "webp"}.get(mime, "bin")
    return data, ext


def exec_call(call, args, io, ledger, run_dir, base_url, cap):
    """Run one confirmed call. Returns 'done', 'failed', 'cancelled', 'timeout' or 'declined'."""
    key = io.env.get("FAL_KEY", "")
    lid = uuid.uuid4().hex
    t_budget = float(args.timeout)
    body = json.dumps(render_body(call.body, True)).encode("utf-8")
    rec = {
        "id": lid, "state": "sending", "run": args.run, "scenario": args.scenario, "slug": call.slug,
        "endpoint": call.endpoint, "est_cost_usd": str(call.est), "price_date": PRICE_DATE,
        "inputs": [i.ledger() for i in call.inputs], "base_url": base_url,
    }
    call_rec = {
        "call": call.name, "slug": call.slug, "endpoint": call.endpoint, "run": args.run, "scenario": args.scenario,
        "est_cost_usd": str(call.est), "price_date": PRICE_DATE, "state": "sending", "notes": call.notes,
        "request": render_body(call.body, False), "extra_headers": call.headers_extra(),
        "inputs": [dict(i.ledger(), path=i.path) for i in call.inputs], "outputs": [],
    }
    # Written BEFORE the network call so that a crash cannot hide spend.
    ledger.append(rec)
    save_call_record(run_dir, call, call_rec)
    (run_dir / f"{call.name}.request.json").write_text(json.dumps(call_rec["request"], indent=2), encoding="utf-8")

    t0 = _now()
    headers = {"Authorization": f"Key {key}", "Content-Type": "application/json", **call.headers_extra()}
    auth = {"Authorization": f"Key {key}"}
    state, err, request_id, urls = "failed", None, None, {}

    def finish(st, **extra):
        call_rec.update(state=st, latency_s=round(_now() - t0, 3), **extra)
        if err:
            call_rec["error"] = redact(err)
        ledger.append(dict(id=lid, state=st, latency_s=call_rec["latency_s"], request_id=request_id,
                           **({"error": redact(err)} if err else {}), **{k: v for k, v in extra.items() if k == "cancel_http"}))
        save_call_record(run_dir, call, call_rec)
        return st

    def remaining():
        return t_budget - (_now() - t0)

    def derive(kind):
        if kind in urls and urls[kind].startswith(base_url + "/"):
            return urls[kind]
        owner_app = "/".join(call.endpoint.split("/")[:2])
        suffix = {"status": "/status", "cancel": "/cancel", "result": ""}[kind]
        return f"{base_url}/{owner_app}/requests/{request_id}{suffix}"

    def cancel():
        try:
            code, _ = http("PUT", derive("cancel"), auth, timeout=20)
            return code
        except NetError as e:
            return f"error: {e}"

    try:
        try:
            code, resp = http("POST", f"{base_url}/{call.endpoint}", headers, body, timeout=max(5, min(120, remaining())))
        except NetError as e:
            err = str(e)
            return finish("failed")
        if code != 200:
            err = f"submit returned HTTP {code}: {resp[:300].decode('utf-8', 'replace')}"
            return finish("failed")
        try:
            sub = json.loads(resp)
            request_id = sub["request_id"]
        except (ValueError, KeyError, TypeError):
            err = "submit response had no request_id"
            return finish("failed")
        urls = {"status": sub.get("status_url", ""), "cancel": sub.get("cancel_url", ""), "result": sub.get("response_url", "")}
        ledger.append(dict(id=lid, state="submitted", request_id=request_id))
        io.p(f"  submitted, request_id {request_id}; waiting (Ctrl-C cancels this request)")
        while True:
            if remaining() <= 0:
                code = cancel()
                err = f"timed out after {args.timeout} s; cancel request answered {code}"
                return finish("timeout", cancel_http=str(code))
            code, resp = http("GET", derive("status"), auth, timeout=max(5, min(60, remaining())))
            # fal answers 202 while a request is still queued or running; only other codes are failures
            if code not in (200, 202):
                err = f"status returned HTTP {code}"
                return finish("failed")
            try:
                st = json.loads(resp).get("status")
            except (ValueError, AttributeError):
                err = "status response unreadable"
                return finish("failed")
            if st == "COMPLETED":
                break
            _sleep(args.poll_interval)
        code, resp = http("GET", derive("result"), auth, timeout=max(5, min(120, remaining())))
        if code != 200:
            err = f"result returned HTTP {code}: {resp[:300].decode('utf-8', 'replace')}"
            return finish("failed")
        try:
            result = json.loads(resp)
            images = result["images"]
        except (ValueError, KeyError, TypeError):
            err = "result had no images (it may have been refused by the safety checker or failed)"
            return finish("failed")
        saved = []
        for k, im in enumerate(images[:4]):
            data, ext = decode_output(im["url"], 60)
            suffix = "" if k == 0 else f"-{k + 1}"
            out = run_dir / f"{call.name}{suffix}.{ext}"
            if out.exists():
                err = f"refusing to overwrite {out.name}"
                return finish("failed")
            out.write_bytes(data)
            saved.append(out.name)
        slim = json.loads(json.dumps(result))
        for im in slim.get("images", []):
            if isinstance(im, dict) and str(im.get("url", "")).startswith("data:"):
                im["url"] = "<data URI saved to file>"
        (run_dir / f"{call.name}.response.json").write_text(redact(json.dumps(slim, indent=2)), encoding="utf-8")
        call_rec["outputs"] = saved
        return finish("done")
    except KeyboardInterrupt:
        if request_id:
            code = cancel()
            err = f"interrupted by user; cancel request answered {code} (200 removed, 202 may still run and be billed)"
            io.e("\nInterrupted. " + err)
            finish("cancelled", cancel_http=str(code))
        else:
            err = "interrupted before fal returned a request id; nothing to cancel, but the call may still run"
            io.e("\nInterrupted. " + err)
            finish("cancelled")
        raise
    except NetError as e:
        err = str(e)
        return finish("failed")
    except Exception as e:  # never leak a key through an unexpected error
        err = f"{type(e).__name__}: {e}"
        return finish("failed")


def cmd_run(args, io):
    default_run(args)
    if args.live and args.dry_run:
        raise Refusal("--live and --dry-run are mutually exclusive")
    live = bool(args.live)
    cap = parse_cap(args.cap)
    if cap > DEFAULT_CAP:
        io.e(f"WARNING: cap {cap} is above the owner-approved {DEFAULT_CAP}.")
    calls = build_calls(args)
    run_dir = run_dir_of(args)
    check_outputs_free(run_dir, calls)
    base_url = base_url_of(io)
    ledger = Ledger(Path(args.workdir).expanduser() / "ledger.jsonl")
    if live:
        key = io.env.get("FAL_KEY", "")
        if not key:
            raise Refusal("live mode needs the FAL_KEY environment variable (it is never accepted on the command line)")
        if not re.fullmatch(r"[\x21-\x7e]+", key):
            raise Refusal("FAL_KEY contains spaces or unusual characters; check it was copied correctly")
        register_secret(key)
        parsed = urllib.parse.urlparse(base_url)
        if parsed.scheme not in ("http", "https") or not parsed.hostname:
            raise Refusal("FAL_BASE_URL is not a valid http(s) URL")
        if parsed.scheme != "https" and not host_is_loopback(base_url):
            raise Refusal("FAL_BASE_URL must be https unless it is a loopback address")
        if not host_is_fal(base_url):
            io.e("!" * 72)
            io.e(f"WARNING: FAL_BASE_URL points at {parsed.hostname}, which is NOT fal.ai. Your FAL_KEY and your")
            io.e("images will be sent THERE. This is only correct for local tests with a fake server.")
            io.e("!" * 72)
    runnable = [c for c in calls if not c.skip]
    if not runnable:
        for c in calls:
            io.p(f"[{c.name}] {c.endpoint}: SKIPPED: {c.skip}")
        raise Refusal("nothing to run (see the skipped reasons above)")
    io.p("DRY RUN: nothing is sent, nothing is written, no key is read." if not live
         else "LIVE: each call below needs you to type y. Nothing is sent without it.")
    for c in calls:
        if c.skip:
            io.p(f"[{c.name}] {c.endpoint}: SKIPPED: {c.skip}")
    ran = 0
    sim = Decimal("0")  # dry-run only: pretend earlier calls were sent so the cap check is realistic
    for c in runnable:
        spent = ledger.spent() + sim
        if spent + c.est > cap:
            msg = f"REFUSED {c.name}: estimated spent {fmt_usd(spent)} + {fmt_usd(c.est)} would exceed the cap {fmt_usd(cap)}."
            if live:
                io.p(msg)
                raise Refusal("spend cap reached; no further calls will be made")
            io.p(msg + " (a live run would stop here)")
            break
        io.p(disclosure(c, spent, cap, base_url, live))
        if not live:
            sim += c.est
            continue
        try:
            ans = io.input_fn(f"Type y to SEND call {c.name} now (anything else skips it): ")
        except (EOFError, KeyboardInterrupt):
            io.p("\nNo answer: not sent. Stopping.")
            return 1
        if ans.strip() != "y":
            io.p(f"Declined: {c.name} was NOT sent.")
            ledger.append({"id": uuid.uuid4().hex, "state": "declined", "run": args.run, "slug": c.slug,
                           "endpoint": c.endpoint, "est_cost_usd": "0", "inputs": [i.ledger() for i in c.inputs]})
            continue
        try:
            st = exec_call(c, args, io, ledger, run_dir, base_url, cap)
        except KeyboardInterrupt:
            io.p("Stopped by Ctrl-C. The ledger records the cancelled call and keeps its estimated cost counted.")
            return 130
        ran += 1
        io.p(f"  result: {st}   (total estimated spend now {fmt_usd(ledger.spent())})")
    if live:
        io.p(f"\nDone. Run folder: {run_dir}\nNext: python3 run.py report --run {args.run}")
    else:
        io.p("\nDry run finished. To send for real: same command with --live (and FAL_KEY set).")
    return 0


def cmd_spent(args, io):
    ledger = Ledger(Path(args.workdir).expanduser() / "ledger.jsonl")
    entries = ledger.entries()
    cap = parse_cap(args.cap)
    spent = ledger.spent()
    counts = {}
    for e in entries:
        counts[e.get("state", "?")] = counts.get(e.get("state", "?"), 0) + 1
    io.p(f"Estimated spend (list prices {PRICE_DATE}, every sent call counted, cancelled ones too): {fmt_usd(spent)}")
    io.p(f"Cap {fmt_usd(cap)}; remaining {fmt_usd(cap - spent)}. Calls by state: {json.dumps(counts, sort_keys=True)}")
    io.p("This is an ESTIMATE. The real bill is on your fal dashboard; compare them and note the difference in #77.")
    return 0


# ---------------------------------------------------------------- report

def img_tag(path_or_bytes, label, mime=None):
    if isinstance(path_or_bytes, (str, Path)):
        p = Path(path_or_bytes)
        if not p.is_file():
            return f'<div class="miss">{html.escape(label)}: file not found</div>'
        data = p.read_bytes()
        mime = mime or ("image/png" if data[:4] == b"\x89PNG" else "image/jpeg")
    else:
        data = path_or_bytes
    uri = f"data:{mime};base64," + base64.b64encode(data).decode("ascii")
    return f'<figure><img src="{uri}" alt="{html.escape(label)}"><figcaption>{html.escape(label)}</figcaption></figure>'


def cmd_report(args, io):
    if not args.run:
        raise Refusal("report needs --run NAME")
    run_dir = run_dir_of(args)
    recs = sorted(run_dir.glob("*.call.json")) if run_dir.exists() else []
    if not recs:
        raise Refusal(f"no calls recorded in {run_dir}")
    out_path = run_dir / "contact-sheet.html"
    if out_path.exists() and not args.overwrite_report:
        raise Refusal(f"{out_path} exists; pass --overwrite-report to replace the report (it never touches outputs)")
    rows, total = [], Decimal("0")
    for rp in recs:
        r = json.loads(rp.read_text(encoding="utf-8"))
        if r.get("state") != "declined":
            total += Decimal(str(r.get("est_cost_usd", "0")))
        ins = "".join(img_tag(i["path"], f'{i["role"]}: {i["name"]}') for i in r.get("inputs", []))
        outs = "".join(img_tag(run_dir / o, o) for o in r.get("outputs", [])) or '<div class="miss">no output (failed, cancelled or timed out)</div>'
        prompt = r.get("request", {}).get("prompt", "")
        rows.append(
            f'<section><h2>{html.escape(r["call"])} <small>{html.escape(r["endpoint"])}</small></h2>'
            f'<p class="meta">state <b>{html.escape(str(r.get("state")))}</b> &middot; est. cost {html.escape(str(r.get("est_cost_usd")))} USD '
            f'(price date {html.escape(str(r.get("price_date")))}) &middot; latency {html.escape(str(r.get("latency_s", "n/a")))} s '
            f'&middot; request id {html.escape(str(r.get("request_id", "n/a")))}</p>'
            f'<p class="meta">prompt: {html.escape(str(prompt))}</p>'
            + (f'<p class="err">error: {html.escape(str(r.get("error")))}</p>' if r.get("error") else "")
            + f'<div class="pair"><div><h3>Inputs</h3>{ins}</div><div><h3>Output</h3>{outs}</div></div></section>')
    doc = ("<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>"
           f"<title>Sketch Assist proof: {html.escape(args.run)}</title>"
           "<style>body{font:15px system-ui,sans-serif;margin:1rem auto;max-width:1100px;padding:0 16px;background:#fafafa;color:#222}"
           "section{border:1px solid #ccc;border-radius:8px;padding:12px;margin:16px 0;background:#fff}"
           ".pair{display:grid;grid-template-columns:1fr 1fr;gap:16px}img{max-width:100%;border:1px solid #ddd}"
           "figure{margin:0 0 8px}.meta{color:#555}.err{color:#a00}.miss{color:#a00;font-style:italic}small{font-weight:normal;color:#666}"
           "@media(max-width:700px){.pair{grid-template-columns:1fr}}</style>"
           f"<h1>Run {html.escape(args.run)}</h1><p>Estimated cost of this run: <b>{total} USD</b> at list prices. "
           "Local file; nothing here is uploaded. Failures are listed too: keep them in your scoring.</p>"
           + "".join(rows))
    out_path.write_text(doc, encoding="utf-8")
    io.p(f"Wrote {out_path}")
    return 0


def cmd_blank(args, io):
    """Write a plain white or black PNG (a background or mask starting point). No network, reads nothing."""
    m = re.fullmatch(r"(\d{1,4})x(\d{1,4})", args.size)
    if not m:
        raise Refusal("--size must look like 1280x720")
    w, h = int(m.group(1)), int(m.group(2))
    if not (1 <= w <= MAX_PX and 1 <= h <= MAX_PX):
        raise Refusal(f"--size must be between 1 and {MAX_PX} px on each side")
    out = Path(args.out)
    if out.exists():
        raise Refusal(f"{out} already exists; not overwriting")
    v = 255 if args.color == "white" else 0
    row = b"\x00" + bytes([v]) * w
    raw = zlib.compress(row * h)

    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    out.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 0, 0, 0, 0))
                    + chunk(b"IDAT", raw) + chunk(b"IEND", b""))
    io.p(f"Wrote {out} ({w}x{h}, {args.color})")
    return 0


# ---------------------------------------------------------------- cli

def make_parser():
    ap = argparse.ArgumentParser(
        prog="run.py",
        description="Sketch Assist proof kit (#77). The API key is read only from the FAL_KEY environment variable. "
        "Default is a dry run that sends nothing; live mode needs --live and a typed y for every call.",
    )
    sub = ap.add_subparsers(dest="cmd", required=True)

    def common(p, scenario=True):
        p.add_argument("--workdir", default="~/waltcut-proof", help="ledger.jsonl and runs/ live here (default ~/waltcut-proof, outside the repo)")
        p.add_argument("--cap", default=str(DEFAULT_CAP), help=f"spend cap in USD (default {DEFAULT_CAP}, max {MAX_CAP})")
        p.add_argument("--run", help="run folder name under <workdir>/runs/")

    def scen(p):
        p.add_argument("scenario", choices=sorted(SCENARIOS))
        p.add_argument("--sketch"); p.add_argument("--approved"); p.add_argument("--mask"); p.add_argument("--reference")
        p.add_argument("--prompt"); p.add_argument("--style", default=DEFAULT_STYLE)
        p.add_argument("--only", help="comma list of recipes: " + ",".join(ORDER))
        p.add_argument("--include-experimental", action="store_true", help="enable EXPERIMENTAL recipes (flux-general)")
        p.add_argument("--controlnet-union-path", help="EXPERIMENTAL flux-general: Hugging Face repo of a ControlNet-Union (no default is documented)")
        p.add_argument("--control-mode", default="canny", choices=["canny", "tile", "depth", "blur", "pose", "gray", "low-quality"])
        p.add_argument("--conditioning-scale", type=float, default=1.0)
        p.add_argument("--reference-strength", type=float, default=0.65)
        p.add_argument("--ip-adapter-path"); p.add_argument("--ip-adapter-encoder-path"); p.add_argument("--ip-scale", type=float, default=0.6)

    p = sub.add_parser("plan", help="list planned calls, endpoints, parameters, estimates, price date")
    common(p); scen(p)
    p = sub.add_parser("run", help="execute (dry run unless --live)")
    common(p); scen(p)
    p.add_argument("--live", action="store_true", help="really send, one typed y per call")
    p.add_argument("--dry-run", action="store_true", help="send nothing (this is the default)")
    p.add_argument("--timeout", type=float, default=300, help="per-call limit in seconds, then the request is cancelled")
    p.add_argument("--poll-interval", type=float, default=2.0)
    p = sub.add_parser("report", help="build contact-sheet.html for a run (local file)")
    common(p); p.add_argument("--overwrite-report", action="store_true")
    p = sub.add_parser("make-blank", help="write a plain white or black PNG (offline helper)")
    p.add_argument("--color", choices=["white", "black"], required=True)
    p.add_argument("--size", required=True, help="for example 1280x720")
    p.add_argument("--out", required=True)
    p = sub.add_parser("spent", help="total estimated spend so far")
    common(p)
    return ap


def main(argv=None, io=None):
    io = io or IO()
    args = make_parser().parse_args(argv)
    try:
        if args.cmd == "plan":
            return cmd_plan(args, io)
        if args.cmd == "run":
            return cmd_run(args, io)
        if args.cmd == "report":
            return cmd_report(args, io)
        if args.cmd == "make-blank":
            return cmd_blank(args, io)
        return cmd_spent(args, io)
    except Refusal as e:
        io.e(f"REFUSED: {e}")
        return 2
    except KeyboardInterrupt:
        return 130
    except Exception as e:
        io.e(f"ERROR: {type(e).__name__}: {e}")
        return 1


if __name__ == "__main__":
    sys.exit(main())
