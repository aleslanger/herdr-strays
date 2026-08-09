# Every tracked file, with the changed ones standing out.
#
# The point of `a` is that a change is legible against the repository around
# it, so the take has to end on a changed file with its diff open. An earlier
# one left the cursor on an unchanged file, and a reader saw a full list beside
# an empty pane — the feature looking like a failure. Reaching the file with
# `/` rather than a count of `j` presses is what keeps that from recurring.
#
# Run through scripts/record-demos.sh, which supplies $DEMO_REPO and $STRAYS.

Output docs/demo/show-all.gif
Output docs/demo/show-all.mp4

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

# The list as it opens: only what strayed.
Sleep 2s

# `a` brings in everything else, dimmed around the changes.
Type "a"
Sleep 3s

# A changed file still stands out in the crowd — and still opens.
Type "j"
Sleep 500ms
Type "/panels"
Sleep 1200ms
Enter
Sleep 3s

# Read a little of it, which is what the reader would do next.
Type "jjjjjj"
Sleep 2500ms

Hide
Type "q"
Sleep 500ms
