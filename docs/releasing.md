# Releasing FilmCraft

Until the later product-branding and release step in [ROADMAP.md](../ROADMAP.md), the existing
release workflow and `filmcraft` / `filmcraft-cli` identifiers remain unchanged.

Every push to the `release` branch runs `.github/workflows/release.yml`. The workflow builds
the macOS installer and creates or updates a **draft**
GitHub Release named `FilmCraft v<version>`. Nobody sees a draft until a maintainer publishes it.

User-facing names say **FilmCraft**. Files, binaries and ids stay lowercase
(`filmcraft-<version>-<platform>-<arch>.<ext>`, `ai.storyteller.filmcraft`).

## Cutting a release

1. **Bump the version** on `main`. It lives in exactly one place, `[workspace.package] version`
   in the root `Cargo.toml`; every crate inherits it and the packaging scripts read it from there:

   ```sh
   cargo xtask version                 # prints the current version, e.g. 0.2.1
   cargo xtask version set 0.3.0       # or 0.3.0-rc.1; updates Cargo.toml and Cargo.lock
   ```

   Commit the change (`Cargo.toml` + `Cargo.lock`) through the normal review flow, as a
   `Release: FilmCraft v0.3.0` commit whose message says what changed for users.
2. **Merge `main` into `release`** (or fast-forward it) and push. The workflow starts by itself.
3. **Wait for the draft.** When every job is done (macOS notarization is the slow part), the
   Releases page has a draft `FilmCraft v0.3.0`, targeting the pushed commit, with every artifact
   and `SHA256SUMS.txt`. The notes are generated from the merged pull requests.
4. **Check it.** Download an installer or two and read the job summaries. A `::warning::` there
   means a signing secret was missing and that artifact is unsigned.
5. **Publish** the draft in the GitHub UI. Publishing creates the `v0.3.0` tag. A version with a
   pre-release suffix (`-rc.1`) is marked as a pre-release.

Pushing to `release` again before you publish rebuilds the same draft and replaces its assets.
Once the draft is published, the workflow refuses to touch that version again: bump it first.

**Test runs:** *Actions › Release › Run workflow* runs the whole pipeline by hand. The optional
`version` input (such as `0.3.0-rc.1`) overrides `Cargo.toml` for that run only; each build job
applies it with `cargo xtask version set` before building. The signing job (macOS)
and the draft-release job run in the `release` environment, which only the `release` branch can
use, so pick that branch for a full run. Run from any other branch, it is a dry run: the
signing job is refused, and the draft-release job, which needs it, is skipped.

## What gets built

| Platform | Artifacts | Built on |
|---|---|---|
| macOS 11+ (universal: Apple silicon and Intel) | `filmcraft-<v>-macos-universal.dmg`, `filmcraft-cli-<v>-macos-universal.zip` | `macos-15` |

`filmcraft --version` and `filmcraft-cli --version` print the version from `Cargo.toml`.

**Fonts.** Every build job checks out [craft-fonts](https://github.com/storytold/craft-fonts) at a
pinned commit (the `ref:` of its craft-fonts checkout step, marked `# bump deliberately`) and builds
with `CRAFT_FONTS_DIR` and `CRAFT_FONTS_REQUIRED=1`, so releases embed its Japanese fonts and fail
rather than ship without them. The packages
carry each embedded font's licence (`copy_font_licences` in `packaging/env.sh`). Bump the pins
deliberately, in every job at once.

### macOS

`packaging/macos/package.sh` builds `aarch64-apple-darwin` and `x86_64-apple-darwin` with
`MACOSX_DEPLOYMENT_TARGET=11.0`, joins them with `lipo` and assembles `FilmCraft.app`:

- `Info.plist` is generated from `Info.plist.in` (bundle id `ai.storyteller.filmcraft`, the
  version and the build commit).
- **Signing** uses the hardened runtime and a secure timestamp, with the entitlements in
  `entitlements.plist`. The Developer ID certificate is imported into a temporary keychain by
  `packaging/macos/import-cert.sh`.
- **Notarization:** the app is sent with `xcrun notarytool submit`, then the ticket is stapled
  and checked with `stapler validate`. The app goes on a DMG (`hdiutil makehybrid`), which is
  signed and notarized too. The universal `filmcraft-cli` is signed the same way, zipped, and the zip is notarized.
  Its Finder window (background, icon size and positions) comes from
  [`packaging/macos/dmg/`](../packaging/macos/dmg/README.md), and its volume is named `FilmCraft`
  without the version, which the window's background needs; the DMG file name keeps the version.

Locally, without certificates, the script signs ad hoc and skips notarization, which is enough to
check the bundle and the DMG on your own Mac (`packaging/macos/package.sh`).

## The draft release

The last job waits for every build, downloads their artifacts, writes `SHA256SUMS.txt` and creates
the draft `FilmCraft v<version>` with notes generated from the merged pull requests. If the draft
already exists, it replaces its assets and keeps it a draft. If that version is already published,
the job fails and asks for a version bump (`cargo xtask version set`).

## Secrets

The macOS and draft-release jobs run in the `release` environment, which only the
`release` branch can use and which holds the signing secrets. The other jobs sign nothing and get
no secrets. Every secret is optional: a missing one produces unsigned artifacts and a
warning, never a failed build.

| Secret | Used for |
|---|---|
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD` | the Developer ID certificate (base64 `.p12`), imported into a temporary keychain |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | notarization (`APPLE_PASSWORD` is an app-specific password) |
