# WaltCut interface

The egui frontend draws panels and dialogs, and dispatches project changes through the engine.
Interactive controls keep stable automation ids.

## Transition drop previews

Dragging a video or audio transition over a compatible clip shows its actual time span, the
selected cut and an In/Out label. Shared cuts center the span; isolated edges use a one-sided
span. The preview uses the engine's read-only transition planner, including Timeline duration
preferences, and release dispatches `effects.apply` with the same edge. Locked tracks and
incompatible clip types show no valid transition preview. Ordinary effects retain their clip
outline. `timeline.transitionDropPreview` exposes the span and edge to automation.

`transition_drop_ui` covers both edges of short video/audio clips, hover without project edits,
the committed span and undo. Set `FILMCRAFT_UI_SNAPSHOT_DIR` to render screenshots with wgpu.

## Language

The interface is English only. `tl!` and `i18n::t` return the source string; `tlf!` fills `{name}`
templates. Placeholder values (including user filenames containing braces) are inserted literally.
