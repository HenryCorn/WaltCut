"""A local fake of the fal queue API. Records everything it receives."""
import base64
import json
import struct
import threading
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def make_png(w, h, rgb=(200, 30, 30)):
    row = b"\x00" + bytes(rgb) * w
    raw = zlib.compress(row * h)

    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", raw) + chunk(b"IEND", b""))


class FakeFal:
    def __init__(self, polls_before_done=1, hang=False, submit_status=200):
        self.requests = []  # (method, path, headers dict, body bytes)
        self.polls_before_done = polls_before_done
        self.hang = hang
        self.submit_status = submit_status
        self.cancelled = threading.Event()
        outer = self

        class H(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def _send(self, code, obj):
                b = json.dumps(obj).encode()
                self.send_response(code)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(b)))
                self.end_headers()
                self.wfile.write(b)

            def _body(self):
                n = int(self.headers.get("Content-Length") or 0)
                return self.rfile.read(n) if n else b""

            def do_POST(self):
                body = self._body()
                outer.requests.append(("POST", self.path, dict(self.headers), body))
                if outer.submit_status != 200:
                    return self._send(outer.submit_status, {"detail": "nope"})
                owner_app = "/".join(self.path.strip("/").split("/")[:2])
                base = f"http://127.0.0.1:{outer.port}/{owner_app}/requests/req1"
                self._send(200, {"request_id": "req1", "status_url": base + "/status",
                                 "cancel_url": base + "/cancel", "response_url": base})

            def do_GET(self):
                outer.requests.append(("GET", self.path, dict(self.headers), b""))
                if self.path.endswith("/status"):
                    n = sum(1 for r in outer.requests if r[0] == "GET" and r[1].endswith("/status"))
                    done = (not outer.hang) and n > outer.polls_before_done
                    return self._send(200, {"status": "COMPLETED" if done else "IN_PROGRESS"})
                uri = "data:image/png;base64," + base64.b64encode(make_png(8, 8, (1, 2, 3))).decode()
                self._send(200, {"images": [{"url": uri, "width": 8, "height": 8}], "seed": 1})

            def do_PUT(self):
                outer.requests.append(("PUT", self.path, dict(self.headers), b""))
                outer.cancelled.set()
                self._send(200, {"status": "OK"})

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), H)
        self.port = self.server.server_address[1]
        self.url = f"http://127.0.0.1:{self.port}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *a):
        self.server.shutdown()
        self.server.server_close()

    def count(self, method):
        return sum(1 for r in self.requests if r[0] == method)
