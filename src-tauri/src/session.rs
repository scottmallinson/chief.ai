//! Reading from a connected account with the credentials Chief holds for it.
//!
//! Access tokens expire. Rather than making every caller think about that,
//! [`Session`] renews a rejected credential and the caller retries once — so a
//! connection made weeks ago keeps working without the user reconnecting.
//!
//! [`Session`] is generic over the provider because storing, renewing and
//! re-storing a credential is the same work whoever issued it. What is *not*
//! generic is the reading, so each provider keeps a thin wrapper of its own —
//! [`GithubSession`] here. One example is not enough to draw a general reading
//! abstraction from, and guessing at one would cost more than it saves.

use sqlx::SqlitePool;

use crate::github::{self, Client, PullRequest, State};
use crate::integrations;
use crate::oauth::Provider;

/// A conversation with one connected account.
pub struct Session<'a, P: Provider> {
    pool: &'a SqlitePool,
    provider: &'a P,
    account_id: i64,
    /// Needed only to renew. Resolved up front because a build without one
    /// simply cannot refresh — which is no reason to fail a plain read.
    client_id: Option<String>,
}

impl<'a, P: Provider> Session<'a, P> {
    pub fn new(pool: &'a SqlitePool, provider: &'a P, account_id: i64) -> Self {
        Self {
            pool,
            provider,
            account_id,
            client_id: provider.client_id().ok(),
        }
    }

    /// A session that renews with a known client id, for tests.
    #[cfg(test)]
    pub fn with_client_id(
        pool: &'a SqlitePool,
        provider: &'a P,
        account_id: i64,
        client_id: &str,
    ) -> Self {
        Self {
            pool,
            provider,
            account_id,
            client_id: Some(client_id.to_string()),
        }
    }

    /// The stored access token, or an error saying to reconnect.
    pub async fn token(&self) -> Result<String, P::Error> {
        Ok(self.credentials().await?.access_token)
    }

    async fn credentials(&self) -> Result<integrations::Credentials, P::Error> {
        integrations::credentials(self.pool, self.account_id)
            .await
            .map_err(P::Error::from)?
            .ok_or_else(|| P::Error::from(crate::db::Error::NotLoaded))
    }

    /// Swap an expired credential for a fresh one and store it.
    ///
    /// Without a refresh token or a client id there is nothing to try, and the
    /// caller reports that the user should reconnect rather than failing
    /// silently.
    pub async fn renew(&self) -> Result<Option<String>, P::Error> {
        let credentials = self.credentials().await?;

        let (Some(refresh_token), Some(client_id)) = (&credentials.refresh_token, &self.client_id)
        else {
            return Ok(None);
        };

        let tokens = self.provider.refresh(client_id, refresh_token).await?;

        integrations::store_tokens(
            self.pool,
            self.account_id,
            &tokens.access_token,
            // `store_tokens` writes what it is given, so a `None` here would
            // *clear* the refresh token rather than leave it. GitHub rotates
            // the pair on every renewal, but OAuth only says a provider *may*
            // issue a new refresh token — one that reuses the old one would
            // otherwise lose the only way back on its first renewal.
            tokens
                .refresh_token
                .as_deref()
                .or(credentials.refresh_token.as_deref()),
            // Nothing to record: the only provider here does not report a
            // lifetime for a refreshed token, and a stale expiry copied
            // forward would describe the token that has just been replaced.
            None,
        )
        .await
        .map_err(P::Error::from)?;

        Ok(Some(tokens.access_token))
    }

    /// Record who this account belongs to, the first time we find out.
    pub async fn name_once(&self, identity: &str) -> Result<(), P::Error> {
        integrations::set_identity(self.pool, self.account_id, identity)
            .await
            .map_err(P::Error::from)
    }
}

/// Reading GitHub on behalf of one connected account.
pub struct GithubSession<'a> {
    session: Session<'a, Client>,
    client: &'a Client,
}

impl<'a> GithubSession<'a> {
    pub fn new(pool: &'a SqlitePool, client: &'a Client, account_id: i64) -> Self {
        Self {
            session: Session::new(pool, client, account_id),
            client,
        }
    }

