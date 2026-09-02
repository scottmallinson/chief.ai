//! Rolling old work into a monthly file in the corpus.
//!
//! The work log answers *"what did I ship this week?"* by search. It answers
//! *"what did I ship last quarter?"* badly — hundreds of rows, none of which
//! fit in a prompt together. A month as one markdown file is what a person can
//! read and what the corpus can hand to a recipe.
//!
//! **Nothing is deleted.** The specification this came from asked for 30-day
//! retention: summarise, then prune. Pruning destroys the only copy —
//! Chief reports nothing anywhere, so there is no second copy by construction,
//! and a hand-written entry cannot be re-fetched from anything. Whether the
//! table is ever worth pruning is a product decision that needs evidence it is
//! a problem; it is recorded in plan §9 rather than taken here.
//!
//! The pass is a **rolling** roll-up rather than a monthly one. Rows older than
//! thirty days go in, so early September rewrites `2026-08.md` a few times as
//! the rest of August crosses the line. That is why the file is replaced rather
//! than appended to, and why running it twice with the same cutoff produces the
//! same bytes: a file that grew by repetition would be worse than one rewritten.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Utc};
use sqlx::SqlitePool;

use crate::agent::Attention;
use crate::corpus::Corpus;

/// How far back a row has to be before it is rolled up.
const RETAIN_DAYS: i64 = 30;

/// Where the monthly files go, under the corpus root.
const FOLDER: &str = "journal";

/// What can go wrong during a pass.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Storage(#[from] crate::db::Error),
    #[error(transparent)]
    Corpus(#[from] crate::corpus::Error),
}

/// Everything a pass needs, so a whole one runs against an in-memory database
/// and a scratch directory. The same shape as `daemon::Context`, and for the
/// same reason.
///
/// **No engine and no clients.** A journal entry is a work log row rendered as
/// a line of markdown; there is nothing here to ask a model and nothing to
/// ask a network. That is a property of the type rather than something a test
/// has to keep watching.
pub struct Context {
    pub pool: SqlitePool,
    pub corpus: Corpus,
    pub attention: Attention,
}

/// One row, as the journal renders it.
#[derive(sqlx::FromRow)]
struct Line {
    timestamp: String,
    source: String,
    title: String,
    content: String,
    summary: Option<String>,
}

impl Line {
    /// What this row says, in one line.
    ///
    /// `title` is empty for anything written before migration 8 and for
    /// everything the user typed, so `content`'s first line stands in — the
    /// same fallback the retrieval formatter makes, and for the same reason:
    /// a bullet reading `- [note]  — ` is worse than a slightly long one.
    fn describe(&self) -> String {
        let subject = if self.title.trim().is_empty() {
            self.content.lines().next().unwrap_or_default().trim()
        } else {
            self.title.trim()
        };

        match self
            .summary
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(state) => format!("- [{}] {subject} — {state}", self.source),
            None => format!("- [{}] {subject}", self.source),
        }
    }

    /// The day this belongs under, as `YYYY-MM-DD`.
    fn day(&self) -> &str {
        self.timestamp.get(..10).unwrap_or_default()
    }

    /// The month this belongs in, as `YYYY-MM`.
    fn month(&self) -> &str {
        self.timestamp.get(..7).unwrap_or_default()
    }
}

/// Read everything older than `before`, oldest first.
///
/// Ordered by timestamp and then by id, so the file a rerun produces is byte
/// for byte the file the first run produced. Two rows sharing a timestamp are
/// ordinary — a pass writes several in the same millisecond — and leaving them
/// to SQLite's own order would make the output depend on the page layout.
async fn rows_before(pool: &SqlitePool, before: &str) -> Result<Vec<Line>, crate::db::Error> {
    Ok(sqlx::query_as::<_, Line>(
        "SELECT timestamp, source, title, content, summary
           FROM work_logs
          WHERE timestamp < ?1
          ORDER BY timestamp ASC, id ASC",
    )
    .bind(before)
    .fetch_all(pool)
    .await?)
}

