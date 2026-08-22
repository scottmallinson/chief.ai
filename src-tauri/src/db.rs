//! Local SQLite storage.
//!
//! The database lives in this application's config directory on the user's own
//! machine and is never synchronised anywhere. The Tauri SQL plugin owns the
//! connection pool and applies the migrations below at startup; the rest of the
//! crate borrows that pool rather than opening a second connection.

use sqlx::SqlitePool;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_sql::{DbInstances, DbPool, Migration, MigrationKind};

/// Connection string for the local database. Relative paths are resolved
/// against the app config directory by the SQL plugin.
pub const DB_URL: &str = "sqlite:chief.db";

/// Schema for the initial two tables.
///
/// `work_logs` is the chronological record of the user's work. `integrations`
/// holds the OAuth credentials for connected services — it stays on this
/// machine, which is why there is no server-side equivalent.
const CREATE_INITIAL_TABLES: &str = r"
CREATE TABLE IF NOT EXISTS work_logs (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    source    TEXT NOT NULL,
    content   TEXT NOT NULL,
    summary   TEXT
);

CREATE INDEX IF NOT EXISTS idx_work_logs_timestamp ON work_logs (timestamp DESC);

CREATE TABLE IF NOT EXISTS integrations (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    service_name  TEXT NOT NULL UNIQUE,
    access_token  TEXT NOT NULL,
    refresh_token TEXT,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
";

/// Lets an entry be traced back to the thing it describes, so the background
/// daemon can run again without writing the same achievement twice.
///
/// The index is partial: entries the user writes by hand have no external id
/// and must not collide with each other.
const ADD_WORK_LOG_EXTERNAL_ID: &str = r"
ALTER TABLE work_logs ADD COLUMN external_id TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_work_logs_external_id
    ON work_logs (source, external_id)
    WHERE external_id IS NOT NULL;
";

/// One row per connected *account*, rather than one per service.
///
/// `integrations` made `service_name` unique, so a person with a work and a
/// personal mailbox could connect only one. SQLite cannot drop a constraint, so
/// the table is rebuilt and its single row carried across.
///
/// Three columns name an account and none of them substitutes for another.
/// `account_key` is the provider's *stable* identifier, and the only one this
/// schema keys on: it is half of `UNIQUE (service, account_key)`, so
/// reconnecting the same account replaces its row instead of adding a second.
/// The migration seeds it from the service name because a one-row-per-service
/// database has nothing better to offer. `identity` is what the provider says
/// the account is — a login, an email address, a workspace — filled in when a
/// provider is asked, and shown to the user when there is no `label`. `label`
/// is the name the *user* gave the account ("Work"), and nothing but the
/// interface reads it.
///
/// `credential_kind` records how the credential was obtained rather than
/// leaving it to be inferred from which columns happen to be NULL: `'oauth'`
/// for a browser or device flow, `'token'` for one the user pasted in, `'dcr'`
/// for a client minted at run time by Dynamic Client Registration. `scopes`
/// records what was actually granted, which is not always what was asked for.
///
/// Three further columns exist for providers Chief has not added yet, because
/// adding them later would mean a migration for a value the provider hands over
/// on the first day: `expires_at` (Graph tokens last an hour), and `client_id` /
/// `client_secret`, which hold either a registration the user brought or one
/// minted at run time. Neither is a *shipped* secret — both belong to one
/// installation and never leave this machine.
///
/// `work_logs.account_id` is `NOT NULL DEFAULT 0` rather than nullable, because
/// it joins the dedupe key and SQLite does not consider two NULLs equal: a
/// nullable column would exempt every unattributed entry from the very index
/// that stops the daemon logging a merge twice. Zero is the sentinel for "no
/// account", which `AUTOINCREMENT` never issues, and it is what the backfill
/// writes for an entry whose service has no surviving credential — a database
/// that had GitHub connected and then disconnected still has its log.
const ADD_INTEGRATION_ACCOUNTS: &str = r"
CREATE TABLE integration_accounts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    service         TEXT NOT NULL,
    account_key     TEXT NOT NULL,
    label           TEXT,
    identity        TEXT,
    credential_kind TEXT NOT NULL DEFAULT 'oauth',
    access_token    TEXT NOT NULL,
    refresh_token   TEXT,
    expires_at      TEXT,
    scopes          TEXT,
    client_id       TEXT,
    client_secret   TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (service, account_key)
);

INSERT INTO integration_accounts
    (service, account_key, credential_kind, access_token, refresh_token, created_at)
SELECT service_name, service_name, 'oauth', access_token, refresh_token, created_at
  FROM integrations;

DROP TABLE integrations;

ALTER TABLE work_logs ADD COLUMN account_id INTEGER NOT NULL DEFAULT 0;

