//! The chronological record of the user's work.
//!
//! Query functions take a pool so they can be exercised directly in tests; the
//! `#[tauri::command]` wrappers below are the surface the frontend sees.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tauri::{AppHandle, Runtime};

use crate::db::{self, Error};

/// How many entries a single read returns when the caller does not say.
const DEFAULT_LIMIT: i64 = 100;

/// The largest page we will build, so a runaway caller cannot exhaust memory.
const MAX_LIMIT: i64 = 1000;

/// A stored entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct WorkLogEntry {
    pub id: i64,
    /// ISO-8601, UTC.
    pub timestamp: String,
    /// Where the entry came from, e.g. `github` or `calendar`.
    pub source: String,
    /// What the entry is *about*: the repository, number and title of a pull
    /// request, or the subject of a meeting.
    ///
    /// **Selected because the interface had nothing else to show.** Migration 8
    /// added this column and `ENTRY_COLUMNS` never listed it, so every screen
    /// fell back to `summary` — which deterministic ingestion turned from a
    /// model-written sentence into a bare state word. Measured in the running
    /// app, the Today feed read `merged`, `merged`, `open` five rows deep, with
    /// nothing saying what had been merged.
    ///
    /// Empty for an entry the user typed by hand: migration 8 gave the column
    /// a default rather than inventing one, so the interface falls back to
    /// `content` for those.
    pub title: String,
    pub content: String,
    /// A one line summary of what happened. Written deterministically from
    /// what the provider said (see `ingest`), not generated.
    pub summary: Option<String>,
    /// Where the thing this describes actually is, canonical and ready to
    /// open. `None` for an entry the user wrote by hand, and for everything
    /// logged before migration 8.
    ///
    /// **Rendered from this column and never from a model.** The retrieval
    /// context deliberately omits links — a GitHub URL is 13–15 tokens, more
    /// than half a row, and a model that has never seen one cannot invent one.
    /// The interface putting it back from here is the other half of that
    /// trade.
    pub url: Option<String>,
    /// Identifies the thing this entry describes, for entries written by the
    /// background daemon. `None` for entries the user wrote themselves.
    pub external_id: Option<String>,
    /// Which connected account this came from, and `0` — the sentinel the
    /// schema uses, which `AUTOINCREMENT` never issues — for an entry the user
    /// wrote by hand or one an upgrade could attribute to nothing.
    pub account_id: i64,
}

/// The columns that make up a [`WorkLogEntry`], so every query agrees.
const ENTRY_COLUMNS: &str =
    "id, timestamp, source, title, content, summary, url, external_id, account_id";

/// A new entry. `timestamp` defaults to now, `summary` to nothing.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewWorkLogEntry {
    pub source: String,
    pub content: String,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    /// Set by the daemon so the same activity is only ever logged once.
    #[serde(default)]
    pub external_id: Option<String>,
    /// Which connected account the activity came from. `None` for an entry the
    /// user wrote by hand.
    #[serde(default)]
    pub account_id: Option<i64>,
}

/// Read entries newest first.
pub async fn fetch(pool: &SqlitePool, limit: Option<i64>) -> Result<Vec<WorkLogEntry>, Error> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let entries = sqlx::query_as::<_, WorkLogEntry>(&format!(
        "SELECT {ENTRY_COLUMNS}
         FROM work_logs
         ORDER BY timestamp DESC, id DESC
         LIMIT ?1"
    ))
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(entries)
}

