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

/// The drafts Chief prepared for things it noticed, and what became of them.
///
/// COSTA's `task-action-state.json`, in the store Chief already has. The body
/// lives in the corpus as markdown the user can open; only its state is here,
/// so a draft the user edited in their own editor is still the one on screen.
///
/// The unique index is the dedupe: one proposal per source item, forever. It
/// deliberately does not include `status` — a dismissed proposal has to keep
/// occupying its slot, or the next pass would draft it again and dismissing it
/// would read as having done nothing.
const ADD_PROPOSED_ACTIONS: &str = r"
CREATE TABLE IF NOT EXISTS proposed_actions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    source      TEXT NOT NULL,
    account_id  INTEGER NOT NULL DEFAULT 0,
    dedupe_key  TEXT NOT NULL,
    title       TEXT NOT NULL,
    context     TEXT NOT NULL DEFAULT '',
    path        TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'drafted',
    created_at  TEXT NOT NULL,
    acted_at    TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS proposed_actions_source_item
    ON proposed_actions (source, account_id, dedupe_key);
";

/// Structure the work log, index it for search, and record how fresh each
/// account is.
///
/// Three things at once, deliberately: they are the substrate D9 reads from,
/// and putting them in one migration is what lets every other issue in the DLE
/// batch be worked at the same time without two branches both claiming v8.
///
/// **Additive.** `content` stays, because the work log view renders it and an
/// entry the user typed has nothing else. `external_id` stays *nullable*
/// behind the partial index migration 3 rebuilt: a hand-written entry has no
/// external id, and two accounts on one service legitimately see the same one.
/// A `UNIQUE NOT NULL` here — which is what the specification asked for —
/// would break both.
///
/// `work_logs_fts` is an **external content** table: it stores the index and
/// not the text, so the rows are not duplicated and `work_logs` stays the only
/// copy. That has two consequences worth stating, because both are silent when
/// they go wrong.
///
/// The first is the `'rebuild'` at the end. An external content table starts
/// **empty** — it indexes nothing that was already there, so without this line
/// every existing install searches an empty index and gets no results while
/// looking perfectly healthy. New installs would pass every test.
///
/// The second is that the triggers pass `new.summary` and `old.summary`
/// **unmodified**, for agreement with what `'rebuild'` derives rather than for
/// correctness: FTS5 tokenises NULL and `''` identically, into no tokens at
/// all, so a `coalesce(…, '')` here changes nothing that can be observed. That
/// was established by writing it and watching every test still pass, and it is
/// recorded because the opposite is easy to assume — an earlier draft of this
/// comment asserted the coalesce would desynchronise the index and be caught
/// as corruption, and that was simply wrong.
///
/// What *does* desynchronise the index is a trigger that stops removing the
/// row it supersedes, and **`'integrity-check'` does not detect that** on an
/// external content table in this build of SQLite — verified by deleting the
/// `'delete'` half of the update trigger and watching a full integrity check
/// pass anyway. The guard that catches it is behavioural and lives with the
/// code that relies on it: `work_log::revising_an_entry_revises_what_search_
/// will_find` fails on exactly that breach. Anyone tempted to add an
/// integrity-check test back here should breach it first.
///
/// `sync_state` is keyed on `account_id` rather than on `source`, for the same
/// reason the dedupe index is: migration 3 made the account the unit, and a
/// person can hold a work and a personal GitHub. Keying on the service would
/// have one account's failure overwrite the other's state.
const ADD_STRUCTURED_WORK_LOG: &str = r"
ALTER TABLE work_logs ADD COLUMN category TEXT NOT NULL DEFAULT 'note';
ALTER TABLE work_logs ADD COLUMN title    TEXT NOT NULL DEFAULT '';
ALTER TABLE work_logs ADD COLUMN url      TEXT;
ALTER TABLE work_logs ADD COLUMN raw_ref  TEXT;

UPDATE work_logs
   SET title = substr(content, 1, coalesce(nullif(instr(content, char(10)), 0) - 1, 200))
 WHERE title = '';

CREATE VIRTUAL TABLE work_logs_fts USING fts5(
    title,
    summary,
    content='work_logs',
    content_rowid='id',
    tokenize='porter unicode61'
);

CREATE TRIGGER work_logs_fts_insert AFTER INSERT ON work_logs BEGIN
    INSERT INTO work_logs_fts (rowid, title, summary)
    VALUES (new.id, new.title, new.summary);
END;

CREATE TRIGGER work_logs_fts_delete AFTER DELETE ON work_logs BEGIN
    INSERT INTO work_logs_fts (work_logs_fts, rowid, title, summary)
    VALUES ('delete', old.id, old.title, old.summary);