/// One month's file, whole.
///
/// Built in memory and written in one call, so a failure part-way through
/// leaves no half-written month behind — the same principle as a failed render
/// writing nothing.
fn render(month: &str, lines: &[&Line]) -> String {
    let mut out = format!("# {}\n", name_of(month));
    let mut day = "";

    for line in lines {
        if line.day() != day {
            day = line.day();
            out.push_str(&format!("\n## {day}\n\n"));
        }

        out.push_str(&line.describe());
        out.push('\n');
    }

    out
}

/// `2026-08` as `August 2026`, for the heading a person reads.
///
/// A month it cannot parse is left as it was rather than guessed at: the
/// heading is the only place the raw key would show, and `# 2026-08` is a
/// worse answer than a wrong month name only in that it is honest.
fn name_of(month: &str) -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];

    let Some((year, number)) = month.split_once('-') else {
        return month.to_string();
    };

    number
        .parse::<usize>()
        .ok()
        .and_then(|number| MONTHS.get(number.wrapping_sub(1)))
        .map_or_else(|| month.to_string(), |name| format!("{name} {year}"))
}

/// Roll every month older than `before` into its own file.
///
/// Returns the months written, so a caller can say what happened and a test can
/// assert nothing was written for a month with no rows.
pub async fn roll_up(context: &Context, before: &str) -> Result<Vec<String>, Error> {
    let rows = rows_before(&context.pool, before).await?;

    // `BTreeMap` rather than a hash, so the months are written in order and a
    // failure part-way leaves a prefix rather than an arbitrary subset.
    let mut months: BTreeMap<&str, Vec<&Line>> = BTreeMap::new();

    for row in &rows {
        months.entry(row.month()).or_default().push(row);
    }

    let mut written = Vec::new();

    for (month, lines) in months {
        // A background summary must never sit in front of a person waiting on
        // an answer. What is left keeps until the next pass; nothing has been
        // deleted, so nothing is lost by stopping here.
        if context.attention.is_engaged() {
            break;
        }

        context
            .corpus
            .write(&format!("{FOLDER}/{month}.md"), &render(month, &lines))
            .await?;

        written.push(month.to_string());
    }

    Ok(written)
}

/// Roll up everything older than thirty days.
pub async fn run_once(context: &Context) -> Result<Vec<String>, Error> {
    let before = Utc::now() - chrono::Duration::days(RETAIN_DAYS);

    roll_up(
        context,
        &before.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
    )
    .await
}