/// Append an entry and return it as stored, including the values SQLite filled in.
pub async fn insert(pool: &SqlitePool, entry: NewWorkLogEntry) -> Result<WorkLogEntry, Error> {
    let stored = sqlx::query_as::<_, WorkLogEntry>(&format!(
        "INSERT INTO work_logs (timestamp, source, content, summary, external_id, account_id)
         VALUES (COALESCE(?1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')), ?2, ?3, ?4, ?5,
                 IFNULL(?6, 0))
         RETURNING {ENTRY_COLUMNS}"
    ))
    .bind(entry.timestamp)
    .bind(entry.source)
    .bind(entry.content)
    .bind(entry.summary)
    .bind(entry.external_id)
    .bind(entry.account_id)
    .fetch_one(pool)
    .await?;

    Ok(stored)
}

/// A structured entry from a connected service, ready to be written.
///
/// Separate from [`NewWorkLogEntry`], which is what the *user* submits by hand
/// and carries no `url` and no category. This is what ingestion builds: the
/// fields a feed row and a retrieved search hit need, filled in
/// deterministically from the provider's own response rather than by asking
/// the model to write prose about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkLogRecord {
    /// ISO-8601, UTC.
    pub timestamp: String,
    pub source: String,
    /// What kind of thing this is: `pr`, `issue`, `calendar`, `message`.
    pub category: String,
    /// One line, and the field search ranks most heavily.
    pub title: String,
    /// The fuller text, kept because the work log view renders it.
    pub content: String,
    /// One or two sentences. Written deterministically, not generated.
    pub summary: Option<String>,
    /// The canonical link a person would open, with tracking parameters
    /// already stripped. Never put in a prompt — see `retrieval`.
    pub url: Option<String>,
    /// Optional path to a corpus file holding the detail.
    pub raw_ref: Option<String>,
    /// The provider's identifier for the thing, which is half the dedupe key.
    pub external_id: String,
    pub account_id: i64,
}

/// Write a record, replacing the one that account already stored for it.
///
/// The conflict target has to name the *partial* index migration 3 built, so
/// the `WHERE external_id IS NOT NULL` is repeated here rather than omitted:
/// SQLite matches an upsert target against the index definition, and without
/// the predicate it finds no unique index to use and rejects the statement.
///
/// It is `DO UPDATE` where ingestion used to be `DO NOTHING`, and that is the
/// difference between a log and a mirror. A pull request that was open when it
/// was first seen and is merged by the next pass has to change; an entry the
/// daemon can never revise would leave the feed asserting something that has
/// stopped being true.
///
/// `account_id` is deliberately not updated: it is part of the key, so a row
/// that matched already has it.
///
/// **The `WHERE` on the update is what keeps a caught-up pass free.** Without
/// it `DO UPDATE` rewrites the row every time the daemon sees the same
/// unchanged pull request, which is not just a wasted statement: every write
/// fires migration 8's update trigger, so the search index is deleted and
/// reinserted for a row that did not move. `IS NOT` rather than `<>` because
/// three of these columns are nullable and `NULL <> NULL` is null, not true.
///
/// Returns whether anything actually changed, so a caller can report work done
/// rather than work considered — which is what the `Option` the daemon used to
/// read meant, and what its count still needs to mean.
pub async fn upsert(pool: &SqlitePool, record: WorkLogRecord) -> Result<bool, Error> {
    let done = sqlx::query(
        "INSERT INTO work_logs
             (timestamp, source, category, title, content, summary, url, raw_ref,
              external_id, account_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (source, account_id, external_id) WHERE external_id IS NOT NULL
         DO UPDATE SET timestamp = excluded.timestamp,
                       category  = excluded.category,
                       title     = excluded.title,
                       content   = excluded.content,
                       summary   = excluded.summary,
                       url       = excluded.url,
                       raw_ref   = excluded.raw_ref
         WHERE work_logs.title     IS NOT excluded.title
            OR work_logs.summary   IS NOT excluded.summary
            OR work_logs.url       IS NOT excluded.url
            OR work_logs.category  IS NOT excluded.category
            OR work_logs.content   IS NOT excluded.content
            OR work_logs.timestamp IS NOT excluded.timestamp
            OR work_logs.raw_ref   IS NOT excluded.raw_ref",
    )
    .bind(record.timestamp)
    .bind(record.source)
    .bind(record.category)
    .bind(record.title)
    .bind(record.content)
    .bind(record.summary)
    .bind(record.url)
    .bind(record.raw_ref)
    .bind(record.external_id)
    .bind(record.account_id)
    .execute(pool)
    .await?;

    Ok(done.rows_affected() > 0)
}

/// Forget everything one account put in the work log.
///
/// Keyed on the **account**, never on the source: migration v3 made the
/// account the unit because a person can hold a work and a personal GitHub,
/// and unlinking one of them must not take the other's history with it.
///
/// Hand-written entries carry `account_id = 0` — the sentinel for "no account",
/// which `AUTOINCREMENT` never issues — so nothing the user typed can be
/// reached by this. The search index follows through migration 8's delete
/// trigger.
///
/// Returns how many rows went, for the confirmation that has to say so before
/// anything is deleted.
pub async fn forget_account(pool: &SqlitePool, account_id: i64) -> Result<u64, Error> {
    let done = sqlx::query("DELETE FROM work_logs WHERE account_id = ?1")
        .bind(account_id)
        .execute(pool)
        .await?;

    Ok(done.rows_affected())
}

/// How many work log rows one account is holding.
pub async fn count_for_account(pool: &SqlitePool, account_id: i64) -> Result<i64, Error> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM work_logs WHERE account_id = ?1")
            .bind(account_id)
            .fetch_one(pool)
            .await?,
    )
}

