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
/// reconnecting an account whose key is already stored replaces that row rather
/// than adding a second. `identity` is what the provider says the account is —
/// a login, an email address, a workspace — filled in when a provider is asked,
/// and shown to the user when there is no `label`. `label` is the name the
/// *user* gave the account ("Work"), and nothing but the interface reads it.
///
/// That guarantee has a seam, and it is here rather than left to be discovered.
/// An upgraded database has no provider identifier to seed `account_key` with,
/// so the migration writes the service name as a **placeholder** — `'github'`,
/// where a real connection stores a login. The first genuine reconnect supplies
/// the login, which does not match, so the constraint alone would insert a
/// second row and strand every backfilled `work_logs.account_id` on the
/// orphaned first one — defeating the backfill for exactly the installs it was
/// written for. Closing it is the connect path's job, not the schema's, and
/// `integrations::save` does it: saving a credential for a service whose only
/// account still carries the placeholder adopts that row rather than inserting
/// beside it.
///
/// That adoption is a heuristic, and the cost of it lands on the backfill
/// below, so it belongs here too. Because nothing written by this migration
/// marks a row as the migration's, adoption keys on the shape of the row and
/// never on whose credential it held — the login is not something SQL can ask
/// the provider for. Sign in after an upgrade as a *different* login from the
/// one the v2 row carried and the placeholder is relabelled to the new login
/// rather than split from it, so every `work_logs.account_id` this migration
/// backfills silently ends up attributing that history to the wrong person.
/// Re-logging the user's whole work log on every upgrade was judged the worse
/// outcome, and that judgement stands. What would close it properly is a column
/// recorded right here, at migration time — a flag marking the row as a
/// placeholder, and an identity captured while the old credential could still
/// be asked about itself. See `ADOPT_PLACEHOLDER` in `integrations` for the
/// statement and the full reasoning.
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

/// Somewhere to keep what Chief has worked out about this machine.
///
/// A key/value table rather than a column per fact, because the facts are few,
/// unrelated, and read one at a time: the tier this machine qualified for, and
/// what it measured when it last ran. Storing the measurement is what lets the
/// settings screen say how fast this machine answers without spending a
/// generation to find out again every time somebody opens it.
const ADD_SETTINGS: &str = r"
CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
";

/// An index of the corpus, so assembling a prompt is a query.
///
/// The corpus is a folder of markdown in the user's own directory, and the
/// question asked of it — which files exist, and what would they cost to put in
/// a prompt — is one a table answers in a statement and a filesystem answers in
/// a walk and a `stat` per file. The rows are a cache of the disk and the disk
/// wins every disagreement: nothing here is authoritative, which is why it can
/// be rebuilt wholesale.
///
/// `path` is relative to the corpus root and slash-separated whatever the
/// platform, so an index built on one machine reads on another.
const ADD_CORPUS_FILES: &str = r"
CREATE TABLE IF NOT EXISTS corpus_files (
    path             TEXT PRIMARY KEY,
    size             INTEGER NOT NULL,
    modified_at      TEXT NOT NULL,
    estimated_tokens INTEGER NOT NULL
);
";