UPDATE work_logs
   SET account_id = ifnull(
           (SELECT id FROM integration_accounts WHERE service = work_logs.source),
           0
       )
 WHERE external_id IS NOT NULL;

DROP INDEX IF EXISTS idx_work_logs_external_id;

CREATE UNIQUE INDEX idx_work_logs_external_id
    ON work_logs (source, account_id, external_id)
    WHERE external_id IS NOT NULL;
";

/// Migrations applied to [`DB_URL`], in order.
///
/// Migrations are append-only: once a version has shipped, add a new one rather
/// than editing it, or existing installations will drift from the schema.
pub fn migrations() -> Vec<Migration> {
    vec![
        Migration {
            version: 1,
            description: "create work_logs and integrations",
            sql: CREATE_INITIAL_TABLES,
            kind: MigrationKind::Up,
        },
        Migration {
            version: 2,
            description: "trace work log entries back to their source",
            sql: ADD_WORK_LOG_EXTERNAL_ID,
            kind: MigrationKind::Up,
        },
        Migration {
            version: 3,
            description: "hold many labelled accounts per service",
            sql: ADD_INTEGRATION_ACCOUNTS,
            kind: MigrationKind::Up,
        },
    ]
}

/// Errors surfaced to the frontend from database work.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the local database is not available")]
    NotLoaded,
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// Borrow the pool the SQL plugin opened for [`DB_URL`].
///
/// The pool is preloaded at startup (see `plugins.sql.preload` in
/// `tauri.conf.json`), so this only fails if the plugin failed to initialise.
/// `SqlitePool` is reference counted, so cloning it is cheap.
pub async fn pool<R: Runtime>(app: &AppHandle<R>) -> Result<SqlitePool, Error> {
    let instances = app.try_state::<DbInstances>().ok_or(Error::NotLoaded)?;
    let instances = instances.0.read().await;

    match instances.get(DB_URL).ok_or(Error::NotLoaded)? {
        DbPool::Sqlite(pool) => Ok(pool.clone()),
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use sqlx::SqlitePool;

    /// An empty in-memory database with the migrations already applied.
    pub async fn migrated_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("failed to open in-memory database");

        for migration in super::migrations() {
            sqlx::raw_sql(migration.sql)
                .execute(&pool)
                .await
                .expect("failed to apply migration");
        }

        pool
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::migrated_pool;
    use sqlx::Row;

    #[tokio::test]
    async fn migration_creates_both_tables() {
        let pool = migrated_pool().await;

        let tables: Vec<String> =
            sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
                .fetch_all(&pool)
                .await
                .expect("failed to read schema")
                .iter()
                .map(|row| row.get::<String, _>("name"))
                .collect();

        assert!(tables.contains(&"work_logs".to_string()));
        assert!(tables.contains(&"integration_accounts".to_string()));
    }

    #[tokio::test]
    async fn upgrading_an_existing_database_keeps_its_entries() {
        // A database as it stood before migration 2 shipped.
        let pool = super::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("failed to open in-memory database");

        let mut applied = super::migrations().into_iter();
        let first = applied.next().expect("there is a first migration");
        sqlx::raw_sql(first.sql)
            .execute(&pool)
            .await
            .expect("migration 1 should apply");

        sqlx::query("INSERT INTO work_logs (source, content) VALUES ('github', 'Merged PR #4')")
            .execute(&pool)
            .await
            .expect("an entry should be storable before the upgrade");

        for migration in applied {
            sqlx::raw_sql(migration.sql)
                .execute(&pool)
                .await
                .expect("later migrations should apply to an existing database");
        }

        // The entry survives, and the new column is there but empty for it.
        let (content, external_id): (String, Option<String>) =
            sqlx::query_as("SELECT content, external_id FROM work_logs")
                .fetch_one(&pool)
                .await
                .expect("the existing entry should still be there");

        assert_eq!(content, "Merged PR #4");
        assert_eq!(external_id, None);
    }

    #[tokio::test]
    async fn accounts_are_unique_per_service_and_key() {
        let pool = migrated_pool().await;

        let insert = "INSERT INTO integration_accounts (service, account_key, access_token)
                      VALUES (?1, ?2, ?3)";

        sqlx::query(insert)
            .bind("github")
            .bind("octocat")
            .bind("token-1")
            .execute(&pool)
            .await
            .expect("first account should insert");

        // A second account on the same service is the whole point.
        sqlx::query(insert)
            .bind("github")
            .bind("hubot")
            .bind("token-2")
            .execute(&pool)
            .await
            .expect("a second account should insert");

        let duplicate = sqlx::query(insert)
            .bind("github")
            .bind("octocat")
            .bind("token-3")
            .execute(&pool)
            .await;

        assert!(
            duplicate.is_err(),
            "(service, account_key) should be unique"
        );
    }

    #[tokio::test]
    async fn upgrading_from_v2_keeps_the_credential_and_the_log() {
        // A database as it stood before migration 3 shipped.
        let pool = super::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("failed to open in-memory database");

        let mut applied = super::migrations().into_iter();

        for migration in applied.by_ref().take(2) {
            sqlx::raw_sql(migration.sql)
                .execute(&pool)
                .await
                .expect("the first two migrations should apply");
        }

        sqlx::query(
            "INSERT INTO integrations (service_name, access_token, refresh_token)
                     VALUES ('github', 'gho_old', 'ghr_old')",
        )
        .execute(&pool)
        .await
        .expect("a credential should be storable before the upgrade");
        sqlx::query(
            "INSERT INTO work_logs (source, content, external_id)
                     VALUES ('github', 'Merged PR #4', 'owner/repo#4')",
        )
        .execute(&pool)
        .await
        .expect("an entry should be storable before the upgrade");

        for migration in applied {
            sqlx::raw_sql(migration.sql)
                .execute(&pool)
                .await
                .expect("migration 3 should apply to an existing database");
        }

        // The credential survives, carried onto the new table.
        let (service, account_key, access_token, kind): (String, String, String, String) =
            sqlx::query_as(
                "SELECT service, account_key, access_token, credential_kind
                 FROM integration_accounts",
            )
            .fetch_one(&pool)
            .await
            .expect("the credential should have been carried across");

        assert_eq!(service, "github");
        assert_eq!(account_key, "github");
        assert_eq!(access_token, "gho_old");
        assert_eq!(kind, "oauth");

        // The log entry survives and is attributed to *that* account, so the
        // daemon does not log the same merge a second time.
        let carried: i64 = sqlx::query_scalar("SELECT id FROM integration_accounts")
            .fetch_one(&pool)
            .await
            .expect("the carried account should have an id");

        let (content, account_id): (String, i64) =
            sqlx::query_as("SELECT content, account_id FROM work_logs")
                .fetch_one(&pool)
                .await
                .expect("the existing entry should still be there");

        assert_eq!(content, "Merged PR #4");
        assert_eq!(
            account_id, carried,
            "the entry should be attributed to the account it was carried onto"
        );

        // The old table is gone.
        let leftover: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'integrations'",
        )
        .fetch_optional(&pool)
        .await
        .expect("should read the schema");

        assert_eq!(leftover, None, "the v1 table should have been dropped");
    }

    #[tokio::test]
    async fn an_unattributed_entry_is_still_logged_only_once() {
        // `account_id` joins the dedupe key, and SQLite does not consider two
        // NULLs equal — so an entry written without an account has to carry the
        // sentinel rather than a NULL, or the index would not apply to it.
        let pool = migrated_pool().await;

        let insert = "INSERT INTO work_logs (source, content, external_id)
                      VALUES ('github', 'Merged PR #4', 'owner/repo#4')";

        sqlx::query(insert)
            .execute(&pool)
            .await
            .expect("the first entry should insert");

        let duplicate = sqlx::query(insert).execute(&pool).await;

        assert!(
            duplicate.is_err(),
            "the same merge should not be loggable twice without an account"
        );
    }

    #[tokio::test]
    async fn upgrading_from_v2_without_a_credential_keeps_the_log_deduped() {
        // The common case: an install that never connected GitHub, or connected
        // and then disconnected it, still has daemon-written entries.
        let pool = super::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("failed to open in-memory database");

        let mut applied = super::migrations().into_iter();

        for migration in applied.by_ref().take(2) {
            sqlx::raw_sql(migration.sql)
                .execute(&pool)
                .await
                .expect("the first two migrations should apply");
        }

        sqlx::query(
            "INSERT INTO work_logs (source, content, external_id)
             VALUES ('github', 'Merged PR #4', 'owner/repo#4')",
        )
        .execute(&pool)
        .await
        .expect("an entry should be storable before the upgrade");

        for migration in applied {
            sqlx::raw_sql(migration.sql)
                .execute(&pool)
                .await
                .expect("migration 3 should apply with no credential to carry");
        }

        // Nothing to attribute it to, so it carries the sentinel rather than a
        // NULL that would fall outside the unique index.
        let (content, account_id): (String, i64) =
            sqlx::query_as("SELECT content, account_id FROM work_logs")
                .fetch_one(&pool)
                .await
                .expect("the existing entry should still be there");

        assert_eq!(content, "Merged PR #4");
        assert_eq!(account_id, 0, "an unattributed entry carries the sentinel");

        let duplicate = sqlx::query(
            "INSERT INTO work_logs (source, content, external_id)
             VALUES ('github', 'Merged PR #4', 'owner/repo#4')",
        )
        .execute(&pool)
        .await;

        assert!(
            duplicate.is_err(),
            "the same merge should not be loggable twice after the upgrade"
        );
    }
}
