//! The GitHub integration: signing in, and reading the user's pull requests.
//!
//! This is the one place allowed to talk to a host that is not this machine,
//! and only because the user explicitly connected the account. Requests carry
//! the user's own token and go straight from here to GitHub — there is no
//! server of ours in between, and nothing is sent anywhere else.
//!
//! Sign-in uses the **device flow**. GitHub still requires a client secret to
//! exchange an authorization code, even with PKCE, and a secret shipped inside
//! a desktop binary is not a secret. The device flow needs no secret at all.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Where the device flow is driven from.
const AUTH_HOST: &str = "https://github.com";

/// Where the REST API lives.
const API_HOST: &str = "https://api.github.com";

/// What Chief asks for: read-only access to the user's repositories and
/// profile, so it can see pull requests. Nothing that can write.
const SCOPES: &str = "repo:status read:user";

/// GitHub asks clients to identify themselves.
const USER_AGENT: &str = concat!("chief-ai/", env!("CARGO_PKG_VERSION"));

/// Give up on a single request after this long.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Stop polling for the user to finish signing in after this long, whatever
/// GitHub says the code's lifetime is.
const MAX_POLL: Duration = Duration::from_secs(15 * 60);

/// What can go wrong connecting to or reading from GitHub.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "no GitHub client id is configured. Register an OAuth app with the device flow enabled \
         and set CHIEF_GITHUB_CLIENT_ID."
    )]
    NoClientId,
    #[error("GitHub is not connected. Connect it in Settings.")]
    NotConnected,
    #[error("sign-in was declined on GitHub")]
    Declined,
    #[error("the sign-in code expired before it was entered")]
    CodeExpired,
    #[error("GitHub rejected the stored token. Reconnect GitHub in Settings.")]
    TokenRejected,
    #[error("GitHub's rate limit is exhausted; try again later")]
    RateLimited,
    #[error("GitHub returned HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("could not read GitHub's response: {0}")]
    Decode(String),
    #[error("could not reach GitHub: {0}")]
    Transport(String),
    #[error(transparent)]
    Storage(#[from] crate::db::Error),
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// The OAuth client id. Not a secret — the device flow has none — so it can be
/// baked in at build time or supplied at run time.
pub fn client_id() -> Result<String, Error> {
    if let Ok(from_env) = std::env::var("CHIEF_GITHUB_CLIENT_ID") {
        if !from_env.trim().is_empty() {
            return Ok(from_env);
        }
    }

    option_env!("CHIEF_GITHUB_CLIENT_ID")
        .filter(|id| !id.trim().is_empty())
        .map(ToString::to_string)
        .ok_or(Error::NoClientId)
}

/// What the user needs to do to finish signing in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLogin {
    /// The code the user types into GitHub.
    pub user_code: String,
    /// Where they type it.
    pub verification_uri: String,
    /// Seconds until the code stops working.
    pub expires_in: u64,
}

/// GitHub's answer to a device code request.
#[derive(Debug, Clone, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    /// How many seconds GitHub wants between polls.
    interval: u64,
}

/// A device flow in progress: what to show the user, and what to poll with.
#[derive(Debug, Clone)]
pub struct PendingLogin {
    pub device_code: String,
    pub interval: Duration,
    pub login: DeviceLogin,
}

/// GitHub's answer while polling: either a token, or why not yet.
#[derive(Debug, Clone, Deserialize)]
struct AccessTokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    error: Option<String>,
}

/// A pull request, reduced to what the agent needs to answer questions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PullRequest {
    pub number: i64,
    pub title: String,
    pub repository: String,
    pub state: String,
    pub draft: bool,
    pub url: String,
    pub updated_at: String,
}

/// Which pull requests to ask GitHub for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Open,
    Closed,
    All,
}

impl State {
    fn qualifier(self) -> &'static str {
        match self {
            State::Open => " is:open",
            State::Closed => " is:closed",
            State::All => "",
        }
    }
}

