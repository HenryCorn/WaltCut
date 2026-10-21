# Contributing to WaltCut

WaltCut is a fork of FilmCraft focused on drawing and 2D animation. Thanks for helping improve it.
Before you start:

1. Read **[AGENTS.md](AGENTS.md)**. Its rules are absolute. In particular: no Adobe assets of any
   kind, every asset openly licensed with an attribution sidecar, no GPL/LGPL code, and ffmpeg only
   as an external test oracle.
2. **Never crash.** A crash loses someone's work, so this rule outranks feature work: no `unwrap()`,
   `expect()`, `panic!`, `unreachable!`, `todo!`, `unimplemented!` or `unsafe` in production code;
   return `Result` and treat every input as hostile. See [AGENTS.md](AGENTS.md) §0.
3. Follow **[docs/contributing.md](docs/contributing.md)**: setup per OS, building and running,
   the quality gates (`cargo xtask ci`), commit conventions, and how to add commands, effects,
   transitions, codecs, panels and assets.

More reading: [docs/architecture.md](docs/architecture.md), [docs/testing.md](docs/testing.md),
[docs/agents.md](docs/agents.md), [ROADMAP.md](ROADMAP.md).

Contributions are licensed under MIT OR Apache-2.0, the project licence.

Pull requests target `main`. Use a short branch named for the change (for example,
`docs/animation-guide` or `fix/cel-timing`), link the related issue, and describe the user-visible
change and how you verified it. The pull request template lists the checks to report. CI runs
`cargo xtask ci` on Linux, macOS, and Windows; all three checks must pass before merging. CodeRabbit
review is configured for pull requests when the repository integration is enabled; address its
findings and resolve review conversations before merging.
