//! Narrowing who on this machine can read Chief's data.
//!
//! Chief's one promise is that everything stays here. That is a claim about
//! *this machine*, and on a machine with more than one account it has to mean
//! this **user** — the corpus is the user's writing, and the app-config
//! directory holds `chief.db`, which holds the OAuth tokens as plain text
//! (CLAUDE.md records that, and the OS user account is what protects them).
//! A directory readable by everybody makes "protected by the OS user account"
//! weaker than it sounds.
//!
//! **This narrows the exposure; it does not close it.** A token in a file is
//! still a token in a file, and anything running as this user still reads it.
//! Moving them into the OS keychain is the real fix and is REC-28.
//!
//! Two rules and nothing else:
//!
//! - a directory is `0o700` — the owner may enter and list it, nobody else;
//! - a file that carries a credential is `0o600`.
//!
//! On Windows both are a **no-op, deliberately**. The permission model there is
//! ACLs rather than a mode, so there is no correct translation of `0o700` —
//! tightening a Windows ACL properly means naming a SID and building a DACL,
//! which is a different feature and not one this issue is. Reporting failure
//! would be worse than doing nothing, because nothing is what is actually
//! happening.

use std::path::Path;

/// Restrict a directory to its owner.
///
/// **Never fatal.** An unusual mount, a network filesystem, a read-only
/// volume: failing to tighten a permission is not a reason to refuse to run,
/// and refusing would take the whole app down over a directory that is merely
/// as open as it was a moment ago. It is reported and the caller carries on.
///
/// Returns whether the mode was applied, so a caller — and a test — can tell
/// "tightened" from "left as it was".
pub fn restrict_directory(path: &Path) -> bool {
    apply(path, 0o700)
}

/// Restrict a file that carries a credential to its owner.
pub fn restrict_file(path: &Path) -> bool {
    apply(path, 0o600)
}

/// Set a mode, on the platforms that have one.
///
/// The `unix` and `windows` halves are written as one function with two bodies
/// rather than two functions, so the signature cannot drift between them —
/// CLAUDE.md records `#[cfg(windows)]` as where the last two bugs on `main`
/// came from, and a signature that compiles on one platform only is exactly
/// that shape of mistake.
fn apply(path: &Path, mode: u32) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        match std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
            Ok(()) => true,
            Err(error) => {
                eprintln!(
                    "could not restrict {} to its owner: {error}",
                    path.display()
                );
                false
            }
        }
    }

    #[cfg(windows)]
    {
        // Consumed rather than silenced with a lint attribute. An `expect` or
        // an `allow` here would be a claim about a compiler warning on a
        // platform this branch cannot be tested on, and CLAUDE.md records
        // `#[cfg(windows)]` as where the last two bugs on `main` came from.
        let _ = (path, mode);

        false
    }
}