/// What one poll of the token endpoint told us.
#[derive(Debug)]
enum Poll {
    Granted {
        access_token: String,
        refresh_token: Option<String>,
    },
    /// The user has not finished in the browser yet.
    KeepWaiting,
    /// We are asking too often.
    SlowDown,
    Failed(Error),
}

/// How much longer to wait after GitHub tells us to slow down.
const SLOW_DOWN_BACKOFF: Duration = Duration::from_secs(5);

/// Read one poll response. Pure, so every branch is testable without a clock.
fn interpret(response: AccessTokenResponse) -> Poll {
    if let Some(access_token) = response.access_token {
        return Poll::Granted {
            access_token,
            refresh_token: response.refresh_token,
        };
    }

    match response.error.as_deref() {
        Some("authorization_pending") => Poll::KeepWaiting,
        Some("slow_down") => Poll::SlowDown,
        Some("expired_token") => Poll::Failed(Error::CodeExpired),
        Some("access_denied") => Poll::Failed(Error::Declined),
        Some(other) => Poll::Failed(Error::Status {
            status: 200,
            body: other.to_string(),
        }),
        None => Poll::Failed(Error::Decode("no token and no error".to_string())),
    }
}

/// Talks to GitHub over HTTPS.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    auth_host: String,
    api_host: String,
}

impl Client {
    /// A client pointed at GitHub itself.
    pub fn new() -> Result<Self, Error> {
        Self::build(AUTH_HOST.to_string(), API_HOST.to_string())
    }

    /// A client pointed at a stub. Test-only, so a build cannot be pointed at
    /// a host the user did not choose.
    #[cfg(test)]
    pub(crate) fn against(host: &str) -> Result<Self, Error> {
        Self::build(host.to_string(), host.to_string())
    }

    fn build(auth_host: String, api_host: String) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|error| Error::Transport(error.to_string()))?;

        Ok(Self {
            http,
            auth_host,
            api_host,
        })
    }

    /// Ask GitHub for a device code, which the user then enters in a browser.
    pub async fn start_login(&self, client_id: &str) -> Result<PendingLogin, Error> {
        let response: DeviceCodeResponse = self
            .post_form(
                &format!("{}/login/device/code", self.auth_host),
                &[("client_id", client_id), ("scope", SCOPES)],
            )
            .await?;

        Ok(PendingLogin {
            device_code: response.device_code,
            // GitHub asks us not to poll faster than this.
            interval: Duration::from_secs(response.interval.max(1)),
            login: DeviceLogin {
                user_code: response.user_code,
                verification_uri: response.verification_uri,
                expires_in: response.expires_in,
            },
        })
    }

    /// Poll until the user finishes signing in, then return the access token
    /// and any refresh token.
    pub async fn finish_login(
        &self,
        client_id: &str,
        pending: &PendingLogin,
    ) -> Result<(String, Option<String>), Error> {
        let deadline = tokio::time::Instant::now() + MAX_POLL;
        let mut interval = pending.interval;

        loop {
            tokio::time::sleep(interval).await;

            if tokio::time::Instant::now() > deadline {
                return Err(Error::CodeExpired);
            }

            let response: AccessTokenResponse = self
                .post_form(
                    &format!("{}/login/oauth/access_token", self.auth_host),
                    &[
                        ("client_id", client_id),
                        ("device_code", &pending.device_code),
                        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ],
                )
                .await?;

            match interpret(response) {
                Poll::Granted {
                    access_token,
                    refresh_token,
                } => return Ok((access_token, refresh_token)),
                Poll::KeepWaiting => {}
                Poll::SlowDown => interval += SLOW_DOWN_BACKOFF,
                Poll::Failed(error) => return Err(error),
            }
        }
    }

    /// The user's pull requests, newest activity first.
    pub async fn pull_requests(
        &self,
        token: &str,
        state: State,
        limit: u8,
    ) -> Result<Vec<PullRequest>, Error> {
        let query = format!("is:pr author:@me{}", state.qualifier());
        let url = format!("{}/search/issues", self.api_host);

        let response = self
            .http
            .get(url)
            .query(&[
                ("q", query.as_str()),
                ("sort", "updated"),
                ("order", "desc"),
                ("per_page", &limit.clamp(1, 100).to_string()),
            ])
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;

        let body: Value = self.read(response).await?;

        let items = body["items"]
            .as_array()
            .ok_or_else(|| Error::Decode("the search response had no items".to_string()))?;

        Ok(items.iter().map(pull_request_from).collect())
    }

    async fn post_form<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        form: &[(&str, &str)],
    ) -> Result<T, Error> {
        let response = self
            .http
            .post(url)
            // Without this GitHub answers in form encoding.
            .header("Accept", "application/json")
            .form(form)
            .send()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;

        self.read(response).await
    }

    async fn read<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T, Error> {
        let status = response.status();

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();

            return Err(match status.as_u16() {
                401 => Error::TokenRejected,
                403 | 429 if body.contains("rate limit") => Error::RateLimited,
                status => Error::Status { status, body },
            });
        }

        response
            .json::<T>()
            .await
            .map_err(|error| Error::Decode(error.to_string()))
    }
}

