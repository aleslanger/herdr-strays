# Comparing against HEAD, or against the branch point.
#
# The panel title is the thing to watch: it names which comparison is on
# screen, and without it the two states are hard to tell apart from the diff
# alone. The pauses after each `m` are long enough to read it.
#
# Run through scripts/record-demos.sh, which supplies $DEMO_REPO and $STRAYS.

Output docs/demo/base.gif
Output docs/demo/base.mp4

Set Shell bash
Set FontSize 16
Set Width 1500
Set Height 800
Set Padding 12
Set Framerate 24

Hide
Type "cd $DEMO_REPO && clear"
Enter
Sleep 500ms
Type "$STRAYS"
Enter
Sleep 3s
Show

Sleep 1s

Type "j"
Sleep 500ms
# `app/mod.rs` is the file to use here: it exists on `origin/master`, so the
# two comparisons genuinely differ. A file added since the branch point reads
# as one solid block of additions against the branch point and shows nothing
# about the toggle, which is what this recording is for.
Type "/app/mod.rs"
Sleep 1200ms
Enter
Sleep 2500ms

# Against HEAD: what is not committed yet.
Type "jjjj"
Sleep 2s

# Against the branch point: everything the branch has done.
Type "m"
Sleep 3500ms
Type "jjjj"
Sleep 2s

# And back.
Type "m"
Sleep 3s

Hide
Type "q"
Sleep 500ms
