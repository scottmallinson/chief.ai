//! Jira and Confluence, read through Atlassian's Remote MCP server.
//!
//! The odd one out. Every other provider here is either an OAuth sign-in
//! against a documented public client or a credential the user pastes; this
//! one is neither, because **Atlassian's classic 3LO has no public client at
//! all**. Its discovery document offers `client_secret_basic` and
//! `client_secret_post` and nothing else, so every 3LO client is confidential
//! — and a secret compiled into a binary Chief distributes is not a secret.
//! PKCE does not rescue it: there it hardens the code exchange *on top of*
//! client authentication rather than replacing it.
//!
//! The Remote MCP server runs a **different authorization server**, and that
//! one takes public clients. So Chief registers itself at runtime, per
//! installation, against an endpoint Atlassian does not document, and the
//! registration lands in that machine's own database. Nothing is baked in and
//! nothing is shipped.
//!
//! ## What was verified, and when
//!
//! The mechanism is undocumented, so what it does is written down here rather
//! than cited. Fetched 2026-09-08 from
//! `mcp.atlassian.com/.well-known/oauth-protected-resource` and the issuer it
//! names:
//!
//! - The protected resource is `https://mcp.atlassian.com/`, and its one
//!   authorization server is
//!   `https://auth.atlassian.com/VCeDsk8ZHncYF1g234fKtc4lNipbBhu3` — a
//!   different issuer from classic 3LO.
//! - That issuer advertises `registration_endpoint`, `S256`, a
//!   `refresh_token` grant, and `token_endpoint_auth_methods_supported`
//!   including **`none`**. That last one is the whole reason this path exists.
//! - Both `/v1/mcp` (Streamable HTTP) and `/v1/sse` (legacy) answer 401 with a
//!   `WWW-Authenticate: Bearer` challenge. Chief speaks to `/v1/mcp`: one POST
//!   per call, rather than a session to hold open.
//!
//! **The issuer now also advertises `client_id_metadata_document_supported`.**
//! That is the successor MCP's own spec prefers, and it is *worse* here: a
//! Client ID Metadata Document has to be hosted at a stable HTTPS URL the
//! client controls, and that URL becomes the client id. It is a permanent
//! off-machine dependency for an app whose whole claim is that none of it
//! exists off the user's machine, and a lapsed domain would break every
//! installed copy at once. So Chief stays on registration for as long as
//! registration exists, and this comment is here for whoever finds it gone.
//!
//! ## Registration happens per sign-in, not per install
//!
//! [`Client::start_login`] binds the loopback port **first**, then registers a
//! client whose one redirect URI names that exact port. RFC 8252 §7.3 requires
//! an authorization server to accept any port on a loopback redirect, but this
//! endpoint is undocumented and nothing says it follows that rule — and an
//! ephemeral port cannot be registered in advance anyway. Registering after
//! the bind costs one request per sign-in and cannot be wrong about the port.
//! The minted id is stored on the account, so renewal reuses it and no second
//! registration is needed until the user connects again.
//!
//! ## Read-only, and enforced by the authorization server
//!
//! Atlassian's write scopes sit on the same resource as its read scopes, so
//! asking for none of them makes read-only a property of the grant rather than
//! a promise about Chief's code — which is strictly better than the `repo`
//! scope Chief settles for on GitHub. [`SCOPES`] is that list, and D4's send
//! path is where forfeiting it would be argued, not here. The connect screen
//! says so out loud rather than burying it.
//!
//! ## Deterministic tools only
//!
//! Atlassian's MCP surface includes `searchAtlassian` and `fetchAtlassian`,
//! the Rovo natural-language layer. Calling either would send the user's typed
//! question to Atlassian, and `Sidebar.tsx` promises on every screen of the app
//! that nothing they type leaves this machine. [`DETERMINISTIC`] is the
//! allowlist, [`Client::call`] refuses anything outside it **before** a request
//! is built, and a test asserts the refusal costs no network call.
//!
//! Chief also never declares the MCP `sampling` capability: a server that can
//! request sampling can run its own agentic loop on the user's local model.
//! [`CAPABILITIES`] is empty, and a test reads what actually went out to prove
//! it.
//!
//! ## Two ways in, and the second is not a lesser one
//!
//! This file is the MCP path. [`rest`] is the other: an API token the user
//! pastes, used as HTTP Basic against their own site. The design spec kept it
//! deliberately — "the answer for Free-tier sites and for organisations that
//! disable the MCP server" — and an organisation restricting Rovo is exactly
//! that case, reported from a real tenant. Neither path is a fallback in the
//! sense of being worse: the token reaches Confluence, which the MCP path
//! still does not.
//!
//! What they share lives here: [`Error`] and its classification, [`Issue`],
//! [`Site`], [`describe`], and the read limit. What differs is the whole
//! transport, which is why they are two files rather than one with branches.

pub mod rest;

use std::time::Duration;

use serde::Deserialize;

use crate::oauth::{loopback, pkce, Endpoints, Tokens};

/// Where the MCP server lives. Pinned, like every other host Chief talks to.
const HOST: &str = "https://mcp.atlassian.com";

/// The canonical resource identifier, sent as RFC 8707 `resource` on both the
/// authorize and token requests because MCP's authorization spec requires it.
const RESOURCE: &str = "https://mcp.atlassian.com/";

/// The only origin Chief will follow discovery to.
///
/// Discovery is a document a host hands over, so it is an input rather than a
/// fact. One of the MCP servers surveyed for the design spec published
/// *forged* metadata naming an issuer it did not own; a client that follows
/// discovery wherever it leads would have acted on that. Chief reads the
/// document for its endpoint paths and refuses an issuer that is not here.
///
/// An **origin**, scheme included, rather than a host: comparing hosts alone
/// would accept `http://auth.atlassian.com`, which is the same name over a
/// transport anyone on the path can rewrite.
const ISSUER_ORIGIN: &str = "https://auth.atlassian.com";

/// The MCP revision Chief speaks. Pinned rather than negotiated: this client
/// implements three methods, and a revision that changed them should fail
/// loudly rather than be papered over.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// How long to wait before deciding Atlassian is not answering.
const TIMEOUT: Duration = Duration::from_secs(30);

/// What Chief calls itself on the wire.
///
/// `reqwest` sends **no** `User-Agent` at all unless one is set, and this
/// module shipped without one. `github.rs` has always set the same string, and
/// it is the only client here whose reads are known to work from the machine
/// where Atlassian's failed — which is not proof, but a nameless request is
/// the wrong thing to send a third party either way. `mcp.atlassian.com` is
/// the one Atlassian host behind Cloudflare, whose managed rules commonly
/// action exactly that.
const USER_AGENT: &str = concat!("chief-ai/", env!("CARGO_PKG_VERSION"));

/// What Chief asks for: reading Jira, and nothing else.
///
/// **Every write scope Atlassian offers on this resource is deliberately
/// absent** — `write:jira-work`, `write:page:confluence`,
/// `write:comment:confluence` — as is Compass and Teamwork Graph, which Chief
/// does not read. `offline_access` is what makes renewal possible; `read:me`
/// is how an account names itself.
///
/// **Confluence is deliberately absent too, though it is half of this step.**
/// Its scopes are granular and available — `search:confluence`,
/// `read:page:confluence`, `read:space:confluence` — and asking for them now
/// would save the user a second consent later. It would also mean a consent
/// screen listing access to their whole Confluence for a read no code path
/// makes, which is the criticism Chief levels at GitHub's `repo` scope from
/// the other side. So they arrive with the read that uses them.
///
/// Changing this list is a change to what the grant permits, not a
/// convenience. See D4.
pub const SCOPES: [&str; 3] = ["offline_access", "read:me", "read:jira-work"];

/// Which sites this credential can reach. Takes no arguments.
pub const ACCESSIBLE_RESOURCES: &str = "getAccessibleAtlassianResources";
/// Jira search by a JQL string Chief builds.
pub const SEARCH_JIRA: &str = "searchJiraIssuesUsingJql";
/// One Jira issue by key.
pub const GET_JIRA_ISSUE: &str = "getJiraIssue";

/// The tools Chief may call, and therefore the only ones it can.
///
/// An **allowlist**, not a denylist of the two natural-language tools. A
/// denylist is wrong the moment Atlassian adds a third: the rule is that Chief
/// calls tools whose arguments it built, and a tool nobody has vetted is not
/// one of those whatever it happens to be named.
///
/// Confluence's `getConfluencePage` belongs here and is not here yet, for the
/// same reason its scope is not in [`SCOPES`]: the allowlist says what Chief
/// may call, and it may not call what it has not been granted.
pub const DETERMINISTIC: [&str; 3] = [ACCESSIBLE_RESOURCES, SEARCH_JIRA, GET_JIRA_ISSUE];