/// Whether this account has already logged that thing.
///
/// Test-only since ingestion moved to [`upsert`], which asks the index the
/// same question and needs no answer beforehand. Kept because it is what the
/// daemon's per-account assertions want to say, and writing the `SELECT` out
/// three times would say it less clearly.
#[cfg(test)]
pub async fn has_logged(
    pool: &SqlitePool,
    source: &str,
    account_id: i64,
    external_id: &str,
) -> Result<bool, Error> {
    let existing = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM work_logs
          WHERE source = ?1 AND account_id = ?2 AND external_id = ?3
          LIMIT 1",
    )
    .bind(source)
    .bind(account_id)
    .bind(external_id)
    .fetch_optional(pool)
    .await?;

    Ok(existing.is_some())
}

/// Read the work log, newest first.
#[tauri::command]
pub async fn list_work_logs<R: Runtime>(
    app: AppHandle<R>,
    limit: Option<i64>,
) -> Result<Vec<WorkLogEntry>, Error> {
    let pool = db::pool(&app).await?;
    fetch(&pool, limit).await
}

/// Append an entry to the work log.
#[tauri::command]
pub async fn create_work_log<R: Runtime>(
    app: AppHandle<R>,
    entry: NewWorkLogEntry,
) -> Result<WorkLogEntry, Error> {
    let pool = db::pool(&app).await?;
    insert(&pool, entry).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::migrated_pool;

    fn entry(source: &str, content: &str) -> NewWorkLogEntry {
        NewWorkLogEntry {
            source: source.to_string(),
            content: content.to_string(),
            timestamp: None,
            summary: None,
            external_id: None,
            account_id: None,
        }
    }

    #[tokio::test]
    async fn stores_and_reads_back_an_entry() {
        let pool = migrated_pool().await;

        let stored = insert(&pool, entry("github", "Merged PR #4"))
            .await
            .expect("insert should succeed");

        assert_eq!(stored.source, "github");
        assert_eq!(stored.content, "Merged PR #4");
        assert_eq!(stored.summary, None);

        let entries = fetch(&pool, None).await.expect("read should succeed");
        assert_eq!(entries, vec![stored]);
    }

    #[tokio::test]
    async fn defaults_the_timestamp_to_now() {
        let pool = migrated_pool().await;

        let stored = insert(&pool, entry("calendar", "Standup"))
            .await
            .expect("insert should succeed");

        assert!(
            stored.timestamp.contains('T') && stored.timestamp.ends_with('Z'),
            "expected an ISO-8601 timestamp, got {}",
            stored.timestamp
        );
    }

    #[tokio::test]
    async fn keeps_a_caller_supplied_timestamp() {
        let pool = migrated_pool().await;

        let stored = insert(
            &pool,
            NewWorkLogEntry {
                timestamp: Some("2026-08-19T09:00:00.000Z".to_string()),
                summary: Some("Shipped the shell".to_string()),
                ..entry("github", "Opened PR #4")
            },
        )
        .await
        .expect("insert should succeed");

        assert_eq!(stored.timestamp, "2026-08-19T09:00:00.000Z");
        assert_eq!(stored.summary.as_deref(), Some("Shipped the shell"));
    }

    #[tokio::test]
    async fn says_which_account_an_entry_came_from() {
        // Attribution is written on every insert; without it on the way out,
        // whose work an entry records is only visible to raw SQL.
        let pool = migrated_pool().await;

        let attributed = insert(
            &pool,
            NewWorkLogEntry {
                account_id: Some(7),
                ..entry("github", "Merged PR #4")
            },
        )
        .await
        .expect("insert should succeed");
        let by_hand = insert(&pool, entry("manual", "Wrote something down"))
            .await
            .expect("insert should succeed");

        assert_eq!(attributed.account_id, 7);
        assert_eq!(
            by_hand.account_id, 0,
            "an entry the user wrote carries the sentinel, not an account"
        );

        let read = fetch(&pool, None).await.expect("read should succeed");
        assert_eq!(
            read.iter()
                .map(|entry| entry.account_id)
                .collect::<Vec<_>>(),
            [0, 7],
            "reading the log back should say the same"
        );
    }

    #[tokio::test]
    async fn returns_newest_entries_first() {
        let pool = migrated_pool().await;

        for (timestamp, content) in [
            ("2026-08-17T09:00:00.000Z", "oldest"),
            ("2026-08-19T09:00:00.000Z", "newest"),
            ("2026-08-18T09:00:00.000Z", "middle"),
        ] {
            insert(
                &pool,
                NewWorkLogEntry {
                    timestamp: Some(timestamp.to_string()),
                    ..entry("github", content)
                },
            )
            .await
            .expect("insert should succeed");
        }

        let entries = fetch(&pool, None).await.expect("read should succeed");
        let order: Vec<&str> = entries.iter().map(|e| e.content.as_str()).collect();

        assert_eq!(order, ["newest", "middle", "oldest"]);
    }

    #[tokio::test]
    async fn clamps_the_page_size() {
        let pool = migrated_pool().await;

        for index in 0..3 {
            insert(&pool, entry("github", &format!("entry {index}")))
                .await
                .expect("insert should succeed");
        }

        let entries = fetch(&pool, Some(2)).await.expect("read should succeed");
        assert_eq!(entries.len(), 2);

        // A nonsensical limit must not return everything or panic.
        let entries = fetch(&pool, Some(0)).await.expect("read should succeed");
        assert_eq!(entries.len(), 1);
    }
}

