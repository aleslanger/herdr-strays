//! The manifest and Cargo.toml have to agree.
//!
//! Two files carry the version, and the release workflow builds a tag from the
//! manifest's copy while `scripts/install.sh` asks GitHub for a release named
//! after it. If Cargo.toml drifts, the crate reports one version and the
//! plugin fetches another — so this is checked rather than remembered.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Read the first `version = "..."` line of a TOML file.
///
/// Deliberately not a TOML parser: the same one-line rule the shell script
/// applies (`sed -n 's/^version *= *"..."'`) is what this must verify, and a
/// real parser would accept files the script cannot read.
fn first_version(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .find_map(|line| {
            let rest = line.strip_prefix("version")?;
            let rest = rest.trim_start().strip_prefix('=')?;
            let rest = rest.trim_start().strip_prefix('"')?;
            rest.split('"').next()
        })
        .map(str::to_owned)
}

#[test]
fn the_manifest_and_the_crate_carry_the_same_version() {
    let root = repo_root();
    let cargo = first_version(&root.join("Cargo.toml")).expect("Cargo.toml has a version");
    let manifest =
        first_version(&root.join("herdr-plugin.toml")).expect("herdr-plugin.toml has a version");

    assert_eq!(
        cargo, manifest,
        "Cargo.toml says {cargo} and herdr-plugin.toml says {manifest}; \
         a release tag follows the manifest, so the two must match"
    );
}

#[test]
fn the_manifest_launches_the_binary_the_install_script_writes() {
    // install.sh puts the binary in bin/, and `plugin link` skips the build
    // step entirely — so a manifest pointing anywhere else launches nothing.
    //
    // The `./` is not decoration. Measured against herdr 0.7.3: it resolves a
    // command containing a path separator against the plugin root, looks a bare
    // `bin/herdr-strays` up in $PATH, and never expands $HERDR_PLUGIN_ROOT —
    // both of the latter fail with "No viable candidates found in PATH".
    let manifest = std::fs::read_to_string(repo_root().join("herdr-plugin.toml"))
        .expect("herdr-plugin.toml is readable");

    assert!(
        manifest.contains("\"./bin/herdr-strays\""),
        "the pane command must launch ./bin/herdr-strays"
    );
    // Only the commands, not the prose explaining why: install.sh reads the
    // variable too, and there a shell does expand it.
    let in_a_command = manifest
        .lines()
        .filter(|l| l.trim_start().starts_with("command ="))
        .any(|l| l.contains("HERDR_PLUGIN_ROOT"));
    assert!(
        !in_a_command,
        "herdr does not expand that variable; the command reaches the spawner \
         as a literal and nothing starts"
    );
}

#[test]
fn the_build_step_runs_the_install_script() {
    let manifest = std::fs::read_to_string(repo_root().join("herdr-plugin.toml"))
        .expect("herdr-plugin.toml is readable");

    assert!(
        manifest.contains("scripts/install.sh"),
        "the build step must run the install script, not cargo directly — \
         installing without a Rust toolchain depends on it"
    );
}

#[test]
fn the_install_script_is_posix_sh() {
    // herdr runs plugin commands with a minimal PATH, and the manifest invokes
    // this with `sh` — bashisms would fail there rather than here.
    let text = std::fs::read_to_string(repo_root().join("scripts/install.sh"))
        .expect("install.sh is readable");

    // Compared line by line rather than against "#!/bin/sh\n": .gitattributes
    // asks for LF everywhere, but a checkout made before it existed still has
    // CRLF, and the shebang would then be read as "#!/bin/sh\r" and miss.
    assert_eq!(
        text.lines().next(),
        Some("#!/bin/sh"),
        "install.sh must declare POSIX sh"
    );
}

#[test]
fn the_install_script_verifies_what_it_downloads() {
    // A binary fetched over the network is executed on the user's machine. The
    // checksum step is the only thing standing between a tampered release and
    // that, so its absence must fail loudly rather than quietly.
    let text = std::fs::read_to_string(repo_root().join("scripts/install.sh"))
        .expect("install.sh is readable");

    assert!(
        text.contains("SHA256SUMS"),
        "install.sh must fetch the published checksums"
    );
    assert!(
        text.contains("checksum mismatch"),
        "install.sh must refuse an archive whose hash does not match"
    );
}
