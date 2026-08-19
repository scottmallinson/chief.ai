//! Credentials for services the user has connected.
//!
//! Tokens live in the local `integrations` table and are never sent anywhere
//! except to the service they belong to. There is one row per service, so
//! reconnecting replaces the credential rather than accumulating copies.

use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::Error;

/// The services Chief knows how to connect.
pub const GITHUB: &str = "github";

/// Whether a service is connected, for the settings screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub service: String,
    pub connected: bool,
    /// When the credential was stored, ISO-8601. `None` when not connected.
    pub connected_at: Option<String>,
}

/// Store a credential, replacing any previous one for this service.
pub async fn save(
    pool: &SqlitePool,
    service: &str,
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO integrations (service_name, access_token, refresh_token)
         VALUES (?1, ?2, ?3)
         ON CONFLICT (service_name) DO UPDATE SET
             access_token = excluded.access_token,
             refresh_token = excluded.refresh_token,
             created_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
    )
    .bind(service)
    .bind(access_token)
    .bind(refresh_token)
    .execute(pool)
    .await?;

    Ok(())
}

/// What is stored for a connected service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub access_token: String,
    /// Present only when the service issues expiring tokens.
    pub refresh_token: Option<String>,
}

/// Both stored tokens for a service, if it is connected.
pub async fn credentials(pool: &SqlitePool, service: &str) -> Result<Option<Credentials>, Error> {
    let row = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT access_token, refresh_token FROM integrations WHERE service_name = ?1",
    )
    .bind(service)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(access_token, refresh_token)| Credentials {
        access_token,
        refresh_token,
    }))
}

/// The access token for a service, if one is stored.
pub async fn token(pool: &SqlitePool, service: &str) -> Result<Option<String>, Error> {
    let token = sqlx::query_scalar::<_, String>(
        "SELECT access_token FROM integrations WHERE service_name = ?1",
    )
    .bind(service)
    .fetch_optional(pool)
    .await?;

    Ok(token)
}

/// Whether a service is connected, without reading the credential itself.
pub async fn status(pool: &SqlitePool, service: &str) -> Result<Connection, Error> {
    let connected_at = sqlx::query_scalar::<_, String>(
        "SELECT created_at FROM integrations WHERE service_name = ?1",
    )
    .bind(service)
    .fetch_optional(pool)
    .await?;

    Ok(Connection {
        service: service.to_string(),
        connected: connected_at.is_some(),
        connected_at,
    })
}

/// Forget a service's credential.
pub async fn forget(pool: &SqlitePool, service: &str) -> Result<(), Error> {
    sqlx::query("DELETE FROM integrations WHERE service_name = ?1")
        .bind(service)
        .execute(pool)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::migrated_pool;

    #[tokio::test]
    async fn stores_and_reads_back_a_token() {
        let pool = migrated_pool().await;

        save(&pool, GITHUB, "gho_first", None)
            .await
            .expect("should save");

        assert_eq!(
            token(&pool, GITHUB).await.expect("should read"),
            Some("gho_first".to_string())
        );
    }

    #[tokio::test]
    async fn reconnecting_replaces_the_credential() {
        let pool = migrated_pool().await;

        save(&pool, GITHUB, "gho_first", None)
            .await
            .expect("should save");
        save(&pool, GITHUB, "gho_second", Some("ghr_refresh"))
            .await
            .expect("should save again");

        assert_eq!(
            token(&pool, GITHUB).await.expect("should read"),
            Some("gho_second".to_string())
        );

        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM integrations")
            .fetch_one(&pool)
            .await
            .expect("should count");
        assert_eq!(rows, 1, "reconnecting should not add a second row");
    }

    #[tokio::test]
    async fn reports_nothing_when_a_service_is_not_connected() {
        let pool = migrated_pool().await;

        assert_eq!(token(&pool, GITHUB).await.expect("should read"), None);

        let status = status(&pool, GITHUB).await.expect("should read status");
        assert!(!status.connected);
        assert_eq!(status.connected_at, None);
    }

    #[tokio::test]
    async fn reports_when_a_service_is_connected() {
        let pool = migrated_pool().await;
        save(&pool, GITHUB, "gho_token", None)
            .await
            .expect("should save");

        let status = status(&pool, GITHUB).await.expect("should read status");

        assert!(status.connected);
        assert_eq!(status.service, GITHUB);
        assert!(
            status
                .connected_at
                .as_deref()
                .is_some_and(|at| at.contains('T') && at.ends_with('Z')),
            "expected an ISO-8601 timestamp, got {:?}",
            status.connected_at
        );
    }

    #[tokio::test]
    async fn forgetting_removes_the_credential() {
        let pool = migrated_pool().await;
        save(&pool, GITHUB, "gho_token", None)
            .await
            .expect("should save");

        forget(&pool, GITHUB).await.expect("should forget");

        assert_eq!(token(&pool, GITHUB).await.expect("should read"), None);
    }
}
