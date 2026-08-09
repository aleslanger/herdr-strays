# The still at the top of the README.
#
# A single frame has to carry the whole idea on its own, so it shows the split
# diff rather than the plain one: two columns of real code, the list beside
# them, syntax colour and word-level emphasis all visible at once.
#
# Writes into docs/ rather than docs/demo/, because it is the hero image and
# not one of the walkthrough recordings.
#
# Run through scripts/record-demos.sh, which supplies $DEMO_REPO and $STRAYS.

# A GIF, not a PNG: vhs writes gif, mp4, webm and ascii, and silently ignores
# an unknown extension rather than failing. record-demos.sh pulls the still out
# of the last frame with ffmpeg.
Output docs/demo/screenshot.gif

Set Shell bash
Set FontSize 16
Set Width 1500
Set Height 800
Set Padding 12

Hide
Type "cd $DEMO_REPO && clear"
Enter
Sleep 500ms
Type "$STRAYS"
Enter
Sleep 3s

# `ui/panels.rs`: its diff has a replaced signature, a block of added comment
# lines with nothing opposite them, and word-level changes within a line — the
# three things the split view exists to show, on one screen.
#
# Walked to rather than filtered to. `/` leaves its filter live, and the still
# would then show a one-row list — a tool for finding files, with no files in
# it, which is the opposite of the point. Directory rows count too, so this
# lands on the last file rather than the sixth row.
Type "jjjjjj"
Sleep 1500ms
Enter
Sleep 2s

# Into the split, then down to where the change is dense rather than at the
# file header, which is mostly grey metadata.
Type "V"
Sleep 1s
Type "jjjjjjjjjj"
Sleep 2s
Show

# One frame is all that is kept.
Sleep 100ms

Hide
Type "q"
Sleep 500ms
