//! Keeping annotations on disk between sessions.
//!
//! # Where
//!
//! `$XDG_STATE_HOME/herdr-strays/`, falling back to `~/.local/state/`. Outside
//! the repository on purpose: notes are one reviewer's working memory, not a
//! shared artefact, and a file written into the worktree would show up as a
//! stray in the very list it was written from.
//!
//! # Format
//!
//! One record per line, tab-separated: `line<TAB>hash<TAB>kind<TAB>path<TAB>text`.
//! Hand-rolled rather than JSON for the reason the herdr parsers are: the shape
//! is three integers and two strings, a dependency would be the larger cost,
//! and an unreadable file must degrade to "no annotations" rather than fail.
//!
//! Paths and text are escaped so a tab or a newline in either cannot break the
//! record apart. Nothing here is executed or interpreted — it is read back as
//! data and rendered.

use std::path::{Path, PathBuf};

use super::{Anchor, Annotation, Annotations, Kind};

#[derive(Debug)]
pub enum StoreError {
    /// No state directory could be determined, so there is nowhere to write.
    NoStateDir,
    Io(std::io::Error),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::NoStateDir => {
                write!(f, "no state directory — set XDG_STATE_HOME or HOME")
            }
            StoreError::Io(e) => write!(f, "could not write annotations: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// The directory annotations live in.
fn state_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("herdr-strays"));
        }
    }
    Some(crate::home::dir()?.join(".local/state/herdr-strays"))
}

/// The file holding one repository's annotations, under a given state root.
///
/// Named by a hash of the worktree path rather than the path itself: a
/// repository path contains separators and is longer than some filesystems
/// allow, and the directory is ours alone so collisions are the only risk worth
/// managing.
fn file_in(dir: &Path, repo: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    repo.hash(&mut hasher);
    let key = hasher.finish();

    // The directory name is kept alongside the hash purely so a human looking
    // in the state directory can tell which file belongs to which checkout.
    let name = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    let name: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();

    dir.join(format!("{name}-{key:016x}.tsv"))
}

/// Read the annotations recorded for `repo`.
///
/// A missing or unreadable file means no annotations, not an error: the notes
/// are an addition to the viewer, and losing them must not stop it working.
pub fn load(repo: &Path) -> Annotations {
    let Some(dir) = state_dir() else {
        return Annotations::new();
    };
    load_from(&dir, repo)
}

/// [`load`], against an explicit state directory.
fn load_from(dir: &Path, repo: &Path) -> Annotations {
    let Ok(text) = std::fs::read_to_string(file_in(dir, repo)) else {
        return Annotations::new();
    };
    Annotations::from_items(text.lines().filter_map(parse_record).collect())
}

/// Write `annotations` for `repo`, replacing whatever was there.
///
/// An empty collection removes the file rather than leaving an empty one, so
/// the state directory does not fill with the residue of finished reviews.
pub fn save(repo: &Path, annotations: &Annotations) -> Result<(), StoreError> {
    let dir = state_dir().ok_or(StoreError::NoStateDir)?;
    save_into(&dir, repo, annotations)
}

/// [`save`], against an explicit state directory.
fn save_into(dir: &Path, repo: &Path, annotations: &Annotations) -> Result<(), StoreError> {
    let path = file_in(dir, repo);

    if annotations.is_empty() {
        match std::fs::remove_file(&path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(StoreError::Io(e)),
        }
    }

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(StoreError::Io)?;
    }

    let body: String = annotations.iter().map(format_record).collect();

    // Written to a neighbour and renamed, so an interrupted write cannot leave
    // a half-file that reads back as fewer annotations than were made.
    let temp = path.with_extension("tsv.new");
    std::fs::write(&temp, body).map_err(StoreError::Io)?;
    std::fs::rename(&temp, &path).map_err(StoreError::Io)
}

/// `line<TAB>hash<TAB>kind<TAB>path<TAB>text`, newline-terminated.
fn format_record(a: &Annotation) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\n",
        a.anchor.line,
        a.anchor.hash,
        a.kind.label(),
        escape(&a.anchor.file.to_string_lossy()),
        escape(&a.text)
    )
}

/// Read one record back, skipping anything that does not parse.
///
/// A record this version cannot read is dropped rather than failing the load:
/// one malformed line should cost one annotation, not all of them.
fn parse_record(line: &str) -> Option<Annotation> {
    let mut fields = line.split('\t');
    let stored_line = fields.next()?.parse().ok()?;
    let hash = fields.next()?.parse().ok()?;
    let kind = Kind::parse(fields.next()?);
    let file = unescape(fields.next()?);
    // The text is last, so a tab surviving in it cannot shift the other fields.
    let text = unescape(&fields.collect::<Vec<_>>().join("\t"));

    if text.is_empty() {
        return None;
    }

    Some(Annotation {
        anchor: Anchor {
            file: PathBuf::from(file),
            line: stored_line,
            hash,
        },
        kind,
        text,
    })
}

