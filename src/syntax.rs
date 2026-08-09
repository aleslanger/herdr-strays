//! Colouring a diff by the grammar of the language it is written in.
//!
//! Red and green say whether a line changed. They say nothing about what the
//! line *is* — which token is a string, which is a keyword, which is the name
//! being called. That is the reading most of the time spent in a diff pane
//! actually goes to, and every editor the reader came from provides it.
//!
//! # Why the file and not the line
//!
//! A diff line is not a program. `    })` parses as nothing on its own, and a
//! line inside a string literal would be highlighted as code. So the file on
//! disk is parsed once, whole, and the diff lines are matched back onto it by
//! their `new_line` number — which [`crate::model::number_lines`] has already
//! recovered from the hunk headers.
//!
//! That has a consequence worth stating plainly: **removed lines are not
//! highlighted**. They are not in the file being parsed, so there is nothing to
//! match them against. They keep their red. Highlighting them would mean
//! reconstructing the old file and parsing it too — a second parse of a second
//! text to colour the half of the diff that is on its way out.
//!
//! # Failure is silent
//!
//! An unknown extension, an unreadable file, a parse that fails, a diff whose
//! lines are unnumbered: each falls back to the line's ordinary red or green.
//! The viewer is for reading diffs, and a diff without syntax colour is the one
//! this program showed until now. A notice about a missing grammar would
//! interrupt that to report something the reader cannot act on.

use ratatui::style::Color;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// Largest file this will parse, in bytes.
///
/// Parsing is proportional to file size and happens when the selection moves.
/// A vendored bundle or a checked-in data file would stall that keypress to
/// colour a diff nobody reads token by token. Past this, red and green.
const MAX_FILE: usize = 512 * 1024;

/// The highlight names this asks tree-sitter for, and their order.
///
/// The index into this list is what a [`HighlightEvent::HighlightStart`]
/// carries, so [`COLOURS`] is indexed by the same position — the two arrays are
/// one table split in half, and must stay the same length and order.
///
/// These are the standard tree-sitter capture names. A grammar's query file may
/// use a more specific one, such as `function.method`; tree-sitter matches on
/// the longest listed prefix, so `function` here catches it.
///
/// The reverse does not hold: a name a grammar emits that has no prefix listed
/// here is dropped entirely. That is why both `escape` and `string.escape`
/// appear — the Rust query writes `@escape` where the JavaScript one writes
/// `@string.escape`, and listing only the latter would silently lose the former.
const NAMES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "escape",
    "function",
    "function.builtin",
    "keyword",
    "label",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "string",
    "string.escape",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

/// The colour each name in [`NAMES`] renders as, in the same order.
///
/// Drawn from the sixteen ANSI colours rather than RGB, so the palette follows
/// whatever theme the terminal is already set to — the diff then matches the
/// editor beside it instead of imposing a second scheme on top of it.
///
/// `None` means "leave the line's own colour alone". Ordinary variables and
/// most punctuation get it: colouring every token leaves nothing standing out,
/// and the line's red or green still has to be readable underneath.
const COLOURS: &[Option<Color>] = &[
    Some(Color::Yellow),   // attribute
    Some(Color::DarkGray), // comment
    Some(Color::Cyan),     // constant
    Some(Color::Cyan),     // constant.builtin
    Some(Color::Yellow),   // constructor
    Some(Color::Cyan),     // escape
    Some(Color::Blue),     // function
    Some(Color::Blue),     // function.builtin
    Some(Color::Magenta),  // keyword
    Some(Color::Magenta),  // label
    Some(Color::Cyan),     // number
    None,                  // operator
    None,                  // property
    None,                  // punctuation
    None,                  // punctuation.bracket
    None,                  // punctuation.delimiter
    Some(Color::Green),    // string
    Some(Color::Cyan),     // string.escape
    Some(Color::Cyan),     // string.special
    Some(Color::Yellow),   // tag
    Some(Color::Yellow),   // type
    Some(Color::Yellow),   // type.builtin
    None,                  // variable
    Some(Color::Magenta),  // variable.builtin
    None,                  // variable.parameter
];

