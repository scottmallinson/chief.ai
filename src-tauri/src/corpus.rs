//! The folder of markdown Chief reads from and writes to.
//!
//! Every source document describes this layer the same way: plain, human-
//! editable files that outlive the application. So it is a folder in the user's
//! own directory rather than a hidden one inside the app's data — hiding a
//! corpus whose whole point is that a person can open it contradicts it.
//!
//! ## Nothing here reaches the renderer directly
//!
//! The web view holds no filesystem permission, deliberately, and this module
//! does not change that: reads and writes go through typed commands so that
//! path checking has exactly one home rather than one per caller.
//!
//! ## What is a path here
//!
//! Every path this module accepts is **relative to the root and made only of
//! ordinary components**. No absolute paths, no `..`, no drive prefixes. That
//! is checked before anything touches the disk, because the alternative — a
//! renderer or a model naming `../../.ssh/id_rsa` — is the failure that matters
//! and it is cheap to make impossible.

use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use sqlx::SqlitePool;

/// Where the corpus lives, when the user has not said otherwise.
const DEFAULT_FOLDER: &str = "Chief";

/// The settings key holding the user's chosen root.
const ROOT_KEY: &str = "corpus.root";

/// The shape created on first run.
///
/// Empty folders with a heading in each beat an empty folder: somebody opening
/// this for the first time should be able to see where a meeting note goes
/// without reading documentation. The names follow the source documents rather
/// than being invented here.
const SHAPE: &[(&str, &str)] = &[
    (
        "context/agents/profile/writing_style.md",
        "# Writing style\n\nHow you write, so drafts sound like you rather than like a model.\nTone, greetings and sign-offs, the things you never say.\n",
    ),
    (
        "context/agents/org/team_structure.md",
        "# Team\n\nWho you work with, what they own, and what is going on with them.\nOne heading per person.\n",
    ),
    (
        "briefs/README.md",
        "# Briefs\n\nChief writes daily briefs here, one file per day. Yours to edit or delete.\n",
    ),
    (
        "proposed/README.md",
        "# Proposed actions\n\nDrafts Chief prepared for things it noticed. Nothing here has been sent.\n",
    ),
    (
        "meeting-notes/README.md",
        "# Meeting notes\n\nOne file per meeting. Chief reads these when preparing you for the next one.\n",
    ),
    (
        "1-1s/README.md",
        "# One-to-ones\n\nThe running record of each working relationship. One file per person.\n",
    ),
    (
        "journal/README.md",
        "# Journal\n\nHow the days actually went. Chief reads these for patterns you would not spot in one.\n",
    ),
];

/// The starter Chief writes at `path`, if that is a file it creates.
///
/// Exposed so `profile.rs` can tell an untouched starter from a file the user
/// has made theirs, without keeping a second copy of these strings.
#[must_use]
pub fn starter(path: &str) -> Option<&'static str> {
    SHAPE
        .iter()
        .find(|(name, _)| *name == path)
        .map(|(_, contents)| *contents)
}

/// What can go wrong reading or writing the corpus.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("'{0}' is not a path inside the corpus")]
    OutsideCorpus(String),
    #[error("there is no file called '{0}' in the corpus")]
    NoSuchFile(String),
    #[error("could not read the corpus at {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not write to the corpus at {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Storage(#[from] crate::db::Error),
    #[error("could not read the corpus index: {0}")]
    Index(String),
}

impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        Self::Index(error.to_string())
    }
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// One file in the corpus, as the interface and the index see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// Relative to the root, with forward slashes whatever the platform.
    pub path: String,
    /// Bytes on disk.
    pub size: u64,
    /// When it last changed, ISO-8601, or empty if the platform would not say.
    pub modified_at: String,
    /// Roughly what it would cost to put in a prompt. See [`crate::context`].
    pub estimated_tokens: u32,
}

/// A folder of markdown, rooted somewhere the user can find it.
#[derive(Debug, Clone)]
pub struct Corpus {
    root: PathBuf,
}

