import io as _io
import json
import socket
import tempfile
import unittest
from decimal import Decimal
from pathlib import Path
from unittest import mock

import run
from tests.fakefal import FakeFal, make_png

KEY = "fal-SECRET-key-0123456789abcdef"


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)
        self.work = self.dir / "work"
        self.sketch = self.write("sketch.png", make_png(64, 48))
        self.approved = self.write("approved.png", make_png(64, 48, (9, 9, 9)))
        self.mask = self.write("mask.png", make_png(64, 48, (255, 255, 255)))

    def write(self, name, data):
        p = self.dir / name
        p.write_bytes(data)
        return str(p)

    def invoke(self, argv, env=None, answers=None):
        out, err = _io.StringIO(), _io.StringIO()
        ans = iter(answers if answers is not None else [])
        prompts = []

        def input_fn(p):
            prompts.append(p)
            return next(ans)

        io = run.IO(env=env if env is not None else {}, input_fn=input_fn, out=out, err=err)
        code = run.main(["--help"][:0] + argv, io)
        return code, out.getvalue(), err.getvalue(), prompts

    def run_args(self, *extra, scenario="single-enhance", run_name="r1"):
        base = ["run", scenario, "--workdir", str(self.work), "--run", run_name, "--poll-interval", "0"]
        if scenario == "single-enhance":
            base += ["--sketch", self.sketch]
        return base + list(extra)

    def ledger(self):
        return run.Ledger(self.work / "ledger.jsonl")

    def all_text(self):
        parts = []
        for p in self.work.rglob("*"):
            if p.is_file():
                parts.append(p.read_bytes().decode("latin-1"))
        return "\n".join(parts)


class DryRun(Base):
    def test_dry_run_makes_no_network_calls_and_writes_nothing(self):
        def boom(*a, **k):
            raise AssertionError("network used in dry run")

        with mock.patch.object(socket.socket, "connect", boom), mock.patch.object(socket, "create_connection", boom), \
                mock.patch.object(socket, "getaddrinfo", boom):
            code, out, err, prompts = self.invoke(self.run_args())
        self.assertEqual(code, 0, err)
        self.assertIn("DRY RUN", out)
        self.assertEqual(prompts, [])
        self.assertFalse(self.work.exists(), "dry run must not create files")

    def test_dry_run_shows_files_and_params(self):
        code, out, err, _ = self.invoke(self.run_args())
        self.assertIn("64x48", out)
        self.assertIn("sha256", out)
        self.assertIn("at least 7 days", out)
        self.assertIn("image_url", out)
        self.assertNotIn("base64,", out)  # image bytes are described, not dumped

    def test_default_host_is_real_and_dry_run_is_default(self):
        self.assertEqual(run.DEFAULT_BASE_URL, "https://queue.fal.run")
        self.assertTrue(run.host_is_fal(run.DEFAULT_BASE_URL))
        self.assertFalse(run.host_is_fal("https://queue.fal.run.evil.example"))
        args = run.make_parser().parse_args(["run", "single-enhance", "--sketch", "x"])
        self.assertFalse(args.live)

    def test_no_key_on_command_line(self):
        for flag in ("--key", "--fal-key", "--api-key"):
            with self.assertRaises(SystemExit), mock.patch("sys.stderr", _io.StringIO()):
                run.make_parser().parse_args(["run", "single-enhance", flag, "abc"])

    def test_live_and_dry_run_conflict(self):
        code, out, err, _ = self.invoke(self.run_args("--live", "--dry-run"), env={"FAL_KEY": KEY})
        self.assertEqual(code, 2)

    def test_live_needs_key(self):
        code, out, err, _ = self.invoke(self.run_args("--live"), env={})
        self.assertEqual(code, 2)
        self.assertIn("FAL_KEY", err)