/// A run of bytes within one line, and the colour its grammar gives it.
///
/// Byte offsets into the line's own text, marker included — the same frame of
/// reference [`crate::intraline::Span`] uses, so the renderer can walk both
/// without converting between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coloured {
    pub start: usize,
    pub end: usize,
    pub colour: Color,
}

/// Colours for each line of a diff, indexed alongside it.
///
/// `None` where a line has no highlighting: a removed line, a hunk header, a
/// file whose language is unknown. Empty when nothing could be highlighted at
/// all, which the renderer reads as "colour whole lines" — the behaviour before
/// this existed.
pub type Highlights = Vec<Option<Vec<Coloured>>>;

/// Which grammar to use for a file, chosen by its extension.
///
/// Extension only. Reading a shebang would catch extensionless scripts, but it
/// means opening every file to decide whether to open it, and the files whose
/// diffs are read most have extensions.
///
/// The exports these return are not uniform across the grammar crates — some
/// spell the query `HIGHLIGHT_QUERY` and others `HIGHLIGHTS_QUERY`, and the
/// languages with more than one dialect name the constant after the dialect.
/// That inconsistency is upstream's; this is where it stops.
///
/// The query is owned rather than borrowed because two of them are built by
/// concatenation: the JSX-bearing dialects keep their tag rules in a separate
/// query from the language's own, and a file using both needs both.
fn grammar_for(extension: &str) -> Option<(tree_sitter::Language, std::borrow::Cow<'static, str>)> {
    // The dialects whose markup rules live in a second query. Without it a
    // `.jsx`/`.tsx` file parses but every tag comes back uncaptured, which
    // looks exactly like a plain-text file rather than like a failure.
    let jsx = tree_sitter_javascript::JSX_HIGHLIGHT_QUERY;

    let pair: (tree_sitter::Language, std::borrow::Cow<'static, str>) = match extension {
        "rs" => (
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY.into(),
        ),
        "py" | "pyi" => (
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::HIGHLIGHTS_QUERY.into(),
        ),
        "js" | "mjs" | "cjs" => (
            tree_sitter_javascript::LANGUAGE.into(),
            tree_sitter_javascript::HIGHLIGHT_QUERY.into(),
        ),
        "jsx" => (
            tree_sitter_javascript::LANGUAGE.into(),
            format!("{}\n{jsx}", tree_sitter_javascript::HIGHLIGHT_QUERY).into(),
        ),
        "ts" | "mts" | "cts" => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            tree_sitter_typescript::HIGHLIGHTS_QUERY.into(),
        ),
        "tsx" => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            format!("{}\n{jsx}", tree_sitter_typescript::HIGHLIGHTS_QUERY).into(),
        ),
        "go" => (
            tree_sitter_go::LANGUAGE.into(),
            tree_sitter_go::HIGHLIGHTS_QUERY.into(),
        ),
        "json" => (
            tree_sitter_json::LANGUAGE.into(),
            tree_sitter_json::HIGHLIGHTS_QUERY.into(),
        ),
        "c" | "h" => (
            tree_sitter_c::LANGUAGE.into(),
            tree_sitter_c::HIGHLIGHT_QUERY.into(),
        ),
        // C++'s own query holds only what C++ adds to C — templates, classes,
        // namespaces. Everything a C++ file shares with C, keywords included,
        // is in C's query, so both are needed or a plain function body comes
        // back entirely uncaptured.
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => (
            tree_sitter_cpp::LANGUAGE.into(),
            format!(
                "{}\n{}",
                tree_sitter_c::HIGHLIGHT_QUERY,
                tree_sitter_cpp::HIGHLIGHT_QUERY
            )
            .into(),
        ),
        "java" => (
            tree_sitter_java::LANGUAGE.into(),
            tree_sitter_java::HIGHLIGHTS_QUERY.into(),
        ),
        "rb" => (
            tree_sitter_ruby::LANGUAGE.into(),
            tree_sitter_ruby::HIGHLIGHTS_QUERY.into(),
        ),
        "sh" | "bash" | "zsh" => (
            tree_sitter_bash::LANGUAGE.into(),
            tree_sitter_bash::HIGHLIGHT_QUERY.into(),
        ),
        "css" | "scss" => (
            tree_sitter_css::LANGUAGE.into(),
            tree_sitter_css::HIGHLIGHTS_QUERY.into(),
        ),
        "html" | "htm" => (
            tree_sitter_html::LANGUAGE.into(),
            tree_sitter_html::HIGHLIGHTS_QUERY.into(),
        ),
        "cs" => (
            tree_sitter_c_sharp::LANGUAGE.into(),
            tree_sitter_c_sharp::HIGHLIGHTS_QUERY.into(),
        ),
        "php" => (
            tree_sitter_php::LANGUAGE_PHP.into(),
            tree_sitter_php::HIGHLIGHTS_QUERY.into(),
        ),
        "yml" | "yaml" => (
            tree_sitter_yaml::LANGUAGE.into(),
            tree_sitter_yaml::HIGHLIGHTS_QUERY.into(),
        ),
        _ => return None,
    };
    Some(pair)
}

