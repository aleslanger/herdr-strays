# The built-in key reference, and the scrolling that reaches the end of it.
#
# Recorded against a freshly built binary on purpose: the reference is
# generated from the live bindings, so a stale binary silently documents keys
# that no longer exist and omits ones that do.
#
# Run through scripts/record-demos.sh, which supplies $DEMO_REPO and $STRAYS.

Output docs/demo/keys.gif
Output docs/demo/keys.mp4

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

# Open the reference.
Type "?"
Sleep 3s

# It is taller than the pane, which is the whole reason it scrolls. Page down
# to the end so the keys below the fold are seen, not just claimed.
Type "f"
Sleep 2500ms
Type "f"
Sleep 2500ms
Type "G"
Sleep 2500ms

# Back to the top and away.
Type "g"
Sleep 2s
Type "?"
Sleep 1500ms

Hide
Type "q"
Sleep 500ms
