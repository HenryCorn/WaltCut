# WaltCut interface

The egui frontend draws panels and dialogs, and dispatches project changes through the engine.
Interactive controls keep stable automation ids.

## Graphic clips

Essential Graphics opens the text/shape editor directly, without a template browser. Responsive
layout, per-character styles, source graphics and Replace Fonts remain available. The font dialog
draft is `UiState::replace_fonts`; the control channel edits it with `ui.set {"replaceFonts": …}`.
`graphic_design_ui` covers the editor, font dialog, character styles and absent template commands.
Native macOS execution and screenshots are pending for the #24 draft; cross-target Clippy only
typechecks those tests on Windows.

## Clip compositing

Effect Controls edits ordinary effect parameters, including their keyframes, interpolation menus
and value/velocity graphs. Motion handles, clip opacity, blend modes and graphic paths remain.
Effect-mask creation, rows, tracking controls and Program overlays are removed in the stacked
#24 draft. Brush/cel editing and the drawing eraser are independent of effect masks and remain.
`keyframes_ui` retains an opacity-group movement regression; engine tests cover Motion/Opacity/
blend on media, drawing, text and shape clips. Native macOS UI execution and animation-flow
screenshots remain pending.

## Audio reduction (#23, DRAFT IMPLEMENTATION)

Essential Sound's panel, sub-tab state and automation ids are deleted with its engine commands,
audio-type settings/presets and automatic processing. `scripted::essential_sound_panel_and_commands_are_absent`
checks clean rejection without project or layout mutation. The focus-outline regression now uses
the retained Essential Graphics panel. Mixers and orphan audio-effects/transitions have also been
deleted vertically, including catalogue folders, drag/drop, transition editors, graphical audio
editors and channel-volume/panner controls. The Effects panel now offers retained Motion/Opacity
presets only; generic Effect Controls/keyframe and graphic editing remain.

The timeline line edits static Audio Gain (`gain_db`), independent of intrinsic Volume in Effect
Controls/Properties. The minimal renderer adds enabled/non-bypassed Volume to Audio Gain in dB
per sample, evaluating keyframes in source time. Mute silences the track. Mono/stereo source
mapping, waveform and scrub remain; unsupported multichannel source maps and removed sequence
mix-format parameters reject requests. Export loudness and actual output/source-monitor channel
conversion remain for another slice, not a surround mixer.

`tests/audio_timing.rs` checks actual callback samples and written float WAV samples for mono and
stereo sources, offsets/source In, gain/mute, additive Volume, bypass/disable, channel selection
and keyframes. `clip_audio_ui` checks the independent gain line and retained Volume controls.
**Native macOS test execution, full CI and animation-flow screenshots are pending.** These stacked
drafts are not complete, accepted or merged.

## Language

The interface is English only. `tl!` and `i18n::t` return the source string; `tlf!` fills `{name}`
templates. Placeholder values (including user filenames containing braces) are inserted literally.
