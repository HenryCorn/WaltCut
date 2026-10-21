# WaltCut

WaltCut is a fork of [FilmCraft](https://github.com/storytold/filmcraft), refocused on drawing and
2D frame-by-frame animation over video. It keeps FilmCraft's Rust editing engine and adds drawing
cels, pressure-aware brushes, onion skinning, in-between generation, and AI Restyle.

The general editing engine and much of the technical documentation come from FilmCraft. This
repository's WaltCut-specific animation workflow is described in the [animation guide](docs/animation.md).
Upstream attribution and licensing are recorded in [NOTICE](NOTICE) and the
[MIT](LICENSE-MIT) / [Apache-2.0](LICENSE-APACHE) license files.

## Animation workflow

1. Open a project and sequence, or create a drawing above the footage from **Graphics and Titles ▸
   New Layer ▸ New Drawing**.
2. Choose the **Brush** tool (`Shift+P`) and draw in the Program monitor. Choose Pen, Pencil,
   Marker, or Eraser; use the cel mode to draw on the held cel, start a blank cel, or duplicate the
   held cel.
3. Step through the sequence, make new cels for changed poses, and use onion skins to see the
   previous and next cels. **In-between** interpolates strokes toward the next cel.
4. Optionally open **Graphics and Titles ▸ AI Restyle…** to restyle the frame at the playhead, a
   selected clip, or a sequence In/Out range. See the guide for provider and privacy details.

See [docs/animation.md](docs/animation.md) for the full workflow and current limitations.

## Build from source

This fork retains the upstream Cargo package names for now:

```sh
cargo run --release -p waltcut
cargo run --release -p waltcut-cli -- commands
```

See [docs/contributing.md](docs/contributing.md) for Rust setup and build details, and
[AGENTS.md](AGENTS.md) for project-wide engineering and asset rules.

## Upstream documentation

The remaining architecture, command, and editing references describe the inherited FilmCraft
engine and are retained for contributors:

- [Architecture](docs/architecture.md)
- [Agent and control-channel guide](docs/agents.md)
- [Graphics and titles](docs/graphics.md)
- [Roadmap](ROADMAP.md)
