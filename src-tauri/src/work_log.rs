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
    /// A one-line achievement, written by the local model in a later step.
    pub summary: Option<String>,
}

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
}

/// Read entries newest first.
pub async fn fetch(pool: &SqlitePool, limit: Option<i64>) -> Result<Vec<WorkLogEntry>, Error> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let entries = sqlx::query_as::<_, WorkLogEntry>(
        "SELECT id, timestamp, source, content, summary
         FROM work_logs
         ORDER BY timestamp DESC, id DESC
         LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(entries)
}

/// Append an entry and return it as stored, including the values SQLite filled in.
pub async fn insert(pool: &SqlitePool, entry: NewWorkLogEntry) -> Result<WorkLogEntry, Error> {
    let stored = sqlx::query_as::<_, WorkLogEntry>(
        "INSERT INTO work_logs (timestamp, source, content, summary)
         VALUES (COALESCE(?1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')), ?2, ?3, ?4)
         RETURNING id, timestamp, source, content, summary",
    )
    .bind(entry.timestamp)
    .bind(entry.source)
    .bind(entry.content)
    .bind(entry.summary)
    .fetch_one(pool)
    .await?;

    Ok(stored)
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
