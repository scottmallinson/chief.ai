//! Linear, read with a key the user pasted.
//!
//! The simplest integration Chief has, and deliberately so. Linear issues
//! **personal API keys**: the user makes one in their own Linear settings and
//! pastes it. No OAuth application, no registration, no redirect URI, no
//! administrator consent, no cost — and none of `oauth::Provider`, PKCE, the
//! loopback listener or token renewal.
//!
//! An OAuth application would be right if Chief ever acted on behalf of a
//! workspace. It does not. It reads what is assigned to one person, so it
//! stays a header on one request.
//!
//! ## The key is a credential and never comes back
//!
//! A personal key carries that person's whole Linear access — there is no
//! read-only kind. It is stored in `integrations` beside the OAuth tokens, and
//! like the calendar subscription address in [`crate::calendar`] it is **never
//! logged, never put in an error, never returned to the frontend**. [`Error`]
//! carries no key, which a test asserts.
//!
//! ## One query, asking for exactly what is used
//!
//! The API is GraphQL, so this is one POST rather than a path per resource.
//! The query names the fields the brief renders and nothing else: everything
//! extra is prompt budget spent on something no one reads.

use std::time::Duration;

use serde::Deserialize;

/// Where Linear's API lives. A constant, unlike a calendar subscription.
const API: &str = "https://api.linear.app/graphql";

/// How long to wait before deciding Linear is not answering.
const TIMEOUT: Duration = Duration::from_secs(15);

/// How many issues one read returns. A brief is five bullets; a hundred open
/// issues would fill the prompt budget and tell the reader nothing new.
pub const READ_LIMIT: u8 = 25;

/// What can go wrong reading Linear.
///
/// **No variant carries the key.** These strings reach the screen and the log.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("that Linear key was not accepted")]
    Rejected,
    #[error("Linear could not be reached")]
    Transport,
    #[error("Linear answered with something unexpected")]
    Decode,
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// One issue, reduced to what a brief actually says about it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Issue {
    /// The human identifier, e.g. `REC-42`.
    pub identifier: String,
    pub title: String,
    /// The workflow state's name, e.g. `In Progress`.
    pub state: String,
    /// Linear's own scale: 0 none, 1 urgent, 4 low.
    pub priority: f64,
}

/// Who a key belongs to, so an account can name itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewer {
    pub name: String,
    pub email: String,
}

/// Everything asked of Linear, in one query.
///
/// `assignee: { isMe: { eq: true } }` rather than a user id, so the key names
/// its own owner and Chief never has to ask who that is first. `completedAt`
/// and `canceledAt` null is how Linear says "still open" without hardcoding
/// the workflow states a particular workspace happens to use — every workspace
/// renames those, and none of them can rename these two fields.
const ASSIGNED_QUERY: &str = r"
query Assigned($first: Int!) {
  viewer { name email }
  issues(
    first: $first
    filter: {
      assignee: { isMe: { eq: true } }
      completedAt: { null: true }
      canceledAt: { null: true }
    }
    orderBy: updatedAt
  ) {
    nodes { identifier title priority state { name } }
  }
}
";

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default)]
    data: Option<Data>,
    #[serde(default)]
    errors: Option<Vec<GraphqlError>>,
}

#[derive(Debug, Deserialize)]
struct GraphqlError {
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
struct Data {
    viewer: Option<ViewerNode>,
    issues: Option<IssueConnection>,
}

#[derive(Debug, Deserialize)]
struct ViewerNode {
    #[serde(default)]
    name: String,
    #[serde(default)]
    email: String,
}

#[derive(Debug, Deserialize)]
struct IssueConnection {
    #[serde(default)]
    nodes: Vec<IssueNode>,
}

#[derive(Debug, Deserialize)]
struct IssueNode {
    #[serde(default)]
    identifier: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    priority: f64,
    #[serde(default)]
    state: Option<StateNode>,
}

#[derive(Debug, Deserialize)]
struct StateNode {
    #[serde(default)]
    name: String,
}

/// What one read of Linear found.
#[derive(Debug, Clone, PartialEq)]
pub struct Assigned {
    pub viewer: Viewer,
    pub issues: Vec<Issue>,
}

/// Reads Linear with the user's own key.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    api: String,
}

impl Client {
    pub fn new() -> Result<Self, Error> {
        Self::at(API)
    }

