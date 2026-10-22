# Roadmap detail

The one-page summary is [ROADMAP.md](../ROADMAP.md). The scope is [animation.md](animation.md).

## Current focus

1. Finish the removal epic [#14](https://github.com/HenryCorn/WaltCut/issues/14): effects (#24),
   audio (#23), codecs and export (#25), then the deny-list check (#27).
2. Rename the product to WaltCut with the `.waltcut` extension (#2), then cut v0.1.0 (unsigned arm64).
3. Update dependencies and fix security findings (#28).
4. Animation features: reference images as tracing paper (#3), storage panel (#10), showcase video (#4).

## Rules while the epic runs

- One removal slice per pull request, with a before/after count of crates, dependencies and lines.
- Every slice leaves the animation flow working: draw, new cel, in-between, preview, export.
