//! Which words inside a changed line actually changed.
//!
//! A diff that marks a whole line red and a whole line green is honest but
//! coarse: when a rename touches one identifier in a ninety-column line, the
//! reader has to compare the two lines by eye to find it. This narrows the
//! claim from "this line changed" to "these words changed".
//!
//! Computed here rather than asked of git. `git diff --word-diff` returns a
//! different document shape — one where an addition and a removal share a
//! line — which would have to be undone before the hunk walk in
//! [`crate::model::number_lines`] could number it, and would take the anchors
//! that annotations and search depend on with it. Pairing lines locally keeps
//! one git call, one diff shape, and every feature already built on it.

/// Longest pair of lines compared word by word.
///
/// A minified bundle arrives as one enormous line; the quadratic middle of the
/// algorithm on two of those would stall the draw. Past this the line keeps its
/// ordinary whole-line colour, which is what it had before this existed.
const MAX_LINE: usize = 2000;

/// A run of bytes within one line, and whether it is part of what changed.
///
/// Byte offsets rather than character indices: the caller slices the original
/// `String` with them, and every boundary here comes from a split that already
/// respects character boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub changed: bool,
}

/// Split a line into words, keeping the separators.
///
/// Every byte of the input lands in exactly one token, so the tokens can be
/// concatenated back into the original line — the offsets below depend on it.
/// Whitespace is its own token rather than being attached to a word, so that
/// re-indenting a line does not report the word beside it as changed.
fn tokenize(line: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut rest = line;

    while !rest.is_empty() {
        let first = rest.chars().next().expect("non-empty");
        let class = classify(first);
        let end = rest
            .char_indices()
            .find(|(_, c)| classify(*c) != class)
            .map(|(at, _)| at)
            .unwrap_or(rest.len());

        let (token, tail) = rest.split_at(end);
        tokens.push(token);
        rest = tail;
    }

    tokens
}

/// Which run a character belongs to: a word, a stretch of whitespace, or a
/// single punctuation mark.
///
/// Punctuation gets a class per character, so `foo(bar)` splits at the bracket
/// rather than being one opaque token — the parentheses are usually the part
/// that stayed.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Class {
    Word,
    Space,
    /// Carries the character itself so two different marks never merge.
    Punct(char),
}

fn classify(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Punct(c)
    }
}

/// Spans of `new` that are not in `old`, and of `old` that are not in `new`.
///
/// Returns `(old_spans, new_spans)`. Both cover their whole line, changed and
/// unchanged runs alike, so a caller can render one line by walking one list.
///
/// The comparison strips the leading `-`/`+` marker, then puts it back as an
/// unchanged span: the marker is not part of the text and would otherwise
/// report every line as starting with a change.
pub fn compare(old: &str, new: &str) -> (Vec<Span>, Vec<Span>) {
    let (old_body, old_offset) = strip_marker(old);
    let (new_body, new_offset) = strip_marker(new);

    if old_body.len() > MAX_LINE || new_body.len() > MAX_LINE {
        return (whole(old), whole(new));
    }

    let old_tokens = tokenize(old_body);
    let new_tokens = tokenize(new_body);

    let (old_kept, new_kept) = common(&old_tokens, &new_tokens);

    let old_spans = spans_from(&old_tokens, &old_kept, old_offset);
    let new_spans = spans_from(&new_tokens, &new_kept, new_offset);

    // Two lines with nothing in common are not a rewrite of each other; saying
    // so word by word is noise, and the whole-line colour already said it.
    if changed_ratio(&old_spans, old.len()) > REWRITE
        || changed_ratio(&new_spans, new.len()) > REWRITE
    {
        return (whole(old), whole(new));
    }

    (old_spans, new_spans)
}

/// Above this fraction of a line being changed, the word-level answer stops
/// being more informative than the line-level one.
const REWRITE: f32 = 0.75;

fn changed_ratio(spans: &[Span], total: usize) -> f32 {
    if total == 0 {
        return 0.0;
    }
    let changed: usize = spans
        .iter()
        .filter(|s| s.changed)
        .map(|s| s.end - s.start)
        .sum();
    changed as f32 / total as f32
}

/// One span covering the whole line, marked unchanged.
///
/// The fallback shape: the line keeps the single colour it had before any of
/// this, since the caller renders an all-unchanged line with its line style.
fn whole(line: &str) -> Vec<Span> {
    vec![Span {
        start: 0,
        end: line.len(),
        changed: false,
    }]
}