/// What Chief tells the server it can do: nothing.
///
/// Empty on purpose and asserted by a test that reads the bytes actually sent.
/// `sampling` is the one that matters — a server holding that capability can
/// ask the client to run a completion, which on this machine means somebody
/// else's server driving the user's own model. `roots` and `elicitation` are
/// declined on the same principle: this client makes calls, it does not offer
/// services.
///
/// A function rather than a constant because it must serialise as `{}` and an
/// empty `serde_json::Map` cannot be built in a `const`. The emptiness is the
/// contract, so it is stated in one place and tested through what goes out.
fn capabilities() -> serde_json::Value {
    serde_json::json!({})
}

/// How many issues one read returns. A brief is five bullets.
pub const READ_LIMIT: u8 = 25;

/// What is assigned to this person and not finished.
///
/// Built here, from constants, and never from anything the user typed —
/// `currentUser()` is Jira's own way of saying "whoever this token belongs
/// to", so Chief never has to ask who that is or interpolate an identity.
/// `resolution = EMPTY` is Jira's equivalent of Linear's null `completedAt`:
/// every site renames its workflow states and none of them can rename this.
const ASSIGNED_JQL: &str = "assignee = currentUser() AND resolution = EMPTY ORDER BY updated DESC";

/// What can go wrong reading Atlassian.
///
/// **No variant carries a token, a code or a verifier.** These strings reach
/// the screen and the log, which a test asserts.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Atlassian is not connected")]
    NotConnected,
    /// The connection never got as far as an answer: DNS, TLS, no route, a
    /// timeout.
    ///
    /// **Carries why, and which host.** It used to be a bare "Atlassian could
    /// not be reached", which is true of a name that does not resolve, a
    /// certificate that does not verify, a firewall, and a 404 alike — and
    /// being told that on a button press leaves the user and the next person
    /// to debug it with nowhere to start. Ten call sites threw the cause away
    /// with `map_err(|_| …)`; none do now.
    #[error("{host} could not be reached: {because}")]
    Unreachable { host: String, because: String },
    /// The TLS certificate did not verify.
    ///
    /// **Split out of [`Error::Unreachable`] because it is the one failure
    /// this module was actually reported for, and because it is not a network
    /// problem the user can wait out.** On the machine that reported it,
    /// corporate DNS resolved `mcp.atlassian.com` to a network that is not
    /// Atlassian's, and Windows then refused the substituted certificate with
    /// `CRYPT_E_NO_REVOCATION_CHECK` — while `api.github.com`, which was not
    /// redirected, answered 200. "Could not be reached" sends somebody to
    /// check their wifi; naming the certificate sends them to the right place.
    ///
    /// Chief does **not** offer a way to skip verification, and must not grow
    /// one. An intercepted connection to a host holding somebody's Jira is
    /// exactly the thing certificate verification is for.
    #[error(
        "the certificate for {host} could not be verified, so Chief stopped rather than \
         trusting it. Something on this network may be inspecting HTTPS traffic: {because}"
    )]
    Untrusted { host: String, because: String },
    /// The host answered, but not with success. Distinct from
    /// [`Error::Unreachable`] because the network is fine and something is
    /// refusing on purpose — which is a different thing for the user to do
    /// something about.
    #[error("{host} answered {status}")]
    Answered { host: String, status: u16 },
    #[error("Atlassian answered with something unexpected")]
    Decode,
    #[error("Atlassian refused the sign-in: {0}")]
    Refused(String),
    #[error("that Atlassian sign-in was declined or expired")]
    Rejected,
    #[error("Atlassian would not register this copy of Chief: {0}")]
    Registration(String),
    #[error("Chief does not trust '{0}' to sign you in")]
    UntrustedIssuer(String),
    #[error("'{0}' is not a tool Chief is allowed to call")]
    NotDeterministic(String),
    /// The site address the user typed is not one. Carries what they typed,
    /// which is not a credential — the token is, and it is in a header.
    #[error("{0}")]
    Site(String),
    #[error("Atlassian did not recognise that request: {0}")]
    Tool(String),
    #[error(transparent)]
    Storage(#[from] crate::db::Error),
}

/// The host a URL names, so an error can say where it was going without
/// quoting a whole URL — and without ever quoting a query string, which is
/// where a code or a token would be if one were ever put in one.
#[must_use]
pub(crate) fn host_of(url: &str) -> String {
    url.split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

/// Why a request failed, in the words of whatever actually failed.
///
/// `reqwest`'s own `Display` is "error sending request for url (…)" every
/// time; the useful sentence — `dns error`, `certificate verify failed`,
/// `connection refused`, `operation timed out` — is further down the source
/// chain. Both ends are kept: the outer says what stage it was at and the
/// inner says what went wrong.
///
/// **Nothing here can carry a credential.** A `reqwest::Error` holds a URL and
/// an I/O cause, never headers, and the one request that carries a bearer
/// token puts it in a header. A test asserts the rendering.
#[must_use]
pub(crate) fn because(error: &reqwest::Error) -> String {
    let mut said = vec![error.to_string()];
    let mut source = std::error::Error::source(error);

    // Bounded: a cause chain is a linked list and a cycle would hang here.
    while let Some(inner) = source.filter(|_| said.len() < 4) {
        said.push(inner.to_string());
        source = std::error::Error::source(inner);
    }

    said.dedup();
    said.join(": ")
}

/// Whether a transport failure was the certificate rather than the network.
///
/// Matched on the text, because that is all a `reqwest::Error` offers: the
/// certificate failure is raised inside the TLS stack and arrives as a string
/// by the time it reaches here. Deliberately a short list of unambiguous
/// markers rather than anything clever — a false positive would tell somebody
/// their network is inspecting traffic when their wifi is simply off, which is
/// worse than the generic message it replaces.
///
/// The `revocation` entry is the one that was actually reported:
/// `CRYPT_E_NO_REVOCATION_CHECK`, from Windows refusing a certificate it could
/// not check a revocation list for.
#[must_use]
fn is_certificate_failure(because: &str) -> bool {
    const MARKERS: [&str; 6] = [
        "certificate",
        "cert_",
        "crypt_e_no_revocation",
        "unknownissuer",
        "certificate verify failed",
        "invalid peer certificate",
    ];

    let said = because.to_lowercase();

    MARKERS.iter().any(|marker| said.contains(marker))
}

impl Error {
    /// A failed request, named and explained.
    ///
    /// Also written to stderr, because `tauri dev` shows that stream and a
    /// person debugging this is usually looking at it — the same reason
    /// `engine.rs` reads the server's stderr rather than inheriting it.
    fn unreachable(url: &str, error: &reqwest::Error) -> Self {
        Self::from_transport(host_of(url), because(error))
    }

    /// Which failure a transport error was, given what the transport said.
    ///
    /// **Split out of [`Error::unreachable`] and pure, because the branch was
    /// otherwise untestable**: disabling the classification left the whole
    /// suite green, since one test covered [`is_certificate_failure`] and
    /// another covered the rendering, and nothing joined them. A
    /// `reqwest::Error` cannot be constructed by hand, so taking the string
    /// instead is what makes the wiring provable.
    pub(crate) fn from_transport(host: String, because: String) -> Self {
        if is_certificate_failure(&because) {
            eprintln!(
                "atlassian: {host} presented a certificate Chief could not verify: {because}"
            );

            return Self::Untrusted { host, because };
        }

        eprintln!("atlassian: {host} could not be reached: {because}");

        Self::Unreachable { host, because }
    }

    /// A request that was answered, but not with success.
    pub(crate) fn answered(url: &str, status: reqwest::StatusCode) -> Self {
        let host = host_of(url);

        eprintln!("atlassian: {host} answered {status}");

        Self::Answered {
            host,
            status: status.as_u16(),
        }
    }
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl From<loopback::Error> for Error {
    fn from(error: loopback::Error) -> Self {
        match error {
            loopback::Error::Refused(said) => Self::Refused(said),
            _ => Self::Rejected,
        }
    }
}

/// One Atlassian site this credential can reach.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Site {
    /// The cloud id, which every Jira and Confluence call is scoped by.
    pub id: String,
    /// `https://example.atlassian.net`.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub name: String,
}

/// One Jira issue, reduced to what a brief says about it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Issue {
    /// The human key, e.g. `PROJ-42`.
    pub key: String,
    pub summary: String,
    /// The workflow status' name, e.g. `In Progress`.
    pub status: String,
    /// Where to open it, when the site is known.
    pub url: Option<String>,
}

/// A sign-in in flight.
///
/// Carries the registration it minted, because that client id is the only one
/// that can spend the code the browser is about to bring back — and the only
/// one that can renew the token afterwards.
pub struct PendingLogin {
    pub listener: loopback::Listener,
    pub verifier: pkce::Verifier,
    pub state: pkce::State,
    pub redirect_uri: String,
    pub registration: Registered,
    pub token_endpoint: String,
}

