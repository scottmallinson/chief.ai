//! Outlook mail and calendar, through Microsoft Graph.
//!
//! The second service Chief reads, and the first one it signs into with a
//! browser. Requests carry the user's own token and go straight from here to
//! Microsoft; nothing about this passes through anything of Chief's.
//!
//! ## Why the browser rather than a device code
//!
//! GitHub's device flow needs no secret, which is why Chief uses it there.
//! Microsoft has one too and it cannot be relied on: security defaults now
//! include blocking device code flow, new tenants block it by default, and
//! Microsoft's own guidance is to "get as close as possible to a unilateral
//! block". So Outlook is authorization code + PKCE against a loopback
//! listener — which Entra explicitly supports for public clients, and which
//! needs no secret either.
//!
//! ## Why the flow lives here and not in `oauth`
//!
//! [`crate::oauth::Provider`] says it plainly: a flow lives in the provider's
//! own module until a second provider wants the same one, because an
//! abstraction drawn from one example is a guess. This is the first
//! loopback-and-PKCE provider. When Google arrives it will be the second, and
//! that is the point to lift this into `oauth::flow`.

use std::time::Duration;

use serde::Deserialize;

use crate::integrations::MICROSOFT;
use crate::oauth::{loopback, pkce, Endpoints, Tokens};

/// How long any one request may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The `/common` authority, so a personal Microsoft account and a work account
/// share one code path. The registration is multi-tenant
/// (`signInAudience = AzureADandPersonalMicrosoftAccount`) for the same reason.
const AUTHORIZE: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize";
const TOKEN: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";

/// Where Graph is read from.
const GRAPH: &str = "https://graph.microsoft.com";

/// What Chief asks for.
///
/// Full message content rather than a metadata tier, because Microsoft has no
/// restricted-scope tax to tier away from — reading a subject line costs the
/// same review as reading the body, which is none. `offline_access` is what
/// makes a refresh token come back at all; without it the connection dies in an
/// hour. `Tasks.Read` for Microsoft To Do is available later on this same
/// registration and the same API.
const SCOPE_LIST: &[&str] = &["Mail.Read", "Calendars.Read", "offline_access", "User.Read"];

/// The environment variable that points Chief at another registration.
const CLIENT_ID_VAR: &str = "CHIEF_MICROSOFT_CLIENT_ID";

/// The Entra error code for risk-based step-up consent.
///
/// Not a transport failure and not the user's mistake: it means an unverified
/// multi-tenant app asked for more than basic sign-in in somebody else's
/// tenant, and the tenant said an administrator has to approve it first. It is
/// on by default, so this is a routine outcome rather than an exotic one.
const ADMIN_CONSENT_CODE: &str = "AADSTS90094";