/// Split a diff line into its body and the width of the marker before it.
fn strip_marker(line: &str) -> (&str, usize) {
    match line.chars().next() {
        Some('+') | Some('-') | Some(' ') => (&line[1..], 1),
        _ => (line, 0),
    }
}

/// Which tokens of each side survive into the other, by longest common
/// subsequence.
///
/// Returns two masks, one per side, `true` where a token is common. LCS rather
/// than a set intersection: order carries meaning in code, and a token that
/// merely appears on both sides in a different place did move.
fn common(old: &[&str], new: &[&str]) -> (Vec<bool>, Vec<bool>) {
    let (rows, cols) = (old.len(), new.len());

    // table[i][j] — length of the LCS of old[i..] and new[j..]. Built from the
    // end so the walk below can move forwards, which is the order the spans
    // are emitted in.
    let mut table = vec![vec![0usize; cols + 1]; rows + 1];
    for i in (0..rows).rev() {
        for j in (0..cols).rev() {
            table[i][j] = if old[i] == new[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }

    let mut old_kept = vec![false; rows];
    let mut new_kept = vec![false; cols];
    let (mut i, mut j) = (0, 0);

    while i < rows && j < cols {
        if old[i] == new[j] {
            old_kept[i] = true;
            new_kept[j] = true;
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }

    (old_kept, new_kept)
}

/// Turn a token list and its keep-mask into byte spans over the original line.
///
/// Adjacent tokens of the same kind are merged, so a changed identifier and the
/// changed bracket beside it become one highlighted run rather than two.
fn spans_from(tokens: &[&str], kept: &[bool], offset: usize) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();

    // The marker that was stripped is unchanged by definition: it is diff
    // punctuation, not part of the line's text.
    if offset > 0 {
        spans.push(Span {
            start: 0,
            end: offset,
            changed: false,
        });
    }

    let mut at = offset;
    for (token, keep) in tokens.iter().zip(kept) {
        let end = at + token.len();
        let changed = !keep;

        match spans.last_mut() {
            Some(last) if last.changed == changed && last.end == at => last.end = end,
            _ => spans.push(Span {
                start: at,
                end,
                changed,
            }),
        }
        at = end;
    }

    spans
}

/// Word-level spans for every line of a diff, indexed the same way.
///
/// `None` where a line has no counterpart to compare against — context, hunk
/// headers, and a removal or addition that stands alone. The renderer takes
/// that as "colour the whole line", which is what it did before.
pub type Intraline = Vec<Option<Vec<Span>>>;

/// Pair up adjacent removals and additions and compare each pair.
///
/// Git emits a modified line as a run of removals followed by a run of
/// additions. Pairing them by position within those runs is what makes the
/// common case — one line replaced by one line — come out right, and is
/// deliberately not cleverer than that: a run of five removals followed by two
/// additions has no correspondence worth guessing at, so only the first two
/// pair up and the rest keep their line colour.
pub fn compute(lines: &[crate::model::DiffLine]) -> Intraline {
    use crate::model::DiffLineKind;

    let mut spans: Intraline = vec![None; lines.len()];
    let mut at = 0;

    while at < lines.len() {
        if lines[at].kind != DiffLineKind::Removed {
            at += 1;
            continue;
        }

        // The removals, then the additions immediately after them.
        let removed_end = run_of(lines, at, DiffLineKind::Removed);
        let added_end = run_of(lines, removed_end, DiffLineKind::Added);

        for (old, new) in (at..removed_end).zip(removed_end..added_end) {
            let (old_spans, new_spans) = compare(&lines[old].text, &lines[new].text);
            spans[old] = Some(old_spans);
            spans[new] = Some(new_spans);
        }

        // Continue past both runs: the additions have been considered, and
        // restarting inside them would pair them with the next removal.
        at = added_end.max(at + 1);
    }

    spans
}

/// Index just past the run of `kind` starting at `from`.
fn run_of(
    lines: &[crate::model::DiffLine],
    from: usize,
    kind: crate::model::DiffLineKind,
) -> usize {
    let mut at = from;
    while at < lines.len() && lines[at].kind == kind {
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DiffLine;

    /// Parse raw diff lines the way the diff pane receives them.
    fn parsed(raw: &[&str]) -> Vec<DiffLine> {
        raw.iter().map(|l| DiffLine::parse(l)).collect()
    }

    /// The changed substrings of a line, in order — what the reader sees
    /// highlighted.
    fn changed<'a>(line: &'a str, spans: &[Span]) -> Vec<&'a str> {
        spans
            .iter()
            .filter(|s| s.changed)
            .map(|s| &line[s.start..s.end])
            .collect()
    }

    #[test]
    fn only_the_word_that_changed_is_marked() {
        let old = "-let widget = compute(a, b);";
        let new = "+let gadget = compute(a, b);";
        let (old_spans, new_spans) = compare(old, new);

        assert_eq!(changed(old, &old_spans), vec!["widget"]);
        assert_eq!(changed(new, &new_spans), vec!["gadget"]);
    }

    #[test]
    fn the_diff_marker_is_never_part_of_the_change() {
        // Otherwise every line would open with a highlighted `+` or `-`. The
        // marker merges into the unchanged run beside it, which is the point:
        // what must never happen is it landing inside a changed span.
        let new = "+alpha beta";
        let (_, new_spans) = compare("-alpha gamma", new);

        assert!(
            !new_spans[0].changed,
            "the line does not open with a change"
        );
        assert!(
            new_spans.iter().any(|s| s.changed),
            "something did change on this line"
        );
        assert_eq!(changed(new, &new_spans), vec!["beta"]);
    }

    #[test]
    fn a_change_at_the_very_start_still_leaves_the_marker_out() {
        // The word next to the marker is what changed, so the two are adjacent
        // and the merge in `spans_from` must not pull the marker in with it.
        let new = "+gadget = 1;";
        let (_, new_spans) = compare("-widget = 1;", new);

        assert_eq!(changed(new, &new_spans), vec!["gadget"]);
        assert_eq!(new_spans[0].end, 1, "the marker stands alone here");
        assert!(!new_spans[0].changed);
    }

    #[test]
    fn spans_tile_the_whole_line_without_gaps() {
        // The renderer walks them in order to rebuild the line; a gap would
        // silently drop text from the pane.
        let new = "+fn alpha(x: u32) -> u32 { x + 1 }";
        let (_, spans) = compare("-fn alpha(x: u32) -> u32 { x }", new);

        let mut at = 0;
        for span in &spans {
            assert_eq!(span.start, at, "spans must be contiguous");
            at = span.end;
        }
        assert_eq!(at, new.len(), "spans must reach the end");
    }

    #[test]
    fn an_added_argument_marks_only_itself() {
        let old = "-call(first)";
        let new = "+call(first, second)";
        let (_, new_spans) = compare(old, new);
        assert_eq!(changed(new, &new_spans), vec![", second"]);
    }

    #[test]
    fn a_removed_argument_marks_only_itself() {
        let old = "-call(first, second)";
        let new = "+call(first)";
        let (old_spans, _) = compare(old, new);
        assert_eq!(changed(old, &old_spans), vec![", second"]);
    }

    #[test]
    fn re_indenting_does_not_mark_the_code_beside_it() {
        // Whitespace is its own token, so the words either side stay put.
        let old = "-  value = 1;";
        let new = "+    value = 1;";
        let (_, new_spans) = compare(old, new);
        assert_eq!(changed(new, &new_spans), vec!["    "]);
    }

    #[test]
    fn punctuation_splits_so_brackets_are_not_swallowed() {
        let old = "-foo(bar)";
        let new = "+foo(baz)";
        let (_, new_spans) = compare(old, new);
        assert_eq!(
            changed(new, &new_spans),
            vec!["baz"],
            "the brackets are common to both"
        );
    }

    #[test]
    fn a_wholesale_rewrite_falls_back_to_the_line_colour() {
        // Marking nearly everything adds nothing the red and green did not
        // already say, and reads as visual noise.
        let old = "-alpha beta gamma delta";
        let new = "+one two three four";
        let (old_spans, new_spans) = compare(old, new);

        assert_eq!(old_spans.len(), 1);
        assert!(!old_spans[0].changed, "left whole");
        assert_eq!(new_spans.len(), 1);
        assert!(!new_spans[0].changed);
    }

    #[test]
    fn an_enormous_line_is_left_alone_rather_than_stalling_the_draw() {
        // A minified bundle is one line; the quadratic middle would be felt.
        let old = format!("-{}", "a ".repeat(MAX_LINE));
        let new = format!("+{}", "b ".repeat(MAX_LINE));
        let (old_spans, _) = compare(&old, &new);
        assert_eq!(old_spans.len(), 1, "no word-level work was attempted");
    }

    #[test]
    fn identical_lines_report_nothing_changed() {
        let (old_spans, new_spans) = compare("-same text", "+same text");
        assert!(changed("-same text", &old_spans).is_empty());
        assert!(changed("+same text", &new_spans).is_empty());
    }

    #[test]
    fn multibyte_text_slices_on_character_boundaries() {
        // The spans are byte offsets used to slice the line; landing inside a
        // character would panic at render time.
        let old = "-let název = 1;";
        let new = "+let hodnota = 1;";
        let (old_spans, new_spans) = compare(old, new);

        // Slicing is the assertion: a bad boundary panics here.
        assert_eq!(changed(old, &old_spans), vec!["název"]);
        assert_eq!(changed(new, &new_spans), vec!["hodnota"]);
    }

    #[test]
    fn an_empty_line_produces_one_span_covering_nothing() {
        let (_, spans) = compare("-", "+");
        let total: usize = spans.iter().map(|s| s.end - s.start).sum();
        assert_eq!(total, 1, "just the marker");
    }

    #[test]
    fn a_removal_and_the_addition_after_it_are_compared() {
        let lines = parsed(&["@@ -1,1 +1,1 @@", "-let a = 1;", "+let b = 1;"]);
        let spans = compute(&lines);

        assert!(spans[0].is_none(), "a hunk header has nothing to pair with");
        let removed = spans[1].as_ref().expect("the removal was compared");
        assert_eq!(changed(&lines[1].text, removed), vec!["a"]);
        let added = spans[2].as_ref().expect("the addition was compared");
        assert_eq!(changed(&lines[2].text, added), vec!["b"]);
    }

    #[test]
    fn runs_pair_up_line_by_line_in_order() {
        // Git emits every removal, then every addition; the first removal
        // belongs with the first addition, not with the one next to it.
        let lines = parsed(&["-one a", "-two b", "+one x", "+two y"]);
        let spans = compute(&lines);

        assert_eq!(
            changed(&lines[0].text, spans[0].as_ref().unwrap()),
            vec!["a"]
        );
        assert_eq!(
            changed(&lines[1].text, spans[1].as_ref().unwrap()),
            vec!["b"]
        );
        assert_eq!(
            changed(&lines[2].text, spans[2].as_ref().unwrap()),
            vec!["x"]
        );
        assert_eq!(
            changed(&lines[3].text, spans[3].as_ref().unwrap()),
            vec!["y"]
        );
    }

    #[test]
    fn an_unpaired_line_keeps_its_whole_line_colour() {
        // Three removals and one addition: only the first pair corresponds to
        // anything, and guessing at the rest would highlight noise.
        let lines = parsed(&["-a", "-b", "-c", "+a!"]);
        let spans = compute(&lines);

        assert!(spans[0].is_some(), "the first removal pairs");
        assert!(spans[1].is_none(), "the second has no counterpart");
        assert!(spans[2].is_none());
    }

    #[test]
    fn a_pure_addition_is_left_whole() {
        // Nothing was replaced, so there is no "which words" question to ask.
        let lines = parsed(&["@@ -1,0 +1,2 @@", "+brand new", "+also new"]);
        let spans = compute(&lines);
        assert!(spans.iter().all(Option::is_none));
    }

    #[test]
    fn a_pure_removal_is_left_whole() {
        let lines = parsed(&["@@ -1,2 +1,0 @@", "-gone", "-also gone"]);
        let spans = compute(&lines);
        assert!(spans.iter().all(Option::is_none));
    }

    #[test]
    fn context_between_two_changes_separates_them() {
        // The addition below the context belongs to the second change, and
        // must not be paired with the removal above it.
        let lines = parsed(&["-first", " context", "+second"]);
        let spans = compute(&lines);
        assert!(spans[0].is_none(), "no addition follows this removal");
        assert!(spans[2].is_none(), "no removal precedes this addition");
    }

    #[test]
    fn the_result_is_indexed_the_same_way_as_the_diff() {
        // The renderer looks spans up by line index; a length mismatch would
        // silently shift every highlight.
        let lines = parsed(&["@@ -1,3 +1,3 @@", " a", "-b", "+c", " d"]);
        assert_eq!(compute(&lines).len(), lines.len());
    }

    #[test]
    fn a_word_moved_within_the_line_is_reported_as_a_change() {
        // Order carries meaning in code: `a - b` is not `b - a`, and a set
        // comparison would call this line unchanged.
        let old = "-x = a - b;";
        let new = "+x = b - a;";
        let (_, new_spans) = compare(old, new);
        assert!(
            !changed(new, &new_spans).is_empty(),
            "the reordering is visible"
        );
    }
}