/// Whether a file is one this can colour at all.
///
/// Lets the caller skip reading a file it could not highlight anyway.
pub fn is_supported(path: &std::path::Path) -> bool {
    extension_of(path).is_some_and(|ext| grammar_for(&ext).is_some())
}

/// A file's extension, lowercased.
///
/// Lowercased because `.RS` and `.rs` are the same language, and a file named
/// in capitals should not silently lose its colour.
fn extension_of(path: &std::path::Path) -> Option<String> {
    Some(path.extension()?.to_str()?.to_lowercase())
}

/// Colour every line of `source`, as byte ranges within each line.
///
/// The outer index is the line number minus one, so line `n` of the file is at
/// `[n - 1]`. Returns `None` if the language is unknown or the parse fails —
/// both of which the caller renders as an uncoloured diff.
fn colour_file(path: &std::path::Path, source: &str) -> Option<Vec<Vec<Coloured>>> {
    let (language, query) = grammar_for(&extension_of(path)?)?;

    // The name given to the configuration only appears in tree-sitter's own
    // error messages, which are not surfaced; the extension is as good as
    // anything and makes a stray debug print legible.
    let mut config = HighlightConfiguration::new(language, "diff", &query, "", "").ok()?;
    config.configure(NAMES);

    // Where each line starts, so a byte offset into the file can be turned into
    // a (line, column) pair without scanning from the top each time.
    let starts = line_starts(source);
    let mut lines: Vec<Vec<Coloured>> = vec![Vec::new(); starts.len()];

    let mut highlighter = Highlighter::new();
    // The event iterator borrows `config`, so it is consumed inside this scope
    // rather than being returned from it.
    let events = highlighter
        .highlight(&config, source.as_bytes(), None, |_| None)
        .ok()?;

    // Captures nest — a string inside a call inside a function. The innermost
    // is the one that should win, so this is a stack and the top is used.
    let mut open: Vec<usize> = Vec::new();

    for event in events {
        match event.ok()? {
            HighlightEvent::HighlightStart(highlight) => open.push(highlight.0),
            HighlightEvent::HighlightEnd => {
                open.pop();
            }
            HighlightEvent::Source { start, end } => {
                let Some(colour) = open
                    .last()
                    .and_then(|at| COLOURS.get(*at).copied().flatten())
                else {
                    continue;
                };
                spread(&mut lines, &starts, source, start, end, colour);
            }
        }
    }

    Some(lines)
}

/// Byte offset at which each line of `source` begins.
fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(
        source
            .char_indices()
            .filter(|(_, c)| *c == '\n')
            .map(|(at, _)| at + 1),
    );
    // A file ending in a newline yields a final empty line that holds nothing;
    // keeping it costs one empty vector and keeps the indices honest.
    starts
}