/// What can go wrong reading Outlook.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "Chief has no Microsoft client id. Set {CLIENT_ID_VAR} to one from your own app \
         registration."
    )]
    NoClientId,
    #[error("could not reach Microsoft: {0}")]
    Transport(String),
    #[error(
        "your organisation requires an administrator to approve Chief before you can connect \
         Outlook. Ask them to grant consent for it to read your mail and calendar."
    )]
    AdminConsentRequired,
    #[error("the sign-in was refused: {0}")]
    Refused(String),
    #[error("Outlook needs connecting again — the sign-in was not completed")]
    NotConnected,
    #[error("Outlook needs connecting again. Signing in again will fix it.")]
    TokenRejected,
    #[error("Microsoft answered with HTTP {status}")]
    Status { status: u16 },
    #[error("could not read Microsoft's reply: {0}")]
    Decode(String),
    #[error(transparent)]
    SignIn(#[from] loopback::Error),
    #[error(transparent)]
    Storage(#[from] crate::db::Error),
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// The registration Chief signs in with.
///
/// **Not a secret.** A public-client id is public by design; what makes the
/// client public is registering the redirect under "Mobile and desktop
/// applications", not keeping the id quiet. The run-time override exists so a
/// developer — or an organisation that would rather register its own app than
/// approve Chief's — can point at theirs without a rebuild.
pub fn client_id() -> Result<String, Error> {
    if let Ok(from_env) = std::env::var(CLIENT_ID_VAR) {
        if !from_env.trim().is_empty() {
            return Ok(from_env);
        }
    }

    option_env!("CHIEF_MICROSOFT_CLIENT_ID")
        .filter(|id| !id.trim().is_empty())
        .map(ToString::to_string)
        .ok_or(Error::NoClientId)
}

/// A sign-in that has been started and is waiting on the browser.
///
/// Holds the listener, so the port is bound before the authorization URL is
/// built and stays bound for the whole flow — a port chosen and then let go
/// could be taken by anything before Microsoft redirects to it.
pub struct PendingLogin {
    pub listener: loopback::Listener,
    pub verifier: pkce::Verifier,
    pub state: pkce::State,
    pub redirect_uri: String,
}

/// Reads Outlook for the account the user connected.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    auth_host: String,
    graph_host: String,
}

impl Client {
    /// A client pinned to Microsoft.
    pub fn new() -> Result<Self, Error> {
        Self::build(
            "https://login.microsoftonline.com".to_string(),
            GRAPH.to_string(),
        )
    }

    /// A client pointed somewhere else, for tests only.
    ///
    /// `#[cfg(test)]` so a release build cannot be aimed anywhere but Microsoft.
    #[cfg(test)]
    pub(crate) fn against(host: &str) -> Result<Self, Error> {
        Self::build(host.to_string(), host.to_string())
    }

    fn build(auth_host: String, graph_host: String) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            // No proxy, for the same reason the inference client has none: a
            // proxy is a third party to a conversation that has two.
            .no_proxy()
            .build()
            .map_err(|error| Error::Transport(error.to_string()))?;

        Ok(Self {
            http,
            auth_host,
            graph_host,
        })
    }

    fn authorize_url(&self) -> String {
        if self.auth_host == "https://login.microsoftonline.com" {
            AUTHORIZE.to_string()
        } else {
            format!("{}/common/oauth2/v2.0/authorize", self.auth_host)
        }
    }

    fn token_url(&self) -> String {
        if self.auth_host == "https://login.microsoftonline.com" {
            TOKEN.to_string()
        } else {
            format!("{}/common/oauth2/v2.0/token", self.auth_host)
        }
    }

    /// Bind a port, and return where to send the user.
    ///
    /// The listener is bound *before* the URL is built, so the port in the
    /// redirect is one this process already holds.
    pub async fn start_login(&self, client_id: &str) -> Result<(PendingLogin, String), Error> {
        let listener = loopback::Listener::bind().await?;
        let redirect_uri = listener.redirect_uri();
        let verifier = pkce::Verifier::generate();
        let state = pkce::State::generate();

        let url = authorization_url(
            &self.authorize_url(),
            client_id,
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
            },
            url,
        ))
    }

    /// Wait for the browser to come back, then trade the code for tokens.
    ///
    /// The listener owns the waiting and the timeout, and it already turns an
    /// `error=` redirect into a refusal in the provider's own words. All that
    /// is left here is recognising which refusals Outlook has a better answer
    /// for than "sign-in failed".
    pub async fn finish_login(
        &self,
        client_id: &str,
        pending: PendingLogin,
    ) -> Result<Tokens, Error> {
        let redirect = pending.listener.wait().await.map_err(refusal)?;

        // Compared before the code is spent: a redirect whose state does not
        // match is not the flow this process started.
        if !pending.state.matches(&redirect.state) {
            return Err(Error::Refused(
                "the sign-in came back with a state Chief did not send".to_string(),
            ));
        }

        let form = [
            ("client_id", client_id),
            ("grant_type", "authorization_code"),
            ("code", redirect.code.as_str()),
            ("redirect_uri", pending.redirect_uri.as_str()),
            ("code_verifier", pending.verifier.as_str()),
            ("scope", &SCOPE_LIST.join(" ")),
        ];

        self.exchange(&form).await
    }

    /// Swap a refresh token for a fresh pair.
    pub async fn refresh(&self, client_id: &str, refresh_token: &str) -> Result<Tokens, Error> {
        let form = [
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("scope", &SCOPE_LIST.join(" ")),
        ];

        self.exchange(&form).await
    }

    async fn exchange(&self, form: &[(&str, &str)]) -> Result<Tokens, Error> {
        let response = self
            .http
            .post(self.token_url())
            .form(form)
            .send()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| Error::Decode(error.to_string()))?;

        let parsed: TokenReply = serde_json::from_str(&body)
            .map_err(|error| Error::Decode(format!("{error}; body: {body}")))?;

        if let Some(error) = parsed.error {
            return Err(classify(&error, parsed.error_description.as_deref()));
        }

        if !status.is_success() {
            return Err(Error::Status {
                status: status.as_u16(),
            });
        }

        Ok(Tokens {
            access_token: parsed.access_token.ok_or(Error::NotConnected)?,
            refresh_token: parsed.refresh_token,
            expires_in: parsed.expires_in,
        })
    }

    /// Who this token belongs to, for naming the account.
    ///
    /// Asked before the credential is stored, so two mailboxes on one service
    /// do not collide on a placeholder key.
    pub async fn me(&self, access_token: &str) -> Result<String, Error> {
        let response = self
            .http
            .get(format!("{}/v1.0/me", self.graph_host))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::TokenRejected);
        }

        if !response.status().is_success() {
            return Err(Error::Status {
                status: response.status().as_u16(),
            });
        }

        let me: Identity = response
            .json()
            .await
            .map_err(|error| Error::Decode(error.to_string()))?;

        me.user_principal_name
            .or(me.mail)
            .ok_or_else(|| Error::Decode("Microsoft named no mailbox for this account".into()))
    }
}

