#!/usr/bin/env bash
#
# Record the README's demo GIFs.
#
# Recording strays against its own working tree sounds convenient and is a
# trap: the GIFs and tapes being written land in the very list being filmed, so
# a take shows half-written demo output as untracked files, and the next take
# shows the previous one. This builds a throwaway worktree at HEAD, dirties it
# with a handful of real source files, and records against that — the tool
# looking at an ordinary repository, which is what a reader needs to see.
#
# Usage: scripts/record-demos.sh [name ...]
#        scripts/record-demos.sh              # every tape
#        scripts/record-demos.sh split keys   # just those two

set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

command -v vhs >/dev/null || { echo "vhs is not installed: https://github.com/charmbracelet/vhs" >&2; exit 1; }

# A release build, because the reference and the key handling are read out of
# the binary: a stale one documents keys that no longer exist.
echo "building..."
cargo build --release --quiet
STRAYS="$root/target/release/herdr-strays"
export STRAYS

# Point the binary at a herdr that reports nothing, so it falls back to the
# repository it is standing in — the demo one. Without this it lists whatever
# is genuinely open in herdr, which during a recording session includes this
# repository, half-written demo output and all.
HERDR_BIN_PATH="${TMPDIR:-/tmp}/strays-demo-herdr"
export HERDR_BIN_PATH
printf '#!/bin/sh\nexit 1\n' > "$HERDR_BIN_PATH"
chmod +x "$HERDR_BIN_PATH"

# And clear the workspace id, so the scope starts at "this workspace" with
# nothing inherited from the terminal this script was launched from.
unset HERDR_WORKSPACE_ID

# The demo repository: HEAD, plus a few files made dirty so there is something
# to look at. Real files rather than invented ones, so the diffs on screen are
# genuine code a reader can follow.
DEMO_REPO="${TMPDIR:-/tmp}/strays-demo-repo"
export DEMO_REPO

cleanup() {
    git worktree remove --force "$DEMO_REPO" 2>/dev/null || true
}
trap cleanup EXIT

cleanup
echo "preparing demo repository at $DEMO_REPO"
git worktree add --detach --quiet "$DEMO_REPO" HEAD

# A worktree starts clean, but anything ignored here — a build directory, the
# demo output being written right now — is not carried over, and anything the
# recording itself creates would be. Clearing untracked files keeps the list on
# screen to the repository's own files rather than this script's leavings.
git -C "$DEMO_REPO" clean -qfdx

# Files whose working-tree state differs from HEAD, giving the diff pane
# something with replacements *and* one-sided additions to show. If the working
# tree is clean these copies are no-ops and the demo repository stays empty, so
# say so rather than filming an empty list.
dirty=0
for f in src/app/mod.rs src/app/scroll.rs src/config/keys.rs src/ui/diff.rs src/ui/panels.rs; do
    if ! cmp -s "$root/$f" "$DEMO_REPO/$f"; then
        cp "$root/$f" "$DEMO_REPO/$f"
        dirty=$((dirty + 1))
    fi
done

if [ "$dirty" -eq 0 ]; then
    echo "working tree matches HEAD, so there is nothing for the demo to show." >&2
    echo "record from a branch with uncommitted work, or edit one of the files above." >&2
    exit 1
fi
echo "$dirty files differ from HEAD"

tapes=("$@")
if [ ${#tapes[@]} -eq 0 ]; then
    tapes=(screenshot diff split search base show-all keys views)
fi

for name in "${tapes[@]}"; do
    tape="docs/demo/$name.tape"
    [ -f "$tape" ] || { echo "no such tape: $tape" >&2; exit 1; }
    echo "recording $name..."
    vhs "$tape"

    # The hero still is the last frame of its own short recording: vhs has no
    # PNG output, so the frame is cut out here and the recording thrown away.
    if [ "$name" = screenshot ]; then
        command -v ffmpeg >/dev/null || { echo "ffmpeg is needed for the still" >&2; exit 1; }
        ffmpeg -v error -sseof -0.1 -i docs/demo/screenshot.gif \
            -vframes 1 -y docs/screenshot.png
        rm -f docs/demo/screenshot.gif
        echo "  wrote docs/screenshot.png"
    fi
done

echo "done. Check the result before committing:"
echo "  the pane should never be left empty, and the key reference should match the current bindings."
