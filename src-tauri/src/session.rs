//! Reading from GitHub with the user's stored credentials.
//!
//! Access tokens expire if the OAuth app is set to expire them. Rather than
//! making every caller think about that, this renews the credential when
//! GitHub rejects it and retries once — so a connection made weeks ago keeps
//! working without the user reconnecting.

use sqlx::SqlitePool;

use crate::github::{self, Client, PullRequest, State};
use crate::integrations::{self, Credentials};

/// A GitHub conversation on behalf of the connected user.
pub struct Session<'a> {
    pool: &'a SqlitePool,
    client: &'a Client,
    /// Needed only to renew a token. Resolved up front because a build without
    /// one simply cannot refresh — that is not a reason to fail a plain read.
    client_id: Option<String>,
}

impl<'a> Session<'a> {
    pub fn new(pool: &'a SqlitePool, client: &'a Client) -> Self {
        Self {
            pool,
            client,
            client_id: github::client_id().ok(),
        }
    }

    /// A session that renews with a known client id, for tests.
    #[cfg(test)]
    pub fn with_client_id(pool: &'a SqlitePool, client: &'a Client, client_id: &str) -> Self {
        Self {
            pool,
            client,
            client_id: Some(client_id.to_string()),
        }
    }

    /// The user's pull requests, renewing the token if GitHub says it expired.
    pub async fn pull_requests(
        &self,
        state: State,
        limit: u8,
    ) -> Result<Vec<PullRequest>, github::Error> {
        let credentials = integrations::credentials(self.pool, integrations::GITHUB)
            .await?
            .ok_or(github::Error::NotConnected)?;

        match self
            .client
            .pull_requests(&credentials.access_token, state, limit)
            .await
        {
            Err(github::Error::TokenRejected) => {
                let renewed = self.renew(&credentials).await?;

                self.client.pull_requests(&renewed, state, limit).await
            }
            other => other,
        }
    }

    /// Swap an expired credential for a fresh one and store it.
    ///
    /// Without a refresh token or a client id there is nothing to try, and the
    /// user is told to reconnect rather than being left with a silent failure.
    async fn renew(&self, credentials: &Credentials) -> Result<String, github::Error> {
        let (Some(refresh_token), Some(client_id)) = (&credentials.refresh_token, &self.client_id)
        else {
            return Err(github::Error::TokenRejected);
        };

        let (access_token, refresh_token) = self.client.refresh(client_id, refresh_token).await?;

        integrations::save(
            self.pool,
            integrations::GITHUB,
            &access_token,
            refresh_token.as_deref(),
        )
        .await?;

        Ok(access_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::migrated_pool;
    use crate::llama::test_support::serve;

    const SEARCH_RESULTS: &str = r#"{
        "total_count": 1,
        "items": [{
            "number": 12,
            "title": "Add the daemon",
            "repository_url": "https://api.github.com/repos/scottmallinson/chief.ai",
            "state": "open",
            "draft": false,
            "html_url": "https://github.com/scottmallinson/chief.ai/pull/12",
            "updated_at": "2026-08-19T14:00:00Z"
        }]
    }"#;

    const EXPIRED: &str = r#"{"message":"Bad credentials"}"#;
    const RENEWED: &str =
        r#"{"access_token":"gho_new","refresh_token":"ghr_new","token_type":"bearer"}"#;

    async fn connected(refresh_token: Option<&str>) -> SqlitePool {
        let pool = migrated_pool().await;
        integrations::save(&pool, integrations::GITHUB, "gho_old", refresh_token)
            .await
            .expect("should store credentials");

        pool
    }

    #[tokio::test]
    async fn reads_without_renewing_when_the_token_still_works() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", SEARCH_RESULTS)]);
        let pool = connected(Some("ghr_old")).await;
        let client = Client::against(&host).expect("should build a client");

        let prs = Session::with_client_id(&pool, &client, "Iv1.clientid")
            .pull_requests(State::Open, 25)
            .await
            .expect("the stub should answer");

        assert_eq!(prs.len(), 1);

        let requests = server.await.expect("the stub should finish");
        assert_eq!(requests.len(), 1, "no renewal should have been attempted");
    }