    #[cfg(test)]
    pub fn with_client_id(
        pool: &'a SqlitePool,
        client: &'a Client,
        account_id: i64,
        client_id: &str,
    ) -> Self {
        Self {
            session: Session::with_client_id(pool, client, account_id, client_id),
            client,
        }
    }

    /// The user's pull requests, renewing the token if GitHub says it expired.
    pub async fn pull_requests(
        &self,
        state: State,
        limit: u8,
    ) -> Result<Vec<PullRequest>, github::Error> {
        // Any failure to produce a credential — no such account, or a database
        // that will not answer — reads to the user as "GitHub is not
        // connected", which is the sentence that tells them what to do.
        let token = self
            .session
            .token()
            .await
            .map_err(|_| github::Error::NotConnected)?;

        match self.client.pull_requests(&token, state, limit).await {
            Err(github::Error::TokenRejected) => {
                let renewed = self
                    .session
                    .renew()
                    .await?
                    .ok_or(github::Error::TokenRejected)?;

                self.client.pull_requests(&renewed, state, limit).await
            }
            other => other,
        }
    }

    /// Who this account belongs to, recorded so the settings screen can say.
    pub async fn name_account(&self) -> Result<String, github::Error> {
        let token = self
            .session
            .token()
            .await
            .map_err(|_| github::Error::NotConnected)?;
        let login = self.client.viewer(&token).await?;
        self.session.name_once(&login).await?;

        Ok(login)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::migrated_pool;
    use crate::integrations::{NewAccount, OAUTH};
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

    /// A pool with one connected GitHub account, and that account's id.
    async fn connected(refresh_token: Option<&str>) -> (SqlitePool, i64) {
        let pool = migrated_pool().await;
        let account = integrations::save(
            &pool,
            NewAccount {
                service: integrations::GITHUB,
                account_key: "octocat",
                identity: Some("octocat"),
                credential_kind: OAUTH,
                access_token: "gho_old",
                refresh_token,
                expires_at: None,
                scopes: None,
                client_id: None,
                client_secret: None,
            },
        )
        .await
        .expect("should store credentials");

        (pool, account.id)
    }

    #[tokio::test]
    async fn reads_without_renewing_when_the_token_still_works() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", SEARCH_RESULTS)]);
        let (pool, account_id) = connected(Some("ghr_old")).await;
        let client = Client::against(&host).expect("should build a client");

        let prs = GithubSession::with_client_id(&pool, &client, account_id, "Iv1.clientid")
            .pull_requests(State::Open, 25)
            .await
            .expect("the stub should answer");

        assert_eq!(prs.len(), 1);
        assert_eq!(
            server.await.expect("the stub should finish").len(),
            1,
            "no renewal should have been attempted"
        );
    }

    #[tokio::test]
    async fn renews_an_expired_token_and_retries() {
        let (host, server) = serve(vec![
            ("HTTP/1.1 401 Unauthorized", EXPIRED),
            ("HTTP/1.1 200 OK", RENEWED),
            ("HTTP/1.1 200 OK", SEARCH_RESULTS),
        ]);
        let (pool, account_id) = connected(Some("ghr_old")).await;
        let client = Client::against(&host).expect("should build a client");

        let prs = GithubSession::with_client_id(&pool, &client, account_id, "Iv1.clientid")
            .pull_requests(State::Open, 25)
            .await
            .expect("the read should succeed after renewing");

        assert_eq!(prs.len(), 1);

        let requests = server.await.expect("the stub should finish");
        assert_eq!(requests.len(), 3, "expected read, renew, read");

        let (_, renewal) = crate::llama::test_support::split(&requests[1]);
        assert!(
            renewal.contains("grant_type=refresh_token"),
            "got {renewal}"
        );
        assert!(
            !renewal.contains("client_secret"),
            "a device-flow refresh needs no secret: {renewal}"
        );
        assert!(
            requests[2]
                .to_lowercase()
                .contains("authorization: bearer gho_new"),
            "the retry should use the renewed token"
        );
    }

