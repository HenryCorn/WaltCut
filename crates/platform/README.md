# waltcut-platform

macOS integration for WaltCut (layer L5): VideoToolbox hardware **decoding** behind
`waltcut_codecs::VideoDecoder`, safe hybrid fallback to the clean-room Rust decoders, and the
existing tablet-pressure input bridge. This is the workspace's narrow OS-FFI boundary; see
[ADR 0001](../../docs/adr/0001-platform-ffi.md) and [AGENTS.md](../../AGENTS.md).

```rust
// Desktop, CLI and benchmark startup: register hardware decoding, not encoding.
let availability = waltcut_platform::register();
```

## Retained decoder path

- `videotoolbox.rs` accepts H.264 (`avcC`) and HEVC (`hvcC`), 8/10-bit and 4:2:0/4:2:2 where the
  machine supports them. Hardware is required when creating the OS session; otherwise the factory
  declines and the application's Rust decoder is used.
- Compressed samples go through asynchronous decompression with two access units in flight.
  NV12/P010-style biplanar output is copied into planar `Yuv8`/`Yuv16`, cropped to the stream's
  conformance window. The stream's reorder depth restores presentation order; CRA starts omit
  RASL pictures. Reset creates a fresh session.
- `hybrid.rs` replays samples from a restart point through the retained software decoder after a
  mid-stream hardware failure. Replay is bounded to 600 samples / 256 MB, and already-returned
  pictures are not emitted twice. Colour, pixel aspect, timestamps and disposable/random-access
  answers use shared codec helpers.
- Hardware decoding Off declines before session creation. `perf.stats` → `decode.hardware`
  retains backend/frame/session/decline/fallback diagnostics. The public API is safe, failures are
  results, callbacks contain panics, and unsafe FFI blocks keep their safety comments.
- `pen.rs` retains pressure/proximity input used by the drawing brushes; export reduction does not
  remove or replace this input path.

## Export boundary

Hardware encoding, capability probes, encoder registration and their
tests are deleted. This crate no longer depends on `waltcut-export`. HEVC remains an **import**
format (including iPhone Main10 HDR), not an output choice. Export uses fixed built-in Rust
H.264 + AAC MP4 and alpha ProRes 4444 MOV; alpha PNG sequences and PNG/JPEG stills are handled by
the retained raster path. See [the export contract](../export/README.md).

## Tests and acceptance

| Test | Retained proof |
|---|---|
| `tests/videotoolbox.rs` | H.264 High, HEVC Main/Main10, reordering/cropping, seeks/flush, forced mid-stream failures, mutation containment; HEVC 4:2:2 against the external oracle |
| `tests/fallback.rs` | Stand-in failing hardware, replay and in-band parameter-set changes versus the software decoder |
| `tests/setting.rs` | Hardware decoding Off/Auto through the factory and actual media stack |

Fixtures use the existing external ffmpeg recipes under `target/fixtures/platform`; no third-party
decoder source, linked ffmpeg or committed media is required. These tests run natively on macOS in
CI. A real iPhone HEVC Main10 HLG MOV + AAC file has not been checked by hand.