/// Tighten the directories Chief keeps things in, at startup.
///
/// Two of them, and the second is the one that matters. The corpus holds
/// markdown; **the app-config directory holds `chief.db`, and `chief.db` holds
/// the OAuth tokens** — the DLE specification asked for `0700` on the corpus
/// and never mentioned this one. The database file itself is restricted as
/// well as its directory, because a mode on a directory says who may reach a
/// path and a mode on the file says who may read it, and a file created by
/// SQLite before this ran already exists at whatever the umask allowed.
///
/// The corpus is tightened by `Corpus::ensure_shape`, which is where it is
/// created; doing it in both places would mean the settings screen and this
/// disagreeing about who owns the decision.
///
/// **Failing is never fatal.** Every call reports and returns, so a read-only
/// volume or an unusual mount leaves the app running exactly as it did before
/// this existed.
pub fn prepare<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    use tauri::Manager;

    let Ok(config) = app.path().app_config_dir() else {
        eprintln!("could not find the app config directory, so it was left as it is");
        return;
    };

    if config.exists() {
        restrict_directory(&config);
    }

    // `db::DB_URL` is `sqlite:chief.db`, which the SQL plugin resolves against
    // this directory. Named here rather than derived, because the plugin owns
    // the resolution and there is no accessor for the path it chose.
    let database = config.join("chief.db");

    if database.exists() {
        restrict_file(&database);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(std::path::PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(name: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("chief-perms-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("should create a scratch directory");

        Scratch(root)
    }

    /// Compiles and runs on every platform, which is the point.
    ///
    /// A `#[cfg(unix)]` test would leave the Windows arm exercised by nothing
    /// at all — it would compile on Windows and be run by nobody, since the
    /// app build is the only job that touches Windows and it does not run
    /// tests. So the assertion is about the *contract*: it is answered on both,
    /// and neither panics.
    #[test]
    fn answers_on_every_platform_and_never_panics() {
        let scratch = scratch("contract");

        let tightened = restrict_directory(&scratch.0);

        assert_eq!(
            tightened,
            cfg!(unix),
            "Unix applies a mode; Windows is a documented no-op"
        );
    }

    /// The mode as it is written down — octal, because `assert_eq!` on a `u32`
    /// reports `493` and nobody reads that as `0o755`.
    #[cfg(unix)]
    fn mode_of(path: &Path) -> String {
        use std::os::unix::fs::PermissionsExt;

        format!(
            "{:o}",
            std::fs::metadata(path)
                .expect("should stat")
                .permissions()
                .mode()
                & 0o777
        )
    }

    #[test]
    #[cfg(unix)]
    fn a_directory_is_left_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let scratch = scratch("directory");
        std::fs::set_permissions(&scratch.0, std::fs::Permissions::from_mode(0o755))
            .expect("should loosen");

        assert!(restrict_directory(&scratch.0));

        assert_eq!(
            mode_of(&scratch.0),
            "700",
            "nobody but the owner should be able to look"
        );
    }

    /// Proved by applying `0o644` instead:
    ///
    /// ```text
    ///   left: "644"
    ///  right: "600"
    /// ```
    #[test]
    #[cfg(unix)]
    fn a_credential_file_is_left_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let scratch = scratch("file");
        let file = scratch.0.join("chief.db");
        std::fs::write(&file, "not really a database").expect("should write");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644))
            .expect("should loosen");

        assert!(restrict_file(&file));

        assert_eq!(mode_of(&file), "600");
        assert_eq!(
            std::fs::read_to_string(&file).expect("should read"),
            "not really a database",
            "changing a mode does not change what is in the file"
        );
    }

    /// A folder the user has been keeping notes in for months is theirs. The
    /// mode is the only thing this touches.
    #[test]
    #[cfg(unix)]
    fn tightening_an_existing_directory_leaves_its_contents_alone() {
        let scratch = scratch("existing");
        std::fs::create_dir_all(scratch.0.join("journal")).expect("should create");
        std::fs::write(scratch.0.join("journal/2026-08.md"), "# August\n").expect("should write");
        std::fs::write(scratch.0.join("writing_style.md"), "Plain, short.\n")
            .expect("should write");

        assert!(restrict_directory(&scratch.0));

        assert_eq!(
            std::fs::read_to_string(scratch.0.join("journal/2026-08.md")).expect("should read"),
            "# August\n"
        );
        assert_eq!(
            std::fs::read_to_string(scratch.0.join("writing_style.md")).expect("should read"),
            "Plain, short.\n"
        );
    }

    /// Failing to tighten a permission is not a reason to refuse to run.
    ///
    /// A directory that has gone stands in for the unusual mount and the
    /// read-only volume: the call answers `false` rather than raising, and the
    /// caller carries on.
    ///
    /// Proved by unwrapping the `set_permissions` result:
    ///
    /// ```text
    /// should be able to set a mode: Os { code: 2, kind: NotFound,
    ///   message: "No such file or directory" }
    /// ```
    #[test]
    fn a_directory_that_cannot_be_restricted_says_so_rather_than_raising() {
        let missing = std::env::temp_dir().join(format!("chief-perms-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&missing);

        assert!(!restrict_directory(&missing));
    }
}