/// When a brief was written, and from what.
///
/// One row per day, replaced when the day's brief is regenerated: a brief is
/// what today looks like now, not a history of what it looked like at each
/// point during it. The brief itself is a markdown file in the corpus — this is
/// only the record that it exists, so the daemon can tell whether the day has
/// been briefed without reading the folder.
const ADD_BRIEFS: &str = r"
CREATE TABLE IF NOT EXISTS briefs (
    date         TEXT PRIMARY KEY,
    generated_at TEXT NOT NULL,
    sources      TEXT NOT NULL DEFAULT ''
);
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
        Migration {
            version: 4,
            description: "remember what this machine can do",
            sql: ADD_SETTINGS,
            kind: MigrationKind::Up,
        },
        Migration {
            version: 5,
            description: "index the corpus",
            sql: ADD_CORPUS_FILES,
            kind: MigrationKind::Up,
        },
        Migration {
            version: 6,
            description: "record the briefs that were written",
            sql: ADD_BRIEFS,
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

    /// An empty in-memory database with every migration applied.
    pub async fn migrated_pool() -> SqlitePool {
        pool_at_version(EVERY_MIGRATION).await
    }

    /// An empty in-memory database migrated only as far as `version`, the way
    /// an install running an older release has it.
    ///
    /// How far it got is recorded in SQLite's own `user_version`, so [`upgrade`]
    /// resumes from there and the version an upgrade test is about is written
    /// once, in the call that says what the number means.
    pub async fn pool_at_version(version: i64) -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("failed to open in-memory database");

        migrate(&pool, version).await;

        pool
    }

    /// Apply every migration a [`pool_at_version`] database has not seen, the
    /// way installing a new release does.
    pub async fn upgrade(pool: &SqlitePool) {
        migrate(pool, EVERY_MIGRATION).await;
    }

    /// Beyond any version this schema will be given, so `migrate` runs to the
    /// end of the list.
    const EVERY_MIGRATION: i64 = i64::MAX;

    /// Apply the migrations between where `pool` got to and `through`.
    async fn migrate(pool: &SqlitePool, through: i64) {
        let mut at: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(pool)
            .await
            .expect("failed to read the schema version");

        for migration in super::migrations() {
            if migration.version <= at || migration.version > through {
                continue;
            }

            sqlx::raw_sql(migration.sql)
                .execute(pool)
                .await
                .expect("failed to apply migration");

            at = migration.version;
        }

        // `PRAGMA user_version` takes no bind parameter; `at` is one of our own
        // version numbers rather than anything a test supplies.
        sqlx::raw_sql(&format!("PRAGMA user_version = {at}"))
            .execute(pool)
            .await
            .expect("failed to record the schema version");
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{migrated_pool, pool_at_version, upgrade};
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
        let pool = pool_at_version(1).await;

        sqlx::query("INSERT INTO work_logs (source, content) VALUES ('github', 'Merged PR #4')")
            .execute(&pool)
            .await
            .expect("an entry should be storable before the upgrade");

        upgrade(&pool).await;

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
    async fn accounts_timestamp_themselves() {
        // `created_at` reaches the interface as `connected_at`, so the format
        // string in the schema is user-visible and a typo in it would be too.
        let pool = migrated_pool().await;

        sqlx::query(
            "INSERT INTO integration_accounts (service, account_key, access_token)
             VALUES ('github', 'octocat', 'token')",
        )
        .execute(&pool)
        .await
        .expect("insert should succeed");

        let created_at: String = sqlx::query_scalar(
            "SELECT created_at FROM integration_accounts WHERE account_key = 'octocat'",
        )
        .fetch_one(&pool)
        .await
        .expect("row should exist");

        assert!(
            created_at.ends_with('Z') && created_at.contains('T'),
            "expected an ISO-8601 timestamp, got {created_at}"
        );
    }

    #[tokio::test]
    async fn two_accounts_may_each_log_the_same_merge() {
        // Why `account_id` is in the dedupe key at all: a pull request both of a
        // person's GitHub accounts can see is two pieces of work to two logs,
        // and one piece of work to each of them.
        let pool = migrated_pool().await;

        let account = "INSERT INTO integration_accounts (service, account_key, access_token)
                       VALUES ('github', ?1, 'token')";

        for key in ["octocat", "hubot"] {
            sqlx::query(account)
                .bind(key)
                .execute(&pool)
                .await
                .expect("both accounts should insert");
        }

        let accounts: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM integration_accounts ORDER BY id")
                .fetch_all(&pool)
                .await
                .expect("both accounts should have ids");

        let entry = "INSERT INTO work_logs (source, content, external_id, account_id)
                     VALUES ('github', 'Merged PR #4', 'owner/repo#4', ?1)";

        for account_id in &accounts {
            sqlx::query(entry)
                .bind(account_id)
                .execute(&pool)
                .await
                .expect("each account should log the merge it saw");
        }

        let duplicate = sqlx::query(entry).bind(accounts[0]).execute(&pool).await;

        assert!(
            duplicate.is_err(),
            "one account should still log the same merge only once"
        );
    }

    #[tokio::test]
    async fn upgrading_from_v2_keeps_the_credential_and_the_log() {
        // A database as it stood before migration 3 shipped.
        let pool = pool_at_version(2).await;

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

        upgrade(&pool).await;

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
        let pool = pool_at_version(2).await;

        sqlx::query(
            "INSERT INTO work_logs (source, content, external_id)
             VALUES ('github', 'Merged PR #4', 'owner/repo#4')",
        )
        .execute(&pool)
        .await
        .expect("an entry should be storable before the upgrade");

        upgrade(&pool).await;

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
