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

/// Migrations applied to [`DB_URL`], in order.
///
/// Migrations are append-only: once a version has shipped, add a new one rather
/// than editing it, or existing installations will drift from the schema.
pub fn migrations() -> Vec<Migration> {
    vec![Migration {
        version: 1,
        description: "create work_logs and integrations",
        sql: CREATE_INITIAL_TABLES,
        kind: MigrationKind::Up,
    }]
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
        assert!(tables.contains(&"integrations".to_string()));
    }

    #[tokio::test]
    async fn integrations_hold_one_row_per_service() {
        let pool = migrated_pool().await;

        let insert = "INSERT INTO integrations (service_name, access_token) VALUES (?1, ?2)";
        sqlx::query(insert)
            .bind("github")
            .bind("token-1")
            .execute(&pool)
            .await
            .expect("first insert should succeed");

        let duplicate = sqlx::query(insert)
            .bind("github")
            .bind("token-2")
            .execute(&pool)
            .await;

        assert!(duplicate.is_err(), "service_name should be unique");
    }

    #[tokio::test]
    async fn integrations_timestamp_themselves() {
        let pool = migrated_pool().await;

        sqlx::query("INSERT INTO integrations (service_name, access_token) VALUES ('github', 't')")
            .execute(&pool)
            .await
            .expect("insert should succeed");

        let created_at: String =
            sqlx::query_scalar("SELECT created_at FROM integrations WHERE service_name = 'github'")
                .fetch_one(&pool)
                .await
                .expect("row should exist");

        assert!(
            created_at.ends_with('Z') && created_at.contains('T'),
            "expected an ISO-8601 timestamp, got {created_at}"
        );
    }
}