#[cfg(test)]
mod deduplication_tests {
    use super::*;
    use crate::db::test_support::migrated_pool;

    fn record(account_id: i64, title: &str, state: &str) -> WorkLogRecord {
        WorkLogRecord {
            timestamp: "2026-09-01T09:00:00.000Z".to_string(),
            source: "github".to_string(),
            category: "pr".to_string(),
            title: title.to_string(),
            content: format!("{title} is {state}"),
            summary: Some(state.to_string()),
            url: Some("https://github.com/o/r/pull/7".to_string()),
            raw_ref: None,
            external_id: "owner/repo#7".to_string(),
            account_id,
        }
    }

    async fn titles(pool: &SqlitePool) -> Vec<String> {
        sqlx::query_scalar("SELECT title FROM work_logs ORDER BY id")
            .fetch_all(pool)
            .await
            .expect("should read")
    }

    /// The reason ingestion upserts rather than inserting-and-skipping: a pull
    /// request that was open when it was first seen and is merged by the next
    /// pass has to change, or the feed keeps asserting something that stopped
    /// being true.
    #[tokio::test]
    async fn upserting_the_same_thing_revises_it_rather_than_repeating_it() {
        let pool = migrated_pool().await;

        upsert(&pool, record(1, "Add PKCE auth handler", "open"))
            .await
            .expect("should store");
        upsert(&pool, record(1, "Add PKCE auth handler", "merged"))
            .await
            .expect("should store");

        assert_eq!(titles(&pool).await.len(), 1, "one row, not two");

        let summary: Option<String> = sqlx::query_scalar("SELECT summary FROM work_logs")
            .fetch_one(&pool)
            .await
            .expect("should read");

        assert_eq!(
            summary.as_deref(),
            Some("merged"),
            "the second call's values should win"
        );
    }

