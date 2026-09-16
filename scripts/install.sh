#!/usr/bin/env bash
# term-markdown installer — fetches the latest (or a pinned) GitHub release
# binary and installs it to a directory on your PATH. No root/sudo, no
# service, no config — term-markdown is a plain CLI, not a daemon.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/rpmoore/term-markdown/main/scripts/install.sh | bash
#   curl -fsSL .../install.sh | bash -s -- --yes
#   curl -fsSL .../install.sh | bash -s -- --version v0.1.2
#   curl -fsSL .../install.sh | bash -s -- --install-dir /usr/local/bin

set -euo pipefail

REPO="rpmoore/term-markdown"
BIN_NAME="term-markdown"
INSTALL_DIR="${TERM_MARKDOWN_INSTALL_DIR:-$HOME/.local/bin}"

VERSION_OVERRIDE=""
AUTO_YES=0

log() { printf '==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# True (0) if $1 is a strictly lower dotted version than $2.
version_lt() {
  [ "$1" = "$2" ] && return 1
  [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | head -n1)" = "$1" ]
}

# True (0) if $1 is a plain dotted-numeric version (e.g. "0.1.4") that's
# safe to feed to version_lt.
looks_like_version() {
  case "$1" in
    ''|*[!0-9.]*) return 1 ;;
  esac
}

# Verifies "$1" against the checksum file "$1.sha256" (format: `<hex>  <file>`,
# produced by both GNU sha256sum and BSD/macOS shasum -a 256), using whichever
# of the two tools is available on this machine.
sha256_check() {
  local file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c "$file.sha256"
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -c "$file.sha256"
  else
    die "neither sha256sum nor shasum found; cannot verify checksum"
  fi
}

usage() {
  cat <<'EOF'
Usage: install.sh [--yes] [--version <tag>] [--install-dir <path>] [--help]

  --yes, -y              Skip the downgrade confirmation prompt.
  --version <tag>        Install a specific release tag (e.g. v0.1.2) instead
                          of latest — this can also downgrade an existing
                          install. Downgrading prompts for confirmation
                          unless --yes (or TERM_MARKDOWN_INSTALL_YES=1).
  --install-dir <path>   Install location (default: ~/.local/bin, or
                          $TERM_MARKDOWN_INSTALL_DIR if set).
  --help                 Show this help.

Environment variable equivalents (useful for scripted/CI installs):
  TERM_MARKDOWN_INSTALL_YES=1
  TERM_MARKDOWN_INSTALL_DIR=<path>
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --yes|-y) AUTO_YES=1 ;;
    --version)
      shift
      [ $# -gt 0 ] || die "--version requires an argument"
      VERSION_OVERRIDE="$1"
      ;;
    --install-dir)
      shift
      [ $# -gt 0 ] || die "--install-dir requires an argument"
      INSTALL_DIR="$1"
      ;;
    --help|-h) usage; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
  esac
  shift
done

[ "${TERM_MARKDOWN_INSTALL_YES:-0}" = "1" ] && AUTO_YES=1

# --- Preflight -----------------------------------------------------------

os="$(uname -s)"
arch="$(uname -m)"

case "$os/$arch" in
  Linux/x86_64|Linux/amd64) platform="linux-x86_64" ;;
  Linux/aarch64|Linux/arm64) platform="linux-aarch64" ;;
  Darwin/arm64|Darwin/aarch64) platform="macos-aarch64" ;;
  Darwin/x86_64) platform="macos-x86_64" ;;
  *) die "unsupported platform: $os/$arch (supported: Linux x86_64/arm64, macOS arm64/x86_64)" ;;
esac

# Release binaries for Linux (x86_64 and arm64) are built on GitHub-hosted
# Ubuntu runners and are glibc-linked. A musl-only system (e.g. Alpine)
# reports as Linux too, but the binary won't run there — ldd itself is the
# musl libc on those systems and says so.
if [ "$os" = "Linux" ] && command -v ldd >/dev/null 2>&1 && ldd --version 2>&1 | grep -qi musl; then
  die "term-markdown's release binary is glibc-linked and will not run on musl-based systems (e.g. Alpine); detected musl libc"
fi

for tool in curl tar grep sed head install mktemp sort awk; do
  command -v "$tool" >/dev/null 2>&1 || die "required tool '$tool' not found; install it and re-run"
done
if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
  die "neither sha256sum nor shasum found; install one and re-run"
fi

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

# --- Resolve version -------------------------------------------------------

if [ -n "$VERSION_OVERRIDE" ]; then
  TAG="$VERSION_OVERRIDE"