/// Make a field safe to sit between tabs on one line.
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Reverse [`escape`].
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();

    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            // An escape this version does not know: keep both characters
            // rather than swallowing them.
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn annotation(file: &str, line: u32, text: &str, kind: Kind) -> Annotation {
        Annotation {
            anchor: Anchor {
                file: PathBuf::from(file),
                line,
                hash: 0xdead_beef,
            },
            kind,
            text: text.to_string(),
        }
    }

    #[test]
    fn a_record_survives_the_round_trip() {
        let original = annotation("src/main.rs", 42, "why is this here?", Kind::Question);
        let restored = parse_record(format_record(&original).trim_end()).expect("parses back");

        assert_eq!(restored, original);
    }

    #[test]
    fn a_tab_in_the_text_does_not_break_the_record_apart() {
        let original = annotation("a.rs", 1, "before\tafter", Kind::Note);
        let restored = parse_record(format_record(&original).trim_end()).expect("parses back");

        assert_eq!(restored.text, "before\tafter");
    }

    #[test]
    fn a_newline_in_the_text_stays_inside_one_record() {
        // Two records where one was meant would silently invent an annotation.
        let original = annotation("a.rs", 1, "first\nsecond", Kind::Issue);
        let formatted = format_record(&original);

        assert_eq!(formatted.matches('\n').count(), 1, "only the terminator");
        assert_eq!(
            parse_record(formatted.trim_end()).unwrap().text,
            "first\nsecond"
        );
    }

    #[test]
    fn a_path_containing_a_tab_survives() {
        let original = annotation("odd\tname.rs", 1, "hi", Kind::Note);
        let restored = parse_record(format_record(&original).trim_end()).unwrap();

        assert_eq!(restored.anchor.file, PathBuf::from("odd\tname.rs"));
    }

    #[test]
    fn a_backslash_in_the_text_is_not_eaten() {
        let original = annotation("a.rs", 1, r"a \n that is literal", Kind::Note);
        let restored = parse_record(format_record(&original).trim_end()).unwrap();

        assert_eq!(restored.text, r"a \n that is literal");
    }

    #[test]
    fn a_malformed_record_is_skipped_rather_than_failing_the_load() {
        assert_eq!(parse_record("garbage"), None);
        assert_eq!(parse_record(""), None);
        assert_eq!(parse_record("1\tnot-a-hash\tissue\ta.rs\ttext"), None);
        assert_eq!(parse_record("1\t2\tissue\ta.rs\t"), None, "empty text");
    }

    // These drive `save_into`/`load_from` with an explicit directory rather
    // than setting XDG_STATE_HOME: the environment is process-wide, and cargo
    // runs tests in parallel threads, so one test's directory would leak into
    // another's.

    #[test]
    fn saving_and_loading_returns_what_was_written() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = Path::new("/tmp/some-repo");
        let notes = Annotations::new()
            .with(annotation("src/a.rs", 3, "look here", Kind::Issue))
            .with(annotation("src/b.rs", 9, "and here", Kind::Suggestion));

        save_into(dir.path(), repo, &notes).expect("save succeeds");
        let loaded = load_from(dir.path(), repo);

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.iter().next().unwrap().text, "look here");
    }

    #[test]
    fn two_repositories_do_not_share_a_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let one = Path::new("/tmp/repo-one");
        let two = Path::new("/tmp/repo-two");

        save_into(
            dir.path(),
            one,
            &Annotations::new().with(annotation("a.rs", 1, "one", Kind::Note)),
        )
        .unwrap();
        save_into(
            dir.path(),
            two,
            &Annotations::new().with(annotation("a.rs", 1, "two", Kind::Note)),
        )
        .unwrap();

        assert_eq!(
            load_from(dir.path(), one).iter().next().unwrap().text,
            "one"
        );
        assert_eq!(
            load_from(dir.path(), two).iter().next().unwrap().text,
            "two"
        );
    }

    #[test]
    fn clearing_the_annotations_removes_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = Path::new("/tmp/repo-cleared");

        save_into(
            dir.path(),
            repo,
            &Annotations::new().with(annotation("a.rs", 1, "x", Kind::Note)),
        )
        .unwrap();
        save_into(dir.path(), repo, &Annotations::new()).expect("clearing succeeds");

        assert!(load_from(dir.path(), repo).is_empty());
        // Saving an empty set twice must not fail on the already-absent file.
        save_into(dir.path(), repo, &Annotations::new()).expect("clearing again is fine");
    }

    #[test]
    fn loading_a_repository_with_no_annotations_is_empty_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(load_from(dir.path(), Path::new("/tmp/never-annotated")).is_empty());
    }

    #[test]
    fn an_interrupted_write_cannot_leave_a_half_file() {
        // The rename is what guarantees this; the test pins the behaviour that
        // no `.new` residue is left behind on success.
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = Path::new("/tmp/repo-atomic");

        save_into(
            dir.path(),
            repo,
            &Annotations::new().with(annotation("a.rs", 1, "x", Kind::Note)),
        )
        .unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".new"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "the temporary file must be renamed away"
        );
    }
}
