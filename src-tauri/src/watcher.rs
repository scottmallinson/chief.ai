//! Noticing that the user edited the corpus in their own editor.
//!
//! The corpus is a folder Chief invites people to open, so the index has to
//! keep up with a text editor rather than only with Chief's own writes. Every
//! command that touches the corpus already reindexes, which is why the index is
//! correct *whenever Chief looks* — this closes the gap between the user saving
//! a file and something else happening to trigger a scan.
//!
//! **Why a debounce, and why that is the whole of it.** Saving one file in one
//! editor produces several filesystem events: a write, a rename from a
//! temporary file, an attribute change. Reindexing on each is a walk of the
//! folder per keystroke-adjacent event.
//!
//! It is worth being exact about the storm the plan warns of, because the fix
//! it implies is not the one that is needed. Chief writes into this directory
//! too — briefs, and now proposed actions — and a watcher that reacted to its
//! own writes was expected to loop. **It cannot.** Reindexing writes to SQLite
//! and never to the corpus, so there is no edge from a reindex back to a
//! filesystem event, and the cycle has nothing to close it. What Chief's own
//! writes cost is one reindex per burst, the same as anybody else's, and the
//! debounce is what makes that one rather than several.

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Runtime};

/// How long the folder has to be quiet before the index is rebuilt.
///
/// Long enough to cover an editor's write-rename-chmod dance and a person
/// holding down a key; short enough that a file saved and then asked about
/// reads as immediate. `recipe::edited_by_hand` uses two seconds for the
/// neighbouring question of whether a file changed since Chief wrote it, and
/// this being shorter is deliberate: that one is deciding whether to overwrite
/// somebody's work, and this one is deciding when to run a cheap query.
const QUIET: Duration = Duration::from_millis(750);

/// How often the drain loop looks at the clock while nothing is arriving.
const TICK: Duration = Duration::from_millis(250);

/// Collapses a burst of filesystem events into one reindex.
///
/// Pure, and driven by an `Instant` passed in rather than read from the clock,
/// so the behaviour can be tested without sleeping through it.
#[derive(Debug)]
pub struct Debounce {
    window: Duration,
    /// When the most recent event arrived, if anything is waiting.
    latest: Option<Instant>,
}

impl Debounce {
    #[must_use]
    pub const fn new(window: Duration) -> Self {
        Self {
            window,
            latest: None,
        }
    }

    /// An event arrived. Pushes the deadline out rather than starting a timer:
    /// a burst that keeps arriving keeps deferring, which is the point.
    pub fn note(&mut self, at: Instant) {
        self.latest = Some(at);
    }

    /// Has it been quiet long enough to rebuild the index?
    #[must_use]
    pub fn is_due(&self, now: Instant) -> bool {
        self.latest
            .is_some_and(|latest| now.duration_since(latest) >= self.window)
    }

    /// Take the pending work, if it is due. Returns whether there was any.
    pub fn take(&mut self, now: Instant) -> bool {
        if !self.is_due(now) {
            return false;
        }

        self.latest = None;
        true
    }
}