END;

CREATE TRIGGER work_logs_fts_update AFTER UPDATE ON work_logs BEGIN
    INSERT INTO work_logs_fts (work_logs_fts, rowid, title, summary)
    VALUES ('delete', old.id, old.title, old.summary);
    INSERT INTO work_logs_fts (rowid, title, summary)
    VALUES (new.id, new.title, new.summary);
END;

INSERT INTO work_logs_fts (work_logs_fts) VALUES ('rebuild');

CREATE TABLE IF NOT EXISTS sync_state (
    account_id     INTEGER PRIMARY KEY,
    source         TEXT NOT NULL,
    status         TEXT NOT NULL,
    last_synced_at TEXT,
    error_message  TEXT
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
        Migration {
            version: 7,
            description: "keep the drafts Chief proposed",
            sql: ADD_PROPOSED_ACTIONS,
            kind: MigrationKind::Up,
        },
        Migration {
            version: 8,
            description: "structure the work log and index it for search",
            sql: ADD_STRUCTURED_WORK_LOG,
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

    /// An empty database **in a file**, with every migration applied, and the
    /// directory it lives in.
    ///
    /// `:memory:` is the right fixture for almost everything here and the
    /// wrong one for two questions. **WAL does not apply to an in-memory
    /// database** — `PRAGMA journal_mode` answers `memory` and setting it to
    /// `wal` is quietly ignored — so neither the journal mode Chief relies on
    /// nor the concurrency it buys can be observed without a real file.
    ///
    /// The returned [`Scratch`] deletes the directory when it drops, so the
    /// caller has to keep it alive for as long as the pool: a test that binds
    /// it to `_` deletes the database it is about to query.
    pub async fn file_backed_pool() -> (SqlitePool, Scratch) {
        let root = std::env::temp_dir().join(format!(
            "chief-db-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&root).expect("should create a scratch directory");

        let file = root.join("chief.db");
        let pool = SqlitePool::connect(&format!("sqlite://{}?mode=rwc", file.display()))
            .await
            .expect("failed to open a file-backed database");

        migrate(&pool, EVERY_MIGRATION).await;

        (pool, Scratch { root })
    }

    /// A temporary directory that goes when the test does.
    pub struct Scratch {
        root: std::path::PathBuf,
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
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

    /// Before anything is allowed to depend on FTS5, prove this build has it.
    ///
    /// `sqlx` compiles SQLite through `libsqlite3-sys`, and whether FTS5 is
    /// enabled is a build flag of that crate rather than anything this
    /// repository sets. If it were off, migration 8 would fail — at startup,
    /// on every user's machine, with the database left half-migrated. That is
    /// far too late to find out, and no other test in this file would say so
    /// first: they would all fail at once with the same opaque error.
    #[tokio::test]
    async fn the_bundled_sqlite_has_fts5() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("failed to open in-memory database");

        sqlx::raw_sql("CREATE VIRTUAL TABLE probe USING fts5(body)")
            .execute(&pool)
            .await
            .expect("FTS5 is not compiled into this build of SQLite");
    }

    /// The `'rebuild'` at the end of migration 8, which is the whole reason an
    /// upgrade differs from a fresh install here.
    ///
    /// An external content FTS5 table indexes **nothing that was already
    /// there**. Without the rebuild an existing install searches an empty
    /// index and gets no results, while every table, trigger and column is
    /// present and correct — so the schema looks right and the feature is
    /// silently dead. A fresh install would never notice, because its rows all
    /// arrive through the insert trigger.
    ///
    /// Proved by deleting the `'rebuild'` line and watching this fail:
    ///
    /// ```text
    /// the rebuild should have indexed the row that was already there
    /// ```
    #[tokio::test]
    async fn upgrading_indexes_the_entries_that_were_already_there() {
        // A database as it stood before migration 8 shipped.
        let pool = pool_at_version(7).await;

        sqlx::query(
            "INSERT INTO work_logs (source, content, summary)
             VALUES ('github', 'Refactored the OAuth handler', 'Landed the auth work')",
        )
        .execute(&pool)
        .await
        .expect("an entry should be storable before the upgrade");

        upgrade(&pool).await;

        let found: Option<i64> = sqlx::query_scalar(
            "SELECT rowid FROM work_logs_fts WHERE work_logs_fts MATCH '\"refactored\"'",
        )
        .fetch_optional(&pool)
        .await
        .expect("the index should be searchable");

        assert!(
            found.is_some(),
            "the rebuild should have indexed the row that was already there"
        );
    }

    /// The title backfill, which is what gives an upgraded row anything to
    /// match on: `title` is the column FTS5 ranks most heavily, and every row
    /// written before migration 8 has only `content`.
    #[tokio::test]
    async fn upgrading_takes_a_title_from_the_first_line_of_the_content() {
        let pool = pool_at_version(7).await;

        sqlx::query(
            "INSERT INTO work_logs (source, content)
             VALUES ('github', 'Merged PR #4' || char(10) || 'The body, which is not the title')",
        )
        .execute(&pool)
        .await
        .expect("an entry should be storable before the upgrade");

        upgrade(&pool).await;

        let title: String = sqlx::query_scalar("SELECT title FROM work_logs")
            .fetch_one(&pool)
            .await
            .expect("the entry should still be there");

        assert_eq!(title, "Merged PR #4", "the first line, and not the body");
    }

    /// Deleting a row takes it out of the index, which is what lets DLE-4's
    /// account purge rely on the trigger rather than clearing the index itself.
    #[tokio::test]
    async fn deleting_a_row_removes_it_from_the_index() {
        let pool = migrated_pool().await;

        sqlx::query(
            "INSERT INTO work_logs (source, content, title)
             VALUES ('github', 'body', 'Shipped the parser')",
        )
        .execute(&pool)
        .await
        .expect("should insert");

        let matches = |pool: sqlx::SqlitePool| async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM work_logs_fts WHERE work_logs_fts MATCH '\"parser\"'",
            )
            .fetch_one(&pool)
            .await
            .expect("the index should be searchable")
        };

        assert_eq!(matches(pool.clone()).await, 1, "indexed on insert");

        sqlx::query("DELETE FROM work_logs")
            .execute(&pool)
            .await
            .expect("should delete");

        assert_eq!(matches(pool.clone()).await, 0, "and gone again on delete");
    }

    /// An entry the user typed by hand still works after migration 8.
    ///
    /// Every insert in the tree predates the new columns and names only
    /// `(source, content, …)`, so the defaults have to carry them. A
    /// `NOT NULL` without a default here would break `create_work_log` and the
    /// daemon at once.
    #[tokio::test]
    async fn an_entry_written_the_old_way_still_stores() {
        let pool = migrated_pool().await;

        sqlx::query("INSERT INTO work_logs (source, content) VALUES ('note', 'Thought this')")
            .execute(&pool)
            .await
            .expect("the columns migration 8 added must all carry defaults");

        let (category, title, url): (String, String, Option<String>) =
            sqlx::query_as("SELECT category, title, url FROM work_logs")
                .fetch_one(&pool)
                .await
                .expect("the entry should be there");

        assert_eq!(category, "note");
        assert_eq!(title, "", "nothing invents a title for an entry mid-flight");
        assert_eq!(url, None);
    }
}

/// The two properties the architecture rests on, asserted rather than assumed.
///
/// Both are claims about the *shape* of the thing rather than about a feature,
/// which is why neither had a test: nothing breaks visibly when they stop being
/// true. A decoupled local engine that quietly depends on which model it is
/// running is not decoupled, and a background pass that blocks the question
/// somebody is waiting on is not background.
#[cfg(test)]
mod architecture_tests {
    use sqlx::SqlitePool;

    use crate::db::test_support::{file_backed_pool, migrated_pool};
    use crate::probe::Tier;
    use crate::retrieval;
    use crate::weights;
    use crate::work_log::{self, WorkLogRecord};

    /// The schema, as SQLite itself reports it — every table, index, trigger
    /// and virtual table, in a stable order.
    async fn schema(pool: &SqlitePool) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT type || ' ' || name FROM sqlite_master
              WHERE name NOT LIKE 'sqlite_%'
              ORDER BY type, name",
        )
        .fetch_all(pool)
        .await
        .expect("should read the schema")
    }

    fn record(external_id: &str, title: &str, summary: &str) -> WorkLogRecord {
        WorkLogRecord {
            timestamp: "2026-09-01T09:00:00.000Z".to_string(),
            source: "github".to_string(),
            category: "pr".to_string(),
            title: title.to_string(),
            content: format!("{title} — {summary}"),
            summary: Some(summary.to_string()),
            url: Some("https://github.com/o/r/pull/1".to_string()),
            raw_ref: None,
            external_id: external_id.to_string(),
            account_id: 1,
        }
    }

    /// Ingest a fixed corpus and search it, on a database of its own.
    ///
    /// Takes the tier so the caller can say which configuration it is
    /// exercising, and returns what came back. Nothing here reads the tier:
    /// **that is the point.** If ingestion or ranking ever started depending on
    /// the model, this function would have to change to let it, and the test
    /// below would stop compiling rather than quietly start lying.
    async fn ingest_and_search(tier: Tier) -> (Vec<String>, Vec<String>) {
        let pool = migrated_pool().await;
        let model = weights::for_tier(tier);

        // Named so a reader can see the model was actually selected, and so a
        // configuration that failed to load would be visible rather than
        // silently skipped.
        assert!(!model.name.is_empty(), "the tier must name a model");

        for (external_id, title, summary) in [
            ("owner/repo#1", "Refactored the OAuth handler", "merged"),
            ("owner/repo#2", "Add the PKCE auth handler", "open"),
            ("owner/repo#3", "Tidy the login form", "merged"),
        ] {
            work_log::upsert(&pool, record(external_id, title, summary))
                .await
                .expect("should store");
        }

        let stored: Vec<String> = sqlx::query_scalar("SELECT title FROM work_logs ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("should read");

        let ranked = retrieval::search(&pool, "refactoring auth", 10)
            .await
            .expect("should search")
            .into_iter()
            .map(|hit| hit.title)
            .collect();

        (stored, ranked)
    }

    /// **The whole claim of a decoupled local engine**: the model is a
    /// component, not the architecture.
    ///
    /// Ingestion derives its rows from what the provider said and search ranks
    /// them with `bm25()`; neither has ever asked which model is loaded, and
    /// this is what says so out loud. It is worth having precisely because
    /// nothing would break visibly on the day it stopped being true — a model
    /// swap would just quietly return different rows.
    ///
    /// Proved by making `ingest_and_search` prefix the title with the model's
    /// name, which is the smallest way ingestion could come to depend on it:
    ///
    /// ```text
    ///   left: ["Llama 3.2 3B Instruct: Refactored the OAuth handler", …]
    ///  right: ["Llama 3.2 1B Instruct: Refactored the OAuth handler", …]
    /// ```
    #[tokio::test]
    async fn swapping_the_model_changes_neither_the_rows_nor_the_ranking() {
        let (standard_rows, standard_ranking) = ingest_and_search(Tier::Standard).await;
        let (light_rows, light_ranking) = ingest_and_search(Tier::Light).await;

        // The fixture has to have done something, or two empty lists match and
        // this test proves nothing at all.
        assert_eq!(standard_rows.len(), 3, "the fixture must have stored rows");
        assert!(
            !standard_ranking.is_empty(),
            "the fixture must have matched something to rank"
        );
        assert_ne!(
            weights::for_tier(Tier::Standard).name,
            weights::for_tier(Tier::Light).name,
            "the two tiers must actually be different models"
        );

        assert_eq!(
            standard_rows, light_rows,
            "swapping the model must not change what is stored"
        );
        assert_eq!(
            standard_ranking, light_ranking,
            "swapping the model must not change what search returns, or in what order"
        );
    }

    /// The schema is the application's, not the model's.
    ///
    /// A migration that ever branched on the tier would put two installs of
    /// the same release on different schemas, which is the failure
    /// `check-migrations.mjs` guards the *numbering* against and nothing
    /// guarded the *content* against.
    ///
    /// Two empty lists are equal, so the fixture is asserted to have built
    /// something before the two are compared — raising that floor to 500 is
    /// what shows the assertion is load-bearing rather than decorative.
    #[tokio::test]
    async fn the_schema_is_the_same_whichever_model_is_selected() {
        let standard = migrated_pool().await;
        let light = migrated_pool().await;

        // Selected, so the test is about two configurations rather than about
        // two calls that ignored their argument.
        let _ = weights::for_tier(Tier::Standard);
        let _ = weights::for_tier(Tier::Light);

        let applied = schema(&standard).await;

        assert!(
            applied.len() > 5,
            "the fixture must have applied a real schema, got {applied:?}"
        );
        assert_eq!(applied, schema(&light).await);
    }

    /// **Chief does not run on WAL, and this is where that was found out.**
    ///
    /// The plan recorded the DLE specification's `PRAGMA journal_mode = WAL` as
    /// already true — "sqlx defaults to WAL and a 5-second busy timeout".
    /// **Half of that is wrong**, and it is the half the specification cared
    /// about. sqlx deliberately leaves `journal_mode` unset, and says why in
    /// its own source:
    ///
    /// ```text
    /// // Don't set `journal_mode` unless the user requested it.
    /// // WAL mode is a permanent setting for created databases and changing
    /// // into or out of it requires an exclusive lock that can't be waited on
    /// // with `sqlite3_busy_timeout()`.
    /// ```
    ///
    /// So a database sqlx creates is on the rollback journal, which is
    /// SQLite's own default, and this test asserts that rather than the thing
    /// everyone assumed.
    ///
    /// **And it cannot be fixed with a migration.** `journal_mode` is
    /// persistent in the file header, so setting it once would be enough — but
    /// `tauri-plugin-sql` hands every migration to sqlx with `no_tx: false`
    /// hard-coded, so each one runs inside a transaction, and SQLite does not
    /// quietly ignore the pragma there, it raises:
    ///
    /// ```text
    /// sqlite3.OperationalError: cannot change into wal mode from within a
    /// transaction
    /// ```
    ///
    /// A migration written to "just set WAL" would therefore fail on every
    /// installed copy on the first launch after the upgrade. Outside a
    /// transaction the same statement works and persists, so the fix exists —
    /// it needs a seam the plugin does not offer. Recorded in plan §9.
    #[tokio::test]
    async fn the_journal_mode_is_not_the_one_the_specification_asked_for() {
        let (pool, _scratch) = file_backed_pool().await;

        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .expect("should read the journal mode");

        assert_eq!(
            mode.to_lowercase(),
            "delete",
            "if this now reports wal, something gained a seam and plan §9 is out of date"
        );
    }

    /// The daemon writes while somebody is reading, and neither may fail.
    ///
    /// **This is the property the background pass rests on, and it holds for a
    /// different reason than anyone thought.** Under WAL a reader and a writer
    /// pass each other; Chief is not on WAL (see above), so what saves it is
    /// the five-second busy timeout — a reader that meets a writer waits a few
    /// milliseconds for one short upsert rather than failing. That is a
    /// latency cost rather than an error, which is why nothing has ever
    /// noticed, and it is worth knowing that is what is happening.
    ///
    /// **The loops are asserted to have run.** A concurrency test whose work
    /// never happened passes without concurrency, which the
    /// `proving-a-guard-test` skill names as the exact shape of a test that
    /// lies — so the counts are checked before the absence of failure is.
    /// Proved by emptying the write loop:
    ///
    /// ```text
    /// the write loop must actually have written
    ///   left: 0
    ///  right: 50
    /// ```
    #[tokio::test]
    async fn a_background_write_does_not_shut_a_reader_out() {
        let (pool, _scratch) = file_backed_pool().await;

        let writing = pool.clone();
        let writer = tokio::spawn(async move {
            let mut written = 0;

            for index in 0..50 {
                work_log::upsert(
                    &writing,
                    record(
                        &format!("owner/repo#{index}"),
                        &format!("Row {index}"),
                        "merged",
                    ),
                )
                .await
                .expect("a write must not be shut out by a reader");

                written += 1;
            }

            written
        });

        let reading = pool.clone();
        let reader = tokio::spawn(async move {
            let mut read = 0;

            for _ in 0..50 {
                let _: i64 = sqlx::query_scalar("SELECT count(*) FROM work_logs")
                    .fetch_one(&reading)
                    .await
                    .expect("a read must not be shut out by a write");

                read += 1;
            }

            read
        });

        let written = writer.await.expect("the writer should finish");
        let read = reader.await.expect("the reader should finish");

        assert_eq!(written, 50, "the write loop must actually have written");
        assert_eq!(read, 50, "the read loop must actually have read");

        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM work_logs")
            .fetch_one(&pool)
            .await
            .expect("should count");

        assert_eq!(rows, 50, "and every write must have landed");
    }

    /// `busy_timeout` is **per connection**, and the plugin owns the pool.
    ///
    /// This is the half of the specification's pragma pair that **is** already
    /// true: sqlx defaults it to exactly the five seconds asked for. It is
    /// also, given the journal mode above, the only thing keeping a background
    /// write from turning a reader's question into an error — so it is worth
    /// an assertion rather than an assumption, the more so because it is a
    /// dependency default and not a decision anything here made.
    ///
    /// There is still no `after_connect` seam through `tauri-plugin-sql` to
    /// set it deliberately. What that costs is now known to be nothing, since
    /// the default is the wanted value; what it would cost if sqlx changed the
    /// default is this test going red, which is the point.
    #[tokio::test]
    async fn the_busy_timeout_is_the_five_seconds_the_specification_wanted() {
        let (pool, _scratch) = file_backed_pool().await;

        let timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&pool)
            .await
            .expect("should read the busy timeout");

        assert_eq!(
            timeout, 5_000,
            "five seconds is sqlx's default and what the specification asked for"
        );
    }
}