else
  log "Looking up latest release for ${REPO}..."
  http_code="$(curl -sSL -o "$TMPDIR/latest.json" -w '%{http_code}' \
    "https://api.github.com/repos/${REPO}/releases/latest" || true)"
  [ -n "$http_code" ] || http_code="000"
  case "$http_code" in
    200) ;;
    404) die "no published release found for ${REPO} yet" ;;
    403) die "GitHub API rate limit hit while resolving the latest release; try again shortly" ;;
    *) die "failed to query GitHub releases API (HTTP $http_code)" ;;
  esac
  TAG="$(grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' "$TMPDIR/latest.json" \
    | head -1 | sed -E 's/.*"([^"]*)"$/\1/')"
  [ -n "$TAG" ] || die "could not parse a release tag from the GitHub API response"
fi

case "$TAG" in
  *[!A-Za-z0-9._-]*|"") die "invalid release tag: '$TAG'" ;;
esac

# --- Warn/confirm on downgrade ----------------------------------------------

installed_version=""
# Reject a symlink at this path outright rather than executing whatever it
# points to; only run a plain, non-symlink, executable regular file (this
# does not check ownership/permissions beyond that).
if [ -f "$INSTALL_DIR/$BIN_NAME" ] && [ ! -L "$INSTALL_DIR/$BIN_NAME" ] && [ -x "$INSTALL_DIR/$BIN_NAME" ]; then
  installed_version="$("$INSTALL_DIR/$BIN_NAME" --version 2>/dev/null | awk '{print $2}')" || true
fi

target_version="${TAG#v}"
installed_version="${installed_version#v}"

# installed_version fails to look like a version for a fresh install, if the
# existing binary predates --version, or if its output was garbage.
# target_version fails to look like one for a non-semver tag (e.g. "latest").
# Either way, skip the downgrade check rather than risk a wrong verdict.
if looks_like_version "$installed_version" && looks_like_version "$target_version" \
  && version_lt "$target_version" "$installed_version"; then
  warn "downgrading term-markdown: installed version is ${installed_version}, requested is ${target_version} (${TAG})."
  if [ "$AUTO_YES" -eq 1 ]; then
    log "proceeding with downgrade (--yes/TERM_MARKDOWN_INSTALL_YES set)."
  elif [ -r /dev/tty ] && { exec 3<>/dev/tty; } 2>/dev/null; then
    printf 'Continue with downgrade to %s? [y/N] ' "$TAG" >&3
    read -r reply <&3
    exec 3<&-
    case "$reply" in
      [yY]|[yY][eE][sS]) ;;
      *) die "downgrade cancelled" ;;
    esac
  else
    die "downgrading from ${installed_version} to ${target_version} requires confirmation; re-run interactively, or pass --yes (or TERM_MARKDOWN_INSTALL_YES=1) to confirm non-interactively."
  fi
fi

log "Installing term-markdown ${TAG} (${platform})"

ASSET="term-markdown-${TAG}-${platform}.tar.gz"
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"
CHECKSUM_URL="${DOWNLOAD_URL}.sha256"

# --- Download & verify -----------------------------------------------------

log "Downloading ${ASSET}..."
curl -fsSL --retry 3 -o "$TMPDIR/$ASSET" "$DOWNLOAD_URL" \
  || die "failed to download $DOWNLOAD_URL (check that this release/asset exists)"

log "Verifying download integrity..."
curl -fsSL --retry 3 -o "$TMPDIR/$ASSET.sha256" "$CHECKSUM_URL" \
  || die "failed to download checksum file $CHECKSUM_URL — refusing to install without an integrity check"

# Note: this only proves the tarball matches what release.yml published — the
# checksum comes from the same GitHub origin as the binary, so it guards
# against a corrupted/incomplete download, not against a compromised release.
( cd "$TMPDIR" && sha256_check "$ASSET" ) \
  || die "checksum mismatch for $ASSET — the download is corrupted or was tampered with in transit"

# --- Extract & install -------------------------------------------------------

archive_members="$(tar -tzf "$TMPDIR/$ASSET")"
[ "$archive_members" = "$BIN_NAME" ] \
  || die "unexpected archive contents (expected only '$BIN_NAME', got: $archive_members)"

tar -xzf "$TMPDIR/$ASSET" -C "$TMPDIR"
[ -f "$TMPDIR/$BIN_NAME" ] || die "extracted archive did not contain a '$BIN_NAME' binary"

install -d -m 0755 "$INSTALL_DIR"
install -m 0755 "$TMPDIR/$BIN_NAME" "$INSTALL_DIR/$BIN_NAME"

# --- Summary -----------------------------------------------------------

echo
log "term-markdown ${TAG} installed to ${INSTALL_DIR}/${BIN_NAME}"

case ":$PATH:" in
  *":$INSTALL_DIR:"*)
    log "Run it with: ${BIN_NAME} <file.md>"
    ;;
  *)
    warn "${INSTALL_DIR} is not on your PATH."
    echo
    case "${SHELL:-}" in
      */fish)
        echo "    Add it (fish):"
        echo "      fish_add_path ${INSTALL_DIR}"
        ;;
      *)
        echo "    Add it by appending this to your shell profile"
        echo "    (~/.bashrc, ~/.zshrc, etc.), then restart your shell:"
        echo "      export PATH=\"${INSTALL_DIR}:\$PATH\""
        ;;
    esac
    echo
    echo "    Or run it directly for now: ${INSTALL_DIR}/${BIN_NAME} <file.md>"
    ;;
esac