    /// Point at another host. `#[cfg(test)]`, so a release build cannot be
    /// aimed anywhere but Linear — the same shape as `github::Client::against`.
    #[cfg(test)]
    pub fn against(host: &str) -> Result<Self, Error> {
        Self::at(host)
    }

    fn at(api: &str) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            // No proxy, the same as everywhere else a credential goes out.
            .no_proxy()
            .build()
            .map_err(|_| Error::Transport)?;

        Ok(Self {
            http,
            api: api.to_string(),
        })
    }

    /// Who this key belongs to, and what is assigned to them.
    ///
    /// One request. The viewer comes back with the issues rather than from a
    /// second call, because a key that cannot name its owner is a key that
    /// cannot read issues either — one round trip answers both.
    pub async fn assigned(&self, key: &str) -> Result<Assigned, Error> {
        let body = serde_json::json!({
            "query": ASSIGNED_QUERY,
            "variables": { "first": i32::from(READ_LIMIT) },
        });

        let response = self
            .http
            .post(&self.api)
            .header("Authorization", key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|_| Error::Transport)?;

        // Linear answers 400 for a malformed key and 401 for a wrong one, and
        // both mean the same thing to the person who just pasted it.
        if response.status() == 401 || response.status() == 400 {
            return Err(Error::Rejected);
        }

        if !response.status().is_success() {
            return Err(Error::Transport);
        }

        let envelope: Envelope = response.json().await.map_err(|_| Error::Decode)?;

        read(envelope)
    }
}

/// Turn what Linear said into what Chief keeps.
///
/// GraphQL answers 200 with an `errors` array, so a failure here looks exactly
/// like a success to anything only reading the status.
fn read(envelope: Envelope) -> Result<Assigned, Error> {
    if let Some(errors) = envelope.errors.as_ref().filter(|errors| !errors.is_empty()) {
        let said = errors
            .iter()
            .map(|error| error.message.to_lowercase())
            .collect::<Vec<_>>()
            .join(" ");

        // Authentication is the user's problem to fix and everything else is
        // ours to report; they read differently on screen.
        return Err(
            if said.contains("authentication") || said.contains("unauthorized") {
                Error::Rejected
            } else {
                Error::Decode
            },
        );
    }

    let data = envelope.data.ok_or(Error::Decode)?;
    let viewer = data.viewer.ok_or(Error::Decode)?;

    let issues = data
        .issues
        .map(|connection| connection.nodes)
        .unwrap_or_default()
        .into_iter()
        .map(|node| Issue {
            identifier: node.identifier,
            title: node.title,
            state: node.state.map(|state| state.name).unwrap_or_default(),
            priority: node.priority,
        })
        .collect();

    Ok(Assigned {
        viewer: Viewer {
            name: viewer.name,
            email: viewer.email,
        },
        issues,
    })
}

