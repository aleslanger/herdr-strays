#!/bin/sh
# Put the strays binary in place, without asking the user for a Rust toolchain.
#
# Run by `herdr plugin install` through the manifest's [[build]] step. herdr has
# no manifest field for prebuilt artefacts — a build step is an argv command and
# nothing else — so fetching a release is this script's job.
#
# Two ways to end up with a binary, in order:
#
#   1. Download the release built for this platform and verify its checksum.
#   2. Failing that, compile from source with cargo.
#
# The download is what removes the toolchain requirement; the build is what
# keeps unusual platforms and offline checkouts working. Either way the binary
# lands at bin/herdr-strays, which is what the manifest launches.
#
# herdr runs plugin commands with a minimal PATH, so nothing here depends on
# more than a POSIX shell, curl or wget, and a SHA-256 tool.
set -eu

# The plugin root: herdr injects it, but `sh scripts/install.sh` from a checkout
# should work too.
ROOT="${HERDR_PLUGIN_ROOT:-$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)}"
[ -n "$ROOT" ] && [ -d "$ROOT" ] ||
  { printf 'strays: cannot locate the plugin root\n' >&2; exit 1; }
BIN_DIR="$ROOT/bin"
BIN="$BIN_DIR/herdr-strays"

REPO="aleslanger/herdr-strays"

say() { printf 'strays: %s\n' "$1" >&2; }
die() { say "$1"; exit 1; }

# ---------------------------------------------------------------- version ---

# The manifest is the single source of truth for the version, and a test keeps
# it in step with Cargo.toml. Read the first `version` line so a later key
# holding the same word cannot win.
version=$(
  sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "$ROOT/herdr-plugin.toml" 2>/dev/null |
    head -n 1
) || version=""
[ -n "$version" ] || die "could not read the version from herdr-plugin.toml"

# --------------------------------------------------------------- platform ---

# musl for linux: a statically linked binary runs whatever the host's glibc is,
# which matters when the release is built on newer CI than the user's machine.
target=""
case "$(uname -s)" in
  Linux)
    case "$(uname -m)" in
      x86_64 | amd64) target="x86_64-unknown-linux-musl" ;;
      aarch64 | arm64) target="aarch64-unknown-linux-musl" ;;
    esac
    ;;
  Darwin)
    case "$(uname -m)" in
      x86_64) target="x86_64-apple-darwin" ;;
      arm64) target="aarch64-apple-darwin" ;;
    esac
    ;;
esac

# --------------------------------------------------------------- download ---

# Fetch $1 into $2. Retries: a release published seconds ago may not have
# reached every GitHub CDN edge yet.
fetch() {
  if command -v curl >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -fsSL --retry 3 --retry-delay 2 \
      -o "$2" -- "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget --https-only -q --tries=3 -O "$2" -- "$1"
  else
    return 1
  fi
}

# Print the SHA-256 of $1, whichever tool this system ships.
sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -- "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -- "$1" | cut -d' ' -f1
  else
    return 1
  fi
}

# Scratch space for the download, removed however this script ends. Set up once
# at the top level rather than inside the function: a trap is process-wide, so
# arming it per call would leave the fallback build running under a trap that
# names an already-deleted directory.
tmp=""
cleanup() { [ -n "$tmp" ] && rm -rf -- "$tmp"; }
trap cleanup EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

# Try the release for this platform. Returns non-zero if anything at all goes
# wrong, so the caller can fall back to building.
try_download() {
  [ -n "$target" ] || return 1

  archive="herdr-strays-$target.tar.gz"
  base="https://github.com/$REPO/releases/download/v$version"

  tmp=$(mktemp -d) || return 1

  say "downloading $archive"
  fetch "$base/$archive" "$tmp/$archive" || return 1

  # No checksum, no install: an unverified binary is worse than a slow build.
  fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || return 1

  want=$(grep -F " $archive" "$tmp/SHA256SUMS" | cut -d' ' -f1 | head -n 1) || return 1
  [ -n "$want" ] || return 1

  got=$(sha256_of "$tmp/$archive") || return 1
  if [ "$got" != "$want" ]; then
    say "checksum mismatch for $archive — refusing it"
    return 1
  fi

  tar -xzf "$tmp/$archive" -C "$tmp" || return 1
  [ -f "$tmp/herdr-strays" ] || return 1

  mkdir -p "$BIN_DIR"
  # Move into place only once the bytes are verified, so a half-written binary
  # is never launchable. Falls back to a copy when $TMPDIR is on another
  # filesystem, where rename(2) cannot cross the boundary.
  mv -- "$tmp/herdr-strays" "$BIN" 2>/dev/null ||
    cp -- "$tmp/herdr-strays" "$BIN" || return 1
  chmod 0755 "$BIN"
  return 0
}

# ------------------------------------------------------------------ build ---

try_build() {
  command -v cargo >/dev/null 2>&1 || return 1

  say "building from source"
  ( cd -- "$ROOT" && cargo build --release ) || return 1
  [ -f "$ROOT/target/release/herdr-strays" ] || return 1

  # Copied rather than symlinked: `cargo clean` should not leave the manifest
  # pointing at a path that no longer exists.
  mkdir -p "$BIN_DIR"
  cp -- "$ROOT/target/release/herdr-strays" "$BIN"
  chmod 0755 "$BIN"
  return 0
}

# ------------------------------------------------------------------- main ---

if try_download; then
  say "installed $version"
  exit 0
fi

if [ -n "$target" ]; then
  say "could not fetch the release for $target — falling back to a build"
else
  say "no release is published for $(uname -s) $(uname -m) — building instead"
fi

if try_build; then
  say "built $version"
  exit 0
fi

die "no prebuilt binary and no cargo to build one.
  Install Rust: https://rustup.rs
  Then restart the herdr server from a shell that can find cargo:
    herdr server stop && herdr server"