    #[tokio::test]
    async fn stores_the_renewed_credentials_against_that_account() {
        let (host, server) = serve(vec![
            ("HTTP/1.1 401 Unauthorized", EXPIRED),
            ("HTTP/1.1 200 OK", RENEWED),
            ("HTTP/1.1 200 OK", SEARCH_RESULTS),
        ]);
        let (pool, account_id) = connected(Some("ghr_old")).await;
        let client = Client::against(&host).expect("should build a client");

        GithubSession::with_client_id(&pool, &client, account_id, "Iv1.clientid")
            .pull_requests(State::Open, 25)
            .await
            .expect("the read should succeed after renewing");

        let stored = integrations::credentials(&pool, account_id)
            .await
            .expect("should read")
            .expect("should be connected");

        assert_eq!(stored.access_token, "gho_new");
        assert_eq!(stored.refresh_token.as_deref(), Some("ghr_new"));

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn asks_the_user_to_reconnect_when_there_is_nothing_to_renew_with() {
        let (host, server) = serve(vec![("HTTP/1.1 401 Unauthorized", EXPIRED)]);
        let (pool, account_id) = connected(None).await;
        let client = Client::against(&host).expect("should build a client");

        let error = GithubSession::with_client_id(&pool, &client, account_id, "Iv1.clientid")
            .pull_requests(State::Open, 25)
            .await
            .expect_err("without a refresh token there is no way back");

        assert!(
            matches!(error, github::Error::TokenRejected),
            "got {error:?}"
        );
        assert_eq!(server.await.expect("the stub should finish").len(), 1);
    }

    #[tokio::test]
    async fn says_when_the_account_is_not_connected() {
        let pool = migrated_pool().await;
        let client = Client::against("http://127.0.0.1:1").expect("should build a client");

        let error = GithubSession::with_client_id(&pool, &client, 404, "Iv1.clientid")
            .pull_requests(State::Open, 25)
            .await
            .expect_err("there is nothing to read with");

        assert!(
            matches!(error, github::Error::NotConnected),
            "got {error:?}"
        );
    }

    /// A provider that renews without issuing a new refresh token, which OAuth
    /// allows and GitHub never does — so only a fake can stand in for it.
    struct Reuser;

    #[derive(Debug, thiserror::Error)]
    #[error("{0}")]
    struct ReuserError(String);

    impl From<crate::db::Error> for ReuserError {
        fn from(error: crate::db::Error) -> Self {
            Self(error.to_string())
        }
    }

    impl Provider for Reuser {
        const SERVICE: &'static str = "reuser";

        type Error = ReuserError;

        fn endpoints(&self) -> crate::oauth::Endpoints {
            crate::oauth::Endpoints {
                authorize: "https://example.invalid/authorize",
                token: "https://example.invalid/token",
                device_code: None,
            }
        }

        fn client_id(&self) -> Result<String, ReuserError> {
            Ok("reuser-client".to_string())
        }

        fn scopes(&self) -> &'static [&'static str] {
            &[]
        }

        async fn refresh(&self, _: &str, _: &str) -> Result<crate::oauth::Tokens, ReuserError> {
            Ok(crate::oauth::Tokens {
                access_token: "fresh".to_string(),
                refresh_token: None,
                expires_in: None,
            })
        }

        fn is_token_rejected(_: &ReuserError) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn keeps_a_refresh_token_the_provider_did_not_replace() {
        let (pool, account_id) = connected(Some("ghr_old")).await;

        Session::with_client_id(&pool, &Reuser, account_id, "reuser-client")
            .renew()
            .await
            .expect("the renewal should succeed");

        let stored = integrations::credentials(&pool, account_id)
            .await
            .expect("should read")
            .expect("should be connected");

        assert_eq!(stored.access_token, "fresh");
        assert_eq!(
            stored.refresh_token.as_deref(),
            Some("ghr_old"),
            "a provider that reuses its refresh token must not lose it"
        );
    }
}
