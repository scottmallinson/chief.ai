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
    pub content: String,
    /// A one-line achievement, written by the local model.
    pub summary: Option<String>,
    /// Identifies the thing this entry describes, for entries written by the
    /// background daemon. `None` for entries the user wrote themselves.
    pub external_id: Option<String>,
    /// Which connected account this came from, and `0` — the sentinel the
    /// schema uses, which `AUTOINCREMENT` never issues — for an entry the user
    /// wrote by hand or one an upgrade could attribute to nothing.
    pub account_id: i64,
}

/// The columns that make up a [`WorkLogEntry`], so every query agrees.
const ENTRY_COLUMNS: &str = "id, timestamp, source, content, summary, external_id, account_id";

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

/// Append an entry unless this account has already logged that thing.
///
/// Returns the stored entry, or `None` when it was already there. This is what
/// lets the daemon run as often as it likes.
pub async fn insert_new(
    pool: &SqlitePool,
    entry: NewWorkLogEntry,
) -> Result<Option<WorkLogEntry>, Error> {
    let stored = sqlx::query_as::<_, WorkLogEntry>(&format!(
        "INSERT INTO work_logs (timestamp, source, content, summary, external_id, account_id)
         VALUES (COALESCE(?1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')), ?2, ?3, ?4, ?5,
                 IFNULL(?6, 0))
         ON CONFLICT (source, account_id, external_id) WHERE external_id IS NOT NULL DO NOTHING
         RETURNING {ENTRY_COLUMNS}"
    ))
    .bind(entry.timestamp)
    .bind(entry.source)
    .bind(entry.content)
    .bind(entry.summary)
    .bind(entry.external_id)
    .bind(entry.account_id)
    .fetch_optional(pool)
    .await?;

    Ok(stored)
}

/// Whether this account has already logged that thing.
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

    fn from_github(external_id: &str, content: &str) -> NewWorkLogEntry {
        NewWorkLogEntry {
            source: "github".to_string(),
            content: content.to_string(),
            timestamp: None,
            summary: Some("Shipped something".to_string()),
            external_id: Some(external_id.to_string()),
            account_id: None,
        }
    }

    #[tokio::test]
    async fn logs_an_entry_the_first_time() {
        let pool = migrated_pool().await;

        let stored = insert_new(&pool, from_github("pr-12", "Merged PR #12"))
            .await
            .expect("insert should succeed");

        assert!(stored.is_some(), "the first sighting should be logged");
    }

    #[tokio::test]
    async fn does_not_log_the_same_activity_twice() {
        let pool = migrated_pool().await;

        insert_new(&pool, from_github("pr-12", "Merged PR #12"))
            .await
            .expect("insert should succeed");
        let again = insert_new(&pool, from_github("pr-12", "Merged PR #12"))
            .await
            .expect("insert should succeed");

        assert!(again.is_none(), "the second sighting should be skipped");
        assert_eq!(fetch(&pool, None).await.expect("read").len(), 1);
    }

    #[tokio::test]
    async fn tells_two_sources_apart() {
        let pool = migrated_pool().await;

        insert_new(&pool, from_github("12", "Merged PR #12"))
            .await
            .expect("insert should succeed");

        let calendar = NewWorkLogEntry {
            source: "calendar".to_string(),
            ..from_github("12", "Attended event 12")
        };
        let stored = insert_new(&pool, calendar)
            .await
            .expect("insert should succeed");

        assert!(
            stored.is_some(),
            "the same id from another source is a different thing"
        );
    }

    #[tokio::test]
    async fn hand_written_entries_never_collide() {
        let pool = migrated_pool().await;

        for _ in 0..2 {
            let stored = insert_new(
                &pool,
                NewWorkLogEntry {
                    source: "manual".to_string(),
                    content: "Wrote something down".to_string(),
                    timestamp: None,
                    summary: None,
                    external_id: None,
                    account_id: None,
                },
            )
            .await
            .expect("insert should succeed");

            assert!(stored.is_some(), "entries without an id are always kept");
        }

        assert_eq!(fetch(&pool, None).await.expect("read").len(), 2);
    }

    #[tokio::test]
    async fn two_accounts_may_log_the_same_identifier() {
        let pool = migrated_pool().await;

        // Two people can merge the same pull request number in the same
        // repository — on two different accounts, it is two people's work.
        let entry = |account_id| NewWorkLogEntry {
            source: "github".to_string(),
            content: "Merged PR #7".to_string(),
            timestamp: None,
            summary: None,
            external_id: Some("owner/repo#7".to_string()),
            account_id: Some(account_id),
        };

        assert!(
            insert_new(&pool, entry(1))
                .await
                .expect("should insert")
                .is_some(),
            "the first account should log it"
        );
        assert!(
            insert_new(&pool, entry(2))
                .await
                .expect("should insert")
                .is_some(),
            "a different account is different work"
        );
        assert!(
            insert_new(&pool, entry(1))
                .await
                .expect("should insert")
                .is_none(),
            "the same account should not log it twice"
        );

        assert!(has_logged(&pool, "github", 1, "owner/repo#7")
            .await
            .expect("should read"));
        assert!(!has_logged(&pool, "github", 3, "owner/repo#7")
            .await
            .expect("should read"));
    }
}