/// Is this a path the index cares about?
///
/// Markdown only, matching [`crate::corpus::Corpus::list`] — whatever else the
/// user keeps beside their notes is not Chief's to read, and an editor's swap
/// files would otherwise be a reindex each.
#[must_use]
pub fn is_indexed(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

/// Watch the corpus and keep the index in step with it.
///
/// Returns immediately. The watcher lives for as long as the app does: it is
/// moved into the task that drains it, and both end when the process does.
///
/// Failing to watch is not fatal and never reported twice. The index is still
/// rebuilt by every command that touches the corpus, so a machine where the
/// watch could not be established is exactly as correct as one from before this
/// module existed — it just learns about an outside edit later.
pub fn spawn<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();

    tauri::async_runtime::spawn(async move {
        let Ok(context) = crate::recipe::context(&app).await else {
            return;
        };

        let root = context.corpus.root().to_path_buf();

        // A std channel rather than a tokio one: `notify` calls the handler
        // from its own thread, which is not inside the runtime.
        let (events, arrivals) = mpsc::channel::<()>();

        let watcher =
            match notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                let Ok(event) = result else {
                    return;
                };

                if event.paths.iter().any(|path| is_indexed(path)) {
                    // A closed channel means the drain task has gone, which means
                    // the app is going. Nothing to report.
                    let _ = events.send(());
                }
            }) {
                Ok(watcher) => watcher,
                Err(error) => {
                    eprintln!("could not watch the corpus: {error}");
                    return;
                }
            };

        let mut watcher = watcher;
        {
            use notify::Watcher as _;

            if let Err(error) = watcher.watch(&root, notify::RecursiveMode::Recursive) {
                eprintln!("could not watch {}: {error}", root.display());
                return;
            }
        }

        // Held for the life of the task: dropping it stops the watch.
        let _watcher = watcher;
        let mut debounce = Debounce::new(QUIET);

        loop {
            match arrivals.recv_timeout(TICK) {
                Ok(()) => debounce.note(Instant::now()),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                // The sender has gone, so nothing more will arrive.
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }

            if !debounce.take(Instant::now()) {
                continue;
            }

            let Ok(context) = crate::recipe::context(&app).await else {
                continue;
            };

            if let Err(error) = crate::corpus::reindex(&context.pool, &context.corpus).await {
                eprintln!("could not reindex the corpus: {error}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, millis: u64) -> Instant {
        start + Duration::from_millis(millis)
    }

    #[test]
    fn has_nothing_to_do_until_something_happens() {
        let mut debounce = Debounce::new(QUIET);

        assert!(!debounce.is_due(Instant::now()));
        assert!(!debounce.take(Instant::now()), "nothing to index");
    }

    #[test]
    fn waits_for_the_folder_to_go_quiet() {
        let start = Instant::now();
        let mut debounce = Debounce::new(QUIET);

        debounce.note(start);

        assert!(!debounce.is_due(at(start, 100)), "still within the window");
        assert!(debounce.is_due(at(start, 800)), "quiet for long enough");
    }

    /// The behaviour the whole module exists for.
    #[test]
    fn collapses_a_burst_into_one_reindex() {
        let start = Instant::now();
        let mut debounce = Debounce::new(QUIET);

        // An editor saving one file: write, rename, chmod, all within a moment.
        for millis in [0, 40, 90, 120] {
            debounce.note(at(start, millis));
        }

        assert!(
            !debounce.take(at(start, 500)),
            "a burst still arriving must not be indexed mid-flight"
        );
        assert!(
            debounce.take(at(start, 900)),
            "and once it stops, exactly one reindex is due"
        );
        assert!(
            !debounce.take(at(start, 5_000)),
            "and only one: the second look has nothing to do"
        );
    }

    #[test]
    fn keeps_deferring_while_events_keep_arriving() {
        let start = Instant::now();
        let mut debounce = Debounce::new(QUIET);

        debounce.note(start);
        // Somebody holding down a key: an event every 300ms for two seconds.
        for millis in (300..=2_100).step_by(300) {
            assert!(
                !debounce.take(at(start, millis)),
                "nothing is due while the folder is still busy at {millis}ms"
            );
            debounce.note(at(start, millis));
        }

        assert!(
            debounce.take(at(start, 3_000)),
            "one reindex, once they stop"
        );
    }

    #[test]
    fn starts_again_after_it_has_indexed() {
        let start = Instant::now();
        let mut debounce = Debounce::new(QUIET);

        debounce.note(start);
        assert!(debounce.take(at(start, 800)));

        debounce.note(at(start, 1_000));
        assert!(
            debounce.take(at(start, 1_800)),
            "a later edit is a later index"
        );
    }

    #[test]
    fn watches_markdown_and_ignores_everything_else() {
        assert!(is_indexed(Path::new("/corpus/briefs/2026-08-29.md")));
        assert!(is_indexed(Path::new("/corpus/NOTES.MD")));
    }

    #[test]
    fn ignores_the_files_an_editor_leaves_lying_around() {
        // Each of these would otherwise be a reindex of its own.
        for path in [
            "/corpus/.briefs.md.swp",
            "/corpus/briefs/2026-08-29.md~",
            "/corpus/.DS_Store",
            "/corpus/briefs/4913",
        ] {
            assert!(!is_indexed(Path::new(path)), "{path} should be ignored");
        }
    }
}

#[cfg(test)]
mod indexing {
    //! What the watcher is for, tested against the reindex it actually calls.
    //!
    //! The watcher itself is not driven here: `notify` needs a real filesystem
    //! event from a real OS, and a test that sleeps waiting for one is a test
    //! that fails on a loaded CI runner. The debounce above is the part with
    //! logic in it, and this is the part it calls.

    use crate::corpus::{reindex, Corpus};
    use crate::db::test_support::migrated_pool;

    struct Scratch(std::path::PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn corpus_at(name: &str) -> (Corpus, Scratch) {
        let root =
            std::env::temp_dir().join(format!("chief-watcher-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        let corpus = Corpus::at(root.clone());
        corpus
            .ensure_shape()
            .await
            .expect("should create the shape");

        (corpus, Scratch(root))
    }

    /// Written the way a text editor writes it: straight to disk, past Chief.
    fn write_behind_chiefs_back(corpus: &Corpus, relative: &str, contents: &str) {
        let path = corpus.root().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("should create");
        }
        std::fs::write(path, contents).expect("should write");
    }

    #[tokio::test]
    async fn indexes_a_file_the_user_wrote_in_their_own_editor() {
        let pool = migrated_pool().await;
        let (corpus, _scratch) = corpus_at("outside").await;

        write_behind_chiefs_back(&corpus, "journal/monday.md", "# Monday\n\nIt went fine.");

        let indexed = reindex(&pool, &corpus).await.expect("should index");

        assert!(
            indexed
                .iter()
                .any(|entry| entry.path == "journal/monday.md"),
            "a file Chief never wrote still belongs in the index: {indexed:?}"
        );
    }

    #[tokio::test]
    async fn forgets_a_file_that_has_been_deleted() {
        let pool = migrated_pool().await;
        let (corpus, _scratch) = corpus_at("deleted").await;

        write_behind_chiefs_back(&corpus, "journal/monday.md", "# Monday");
        reindex(&pool, &corpus).await.expect("should index");

        std::fs::remove_file(corpus.root().join("journal/monday.md")).expect("should remove");

        let indexed = reindex(&pool, &corpus).await.expect("should index");

        assert!(
            !indexed
                .iter()
                .any(|entry| entry.path == "journal/monday.md"),
            "deleting the file is how somebody says no: {indexed:?}"
        );
    }

    /// Why a self-write cannot storm, asserted rather than reasoned about.
    ///
    /// The fear is a cycle: Chief writes, the watcher notices, the reindex
    /// writes, the watcher notices again. The cycle needs an edge from
    /// reindexing back to the filesystem, and this is the test that there is
    /// none — reindexing goes to SQLite and leaves the folder byte-identical,
    /// so the loop has nothing to close it however many events arrive.
    #[tokio::test]
    async fn reindexing_does_not_touch_the_corpus_at_all() {
        let pool = migrated_pool().await;
        let (corpus, _scratch) = corpus_at("noloop").await;

        write_behind_chiefs_back(&corpus, "journal/monday.md", "# Monday");

        let before = corpus.list().await.expect("should list");

        for _ in 0..3 {
            reindex(&pool, &corpus).await.expect("should index");
        }

        let after = corpus.list().await.expect("should list");

        assert_eq!(
            before, after,
            "reindexing must leave the folder exactly as it found it"
        );
    }

    #[tokio::test]
    async fn stays_correct_when_it_is_run_repeatedly() {
        let pool = migrated_pool().await;
        let (corpus, _scratch) = corpus_at("repeat").await;

        write_behind_chiefs_back(&corpus, "journal/monday.md", "# Monday");

        let once = reindex(&pool, &corpus).await.expect("should index");
        let twice = reindex(&pool, &corpus).await.expect("should index");

        assert_eq!(once, twice, "the index is a mirror, not an accumulation");
    }
}