/// Record `start..end` of the file as coloured, splitting it across the lines
/// it spans.
///
/// A capture can cover more than one line — a block comment, a multi-line
/// string — and each line has to carry its own piece, because the renderer only
/// ever sees one line at a time.
fn spread(
    lines: &mut [Vec<Coloured>],
    starts: &[usize],
    source: &str,
    start: usize,
    end: usize,
    colour: Color,
) {
    // The line containing `start`: the last one beginning at or before it.
    let first = match starts.binary_search(&start) {
        Ok(at) => at,
        Err(at) => at.saturating_sub(1),
    };

    // Walked as pairs so each line comes with the offsets it owns; the two
    // slices are built together and are the same length.
    let rest = starts.iter().zip(lines.iter_mut()).skip(first);

    for (index, (line_start, line)) in rest.enumerate() {
        if *line_start >= end && *line_start > start {
            break;
        }
        // Where this line ends: just before the next one starts, or at the end
        // of the file. The trailing newline is not part of the line's text.
        let line_end = starts
            .get(first + index + 1)
            .map(|next| next.saturating_sub(1))
            .unwrap_or(source.len());

        let from = start.max(*line_start);
        let to = end.min(line_end);
        if from >= to {
            continue;
        }

        line.push(Coloured {
            start: from - line_start,
            end: to - line_start,
            colour,
        });
    }
}

/// Colours for each line of `diff`, matched back onto the file on disk.
///
/// Reads the file once and parses it once, then walks the diff assigning each
/// line the colours of the file line it came from. Lines that are not in the
/// file — removals, hunk headers, metadata — get `None`.
///
/// Returns an empty vector rather than an error for every failure: an unknown
/// language, a file that will not read, a parse that fails. See the module
/// documentation for why that is silent.
pub fn compute(
    root: &std::path::Path,
    relative: &std::path::Path,
    diff: &[crate::model::DiffLine],
) -> Highlights {
    if !is_supported(relative) {
        return Vec::new();
    }

    let full = root.join(relative);
    let Ok(source) = std::fs::read_to_string(&full) else {
        return Vec::new();
    };
    if source.len() > MAX_FILE {
        return Vec::new();
    }

    let Some(by_line) = colour_file(relative, &source) else {
        return Vec::new();
    };

    diff.iter()
        .map(|line| {
            // Only lines that exist in the file on disk can be matched to it.
            // `number_lines` leaves `new_line` unset for everything else.
            let number = line.new_line?;
            let colours = by_line.get(usize::try_from(number).ok()?.checked_sub(1)?)?;
            if colours.is_empty() {
                return None;
            }

            // The diff line carries a leading marker that the file line does
            // not, so every offset shifts by one. A line whose text does not
            // match the file — the worktree moved on since git was asked —
            // is dropped rather than coloured at the wrong offsets.
            let (body, offset) = split_marker(&line.text);
            let file_line = source_line(&source, number)?;
            if body != file_line {
                return None;
            }

            Some(
                colours
                    .iter()
                    .map(|c| Coloured {
                        start: c.start + offset,
                        end: c.end + offset,
                        colour: c.colour,
                    })
                    .collect(),
            )
        })
        .collect()
}

/// The text of line `number` (1-based) of `source`, without its newline.
fn source_line(source: &str, number: u32) -> Option<&str> {
    source
        .lines()
        .nth(usize::try_from(number).ok()?.checked_sub(1)?)
}

/// A diff line's text without its leading `+`/`-`/space, and how many bytes
/// were removed.
///
/// Mirrors `intraline::strip_marker`. Kept separate rather than shared because
/// the two modules answer different questions with it and neither should have
/// to change when the other does.
fn split_marker(text: &str) -> (&str, usize) {
    match text.as_bytes().first() {
        Some(b'+') | Some(b'-') | Some(b' ') => (&text[1..], 1),
        _ => (text, 0),
    }
}

/// Look up the colour a byte offset falls in, if any.
///
/// Linear over the line's spans. A line has a handful of them, and this is
/// called once per rendered piece rather than once per byte.
pub fn colour_at(spans: &[Coloured], at: usize) -> Option<Color> {
    spans
        .iter()
        .find(|span| at >= span.start && at < span.end)
        .map(|span| span.colour)
}

