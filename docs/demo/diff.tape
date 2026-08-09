# The first thing a reader does: pick a file and read what changed in it.
#
# This is the README's opening moving picture, so it stays plain — move, open,
# read. Anything cleverer belongs in the tape for the feature it shows.
#
# Run through scripts/record-demos.sh, which supplies $DEMO_REPO and $STRAYS.

Output docs/demo/diff.gif
Output docs/demo/diff.mp4

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

# The list: what strayed from HEAD, across the projects that are open.
Sleep 2s

# Move through it, so the list is seen as a list.
Type "jj"
Sleep 1200ms
Type "j"
Sleep 1200ms

# Open one and read it.
Enter
Sleep 3s
Type "jjjjjj"
Sleep 2s
Type "f"
Sleep 2500ms

Hide
Type "q"
Sleep 500ms