impl Corpus {
    /// A corpus at `root`. The folder need not exist yet.
    #[must_use]
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Turn a relative path into an absolute one, or refuse.
    ///
    /// The whole of the traversal defence. Structural rather than
    /// canonicalising, because a file being written does not exist yet and
    /// `canonicalize` cannot answer for it — so every component is required to
    /// be an ordinary name, which no traversal survives.
    pub fn resolve(&self, relative: &str) -> Result<PathBuf, Error> {
        let candidate = Path::new(relative);
        let refuse = || Error::OutsideCorpus(relative.to_string());

        if relative.trim().is_empty() {
            return Err(refuse());
        }

        for component in candidate.components() {
            match component {
                Component::Normal(part) => {
                    // A component that is only dots is not a name anyone means.
                    if part.to_string_lossy().chars().all(|c| c == '.') {
                        return Err(refuse());
                    }
                }
                // Absolute paths, `..`, `.`, and Windows drive prefixes.
                _ => return Err(refuse()),
            }
        }

        Ok(self.root.join(candidate))
    }

    /// Create the folders and the starter files, leaving anything that exists.
    ///
    /// Safe to run on every launch: a file the user has edited is never
    /// replaced, because the only files written are ones that are not there.
    pub async fn ensure_shape(&self) -> Result<(), Error> {
        for (path, contents) in SHAPE {
            let absolute = self.resolve(path)?;

            if absolute.exists() {
                continue;
            }

            if let Some(parent) = absolute.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|source| Error::Write {
                        path: parent.display().to_string(),
                        source,
                    })?;
            }

