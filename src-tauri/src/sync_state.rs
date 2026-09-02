//! How fresh each connected account is, and whether it still works.
//!
//! This exists because of what D9 costs. Once a read question is answered from
//! `work_logs` rather than from the service it came from, the answer is only
//! as good as the last sync — and staleness is invisible in a way a spinner is
//! not. Chief used to be wrong slowly; it can now be stale quickly. Recording
//! the state is what lets the interface say so, which is the only thing that
//! makes the trade honest.
//!
//! Keyed on the **account**, not the service. Migration 3 made that the unit
//! because a person can hold a work and a personal GitHub, and keying on the
//! service would have one account's failure overwrite the other's state.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tauri::{AppHandle, Runtime};

use crate::db::Error;

/// What Chief last observed about an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// The last pass read it successfully.
    Ok,
    /// A pass is running now.
    Syncing,
    /// The credential is gone — revoked, expired past renewal, or consent
    /// withdrawn. **Only a rejected credential**, never a failure to reach the
    /// host: conflating the two is how this becomes a false alarm every time
    /// somebody closes their laptop, which is why the two have separate
    /// variants rather than one "broken".
    AuthRequired,
    /// Something else went wrong, and trying again later is the right response.
    Error,
}

impl Status {
    /// How the status is stored. Written out rather than derived, so renaming
    /// a variant cannot silently change what is already in the database.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "Ok",
            Self::Syncing => "Syncing",
            Self::AuthRequired => "AuthRequired",
            Self::Error => "Error",
        }
    }

    /// Read a stored status.
    ///
    /// An unrecognised value becomes [`Status::Error`] rather than raising: a
    /// row written by a newer release than the one reading it is a reason to
    /// show something cautious, not to fail the whole settings screen.
    fn from_stored(value: &str) -> Self {
        match value {
            "Ok" => Self::Ok,
            "Syncing" => Self::Syncing,
            "AuthRequired" => Self::AuthRequired,
            _ => Self::Error,
        }
    }
}

/// One account's freshness, as the interface needs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    pub account_id: i64,
    pub source: String,
    pub status: Status,
    /// When the last **successful** read finished, ISO-8601 UTC. `None` until
    /// one has, which is a legitimate state and reads as "never synced" rather
    /// than as a failure.
    pub last_synced_at: Option<String>,
    /// Why it is not `Ok`, in words a person can act on.
    ///
    /// **Never a credential.** Plan §9 records the OAuth token, the pasted
    /// Linear key and the calendar subscription address as three things that
    /// are never logged and never put in an error; this column is written from
    /// error types that are built not to carry them.
    pub error_message: Option<String>,
}

/// The row shape, kept apart from [`SyncState`] because `status` is text in
/// SQLite and an enum above it.
#[derive(sqlx::FromRow)]
struct Row {
    account_id: i64,
    source: String,
    status: String,
    last_synced_at: Option<String>,
    error_message: Option<String>,
}

impl From<Row> for SyncState {
    fn from(row: Row) -> Self {
        Self {
            account_id: row.account_id,
            source: row.source,
            status: Status::from_stored(&row.status),
            last_synced_at: row.last_synced_at,
            error_message: row.error_message,
        }
    }
}

/// Record what just happened to an account.
///
/// `last_synced_at` moves **only on success**, and is otherwise carried across
/// from whatever was there before. A failed pass must not look like a fresh
/// one, and it must not erase the fact that the data on disk came from
/// somewhere at some point — "last read at 09:00, failing since" is the useful
/// thing to be able to say, and overwriting the timestamp on failure would
/// lose half of it.
pub async fn record(
    pool: &SqlitePool,
    account_id: i64,
    source: &str,
    status: Status,
    error_message: Option<&str>,
) -> Result<(), Error> {
    let synced = matches!(status, Status::Ok);

    sqlx::query(
        "INSERT INTO sync_state (account_id, source, status, last_synced_at, error_message)
         VALUES (?1, ?2, ?3,
                 CASE WHEN ?4 THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') ELSE NULL END,
                 ?5)
         ON CONFLICT (account_id) DO UPDATE
            SET source         = excluded.source,
                status         = excluded.status,
                error_message  = excluded.error_message,
                last_synced_at = CASE WHEN ?4 THEN excluded.last_synced_at
                                      ELSE sync_state.last_synced_at END",
    )
    .bind(account_id)
    .bind(source)
    .bind(status.as_str())
    .bind(synced)
    .bind(error_message)
    .execute(pool)
    .await?;

    Ok(())
}

/// Every account's state, oldest read first — so the interface can show the
/// staleness that matters without sorting it again.
pub async fn all(pool: &SqlitePool) -> Result<Vec<SyncState>, Error> {
    let rows = sqlx::query_as::<_, Row>(
        "SELECT account_id, source, status, last_synced_at, error_message
           FROM sync_state
          ORDER BY last_synced_at IS NULL DESC, last_synced_at ASC, account_id ASC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(SyncState::from).collect())
}

