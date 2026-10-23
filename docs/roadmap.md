# Roadmap detail

The one-page summary is [ROADMAP.md](../ROADMAP.md). The scope is [animation.md](animation.md).

## Current focus

The reduction epic [#14](https://github.com/HenryCorn/WaltCut/issues/14) is done and v0.1.0 is out.
What comes next:

1. **Deepen drawing and cel animation** (epic [#53](https://github.com/HenryCorn/WaltCut/issues/53)).
   Foundations first: preserve per-event pen position, pressure and timing (#54), make live strokes,
   erased onion skins and export render the same (#55), and set native input, scrub and playback
   baselines (#70) so later work is measured. Then cel timing (#59, #60), onion skin (#62), a light
   table (#64), stroke smoothing (#57) and pressure curves (#56).
2. **Reference images as tracing paper** (#3) and the **storage panel** (#10).
3. **Sketch Assist** (epic #75), starting with its evaluations (#76, #73), and the AI provider work
   it depends on (#6, #11, #1).
4. **Distribution:** signing and notarization once there is an Apple Developer ID, then drop the
   right-click ▸ Open step.
5. **Engineering:** the quick-wins epic #99 (flaky tests, cache measurement, remaining fixture
   generators) and the dependency migrations in #28 (egui 0.37, cpal 0.18, symphonia 0.6, skrifa and
   harfrust).

Hardware checks only the maintainer can do: the GAOMON M10K pen on real hardware (#5), and
AI Restyle with a real fal.ai key (#6).

## Rules

- Every pull request passes the macOS gate (`cargo xtask check`, and `cargo xtask ci` before
  merging large changes); the removed-capability deny-list runs inside `cargo xtask layers`.
- Every change leaves the animation flow working: draw, new cel, in-between, preview, export.
  `tools/e2e-animation-flow.py` ([e2e.md](e2e.md)) checks it end to end.
- Removed features stay removed: no feature flags, "advanced" modes or compatibility shells.