/// How many of anything one read returns.
///
/// Graph will page further, and Chief does not: what the agent needs is what is
/// recent, and every extra item is prompt the model has to read before it can
/// answer. The context budget is the constraint, not the API's.
pub const READ_LIMIT: u8 = 25;

/// One meeting, reduced to what the agent needs to answer questions.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Event {
    pub subject: String,
    /// ISO-8601, in the time zone Graph was asked for.
    pub start: String,
    pub end: String,
    pub organiser: Option<String>,
    /// Names rather than addresses: this is what goes to the model, and a
    /// mailbox address is more identifying than the answer needs.
    pub attendees: Vec<String>,
    pub online: bool,
}

/// One message, reduced the same way.
///
/// **Preview rather than body.** `Mail.Read` grants the whole message and Chief
/// takes the first couple of hundred characters Graph already summarises. Two
/// reasons and both matter: a mailbox of full bodies would exhaust the context
/// budget several times over on a single read, and the model does not need the
/// whole of a message to say who is waiting on what.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MailMessage {
    pub subject: String,
    pub from: Option<String>,
    pub received_at: String,
    pub preview: String,
    pub unread: bool,
}

impl Client {
    /// What is in the calendar between two instants.
    ///
    /// `calendarView` rather than `events`, because it expands recurrences —
    /// asking for `events` returns the series and leaves the app to work out
    /// which Tuesday it is, which is exactly the date arithmetic a small model
    /// is bad at and Chief does deterministically or not at all.
    pub async fn events(
        &self,
        access_token: &str,
        from: &str,
        to: &str,
        limit: u8,
    ) -> Result<Vec<Event>, Error> {
        let url = format!(
            "{}/v1.0/me/calendarView?startDateTime={}&endDateTime={}&$top={}&$orderby=start/dateTime&$select=subject,start,end,organizer,attendees,isOnlineMeeting",
            self.graph_host,
            urlencode(from),
            urlencode(to),
            limit
        );

        let page: Page<RawEvent> = self.read(access_token, &url).await?;

        Ok(page.value.into_iter().map(RawEvent::reduce).collect())
    }

