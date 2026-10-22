# WaltCut roadmap

WaltCut is being reduced to the drawing and 2D animation workflow described in
[`docs/animation.md`](docs/animation.md). That guide is the retained-scope allowlist: until the
reduction work is complete, inherited FilmCraft systems may still be present in the repository.
Their presence does not expand WaltCut's intended product scope.

## Reduction status

| Issue | Status | Scope |
|---|---|---|
| [#17](https://github.com/HenryCorn/WaltCut/issues/17) | Complete | Decide the reduced product scope and record the retained workflow. |
| [#18](https://github.com/HenryCorn/WaltCut/issues/18) | Complete | Define the legacy-project compatibility boundary. |
| [#2](https://github.com/HenryCorn/WaltCut/issues/2), steps 1–2 | Complete | Initial steps complete; product branding and release remain deferred. |
| #19–#22 | Merged | Initial feature and platform reduction work. |
| [#29](https://github.com/HenryCorn/WaltCut/issues/29) | Merged | Remove non-macOS and web platform targets. |
| [#26](https://github.com/HenryCorn/WaltCut/issues/26) | In progress | Remove obsolete inherited feature inventories and align contributor documentation with the retained scope. |
| [#24](https://github.com/HenryCorn/WaltCut/issues/24) | Pending | Graphics templates and the broad effects catalogue. |
| [#23](https://github.com/HenryCorn/WaltCut/issues/23) | Pending | Audio and Essential Sound panels; follows #24. |
| [#25](https://github.com/HenryCorn/WaltCut/issues/25) | Pending | Remaining codec and container cuts; follows #23. |
| [#27](https://github.com/HenryCorn/WaltCut/issues/27) | Close-out freeze | Final reduction measurements and close-out after the preceding work. |
| [#2](https://github.com/HenryCorn/WaltCut/issues/2), product branding/release | Later | Rename remaining product-facing identifiers and prepare the release after reduction close-out. |

Do not describe pending removals as complete. Use the issue statuses above and the allowlist in
[`docs/animation.md`](docs/animation.md) to distinguish intended scope from work that has landed.
The actual executable names (`filmcraft`, `filmcraft-cli`), project extension (`.fcproj`), and
`FILMCRAFT_*` environment variables remain unchanged until the later product-branding step.

## Engineering foundations

The reduction does not relax project safety, asset licensing, clean-room implementation, the
platform FFI boundary, pure-Rust product code, dependency layering, exact `Tick` time, command and
automation requirements, contributor credits, or the quality gates in [`AGENTS.md`](AGENTS.md).
The retained technical architecture and test approach are described in
[`docs/architecture.md`](docs/architecture.md), [`docs/testing.md`](docs/testing.md), and
[`docs/contributing.md`](docs/contributing.md).
