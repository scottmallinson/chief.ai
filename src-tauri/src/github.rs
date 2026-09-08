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

/// What Chief asks for, one scope per entry.
///
/// This is the list, and the space-separated string the device flow posts is
/// joined from it, so the scopes Chief requests and the scopes it reports as a
/// provider cannot drift apart.
///
/// `repo` is broader than we would like: it is read *and* write across public
/// and private repositories. Chief only ever reads, but GitHub offers no
/// narrower option — OAuth apps have no read-only scope for private
/// repositories, and `repo:status` covers commit statuses without granting any
/// access to pull requests at all. Fine-grained read-only permissions would
/// mean registering a GitHub App instead, which is the honest upgrade path.
const SCOPE_LIST: &[&str] = &["repo", "read:user"];

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
        "Chief has no GitHub client id to sign in with. Add one in Settings, under GitHub — make \
         an OAuth app on GitHub with the device flow enabled and paste its client id there."
    )]
    NoClientId,
    #[error("GitHub is not connected. Connect it in Settings.")]
    NotConnected,
    #[error("sign-in was declined on GitHub")]
    Declined,
    #[error("the sign-in code expired before it was entered")]
    CodeExpired,
    #[error(
        "This GitHub OAuth app does not have the device flow enabled, so GitHub will not issue a \
         sign-in code. Open the app on GitHub under Settings → Developer settings → \
         OAuth Apps, tick Enable Device Flow, then try again."
    )]
    DeviceFlowDisabled,
    #[error("GitHub would not start the sign-in: {0}")]
    DeviceFlowRefused(String),
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

/// The environment variable that points Chief at another OAuth app.
const CLIENT_ID_VAR: &str = "CHIEF_GITHUB_CLIENT_ID";

/// What this machine's environment says, if anything.
fn from_environment() -> Option<String> {
    std::env::var(CLIENT_ID_VAR)
        .ok()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

/// What this build was compiled with, if anything.
///
/// A release bakes in the repository variable of the same name, so nobody
/// installing Chief has to register an OAuth app. `option_env!` reads it at
/// compile time — see `build.rs`, which is what makes cargo notice when the
/// variable changes rather than reusing a binary compiled without it.
fn built_in() -> Option<String> {
    option_env!("CHIEF_GITHUB_CLIENT_ID")
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToString::to_string)
}

/// The OAuth client id, without reading the database.
///
/// Not a secret — the device flow has none — so it can be baked in at build
/// time or supplied at run time. Prefer [`configured_client_id`] wherever
/// there is a pool: this one cannot see an id the user pasted in Settings.
pub fn client_id() -> Result<String, Error> {
    from_environment()
        .or_else(built_in)
        .ok_or(Error::NoClientId)
}

/// The OAuth client id to actually sign in with.
///
/// The environment, then one the user supplied in Settings, then whatever the
/// build carried. See [`crate::oauth::registration`] for why there are three.
pub async fn configured_client_id(pool: &sqlx::SqlitePool) -> Result<String, Error> {
    registration(pool).await?.client_id.ok_or(Error::NoClientId)
}

/// Which registration GitHub sign-in would use, and where it came from.
pub async fn registration(
    pool: &sqlx::SqlitePool,
) -> Result<crate::oauth::registration::Registration, Error> {
    Ok(crate::oauth::registration::resolve(
        pool,
        crate::integrations::GITHUB,
        from_environment(),
        built_in(),
    )
    .await?)
}

/// What the user needs to do to finish signing in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLogin {
    /// The code the user types into GitHub.
    pub user_code: String,
    /// Where they type it.
    pub verification_uri: String,
    /// The same page with the code already in the box.
    ///
    /// **Chief builds this; GitHub does not send it.** RFC 8628 defines
    /// `verification_uri_complete` for exactly this and leaves it optional,
    /// and GitHub's device-code response has never carried one — the prefill
    /// is real but undocumented, so it is treated as something that may stop
    /// working without notice.
    ///
    /// That is the whole reason it is a *second* field rather than a rewritten
    /// first one. The user code stays on screen and the plain page stays the
    /// fallback, so a prefill GitHub removes tomorrow costs a keystroke rather
    /// than the ability to sign in.
    pub verification_uri_complete: String,
    /// Seconds until the code stops working.
    pub expires_in: u64,
}