            tokio::fs::write(&absolute, contents)
                .await
                .map_err(|source| Error::Write {
                    path: absolute.display().to_string(),
                    source,
                })?;
        }

        Ok(())
    }

    /// Read one file.
    pub async fn read(&self, relative: &str) -> Result<String, Error> {
        let absolute = self.resolve(relative)?;

        match tokio::fs::read_to_string(&absolute).await {
            Ok(contents) => Ok(contents),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                Err(Error::NoSuchFile(relative.to_string()))
            }
            Err(source) => Err(Error::Read {
                path: absolute.display().to_string(),
                source,
            }),
        }
    }

    /// Write one file, creating the folders above it.
    pub async fn write(&self, relative: &str, contents: &str) -> Result<(), Error> {
        let absolute = self.resolve(relative)?;

        if let Some(parent) = absolute.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| Error::Write {
                    path: parent.display().to_string(),
                    source,
                })?;
        }

        tokio::fs::write(&absolute, contents)
            .await
            .map_err(|source| Error::Write {
                path: absolute.display().to_string(),
                source,
            })
    }

    /// When one file last changed, as an ISO-8601 instant, if it is there.
    pub async fn modified_at(&self, relative: &str) -> Option<String> {
        let absolute = self.resolve(relative).ok()?;
        let metadata = tokio::fs::metadata(&absolute).await.ok()?;

        Some(modified_at(&metadata)).filter(|at| !at.is_empty())
    }

    /// Every markdown file in the corpus, deepest folders included.
    ///
    /// Markdown only: the corpus is a folder in the user's own directory and
    /// whatever else they keep beside their notes is not Chief's to read.
    pub async fn list(&self) -> Result<Vec<Entry>, Error> {
        let mut entries = Vec::new();
        let mut pending = vec![self.root.clone()];

        while let Some(directory) = pending.pop() {
            let mut reading = match tokio::fs::read_dir(&directory).await {
                Ok(reading) => reading,
                // A corpus nobody has created yet is empty, not broken.
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => {
                    return Err(Error::Read {
                        path: directory.display().to_string(),
                        source,
                    })
                }
            };

            while let Ok(Some(item)) = reading.next_entry().await {
                let path = item.path();

                let Ok(metadata) = item.metadata().await else {
                    continue;
                };

                if metadata.is_dir() {
                    pending.push(path);
                    continue;
                }

                // Markdown only, and spelled without `is_none_or`: that is
                // stable since 1.82 and this crate builds on 1.77.
                if !matches!(
                    path.extension().and_then(std::ffi::OsStr::to_str),
                    Some("md")
                ) {
                    continue;
                }

                let Some(relative) = self.relative(&path) else {
                    continue;
                };

                entries.push(Entry {
                    path: relative,
                    size: metadata.len(),
                    modified_at: modified_at(&metadata),
                    estimated_tokens: crate::context::estimate_tokens_in_bytes(metadata.len()),
                });
            }
        }

        // Stable, so two listings of an unchanged corpus agree and the index
        // does not churn.
        entries.sort_by(|one, other| one.path.cmp(&other.path));

        Ok(entries)
    }

    /// A path below the root, as the slash-separated string the index stores.
    fn relative(&self, absolute: &Path) -> Option<String> {
        let relative = absolute.strip_prefix(&self.root).ok()?;

        Some(
            relative
                .components()
                .filter_map(|component| match component {
                    Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("/"),
        )
    }
}

/// When a file last changed, as an ISO-8601 instant.
fn modified_at(metadata: &std::fs::Metadata) -> String {
    metadata
        .modified()
        .ok()
        .map(chrono::DateTime::<chrono::Utc>::from)
        .map(|at| at.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_default()
}

/// Where the corpus is, for this installation.
///
/// The user's own directory, because these files are theirs. Stored as a
/// setting so it can be pointed somewhere else — a synced folder, a different
/// disk — without a migration.
pub async fn root(pool: &SqlitePool, home: &Path) -> Result<PathBuf, Error> {
    let stored = crate::settings::get(pool, ROOT_KEY).await?;

    Ok(stored
        .filter(|path| !path.trim().is_empty())
        .map_or_else(|| home.join(DEFAULT_FOLDER), PathBuf::from))
}

/// Point the corpus somewhere else, or back at the default.
pub async fn set_root(pool: &SqlitePool, path: Option<&str>) -> Result<(), Error> {
    match path.map(str::trim).filter(|path| !path.is_empty()) {
        Some(path) => crate::settings::set(pool, ROOT_KEY, path).await?,
        None => crate::settings::clear(pool, ROOT_KEY).await?,
    }

    Ok(())
}

/// Replace the index with what is on disk right now.
///
/// The index exists so that assembling a prompt is a query rather than a walk
/// of the whole folder: a recipe wants "every always-loaded agent file, and
/// what they would cost", and that is a `WHERE` clause here and a lot of
/// `stat` calls otherwise.
pub async fn reindex(pool: &SqlitePool, corpus: &Corpus) -> Result<Vec<Entry>, Error> {
    let entries = corpus.list().await?;

    let mut transaction = pool.begin().await?;

    sqlx::query("DELETE FROM corpus_files")
        .execute(&mut *transaction)
        .await?;

    for entry in &entries {
        sqlx::query(
            "INSERT INTO corpus_files (path, size, modified_at, estimated_tokens)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(&entry.path)
        .bind(i64::try_from(entry.size).unwrap_or(i64::MAX))
        .bind(&entry.modified_at)
        .bind(i64::from(entry.estimated_tokens))
        .execute(&mut *transaction)
        .await?;
    }

    transaction.commit().await?;

    Ok(entries)
}

/// What the index currently holds.
///
/// The reason the index exists: a recipe asks for the always-loaded corpus
/// files and what they would cost, and that is a statement here rather than a
/// walk of the folder and a `stat` per file.
pub async fn indexed(pool: &SqlitePool) -> Result<Vec<Entry>, Error> {
    let rows: Vec<(String, i64, String, i64)> = sqlx::query_as(
        "SELECT path, size, modified_at, estimated_tokens FROM corpus_files ORDER BY path",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(path, size, modified_at, estimated_tokens)| Entry {
            path,
            size: u64::try_from(size).unwrap_or(0),
            modified_at,
            estimated_tokens: u32::try_from(estimated_tokens).unwrap_or(u32::MAX),
        })
        .collect())
}

/// Where the corpus is and what is in it, for the settings screen.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    /// The folder, as a person would type it.
    pub root: String,
    /// Whether it is there yet.
    pub exists: bool,
    /// How many markdown files are in it.
    pub files: usize,
    /// Roughly what the whole corpus would cost to put in a prompt — which is
    /// far more than any one prompt may spend, and is the point: retrieval
    /// picks from this, it never sends it.
    pub estimated_tokens: u32,
}

/// The corpus for this installation.
async fn open<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(Corpus, SqlitePool), Error> {
    use tauri::Manager;

    let pool = crate::db::pool(app).await?;
    let home = app
        .path()
        .home_dir()
        .map_err(|error| Error::Index(error.to_string()))?;

    Ok((Corpus::at(root(&pool, &home).await?), pool))
}

/// Where the corpus is, and what is in it.
#[tauri::command]
pub async fn corpus_location<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Location, Error> {
    let (corpus, pool) = open(&app).await?;
    let entries = reindex(&pool, &corpus).await?;

    Ok(Location {
        root: corpus.root().display().to_string(),
        exists: corpus.root().is_dir(),
        files: entries.len(),
        estimated_tokens: entries.iter().map(|entry| entry.estimated_tokens).sum(),
    })
}

/// Every markdown file in the corpus.
#[tauri::command]
pub async fn list_corpus<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<Vec<Entry>, Error> {
    let (corpus, pool) = open(&app).await?;

    reindex(&pool, &corpus).await
}

/// One file's contents.
#[tauri::command]
pub async fn read_corpus_file<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    path: String,
) -> Result<String, Error> {
    let (corpus, _) = open(&app).await?;

    corpus.read(&path).await
}