class Plan(Base):
    def test_plan_lists_estimates_and_date_and_experimental_off(self):
        code, out, err, _ = self.invoke(["plan", "single-enhance", "--sketch", self.sketch, "--workdir", str(self.work)])
        self.assertEqual(code, 0, err)
        self.assertIn(run.PRICE_DATE, out)
        self.assertIn("fal-ai/flux-pro/kontext", out)
        self.assertIn("fal-ai/nano-banana/edit", out)
        self.assertIn("EXPERIMENTAL, off by default", out)
        self.assertIn("$0.0550", out)
        self.assertIn("$0.0398", out)

    def test_experimental_needs_flag_and_paths(self):
        calls = run.build_calls(run.make_parser().parse_args(
            ["plan", "single-enhance", "--sketch", self.sketch, "--include-experimental"]))
        fg = [c for c in calls if c.slug == "flux-general"][0]
        self.assertIn("missing --controlnet-union-path", fg.skip)
        calls = run.build_calls(run.make_parser().parse_args(
            ["plan", "single-enhance", "--sketch", self.sketch, "--include-experimental", "--controlnet-union-path", "org/repo"]))
        fg = [c for c in calls if c.slug == "flux-general"][0]
        self.assertIsNone(fg.skip)
        self.assertIn("controlnet_unions", fg.body)

    def test_additive_and_two_poses(self):
        code, out, err, _ = self.invoke(["plan", "additive-edit", "--approved", self.approved, "--mask", self.mask,
                                         "--prompt", "a red collar", "--workdir", str(self.work)])
        self.assertEqual(code, 0, err)
        self.assertIn("fal-ai/flux-pro/v1/fill", out)
        code, out, err, _ = self.invoke(["plan", "two-poses", "--reference", self.approved, "--sketch", self.sketch,
                                         "--workdir", str(self.work)])
        self.assertEqual(code, 0, err)
        self.assertIn("nano-banana", out)

    def test_mask_dimension_mismatch_refused(self):
        bad = self.write("badmask.png", make_png(10, 10))
        code, out, err, _ = self.invoke(["plan", "additive-edit", "--approved", self.approved, "--mask", bad,
                                         "--prompt", "x", "--workdir", str(self.work)])
        self.assertEqual(code, 2)
        self.assertIn("must match", err)


class Inputs(Base):
    def test_oversize_pixels_refused(self):
        big = self.write("big.png", make_png(4097, 2))
        code, out, err, _ = self.invoke(self.run_args() + ["--sketch", big])
        self.assertEqual(code, 2)
        self.assertIn("4096", err)

    def test_oversize_bytes_refused(self):
        p = self.dir / "huge.png"
        with open(p, "wb") as f:
            f.write(make_png(8, 8))
            f.write(b"\0" * (run.MAX_BYTES + 1))
        code, out, err, _ = self.invoke(self.run_args() + ["--sketch", str(p)])
        self.assertEqual(code, 2)
        self.assertIn("20 MB", err)

    def test_directory_and_non_image_refused(self):
        code, _, err, _ = self.invoke(self.run_args() + ["--sketch", str(self.dir)])
        self.assertEqual(code, 2)
        self.assertIn("directory", err)
        txt = self.write("a.png", b"not an image at all")
        code, _, err, _ = self.invoke(self.run_args() + ["--sketch", txt])
        self.assertEqual(code, 2)

    def test_truncated_headers_do_not_crash(self):
        for data in (b"\x89PNG\r\n\x1a\n", b"\xff\xd8", b"\xff\xd8\xff\xc0\x00"):
            p = self.write("t.png", data)
            code, _, err, _ = self.invoke(["plan", "single-enhance", "--sketch", p, "--workdir", str(self.work)])
            self.assertEqual(code, 2, err)