    #[tokio::test]
    async fn renews_an_expired_token_and_retries() {
        let (host, server) = serve(vec![
            ("HTTP/1.1 401 Unauthorized", EXPIRED),
            ("HTTP/1.1 200 OK", RENEWED),
            ("HTTP/1.1 200 OK", SEARCH_RESULTS),
        ]);
        let pool = connected(Some("ghr_old")).await;
        let client = Client::against(&host).expect("should build a client");

        let prs = Session::with_client_id(&pool, &client, "Iv1.clientid")
            .pull_requests(State::Open, 25)
            .await
            .expect("the read should succeed after renewing");

        assert_eq!(prs.len(), 1);

        let requests = server.await.expect("the stub should finish");
        assert_eq!(requests.len(), 3, "expected read, renew, read");

        // The renewal must not send a client secret: the device flow has none.
        let (_, renewal) = crate::llama::test_support::split(&requests[1]);
        assert!(
            renewal.contains("grant_type=refresh_token"),
            "unexpected renewal body: {renewal}"
        );
        assert!(
            renewal.contains("refresh_token=ghr_old"),
            "the stored refresh token should be sent: {renewal}"
        );
        assert!(
            !renewal.contains("client_secret"),
            "a device-flow refresh needs no secret: {renewal}"
        );

        // The retry uses the new token.
        assert!(
            requests[2]
                .to_lowercase()
                .contains("authorization: bearer gho_new"),
            "the retry should use the renewed token"
        );
    }

    #[tokio::test]
    async fn stores_the_renewed_credentials() {
        let (host, server) = serve(vec![
            ("HTTP/1.1 401 Unauthorized", EXPIRED),
            ("HTTP/1.1 200 OK", RENEWED),
            ("HTTP/1.1 200 OK", SEARCH_RESULTS),
        ]);
        let pool = connected(Some("ghr_old")).await;
        let client = Client::against(&host).expect("should build a client");

        Session::with_client_id(&pool, &client, "Iv1.clientid")
            .pull_requests(State::Open, 25)
            .await
            .expect("the read should succeed after renewing");

        let stored = integrations::credentials(&pool, integrations::GITHUB)
            .await
            .expect("should read")
            .expect("should be connected");

        assert_eq!(stored.access_token, "gho_new");
        assert_eq!(
            stored.refresh_token.as_deref(),
            Some("ghr_new"),
            "the rotated refresh token should replace the old one"
        );

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn asks_the_user_to_reconnect_when_there_is_nothing_to_renew_with() {
        let (host, server) = serve(vec![("HTTP/1.1 401 Unauthorized", EXPIRED)]);
        let pool = connected(None).await;
        let client = Client::against(&host).expect("should build a client");

        let error = Session::with_client_id(&pool, &client, "Iv1.clientid")
            .pull_requests(State::Open, 25)
            .await
            .expect_err("without a refresh token there is no way back");

        assert!(
            matches!(error, github::Error::TokenRejected),
            "got {error:?}"
        );

        let requests = server.await.expect("the stub should finish");
        assert_eq!(requests.len(), 1, "no renewal should have been attempted");
    }

    #[tokio::test]
    async fn surfaces_a_refusal_to_renew() {
        let (host, server) = serve(vec![
            ("HTTP/1.1 401 Unauthorized", EXPIRED),
            ("HTTP/1.1 200 OK", r#"{"error":"bad_refresh_token"}"#),
        ]);
        let pool = connected(Some("ghr_stale")).await;
        let client = Client::against(&host).expect("should build a client");

        let error = Session::with_client_id(&pool, &client, "Iv1.clientid")
            .pull_requests(State::Open, 25)
            .await
            .expect_err("a refused renewal should surface");

        match error {
            github::Error::Status { body, .. } => {
                assert!(body.contains("bad_refresh_token"), "got {body}")
            }
            other => panic!("expected Status, got {other:?}"),
        }

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn says_when_github_was_never_connected() {
        let pool = migrated_pool().await;
        let client = Client::against("http://127.0.0.1:1").expect("should build a client");

        let error = Session::with_client_id(&pool, &client, "Iv1.clientid")
            .pull_requests(State::Open, 25)
            .await
            .expect_err("there is nothing to read with");

        assert!(
            matches!(error, github::Error::NotConnected),
            "got {error:?}"
        );
    }
}