/// What the registration endpoint handed back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered {
    pub client_id: String,
    /// Returned even for a client that registered as public, so it is kept
    /// rather than dropped — but only *sent* when the server said to
    /// authenticate with it. See [`Registered::authenticates`].
    pub client_secret: Option<String>,
    /// What the server echoed back for `token_endpoint_auth_method`.
    pub auth_method: String,
}

impl Registered {
    /// Whether the token request should carry the secret.
    ///
    /// `none` means the server registered this as a **public** client, and
    /// sending a secret to a public client's token endpoint is at best ignored
    /// and at worst an `invalid_client`. The secret is stored either way,
    /// because a user who brings their own 3LO app has a real one and the
    /// column is the same.
    #[must_use]
    pub fn authenticates(&self) -> bool {
        self.auth_method != "none" && self.client_secret.is_some()
    }
}

/// Where this authorization server's endpoints are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub issuer: String,
    pub authorize: String,
    pub token: String,
    pub registration: String,
}

/// Whether an issuer named by a discovery document may be followed.
///
/// Pure, so the hostile cases are tested directly rather than through a
/// server. Everything here is a way of looking like the real issuer without
/// being it: a suffix (`auth.atlassian.com.evil.test`), userinfo
/// (`https://auth.atlassian.com@evil.test`), a port, and plain HTTP.
///
/// Compared as whole **origins**, which is what makes the scheme part of the
/// answer rather than a separate check somebody can forget.
#[must_use]
pub fn trusted_issuer(issuer: &str, expected: &str) -> bool {
    match (origin_of(issuer), origin_of(expected)) {
        (Some(issuer), Some(expected)) => issuer.eq_ignore_ascii_case(&expected),
        _ => false,
    }
}

/// `scheme://authority`, or `None` for anything that is not a plain absolute
/// URL.
///
/// Userinfo would put the real host on the left of an `@` and the host that is
/// actually dialled on the right, so an authority containing one is refused
/// rather than parsed.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;

    if scheme.is_empty() {
        return None;
    }

    let authority = rest.split('/').next().unwrap_or_default();

    if authority.is_empty() || authority.contains('@') {
        return None;
    }

    Some(format!("{scheme}://{authority}"))
}

#[derive(Debug, Deserialize)]
struct ProtectedResource {
    #[serde(default)]
    authorization_servers: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AuthorizationServer {
    #[serde(default)]
    issuer: String,
    #[serde(default)]
    authorization_endpoint: String,
    #[serde(default)]
    token_endpoint: String,
    #[serde(default)]
    registration_endpoint: String,
}

#[derive(Debug, Deserialize)]
struct RegistrationReply {
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    token_endpoint_auth_method: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenReply {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

/// Reads Jira and Confluence for the account the user connected.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    /// Where the MCP server and its metadata live.
    host: String,
    /// The origin discovery is allowed to name.
    issuer_origin: String,
}

impl Client {
    /// A client pinned to Atlassian.
    pub fn new() -> Result<Self, Error> {
        Self::build(HOST.to_string(), ISSUER_ORIGIN.to_string())
    }

    /// A client pointed somewhere else, for tests only.
    ///
    /// `#[cfg(test)]`, the same shape as every other provider here, so a
    /// release build cannot be aimed anywhere but Atlassian.
    #[cfg(test)]
    pub(crate) fn against(host: &str) -> Result<Self, Error> {
        Self::build(host.to_string(), host.to_string())
    }

    /// A client whose discovery starts at one stub and is allowed to follow it
    /// to another, so the two hops can be told apart. `#[cfg(test)]`.
    #[cfg(test)]
    pub(crate) fn against_with_issuer(host: &str, issuer_origin: &str) -> Result<Self, Error> {
        Self::build(host.to_string(), issuer_origin.to_string())
    }

    fn build(host: String, issuer_origin: String) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(USER_AGENT)
            // No proxy, for the same reason every other credential-carrying
            // client here has none: a proxy is a third party to a conversation
            // that has two.
            .no_proxy()
            .build()
            .map_err(|error| Error::Unreachable {
                host: host_of(&host),
                because: error.to_string(),
            })?;

        Ok(Self {
            http,
            host,
            issuer_origin,
        })
    }

    fn mcp_url(&self) -> String {
        format!("{}/v1/mcp", self.host)
    }

    /// Find the authorization server, following one hop of discovery and no
    /// further.
    ///
    /// Two documents, because that is what RFC 9728 specifies: the protected
    /// resource names its authorization servers, and each of those describes
    /// its own endpoints. The issuer is checked against [`ISSUER_HOST`] before
    /// its metadata is fetched, so an untrusted document is never even read.
    pub async fn discover(&self) -> Result<Metadata, Error> {
        let resource: ProtectedResource = self
            .get_json(&format!(
                "{}/.well-known/oauth-protected-resource",
                self.host
            ))
            .await?;

        let issuer = resource
            .authorization_servers
            .into_iter()
            .next()
            .ok_or(Error::Decode)?;

        if !trusted_issuer(&issuer, &self.issuer_origin) {
            return Err(Error::UntrustedIssuer(issuer));
        }

        let server: AuthorizationServer = self
            .get_json(&format!(
                "{}/.well-known/oauth-authorization-server",
                issuer.trim_end_matches('/')
            ))
            .await?;

        // The issuer the metadata claims must be the one we asked, or the two
        // documents are describing different servers and neither can be
        // trusted about the other.
        if server.issuer.trim_end_matches('/') != issuer.trim_end_matches('/') {
            return Err(Error::UntrustedIssuer(server.issuer));
        }

        if server.registration_endpoint.is_empty()
            || server.authorization_endpoint.is_empty()
            || server.token_endpoint.is_empty()
        {
            return Err(Error::Decode);
        }

        Ok(Metadata {
            issuer,
            authorize: server.authorization_endpoint,
            token: server.token_endpoint,
            registration: server.registration_endpoint,
        })
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, Error> {
        let response = self
            .http
            .get(url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|error| Error::unreachable(url, &error))?;

        let status = response.status();

        if !status.is_success() {
            return Err(Error::answered(url, status));
        }

        response.json().await.map_err(|_| Error::Decode)
    }

    /// Mint a client for this installation.
    ///
    /// Registers as a **public** client — `token_endpoint_auth_method: "none"`
    /// — with exactly one redirect URI, the loopback the caller has already
    /// bound. What comes back is echoed rather than assumed: Atlassian returns
    /// a `client_secret` even here, and whether to send it is
    /// [`Registered::authenticates`]'s to decide from the method the server
    /// confirmed, not from whether a secret happens to be present.
    pub async fn register(
        &self,
        registration_endpoint: &str,
        redirect_uri: &str,
    ) -> Result<Registered, Error> {
        let body = serde_json::json!({
            "client_name": "Chief",
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "application_type": "native",
            "scope": SCOPES.join(" "),
        });

        let response = self
            .http
            .post(registration_endpoint)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|error| Error::unreachable(registration_endpoint, &error))?;

        let status = response.status();
        let reply: RegistrationReply = response.json().await.map_err(|_| Error::Decode)?;

        if let Some(error) = reply.error {
            return Err(Error::Registration(
                reply.error_description.unwrap_or(error),
            ));
        }

        if !status.is_success() || reply.client_id.is_empty() {
            return Err(Error::Registration(format!("it answered {status}")));
        }

        Ok(Registered {
            client_id: reply.client_id,
            client_secret: reply.client_secret,
            auth_method: reply
                .token_endpoint_auth_method
                .unwrap_or_else(|| "none".to_string()),
        })
    }

    /// Bind a port, register a client for it, and say where to send the user.
    ///
    /// In that order, and the order is the design: see the module docs. The
    /// listener stays bound for the whole flow, so the port named in the
    /// registration is one this process still holds when the browser returns.
    pub async fn start_login(&self) -> Result<(PendingLogin, String), Error> {
        let metadata = self.discover().await?;

        let listener = loopback::Listener::bind().await?;
        let redirect_uri = listener.redirect_uri();

        let registration = self.register(&metadata.registration, &redirect_uri).await?;

        let verifier = pkce::Verifier::generate();
        let state = pkce::State::generate();

        let url = authorization_url(
            &metadata.authorize,
            &registration.client_id,
            &redirect_uri,
            &verifier.challenge(),
            state.as_str(),
        );

        Ok((
            PendingLogin {
                listener,
                verifier,
                state,
                redirect_uri,
                registration,
                token_endpoint: metadata.token,
            },
            url,
        ))
    }

