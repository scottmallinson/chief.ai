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
    /// Two people in the workspace share the name, so whose work it is cannot
    /// be told from the name.
    #[error("more than one person in Linear has that name")]
    Ambiguous,
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
    /// When it was completed, for an issue that was. Set only by
    /// [`Client::completed`]; an open issue has none.
    pub completed_at: Option<String>,
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

/// What the user finished since a date.
///
/// `completedAt: { gte }` and not a state called "Done", for the reason
/// [`ASSIGNED_QUERY`] gives: every workspace renames its states and none of
/// them can rename this field. The date is one Chief formats itself.
const COMPLETED_QUERY: &str = r"
query Completed($first: Int!, $since: DateTimeOrDuration!) {
  viewer { name email }
  issues(
    first: $first
    filter: {
      assignee: { isMe: { eq: true } }
      completedAt: { gte: $since }
    }
    orderBy: updatedAt
  ) {
    nodes { identifier title priority state { name } completedAt }
  }
}
";

/// One assigned issue with what a report on work in progress needs and a
/// brief does not: when it is due, what blocks it, and which pull requests
/// belong to it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Item {
    pub identifier: String,
    pub title: String,
    pub state: String,
    /// Linear's own state kind: `triage`, `backlog`, `unstarted` or `started`
    /// for what is open. Every workspace renames its states and none of them
    /// can rename this.
    pub state_kind: String,
    /// Linear's own scale: 0 none, 1 urgent, 4 low.
    pub priority: u8,
    /// `YYYY-MM-DD`, when somebody set one.
    pub due: Option<String>,
    pub url: Option<String>,
    pub updated_at: Option<String>,
    /// Identifiers of the unfinished issues that block this one.
    pub blocked_by: Vec<String>,
    /// Pull requests attached to the issue, as `owner/name#number`.
    pub pull_requests: Vec<String>,
    /// Status moves and blocks, newest first as Linear returns them.
    pub changes: Vec<crate::items::Change>,
}

/// What is assigned to the key holder, with the detail [`Item`] carries.
///
/// A query of its own, not extra fields on [`ASSIGNED_QUERY`]: that one feeds
/// the brief and the work overview, and a field Linear does not recognise
/// fails the whole request. Kept apart, a mistake here costs the report and
/// nothing else.
const ITEMS_QUERY: &str = r"
query Items($first: Int!) {
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
    nodes {
      identifier title priority dueDate url updatedAt
      state { name type }
      inverseRelations { nodes { type createdAt issue { identifier completedAt canceledAt } } }
      attachments { nodes { url } }
      history(first: 25) { nodes { createdAt fromState { name } toState { name } } }
    }
  }
}
";

/// [`ITEMS_QUERY`] asked about somebody by name instead of about the key
/// holder, and also returning who each issue is assigned to so two people with
/// one name can be told apart.
///
/// Derived from the same text so the two cannot drift in what they select.
fn items_of_query() -> String {
    ITEMS_QUERY
        .replace(
            "query Items($first: Int!)",
            "query ItemsOf($first: Int!, $name: String!)",
        )
        .replace(
            "assignee: { isMe: { eq: true } }",
            "assignee: { name: { eqIgnoreCase: $name } }",
        )
        .replace(
            "identifier title priority",
            "assignee { id } identifier title priority",
        )
}

/// A GitHub pull request address as `owner/name#number`, or `None` for any
/// other kind of link. Read from the address and not from a field Linear calls
/// the source, so a link added by hand reads the same as one a bot attached.
#[must_use]
pub fn pull_request_in(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    let mut parts = rest.split('/');
    let (owner, name, kind, number) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    let number: String = number.chars().take_while(char::is_ascii_digit).collect();

    (kind == "pull" && !owner.is_empty() && !name.is_empty() && !number.is_empty())
        .then(|| format!("{owner}/{name}#{number}"))
}