class Live(Base):
    def env(self, fal):
        return {"FAL_KEY": KEY, "FAL_BASE_URL": fal.url}

    def test_success_flow_ledger_files_and_privacy_headers(self):
        with FakeFal() as fal:
            code, out, err, prompts = self.invoke(self.run_args("--live", "--only", "kontext"), self.env(fal), ["y"])
            self.assertEqual(code, 0, err + out)
            self.assertEqual(len(prompts), 1)
            posts = [r for r in fal.requests if r[0] == "POST"]
            self.assertEqual(len(posts), 1)
            hdrs = {k.lower(): v for k, v in posts[0][2].items()}
            self.assertEqual(hdrs["authorization"], f"Key {KEY}")
            self.assertEqual(hdrs["x-fal-store-io"], "0")
            self.assertIn("expiration_duration_seconds", hdrs["x-fal-object-lifecycle-preference"])
            body = json.loads(posts[0][3])
            self.assertTrue(body["image_url"].startswith("data:image/png;base64,"))
            self.assertTrue(body["sync_mode"])
        self.assertIn("NOT fal.ai", err)  # loud warning for a non-fal host
        rd = self.work / "runs" / "r1"
        self.assertTrue((rd / "01-kontext.png").exists() or any(rd.glob("01-kontext*.png")))
        self.assertTrue((rd / "01-kontext.request.json").exists())
        entries = self.ledger().entries()
        self.assertEqual(entries[0]["state"], "done")
        self.assertEqual(entries[0]["inputs"][0]["sha256"], run.Loaded("sketch", self.sketch).sha256)
        self.assertEqual(self.ledger().spent(), Decimal("0.055"))

    def test_key_never_written_or_printed(self):
        with FakeFal(submit_status=500) as fal:
            code, out, err, _ = self.invoke(self.run_args("--live", "--only", "kontext"), self.env(fal), ["y"])
        for text in (out, err, self.all_text()):
            self.assertNotIn(KEY, text)
        with FakeFal() as fal:
            code, out, err, _ = self.invoke(self.run_args("--live", "--only", "kontext", run_name="r2"), self.env(fal), ["y"])
        for text in (out, err, self.all_text()):
            self.assertNotIn(KEY, text)
        run.register_secret(KEY)
        self.assertNotIn(KEY, run.redact(f"boom Authorization: Key {KEY} and {KEY}"))
        led = run.Ledger(self.dir / "l.jsonl")
        led.append({"id": "x", "state": "sending", "note": KEY})
        self.assertNotIn(KEY, (self.dir / "l.jsonl").read_text())

    def test_ledger_written_before_the_network_call(self):
        seen = {}
        real = run.http

        def spy(method, url, *a, **k):
            if method == "POST" and "ledger" not in seen:
                seen["ledger"] = self.ledger().entries()
            return real(method, url, *a, **k)

        with FakeFal() as fal, mock.patch.object(run, "http", spy):
            self.invoke(self.run_args("--live", "--only", "kontext"), self.env(fal), ["y"])
        self.assertEqual(len(seen["ledger"]), 1)
        self.assertEqual(seen["ledger"][0]["state"], "sending")
        self.assertEqual(seen["ledger"][0]["est_cost_usd"], "0.055")

    def test_declined_means_no_network(self):
        with FakeFal() as fal:
            code, out, err, prompts = self.invoke(self.run_args("--live", "--only", "kontext"), self.env(fal), ["n"])
            self.assertEqual(fal.requests, [])
        self.assertIn("NOT sent", out)
        self.assertEqual(self.ledger().spent(), Decimal("0"))
        for answer in ("yes", "Y", "", " "):
            with FakeFal() as fal:
                self.invoke(self.run_args("--live", "--only", "nano-banana", run_name="r" + str(len(answer))),
                            self.env(fal), [answer])
                self.assertEqual(fal.requests, [], answer)

    def test_confirmation_is_per_call(self):
        with FakeFal() as fal:
            code, out, err, prompts = self.invoke(self.run_args("--live"), self.env(fal), ["y", "n"])
            self.assertEqual(len(prompts), 2)
            self.assertEqual(len([r for r in fal.requests if r[0] == "POST"]), 1)

    def test_cap_refuses_before_call(self):
        led = self.ledger()
        led.append({"id": "old", "state": "done", "est_cost_usd": "3.48"})
        with FakeFal() as fal:
            code, out, err, prompts = self.invoke(self.run_args("--live", "--only", "kontext"), self.env(fal), ["y"])
            self.assertEqual(code, 2)
            self.assertEqual(fal.requests, [])
            self.assertEqual(prompts, [])
        self.assertIn("cap", err.lower())

    def test_cap_stops_midway_and_counts_all(self):
        with FakeFal() as fal:
            code, out, err, _ = self.invoke(self.run_args("--live", "--cap", "0.07"), self.env(fal), ["y", "y"])
            self.assertEqual(code, 2)
            self.assertEqual(len([r for r in fal.requests if r[0] == "POST"]), 1)
        self.assertEqual(self.ledger().spent(), Decimal("0.055"))

    def test_cap_limits(self):
        self.assertEqual(run.parse_cap("5.00"), Decimal("5.00"))
        for bad in ("5.01", "0", "-1", "abc", "nan", "inf"):
            with self.assertRaises(run.Refusal):
                run.parse_cap(bad)
        self.assertEqual(run.DEFAULT_CAP, Decimal("3.50"))

    def test_corrupt_ledger_refuses(self):
        self.work.mkdir()
        (self.work / "ledger.jsonl").write_text("{not json\n")
        with FakeFal() as fal:
            code, _, err, _ = self.invoke(self.run_args("--live", "--only", "kontext"), self.env(fal), ["y"])
            self.assertEqual(code, 2)
            self.assertEqual(fal.requests, [])

    def test_no_overwrite_of_existing_outputs(self):
        with FakeFal() as fal:
            self.invoke(self.run_args("--live", "--only", "kontext"), self.env(fal), ["y"])
            before = len(fal.requests)
            code, out, err, prompts = self.invoke(self.run_args("--live", "--only", "kontext"), self.env(fal), ["y"])
            self.assertEqual(code, 2)
            self.assertIn("overwrite", err)
            self.assertEqual(len(fal.requests), before)

    def test_ctrl_c_cancels_queued_request_and_records_it(self):
        def interrupt(_):
            raise KeyboardInterrupt

        with FakeFal(polls_before_done=99) as fal, mock.patch.object(run, "_sleep", interrupt):
            code, out, err, _ = self.invoke(self.run_args("--live", "--only", "kontext"), self.env(fal), ["y"])
            self.assertEqual(code, 130)
            puts = [r for r in fal.requests if r[0] == "PUT"]
            self.assertEqual(len(puts), 1)
            self.assertTrue(puts[0][1].endswith("/cancel"))
        e = self.ledger().entries()[0]
        self.assertEqual(e["state"], "cancelled")
        self.assertEqual(e["cancel_http"], "200")
        self.assertEqual(self.ledger().spent(), Decimal("0.055"))  # cancelled still counted (may be billed)

    def test_timeout_cancels(self):
        with FakeFal(hang=True) as fal:
            code, out, err, _ = self.invoke(
                self.run_args("--live", "--only", "kontext", "--timeout", "0.3"), self.env(fal), ["y"])
            self.assertTrue(fal.cancelled.is_set())
        self.assertEqual(self.ledger().entries()[0]["state"], "timeout")

    def test_redirects_are_not_followed(self):
        self.assertIsNone(run._NoRedirect().redirect_request(None, None, 302, "", {}, "http://evil.example"))


