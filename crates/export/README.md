# waltcut-export

The bounded media delivery pipeline for WaltCut's drawing/animation workflow.
**#25 IMPLEMENTATION DRAFTED — native macOS acceptance pending.**

## Outputs

| Command format | Written output |
|---|---|
| `h264` (default; alias `mp4`) | H.264 High + stereo AAC-LC in **MP4** |
| `prores` (aliases `prores4444`, `mov`) | ProRes **4444** (`ap4h`) **MOV with straight alpha**, stereo little-endian float32 PCM |
| `png` | RGBA8 **PNG sequence with straight alpha**, no audio sidecar |
| `file.exportFrame` | Full-resolution SDR **PNG** (8/16-bit RGBA) or **JPEG** (8-bit, flattened over black) |

Geometry, frame rate and soundtrack sample rate derive from the selected sequence. H.264 derives
even dimensions (minimum 2); ProRes/PNG preserve odd dimensions. The built-in H.264 encoder uses
one-pass VBR 20/30 Mb/s and approximately two-second keyframes; AAC bitrate is fixed and bounded
by the encoder's per-frame capacity. Encoder profile, quality, channels, muxer and hardware are
not user choices. HDR sequence colour management/signalling remains; stills are tone-mapped SDR.

There is no export queue/backlog, delivery preset catalogue, advanced settings blob, broadcast/
cinema delivery mode, hardware encoder registry/probe, overlay/watermark/burn-in, caption sidecar,
loudness pass or non-allowlisted movie/audio/image output wrapper. WAV remains an import/test
fixture format, not an export API. All retained imports and hardware **decoding** remain intact.

## Commands and ownership

`file.exportMedia` and `export.quick` share strict path/format/sequence/range/start-end/wait/
overwrite parameters. Quick Export can derive its path; media export requires one. Custom bounds
must be paired in the same unit, with an exclusive end. Defaults use sequence In/Out, falling
back to the sequence. Unknown/removed parameters and invalid types fail explicitly before files,
jobs or project edits. See [the complete command contract](../../docs/control-protocol.md#export).

Export and Quick Export show destination, format, core range, Start, progress/status and Cancel.
The engine owns one cancellable worker, immutable project snapshot and bounded completion
history; a second start fails rather than queues. Shutdown cancels and joins it. Worker panics
become failed results, not orphan jobs. GPU composition is automatically selected when a
registered renderer can handle the frame, with the CPU reference as fallback; it is unrelated to
the fixed built-in encoder.

Outputs stage in a uniquely reserved sibling directory. Existing movies/stills need explicit
overwrite/Replace, and are atomically replaced on macOS only after successful encoding. New
movie/still destinations and PNG frames use no-clobber hard links, so the destination filesystem
must support hard links. PNG sequences require unused names even if overwrite was requested.
Failure/cancel rolls back only this job's installed files (file identity is checked), preserving
the previous destination and imported sources/project. Normal cleanup failures reach the caller
and status; panic-time cleanup is best effort with non-panicking logging. A cancellation after
the commit point can observe a completed export rather than undo it.

`jobs.list`/`jobs.cancel` expose the owned job; finished means encoding, installation and normal
cleanup have ended. Waited results, polled jobs and still results report `offlineMedia`; UI warns
about placeholders and CLI exits 1 unless `--allow-offline` accepts them.

## Bounds and shared internals

Output sides are limited to **8192**, total pixels to **16,777,216**, frame count to **1,000,000**
and positive duration to **24 hours**, within supported exact `Tick` time. Rates, dimensions,
sample rates, ranges and pixel buffers are validated before conversions/allocations. Render
batches are memory-bounded and cancellation-aware. PNG numbering starts at `000`, using more
digits when the count requires it, preserving the requested PNG extension case.

Serialized `ExportSettings` has only `format`, `path`, `range`, with unknown fields rejected.
Internal preview scale, SDR proxy generation, batch ownership and compositor choice are not
command parameters. The stepped pipeline and pending-media handling remain shared with retained
render-preview jobs. Private video/audio encoder implementations use the retained codec crates;
there is no plug-in export encoder registry.

## Validation still required

Core/engine/client tests cover hostile/removed arguments, real retained files, exact frame
counts, ProRes/PNG alpha, HDR signalling, deterministic CPU batches, cancellation/panics,
overwrite/source protection and worker shutdown. `ui-egui/tests/audio_timing.rs` retains the
**actual playback callback and actual written MOV samples decoded through the retained media
stack** for mono/stereo timing, clip gain, Volume/keyframes and track mute; it is not a model-only
substitute.

The Windows development host can cross-typecheck macOS targets and run standalone argument/
filesystem regressions, not native product media/UI tests. **Native retained-import fixture
decoding, every output round trip/frame-count/alpha check, actual callback/MOV samples, real
iPhone HEVC Main10 HLG MOV + AAC manual acceptance, full CI and the control-channel
animation-flow screenshot remain pending.** No real iPhone file has been checked.
See [testing](../../docs/testing.md#25-native-acceptance-still-required); this draft is not
complete, accepted or merged and does not close issues or end the feature freeze.