    /// Wait for the browser to come back, then trade the code for tokens.
    pub async fn finish_login(&self, pending: PendingLogin) -> Result<(Tokens, Registered), Error> {
        let redirect = pending.listener.wait().await?;

        // Compared before the code is spent: a redirect whose state does not
        // match is not the flow this process started.
        if !pending.state.matches(&redirect.state) {
            return Err(Error::Refused(
                "the sign-in came back with a state Chief did not send".to_string(),
            ));
        }

        let mut form = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", redirect.code.clone()),
            ("redirect_uri", pending.redirect_uri.clone()),
            ("code_verifier", pending.verifier.as_str().to_string()),
            ("client_id", pending.registration.client_id.clone()),
            ("resource", RESOURCE.to_string()),
        ];

        if pending.registration.authenticates() {
            if let Some(secret) = pending.registration.client_secret.as_ref() {
                form.push(("client_secret", secret.clone()));
            }
        }

        let tokens = self.exchange(&pending.token_endpoint, form).await?;

        Ok((tokens, pending.registration))
    }

    /// Swap a refresh token for a fresh pair.
    ///
    /// Discovery again rather than a stored endpoint: the token URL is a fact
    /// about the server, not about the account, and re-reading it is one cheap
    /// request against a stale value that would fail every renewal at once.
    pub async fn refresh(&self, client_id: &str, refresh_token: &str) -> Result<Tokens, Error> {
        let metadata = self.discover().await?;

        let form = vec![
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh_token.to_string()),
            ("client_id", client_id.to_string()),
            ("resource", RESOURCE.to_string()),
        ];

        self.exchange(&metadata.token, form).await
    }

    async fn exchange(
        &self,
        token_endpoint: &str,
        form: Vec<(&str, String)>,
    ) -> Result<Tokens, Error> {
        let response = self
            .http
            .post(token_endpoint)
            .form(&form)
            .send()
            .await
            .map_err(|error| Error::unreachable(token_endpoint, &error))?;

        let status = response.status();
        let reply: TokenReply = response.json().await.map_err(|_| Error::Decode)?;

        if let Some(error) = reply.error {
            // `invalid_grant` is the credential being gone rather than the
            // host being unreachable, and the two read very differently on
            // screen — one sends the user to reconnect, the other says to try
            // later. See `is_token_rejected`.
            if error == "invalid_grant" || error == "invalid_client" {
                return Err(Error::Rejected);
            }

            return Err(Error::Refused(reply.error_description.unwrap_or(error)));
        }

        if !status.is_success() {
            return Err(Error::answered(token_endpoint, status));
        }

        Ok(Tokens {
            access_token: reply.access_token.ok_or(Error::Decode)?,
            refresh_token: reply.refresh_token,
            expires_in: reply.expires_in,
        })
    }
}

/// Where to send the browser.
///
/// `resource` is RFC 8707 and MCP's authorization spec requires it on both the
/// authorize and the token request: it names which protected resource the
/// token is for, so a token minted for the MCP server cannot be replayed
/// against something else on the same issuer.
#[must_use]
pub fn authorization_url(
    authorize: &str,
    client_id: &str,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> String {
    let query = [
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("scope", &SCOPES.join(" ")),
        ("state", state),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("resource", RESOURCE),
        // Without this Atlassian may return a token with no refresh token, and
        // the first expiry would send the user back to a browser.
        ("prompt", "consent"),
    ]
    .iter()
    .map(|(name, value)| format!("{name}={}", urlencode(value)))
    .collect::<Vec<_>>()
    .join("&");

    format!("{authorize}?{query}")
}

/// Percent-encode a query parameter value.
///
/// Hand-rolled rather than a dependency, the same as `ingest::canonical`: the
/// inputs are a scope list, a loopback URL and two base64url nonces, and
/// pulling in a crate to escape a space and a colon would be a dependency for
/// a `match`.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());

    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }

    out
}

/// One MCP conversation, held open only as long as a read takes.
///
/// The server is stateful enough to want an `initialize` before anything else
/// and to hand back a session id, so this exists to carry that id — but it is
/// built per read and dropped afterwards rather than kept alive. Chief asks
/// Atlassian a question every half hour at most; a session held open between
/// those is a connection to somebody else's server that exists for no reason.
pub struct Mcp<'a> {
    client: &'a Client,
    token: String,
    session: Option<String>,
    /// JSON-RPC ids are per connection, so a counter is enough and it never
    /// has to be unique across conversations. Atomic rather than a `Cell`
    /// because this rides inside a Tauri command's future, which must be
    /// `Send`.
    next_id: std::sync::atomic::AtomicU64,
}

impl Client {
    /// Open a conversation: `initialize`, then the notification that says the
    /// client is ready.
    ///
    /// **The capabilities sent are empty**, which is the point — see
    /// [`capabilities`].
    pub async fn connect(&self, token: &str) -> Result<Mcp<'_>, Error> {
        let mut mcp = Mcp {
            client: self,
            token: token.to_string(),
            session: None,
            next_id: std::sync::atomic::AtomicU64::new(1),
        };

        let (value, session) = mcp
            .request(
                "initialize",
                serde_json::json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": capabilities(),
                    "clientInfo": { "name": "Chief", "version": env!("CARGO_PKG_VERSION") },
                }),
                true,
            )
            .await?;

        // A server that answers `initialize` with an error is one this client
        // cannot speak to, and every later call would fail the same way.
        if value.get("protocolVersion").is_none() {
            return Err(Error::Decode);
        }

        mcp.session = session;

        // Fire and forget by design: `notifications/initialized` has no id and
        // therefore no reply, and a server that ignores it is within spec.
        mcp.notify("notifications/initialized").await;

        Ok(mcp)
    }
}

impl Mcp<'_> {
    /// Call one tool, by a name that must be on the allowlist.
    ///
    /// The check comes **before** anything is built or sent, so a refused name
    /// costs no request. That is the difference between a rule and a
    /// preference, and a test asserts the stub server saw nothing.
    pub async fn call(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, Error> {
        if !DETERMINISTIC.contains(&name) {
            return Err(Error::NotDeterministic(name.to_string()));
        }

        let (value, _) = self
            .request(
                "tools/call",
                serde_json::json!({ "name": name, "arguments": arguments }),
                false,
            )
            .await?;

        // An MCP tool reports its own failure inside a successful response, so
        // status alone says nothing — the same trap GraphQL sets in
        // `linear::read`.
        if value.get("isError").and_then(serde_json::Value::as_bool) == Some(true) {
            return Err(Error::Tool(text_of(&value).unwrap_or_default()));
        }

        Ok(value)
    }

    async fn notify(&self, method: &str) {
        let body = serde_json::json!({ "jsonrpc": "2.0", "method": method });

        // Nothing to do about a failure here: the notification carries no id,
        // so there is no reply to miss, and the call that follows will report
        // anything that actually matters.
        let _ = self.post(&body).await;
    }

    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
        want_session: bool,
    ) -> Result<(serde_json::Value, Option<String>), Error> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let response = self.post(&body).await?;
        let status = response.status();

        let session = want_session
            .then(|| {
                response
                    .headers()
                    .get("Mcp-Session-Id")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .flatten();

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();

        if status == 401 || status == 403 {
            return Err(Error::Rejected);
        }

        let body = response
            .text()
            .await
            .map_err(|error| Error::unreachable(&self.client.mcp_url(), &error))?;

        if !status.is_success() {
            return Err(Error::answered(&self.client.mcp_url(), status));
        }

        let envelope = envelope(&content_type, &body)?;

        if let Some(error) = envelope.get("error") {
            let said = error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("it did not say why");

            return Err(Error::Tool(said.to_string()));
        }

        let result = envelope.get("result").ok_or(Error::Decode)?.clone();

        Ok((result, session))
    }

    async fn post(&self, body: &serde_json::Value) -> Result<reqwest::Response, Error> {
        let mut request = self
            .client
            .http
            .post(self.client.mcp_url())
            .bearer_auth(&self.token)
            .header("Content-Type", "application/json")
            // Streamable HTTP lets the server answer either way, so both are
            // accepted and `envelope` sorts out which arrived.
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .json(body);

        if let Some(session) = self.session.as_ref() {
            request = request.header("Mcp-Session-Id", session);
        }

        request
            .send()
            .await
            .map_err(|error| Error::unreachable(&self.client.mcp_url(), &error))
    }
}

/// The JSON-RPC envelope, whichever way the server chose to send it.
///
/// Streamable HTTP allows a plain JSON body or a server-sent event stream for
/// the same request, and Atlassian's answer depends on the tool. In the stream
/// case the payload is on a `data:` line; the first one that parses is taken,
/// because this client sends one request at a time and never batches.
fn envelope(content_type: &str, body: &str) -> Result<serde_json::Value, Error> {
    if !content_type.contains("text/event-stream") {
        return serde_json::from_str(body).map_err(|_| Error::Decode);
    }

    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .find_map(|payload| serde_json::from_str::<serde_json::Value>(payload.trim()).ok())
        .ok_or(Error::Decode)
}