/// One issue, as a line in the brief.
#[must_use]
pub fn describe(issue: &Issue) -> String {
    let state = if issue.state.is_empty() {
        String::new()
    } else {
        format!(" [{}]", issue.state)
    };

    format!("{} {}{state}", issue.identifier, issue.title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llama::test_support::{serve, split};

    const ASSIGNED: &str = r#"{
        "data": {
            "viewer": { "name": "Scott Mallinson", "email": "scott@example.com" },
            "issues": { "nodes": [
                { "identifier": "REC-42", "title": "Read Linear", "priority": 2,
                  "state": { "name": "In Progress" } },
                { "identifier": "REC-43", "title": "Something else", "priority": 0,
                  "state": { "name": "Todo" } }
            ] }
        }
    }"#;

    #[tokio::test]
    async fn reads_what_is_assigned_to_the_key_holder() {
        let (host, _server) = serve(vec![("HTTP/1.1 200 OK", ASSIGNED)]);
        let client = Client::against(&host).expect("client");

        let found = client.assigned("lin_api_test").await.expect("should read");

        assert_eq!(found.viewer.name, "Scott Mallinson");
        assert_eq!(found.issues.len(), 2);
        assert_eq!(found.issues[0].identifier, "REC-42");
        assert_eq!(found.issues[0].state, "In Progress");
    }

    #[tokio::test]
    async fn sends_the_key_as_the_authorization_header() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", ASSIGNED)]);
        let client = Client::against(&host).expect("client");

        client
            .assigned("lin_api_secret")
            .await
            .expect("should read");

        // Lowercased: `HeaderName` normalises, so hyper puts `authorization`
        // on the wire whatever case it was given.
        let sent = server.await.expect("server").join("").to_lowercase();

        assert!(
            sent.contains("authorization: lin_api_secret"),
            "the key goes in the header"
        );
        assert!(
            sent.contains("isme"),
            "and the query asks for the key holder's own issues"
        );
    }

    /// The filter that survives a workspace renaming its workflow states.
    #[tokio::test]
    async fn asks_for_open_issues_without_naming_a_workflow_state() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", ASSIGNED)]);
        let client = Client::against(&host).expect("client");

        client.assigned("lin_api_test").await.expect("should read");

        let sent = server.await.expect("server").join("");

        assert!(
            sent.contains("completedAt"),
            "open is the absence of a completion"
        );
        assert!(sent.contains("canceledAt"));
        assert!(
            !sent.contains("\\\"Done\\\"") && !sent.contains("name: \\\"Done\\\""),
            "a hardcoded state name would break on any workspace that renamed it"
        );
    }

    #[tokio::test]
    async fn a_rejected_key_says_so_rather_than_looking_like_an_outage() {
        let (host, _server) = serve(vec![("HTTP/1.1 401 Unauthorized", "{}")]);
        let client = Client::against(&host).expect("client");

        let error = client.assigned("wrong").await.expect_err("rejected");

        assert!(matches!(error, Error::Rejected), "{error:?}");
    }

    /// GraphQL answers 200 with an errors array, so status alone is not enough.
    #[test]
    fn reads_an_authentication_failure_out_of_a_200() {
        let envelope: Envelope = serde_json::from_str(
            r#"{"errors":[{"message":"Authentication required, not authenticated"}]}"#,
        )
        .expect("should parse");

        assert!(matches!(read(envelope), Err(Error::Rejected)));
    }

    #[test]
    fn reads_any_other_graphql_failure_as_ours_to_report() {
        let envelope: Envelope =
            serde_json::from_str(r#"{"errors":[{"message":"Query too complex"}]}"#)
                .expect("should parse");

        assert!(matches!(read(envelope), Err(Error::Decode)));
    }

    #[test]
    fn survives_an_issue_with_no_state() {
        let envelope: Envelope = serde_json::from_str(
            r#"{"data":{"viewer":{"name":"A","email":"a@b.c"},
                "issues":{"nodes":[{"identifier":"REC-1","title":"T","priority":0}]}}}"#,
        )
        .expect("should parse");

        let found = read(envelope).expect("should read");

        assert_eq!(found.issues[0].state, "");
    }

    #[test]
    fn has_nothing_assigned_without_failing() {
        let envelope: Envelope = serde_json::from_str(
            r#"{"data":{"viewer":{"name":"A","email":"a@b.c"},"issues":{"nodes":[]}}}"#,
        )
        .expect("should parse");

        assert!(read(envelope).expect("should read").issues.is_empty());
    }

    /// The failure that hands somebody's Linear access to whoever is reading.
    #[test]
    fn no_error_ever_carries_the_key() {
        for error in [Error::Rejected, Error::Transport, Error::Decode] {
            let said = error.to_string();

            assert!(
                !said.contains("lin_api"),
                "an error must not repeat the key back: {said}"
            );
        }
    }

    #[test]
    fn describes_an_issue_the_way_a_brief_reads_it() {
        let issue = Issue {
            identifier: "REC-42".to_string(),
            title: "Read Linear".to_string(),
            state: "In Progress".to_string(),
            priority: 2.0,
        };

        assert_eq!(describe(&issue), "REC-42 Read Linear [In Progress]");
    }

    #[test]
    fn leaves_the_brackets_off_when_there_is_no_state() {
        let issue = Issue {
            identifier: "REC-42".to_string(),
            title: "Read Linear".to_string(),
            state: String::new(),
            priority: 0.0,
        };

        assert_eq!(describe(&issue), "REC-42 Read Linear");
    }

    #[tokio::test]
    async fn posts_rather_than_gets_because_the_api_is_graphql() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", ASSIGNED)]);
        let client = Client::against(&host).expect("client");

        client.assigned("lin_api_test").await.expect("should read");

        let received = server.await.expect("server");
        let (line, _) = split(&received[0]);

        assert!(line.starts_with("POST "), "{line}");
    }
}
