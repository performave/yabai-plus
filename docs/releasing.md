# Releasing yabai-plus

This document describes how a yabai-plus release is cut. Releases are produced
automatically by the [`release` workflow](../.github/workflows/release.yml) when a
version tag is pushed. You should rarely need to build a release by hand.

> One-time setup (Apple Developer account, GitHub secrets) lives in
> [ci-setup.md](./ci-setup.md). Read that first if releases have never run on
> this repo.

## TL;DR

```bash
# 1. Bump the upstream fallback version in src/yabai.c (MAJOR/MINOR/PATCH).
# 2. Review and update CHANGELOG.md; make sure the notes accurately describe this release.
# 3. Commit, then tag and push:
git tag v7.1.25-plus.1
git push origin v7.1.25-plus.1
```

Pushing the tag triggers the workflow, which builds, signs, notarizes, and
creates a GitHub Release with the archive attached.

Before tagging, confirm `CHANGELOG.md` has a heading matching the release tag
without the leading `v` (for example, `## [7.1.25-plus.1]`) and that its notes
are accurate for the commits included in the release. The workflow uses that
section as the GitHub Release body when it exists.

## Versioning scheme

yabai-plus tracks upstream yabai and adds patches on top. To stay distinguishable
from upstream while remaining sortable, releases use the upstream version plus a
`-plus.N` suffix:

```
v<upstream-version>-plus.<n>
   e.g. v7.1.25-plus.1, v7.1.25-plus.2, v7.1.26-plus.1
```

The version string is compiled into the binary from `src/yabai.c`:

```c
#define MAJOR 7
#define MINOR 1
#define PATCH 25
```

`yabai --version` prints `yabai-${YABAI_VERSION}`. For release builds, the GitHub
Actions workflow passes the pushed tag into `make`, so a tag like
`v7.1.25-plus.1` produces `yabai-v7.1.25-plus.1` and an archive named from that
output. Local builds on an exact tag pick up that tag via `git describe`; untagged
builds fall back to the upstream version string in `src/yabai.c`.

## What the release workflow does

On a `v*` tag push (`.github/workflows/release.yml`), a `macos-14` runner:

1. **Imports** the Developer ID Application certificate into a throwaway keychain.
2. **Builds** a universal (x86_64 + arm64) binary with `make install`.
3. **Builds** the man page (`make man`, needs `asciidoctor`).
4. **Codesigns** `bin/yabai` with the hardened runtime and a secure timestamp
   (`codesign --force --timestamp --options runtime --sign "$APPLE_SIGNING_IDENTITY"`).
5. **Notarizes** via `xcrun notarytool submit --wait` using an App Store Connect
   API key.
6. **Assembles** `bin/yabai-v<version>.tar.gz` containing `bin/`, `doc/`, `examples/`, plus a `.sha256` checksum file.
7. **Creates** a GitHub Release for the tag with the tarball attached.

## Signing & notarization notes

These are the non-obvious bits that cause most release failures:

- **Hardened runtime is required for notarization** (`--options runtime`). It does
  **not** interfere with the scripting addition — SA injection into Dock.app is
  gated by partially-disabled SIP + root, a separate mechanism. Keep the flag.
- **Only the main `bin/yabai` binary is signed.** The scripting-addition
  `payload`/`loader` are compiled into the binary (`src/osax/*_bin.c`) and injected
  into Dock at runtime; they must **not** be hardened-runtime signed. The workflow
  never touches them — leave it that way.
- **A bare CLI binary cannot be stapled.** Notarization still succeeds; Gatekeeper
  verifies the ticket online on first run. `xcrun stapler staple bin/yabai` will
  fail and that is expected. If you ever need offline-trusted installs, ship a
  `.pkg` and staple that instead.
- **Use a stable Developer ID across releases.** TCC (Accessibility/Automation)
  permissions are bound to the signature; keeping the same identity means users
  grant permission once and it persists across updates.

## Building a release manually (fallback)

If CI is unavailable:

```bash
make install VERSION="v7.1.25-plus.1"  # universal build into bin/yabai
make man              # man page (requires asciidoctor)
codesign --force --timestamp --options runtime \
  --sign "Developer ID Application: <Your Name> (TEAMID)" bin/yabai
codesign --verify --strict --verbose=2 bin/yabai

# notarize
ditto -c -k --keepParent bin/yabai /tmp/yabai-notarize.zip
xcrun notarytool submit /tmp/yabai-notarize.zip \
  --key /path/to/AuthKey_XXXX.p8 --key-id <KEY_ID> --issuer <ISSUER_ID> --wait

# archive (matches install.sh expectations)
VERSION="$(bin/yabai --version)"
rm -rf archive && mkdir archive
cp -r bin doc examples archive/
tar -cvzf "bin/${VERSION}.tar.gz" archive
rm -rf archive
SHA256="$(shasum -a 256 "bin/${VERSION}.tar.gz" | cut -d' ' -f1)"
printf '%s  %s.tar.gz\n' "${SHA256}" "${VERSION}" > "bin/${VERSION}.tar.gz.sha256"

# publish
gh release create v7.1.25-plus.1 \
  "bin/${VERSION}.tar.gz" \
  "bin/${VERSION}.tar.gz.sha256" \
  --generate-notes
```

## Homebrew tap

Releases are published to the [`Performave/homebrew-tap`](https://github.com/Performave/homebrew-tap)
tap as the `yabai-plus` formula:

```bash
brew install Performave/tap/yabai-plus
```

The release workflow's **Bump Homebrew tap formula** step updates the formula's
`url`, `version`, and `sha256` automatically on every `v*` tag. It requires a
`HOMEBREW_TAP_TOKEN` Actions secret — a token with `contents:write` on the tap
repo (a fine-grained PAT scoped to `Performave/homebrew-tap` is enough). If the
secret is absent the step is skipped (the release itself still succeeds), and you
can bump the formula by hand.

## After releasing

- `scripts/install.sh` carries a hard-coded `VERSION` and verifies downloads using
  the release's `.sha256` asset. If you distribute via that script, update
  `VERSION` before tagging.
