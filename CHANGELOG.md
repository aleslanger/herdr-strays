# Changelog

Notable changes to strays. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions
follow [semantic versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.1] - 2026-08-09

### Fixed

- Three tests failed on Windows over checkout settings rather than anything
  they were testing. The two that read the README through `include_str!` got
  the file as CRLF and split on `"\n\n"`, which a CRLF file does not contain;
  the TOML parser then rejected the stray `\r`. They normalise the line endings
  first. The third built a path containing a backslash — a separator on
  Windows, so `PathBuf` rewrote it and the round trip was compared against a
  string the path never held.

## [1.0.0] - 2026-08-09

The keys, the panes and the config file are settled enough to promise not to
move them under you: from here a breaking change costs a major version.

### Added

- **Asking for the writes.** `Y` opens the git actions: `Yc` commit, `Ys`
  stage, `Yu` unstage, `Yt` stash, `Yl` lazygit. Commit and stash open a line to
  type a message on; staging and unstaging need nothing and go straight out.
  Strays still writes nothing itself — it composes the request and types it into
  the Claude agent working in that repository, leaving the cursor at the end of
  it. You press Enter there, or you do not. That is deliberate: an agent is
  usually mid-edit in the same repository, and a write from underneath it would
  land in the middle of whatever it is doing. Discard, reset and force push are
  absent by design; they destroy work rather than record it.
- **Finding things.** `/` narrows the file list to what a query names, matching
  as a subsequence so three characters reach a deep path. `s` searches inside
  the diff, with `n` and `N` stepping between matches, wrapping at the ends and
  saying when they did. Both use smart case: a query stays case-insensitive
  until you type a capital.
- **Word-level diffs.** When a line is replaced, the words that actually
  changed are picked out within it, so a one-identifier rename in a long line
  no longer has to be found by eye. The line keeps its red or green, and only
  the differing words are emphasised; a pair too dissimilar to be a rewrite of
  each other falls back to the whole-line colour rather than highlighting
  everything.
- **The old code beside the new.** `V` splits the diff into two columns, the
  way an editor shows a comparison: a replaced line sits opposite its
  replacement, and a line only added or only removed leaves the other column
  blank, so the shape of a change reads without decoding the `+` and `-` marks.
  The pairing is positional, which is what a unified diff supports — it records
  that a run of removals precedes a run of additions, not which line became
  which. Where the runs differ in length the shorter runs out and the remaining
  lines stand alone, because three lines becoming one is not three pairs. Off
  by default: one column reads fine in a narrow pane, and the split wants
  roughly twice the width.
- **A key reference that scrolls.** `?` grew past the height of its pane, so
  the last rows could not be reached. It scrolls on its own offset, leaving the
  diff behind it where you put it, and winds back to the top when closed.
- **The shape of recent history.** `L` draws the commit graph as git draws it,
  branches and merges included, so where work diverged and rejoined is visible
  rather than inferred. Enter shows what a commit did. The connector lines
  between commits are drawn but never selectable — there is no revision on a
  piece of drawn line.
- **The branches here.** `W` lists them — name, how far each has drifted from
  its upstream, when it last moved, and what its latest commit said. Most
  recently committed first, so what you were working on is at the top. Enter
  compares against the merge base with the chosen branch, which answers "what
  have I got that this branch has not"; the branch you are on is refused with a
  reason rather than shown as an empty diff.
- **What has been set aside.** `S` lists the stashes, and Enter shows what one
  holds. A stash is a commit, so the diff points at it exactly as at any other
  revision — with the syntax highlighting, search and annotations that come
  with that. Read-only, like everything else here: nothing applies, pops or
  drops a stash.
- **The commits that touched this file.** `H` lists them in the diff pane —
  commit, author, age and subject — following the file through renames, so the
  history does not appear to start where the file was last moved. Enter shows
  what the chosen commit did to the file, by comparing against its parent; the
  first commit has nothing before it and says so rather than showing an empty
  diff.
- **Who last touched each line.** `B` puts a blame column beside the diff:
  commit, author and age, against the lines it can answer for. An added line
  has no author because nobody has committed it, so its column stays blank
  rather than borrowing the attribution of the line above. Off by default —
  blame is the most expensive query here — and read once when it is turned on
  rather than on every keypress.