/// Turn Linear's answer to [`ITEMS_QUERY`] into items.
///
/// Dug out of a `Value`, so a node missing a field costs that field and not
/// the read.
fn read_items(value: &serde_json::Value) -> Result<Vec<Item>, Error> {
    if let Some(errors) = value["errors"]
        .as_array()
        .filter(|errors| !errors.is_empty())
    {
        let said = errors
            .iter()
            .filter_map(|error| error["message"].as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();

        return Err(
            if said.contains("authentication") || said.contains("unauthorized") {
                Error::Rejected
            } else {
                Error::Decode
            },
        );
    }

    let nodes = value["data"]["issues"]["nodes"]
        .as_array()
        .ok_or(Error::Decode)?;

    let text = |node: &serde_json::Value, key: &str| node[key].as_str().map(str::to_string);

    Ok(nodes
        .iter()
        .filter_map(|node| {
            let identifier = text(node, "identifier")?;

            let blocked_by = node["inverseRelations"]["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|relation| relation["type"] == "blocks")
                .filter(|relation| {
                    // Done or cancelled blockers no longer block anything.
                    relation["issue"]["completedAt"].is_null()
                        && relation["issue"]["canceledAt"].is_null()
                })
                .filter_map(|relation| text(&relation["issue"], "identifier"))
                .collect();

            let mut changes: Vec<crate::items::Change> = node["history"]["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| {
                    // An entry that changed something else has no `toState`.
                    Some(crate::items::Change {
                        at: text(entry, "createdAt")?,
                        kind: crate::items::ChangeKind::Moved {
                            from: text(&entry["fromState"], "name").unwrap_or_default(),
                            to: text(&entry["toState"], "name")?,
                        },
                    })
                })
                .collect();

            changes.extend(
                node["inverseRelations"]["nodes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|relation| relation["type"] == "blocks")
                    .filter(|relation| {
                        relation["issue"]["completedAt"].is_null()
                            && relation["issue"]["canceledAt"].is_null()
                    })
                    .filter_map(|relation| {
                        Some(crate::items::Change {
                            at: text(relation, "createdAt")?,
                            kind: crate::items::ChangeKind::Blocked {
                                by: text(&relation["issue"], "identifier")?,
                            },
                        })
                    }),
            );

            let pull_requests = node["attachments"]["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|attachment| pull_request_in(attachment["url"].as_str()?))
                .collect();

            Some(Item {
                identifier,
                title: text(node, "title").unwrap_or_default(),
                state: text(&node["state"], "name").unwrap_or_default(),
                state_kind: text(&node["state"], "type").unwrap_or_default(),
                priority: node["priority"]
                    .as_f64()
                    .map_or(0, |priority| priority as u8),
                due: text(node, "dueDate"),
                url: text(node, "url"),
                updated_at: text(node, "updatedAt"),
                blocked_by,
                pull_requests,
                changes,
            })
        })
        .collect())
}

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
    #[serde(default, rename = "completedAt")]
    completed_at: Option<String>,
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
        self.ask(
            key,
            ASSIGNED_QUERY,
            serde_json::json!({ "first": i32::from(READ_LIMIT) }),
        )
        .await
    }

    /// What the key holder finished on or after `since`, an ISO date.
    ///
    /// The same request as [`Client::assigned`] with the other filter, so the
    /// key is handled, and its failures reported, in exactly one place.
    pub async fn completed(&self, key: &str, since: &str) -> Result<Assigned, Error> {
        self.ask(
            key,
            COMPLETED_QUERY,
            serde_json::json!({ "first": i32::from(READ_LIMIT), "since": since }),
        )
        .await
    }

    /// What is assigned to the key holder and open, with its due date, what
    /// blocks it and the pull requests attached to it.
    pub async fn items(&self, key: &str) -> Result<Vec<Item>, Error> {
        self.read_items_of(
            key,
            ITEMS_QUERY,
            serde_json::json!({ "first": i32::from(READ_LIMIT) }),
        )
        .await
    }

    /// What is assigned to somebody else in the key holder's workspace, found
    /// by their exact name and nothing looser.
    ///
    /// **A name that two people share reads as nothing.** A wrong name beside
    /// somebody's work is worse than no sheet, so the issues are read and, if
    /// they belong to more than one person, refused as [`Error::Ambiguous`].
    pub async fn items_of(&self, key: &str, name: &str) -> Result<Vec<Item>, Error> {
        self.read_items_of(
            key,
            &items_of_query(),
            serde_json::json!({ "first": i32::from(READ_LIMIT), "name": name }),
        )
        .await
    }

    async fn read_items_of(
        &self,
        key: &str,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<Vec<Item>, Error> {
        let body = serde_json::json!({ "query": query, "variables": variables });

        let response = self
            .http
            .post(&self.api)
            .header("Authorization", key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|_| Error::Transport)?;

        if response.status() == 401 || response.status() == 400 {
            return Err(Error::Rejected);
        }

        if !response.status().is_success() {
            return Err(Error::Transport);
        }

        let value: serde_json::Value = response.json().await.map_err(|_| Error::Decode)?;

        // Only the query that asks by name returns who each issue belongs to.
        let mut owners: Vec<&str> = value["data"]["issues"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|node| node["assignee"]["id"].as_str())
            .collect();
        owners.sort_unstable();
        owners.dedup();
        if owners.len() > 1 {
            return Err(Error::Ambiguous);
        }

        read_items(&value)
    }

    async fn ask(
        &self,
        key: &str,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<Assigned, Error> {
        let body = serde_json::json!({ "query": query, "variables": variables });

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
            completed_at: node.completed_at,
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
    async fn reads_what_the_key_holder_finished_since_a_date() {
        const DONE: &str = r#"{
            "data": {
                "viewer": { "name": "Scott Mallinson", "email": "scott@example.com" },
                "issues": { "nodes": [
                    { "identifier": "REC-40", "title": "Shipped it", "priority": 0,
                      "state": { "name": "Done" }, "completedAt": "2026-10-22T09:30:00.000Z" }
                ] }
            }
        }"#;
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", DONE)]);
        let client = Client::against(&host).expect("client");

        let found = client
            .completed("lin_api_test", "2026-10-22")
            .await
            .expect("should read");

        assert_eq!(found.issues[0].identifier, "REC-40");
        assert_eq!(
            found.issues[0].completed_at.as_deref(),
            Some("2026-10-22T09:30:00.000Z")
        );

        let sent = server.await.expect("server").join("");
        assert!(sent.contains("completedAt"), "{sent}");
        assert!(sent.contains("2026-10-22"), "{sent}");
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
            completed_at: None,
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
            completed_at: None,
        };

        assert_eq!(describe(&issue), "REC-42 Read Linear");
    }

    const ITEMS: &str = r#"{
        "data": { "viewer": { "name": "S", "email": "s@example.com" },
          "issues": { "nodes": [
            { "identifier": "REC-50", "title": "Ship the report", "priority": 1,
              "dueDate": "2026-10-30", "url": "https://linear.app/x/issue/REC-50",
              "updatedAt": "2026-10-27T09:00:00.000Z",
              "state": { "name": "In Progress", "type": "started" },
              "inverseRelations": { "nodes": [
                { "type": "blocks", "createdAt": "2026-10-15T08:00:00.000Z", "issue": { "identifier": "REC-48", "completedAt": null, "canceledAt": null } },
                { "type": "blocks", "createdAt": "2026-10-10T08:00:00.000Z", "issue": { "identifier": "REC-47", "completedAt": "2026-10-20T09:00:00.000Z", "canceledAt": null } },
                { "type": "related", "issue": { "identifier": "REC-46", "completedAt": null, "canceledAt": null } }
              ] },
              "history": { "nodes": [
                { "createdAt": "2026-10-22T10:00:00.000Z", "fromState": { "name": "Todo" }, "toState": { "name": "In Progress" } },
                { "createdAt": "2026-10-21T10:00:00.000Z", "fromState": null, "toState": null }
              ] },
              "attachments": { "nodes": [
                { "url": "https://github.com/scottmallinson/chief.ai/pull/131" },
                { "url": "https://github.com/scottmallinson/chief.ai/issues/9" },
                { "url": "https://example.com/doc" }
              ] } },
            { "identifier": "REC-51", "title": "Bare", "priority": 0,
              "state": { "name": "Todo", "type": "unstarted" } }
          ] } } }"#;

    #[tokio::test]
    async fn reads_the_detail_a_report_needs() {
        let (host, _server) = serve(vec![("HTTP/1.1 200 OK", ITEMS)]);
        let client = Client::against(&host).expect("client");

        let items = client.items("lin_api_test").await.expect("should read");

        assert_eq!(items.len(), 2);
        let first = &items[0];
        assert_eq!(first.identifier, "REC-50");
        assert_eq!(first.state_kind, "started");
        assert_eq!(first.priority, 1);
        assert_eq!(first.due.as_deref(), Some("2026-10-30"));
        assert_eq!(first.pull_requests, ["scottmallinson/chief.ai#131"]);
    }

    #[tokio::test]
    async fn only_an_unfinished_blocks_relation_counts_as_a_blocker() {
        let (host, _server) = serve(vec![("HTTP/1.1 200 OK", ITEMS)]);
        let client = Client::against(&host).expect("client");

        let items = client.items("lin_api_test").await.expect("should read");

        // REC-47 is finished and REC-46 is only related.
        assert_eq!(items[0].blocked_by, ["REC-48"]);
    }

    #[tokio::test]
    async fn reads_when_it_moved_and_when_it_was_blocked() {
        use crate::items::{Change, ChangeKind};

        let (host, _server) = serve(vec![("HTTP/1.1 200 OK", ITEMS)]);
        let client = Client::against(&host).expect("client");

        let items = client.items("lin_api_test").await.expect("should read");

        // The history entry that changed nothing about the state is not a move,
        // and only the unfinished blocker's relation is a block.
        assert_eq!(
            items[0].changes,
            [
                Change {
                    at: "2026-10-22T10:00:00.000Z".to_string(),
                    kind: ChangeKind::Moved {
                        from: "Todo".to_string(),
                        to: "In Progress".to_string()
                    }
                },
                Change {
                    at: "2026-10-15T08:00:00.000Z".to_string(),
                    kind: ChangeKind::Blocked {
                        by: "REC-48".to_string()
                    }
                },
            ]
        );
    }

    #[test]
    fn asks_about_somebody_by_name_and_not_about_the_key_holder() {
        let query = items_of_query();

        assert!(query.contains("$name: String!"), "{query}");
        assert!(query.contains("eqIgnoreCase: $name"), "{query}");
        assert!(!query.contains("isMe"), "{query}");
        // Everything else the report reads is still read.
        assert!(
            query.contains("history(first: 25)") && query.contains("inverseRelations"),
            "{query}"
        );
    }

    #[tokio::test]
    async fn reads_somebody_elses_items_by_name_and_sends_the_name_as_a_variable() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", ITEMS)]);
        let client = Client::against(&host).expect("client");

        let items = client
            .items_of("lin_api_test", "Dana Vance")
            .await
            .expect("should read");

        assert_eq!(items.len(), 2);
        let sent = server.await.expect("server").join("");
        assert!(
            sent.contains("Dana Vance") && sent.contains("eqIgnoreCase"),
            "{sent}"
        );
        // The name travels once, as a variable, and is never spliced into the
        // query text, where it could carry a filter of its own.
        assert_eq!(sent.matches("Dana Vance").count(), 1, "{sent}");
    }

    /// Proved by deleting the owner check in `read_items_of`: two people's
    /// issues were then reported as one person's.
    #[tokio::test]
    async fn a_name_two_people_share_is_refused_rather_than_guessed() {
        const TWO: &str = r#"{ "data": { "viewer": { "name": "S", "email": "s@example.com" },
          "issues": { "nodes": [
            { "assignee": { "id": "u1" }, "identifier": "REC-1", "title": "a", "priority": 0,
              "state": { "name": "Todo", "type": "unstarted" } },
            { "assignee": { "id": "u2" }, "identifier": "REC-2", "title": "b", "priority": 0,
              "state": { "name": "Todo", "type": "unstarted" } }
          ] } } }"#;
        let (host, _server) = serve(vec![("HTTP/1.1 200 OK", TWO)]);
        let client = Client::against(&host).expect("client");

        assert!(matches!(
            client.items_of("lin_api_test", "Dana Vance").await,
            Err(Error::Ambiguous)
        ));
    }

    #[tokio::test]
    async fn an_issue_with_no_detail_still_reads() {
        let (host, _server) = serve(vec![("HTTP/1.1 200 OK", ITEMS)]);
        let client = Client::against(&host).expect("client");

        let items = client.items("lin_api_test").await.expect("should read");

        assert_eq!(items[1].identifier, "REC-51");
        assert!(items[1].blocked_by.is_empty());
        assert!(items[1].pull_requests.is_empty());
        assert_eq!(items[1].due, None);
    }

    #[tokio::test]
    async fn a_graphql_error_in_the_detail_query_is_an_error_not_an_empty_list() {
        const WRONG: &str = r#"{ "errors": [ { "message": "Cannot query field \"dueDate\"" } ] }"#;
        let (host, _server) = serve(vec![("HTTP/1.1 200 OK", WRONG)]);
        let client = Client::against(&host).expect("client");

        assert!(matches!(
            client.items("lin_api_test").await,
            Err(Error::Decode)
        ));
    }

    #[test]
    fn reads_a_pull_request_from_its_address_and_nothing_else() {
        assert_eq!(
            pull_request_in("https://github.com/o/r/pull/12/files").as_deref(),
            Some("o/r#12")
        );
        assert_eq!(pull_request_in("https://github.com/o/r/issues/12"), None);
        assert_eq!(pull_request_in("https://gitlab.com/o/r/pull/12"), None);
        assert_eq!(pull_request_in("https://github.com/o/r/pull/"), None);
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