class Blank(Base):
    def test_make_blank(self):
        out = str(self.dir / "w.png")
        code, o, e, _ = self.invoke(["make-blank", "--color", "white", "--size", "32x16", "--out", out])
        self.assertEqual(code, 0, e)
        L = run.Loaded("x", out)
        self.assertEqual((L.width, L.height), (32, 16))
        code, o, e, _ = self.invoke(["make-blank", "--color", "white", "--size", "32x16", "--out", out])
        self.assertEqual(code, 2)
        code, o, e, _ = self.invoke(["make-blank", "--color", "black", "--size", "9999x9", "--out", str(self.dir / "b.png")])
        self.assertEqual(code, 2)


class Report(Base):
    def test_report_has_cost_latency_and_failures(self):
        with FakeFal() as fal:
            env = {"FAL_KEY": KEY, "FAL_BASE_URL": fal.url}
            self.invoke(self.run_args("--live", "--only", "kontext,nano-banana"), env, ["y", "y"])
        code, out, err, _ = self.invoke(["report", "--workdir", str(self.work), "--run", "r1"])
        self.assertEqual(code, 0, err)
        doc = (self.work / "runs" / "r1" / "contact-sheet.html").read_text()
        self.assertIn("01-kontext", doc)
        self.assertIn("02-nano-banana", doc)
        self.assertIn("latency", doc)
        self.assertIn("data:image/png;base64", doc)
        self.assertNotIn(KEY, doc)
        code, out, err, _ = self.invoke(["report", "--workdir", str(self.work), "--run", "r1"])
        self.assertEqual(code, 2)  # will not overwrite its own previous sheet silently

    def test_spent_command(self):
        self.ledger().append({"id": "a", "state": "done", "est_cost_usd": "0.5"})
        code, out, err, _ = self.invoke(["spent", "--workdir", str(self.work)])
        self.assertEqual(code, 0)
        self.assertIn("$0.5000", out)
        self.assertIn("$3.0000", out)


if __name__ == "__main__":
    unittest.main()
