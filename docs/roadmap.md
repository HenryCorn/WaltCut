# Reduction roadmap

The product scope is the drawing and 2D animation allowlist in
[`animation.md`](animation.md). WaltCut is not pursuing feature breadth against another editor.
Removal work proceeds through the issues below; until each change is merged, inherited systems may
still exist in the codebase. Do not report a pending cut as complete.

## Current sequence

| Issue | Status | Notes |
|---|---|---|
| #17 | Complete | Reduced product scope decided; see the allowlist. |
| #18 | Complete | Legacy-project compatibility boundary decided. |
| #2 steps 1–2 | Complete | Initial steps complete; product branding and release remain deferred. |
| #19–#22 | Merged | Initial reduction tasks. |
| #29 | Merged | Non-macOS and web platform targets. |
| #26 | In progress | Documentation cleanup and retained-scope alignment. |
| #24 | Pending | Graphics templates and the broad effects catalogue. |
| #23 | Pending | Audio and Essential Sound panel removals; begins after #24. |
| #25 | Pending | Remaining codec and container cuts; begins after #23. |
| #27 | Close-out freeze | Final measurements and close-out; not an implementation phase. |
| #2 product branding/release | Later | Rename remaining product identifiers and release after close-out. |

## Kept throughout the reduction

The retained workflow and specific kept systems, media formats, project compatibility, and
platform scope are documented in [`animation.md`](animation.md). In particular, text rendering
continues to support multiple scripts even though the application interface is English-only. There
is one adjustable WaltCut layout that is saved and can be reset, with no named user workspaces.
There is one built-in **WaltCut Default** shortcut map; users can still customize their shortcuts.

The engineering requirements remain those in [`../AGENTS.md`](../AGENTS.md): never crash; keep
assets openly licensed and attributed; work clean-room from public specifications; preserve the
narrow platform FFI exception and pure-Rust product rule; enforce dependency layers; use exact
`Tick` time; route behavior through commands and automation; respect contributor-credit rules; and
run the documented quality gates.

## Identity and compatibility boundary

Until the later product-branding step, keep the current executable names `filmcraft` and
`filmcraft-cli`, `.fcproj` project files, and `FILMCRAFT_*` environment variables. Do not change
these identifiers as part of an earlier reduction task.