/// The verification page with the code already filled in.
///
/// Percent-encodes the code rather than pasting it in: a user code is
/// `ABCD-1234` today and nothing promises it always will be, and a query
/// string assembled by hand is how a stray `&` becomes a parameter somebody
/// else chose.
fn prefilled(verification_uri: &str, user_code: &str) -> String {
    // Anything GitHub might already have put in the URL is left alone; this
    // only ever adds a parameter.
    let separator = if verification_uri.contains('?') {
        '&'
    } else {
        '?'
    };

    format!(
        "{verification_uri}{separator}user_code={}",
        percent_encode(user_code)
    )
}

/// Percent-encode everything that is not unreserved, per RFC 3986.
fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
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

/// What GitHub says when it will not issue a device code at all.
///
/// **This arrives as HTTP 200.** The device-code endpoint answers a refusal
/// with a success status and an `error` in the body, exactly as the polling
/// endpoint does, so a client that only looks at the status reads the refusal
/// as a code it then fails to find fields in.
#[derive(Debug, Clone, Deserialize)]
struct Refusal {
    error: String,
    error_description: Option<String>,
}

/// GitHub's answer to a device code request: a code, or why not.
///
/// Untagged with the code first, so the ordinary answer is matched on the
/// fields it actually has and only something without them is read as a
/// refusal.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum DeviceCodeAnswer {
    Issued(DeviceCodeResponse),
    Refused(Refusal),
}

/// Say what a refusal means, in words the user can act on.
///
/// Pure, so every branch is testable without a server. `device_flow_disabled`
/// is the one worth naming: it is what GitHub says about an OAuth app whose
/// **Enable Device Flow** box was never ticked, which is the state every app
/// is registered in and the single most likely reason a pasted client id
/// produces no code.
fn refusal(refused: Refusal) -> Error {
    match refused.error.as_str() {
        "device_flow_disabled" => Error::DeviceFlowDisabled,
        other => Error::DeviceFlowRefused(
            refused
                .error_description
                .filter(|description| !description.trim().is_empty())
                .unwrap_or_else(|| other.to_string()),
        ),
    }
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
    /// When it was merged, for pull requests that were.
    pub merged_at: Option<String>,
    /// The description the author wrote, when there is one.
    ///
    /// Carried for one reason: it is the user's own prose, and `profile.rs`
    /// samples it to describe how they write. It is deliberately **not** in
    /// [`as_tool_entries`] — a list of pull request bodies would swamp the
    /// prompt budget, and the model does not need one to say what is waiting.
    #[serde(skip_serializing)]
    pub body: Option<String>,
}

impl PullRequest {
    /// A stable identifier for this pull request, so the same merge is never
    /// logged twice.
    pub fn external_id(&self) -> String {
        format!("{}#{}", self.repository, self.number)
    }
}

/// What the user has to do with a pull request.
///
/// The distinction the brief was missing entirely. Chief only ever asked for
/// `author:@me` — the user's own work — while the prompt called that heading
/// "waiting on you". A review somebody has requested *of* them is the thing
/// actually waiting, and it was unreachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Involvement {
    /// Pull requests the user opened. Their own work in flight.
    #[default]
    Authored,
    /// Pull requests waiting on the user's review. Somebody else is blocked.
    Reviewing,
}

impl Involvement {
    fn qualifier(self) -> &'static str {
        match self {
            Self::Authored => "author:@me",
            Self::Reviewing => "review-requested:@me",
        }
    }
}

/// One issue assigned to the user, as the brief reads it.
///
/// Deliberately not a [`PullRequest`]. GitHub's search returns both from the
/// same endpoint in the same shape, and reusing the type would mean every
/// caller downstream having to ask which it was holding.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub number: i64,
    pub title: String,
    pub repository: String,
    pub url: String,
    pub updated_at: String,
}

/// Which pull requests to ask GitHub for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Open,
    Closed,
    /// Merged, which is narrower than closed: a closed pull request may simply
    /// have been abandoned.
    Merged,
    All,
}