/// Every byte offset at which the colour changes within a line.
///
/// The renderer splits a line at the union of these and the word-diff
/// boundaries, so that each rendered piece has one colour and one emphasis.
pub fn boundaries(spans: &[Coloured]) -> Vec<usize> {
    let mut edges: Vec<usize> = spans.iter().flat_map(|s| [s.start, s.end]).collect();
    edges.sort_unstable();
    edges.dedup();
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{number_lines, DiffLine};

    /// Colour a source string as if it were a file with the given name.
    fn coloured(name: &str, source: &str) -> Vec<Vec<Coloured>> {
        colour_file(std::path::Path::new(name), source).expect("a known language parses")
    }

    /// The colour covering the first occurrence of `needle` on `line`.
    fn colour_of(
        lines: &[Vec<Coloured>],
        source: &str,
        line: usize,
        needle: &str,
    ) -> Option<Color> {
        let text = source.lines().nth(line)?;
        let at = text.find(needle)?;
        colour_at(lines.get(line)?, at)
    }

    #[test]
    fn a_keyword_is_coloured_differently_from_the_name_beside_it() {
        // The point of the whole module: `fn` and `main` are not the same kind
        // of thing and should not read as the same kind of thing.
        let source = "fn main() {}\n";
        let lines = coloured("a.rs", source);

        let keyword = colour_of(&lines, source, 0, "fn");
        assert_eq!(keyword, Some(Color::Magenta), "`fn` is a keyword");
        assert_ne!(
            colour_of(&lines, source, 0, "main"),
            keyword,
            "the function name is not the keyword"
        );
    }

    #[test]
    fn a_string_literal_is_coloured_as_a_string() {
        let source = "let x = \"hello\";\n";
        let lines = coloured("a.rs", source);
        assert_eq!(
            colour_of(&lines, source, 0, "\"hello\""),
            Some(Color::Green)
        );
    }

    #[test]
    fn a_comment_recedes_rather_than_competing_with_the_code() {
        let source = "// note\nfn f() {}\n";
        let lines = coloured("a.rs", source);
        assert_eq!(colour_of(&lines, source, 0, "//"), Some(Color::DarkGray));
    }

    #[test]
    fn colours_are_offsets_within_their_own_line_not_the_file() {
        // The renderer slices one line at a time and has no idea where in the
        // file that line sits, so every offset must be line-relative.
        let source = "fn a() {}\nfn b() {}\n";
        let lines = coloured("a.rs", source);

        let second = &lines[1];
        assert!(
            second.iter().all(|c| c.end <= "fn b() {}".len()),
            "line two's spans must fit line two: {second:?}"
        );
        assert_eq!(
            colour_at(second, 0),
            Some(Color::Magenta),
            "`fn` starts at 0 on its own line"
        );
    }

    #[test]
    fn a_capture_spanning_several_lines_gives_each_line_its_own_piece() {
        // A block comment is one capture over three lines. The renderer only
        // ever sees one line, so each needs its own span.
        let source = "/* one\n   two\n   three */\nfn f() {}\n";
        let lines = coloured("a.rs", source);

        for (at, line) in lines.iter().enumerate().take(3) {
            assert_eq!(
                colour_at(line, 0),
                Some(Color::DarkGray),
                "line {at} is inside the block comment"
            );
        }
    }

    #[test]
    fn a_multiline_span_does_not_reach_past_the_end_of_its_line() {
        let source = "/* one\n   two */\n";
        let lines = coloured("a.rs", source);
        assert!(
            lines[0].iter().all(|c| c.end <= "/* one".len()),
            "the newline is not part of the line"
        );
    }

    #[test]
    fn the_innermost_capture_wins_where_they_nest() {
        // An escape inside a string is both; the escape is the more specific
        // claim and is the one worth seeing.
        let source = "let x = \"a\\nb\";\n";
        let lines = coloured("a.rs", source);
        let text = source.lines().next().unwrap();
        let at = text.find("\\n").unwrap();
        assert_eq!(
            colour_at(&lines[0], at),
            Some(Color::Cyan),
            "the escape is not merely string-green"
        );
    }

    #[test]
    fn an_unknown_extension_yields_nothing_rather_than_guessing() {
        assert!(colour_file(std::path::Path::new("a.wat"), "?!").is_none());
        assert!(!is_supported(std::path::Path::new("notes.wat")));
    }

    #[test]
    fn a_file_with_no_extension_at_all_is_not_highlighted() {
        assert!(!is_supported(std::path::Path::new("Makefile")));
    }

    #[test]
    fn the_extension_is_matched_regardless_of_its_case() {
        // A file named in capitals is the same language as one that is not.
        assert!(is_supported(std::path::Path::new("A.RS")));
        assert!(is_supported(std::path::Path::new("Main.Java")));
    }

    #[test]
    fn every_language_offered_actually_parses() {
        // Each grammar crate spells its exports slightly differently, so a
        // wrong pairing compiles and then quietly highlights nothing. This is
        // what catches that.
        let cases: &[(&str, &str)] = &[
            ("a.rs", "fn f() {}"),
            ("a.py", "def f():\n    pass"),
            ("a.js", "function f() {}"),
            ("a.jsx", "const f = () => <div />;"),
            ("a.ts", "function f(): void {}"),
            ("a.tsx", "const f = () => <div />;"),
            ("a.go", "package main\nfunc f() {}"),
            ("a.json", "{\"a\": 1}"),
            ("a.c", "int f(void) { return 0; }"),
            ("a.cpp", "int f() { return 0; }"),
            ("a.java", "class A { void f() {} }"),
            ("a.rb", "def f\nend"),
            ("a.sh", "f() { echo hi; }"),
            ("a.css", "a { color: red; }"),
            ("a.html", "<p>hi</p>"),
            ("a.cs", "class A { void F() {} }"),
            ("a.php", "<?php function f() {} ?>"),
            ("a.yml", "a: 1"),
        ];

        for (name, source) in cases {
            let lines = colour_file(std::path::Path::new(name), source)
                .unwrap_or_else(|| panic!("{name} has a grammar"));
            assert!(
                lines.iter().any(|line| !line.is_empty()),
                "{name} parsed but coloured nothing — check its query constant"
            );
        }
    }

    #[test]
    fn a_diff_line_is_coloured_at_the_offset_its_marker_leaves() {
        // The file line has no `+`; the diff line does. Every span shifts by
        // one, and getting this wrong colours each token one byte early.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), "fn main() {}\n").expect("write");

        let diff = number_lines(
            ["@@ -1,1 +1,1 @@", "+fn main() {}"]
                .iter()
                .map(|l| DiffLine::parse(l))
                .collect(),
        );

        let highlights = compute(dir.path(), std::path::Path::new("a.rs"), &diff);
        let line = highlights[1].as_ref().expect("the added line is coloured");

        assert_eq!(
            colour_at(line, 1),
            Some(Color::Magenta),
            "`fn` sits at 1, after the `+`"
        );
        assert_eq!(
            colour_at(line, 0),
            None,
            "the marker itself is not coloured"
        );
    }

    #[test]
    fn a_removed_line_carries_no_colours() {
        // It is not in the file being parsed. See the module documentation.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), "fn main() {}\n").expect("write");

        let diff = number_lines(
            ["@@ -1,1 +1,1 @@", "-fn gone() {}", "+fn main() {}"]
                .iter()
                .map(|l| DiffLine::parse(l))
                .collect(),
        );

        let highlights = compute(dir.path(), std::path::Path::new("a.rs"), &diff);
        assert!(highlights[1].is_none(), "the removed line keeps its red");
        assert!(highlights[2].is_some(), "the added line is coloured");
    }

    #[test]
    fn hunk_headers_and_metadata_are_left_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), "fn main() {}\n").expect("write");

        let diff = number_lines(
            [
                "diff --git a/a.rs b/a.rs",
                "@@ -1,1 +1,1 @@",
                "+fn main() {}",
            ]
            .iter()
            .map(|l| DiffLine::parse(l))
            .collect(),
        );

        let highlights = compute(dir.path(), std::path::Path::new("a.rs"), &diff);
        assert!(highlights[0].is_none(), "a file header is not code");
        assert!(highlights[1].is_none(), "a hunk header is not code");
    }

    #[test]
    fn a_line_that_no_longer_matches_the_file_is_left_uncoloured() {
        // The worktree can move between git being asked and the file being
        // read. Colouring by a stale line number would put the colours on the
        // wrong words, which is worse than no colour.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), "fn something_else() {}\n").expect("write");

        let diff = number_lines(
            ["@@ -1,1 +1,1 @@", "+fn main() {}"]
                .iter()
                .map(|l| DiffLine::parse(l))
                .collect(),
        );

        let highlights = compute(dir.path(), std::path::Path::new("a.rs"), &diff);
        assert!(highlights[1].is_none(), "the text disagrees, so no colour");
    }

    #[test]
    fn a_missing_file_yields_no_highlights_rather_than_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let diff = number_lines(vec![DiffLine::parse("+fn main() {}")]);

        let highlights = compute(dir.path(), std::path::Path::new("gone.rs"), &diff);
        assert!(
            highlights.is_empty(),
            "silent, per the module documentation"
        );
    }

    #[test]
    fn an_unsupported_file_is_not_even_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.wat"), "(module)\n").expect("write");

        let diff = number_lines(vec![DiffLine::parse("+(module)")]);
        let highlights = compute(dir.path(), std::path::Path::new("a.wat"), &diff);
        assert!(highlights.is_empty());
    }

    #[test]
    fn the_highlights_are_indexed_against_the_diff_they_describe() {
        // They are looked up by line index at render time, so a shorter or
        // longer list would put colours on the wrong lines.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), "fn main() {}\nlet x = 1;\n").expect("write");

        let diff = number_lines(
            ["@@ -1,2 +1,2 @@", " fn main() {}", "+let x = 1;"]
                .iter()
                .map(|l| DiffLine::parse(l))
                .collect(),
        );

        let highlights = compute(dir.path(), std::path::Path::new("a.rs"), &diff);
        assert_eq!(highlights.len(), diff.len(), "one entry per diff line");
    }

    #[test]
    fn a_context_line_is_coloured_like_the_code_it_is() {
        // Context is the bulk of a diff and the part read for orientation;
        // leaving it grey would waste most of the pane.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), "fn main() {}\n").expect("write");

        let diff = number_lines(
            ["@@ -1,1 +1,1 @@", " fn main() {}"]
                .iter()
                .map(|l| DiffLine::parse(l))
                .collect(),
        );

        let highlights = compute(dir.path(), std::path::Path::new("a.rs"), &diff);
        assert!(highlights[1].is_some(), "context is code too");
    }

    #[test]
    fn an_enormous_file_is_left_uncoloured_rather_than_stalling_the_draw() {
        let dir = tempfile::tempdir().expect("tempdir");
        let huge = "fn f() {}\n".repeat(MAX_FILE / 5);
        std::fs::write(dir.path().join("a.rs"), &huge).expect("write");

        let diff = number_lines(vec![DiffLine::parse("+fn f() {}")]);
        let highlights = compute(dir.path(), std::path::Path::new("a.rs"), &diff);
        assert!(highlights.is_empty(), "past the size cap");
    }

    #[test]
    fn the_names_and_colours_tables_are_the_same_length() {
        // They are one table split in half and indexed by the same number; if
        // they drift, every colour past the drift is wrong.
        assert_eq!(NAMES.len(), COLOURS.len());
    }

    #[test]
    fn boundaries_are_sorted_and_free_of_duplicates() {
        let spans = &[
            Coloured {
                start: 5,
                end: 9,
                colour: Color::Red,
            },
            Coloured {
                start: 0,
                end: 5,
                colour: Color::Blue,
            },
        ];
        assert_eq!(boundaries(spans), vec![0, 5, 9], "0,5,5,9 deduped to 0,5,9");
    }

    #[test]
    fn a_byte_outside_every_span_has_no_colour() {
        let spans = &[Coloured {
            start: 0,
            end: 2,
            colour: Color::Red,
        }];
        assert_eq!(colour_at(spans, 2), None, "end is exclusive");
        assert_eq!(colour_at(spans, 99), None);
    }
}