    /// Two people's accounts legitimately see the same pull request number.
    ///
    /// This is why the dedupe index carries `account_id` and why the
    /// specification's `external_id TEXT UNIQUE NOT NULL` would have been
    /// wrong: it would have made the second account's copy unstorable.
    #[tokio::test]
    async fn two_accounts_can_each_hold_the_same_item() {
        let pool = migrated_pool().await;

        upsert(&pool, record(1, "Add PKCE auth handler", "open"))
            .await
            .expect("should store");
        upsert(&pool, record(2, "Add PKCE auth handler", "open"))
            .await
            .expect("should store");

        assert_eq!(
            titles(&pool).await.len(),
            2,
            "the same external id under a different account is different work"
        );
    }

    /// The index carries `source` as well as `account_id`, and this is the
    /// clause that proves it. GitHub numbers pull requests from 1 and so does
    /// every other tracker: `owner/repo#7` from a calendar and `owner/repo#7`
    /// from GitHub are two things, and collapsing them would silently lose one.
    ///
    /// Proved by binding a constant `"github"` in place of `record.source`,
    /// which no other test in this module notices:
    ///
    /// ```text
    /// the same external id under another source is different work
    ///   left: 1
    ///  right: 2
    /// ```
    #[tokio::test]
    async fn the_same_identifier_from_another_source_is_another_thing() {
        let pool = migrated_pool().await;

        upsert(&pool, record(1, "Add PKCE auth handler", "open"))
            .await
            .expect("should store");

        let elsewhere = WorkLogRecord {
            source: "calendar".to_string(),
            category: "calendar".to_string(),
            ..record(1, "Sprint review", "attended")
        };
        upsert(&pool, elsewhere).await.expect("should store");

        assert_eq!(
            titles(&pool).await.len(),
            2,
            "the same external id under another source is different work"
        );
    }

    /// The other half of the same index: entries the user typed have no
    /// external id, and SQLite does not consider two NULLs equal, so the
    /// partial index exempts them and repetition is allowed.
    #[tokio::test]
    async fn two_hand_written_entries_never_collide() {
        let pool = migrated_pool().await;

        for _ in 0..2 {
            insert(
                &pool,
                NewWorkLogEntry {
                    source: "note".to_string(),
                    content: "Spoke to Dana".to_string(),
                    timestamp: None,
                    summary: None,
                    external_id: None,
                    account_id: None,
                },
            )
            .await
            .expect("a hand-written entry is never a duplicate of another");
        }

        assert_eq!(titles(&pool).await.len(), 2);
    }

    /// An upsert has to keep the index in step, which it only does if the
    /// update trigger fires — the reason the `DO UPDATE` writes through the
    /// table rather than anywhere else.
    ///
    /// **This is the guard on migration 8's update trigger**, and it is the
    /// only one: removing the `'delete'` half of that trigger leaves a full
    /// FTS5 `'integrity-check'` passing, and fails here. So a search index
    /// test that lives beside the schema proves less than one that asks the
    /// question a reader would — which is why the integrity check that used to
    /// be in `db.rs` was taken out rather than kept for reassurance.
    ///
    /// ```text
    /// the superseded title should be out of the index
    ///   left: 1
    ///  right: 0
    /// ```
    #[tokio::test]
    async fn revising_an_entry_revises_what_search_will_find() {
        let pool = migrated_pool().await;

        upsert(&pool, record(1, "Add PKCE auth handler", "open"))
            .await
            .expect("should store");
        upsert(&pool, record(1, "Add device flow handler", "merged"))
            .await
            .expect("should store");

        let stale: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM work_logs_fts WHERE work_logs_fts MATCH '\"pkce\"'",
        )
        .fetch_one(&pool)
        .await
        .expect("the index should be searchable");

        assert_eq!(stale, 0, "the superseded title should be out of the index");
    }
}