/// Write one file, then bring the index back in step with the disk.
#[tauri::command]
pub async fn write_corpus_file<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    path: String,
    contents: String,
) -> Result<Vec<Entry>, Error> {
    let (corpus, pool) = open(&app).await?;
    corpus.write(&path, &contents).await?;

    reindex(&pool, &corpus).await
}

/// Point the corpus somewhere else, or back at the default.
///
/// Creates the shape at the new location, so a folder chosen here is usable
/// immediately rather than after a restart.
#[tauri::command]
pub async fn set_corpus_root<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    path: Option<String>,
) -> Result<Location, Error> {
    let pool = crate::db::pool(&app).await?;
    set_root(&pool, path.as_deref()).await?;

    let (corpus, _) = open(&app).await?;
    corpus.ensure_shape().await?;

    corpus_location(app).await
}

/// Create the corpus if it is not there, at startup.
///
/// Failing is not fatal: a corpus that cannot be created — a read-only home, a
/// path the user pointed somewhere that has gone — leaves the rest of the app
/// working, and the settings screen is where that gets said.
pub fn prepare<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let app = app.clone();

    tauri::async_runtime::spawn(async move {
        match open(&app).await {
            Ok((corpus, pool)) => {
                if let Err(error) = corpus.ensure_shape().await {
                    eprintln!("the corpus could not be created: {error}");
                    return;
                }

                if let Err(error) = reindex(&pool, &corpus).await {
                    eprintln!("the corpus could not be indexed: {error}");
                }
            }
            Err(error) => eprintln!("the corpus could not be opened: {error}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::migrated_pool;

    /// A corpus in a directory of its own, cleaned up with the test.
    struct Scratch {
        corpus: Corpus,
        root: PathBuf,
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn scratch(name: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("chief-corpus-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        Scratch {
            corpus: Corpus::at(root.clone()),
            root,
        }
    }

    #[test]
    fn refuses_to_climb_out_of_the_corpus() {
        let corpus = Corpus::at(PathBuf::from("/home/someone/Chief"));

        for attempt in [
            "../.ssh/id_rsa",
            "briefs/../../.ssh/id_rsa",
            "/etc/passwd",
            "..",
            "./secret.md",
            "",
            "   ",
        ] {
            assert!(
                corpus.resolve(attempt).is_err(),
                "'{attempt}' should not resolve to anything"
            );
        }
    }

    #[test]
    fn resolves_an_ordinary_path_below_the_root() {
        let corpus = Corpus::at(PathBuf::from("/home/someone/Chief"));

        assert_eq!(
            corpus
                .resolve("briefs/2026-08-28.md")
                .expect("should resolve"),
            Path::new("/home/someone/Chief/briefs/2026-08-28.md")
        );
    }

    #[tokio::test]
    async fn writes_and_reads_a_file_back() {
        let scratch = scratch("roundtrip");

        scratch
            .corpus
            .write("briefs/2026-08-28.md", "# Today\n")
            .await
            .expect("should write");

        assert_eq!(
            scratch
                .corpus
                .read("briefs/2026-08-28.md")
                .await
                .expect("read"),
            "# Today\n"
        );
    }

    #[tokio::test]
    async fn a_missing_file_says_so_rather_than_reporting_a_disk_error() {
        let scratch = scratch("missing");

        let error = scratch
            .corpus
            .read("briefs/never-written.md")
            .await
            .expect_err("there is no such file");

        assert!(matches!(error, Error::NoSuchFile(_)), "{error:?}");
    }

    #[tokio::test]
    async fn the_starter_shape_never_overwrites_what_the_user_has_written() {
        let scratch = scratch("shape");

        scratch.corpus.ensure_shape().await.expect("should create");
        scratch
            .corpus
            .write("context/agents/profile/writing_style.md", "mine\n")
            .await
            .expect("should write");

        // Every launch runs this.
        scratch.corpus.ensure_shape().await.expect("should create");

        assert_eq!(
            scratch
                .corpus
                .read("context/agents/profile/writing_style.md")
                .await
                .expect("read"),
            "mine\n",
            "a second run must not put the template back"
        );
    }

    #[tokio::test]
    async fn lists_markdown_from_every_depth_and_ignores_everything_else() {
        let scratch = scratch("listing");

        scratch
            .corpus
            .write("briefs/one.md", "a")
            .await
            .expect("write");
        scratch
            .corpus
            .write("context/agents/org/team_structure.md", "bb")
            .await
            .expect("write");
        scratch
            .corpus
            .write("notes.txt", "not markdown")
            .await
            .expect("write");

        let listed = scratch.corpus.list().await.expect("should list");
        let paths: Vec<&str> = listed.iter().map(|entry| entry.path.as_str()).collect();

        assert_eq!(
            paths,
            ["briefs/one.md", "context/agents/org/team_structure.md"]
        );
        assert_eq!(listed[0].size, 1);
    }

    #[tokio::test]
    async fn a_corpus_nobody_has_created_yet_is_empty_rather_than_broken() {
        let scratch = scratch("absent");

        assert_eq!(
            scratch.corpus.list().await.expect("should list"),
            Vec::new()
        );
    }

    #[tokio::test]
    async fn the_index_reflects_the_folder_after_an_edit_made_elsewhere() {
        let pool = migrated_pool().await;
        let scratch = scratch("index");

        scratch
            .corpus
            .write("briefs/one.md", "a")
            .await
            .expect("write");
        reindex(&pool, &scratch.corpus).await.expect("should index");
        assert_eq!(indexed(&pool).await.expect("read").len(), 1);

        // The user deletes one in their editor and adds two more.
        std::fs::remove_file(scratch.root.join("briefs/one.md")).expect("delete");
        scratch
            .corpus
            .write("briefs/two.md", "b")
            .await
            .expect("write");
        scratch
            .corpus
            .write("journal/day.md", "c")
            .await
            .expect("write");

        reindex(&pool, &scratch.corpus).await.expect("should index");
        let held = indexed(&pool).await.expect("read");

        assert_eq!(
            held.iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["briefs/two.md", "journal/day.md"],
            "the index is what is on disk, not what was there before"
        );
    }

    #[tokio::test]
    async fn the_corpus_defaults_to_the_users_own_directory() {
        let pool = migrated_pool().await;

        assert_eq!(
            root(&pool, Path::new("/home/someone")).await.expect("read"),
            Path::new("/home/someone/Chief"),
            "not a dotfile: a corpus a person cannot find is not human-editable"
        );
    }

    #[tokio::test]
    async fn the_corpus_can_be_pointed_somewhere_else_and_back() {
        let pool = migrated_pool().await;

        set_root(&pool, Some("/Volumes/Work/Notes"))
            .await
            .expect("set");
        assert_eq!(
            root(&pool, Path::new("/home/someone")).await.expect("read"),
            Path::new("/Volumes/Work/Notes")
        );

        set_root(&pool, None).await.expect("clear");
        assert_eq!(
            root(&pool, Path::new("/home/someone")).await.expect("read"),
            Path::new("/home/someone/Chief")
        );
    }
}