/// One account's state, or nothing if it has never been read.
// Read by the purge that unlinking performs (DLE-4, REC-47), which is the only
// thing that has a reason to ask about one account rather than all of them, or
// to throw a state away.
#[allow(dead_code)]
pub async fn for_account(pool: &SqlitePool, account_id: i64) -> Result<Option<SyncState>, Error> {
    let row = sqlx::query_as::<_, Row>(
        "SELECT account_id, source, status, last_synced_at, error_message
           FROM sync_state
          WHERE account_id = ?1",
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(SyncState::from))
}

/// Forget an account's state, for when the account itself is forgotten.
// Read by the purge that unlinking performs (DLE-4, REC-47), which is the only
// thing that has a reason to ask about one account rather than all of them, or
// to throw a state away.
#[allow(dead_code)]
pub async fn forget(pool: &SqlitePool, account_id: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM sync_state WHERE account_id = ?1")
        .bind(account_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Every account's freshness, for the settings screen and the header.
///
/// An account that has never been read has **no row here at all**, and that is
/// a legitimate state rather than a failure: a fresh install, or an account
/// connected between one pass and the next. The command returns what exists
/// and the interface says "Never synced" for the rest — filling in a
/// placeholder row here would mean inventing a status the daemon never
/// recorded.
#[tauri::command]
pub async fn sync_status<R: Runtime>(app: AppHandle<R>) -> Result<Vec<SyncState>, String> {
    all(&crate::db::pool(&app)
        .await
        .map_err(|error| error.to_string())?)
    .await
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::migrated_pool;

    #[tokio::test]
    async fn round_trips_every_status() {
        let pool = migrated_pool().await;

        for (account_id, status) in [
            (1, Status::Ok),
            (2, Status::Syncing),
            (3, Status::AuthRequired),
            (4, Status::Error),
        ] {
            record(&pool, account_id, "github", status, None)
                .await
                .expect("should record");

            let stored = for_account(&pool, account_id)
                .await
                .expect("should read")
                .expect("should be there");

            assert_eq!(stored.status, status);
            assert_eq!(stored.source, "github");
        }
    }

    #[tokio::test]
    async fn a_never_read_account_has_no_state_at_all() {
        let pool = migrated_pool().await;

        assert!(
            for_account(&pool, 1).await.expect("should read").is_none(),
            "never synced is the absence of a row, not a row saying so"
        );
        assert!(all(&pool).await.expect("should read").is_empty());
    }

    #[tokio::test]
    async fn one_account_failing_leaves_the_others_alone() {
        let pool = migrated_pool().await;

        record(&pool, 1, "github", Status::Ok, None)
            .await
            .expect("should record");
        record(
            &pool,
            2,
            "github",
            Status::AuthRequired,
            Some("sign in again"),
        )
        .await
        .expect("should record");

        let first = for_account(&pool, 1)
            .await
            .expect("should read")
            .expect("should be there");

        assert_eq!(
            first.status,
            Status::Ok,
            "keyed on the account, so the same service failing elsewhere must not reach it"
        );
    }

    /// A failure must not look like a fresh read.
    #[tokio::test]
    async fn a_failure_keeps_the_last_successful_time() {
        let pool = migrated_pool().await;

        record(&pool, 1, "github", Status::Ok, None)
            .await
            .expect("should record");

        let synced_at = for_account(&pool, 1)
            .await
            .expect("should read")
            .expect("should be there")
            .last_synced_at
            .expect("a successful read should stamp the time");

        record(&pool, 1, "github", Status::Error, Some("host unreachable"))
            .await
            .expect("should record");

        let after = for_account(&pool, 1)
            .await
            .expect("should read")
            .expect("should be there");

        assert_eq!(after.status, Status::Error);
        assert_eq!(
            after.last_synced_at,
            Some(synced_at),
            "the time of the last good read is the useful half of 'failing since'"
        );
    }

    #[tokio::test]
    async fn a_first_pass_that_fails_has_never_synced() {
        let pool = migrated_pool().await;

        record(&pool, 1, "github", Status::Error, Some("host unreachable"))
            .await
            .expect("should record");

        let stored = for_account(&pool, 1)
            .await
            .expect("should read")
            .expect("should be there");

        assert!(
            stored.last_synced_at.is_none(),
            "a failure must never stamp a time that no successful read produced"
        );
    }

    #[tokio::test]
    async fn forgetting_an_account_forgets_its_state() {
        let pool = migrated_pool().await;

        record(&pool, 1, "github", Status::Ok, None)
            .await
            .expect("should record");
        record(&pool, 2, "linear", Status::Ok, None)
            .await
            .expect("should record");

        forget(&pool, 1).await.expect("should forget");

        assert!(for_account(&pool, 1).await.expect("should read").is_none());
        assert!(
            for_account(&pool, 2).await.expect("should read").is_some(),
            "forgetting one account must not touch another"
        );
    }

    #[tokio::test]
    async fn an_unrecognised_status_reads_as_an_error() {
        let pool = migrated_pool().await;

        sqlx::query(
            "INSERT INTO sync_state (account_id, source, status) VALUES (1, 'github', 'Whatever')",
        )
        .execute(&pool)
        .await
        .expect("should store");

        let stored = for_account(&pool, 1)
            .await
            .expect("a value this release does not know must not fail the read")
            .expect("should be there");

        assert_eq!(stored.status, Status::Error);
    }
}
