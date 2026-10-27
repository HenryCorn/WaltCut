"""Tests for the proof kit. No test may touch the real network: every socket connect to a
non-loopback address raises, for the whole test process."""
import os
import socket
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

_real_connect = socket.socket.connect


def _loopback_only(self, address, *a, **k):
    host = address[0] if isinstance(address, tuple) else address
    if host not in ("127.0.0.1", "localhost", "::1"):
        raise AssertionError(f"test tried to reach the real network: {address!r}")
    return _real_connect(self, address, *a, **k)


socket.socket.connect = _loopback_only
