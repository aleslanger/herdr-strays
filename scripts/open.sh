#!/bin/sh
# Launch-or-focus the strays pane.
#
# herdr actions run a command rather than declaring "open this pane", so this
# shells out to the herdr CLI. $HERDR_BIN_PATH is injected by herdr; falling
# back to `herdr` on PATH keeps the script usable when run by hand.
#
# Pressing the key twice must not stack panes. herdr has no "open if absent"
# flag, so this looks for an existing pane first:
#
#   no strays pane in this workspace -> open one
#   one exists but is not focused    -> focus it
#   the focused pane IS ours         -> close it (press again to dismiss)
#
# A pane is ours when its `label` is the manifest entrypoint title, which herdr
# sets on the panes it opens for us. `pane list` carries no plugin ownership
# field (verified against herdr 0.7.3), so the label is the only handle.
#
# herdr runs plugin commands with a minimal PATH, which is why nothing here
# depends on anything beyond the herdr binary and a POSIX shell.
set -eu

herdr_bin="${HERDR_BIN_PATH:-herdr}"
label="strays"

panes=$("$herdr_bin" pane list 2>/dev/null) || panes=""

# Scope the search to the workspace we were invoked in, so a pane sitting in
# another workspace does not suppress the one being asked for here. Without a
# workspace id, fall back to matching anywhere.
want_ws="${HERDR_WORKSPACE_ID:-}"

# Emit "<pane_id> <focused>" for each of our panes in the target workspace.
# Written in awk rather than jq: jq is not guaranteed on the minimal PATH.
#
# Records are delimited by the `{` that opens each pane object, not by a
# leading key name. herdr serialises object keys alphabetically, so which key
# comes first depends on which optional fields a pane happens to carry: a pane
# with no `agent` would be glued onto the previous record and its label read
# from the wrong pane.
#
# Nested objects (`agent_session`, `scroll`) also open with `{`, so brace depth
# is tracked and only depth-1 objects inside the `panes` array are taken as
# records. That is the same rule the Rust side applies (see discover::records).
#
# Brace depth has to survive line boundaries, so the lines are joined and the
# scan runs once in END. herdr answers on a single line today, but a parser
# that silently mis-reads a pretty-printed one is what is being fixed here.
found=$(printf '%s' "$panes" | awk -v label="$label" -v want="$want_ws" '
{ doc = doc $0 }

END {
  # Walk the document once, cutting out each top-level pane object.
  depth = 0
  start = 0
  for (c = 1; c <= length(doc); c++) {
    ch = substr(doc, c, 1)

    if (ch == "{") {
      depth++
      # Depth 3 is a record: envelope { result { panes [ { here } ] } }.
      if (depth == 3 && start == 0) start = c
    } else if (ch == "}") {
      if (depth == 3 && start != 0) {
        emit(substr(doc, start, c - start + 1), label, want)
        start = 0
      }
      depth--
    }
  }
}

function emit(rec, label, want,   id, focused) {
  if (rec !~ ("\"label\":\"" label "\"")) return
  if (want != "" && rec !~ ("\"workspace_id\":\"" want "\"")) return
  if (!match(rec, /"pane_id":"[^"]+"/)) return

  id = substr(rec, RSTART + 11, RLENGTH - 12)
  focused = (rec ~ /"focused":true/) ? "yes" : "no"
  print id, focused
}')

if [ -z "$found" ]; then
  exec "$herdr_bin" plugin pane open \
    --plugin aleslanger.strays \
    --entrypoint strays \
    --placement split \
    --direction right \
    --focus
fi

# Prefer acting on a focused pane; otherwise take the first one found.
target=$(printf '%s\n' "$found" | awk '$2 == "yes" { print $1; exit }')
if [ -n "$target" ]; then
  # Already looking at it: close it, so the same key dismisses the pane.
  exec "$herdr_bin" pane close "$target"
fi

target=$(printf '%s\n' "$found" | awk 'NR == 1 { print $1 }')
exec "$herdr_bin" plugin pane focus "$target"
