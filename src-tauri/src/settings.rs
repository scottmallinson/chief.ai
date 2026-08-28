//! What Chief has worked out or been told, kept between launches.
//!
//! A key/value table rather than a column per fact, because the facts are few,
//! unrelated and read one at a time — the tier this machine measured, where the
//! user keeps their corpus. Values are opaque strings; whoever owns a key owns
//! what its value means and how to parse it.

use sqlx::SqlitePool;

/// Store `value` under `key`, replacing whatever was there.
///
/// A key holds one value, not a history: reading back what a setting *is* is
/// the only question anything asks of this table.
pub async fn set(pool: &SqlitePool, key: &str, value: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at)
         VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         ON CONFLICT (key) DO UPDATE
            SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;

    Ok(())
}

/// What is stored under `key`, if anything is.
pub async fn get(pool: &SqlitePool, key: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT value FROM settings WHERE key = ?1")
        .bind(key)
        .fetch_optional(pool)
        .await
}

/// Forget `key` entirely, so whatever reads it falls back to its default.
pub async fn clear(pool: &SqlitePool, key: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM settings WHERE key = ?1")
        .bind(key)
        .execute(pool)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::db::test_support::migrated_pool;

    #[tokio::test]
    async fn a_setting_that_was_never_stored_is_absent_rather_than_empty() {
        let pool = migrated_pool().await;

        assert_eq!(super::get(&pool, "nothing.here").await.expect("read"), None);
    }

    #[tokio::test]
    async fn stores_and_reads_a_value_back() {
        let pool = migrated_pool().await;

        super::set(&pool, "corpus.root", "/home/someone/Chief")
            .await
            .expect("write");

        assert_eq!(
            super::get(&pool, "corpus.root").await.expect("read"),
            Some("/home/someone/Chief".to_string())
        );
    }

    #[tokio::test]
    async fn writing_again_replaces_rather_than_stacking() {
        let pool = migrated_pool().await;

        super::set(&pool, "corpus.root", "/first")
            .await
            .expect("write");
        super::set(&pool, "corpus.root", "/second")
            .await
            .expect("write");

        assert_eq!(
            super::get(&pool, "corpus.root").await.expect("read"),
            Some("/second".to_string())
        );

        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM settings")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(rows, 1, "a key holds one value");
    }

    #[tokio::test]
    async fn clearing_a_setting_returns_it_to_having_no_value() {
        let pool = migrated_pool().await;

        super::set(&pool, "corpus.root", "/somewhere")
            .await
            .expect("write");
        super::clear(&pool, "corpus.root").await.expect("clear");

        assert_eq!(super::get(&pool, "corpus.root").await.expect("read"), None);
    }
}