impl State {
    fn qualifier(self) -> &'static str {
        match self {
            State::Open => " is:open",
            State::Closed => " is:closed",
            State::Merged => " is:merged",
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
        // Bound rather than inlined: the joined string has to outlive the
        // borrow the form takes of it.
        let scope = SCOPE_LIST.join(" ");

        let answer: DeviceCodeAnswer = self
            .post_form(
                &format!("{}/login/device/code", self.auth_host),
                &[("client_id", client_id), ("scope", &scope)],
            )
            .await?;

        let response = match answer {
            DeviceCodeAnswer::Issued(response) => response,
            DeviceCodeAnswer::Refused(refused) => return Err(refusal(refused)),
        };

        Ok(PendingLogin {
            device_code: response.device_code,
            // GitHub asks us not to poll faster than this.
            interval: Duration::from_secs(response.interval.max(1)),
            login: DeviceLogin {
                verification_uri_complete: prefilled(
                    &response.verification_uri,
                    &response.user_code,
                ),
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

    /// Exchange a refresh token for a fresh access token.
    ///
    /// GitHub requires a client secret here *unless* the token came from the
    /// device flow, which is how Chief signs in — so this needs no secret.
    pub async fn refresh(
        &self,
        client_id: &str,
        refresh_token: &str,
    ) -> Result<(String, Option<String>), Error> {
        let response: AccessTokenResponse = self
            .post_form(
                &format!("{}/login/oauth/access_token", self.auth_host),
                &[
                    ("client_id", client_id),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token),
                ],
            )
            .await?;

        match interpret(response) {
            Poll::Granted {
                access_token,
                refresh_token,
            } => Ok((access_token, refresh_token)),
            Poll::Failed(error) => Err(error),
            // Neither applies to a refresh; whatever happened, the stored
            // credential is no longer usable.
            Poll::KeepWaiting | Poll::SlowDown => Err(Error::TokenRejected),
        }
    }

    /// The user's pull requests, newest activity first.
    pub async fn pull_requests(
        &self,
        token: &str,
        involvement: Involvement,
        state: State,
        limit: u8,
    ) -> Result<Vec<PullRequest>, Error> {
        let query = format!("is:pr {}{}", involvement.qualifier(), state.qualifier());
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

    /// Issues assigned to the user and still open.
    ///
    /// The other half of "waiting on me": a review request is somebody blocked
    /// on you, and an assigned issue is your own queue. Both were invisible.
    ///
    /// `is:issue` rather than `is:pr`, so this and [`Self::pull_requests`]
    /// never return the same thing twice from the one endpoint they share.
    pub async fn assigned_issues(&self, token: &str, limit: u8) -> Result<Vec<Issue>, Error> {
        let url = format!("{}/search/issues", self.api_host);

        let response = self
            .http
            .get(url)
            .query(&[
                ("q", "is:issue assignee:@me is:open"),
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

        Ok(items.iter().map(issue_from).collect())
    }

    /// Who the stored token belongs to, so an account can name itself.
    pub async fn viewer(&self, token: &str) -> Result<String, Error> {
        let response = self
            .http
            .get(format!("{}/user", self.api_host))
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;

        let body: Value = self.read(response).await?;

        body["login"]
            .as_str()
            .map(ToString::to_string)
            .ok_or_else(|| Error::Decode("the user response had no login".to_string()))
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

fn issue_from(item: &Value) -> Issue {
    Issue {
        number: item["number"].as_i64().unwrap_or_default(),
        title: item["title"].as_str().unwrap_or_default().to_string(),
        repository: repository_from(item["repository_url"].as_str().unwrap_or_default()),
        url: item["html_url"].as_str().unwrap_or_default().to_string(),
        updated_at: item["updated_at"].as_str().unwrap_or_default().to_string(),
    }
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
        merged_at: item["pull_request"]["merged_at"]
            .as_str()
            .map(ToString::to_string),
        body: item["body"].as_str().map(ToString::to_string),
    }
}

/// Shape one account's pull requests into the entries the model reads.
///
/// Each carries the account it was read from. A person with a work and a
/// personal account gets one list covering both, and the only way the model can
/// say whose a pull request is — or that two similar ones are not the same work
/// twice — is if the entry says so itself.
pub fn as_tool_entries(account: &str, pull_requests: &[PullRequest]) -> Vec<Value> {
    pull_requests
        .iter()
        .map(|pull_request| {
            let mut entry = json!(pull_request);
            entry["account"] = json!(account);

            entry
        })
        .collect()
}

impl crate::oauth::Provider for Client {
    const SERVICE: &'static str = crate::integrations::GITHUB;

    type Error = Error;

    fn endpoints(&self) -> crate::oauth::Endpoints {
        crate::oauth::Endpoints {
            authorize: "https://github.com/login/oauth/authorize",
            token: "https://github.com/login/oauth/access_token",
            device_code: Some("https://github.com/login/device/code"),
        }
    }

    fn client_id(&self) -> Result<String, Error> {
        client_id()
    }

    fn environment_client_id(&self) -> Option<String> {
        from_environment()
    }

    fn scopes(&self) -> &'static [&'static str] {
        SCOPE_LIST
    }

    // The device flow has none, and one shipped inside a binary Chief
    // distributes would not be a secret, so the default `None` stands.

    async fn refresh(
        &self,
        client_id: &str,
        refresh_token: &str,
    ) -> Result<crate::oauth::Tokens, Error> {
        // Spelled out because `refresh` now names two things: the inherent
        // method, which does the exchange, and this one, which reshapes what it
        // returns. An inherent method wins the lookup, but saying so leaves no
        // doubt that this is not calling itself.
        let (access_token, refresh_token) = Client::refresh(self, client_id, refresh_token).await?;

        Ok(crate::oauth::Tokens {
            access_token,
            refresh_token,
            // GitHub's device-flow refresh does not say, and the reactive
            // renewal on a rejected token covers it.
            expires_in: None,
        })
    }

    fn is_token_rejected(error: &Error) -> bool {
        matches!(error, Error::TokenRejected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::migrated_pool;
    use crate::llama::test_support::{serve, split};
    use crate::oauth::registration::{self, Source};

    const DEVICE_CODE: &str = r#"{
        "device_code": "3584d83530557fdd1f46af8289938c8ef79f9dc5",
        "user_code": "WDJB-MJHT",
        "verification_uri": "https://github.com/login/device",
        "expires_in": 900,
        "interval": 5
    }"#;

    /// The gap between the device flow and "click, accept, done".
    ///
    /// Chief opens this page for the user, so a code already in the box is
    /// the difference between one manual step and none. GitHub does not send
    /// a `verification_uri_complete`, so Chief builds it — and the user code
    /// stays on screen, because the prefill is undocumented.
    #[test]
    fn puts_the_code_in_the_page_it_opens() {
        assert_eq!(
            super::prefilled("https://github.com/login/device", "WDJB-MJHT"),
            "https://github.com/login/device?user_code=WDJB-MJHT"
        );
    }

    /// The guard. The code is pasted into a query string, so a code carrying
    /// anything but unreserved characters must not be able to add a parameter
    /// of its own — GitHub issues `ABCD-1234` today and promises nothing
    /// about tomorrow.
    ///
    /// Proved by interpolating the code as it arrived:
    ///
    /// ```text
    /// assertion `left == right` failed: a user code goes into a URL
    ///   left: "https://github.com/login/device?user_code=A&scope=admin:org"
    ///  right: "https://github.com/login/device?user_code=A%26scope%3Dadmin%3Aorg"
    /// ```
    #[test]
    fn a_user_code_cannot_add_a_parameter_nobody_asked_for() {
        assert_eq!(
            super::prefilled("https://github.com/login/device", "A&scope=admin:org"),
            "https://github.com/login/device?user_code=A%26scope%3Dadmin%3Aorg",
            "a user code goes into a URL"
        );
    }

    /// A verification page that already carries a query keeps it.
    #[test]
    fn adds_to_a_query_rather_than_starting_a_second_one() {
        assert_eq!(
            super::prefilled("https://example.invalid/device?flow=1", "WDJB-MJHT"),
            "https://example.invalid/device?flow=1&user_code=WDJB-MJHT"
        );
    }

    /// The glue REC-60 needed: a build that carries no client id can still be
    /// given one, and sign-in uses it.
    ///
    /// Skipped where the environment is setting one, because there it is
    /// supposed to win — which is the layering `oauth::registration` tests
    /// against all three layers explicitly. Neither CI job sets it.
    #[tokio::test]
    async fn signs_in_with_an_id_the_user_supplied_in_settings() {
        if super::from_environment().is_some() {
            return;
        }

        let pool = migrated_pool().await;

        registration::store(&pool, crate::integrations::GITHUB, "Ov23liTheirs")
            .await
            .expect("store");

        assert_eq!(
            super::configured_client_id(&pool).await.expect("resolve"),
            "Ov23liTheirs"
        );
        assert_eq!(
            super::registration(&pool).await.expect("read").source,
            Source::Stored
        );
    }

    pub(super) const ONE_ISSUE: &str = r#"{
        "total_count": 1,
        "items": [{
            "number": 7,
            "title": "The engine will not start on macOS 12",
            "repository_url": "https://api.github.com/repos/scottmallinson/chief.ai",
            "html_url": "https://github.com/scottmallinson/chief.ai/issues/7",
            "updated_at": "2026-08-28T10:00:00Z"
        }]
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
                verification_uri_complete: "https://github.com/login/device?user_code=WDJB-MJHT"
                    .to_string(),
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
            // Pinned deliberately: widening this widens what every user grants.
            body.contains("scope=repo+read%3Auser") || body.contains("scope=repo%20read%3Auser"),
            "the requested scopes should be sent: {body}"
        );
        assert!(
            requests[0]
                .to_lowercase()
                .contains("accept: application/json"),
            "GitHub answers in form encoding without this"
        );
    }

    /// The defect this file's `Refusal` exists for.
    ///
    /// An OAuth app is registered with the device flow **off**, so the first
    /// thing a user who pasted their own client id meets is this refusal —
    /// and GitHub sends it as HTTP 200 with an `error` in the body. Read as a
    /// device code it is simply missing every field, which reached the screen
    /// as `could not read GitHub's response: error decoding response body`:
    /// a message about Chief's parser, naming nothing the user can do.
    ///
    /// Proved by reading the answer as a device code again, which is what
    /// this replaced:
    ///
    /// ```text
    /// unexpected error: could not read GitHub's response: error decoding
    ///   response body for url (http://127.0.0.1:46113/login/device/code)
    /// ```
    #[tokio::test]
    async fn says_when_the_oauth_app_has_no_device_flow() {
        let (host, server) = serve(vec![(
            "HTTP/1.1 200 OK",
            r#"{"error":"device_flow_disabled","error_description":"Device Flow has not been enabled","error_uri":"https://docs.github.com/"}"#,
        )]);
        let client = Client::against(&host).expect("should build a client");

        let error = client
            .start_login("Ov23liTheirs")
            .await
            .expect_err("a refusal is not a sign-in");

        assert!(
            matches!(error, Error::DeviceFlowDisabled),
            "unexpected error: {error}"
        );
        // The message is the fix, so it is what the test holds: it has to name
        // the box on GitHub that is not ticked.
        assert!(
            error.to_string().contains("Enable Device Flow"),
            "the message should say what to do: {error}"
        );

        server.await.expect("the stub should finish");
    }

    /// Any other refusal is repeated rather than swallowed. GitHub explains
    /// itself in `error_description`, and that sentence is worth more than
    /// anything Chief could write about a code it has not seen before.
    #[test]
    fn repeats_a_refusal_it_has_no_words_of_its_own_for() {
        let error = super::refusal(Refusal {
            error: "unauthorized_client".to_string(),
            error_description: Some("This app is not allowed to use this flow".to_string()),
        });

        assert_eq!(
            error.to_string(),
            "GitHub would not start the sign-in: This app is not allowed to use this flow"
        );
    }

    /// A refusal with no description falls back to the code itself, which is
    /// still something to search for.
    #[test]
    fn falls_back_to_the_error_code_when_github_explains_nothing() {
        let error = super::refusal(Refusal {
            error: "unsupported_grant_type".to_string(),
            error_description: None,
        });

        assert!(
            error.to_string().contains("unsupported_grant_type"),
            "unexpected message: {error}"
        );
    }

    /// The guard on the untagged enum: a real device code must still be read
    /// as one. `Refusal`'s fields are both absent from it, so the ordering
    /// cannot silently start matching the wrong arm.
    #[test]
    fn a_real_device_code_is_not_read_as_a_refusal() {
        let answer: DeviceCodeAnswer =
            serde_json::from_str(DEVICE_CODE).expect("the fixture should parse");

        assert!(
            matches!(answer, DeviceCodeAnswer::Issued(_)),
            "a device code should be read as a device code"
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
        assert_eq!(State::Merged.qualifier(), " is:merged");
        assert_eq!(State::All.qualifier(), "");
    }

    #[test]
    fn describes_itself_as_a_provider() {
        use crate::oauth::Provider;

        let client = Client::against("http://127.0.0.1:1").expect("should build a client");

        assert_eq!(Client::SERVICE, crate::integrations::GITHUB);
        assert_eq!(client.scopes(), &["repo", "read:user"]);
        assert!(
            client.client_secret().is_none(),
            "the device flow has no secret and a shipped one would not be secret"
        );
        assert!(
            client.endpoints().device_code.is_some(),
            "GitHub signs in by device code"
        );
    }

    #[tokio::test]
    async fn reports_who_the_token_belongs_to() {
        let (host, server) = serve(vec![(
            "HTTP/1.1 200 OK",
            r#"{"login":"octocat","name":"The Octocat"}"#,
        )]);
        let client = Client::against(&host).expect("should build a client");

        let viewer = client.viewer("gho_token").await.expect("should read");

        assert_eq!(viewer, "octocat");

        let requests = server.await.expect("the stub should finish");
        let (request_line, _) = split(&requests[0]);
        assert!(request_line.contains("/user"), "got {request_line}");
    }

    #[test]
    fn identifies_a_pull_request_by_repository_and_number() {
        let pr = PullRequest {
            number: 12,
            title: "Add the daemon".to_string(),
            repository: "scottmallinson/chief.ai".to_string(),
            state: "closed".to_string(),
            draft: false,
            url: "https://github.com/scottmallinson/chief.ai/pull/12".to_string(),
            updated_at: "2026-08-19T14:00:00Z".to_string(),
            merged_at: Some("2026-08-19T14:00:00Z".to_string()),
            body: None,
        };

        assert_eq!(pr.external_id(), "scottmallinson/chief.ai#12");
    }
}

#[cfg(test)]
mod waiting_on_me {
    //! The question the composer offers and Chief could not answer.

    use super::tests::ONE_ISSUE;
    use super::*;
    use crate::llama::test_support::{serve, split};

    const NONE: &str = r#"{"total_count":0,"items":[]}"#;

    /// The query GitHub is actually sent, which is the whole of this change.
    #[tokio::test]
    async fn asks_for_reviews_requested_of_the_user() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", NONE)]);
        let client = Client::against(&host).expect("client");

        client
            .pull_requests("gho_token", Involvement::Reviewing, State::Open, 25)
            .await
            .expect("should read");

        let received = server.await.expect("server");
        let (line, _) = split(&received[0]);

        assert!(
            line.contains("review-requested%3A%40me"),
            "the point of the whole issue: {line}"
        );
        assert!(
            !line.contains("author%3A%40me"),
            "and not the user's own: {line}"
        );
    }

    /// The behaviour that must not have changed.
    #[tokio::test]
    async fn still_asks_for_the_users_own_by_default() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", NONE)]);
        let client = Client::against(&host).expect("client");

        client
            .pull_requests("gho_token", Involvement::default(), State::Open, 25)
            .await
            .expect("should read");

        let received = server.await.expect("server");
        let (line, _) = split(&received[0]);

        assert!(line.contains("author%3A%40me"), "{line}");
        assert!(!line.contains("review-requested"), "{line}");
    }

    #[test]
    fn defaults_to_the_users_own_work() {
        assert_eq!(Involvement::default(), Involvement::Authored);
    }

    #[tokio::test]
    async fn reads_the_issues_assigned_to_the_user() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", ONE_ISSUE)]);
        let client = Client::against(&host).expect("client");

        let issues = client
            .assigned_issues("gho_token", 25)
            .await
            .expect("should read");

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].number, 7);
        assert_eq!(issues[0].repository, "scottmallinson/chief.ai");
        assert_eq!(issues[0].title, "The engine will not start on macOS 12");

        let received = server.await.expect("server");
        let (line, _) = split(&received[0]);

        assert!(
            line.contains("is%3Aissue"),
            "issues, not pull requests: {line}"
        );
        assert!(line.contains("assignee%3A%40me"), "{line}");
    }

    /// The two searches share one endpoint, so this is what keeps them apart.
    #[tokio::test]
    async fn never_returns_a_pull_request_as_an_assigned_issue() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", NONE)]);
        let client = Client::against(&host).expect("client");

        client.assigned_issues("gho_token", 25).await.expect("read");

        let received = server.await.expect("server");
        let (line, _) = split(&received[0]);

        assert!(!line.contains("is%3Apr"), "{line}");
    }

    #[tokio::test]
    async fn has_nothing_assigned_without_failing() {
        let (host, _server) = serve(vec![("HTTP/1.1 200 OK", NONE)]);
        let client = Client::against(&host).expect("client");

        assert!(client
            .assigned_issues("gho_token", 25)
            .await
            .expect("read")
            .is_empty());
    }
}
