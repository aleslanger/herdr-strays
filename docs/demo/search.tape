# Searching inside an open diff, and stepping through the matches.
#
# The query is a word that occurs several times in the file on purpose: with a
# single hit `n` has nothing to do, and the half of the feature that matters —
# walking the matches without leaving the file — would not be on screen.
#
# Run through scripts/record-demos.sh, which supplies $DEMO_REPO and $STRAYS.

Output docs/demo/search.gif
Output docs/demo/search.mp4

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

# Walk to the file rather than filtering to it. `/` would be quicker, but its
# filter stays live after `⏎` — the letters of the query below would go into it
# instead of into the diff — and `Esc` clears the selection along with the
# filter.
#
# Nine rows down, not five: the directories between the files are rows too.
# The demo repository dirties a fixed set of files, so the count is stable.
Type "jjjjjjjjj"
Sleep 1800ms
Enter
Sleep 2500ms

# Search inside the diff.
Type "s"
Sleep 800ms
Type "Span"
Sleep 2000ms

# Commit the query before stepping. While it is still being typed the `n`
# below is another letter of it — the query becomes "Spannnn" and matches
# nothing, which is exactly what the first take of this recording showed.
Enter
Sleep 1500ms

# Walk the matches. Every hit stays highlighted, so the others are visible
# without stepping to them.
Type "n"
Sleep 1800ms
Type "n"
Sleep 1800ms
Type "n"
Sleep 2500ms

Hide
Type "q"
Sleep 500ms
