//! Where this user's files live, on every platform herdr runs on.
//!
//! Config and annotations both need a home directory to fall back on when the
//! XDG variables are unset. Unix sets `HOME`; Windows generally does not, so
//! asking for `HOME` alone would leave a Windows user with no config file and
//! no annotations, silently — the code would take the `None` branch and carry
//! on as if nothing had been configured.
//!
//! The lookup order matches what the `dirs` crate does, without the dependency:
//! `HOME` first so a Unix user's export always wins, then Windows' own
//! `USERPROFILE`, then the `HOMEDRIVE`/`HOMEPATH` pair that older Windows
//! setups still provide.

use std::ffi::OsString;
use std::path::PathBuf;

/// This user's home directory, or [`None`] if the environment names none.
///
/// An empty variable counts as unset: `HOME=` would otherwise put the config
/// file at the filesystem root.
pub fn dir() -> Option<PathBuf> {
    if let Some(home) = non_empty("HOME") {
        return Some(PathBuf::from(home));
    }
    if let Some(profile) = non_empty("USERPROFILE") {
        return Some(PathBuf::from(profile));
    }
    // Older Windows splits the home directory across two variables; neither
    // half is usable alone.
    let drive = non_empty("HOMEDRIVE")?;
    let path = non_empty("HOMEPATH")?;
    let mut joined = drive;
    joined.push(path);
    Some(PathBuf::from(joined))
}

/// Read a variable, treating empty as absent.
fn non_empty(key: &str) -> Option<OsString> {
    std::env::var_os(key).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These tests set process-wide environment variables, so they must not run
    /// concurrently with each other. Rust runs tests in threads by default, so
    /// one mutex serialises them.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Run `body` with exactly the given variables set, restoring the
    /// environment afterwards even if `body` panics.
    fn with_env(vars: &[(&str, &str)], body: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        const KEYS: [&str; 4] = ["HOME", "USERPROFILE", "HOMEDRIVE", "HOMEPATH"];

        let saved: Vec<_> = KEYS.iter().map(|k| (*k, std::env::var_os(k))).collect();
        // SAFETY: the mutex above keeps any other test from reading or writing
        // the environment while these calls run.
        unsafe {
            for key in KEYS {
                std::env::remove_var(key);
            }
            for (key, value) in vars {
                std::env::set_var(key, value);
            }
        }

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));

        // SAFETY: as above.
        unsafe {
            for (key, value) in saved {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    }

    #[test]
    fn home_is_used_when_set() {
        with_env(&[("HOME", "/home/ada")], || {
            assert_eq!(dir(), Some(PathBuf::from("/home/ada")));
        });
    }

    #[test]
    fn userprofile_stands_in_where_there_is_no_home() {
        // The Windows case: without this the config file is never found.
        with_env(&[("USERPROFILE", r"C:\Users\Ada")], || {
            assert_eq!(dir(), Some(PathBuf::from(r"C:\Users\Ada")));
        });
    }

    #[test]
    fn home_wins_over_userprofile() {
        // A Unix user who exports HOME means it, even under an emulator that
        // also sets USERPROFILE.
        with_env(
            &[("HOME", "/home/ada"), ("USERPROFILE", r"C:\Users\Ada")],
            || {
                assert_eq!(dir(), Some(PathBuf::from("/home/ada")));
            },
        );
    }

    #[test]
    fn the_drive_and_path_pair_is_joined() {
        with_env(&[("HOMEDRIVE", "C:"), ("HOMEPATH", r"\Users\Ada")], || {
            assert_eq!(dir(), Some(PathBuf::from(r"C:\Users\Ada")));
        });
    }

    #[test]
    fn half_the_pair_is_not_a_home() {
        // `C:` alone is not this user's directory; guessing would write files
        // somewhere unexpected.
        with_env(&[("HOMEDRIVE", "C:")], || assert_eq!(dir(), None));
        with_env(&[("HOMEPATH", r"\Users\Ada")], || assert_eq!(dir(), None));
    }

    #[test]
    fn an_empty_variable_is_no_home_at_all() {
        // `HOME=` would otherwise put the config file at the filesystem root.
        with_env(&[("HOME", "")], || assert_eq!(dir(), None));
        with_env(&[("HOME", ""), ("USERPROFILE", r"C:\Users\Ada")], || {
            assert_eq!(dir(), Some(PathBuf::from(r"C:\Users\Ada")));
        });
    }

    #[test]
    fn nothing_set_names_no_home() {
        with_env(&[], || assert_eq!(dir(), None));
    }
}
