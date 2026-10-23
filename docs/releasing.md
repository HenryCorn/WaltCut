# Releasing WaltCut

Pushing a tag `vX.Y.Z` runs `.github/workflows/release.yml`. It builds the macOS (Apple silicon)
installer and creates a **draft** GitHub Release named `WaltCut X.Y.Z`. Nobody sees a draft until a
maintainer publishes it.

User-facing names say **WaltCut**. Files, binaries and ids stay lowercase
(`waltcut-<version>-<platform>-<arch>.<ext>`, `io.github.henrycorn.waltcut`).

The first releases are **unsigned**: the app is ad-hoc signed, not notarized, so Gatekeeper asks
for **right-click ▸ Open** the first time (the release notes say so). Signing and notarization
come later with an Apple Developer ID; `packaging/macos/package.sh` already supports them through
the environment variables below.

## Cutting a release

1. **Bump the version** on `main`. It lives in exactly one place, `[workspace.package] version`
   in the root `Cargo.toml`; every crate inherits it and the packaging scripts read it from there:

   ```sh
   cargo xtask version                 # prints the current version, e.g. 0.1.0
   cargo xtask version set 0.2.0       # or 0.2.0-rc.1; updates Cargo.toml and Cargo.lock
   ```

   Commit the change (`Cargo.toml` + `Cargo.lock`) through the normal review flow.
2. **Tag the merge commit** and push the tag: `git tag v0.2.0 && git push origin v0.2.0`. The
   workflow refuses a tag that does not match the Cargo version.
3. **Wait for the draft.** The Releases page gets a draft with the DMG, the CLI zip and
   `SHA256SUMS.txt`, marked as a pre-release.
4. **Check it.** Download the DMG, open it, and run the app on a clean Apple silicon Mac.
5. **Publish** the draft in the GitHub UI.

**Test runs:** *Actions › Release › Run workflow* builds and uploads the artifacts to the run
without creating a release.

## What gets built

| Platform | Artifacts | Built on |
|---|---|---|
| macOS 11+ (Apple silicon only) | `waltcut-<v>-macos-aarch64.dmg`, `waltcut-cli-<v>-macos-aarch64.zip` | `macos-15` |

`waltcut --version` and `waltcut-cli --version` print the version from `Cargo.toml`.

**Fonts.** The build checks out [craft-fonts](https://github.com/storytold/craft-fonts) at the
pinned commit in `CRAFT_FONTS_REF` (bump it deliberately) and builds with `CRAFT_FONTS_DIR`, so the
release embeds its Japanese fonts. The packages carry each embedded font's licence
(`copy_font_licences` in `packaging/env.sh`).

### macOS

`packaging/macos/package.sh --arch aarch64` builds `aarch64-apple-darwin` with
`MACOSX_DEPLOYMENT_TARGET=11.0` and assembles `WaltCut.app`:

- `Info.plist` is generated from `Info.plist.in` (bundle id `io.github.henrycorn.waltcut`, the
  version and the build commit).
- **Signing** is ad hoc unless `MACOS_SIGN_IDENTITY` is set. A real identity uses the hardened
  runtime and a secure timestamp, with the entitlements in `entitlements.plist`;
  `packaging/macos/import-cert.sh` imports a Developer ID certificate into a temporary keychain.
- **Notarization** runs when `APPLE_ID`, `APPLE_PASSWORD` (an app-specific password) and
  `APPLE_TEAM_ID` are set with a real identity: the app, then the DMG and the CLI zip, are sent
  with `xcrun notarytool submit` and the tickets are stapled.
- The app goes on a plain DMG (`hdiutil makehybrid`) with an Applications link. The volume is
  named `WaltCut`; the DMG file name keeps the version.

Locally, with no variables set, the script signs ad hoc and skips notarization, which is enough to
check the bundle and the DMG on your own Mac (`packaging/macos/package.sh`).
