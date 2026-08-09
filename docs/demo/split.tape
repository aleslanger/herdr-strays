# The diff as two columns: the file as it was, beside the file as it is.
#
# Recorded on a file with both replacements and one-sided changes, because the
# two read differently and the second is what the split view is for: a blank
# opposite a line says "added" without the reader decoding a `+`.
#
# Run through scripts/record-demos.sh, which supplies $DEMO_REPO and $STRAYS.
# Reaching the file with `/` rather than a count of `j` presses keeps the take
# from drifting when the demo repository gains or loses a row.

Output docs/demo/split.gif
Output docs/demo/split.mp4

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

# Find the file by name, so the take does not depend on where it sits. The
# extension is part of the query on purpose: `/keys` alone also matches this
# very tape, and filming a brand-new file gives an empty left column — the
# split view looking broken in the one recording meant to sell it.
Type "j"
Sleep 500ms
Type "/keys.rs"
Sleep 1200ms
Enter
Sleep 2500ms

# Read it the ordinary way first, so the split is seen as a change from it.
Type "jjjj"
Sleep 2s

# Split it: before on the left, after on the right.
Type "V"
Sleep 3500ms

# Scroll, so the two columns are seen staying in step.
Type "jjjjjj"
Sleep 2500ms
Type "f"
Sleep 3s

# And back to one column.
Type "V"
Sleep 2s

Hide
Type "q"
Sleep 500ms