    /// The most recent messages in the inbox.
    pub async fn messages(&self, access_token: &str, limit: u8) -> Result<Vec<MailMessage>, Error> {
        let url = format!(
            "{}/v1.0/me/messages?$top={limit}&$orderby=receivedDateTime desc&$select=subject,from,receivedDateTime,bodyPreview,isRead",
            self.graph_host
        );

        let page: Page<RawMessage> = self.read(access_token, &url).await?;

        Ok(page.value.into_iter().map(RawMessage::reduce).collect())
    }

    /// One authenticated GET against Graph, with the refusals named.
    async fn read<T: serde::de::DeserializeOwned>(
        &self,
        access_token: &str,
        url: &str,
    ) -> Result<T, Error> {
        let response = self
            .http
            .get(url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::TokenRejected);
        }

        if !response.status().is_success() {
            return Err(Error::Status {
                status: response.status().as_u16(),
            });
        }

        response
            .json()
            .await
            .map_err(|error| Error::Decode(error.to_string()))
    }
}

/// Graph wraps every collection in `value`.
#[derive(Debug, Deserialize)]
struct Page<T> {
    #[serde(default = "Vec::new")]
    value: Vec<T>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEvent {
    subject: Option<String>,
    start: Option<GraphTime>,
    end: Option<GraphTime>,
    organizer: Option<Recipient>,
    #[serde(default)]
    attendees: Vec<Recipient>,
    is_online_meeting: Option<bool>,
}

impl RawEvent {
    fn reduce(self) -> Event {
        Event {
            // A meeting with no subject is a real thing people send, and
            // "(no subject)" is what every mail client calls it.
            subject: self.subject.unwrap_or_else(|| "(no subject)".to_string()),
            start: self.start.map(|at| at.date_time).unwrap_or_default(),
            end: self.end.map(|at| at.date_time).unwrap_or_default(),
            organiser: self.organizer.and_then(Recipient::name),
            attendees: self
                .attendees
                .into_iter()
                .filter_map(Recipient::name)
                .collect(),
            online: self.is_online_meeting.unwrap_or(false),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMessage {
    subject: Option<String>,
    from: Option<Recipient>,
    received_date_time: Option<String>,
    body_preview: Option<String>,
    is_read: Option<bool>,
}

impl RawMessage {
    fn reduce(self) -> MailMessage {
        MailMessage {
            subject: self.subject.unwrap_or_else(|| "(no subject)".to_string()),
            from: self.from.and_then(Recipient::name),
            received_at: self.received_date_time.unwrap_or_default(),
            preview: self.body_preview.unwrap_or_default(),
            unread: !self.is_read.unwrap_or(true),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphTime {
    #[serde(default)]
    date_time: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Recipient {
    email_address: Option<EmailAddress>,
}

impl Recipient {
    /// A person's name, falling back to their address when Graph has no name.
    fn name(self) -> Option<String> {
        let address = self.email_address?;

        address
            .name
            .filter(|name| !name.trim().is_empty())
            .or(address.address)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmailAddress {
    name: Option<String>,
    address: Option<String>,
}

/// Build the authorization URL.
///
/// Split out from the request so the query it sends is covered by a test rather
/// than by hoping: PKCE with S256, the state nonce, and the exact redirect the
/// listener is bound to are each load-bearing.
fn authorization_url(
    endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> String {
    let query = [
        ("client_id", client_id),
        ("response_type", "code"),
        ("redirect_uri", redirect_uri),
        ("response_mode", "query"),
        ("scope", &SCOPE_LIST.join(" ")),
        ("state", state),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
    ];

    let encoded = query
        .iter()
        .map(|(key, value)| format!("{key}={}", urlencode(value)))
        .collect::<Vec<_>>()
        .join("&");

    format!("{endpoint}?{encoded}")
}

/// Percent-encode a query value.
///
/// Hand-rolled rather than another dependency: the set of characters that can
/// appear in these values is small and known — scopes have spaces and dots,
/// redirect URIs have a colon and slashes, PKCE values are base64url.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());

    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }

    out
}

/// Recognise the refusals Outlook has a better answer for.
///
/// The listener has already read what the authorization server said; this only
/// promotes the one code that means something a person can act on, and leaves
/// everything else in Microsoft's own words.
fn refusal(error: loopback::Error) -> Error {
    match &error {
        loopback::Error::Refused(said) if said.contains(ADMIN_CONSENT_CODE) => {
            Error::AdminConsentRequired
        }
        _ => Error::SignIn(error),
    }
}

/// Turn an Entra error code into something a person can act on.
fn classify(code: &str, description: Option<&str>) -> Error {
    let said = description.unwrap_or(code);

    // The code appears in the description too, which is where Entra actually
    // puts it on the redirect leg of the flow.
    if code.contains(ADMIN_CONSENT_CODE) || said.contains(ADMIN_CONSENT_CODE) {
        return Error::AdminConsentRequired;
    }

    if code == "invalid_grant" {
        return Error::TokenRejected;
    }

    Error::Refused(said.to_string())
}

/// What the token endpoint says, whether or not it worked.
#[derive(Debug, Deserialize)]
struct TokenReply {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

/// The parts of `/me` Chief uses.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Identity {
    user_principal_name: Option<String>,
    mail: Option<String>,
}

impl crate::oauth::Provider for Client {
    const SERVICE: &'static str = MICROSOFT;

    type Error = Error;

    fn endpoints(&self) -> Endpoints {
        Endpoints {
            authorize: AUTHORIZE,
            token: TOKEN,
            // Entra blocks the device grant by default and Microsoft's guidance
            // is to block it outright, so no org story can be built on it.
            device_code: None,
        }
    }

    fn client_id(&self) -> Result<String, Error> {
        client_id()
    }

    fn scopes(&self) -> &'static [&'static str] {
        SCOPE_LIST
    }

    // A public client has no secret, and one shipped inside a binary Chief
    // distributes would not be a secret, so the default `None` stands.

    async fn refresh(&self, client_id: &str, refresh_token: &str) -> Result<Tokens, Error> {
        // Spelled out, so it is clear this is not calling itself: the inherent
        // method does the exchange and wins the lookup.
        Client::refresh(self, client_id, refresh_token).await
    }

    fn is_token_rejected(error: &Error) -> bool {
        matches!(error, Error::TokenRejected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llama::test_support::{serve, split};

    #[test]
    fn asks_for_mail_and_calendar_and_a_way_to_stay_connected() {
        assert!(SCOPE_LIST.contains(&"Mail.Read"));
        assert!(SCOPE_LIST.contains(&"Calendars.Read"));

        // Without this the connection dies in an hour and there is no refresh
        // token to renew it with.
        assert!(
            SCOPE_LIST.contains(&"offline_access"),
            "a connection that cannot be renewed is not a connection"
        );
    }

    #[test]
    fn the_authorization_url_carries_pkce_and_a_state_nonce() {
        let url = authorization_url(
            AUTHORIZE,
            "client-id",
            "http://127.0.0.1:51234/callback",
            "the-challenge",
            "the-state",
        );

        assert!(url.starts_with(AUTHORIZE), "{url}");
        assert!(url.contains("response_type=code"), "{url}");
        assert!(url.contains("code_challenge=the-challenge"), "{url}");
        assert!(
            url.contains("code_challenge_method=S256"),
            "plain PKCE is not PKCE: {url}"
        );
        assert!(url.contains("state=the-state"), "{url}");
        assert!(
            url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A51234%2Fcallback"),
            "{url}"
        );
    }

    #[test]
    fn the_redirect_is_the_loopback_address_rather_than_the_name() {
        // `localhost` resolves to ::1 first on plenty of machines, the listener
        // binds IPv4, and Entra does not support [::1] — so the literal address
        // is the only one that works everywhere.
        let url = authorization_url(AUTHORIZE, "id", "http://127.0.0.1:1/cb", "c", "s");

        assert!(url.contains("127.0.0.1"), "{url}");
        assert!(!url.contains("localhost"), "{url}");
    }

    #[test]
    fn a_tenant_that_needs_an_administrator_says_so() {
        let error = classify(
            "consent_required",
            Some("AADSTS90094: The grant requires admin permission."),
        );

        assert!(matches!(error, Error::AdminConsentRequired), "{error:?}");

        let message = error.to_string();
        assert!(
            message.contains("administrator"),
            "the user has to know what to ask for: {message}"
        );
        assert!(
            !message.contains("AADSTS"),
            "a code is not an explanation: {message}"
        );
    }

    #[test]
    fn a_revoked_credential_reads_as_reconnect_rather_than_as_a_failure() {
        // Routine: Entra revokes refresh tokens on a password change, on a
        // self-service reset, and on any admin "revoke all" action.
        let error = classify("invalid_grant", Some("AADSTS70008: expired"));

        assert!(matches!(error, Error::TokenRejected), "{error:?}");
        assert!(error.to_string().contains("connecting again"));
    }

    #[test]
    fn anything_else_is_reported_as_what_microsoft_said() {
        let error = classify("unsupported_response_type", Some("it did not like that"));

        assert!(matches!(error, Error::Refused(_)));
        assert!(error.to_string().contains("it did not like that"));
    }

    #[test]
    fn a_release_build_can_only_be_aimed_at_microsoft() {
        let client = Client::new().expect("should build");

        assert_eq!(client.auth_host, "https://login.microsoftonline.com");
        assert_eq!(client.graph_host, GRAPH);
    }

    #[test]
    fn encodes_the_characters_a_query_value_actually_contains() {
        assert_eq!(
            urlencode("Mail.Read offline_access"),
            "Mail.Read%20offline_access"
        );
        assert_eq!(
            urlencode("http://127.0.0.1:1/cb"),
            "http%3A%2F%2F127.0.0.1%3A1%2Fcb"
        );
        assert_eq!(urlencode("aB9-._~"), "aB9-._~");
    }

    #[tokio::test]
    async fn swaps_a_refresh_token_for_a_fresh_pair() {
        const GRANTED: &str =
            r#"{"access_token":"new-access","refresh_token":"new-refresh","expires_in":3600}"#;
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", GRANTED)]);

        let client = Client::against(&host).expect("should build");
        let tokens = client
            .refresh("client-id", "old-refresh")
            .await
            .expect("should refresh");

        assert_eq!(tokens.access_token, "new-access");
        assert_eq!(tokens.refresh_token.as_deref(), Some("new-refresh"));
        assert_eq!(tokens.expires_in, Some(3600));

        let sent = server.await.expect("the stub should finish");
        let (_, body) = split(&sent[0]);
        assert!(body.contains("grant_type=refresh_token"), "{body}");
        assert!(
            !body.contains("client_secret"),
            "a public client sends no secret: {body}"
        );
    }

    #[tokio::test]
    async fn a_rejected_refresh_asks_for_a_reconnection() {
        const REFUSED: &str =
            r#"{"error":"invalid_grant","error_description":"AADSTS50173: revoked"}"#;
        let (host, _server) = serve(vec![("HTTP/1.1 400 Bad Request", REFUSED)]);

        let client = Client::against(&host).expect("should build");
        let error = client
            .refresh("client-id", "stale")
            .await
            .expect_err("a revoked token should be rejected");

        assert!(matches!(error, Error::TokenRejected), "{error:?}");
    }

    #[tokio::test]
    async fn names_the_account_by_its_mailbox() {
        const ME: &str =
            r#"{"userPrincipalName":"someone@example.com","mail":"someone@example.com"}"#;
        let (host, _server) = serve(vec![("HTTP/1.1 200 OK", ME)]);

        let client = Client::against(&host).expect("should build");

        assert_eq!(
            client.me("token").await.expect("should read /me"),
            "someone@example.com"
        );
    }

    #[tokio::test]
    async fn an_unauthorised_read_asks_for_a_reconnection() {
        let (host, _server) = serve(vec![("HTTP/1.1 401 Unauthorized", "{}")]);

        let client = Client::against(&host).expect("should build");
        let error = client
            .me("stale")
            .await
            .expect_err("401 should be rejected");

        assert!(matches!(error, Error::TokenRejected), "{error:?}");
    }
}