/// GitHub's search results describe a repository by API URL; the agent wants
/// `owner/name`.
fn repository_from(repository_url: &str) -> String {
    repository_url
        .rsplit("/repos/")
        .next()
        .unwrap_or(repository_url)
        .to_string()
}

fn pull_request_from(item: &Value) -> PullRequest {
    PullRequest {
        number: item["number"].as_i64().unwrap_or_default(),
        title: item["title"].as_str().unwrap_or_default().to_string(),
        repository: repository_from(item["repository_url"].as_str().unwrap_or_default()),
        state: item["state"].as_str().unwrap_or("unknown").to_string(),
        draft: item["draft"].as_bool().unwrap_or(false),
        url: item["html_url"].as_str().unwrap_or_default().to_string(),
        updated_at: item["updated_at"].as_str().unwrap_or_default().to_string(),
    }
}

/// Shape pull requests into the payload the model reads.
pub fn as_tool_result(pull_requests: &[PullRequest]) -> Value {
    json!({
        "pull_requests": pull_requests,
        "count": pull_requests.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ollama::test_support::{serve, split};

    const DEVICE_CODE: &str = r#"{
        "device_code": "3584d83530557fdd1f46af8289938c8ef79f9dc5",
        "user_code": "WDJB-MJHT",
        "verification_uri": "https://github.com/login/device",
        "expires_in": 900,
        "interval": 5
    }"#;

    const PENDING: &str = r#"{"error":"authorization_pending"}"#;
    const GRANTED: &str =
        r#"{"access_token":"gho_token","token_type":"bearer","scope":"repo:status"}"#;

    fn pending_login(interval: Duration) -> PendingLogin {
        PendingLogin {
            device_code: "device-code".to_string(),
            interval,
            login: DeviceLogin {
                user_code: "WDJB-MJHT".to_string(),
                verification_uri: "https://github.com/login/device".to_string(),
                expires_in: 900,
            },
        }
    }

    #[tokio::test]
    async fn asks_github_for_a_code_to_show_the_user() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", DEVICE_CODE)]);
        let client = Client::against(&host).expect("should build a client");

        let started = client
            .start_login("Iv1.clientid")
            .await
            .expect("the stub should answer");

        assert_eq!(started.login.user_code, "WDJB-MJHT");
        assert_eq!(
            started.login.verification_uri,
            "https://github.com/login/device"
        );
        assert_eq!(started.interval, Duration::from_secs(5));
        assert_eq!(
            started.device_code,
            "3584d83530557fdd1f46af8289938c8ef79f9dc5"
        );

        let requests = server.await.expect("the stub should finish");
        let (request_line, body) = split(&requests[0]);

        assert!(
            request_line.starts_with("POST /login/device/code "),
            "unexpected request line: {request_line}"
        );
        assert!(body.contains("client_id=Iv1.clientid"), "body was {body}");
        assert!(
            body.contains("scope="),
            "the requested scopes should be sent: {body}"
        );
        assert!(
            requests[0]
                .to_lowercase()
                .contains("accept: application/json"),
            "GitHub answers in form encoding without this"
        );
    }

    #[tokio::test]
    async fn waits_for_the_user_then_stores_the_token() {
        let (host, server) = serve(vec![
            ("HTTP/1.1 200 OK", PENDING),
            ("HTTP/1.1 200 OK", PENDING),
            ("HTTP/1.1 200 OK", GRANTED),
        ]);
        let client = Client::against(&host).expect("should build a client");

        let (token, refresh) = client
            // A real sign-in polls every few seconds; the wait itself is
            // covered by `interpret`, so keep the test quick.
            .finish_login("Iv1.clientid", &pending_login(Duration::from_millis(10)))
            .await
            .expect("the stub should grant a token");

        assert_eq!(token, "gho_token");
        assert_eq!(refresh, None);

        let requests = server.await.expect("the stub should finish");
        assert_eq!(requests.len(), 3, "it should poll until GitHub answers");

        let (request_line, body) = split(&requests[0]);
        assert!(
            request_line.starts_with("POST /login/oauth/access_token "),
            "unexpected request line: {request_line}"
        );
        assert!(body.contains("device_code=device-code"), "body was {body}");
        assert!(
            body.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"),
            "the device grant type should be sent: {body}"
        );
        assert!(
            !body.contains("client_secret"),
            "the device flow must not send a client secret: {body}"
        );
    }

    #[test]
    fn a_granted_poll_yields_the_token() {
        let granted = interpret(AccessTokenResponse {
            access_token: Some("gho_token".to_string()),
            refresh_token: Some("ghr_refresh".to_string()),
            error: None,
        });

        match granted {
            Poll::Granted {
                access_token,
                refresh_token,
            } => {
                assert_eq!(access_token, "gho_token");
                assert_eq!(refresh_token.as_deref(), Some("ghr_refresh"));
            }
            other => panic!("expected Granted, got {other:?}"),
        }
    }

    #[test]
    fn a_pending_poll_keeps_waiting() {
        let pending = interpret(AccessTokenResponse {
            access_token: None,
            refresh_token: None,
            error: Some("authorization_pending".to_string()),
        });

        assert!(matches!(pending, Poll::KeepWaiting), "got {pending:?}");
    }

    #[test]
    fn github_can_ask_us_to_slow_down() {
        let slow = interpret(AccessTokenResponse {
            access_token: None,
            refresh_token: None,
            error: Some("slow_down".to_string()),
        });

        assert!(matches!(slow, Poll::SlowDown), "got {slow:?}");
    }

    #[test]
    fn a_declined_sign_in_is_reported() {
        let declined = interpret(AccessTokenResponse {
            access_token: None,
            refresh_token: None,
            error: Some("access_denied".to_string()),
        });

        assert!(
            matches!(declined, Poll::Failed(Error::Declined)),
            "got {declined:?}"
        );
    }

    #[test]
    fn an_expired_code_is_reported() {
        let expired = interpret(AccessTokenResponse {
            access_token: None,
            refresh_token: None,
            error: Some("expired_token".to_string()),
        });

        assert!(
            matches!(expired, Poll::Failed(Error::CodeExpired)),
            "got {expired:?}"
        );
    }

    #[test]
    fn an_empty_answer_is_not_treated_as_success() {
        let empty = interpret(AccessTokenResponse {
            access_token: None,
            refresh_token: None,
            error: None,
        });

        assert!(
            matches!(empty, Poll::Failed(Error::Decode(_))),
            "got {empty:?}"
        );
    }

    #[test]
    fn turns_a_repository_url_into_owner_and_name() {
        assert_eq!(
            repository_from("https://api.github.com/repos/scottmallinson/chief.ai"),
            "scottmallinson/chief.ai"
        );
        assert_eq!(repository_from(""), "");
    }

    #[test]
    fn asks_for_the_right_pull_requests() {
        assert_eq!(State::Open.qualifier(), " is:open");
        assert_eq!(State::Closed.qualifier(), " is:closed");
        assert_eq!(State::All.qualifier(), "");
    }
}
