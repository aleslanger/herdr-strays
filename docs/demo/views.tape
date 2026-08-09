# The capitals: blame, history, the log graph, and what has been stashed.
#
# Each one is a second view of the same file, and each is given long enough to
# be read rather than merely flashed. `L` comes before `S` because the log
# graph is the fullest of them and the stash list the emptiest — ending on the
# thinnest screen would leave the reader with the weakest impression.
#
# Run through scripts/record-demos.sh, which supplies $DEMO_REPO and $STRAYS.

Output docs/demo/views.gif
Output docs/demo/views.mp4

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
Type "/keys.rs"
Sleep 1200ms
Enter
Sleep 2s

# Who last touched each line.
Type "B"
Sleep 3500ms
Type "B"
Sleep 800ms

# The commits that touched this file.
Type "H"
Sleep 3s
Type "H"
Sleep 800ms

# The shape of recent history across the repository.
Type "L"
Sleep 4s
Type "L"
Sleep 800ms

# The branches here, and what they compare against.
Type "W"
Sleep 3s
Type "W"
Sleep 1500ms

Hide
Type "q"
Sleep 500ms