/// The text an MCP tool result carries, if it carries any.
///
/// A result is `content: [{type, text}]`, and Atlassian puts a JSON document
/// in that text rather than using `structuredContent`. Both are tried, newest
/// shape first, so a server that moves to the structured field keeps working.
fn text_of(result: &serde_json::Value) -> Option<String> {
    if let Some(structured) = result.get("structuredContent") {
        return Some(structured.to_string());
    }

    result
        .get("content")?
        .as_array()?
        .iter()
        .find_map(|part| part.get("text")?.as_str().map(str::to_string))
}

/// A tool result, parsed back out of the text it arrived in.
fn payload(result: &serde_json::Value) -> Result<serde_json::Value, Error> {
    let text = text_of(result).ok_or(Error::Decode)?;

    serde_json::from_str(&text).map_err(|_| Error::Decode)
}

impl Mcp<'_> {
    /// Which sites this credential can reach.
    ///
    /// Sorted by cloud id so the answer is stable: the first one names the
    /// account, and an order that depended on what Atlassian felt like
    /// returning would rename it on some later sync.
    pub async fn sites(&self) -> Result<Vec<Site>, Error> {
        let result = self
            .call(ACCESSIBLE_RESOURCES, serde_json::json!({}))
            .await?;

        let mut sites: Vec<Site> =
            serde_json::from_value(payload(&result)?).map_err(|_| Error::Decode)?;

        sites.sort_by(|left, right| left.id.cmp(&right.id));

        Ok(sites)
    }

    /// What is assigned to this person on one site and not finished.
    ///
    /// The JQL is [`ASSIGNED_JQL`] — a constant, built here, containing
    /// nothing the user typed.
    pub async fn assigned(&self, site: &Site) -> Result<Vec<Issue>, Error> {
        let result = self
            .call(
                SEARCH_JIRA,
                serde_json::json!({
                    "cloudId": site.id,
                    "jql": ASSIGNED_JQL,
                    "maxResults": READ_LIMIT,
                    "fields": ["summary", "status"],
                }),
            )
            .await?;

        Ok(issues(&payload(&result)?, site))
    }
}

