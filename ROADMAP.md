# WaltCut Roadmap

WaltCut is a small, native macOS editor for 2D hand-drawn animation: import footage, images and
audio, arrange a simple timeline, draw and manage cels, preview, export. The scope is described
in [docs/animation.md](docs/animation.md). It is not a general-purpose editor.

The scope reduction (epic [#14](https://github.com/HenryCorn/WaltCut/issues/14)) is finished and
v0.1.0 is released. The detail is in [docs/roadmap.md](docs/roadmap.md).

## Stages

| Stage | What | Status |
|---|---|---|
| Reduce | Delete functionality outside the animation workflow (epic #14) | **done**; final report on [#27](https://github.com/HenryCorn/WaltCut/issues/27) |
| Rename | Crates, binaries, product name, icon and the `.waltcut` extension | **done** |
| Release | v0.1.0, unsigned arm64 macOS build | **published**; signing and notarization need an Apple Developer ID |
| Animation depth | Epic #53: pen input fidelity, consistent rendering, cel timing, onion skin, light table, fills | next |
| Reference images | Import a picture to trace with the tablet (#3) | planned |
| Sketch Assist | Epic #75: beginner drawing with preserved edits and consistent characters | planned |
| AI providers | Real-key verification (#6), pluggable providers (#11), local Stable Diffusion (#1) | planned |
| Engineering | Faster verification (epic #99); dependency migrations (#28) | ongoing |
