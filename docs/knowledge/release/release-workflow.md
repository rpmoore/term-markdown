---
type: concept
title: Release workflow
description: How a published release triggers per-platform builds, packaging, checksums, upload.
resource: .github/workflows/release.yml
tags: [ci, github-actions, release, wix, windows]
---

# Release workflow

`.github/workflows/release.yml` runs on `release: types: [published]`
(`.github/workflows/release.yml:3-5`) and produces one binary artifact set per platform, uploaded
to the same GitHub release, then generates release notes once every platform build has finished.

## Job layout

Two independent build jobs run in a matrix/single-job split, both gated on the same version
check, followed by a `notes` job:

- `build` (`.github/workflows/release.yml:11-72`) — matrix over `linux-x86_64`,
  `linux-aarch64`, `macos-aarch64`, `macos-x86_64` (`.github/workflows/release.yml:20-32`).
  `linux-aarch64` runs natively on GitHub's hosted `ubuntu-24.04-arm` runner — no
  cross-compilation toolchain needed, same as the macOS legs. Each leg builds with
  `cargo build --release --locked`, tars the binary, and checksums it with `shasum -a 256`.
- `build-windows` (`.github/workflows/release.yml:74-139`) — single job on `windows-latest`.
  Builds the release binary, zips it with `7z`, then builds an MSI installer via `cargo-wix`
  (see below), checksums both artifacts, and uploads all four files.
- `notes` (`.github/workflows/release.yml:141-156`) — `needs: [build, build-windows]`
  (`.github/workflows/release.yml:143`), so release notes are only generated after every
  platform's artifacts have uploaded. Adding a platform build job means adding it here too, or
  `notes` will run before that platform's upload completes.

Every build job re-derives `cargo_version` from `cargo metadata` and fails with `::error::` if it
doesn't match `RELEASE_TAG` (`.github/workflows/release.yml:96-103`, mirrored in `build` at
`.github/workflows/release.yml:48-54`) — this is what forces `Cargo.toml`'s version to be bumped
before tagging, rather than after.

## Windows packaging: zip + MSI

`build-windows` produces two installable forms from the one `cargo build --release --locked`
binary (`.github/workflows/release.yml:105-106`):

- A portable zip (`.github/workflows/release.yml:108-111`), checksummed in place.
- An MSI built by `cargo-wix` (`.github/workflows/release.yml:118-122`), driven by
  `wix/main.wxs` and `wix/License.rtf`. `cargo-wix` is installed pinned to a specific version
  (`--version 0.3.9`, `.github/workflows/release.yml:119`) rather than "latest", so a future
  cargo-wix release can't silently change or break tagged-release builds; bump it deliberately.

`cargo-wix` has **no `build` subcommand** — the installer is created by invoking `cargo wix`
directly (the default/`create` command); passing a bare `build` argument is treated as a WiX
source path override and fails before producing an MSI. Because the binary is already built by
the preceding step, the invocation passes `--no-build --target-bin-dir target\release`
(`.github/workflows/release.yml:122`) so `cargo-wix` packages that binary instead of rebuilding.

`wix/main.wxs` targets 64-bit explicitly: `Package` declares `Platform='x64'`, and both
`Component` elements (`binary0`, `Path`) declare `Win64='yes'` — omitting either causes Windows
Installer to treat the package as 32-bit (wrong `Program Files` directory, HKLM registry
redirection). `wix/License.rtf` embeds the full Apache-2.0 text (not just a link to it), since
the installer's license dialog is a recipient's copy of the license under License §4(a).

`wix/main.wxs:8-17` (the `<?if $(sys.BUILDARCH) = x64 ...?>` preprocessor block, before `<Wix>`)
must stay — it's what *defines* `PlatformProgramFilesFolder`, which the `TARGETDIR` directory
tree references as `$(var.PlatformProgramFilesFolder)` (`wix/main.wxs:49`). Removing that block
(or copying only the `Directory` line without it, as happened once) fails `candle.exe` with
`CNDL0150: Undefined preprocessor variable`, only on an actual Windows/WiX run — nothing in this
repo can catch it locally, since candle only runs on `windows-latest` in CI.

## Checksum-file invariant

Every `*.sha256` file must record the artifact's **basename**, because `gh release upload`
publishes assets under their basename and a user who downloads both files into one directory
runs `sha256sum -c <file>.sha256` from that same directory. The Linux/macOS tarball checksums
(`.github/workflows/release.yml:62-63`, in `build`) and the Windows zip checksum
(`.github/workflows/release.yml:113-116`) satisfy this naturally since they run with the artifact
in the current directory. The MSI is built into `target/wix/`, so its checksum step `cd`s there
first — `(cd target/wix && sha256sum "...")` (`.github/workflows/release.yml:124-127`) — rather
than checksumming the full `target/wix/...` path, which `sha256sum -c` would then require
reproducing exactly.