/// Jira's search response, reduced to what a brief says.
///
/// Deliberately dug out of a `Value` rather than deserialised into a struct.
/// Jira's search has more than one response shape across API versions — the
/// issues have been under `issues` and the fields flattened or nested — and a
/// strict struct turns a shape Chief has not seen into no issues at all with
/// nothing said. Anything unrecognised is skipped and the rest still arrives.
#[must_use]
fn issues(value: &serde_json::Value, site: &Site) -> Vec<Issue> {
    let found = value
        .get("issues")
        .and_then(serde_json::Value::as_array)
        .or_else(|| value.as_array());

    let Some(found) = found else {
        return Vec::new();
    };

    found
        .iter()
        .filter_map(|issue| {
            let key = issue.get("key")?.as_str()?.to_string();
            let fields = issue.get("fields");

            let summary = fields
                .and_then(|fields| fields.get("summary"))
                .or_else(|| issue.get("summary"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();

            let status = fields
                .and_then(|fields| fields.get("status"))
                .or_else(|| issue.get("status"))
                .and_then(|status| status.get("name").or(Some(status)))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();

            let url = (!site.url.is_empty())
                .then(|| format!("{}/browse/{key}", site.url.trim_end_matches('/')));

            Some(Issue {
                key,
                summary,
                status,
                url,
            })
        })
        .collect()
}

/// One issue, as a line in the brief. The same shape as `linear::describe`.
#[must_use]
pub fn describe(issue: &Issue) -> String {
    let status = if issue.status.is_empty() {
        String::new()
    } else {
        format!(" [{}]", issue.status)
    };

    format!("{} {}{status}", issue.key, issue.summary)
}

impl crate::oauth::Provider for Client {
    const SERVICE: &'static str = crate::integrations::ATLASSIAN;

    type Error = Error;

    fn endpoints(&self) -> Endpoints {
        // Discovered rather than pinned, which is the whole shape of this
        // provider: the endpoints are read from the authorization server's own
        // metadata at every sign-in and every renewal. `Endpoints` wants
        // `&'static str`, so what it can honestly report is where discovery
        // starts.
        Endpoints {
            authorize: HOST,
            token: HOST,
            device_code: None,
        }
    }

    /// There is no such thing for this provider.
    ///
    /// Every other provider answers with a registration the build carried or
    /// the user pasted. Atlassian's client id belongs to **one account**,
    /// because it was minted for that sign-in, and it lives in
    /// `integration_accounts.client_id`. Answering with anything here would be
    /// answering with somebody else's registration.
    fn client_id(&self) -> Result<String, Self::Error> {
        Err(Error::NotConnected)
    }

    fn environment_client_id(&self) -> Option<String> {
        None
    }

    fn scopes(&self) -> &'static [&'static str] {
        &SCOPES
    }

    async fn refresh(&self, client_id: &str, refresh_token: &str) -> Result<Tokens, Self::Error> {
        Client::refresh(self, client_id, refresh_token).await
    }

    fn is_token_rejected(error: &Self::Error) -> bool {
        matches!(error, Error::Rejected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llama::test_support::{reserve, serve, serve_on};

    /// What the server says to `initialize`.
    fn initialized() -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "atlassian", "version": "1" },
            },
        })
        .to_string()
    }

    /// A `tools/call` reply carrying `payload` as the text an Atlassian tool
    /// returns — a JSON document inside a string, which is what the real
    /// server does rather than using `structuredContent`.
    fn tool_reply(payload: serde_json::Value) -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "content": [{ "type": "text", "text": payload.to_string() }],
                "isError": false,
            },
        })
        .to_string()
    }

    fn one_site() -> serde_json::Value {
        serde_json::json!([
            { "id": "cloud-2", "url": "https://beta.atlassian.net", "name": "Beta" },
            { "id": "cloud-1", "url": "https://alpha.atlassian.net", "name": "Alpha" },
        ])
    }

    // ---------------------------------------------------------------------
    // Guards. Each of these was watched failing before it was kept; the
    // breach and the message it produced are in the pull request.
    // ---------------------------------------------------------------------

    /// **The `sampling` guard.** A server that holds this capability can ask
    /// the client to run a completion, which on this machine means Atlassian
    /// driving the user's own model. Asserted against the bytes that went out
    /// rather than against [`capabilities`], because the constant being empty
    /// says nothing about what `connect` actually sends.
    #[tokio::test]
    async fn never_declares_a_capability_to_the_server() {
        let (host, server) = serve(vec![
            ("HTTP/1.1 200 OK", initialized()),
            ("HTTP/1.1 200 OK", "{}".to_string()),
        ]);
        let client = Client::against(&host).expect("client");

        client.connect("token").await.expect("should initialize");

        let sent = server.await.expect("server");

        assert!(
            !sent.is_empty(),
            "the client should have sent an initialize; an empty list proves nothing"
        );

        let initialize = sent
            .iter()
            .find(|request| request.contains("\"initialize\""))
            .expect("an initialize should have been sent");

        let body: serde_json::Value = serde_json::from_str(
            initialize
                .split_once("\r\n\r\n")
                .expect("request should have a body")
                .1,
        )
        .expect("the body should be JSON");

        assert_eq!(
            body["params"]["capabilities"],
            serde_json::json!({}),
            "capabilities must go out empty, not merely without sampling"
        );
        assert!(
            !initialize.contains("sampling"),
            "sampling must not appear anywhere in what was sent"
        );
    }

    /// **The deterministic-tools guard.** `searchAtlassian` is the Rovo
    /// natural-language layer: calling it would send whatever it was given to
    /// Atlassian, and the sidebar promises on every screen that nothing the
    /// user types leaves this machine.
    ///
    /// **A legitimate call follows the refused one, and that is load-bearing.**
    /// The first version of this test refused a tool and then asserted the
    /// server had seen two requests — and it passed with the check moved
    /// *after* the request was sent, because the stub had already stopped
    /// accepting and the leaked request was refused by the operating system
    /// rather than recorded. A third reply the allowed call consumes is what
    /// makes a leaked request land somewhere the assertion can see it.
    #[tokio::test]
    async fn refuses_a_natural_language_tool_without_asking_atlassian() {
        let (host, server) = serve(vec![
            ("HTTP/1.1 200 OK", initialized()),
            ("HTTP/1.1 200 OK", "{}".to_string()),
            ("HTTP/1.1 200 OK", tool_reply(one_site())),
        ]);
        let client = Client::against(&host).expect("client");
        let mcp = client.connect("token").await.expect("should initialize");

        let error = mcp
            .call(
                "searchAtlassian",
                serde_json::json!({ "query": "anything" }),
            )
            .await
            .expect_err("the natural-language tool must be refused");

        assert!(matches!(error, Error::NotDeterministic(_)), "{error:?}");

        // The allowed call takes the third reply. If the refusal had leaked a
        // request, this one would find nothing left to answer it.
        mcp.call(ACCESSIBLE_RESOURCES, serde_json::json!({}))
            .await
            .expect("an allowed tool must still be callable");

        let sent = server.await.expect("server");

        assert_eq!(
            sent.len(),
            3,
            "initialize, its notification, and the allowed call — nothing else: {sent:?}"
        );
        assert!(
            !sent
                .iter()
                .any(|request| request.contains("searchAtlassian")),
            "the refused name must never reach the wire"
        );
        assert!(
            sent[2].contains(ACCESSIBLE_RESOURCES),
            "the third request must be the allowed call, not a leaked one: {}",
            sent[2]
        );
    }

    /// The same rule stated as a property of the list, so a tool added to
    /// [`DETERMINISTIC`] without thought trips here too.
    #[test]
    fn the_allowlist_holds_no_natural_language_tool() {
        for name in DETERMINISTIC {
            assert!(
                !name.eq_ignore_ascii_case("searchAtlassian")
                    && !name.eq_ignore_ascii_case("fetchAtlassian"),
                "{name} is Atlassian's natural-language layer"
            );
        }
    }

    /// **The read-only guard.** Atlassian's write scopes sit on the same
    /// resource as its read scopes, so asking for none of them is what makes
    /// read-only a property of the grant rather than a promise about this
    /// code. D4 is where forfeiting it would be argued.
    #[test]
    fn asks_for_no_write_scope() {
        for scope in SCOPES {
            assert!(
                !scope.starts_with("write:"),
                "{scope} would forfeit read-only at the authorization server"
            );
        }

        assert!(
            SCOPES.contains(&"offline_access"),
            "without this the first expiry sends the user back to a browser"
        );

        // Not an oversight: a consent screen naming Confluence for a read no
        // code path makes is the thing this project criticises `repo` for.
        // Adding the scope and the read together is what removes this.
        assert!(
            !SCOPES.iter().any(|scope| scope.contains("confluence")),
            "Confluence's scopes arrive with the read that uses them"
        );
    }

    /// **The credential guard.** These strings reach the screen and the log.
    #[test]
    fn no_error_carries_a_credential() {
        let secret = "atl-token-do-not-print";

        let errors = [
            Error::NotConnected,
            Error::Unreachable {
                host: "mcp.atlassian.com".to_string(),
                because: secret.to_string(),
            },
            Error::Answered {
                host: "mcp.atlassian.com".to_string(),
                status: 403,
            },
            Error::Untrusted {
                host: "mcp.atlassian.com".to_string(),
                because: secret.to_string(),
            },
            Error::Decode,
            Error::Refused(secret.to_string()),
            Error::Rejected,
            Error::Registration(secret.to_string()),
            Error::UntrustedIssuer(secret.to_string()),
            Error::NotDeterministic(secret.to_string()),
            Error::Tool(secret.to_string()),
        ];

        // The variants that take a string are handed one here on purpose:
        // they carry what the *server* or the transport said, and the point of
        // the test is that nothing carries what Chief *holds*. Those are the
        // variants with no free-text field at all, which is the design — so
        // this asserts the shape, and the test below asserts the thing that
        // actually matters against a real failure.
        for error in errors {
            let rendered = error.to_string();
            let quotes_what_it_was_told = matches!(
                error,
                Error::Refused(_)
                    | Error::Registration(_)
                    | Error::UntrustedIssuer(_)
                    | Error::NotDeterministic(_)
                    | Error::Tool(_)
                    | Error::Unreachable { .. }
                    | Error::Untrusted { .. }
            );

            assert_eq!(
                rendered.contains(secret),
                quotes_what_it_was_told,
                "only what the server or the transport said may be quoted: {rendered}"
            );
        }
    }

    /// **A nameless request is what shipped**, because `reqwest` sends no
    /// `User-Agent` unless one is set and this module never set one. Asserted
    /// against the bytes on the wire rather than against the constant.
    #[tokio::test]
    async fn names_itself_on_every_request() {
        let (host, server) = serve(vec![
            ("HTTP/1.1 200 OK", initialized()),
            ("HTTP/1.1 200 OK", "{}".to_string()),
        ]);
        let client = Client::against(&host).expect("client");

        client.connect("token").await.expect("should initialize");

        let sent = server.await.expect("server");

        assert!(!sent.is_empty(), "nothing was sent, so this proves nothing");
        // Lowercased: `HeaderName` normalises, so hyper puts `user-agent` on
        // the wire whatever case it was given.
        assert!(
            sent[0].to_lowercase().contains("user-agent: chief-ai/"),
            "every request should name Chief: {}",
            sent[0]
        );
    }

    /// **The real strings, including the one that was reported.** Windows
    /// raised `CRYPT_E_NO_REVOCATION_CHECK` on a certificate substituted by
    /// something between the machine and Atlassian; rustls and OpenSSL word
    /// the same class of failure differently, so all three are here.
    #[test]
    fn tells_a_certificate_failure_from_a_network_one() {
        for certificate in [
            "schannel: next InitializeSecurityContext failed: CRYPT_E_NO_REVOCATION_CHECK \
             (0x80092012) - The revocation function was unable to check revocation for the \
             certificate.",
            "invalid peer certificate: UnknownIssuer",
            "certificate verify failed: unable to get local issuer certificate",
            "invalid peer certificate: Expired",
        ] {
            assert!(
                is_certificate_failure(certificate),
                "should read as a certificate failure: {certificate}"
            );
        }

        // A false positive here would tell somebody their network is
        // inspecting traffic when their wifi is off, which is worse than the
        // generic message it replaces.
        for network in [
            "dns error: failed to lookup address information: Name or service not known",
            "tcp connect error: Connection refused (os error 111)",
            "operation timed out",
            "connection closed before message completed",
        ] {
            assert!(
                !is_certificate_failure(network),
                "should stay a plain transport failure: {network}"
            );
        }
    }

    /// **The wiring, not the pieces.** `is_certificate_failure` and the
    /// rendering each had a test; neither noticed when the branch joining them
    /// was disabled. This drives the function every failed request goes
    /// through.
    #[test]
    fn a_failed_request_is_sorted_into_the_right_failure() {
        let reported = "schannel: next InitializeSecurityContext failed: \
                        CRYPT_E_NO_REVOCATION_CHECK (0x80092012) - The revocation function was \
                        unable to check revocation for the certificate.";

        assert!(
            matches!(
                Error::from_transport("mcp.atlassian.com".to_string(), reported.to_string()),
                Error::Untrusted { .. }
            ),
            "the reported failure should read as a certificate, not as the network"
        );

        assert!(
            matches!(
                Error::from_transport(
                    "mcp.atlassian.com".to_string(),
                    "tcp connect error: Connection refused (os error 111)".to_string(),
                ),
                Error::Unreachable { .. }
            ),
            "and a refused connection should stay a plain transport failure"
        );
    }

    /// The variant exists to be actionable, so the words are part of it.
    #[test]
    fn a_certificate_failure_says_what_to_suspect() {
        let rendered = Error::Untrusted {
            host: "mcp.atlassian.com".to_string(),
            because: "invalid peer certificate: UnknownIssuer".to_string(),
        }
        .to_string();

        assert!(rendered.contains("mcp.atlassian.com"), "{rendered}");
        assert!(rendered.contains("certificate"), "{rendered}");
        assert!(
            rendered.contains("inspecting HTTPS traffic"),
            "the actionable half is the point: {rendered}"
        );
    }

    /// Named without the path, and never with a query string — a code or a
    /// token would be in one if anything ever put it there.
    #[test]
    fn names_the_host_without_the_rest_of_the_url() {
        assert_eq!(host_of("https://mcp.atlassian.com"), "mcp.atlassian.com");
        assert_eq!(
            host_of("https://mcp.atlassian.com/.well-known/oauth-protected-resource"),
            "mcp.atlassian.com"
        );
        assert_eq!(
            host_of("https://auth.atlassian.com/VCeDsk/dcr/register?code=secret"),
            "auth.atlassian.com"
        );
        assert_eq!(
            host_of("http://127.0.0.1:49512/callback"),
            "127.0.0.1:49512"
        );
    }

    /// **The guard that matters, against a real failure rather than a
    /// hand-built one.**
    ///
    /// `Error::Unreachable` now quotes the transport's own words, and the one
    /// request that carries a bearer token is the MCP call. A `reqwest::Error`
    /// holds a URL and an I/O cause and never headers — but that is a fact
    /// about a dependency, which is exactly the kind of fact that changes
    /// under you. So this drives a real failing request with a real token and
    /// reads what reaches the screen.
    ///
    /// Port 1 is never listening, so the failure is immediate and is the
    /// connection itself rather than anything Atlassian said.
    #[tokio::test]
    async fn a_failed_request_never_renders_the_token_it_carried() {
        let token = "atl-bearer-do-not-print";
        let client = Client::against("http://127.0.0.1:1").expect("client");

        let Err(error) = client.connect(token).await else {
            panic!("nothing is listening on port 1, so this cannot succeed");
        };

        let rendered = error.to_string();

        assert!(
            matches!(error, Error::Unreachable { .. }),
            "a refused connection is unreachable, not something Atlassian said: {error:?}"
        );
        assert!(
            !rendered.contains(token),
            "the bearer token reached the screen: {rendered}"
        );
        // Asserted as the *opening* of the message rather than as "appears
        // somewhere", which was the first version and proved nothing: removing
        // `{host}` from the Display left it green, because `because` embeds
        // reqwest's own URL and that contains the host too. Two things say it;
        // only this pins the one the message is built from.
        assert!(
            rendered.starts_with("127.0.0.1:1 "),
            "the message should open by naming where it was going: {rendered}"
        );
    }

    // ---------------------------------------------------------------------
    // Discovery, and the host pinning that bounds it.
    // ---------------------------------------------------------------------

    #[test]
    fn follows_discovery_only_to_the_issuer_it_expects() {
        assert!(trusted_issuer(
            "https://auth.atlassian.com/VCeDsk8ZHncYF1g234fKtc4lNipbBhu3",
            ISSUER_ORIGIN
        ));
        assert!(trusted_issuer(
            "https://AUTH.ATLASSIAN.COM/x",
            ISSUER_ORIGIN
        ));
    }

    /// Every way of looking like the issuer without being it. One MCP server
    /// surveyed for the design spec published forged metadata naming an issuer
    /// it did not own, which is why this is a list rather than a `contains`.
    #[test]
    fn refuses_an_issuer_that_merely_resembles_atlassian() {
        for hostile in [
            "https://auth.atlassian.com.evil.test/x",
            "https://evil.test/auth.atlassian.com",
            "https://auth.atlassian.com@evil.test/x",
            "https://evil-auth.atlassian.com.co/x",
            "http://auth.atlassian.com/x",
            "https://auth.atlassian.com:8443/x",
            "//auth.atlassian.com/x",
            "",
        ] {
            assert!(
                !trusted_issuer(hostile, ISSUER_ORIGIN),
                "{hostile} must not be followed"
            );
        }
    }

    /// An authorization server's metadata, served from its own address.
    fn metadata_at(issuer: &str) -> String {
        serde_json::json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/oauth/token"),
            "registration_endpoint": format!("{issuer}/dcr/register"),
        })
        .to_string()
    }

    /// Two hops, told apart by being two servers: the resource document is
    /// served by one and the authorization server's metadata by another, so a
    /// client that read the wrong document would ask the wrong port.
    ///
    /// The issuer's port is reserved before either body is written, because
    /// both have to name it — see [`reserve`].
    #[tokio::test]
    async fn discovery_reads_the_resource_then_its_authorization_server() {
        let (issuer, port) = reserve();
        let issuer_server = serve_on(port, vec![("HTTP/1.1 200 OK", metadata_at(&issuer))]);

        let (host, resource_server) = serve(vec![(
            "HTTP/1.1 200 OK",
            serde_json::json!({ "authorization_servers": [issuer.clone()] }).to_string(),
        )]);

        let client = Client::against_with_issuer(&host, &issuer).expect("client");

        let found = client.discover().await.expect("should discover");

        assert_eq!(found.registration, format!("{issuer}/dcr/register"));
        assert_eq!(found.token, format!("{issuer}/oauth/token"));

        let asked_resource = resource_server.await.expect("server");
        let asked_issuer = issuer_server.await.expect("server");

        assert_eq!(asked_resource.len(), 1, "one hop, not more");
        assert!(asked_resource[0].contains("/.well-known/oauth-protected-resource"));
        assert_eq!(
            asked_issuer.len(),
            1,
            "the issuer named by the document is the one asked"
        );
        assert!(asked_issuer[0].contains("/.well-known/oauth-authorization-server"));
    }

    /// Two documents describing different servers means neither can be trusted
    /// about the other, however the first one got here.
    #[tokio::test]
    async fn refuses_metadata_that_names_a_different_issuer() {
        let (issuer, port) = reserve();
        let _issuer_server = serve_on(
            port,
            vec![(
                "HTTP/1.1 200 OK",
                metadata_at("https://somewhere.else.test/as"),
            )],
        );

        let (host, _resource_server) = serve(vec![(
            "HTTP/1.1 200 OK",
            serde_json::json!({ "authorization_servers": [issuer.clone()] }).to_string(),
        )]);

        let client = Client::against_with_issuer(&host, &issuer).expect("client");

        let error = client.discover().await.expect_err("should refuse");

        assert!(matches!(error, Error::UntrustedIssuer(_)), "{error:?}");
    }

    /// An untrusted issuer must be refused *before* its metadata is fetched:
    /// reading a hostile document is already too far.
    ///
    /// **The untrusted issuer is a port this test holds and never answers on**,
    /// which is what makes the refusal observable. Pointing it at a name that
    /// does not resolve would fail whether or not the check exists — the error
    /// would just be a transport failure instead — and the test would be
    /// asserting the network's behaviour rather than Chief's. Holding the
    /// listener without serving it means a connection that should never happen
    /// lands in the backlog, where `accept` finds it.
    #[tokio::test]
    async fn never_fetches_metadata_from_an_untrusted_issuer() {
        let (untrusted, listener) = reserve();

        let (host, resource_server) = serve(vec![(
            "HTTP/1.1 200 OK",
            serde_json::json!({ "authorization_servers": [untrusted] }).to_string(),
        )]);

        // The client expects the resource's own origin; the document names
        // another. Everything else about the document is well formed.
        let client = Client::against_with_issuer(&host, &host).expect("client");

        let error = client.discover().await.expect_err("should refuse");

        assert!(matches!(error, Error::UntrustedIssuer(_)), "{error:?}");
        assert!(
            listener.accept().is_err(),
            "nothing should have connected to the issuer the document named"
        );

        let sent = resource_server.await.expect("server");

        assert_eq!(
            sent.len(),
            1,
            "only the resource document should have been read, got: {sent:?}"
        );
    }

    // ---------------------------------------------------------------------
    // Registration.
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn registers_as_a_public_client_for_the_port_it_bound() {
        let reply = serde_json::json!({
            "client_id": "minted-id",
            "client_secret": "minted-secret",
            "token_endpoint_auth_method": "none",
        })
        .to_string();

        let (host, server) = serve(vec![("HTTP/1.1 201 Created", reply)]);
        let client = Client::against(&host).expect("client");

        let registered = client
            .register(
                &format!("{host}/dcr/register"),
                "http://127.0.0.1:49512/callback",
            )
            .await
            .expect("should register");

        assert_eq!(registered.client_id, "minted-id");
        assert_eq!(
            registered.client_secret.as_deref(),
            Some("minted-secret"),
            "kept, because the column holds a brought-your-own secret too"
        );
        assert!(
            !registered.authenticates(),
            "a client the server registered as public must not send a secret"
        );

        let sent = server.await.expect("server").join("");

        assert!(sent.contains("\"token_endpoint_auth_method\":\"none\""));
        assert!(
            sent.contains("127.0.0.1%3A49512") || sent.contains("127.0.0.1:49512"),
            "the redirect registered must name the port already bound: {sent}"
        );
        assert!(
            !sent.contains("write:"),
            "registration asks for the read-only scopes"
        );
    }

    /// A server that upgrades the client to confidential is believed, because
    /// sending no secret to a confidential client fails every token request.
    #[test]
    fn sends_the_secret_only_when_the_server_asked_for_one() {
        let public = Registered {
            client_id: "id".to_string(),
            client_secret: Some("secret".to_string()),
            auth_method: "none".to_string(),
        };
        let confidential = Registered {
            auth_method: "client_secret_post".to_string(),
            ..public.clone()
        };
        let neither = Registered {
            client_secret: None,
            ..confidential.clone()
        };

        assert!(!public.authenticates());
        assert!(confidential.authenticates());
        assert!(!neither.authenticates());
    }

    #[tokio::test]
    async fn says_so_when_atlassian_will_not_register_this_copy() {
        let (host, _server) = serve(vec![(
            "HTTP/1.1 400 Bad Request",
            r#"{"error":"invalid_redirect_uri","error_description":"not allowed"}"#,
        )]);
        let client = Client::against(&host).expect("client");

        let error = client
            .register(&format!("{host}/dcr/register"), "http://127.0.0.1:1/cb")
            .await
            .expect_err("should refuse");

        assert!(
            matches!(&error, Error::Registration(said) if said.contains("not allowed")),
            "{error:?}"
        );
    }

    // ---------------------------------------------------------------------
    // The authorization URL.
    // ---------------------------------------------------------------------

    #[test]
    fn the_authorization_url_carries_pkce_and_the_resource() {
        let url = authorization_url(
            "https://auth.atlassian.com/authorize",
            "minted-id",
            "http://127.0.0.1:49512/callback",
            "the-challenge",
            "the-state",
        );

        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("code_challenge=the-challenge"));
        assert!(url.contains("state=the-state"));
        assert!(url.contains("client_id=minted-id"));
        assert!(
            url.contains("resource=https%3A%2F%2Fmcp.atlassian.com%2F"),
            "RFC 8707 names which resource the token is for: {url}"
        );
        assert!(
            url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A49512%2Fcallback"),
            "{url}"
        );
        assert!(!url.contains("write%3A"), "{url}");
    }

    // ---------------------------------------------------------------------
    // Reading, and the shapes it has to survive.
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn lists_the_sites_a_credential_reaches_in_a_stable_order() {
        let (host, _server) = serve(vec![
            ("HTTP/1.1 200 OK", initialized()),
            ("HTTP/1.1 200 OK", "{}".to_string()),
            ("HTTP/1.1 200 OK", tool_reply(one_site())),
        ]);
        let client = Client::against(&host).expect("client");
        let mcp = client.connect("token").await.expect("should initialize");

        let sites = mcp.sites().await.expect("should list");

        assert_eq!(
            sites
                .iter()
                .map(|site| site.id.as_str())
                .collect::<Vec<_>>(),
            ["cloud-1", "cloud-2"],
            "sorted, so the account keyed on the first one does not get renamed"
        );
    }

    /// The JQL is a constant Chief holds, which is what keeps a read
    /// deterministic — nothing the user typed is ever interpolated into it.
    #[tokio::test]
    async fn asks_jira_with_the_jql_chief_built_and_nothing_else() {
        let issues = serde_json::json!({
            "issues": [
                { "key": "PROJ-7", "fields": {
                    "summary": "Migrate the tenant", "status": { "name": "In Review" } } },
                { "key": "PROJ-9", "fields": {
                    "summary": "Rotate the keys", "status": { "name": "To Do" } } },
            ]
        });

        let (host, server) = serve(vec![
            ("HTTP/1.1 200 OK", initialized()),
            ("HTTP/1.1 200 OK", "{}".to_string()),
            ("HTTP/1.1 200 OK", tool_reply(issues)),
        ]);
        let client = Client::against(&host).expect("client");
        let mcp = client.connect("token").await.expect("should initialize");

        let site = Site {
            id: "cloud-1".to_string(),
            url: "https://alpha.atlassian.net".to_string(),
            name: "Alpha".to_string(),
        };

        let found = mcp.assigned(&site).await.expect("should read");

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].key, "PROJ-7");
        assert_eq!(found[0].status, "In Review");
        assert_eq!(
            found[0].url.as_deref(),
            Some("https://alpha.atlassian.net/browse/PROJ-7")
        );

        let sent = server.await.expect("server").join("");

        assert!(sent.contains("searchJiraIssuesUsingJql"), "{sent}");
        assert!(
            sent.contains("currentUser()"),
            "the token names its own owner: {sent}"
        );
        assert!(
            sent.contains("resolution = EMPTY") || sent.contains("resolution = EMPTY"),
            "open is the absence of a resolution, not a state called Done: {sent}"
        );
        assert!(
            !sent.contains("\"Done\""),
            "a hardcoded status name would break on any site that renamed it: {sent}"
        );
        assert!(
            sent.contains("Bearer token"),
            "the token goes in the header: {sent}"
        );
    }

    /// Jira has moved its search response shape more than once, and a strict
    /// struct would turn one Chief has not seen into no issues at all with
    /// nothing said.
    #[test]
    fn reads_issues_out_of_more_than_one_response_shape() {
        let site = Site {
            id: "c".to_string(),
            url: "https://alpha.atlassian.net".to_string(),
            name: "Alpha".to_string(),
        };

        let nested = serde_json::json!({
            "issues": [{ "key": "A-1", "fields": { "summary": "s", "status": { "name": "Open" } } }]
        });
        let flat = serde_json::json!([{ "key": "A-1", "summary": "s", "status": "Open" }]);

        for shape in [nested, flat] {
            let found = issues(&shape, &site);

            assert_eq!(found.len(), 1, "{shape}");
            assert_eq!(found[0].key, "A-1");
            assert_eq!(found[0].summary, "s");
            assert_eq!(found[0].status, "Open");
        }
    }

    #[test]
    fn survives_an_issue_with_no_fields_at_all() {
        let site = Site {
            id: "c".to_string(),
            url: String::new(),
            name: String::new(),
        };

        let found = issues(&serde_json::json!({ "issues": [{ "key": "A-1" }] }), &site);

        assert_eq!(found.len(), 1);
        assert!(found[0].summary.is_empty());
        assert!(
            found[0].url.is_none(),
            "a site with no address gives no link rather than a broken one"
        );
    }

    #[test]
    fn skips_what_it_cannot_read_rather_than_dropping_the_rest() {
        let site = Site {
            id: "c".to_string(),
            url: String::new(),
            name: String::new(),
        };

        let found = issues(
            &serde_json::json!({ "issues": [{ "nokey": true }, { "key": "A-2" }] }),
            &site,
        );

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key, "A-2");
    }

    /// Streamable HTTP lets the server answer either way for the same request.
    #[test]
    fn reads_the_envelope_out_of_a_plain_body_or_an_event_stream() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;

        let plain = envelope("application/json", body).expect("should parse");
        let streamed = envelope(
            "text/event-stream",
            &format!("event: message\ndata: {body}\n\n"),
        )
        .expect("should parse");

        assert_eq!(plain, streamed);
        assert_eq!(plain["result"]["ok"], serde_json::json!(true));
    }

    /// An MCP tool reports its own failure inside a successful response, so
    /// status alone says nothing — the trap GraphQL sets in `linear::read`.
    #[tokio::test]
    async fn reads_a_tool_failure_out_of_a_200() {
        let refusal = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "content": [{ "type": "text", "text": "no such project" }],
                "isError": true,
            },
        })
        .to_string();

        let (host, _server) = serve(vec![
            ("HTTP/1.1 200 OK", initialized()),
            ("HTTP/1.1 200 OK", "{}".to_string()),
            ("HTTP/1.1 200 OK", refusal),
        ]);
        let client = Client::against(&host).expect("client");
        let mcp = client.connect("token").await.expect("should initialize");

        let error = mcp
            .call(SEARCH_JIRA, serde_json::json!({}))
            .await
            .expect_err("should refuse");

        assert!(
            matches!(&error, Error::Tool(said) if said.contains("no such project")),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn a_refused_token_says_so_rather_than_looking_like_an_outage() {
        let (host, _server) = serve(vec![
            ("HTTP/1.1 200 OK", initialized()),
            ("HTTP/1.1 200 OK", "{}".to_string()),
            ("HTTP/1.1 401 Unauthorized", "{}".to_string()),
        ]);
        let client = Client::against(&host).expect("client");
        let mcp = client.connect("token").await.expect("should initialize");

        let error = mcp
            .call(SEARCH_JIRA, serde_json::json!({}))
            .await
            .expect_err("should refuse");

        assert!(matches!(error, Error::Rejected), "{error:?}");
        assert!(
            <Client as crate::oauth::Provider>::is_token_rejected(&error),
            "and the session layer has to recognise it, or it never renews"
        );
    }

    /// A grant that has been revoked is the user's to fix; anything else is
    /// worth retrying. The two read very differently on screen.
    #[tokio::test]
    async fn a_dead_refresh_token_is_a_rejection_not_a_transport_failure() {
        let (issuer, port) = reserve();
        let _issuer_server = serve_on(
            port,
            vec![
                ("HTTP/1.1 200 OK", metadata_at(&issuer)),
                (
                    "HTTP/1.1 400 Bad Request",
                    r#"{"error":"invalid_grant","error_description":"expired"}"#.to_string(),
                ),
            ],
        );

        let (host, _resource_server) = serve(vec![(
            "HTTP/1.1 200 OK",
            serde_json::json!({ "authorization_servers": [issuer.clone()] }).to_string(),
        )]);

        let client = Client::against_with_issuer(&host, &issuer).expect("client");

        let error = client
            .refresh("minted-id", "stale")
            .await
            .expect_err("should refuse");

        assert!(matches!(error, Error::Rejected), "{error:?}");
    }

    #[test]
    fn describes_an_issue_as_one_line() {
        let issue = Issue {
            key: "PROJ-7".to_string(),
            summary: "Migrate the tenant".to_string(),
            status: "In Review".to_string(),
            url: None,
        };

        assert_eq!(describe(&issue), "PROJ-7 Migrate the tenant [In Review]");

        let stateless = Issue {
            status: String::new(),
            ..issue
        };

        assert_eq!(describe(&stateless), "PROJ-7 Migrate the tenant");
    }
}