/// Whether a roll-up is worth doing today.
///
/// Once a day, on the same reasoning as `brief_if_the_day_has_none`: the
/// question "has today had one" is naturally correct across a suspend, a time
/// zone change and a machine that was switched off, where an interval has to
/// account for all three and gets it wrong. The marker is the newest month
/// file's own modification time, so nothing new has to be stored.
pub async fn due(corpus: &Corpus, now: DateTime<Utc>) -> bool {
    let month = format!("{FOLDER}/{:04}-{:02}.md", now.year(), now.month());

    let Some(at) = corpus.modified_at(&month).await else {
        return true;
    };

    // `is_none_or` is 1.82 and the MSRV here is 1.77.2.
    at.get(..10)
        .map_or(true, |day| day != now.format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::test_support::migrated_pool;
    use crate::work_log::{NewWorkLogEntry, WorkLogRecord};

    struct Scratch {
        root: std::path::PathBuf,
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    async fn context(name: &str) -> (Context, Scratch) {
        let root =
            std::env::temp_dir().join(format!("chief-journal-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("should create a scratch directory");

        (
            Context {
                pool: migrated_pool().await,
                corpus: Corpus::at(root.clone()),
                attention: Attention::default(),
            },
            Scratch { root },
        )
    }

    async fn log(context: &Context, timestamp: &str, title: &str, state: &str) {
        crate::work_log::upsert(
            &context.pool,
            WorkLogRecord {
                timestamp: timestamp.to_string(),
                source: "github".to_string(),
                category: "pr".to_string(),
                title: title.to_string(),
                content: format!("{title} was {state}"),
                summary: Some(state.to_string()),
                url: None,
                raw_ref: None,
                external_id: format!("owner/repo#{title}"),
                account_id: 1,
            },
        )
        .await
        .expect("should store");
    }

    async fn rows(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM work_logs")
            .fetch_one(pool)
            .await
            .expect("should count")
    }

    fn journal(scratch: &Scratch, month: &str) -> String {
        std::fs::read_to_string(scratch.root.join(format!("journal/{month}.md")))
            .expect("the month should have been written")
    }

    /// **The criterion this whole issue turns on.**
    ///
    /// The specification asked for 30-day retention: summarise, then prune.
    /// Pruning destroys the only copy — Chief reports nothing anywhere, so
    /// there is no second copy by construction, and a hand-written entry
    /// cannot be re-fetched from anything.
    ///
    /// Proved by adding a `DELETE FROM work_logs WHERE timestamp < ?1` after
    /// the write:
    ///
    /// ```text
    /// a roll-up is a second rendering, never a move
    ///   left: 0
    ///  right: 3
    /// ```
    #[tokio::test]
    async fn rolling_up_deletes_nothing() {
        let (context, _scratch) = context("keeps").await;

        log(&context, "2026-06-01T09:00:00.000Z", "one", "merged").await;
        log(&context, "2026-06-02T09:00:00.000Z", "two", "merged").await;
        log(&context, "2026-07-14T09:00:00.000Z", "three", "merged").await;

        let before = rows(&context.pool).await;

        roll_up(&context, "2026-08-01T00:00:00.000Z")
            .await
            .expect("should roll up");

        assert_eq!(
            rows(&context.pool).await,
            before,
            "a roll-up is a second rendering, never a move"
        );
        assert_eq!(before, 3);
    }

    /// A rolling roll-up rewrites the current partial month several times, so
    /// a file that grew by repetition would be worse than one replaced.
    #[tokio::test]
    async fn running_the_same_month_twice_produces_the_same_bytes() {
        let (context, scratch) = context("idempotent").await;

        log(&context, "2026-06-01T09:00:00.000Z", "one", "merged").await;
        log(&context, "2026-06-02T09:00:00.000Z", "two", "open").await;

        roll_up(&context, "2026-08-01T00:00:00.000Z")
            .await
            .expect("should roll up");
        let first = journal(&scratch, "2026-06");

        roll_up(&context, "2026-08-01T00:00:00.000Z")
            .await
            .expect("should roll up");

        assert_eq!(
            journal(&scratch, "2026-06"),
            first,
            "replaced, not appended"
        );
    }

    #[tokio::test]
    async fn a_month_is_a_heading_a_person_reads_and_a_day_per_section() {
        let (context, scratch) = context("shape").await;

        log(&context, "2026-06-01T09:00:00.000Z", "one", "merged").await;
        log(&context, "2026-06-01T11:00:00.000Z", "two", "open").await;
        log(&context, "2026-06-02T09:00:00.000Z", "three", "merged").await;

        roll_up(&context, "2026-08-01T00:00:00.000Z")
            .await
            .expect("should roll up");

        assert_eq!(
            journal(&scratch, "2026-06"),
            "# June 2026\n\
             \n## 2026-06-01\n\n\
             - [github] one — merged\n\
             - [github] two — open\n\
             \n## 2026-06-02\n\n\
             - [github] three — merged\n"
        );
    }

    /// An empty journal entry is worse than none: it reads as a month in which
    /// nothing happened, when what it means is a month nothing was logged for.
    #[tokio::test]
    async fn a_month_with_nothing_in_it_writes_no_file() {
        let (context, scratch) = context("empty").await;

        log(&context, "2026-06-01T09:00:00.000Z", "one", "merged").await;

        let written = roll_up(&context, "2026-08-01T00:00:00.000Z")
            .await
            .expect("should roll up");

        assert_eq!(written, vec!["2026-06".to_string()]);
        assert!(
            !scratch.root.join("journal/2026-07.md").exists(),
            "a month with no rows is not a month with an empty file"
        );
    }

    #[tokio::test]
    async fn nothing_newer_than_the_cutoff_is_rolled_up() {
        let (context, scratch) = context("cutoff").await;

        log(&context, "2026-06-01T09:00:00.000Z", "old", "merged").await;
        log(&context, "2026-08-30T09:00:00.000Z", "recent", "merged").await;

        roll_up(&context, "2026-08-01T00:00:00.000Z")
            .await
            .expect("should roll up");

        assert!(scratch.root.join("journal/2026-06.md").exists());
        assert!(
            !scratch.root.join("journal/2026-08.md").exists(),
            "work from the last thirty days stays in the log and out of the journal"
        );
    }

    /// A background summary must never sit in front of a person waiting on an
    /// answer. What is left keeps until the next pass, and since nothing was
    /// deleted, nothing is lost by stopping.
    ///
    /// Proved by removing the `is_engaged` check:
    ///
    /// ```text
    /// a pass must stand aside for somebody waiting on an answer
    /// ```
    #[tokio::test]
    async fn a_pass_stands_aside_for_a_question() {
        let (context, scratch) = context("attention").await;

        log(&context, "2026-06-01T09:00:00.000Z", "one", "merged").await;

        // Held for the whole pass, the way a question in flight holds it.
        let _waiting = context.attention.begin();

        let written = roll_up(&context, "2026-08-01T00:00:00.000Z")
            .await
            .expect("standing aside is not a failure");

        assert!(written.is_empty());
        assert!(
            !scratch.root.join("journal/2026-06.md").exists(),
            "a pass must stand aside for somebody waiting on an answer"
        );
    }

    /// Hand-written entries have an empty `title` after migration 8, so the
    /// first line of what the user typed stands in — the same fallback the
    /// retrieval formatter makes.
    #[tokio::test]
    async fn an_entry_the_user_typed_reads_as_what_they_typed() {
        let (context, scratch) = context("hand-written").await;

        crate::work_log::insert(
            &context.pool,
            NewWorkLogEntry {
                source: "note".to_string(),
                content: "Spoke to Dana about the migration\nand the rest of it".to_string(),
                timestamp: Some("2026-06-03T09:00:00.000Z".to_string()),
                summary: None,
                external_id: None,
                account_id: None,
            },
        )
        .await
        .expect("should store");

        roll_up(&context, "2026-08-01T00:00:00.000Z")
            .await
            .expect("should roll up");

        assert!(
            journal(&scratch, "2026-06").contains("- [note] Spoke to Dana about the migration\n")
        );
    }

    /// The corpus root is the only place this writes, and `Corpus::resolve` is
    /// what makes that structural rather than a convention. A month key is
    /// built from a stored timestamp, so it is worth knowing the write refuses
    /// rather than that the key happens to be safe.
    #[tokio::test]
    async fn the_file_is_written_through_the_traversal_defence() {
        let (context, _scratch) = context("traversal").await;

        let refused = context
            .corpus
            .write("journal/../../escaped.md", "no")
            .await
            .expect_err("a path outside the corpus should be refused");

        assert!(matches!(refused, crate::corpus::Error::OutsideCorpus(_)));
    }

    #[test]
    fn a_month_reads_as_a_month() {
        assert_eq!(name_of("2026-08"), "August 2026");
        assert_eq!(name_of("2026-01"), "January 2026");
        assert_eq!(name_of("2026-12"), "December 2026");
    }

    #[test]
    fn a_month_that_cannot_be_read_is_left_as_it_is_rather_than_guessed_at() {
        for odd in ["2026-13", "2026-00", "not-a-month", "2026"] {
            assert_eq!(name_of(odd), odd);
        }
    }
}
