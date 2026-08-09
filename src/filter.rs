//! Narrowing the list to the files a query names.
//!
//! In the show-all view a repository contributes every tracked file, and the
//! only way through is one row at a time. A query typed at the `/` line cuts
//! that to the handful being looked for.
//!
//! # Matching
//!
//! Subsequence, not substring: `amd` finds `src/app/mod.rs`, the way every
//! fuzzy finder behaves and the reason typing three characters is enough.
//! Matching is case-insensitive until the query contains an upper-case
//! character, at which point it becomes case-sensitive — the same rule vim and
//! ripgrep use, and it lets `Row` mean something different from `row` without
//! a flag.

/// Whether `query` matches `text` as a subsequence.
///
/// An empty query matches everything, so an empty filter line shows the whole
/// list rather than nothing.
pub fn matches(query: &str, text: &str) -> bool {
    if query.is_empty() {
        return true;
    }

    // Smart case: an upper-case character in the query means the user meant it.
    let sensitive = query.chars().any(char::is_uppercase);

    let mut haystack = text.chars();
    query.chars().all(|wanted| {
        haystack.any(|c| {
            if sensitive {
                c == wanted
            } else {
                c.eq_ignore_ascii_case(&wanted) || c.to_lowercase().eq(wanted.to_lowercase())
            }
        })
    })
}

/// How well `query` matches `text`, for ordering. Lower is better.
///
/// Two things are rewarded: a match packed close together, and one starting
/// near the end of the path. The second matters because a query is usually
/// about a filename, and `mod.rs` should beat a directory three levels up that
/// happens to contain the same letters.
///
/// `None` when the query does not match at all.
pub fn score(query: &str, text: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }

    let sensitive = query.chars().any(char::is_uppercase);
    let same = |a: char, b: char| {
        if sensitive {
            a == b
        } else {
            a.eq_ignore_ascii_case(&b) || a.to_lowercase().eq(b.to_lowercase())
        }
    };

    let chars: Vec<char> = text.chars().collect();
    let mut first = None;
    let mut last = 0;
    let mut at = 0;

    for wanted in query.chars() {
        let found = chars[at..].iter().position(|c| same(*c, wanted))? + at;
        first.get_or_insert(found);
        last = found;
        at = found + 1;
    }

    let start = first.unwrap_or(0);
    // Span of the match, plus how far from the end of the path it began.
    Some((last - start) + chars.len().saturating_sub(start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_query_matches_everything() {
        // An empty filter line shows the whole list, not none of it.
        assert!(matches("", "anything at all"));
        assert!(matches("", ""));
    }

    #[test]
    fn characters_match_in_order_but_not_adjacently() {
        // The reason three characters are enough to find a deep path.
        assert!(matches("amd", "src/app/mod.rs"));
        assert!(matches("srcmod", "src/app/mod.rs"));
    }

    #[test]
    fn characters_out_of_order_do_not_match() {
        assert!(!matches("dma", "src/app/mod.rs"));
    }

    #[test]
    fn a_character_the_text_lacks_does_not_match() {
        assert!(!matches("xyz", "src/app/mod.rs"));
        assert!(!matches("modz", "src/app/mod.rs"));
    }

    #[test]
    fn a_lower_case_query_ignores_case() {
        assert!(matches("readme", "README.md"));
        assert!(matches("cargo", "Cargo.toml"));
    }

    #[test]
    fn an_upper_case_query_demands_it() {
        // Smart case, as vim and ripgrep do it: typing a capital means it.
        assert!(matches("README", "README.md"));
        assert!(!matches("README", "readme.md"));
        assert!(matches("Row", "enum Row {"));
        assert!(!matches("Row", "let row = 1;"));
    }

    #[test]
    fn a_query_longer_than_the_text_cannot_match() {
        assert!(!matches("abcdef", "abc"));
    }

    #[test]
    fn a_non_matching_query_has_no_score() {
        assert_eq!(score("xyz", "src/app/mod.rs"), None);
    }

    #[test]
    fn a_tighter_match_scores_better_than_a_scattered_one() {
        // `mod` sits together in the first and is spread across the second.
        let tight = score("mod", "src/mod.rs").expect("matches");
        let loose = score("mod", "m-many-others-d").expect("matches");
        assert!(tight < loose, "tight {tight} should beat loose {loose}");
    }

    #[test]
    fn a_match_in_the_filename_beats_one_in_a_parent_directory() {
        // A query is usually about a filename.
        let in_name = score("app", "src/deep/app.rs").expect("matches");
        let in_dir = score("app", "app/deep/other.rs").expect("matches");
        assert!(
            in_name < in_dir,
            "filename {in_name} should beat directory {in_dir}"
        );
    }

    #[test]
    fn an_exact_filename_scores_well() {
        let exact = score("mod.rs", "src/app/mod.rs").expect("matches");
        let partial = score("mod.rs", "src/models/other-runner.rs").expect("matches");
        assert!(
            exact < partial,
            "exact {exact} should beat partial {partial}"
        );
    }

    #[test]
    fn matching_and_scoring_agree_on_what_matches() {
        // A query that scores must match, and one that matches must score:
        // the list would otherwise show rows it cannot order, or vice versa.
        for (query, text) in [
            ("amd", "src/app/mod.rs"),
            ("xyz", "src/app/mod.rs"),
            ("", "anything"),
            ("README", "readme.md"),
        ] {
            assert_eq!(
                matches(query, text),
                score(query, text).is_some(),
                "{query:?} against {text:?}"
            );
        }
    }
}