- **Diff against the branch, not just the last commit.** `m` switches between
  comparing the worktree against `HEAD` — what you have changed since you
  committed — and against the point where the branch diverged from `main`,
  which is what the branch as a whole contains and what a reviewer will see.
  The file list changes with it: a file committed earlier on the branch and
  clean in the worktree belongs in the branch view and is absent from the other.
  Annotations survive the switch, re-anchored by content as they are across a
  refresh. The title bar names anything other than `HEAD`.
- **Git runs off the drawing thread.** Reading a repository costs around 35 ms,
  which across many of them is over a second — and it used to happen inside the
  key handler, freezing the terminal on every refresh. The projects are now read
  on a worker thread and appear one at a time as their answers arrive. A project
  not yet read shows `…` rather than claiming to be clean.
- **Syntax highlighting.** The diff is coloured by the grammar of the language
  it is written in, across sixteen of them — Rust, Python, JavaScript,
  TypeScript, Go, C, C++, Java, Ruby, shell, C#, PHP, JSON, YAML, CSS and HTML.
  Colour says what a token is and red or green still says whether it changed, so
  both can be read at once. A language without a grammar, or a file that will
  not parse, keeps the plain red and green rather than reporting anything.
- **Line annotations, handed to Claude.** Move a mark through the diff with
  `n`/`p`, press `A` to note something about the line under it as an issue,
  suggestion, question or note, and `R` to hand every note collected to the
  agent running in that repository. Notes survive a refresh — each is anchored
  by content as well as position, so it follows its line when the code above it
  shifts, and is reported as orphaned rather than reassigned when the line goes
  away. They are kept under `$XDG_STATE_HOME/herdr-strays/`, outside the
  repository.
- Each project row now says how far its branch is from the remote branch it
  tracks (`↑3`, `↓5`, `↑3↓5`), how long ago the project last moved, and whether
  a Claude agent is working in it (`●`) or waiting (`○`).
- Unmerged files are reported as conflicts (`U`) rather than as ordinary
  modifications, so a stopped merge is visible at a glance.
- Prebuilt binaries for linux and macOS, on x86_64 and arm64. `herdr plugin
  install` now downloads the release built for your platform and verifies its
  SHA-256 checksum, so a Rust toolchain is no longer required.
- A release workflow that builds every target on a tag and publishes the
  archives alongside a `SHA256SUMS` file.
- Tests keeping `Cargo.toml` and `herdr-plugin.toml` in step, and checking that
  the manifest launches the binary the install script actually writes.

### Changed

- The plugin binary now lives at `bin/herdr-strays` rather than
  `target/release/herdr-strays`, separating a downloaded release from a local
  build.
- Installing without a Rust toolchain falls back to compiling from source only
  when no release matches the platform, or the download cannot be verified.
- ratatui 0.29 → 0.30, which clears both open advisories: RUSTSEC-2024-0436
  (`paste`, unmaintained) and RUSTSEC-2026-0002 (`lru`, unsound `IterMut`).
- The minimum supported Rust version is 1.88, up from 1.82. It now only governs
  the source-build fallback, since a normal install downloads a binary built by
  CI rather than compiling on the user's machine.

## [0.1.0]

First release.

### Added

- A live list of every file that strayed from `HEAD`, grouped by project and
  directory, following the worktree through filesystem events rather than a
  timer.
- Diffs for the selected file, including untracked files rendered as
  all-additions and submodules recognised as directories rather than files.
- Scope covering the current herdr workspace, widened to every workspace with
  one key.
- Hand-off to a Claude agent running in the same repository: a prompt about the
  selected file is typed into the agent's input, never submitted.
- `$VISUAL`/`$EDITOR` hand-off, with the path passed behind a `--` separator so
  a filename never reaches a shell.
- A key reference, a show-all-tracked-files view, and auto-folding for
  oversized directories.

[Unreleased]: https://github.com/aleslanger/herdr-strays/compare/v1.0.1...HEAD
[1.0.1]: https://github.com/aleslanger/herdr-strays/releases/tag/v1.0.1
[1.0.0]: https://github.com/aleslanger/herdr-strays/releases/tag/v1.0.0
[0.1.0]: https://github.com/aleslanger/herdr-strays/releases/tag/0.0.4
