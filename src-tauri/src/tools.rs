//! Tools the agent can call.
//!
//! A tool is a schema the model sees plus a function we run on its behalf.
//! Everything runs on the user's terms: a tool either reads their own local
//! database or calls a service they explicitly connected, with their own token.

use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::SqlitePool;

use crate::github::{self, State as PrState};
use crate::integrations;
use crate::llama::{Tool, ToolCall, ToolCallFunction, ToolFunction};
use crate::microsoft;
use crate::session::{GithubSession, OutlookSession};

/// What the tools need to do their work: the user's local database, and a
/// client for the services they have connected.
///
/// Passing this explicitly, rather than reaching into the Tauri app, keeps the
/// tools runnable in tests against an in-memory database and a stub server.
#[derive(Debug, Clone)]
pub struct Context {
    pub pool: SqlitePool,
    pub github: github::Client,
    pub microsoft: microsoft::Client,
}

/// Names of the tools we advertise, so the dispatcher and the catalogue cannot
/// drift apart.
const FETCH_GITHUB_PRS: &str = "fetch_github_prs";
const FETCH_CALENDAR: &str = "fetch_calendar";
const FETCH_RECENT_MAIL: &str = "fetch_recent_mail";

/// Which stretch of calendar the model is asking about.
///
/// A named range rather than two dates. The model is told today's date and is
/// still poor at arithmetic on it, and a range it gets wrong is a confidently
/// empty answer — so Rust resolves these to instants and the model only has to
/// pick which one it meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Range {
    #[default]
    Today,
    Tomorrow,
    Week,
}

impl Range {
    /// The instants Graph is asked for, in this machine's own time zone.
    fn window(self) -> (String, String) {
        let now = chrono::Local::now();
        let midnight = now.date_naive().and_hms_opt(0, 0, 0).unwrap_or_default();

        let (from, days) = match self {
            Self::Today => (midnight, 1),
            Self::Tomorrow => (midnight + chrono::Duration::days(1), 1),
            Self::Week => (midnight, 7),
        };

        let to = from + chrono::Duration::days(days);

        (
            from.format("%Y-%m-%dT%H:%M:%S").to_string(),
            to.format("%Y-%m-%dT%H:%M:%S").to_string(),
        )
    }
}

/// Arguments to [`FETCH_CALENDAR`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct FetchCalendarArgs {
    range: Range,
}

/// Which pull requests the model is asking about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PullRequestState {
    #[default]
    Open,
    Closed,
    Merged,
    All,
}

impl From<PullRequestState> for PrState {
    fn from(state: PullRequestState) -> Self {
        match state {
            PullRequestState::Open => PrState::Open,
            PullRequestState::Closed => PrState::Closed,
            PullRequestState::Merged => PrState::Merged,
            PullRequestState::All => PrState::All,
        }
    }
}

/// How many pull requests to read in one go, from each connected account. A
/// person with two accounts has two lists to answer from, and halving both to
/// keep one total would answer worse than reading each of them properly.
const PR_LIMIT: u8 = 25;

/// Arguments to [`FETCH_GITHUB_PRS`], as the model produced them.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct FetchGithubPrsArgs {
    state: PullRequestState,
}

/// The most tools that may ever be offered, and the most characters their
/// schemas may take.
///
/// Every tool is prompt: the catalogue is sent on every single turn, before the
/// user's question, and it is paid for in prefill on a CPU. The ceiling exists
/// so that adding a tool is a decision rather than a habit. Characters rather
/// than tokens because a tokenizer here would be a dependency to measure a
/// budget with three digits of slack in it; the ratio is close enough to four.
#[cfg(test)]
const MAX_TOOLS: usize = 6;
#[cfg(test)]
const MAX_SCHEMA_CHARS: usize = 600 * 4;

/// The tools offered to the model on every turn.
pub fn catalog() -> Vec<Tool> {
    vec![
        Tool::function(ToolFunction {
            name: FETCH_GITHUB_PRS.to_string(),
            description: "Fetch the user's GitHub pull requests, including which are still \
             unmerged and who they are waiting on. Use state 'merged' for what they \
             shipped. Call this whenever the user asks about pull requests, code review, \
             or what they have shipped. Covers every GitHub account the user has \
             connected; each pull request names the account it came from, and 'accounts' \
             reports what was read from each."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "state": {
                        "type": "string",
                        "enum": ["open", "closed", "merged", "all"],
                        "description": "Which pull requests to return. Defaults to open.",
                    },
                },
                "required": [],
            }),
        }),
        Tool::function(ToolFunction {
            name: FETCH_CALENDAR.to_string(),
            description: "Fetch the user's Outlook calendar: meetings, when they are, and \
                 who is in them. Call this whenever the user asks about their day, their \
                 schedule, a meeting, or who they are seeing."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "range": {
                        "type": "string",
                        "enum": ["today", "tomorrow", "week"],
                        "description": "Which stretch of calendar to return. Defaults to today.",
                    },
                },
                "required": [],
            }),
        }),
        Tool::function(ToolFunction {
            name: FETCH_RECENT_MAIL.to_string(),
            description: "Fetch the most recent messages in the user's Outlook inbox, with \
                 who sent them and a short preview. Call this whenever the user asks about \
                 email, what has come in, or who is waiting on them."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": [],
            }),
        }),
    ]
}

/// Run the tool the model asked for and return the JSON we feed back to it.
///
/// A failure is reported to the model rather than to the user: it gets an
/// `error` payload and can explain itself or try something else, which beats
/// collapsing the whole conversation.
pub async fn dispatch(context: &Context, call: &ToolCall) -> Value {
    match call.function.name.as_str() {
        FETCH_GITHUB_PRS => match parse(&call.function) {
            Ok(args) => match fetch_github_prs(context, args.state).await {
                Ok(result) => result,
                Err(error) => json!({ "error": error.to_string() }),
            },
            Err(message) => json!({ "error": message }),
        },
        FETCH_CALENDAR => match parse_calendar(&call.function) {
            Ok(args) => match fetch_calendar(context, args.range).await {
                Ok(result) => result,
                Err(error) => json!({ "error": error.to_string() }),
            },
            Err(message) => json!({ "error": message }),
        },
        FETCH_RECENT_MAIL => match fetch_recent_mail(context).await {
            Ok(result) => result,
            Err(error) => json!({ "error": error.to_string() }),
        },
        unknown => json!({
            "error": format!("there is no tool called '{unknown}'"),
        }),
    }
}

fn parse(function: &ToolCallFunction) -> Result<FetchGithubPrsArgs, String> {
    let arguments = function.arguments()?;

    // Models sometimes send an empty value instead of an empty object.
    if arguments.is_null() {
        return Ok(FetchGithubPrsArgs::default());
    }

    serde_json::from_value(arguments)
        .map_err(|error| format!("could not read the arguments: {error}"))
}

/// What to call an account when the model has to say whose work this is: the
/// name the user gave it, else who the provider says it is, else the key the
/// credential is stored under. Something is always available, and the last of
/// those is still recognisable to the person who connected it.
fn describe(account: &integrations::Account) -> String {
    account
        .label
        .clone()
        .or_else(|| account.identity.clone())
        .unwrap_or_else(|| account.account_key.clone())
}

/// Read the user's pull requests from GitHub with their own token.
///
/// Every connected account, and each pull request says which one it came from.
/// Reading only the first was silently answering about half the work of anyone
/// with a work and a personal account — the model could not tell it had half,
/// and neither could the user. Adding a tool *argument* would only move the
/// guess to the model, which knows less about the user's accounts than this
/// does; reading all of them removes the guess instead.
///
/// An account that will not answer is reported beside the ones that did, so a
/// single revoked token does not cost the user the rest of the answer. Nothing
/// readable at all is an error, which is how "Reconnect GitHub" reaches them.
async fn fetch_github_prs(
    context: &Context,
    state: PullRequestState,
) -> Result<Value, github::Error> {
    let accounts = integrations::accounts(&context.pool, integrations::GITHUB).await?;

    let mut pull_requests = Vec::new();
    let mut reports = Vec::new();
    let mut readable = 0;
    let mut refused = None;

    for account in &accounts {
        let name = describe(account);

        match GithubSession::new(&context.pool, &context.github, account.id)
            .pull_requests(state.into(), PR_LIMIT)
            .await
        {
            Ok(found) => {
                readable += 1;
                reports.push(json!({ "account": name, "count": found.len() }));
                pull_requests.extend(github::as_tool_entries(&name, &found));
            }
            Err(error) => {
                reports.push(json!({ "account": name, "error": error.to_string() }));
                refused = Some(error);
            }
        }
    }

    if readable == 0 {
        // Whatever the last account said, or — with none connected — the
        // sentence that tells the user what to do about it.
        return Err(refused.unwrap_or(github::Error::NotConnected));
    }

    Ok(json!({
        "pull_requests": pull_requests,
        "count": pull_requests.len(),
        "accounts": reports,
    }))
}

/// Which Outlook read to perform.
///
/// Data rather than a closure: both reads want the same "every connected
/// account, saying which is which" loop, and passing what to do into it as a
/// value keeps that loop written once without a higher-ranked lifetime nobody
/// has to think about.
enum OutlookRead {
    Events { from: String, to: String },
    Messages,
}

/// Read the user's calendar with their own token.
async fn fetch_calendar(context: &Context, range: Range) -> Result<Value, microsoft::Error> {
    let (from, to) = range.window();
    let (events, accounts) = read_outlook(context, &OutlookRead::Events { from, to }).await?;

    Ok(json!({
        "events": events,
        "count": events.as_array().map_or(0, Vec::len),
        "accounts": accounts,
    }))
}

/// Read the user's inbox with their own token.
async fn fetch_recent_mail(context: &Context) -> Result<Value, microsoft::Error> {
    let (messages, accounts) = read_outlook(context, &OutlookRead::Messages).await?;

    Ok(json!({
        "messages": messages,
        "count": messages.as_array().map_or(0, Vec::len),
        "accounts": accounts,
    }))
}

/// Read every connected Outlook account, saying which is which.
///
/// The same shape as the GitHub read: one account failing is that account's
/// failure, and only every account failing is the tool's.
async fn read_outlook(
    context: &Context,
    what: &OutlookRead,
) -> Result<(Value, Vec<Value>), microsoft::Error> {
    let accounts = integrations::accounts(&context.pool, integrations::MICROSOFT)
        .await
        .map_err(microsoft::Error::Storage)?;

    let mut items: Vec<Value> = Vec::new();
    let mut reports = Vec::new();
    let mut readable = 0;
    let mut refused = None;

    for account in &accounts {
        let name = describe(account);
        let session = OutlookSession::new(&context.pool, &context.microsoft, account.id);

        let found = match what {
            OutlookRead::Events { from, to } => session
                .events(from, to, microsoft::READ_LIMIT)
                .await
                .map(|events| events.into_iter().map(as_entry).collect::<Vec<_>>()),
            OutlookRead::Messages => session
                .messages(microsoft::READ_LIMIT)
                .await
                .map(|messages| messages.into_iter().map(as_entry).collect::<Vec<_>>()),
        };

        match found {
            Ok(found) => {
                readable += 1;
                reports.push(json!({ "account": name, "count": found.len() }));

                // Every entry names the account it came from, so an answer
                // about two mailboxes can say which one it means.
                items.extend(found.into_iter().map(|mut entry| {
                    if let Some(object) = entry.as_object_mut() {
                        object.insert("account".to_string(), json!(name));
                    }
                    entry
                }));
            }
            Err(error) => {
                reports.push(json!({ "account": name, "error": error.to_string() }));
                refused = Some(error);
            }
        }
    }

    if readable == 0 {
        // Whatever the last account said, or — with none connected — the
        // sentence that tells the user what to do about it.
        return Err(refused.unwrap_or(microsoft::Error::NotConnected));
    }

    Ok((Value::Array(items), reports))
}

fn as_entry<T: serde::Serialize>(one: T) -> Value {
    serde_json::to_value(one).unwrap_or_else(|_| json!({}))
}

fn parse_calendar(function: &ToolCallFunction) -> Result<FetchCalendarArgs, String> {
    let arguments = function.arguments()?;

    if arguments.is_null() {
        return Ok(FetchCalendarArgs::default());
    }

    serde_json::from_value(arguments)
        .map_err(|error| format!("could not read the arguments: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::migrated_pool;
    use crate::llama::test_support::serve;

    /// One page of GitHub search results, trimmed to the fields we read.
    const SEARCH_RESULTS: &str = r#"{
        "total_count": 1,
        "items": [{
            "number": 12,
            "title": "Add the tool calling orchestrator",
            "repository_url": "https://api.github.com/repos/scottmallinson/chief.ai",
            "state": "open",
            "draft": false,
            "html_url": "https://github.com/scottmallinson/chief.ai/pull/12",
            "updated_at": "2026-08-19T14:00:00Z"
        }]
    }"#;

    /// A call as it arrives from the engine: the arguments are a JSON string,
    /// not an object.
    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall::new(
            "call_1",
            ToolCallFunction {
                name: name.to_string(),
                arguments: match arguments {
                    Value::Null => String::new(),
                    other => other.to_string(),
                },
            },
        )
    }

    /// A context whose GitHub client talks to `host`.
    async fn context(host: &str) -> Context {
        Context {
            pool: migrated_pool().await,
            github: github::Client::against(host).expect("should build a client"),
            microsoft: microsoft::Client::against(host).expect("should build a client"),
        }
    }

    /// Connect one more GitHub account, named after itself the way a real
    /// sign-in names one.
    async fn connect(context: &Context, account_key: &str) {
        integrations::save(
            &context.pool,
            integrations::NewAccount {
                service: integrations::GITHUB,
                account_key,
                identity: Some(account_key),
                credential_kind: integrations::OAUTH,
                access_token: "gho_token",
                refresh_token: None,
                expires_at: None,
                scopes: None,
                client_id: None,
                client_secret: None,
            },
        )
        .await
        .expect("should store a credential");
    }

    /// The same as [`context`], with one GitHub account connected.
    async fn connected(host: &str) -> Context {
        let context = context(host).await;
        connect(&context, "octocat").await;

        context
    }

    const CALENDAR: &str = r#"{"value":[{
        "subject":"1:1 with Sam",
        "start":{"dateTime":"2026-08-28T10:00:00.0000000","timeZone":"UTC"},
        "end":{"dateTime":"2026-08-28T10:30:00.0000000","timeZone":"UTC"},
        "organizer":{"emailAddress":{"name":"Sam Patel","address":"sam@example.com"}},
        "attendees":[{"emailAddress":{"name":"Sam Patel","address":"sam@example.com"}},
                     {"emailAddress":{"address":"noname@example.com"}}],
        "isOnlineMeeting":true
    }]}"#;

    const INBOX: &str = r#"{"value":[{
        "subject":"Re: the migration",
        "from":{"emailAddress":{"name":"Dana Reid","address":"dana@example.com"}},
        "receivedDateTime":"2026-08-28T08:12:00Z",
        "bodyPreview":"Can you take a look before Friday?",
        "isRead":false
    }]}"#;

    /// A context with one Outlook account connected, talking to `host`.
    async fn outlook(host: &str) -> Context {
        let context = context(host).await;

        integrations::save(
            &context.pool,
            integrations::NewAccount {
                service: integrations::MICROSOFT,
                account_key: "someone@example.com",
                identity: Some("someone@example.com"),
                credential_kind: integrations::OAUTH,
                access_token: "graph_token",
                refresh_token: None,
                expires_at: None,
                scopes: None,
                client_id: None,
                client_secret: None,
            },
        )
        .await
        .expect("should store a credential");

        context
    }

    #[tokio::test]
    async fn reads_the_users_calendar() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", CALENDAR)]);
        let context = outlook(&host).await;

        let result = dispatch(
            &context,
            &call("fetch_calendar", json!({ "range": "today" })),
        )
        .await;
        let events = result["events"]
            .as_array()
            .unwrap_or_else(|| panic!("expected a list, got {result}"));

        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["subject"], json!("1:1 with Sam"));
        assert_eq!(events[0]["organiser"], json!("Sam Patel"));
        assert_eq!(events[0]["online"], json!(true));

        // A person with no name falls back to their address rather than being
        // dropped from the meeting.
        assert_eq!(
            events[0]["attendees"],
            json!(["Sam Patel", "noname@example.com"])
        );

        // Every entry says which mailbox it came from.
        assert_eq!(events[0]["account"], json!("someone@example.com"));

        let requests = server.await.expect("the stub should finish");
        let (request_line, _) = crate::llama::test_support::split(&requests[0]);

        assert!(
            request_line.contains("/me/calendarView"),
            "recurrences have to be expanded by Graph, not by us: {request_line}"
        );
        assert!(
            request_line.contains("startDateTime") && request_line.contains("endDateTime"),
            "the window is computed here, not by the model: {request_line}"
        );
        assert!(
            requests[0]
                .to_lowercase()
                .contains("authorization: bearer graph_token"),
            "the stored token should be sent"
        );
    }

    #[tokio::test]
    async fn reads_the_users_inbox() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", INBOX)]);
        let context = outlook(&host).await;

        let result = dispatch(&context, &call("fetch_recent_mail", json!({}))).await;
        let messages = result["messages"]
            .as_array()
            .unwrap_or_else(|| panic!("expected a list, got {result}"));

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["from"], json!("Dana Reid"));
        assert_eq!(messages[0]["unread"], json!(true));
        assert_eq!(
            messages[0]["preview"],
            json!("Can you take a look before Friday?")
        );

        let requests = server.await.expect("the stub should finish");
        let (request_line, _) = crate::llama::test_support::split(&requests[0]);

        assert!(
            request_line.contains("bodyPreview"),
            "a preview rather than whole bodies, or one read fills the window: {request_line}"
        );
    }

    #[tokio::test]
    async fn tells_the_model_when_outlook_is_not_connected() {
        // No reply is stubbed, because no request should be made: with no
        // account there is no token, and the read never leaves the machine.
        let (host, _server) = serve(Vec::<(&str, &str)>::new());
        let context = context(&host).await;

        let result = dispatch(&context, &call("fetch_calendar", json!({}))).await;

        assert!(
            result["error"]
                .as_str()
                .unwrap_or_default()
                .contains("connecting again"),
            "the model should be told what is missing, got {result}"
        );
    }

    #[test]
    fn the_catalogue_stays_inside_the_budget_it_is_paid_for_in() {
        let catalogue = catalog();

        assert!(
            catalogue.len() <= MAX_TOOLS,
            "{} tools is more than the {MAX_TOOLS} this prompt has room for",
            catalogue.len()
        );

        // Sent on every single turn, before the user has said anything, and
        // paid for in prefill. A tool added without noticing this is a tax on
        // every question thereafter.
        let rendered = serde_json::to_string(&catalogue).expect("should serialize");
        assert!(
            rendered.len() <= MAX_SCHEMA_CHARS,
            "the catalogue is {} characters, over the {MAX_SCHEMA_CHARS} budget",
            rendered.len()
        );
    }

    #[test]
    fn every_advertised_tool_has_somewhere_to_go() {
        // A name in the catalogue with no arm in the dispatcher is a tool the
        // model will call and be told does not exist.
        for tool in catalog() {
            let name = tool.function.name.clone();
            assert!(
                [FETCH_GITHUB_PRS, FETCH_CALENDAR, FETCH_RECENT_MAIL].contains(&name.as_str()),
                "{name} is advertised but the dispatcher does not know it"
            );
        }
    }

    #[test]
    fn a_named_range_becomes_instants_rather_than_leaving_the_model_to_count() {
        let (from, to) = Range::Today.window();
        assert!(from.contains("T00:00:00"), "{from}");
        assert!(to > from);

        let (tomorrow_from, _) = Range::Tomorrow.window();
        assert!(tomorrow_from > from, "tomorrow starts after today");

        let (_, week_to) = Range::Week.window();
        assert!(week_to > to, "a week reaches further than a day");
    }

    #[test]
    fn advertises_the_github_tool_in_the_openai_format() {
        let tools = catalog();
        let schema = serde_json::to_value(&tools).expect("should serialize");

        assert_eq!(schema[0]["type"], json!("function"));
        assert_eq!(schema[0]["function"]["name"], json!("fetch_github_prs"));
        assert_eq!(schema[0]["function"]["parameters"]["type"], json!("object"));
        assert_eq!(
            schema[0]["function"]["parameters"]["properties"]["state"]["enum"],
            json!(["open", "closed", "merged", "all"])
        );
    }

    #[tokio::test]
    async fn reads_the_users_pull_requests() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", SEARCH_RESULTS)]);
        let context = connected(&host).await;

        let result = dispatch(&context, &call("fetch_github_prs", json!({}))).await;
        let prs = result["pull_requests"]
            .as_array()
            .unwrap_or_else(|| panic!("expected a list, got {result}"));

        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0]["number"], json!(12));
        assert_eq!(prs[0]["repository"], json!("scottmallinson/chief.ai"));
        assert_eq!(prs[0]["state"], json!("open"));

        let requests = server.await.expect("the stub should finish");
        let (request_line, _) = crate::llama::test_support::split(&requests[0]);

        assert!(
            request_line.contains("/search/issues"),
            "unexpected request: {request_line}"
        );
        assert!(
            request_line.contains("is%3Apr") && request_line.contains("is%3Aopen"),
            "the query should ask for the user's open pull requests: {request_line}"
        );
        assert!(
            // Header names arrive lowercased on the wire.
            requests[0]
                .to_lowercase()
                .contains("authorization: bearer gho_token"),
            "the stored token should be sent"
        );
    }

    #[tokio::test]
    async fn asks_github_for_the_requested_state() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", SEARCH_RESULTS)]);
        let context = connected(&host).await;

        dispatch(
            &context,
            &call("fetch_github_prs", json!({ "state": "closed" })),
        )
        .await;

        let requests = server.await.expect("the stub should finish");
        let (request_line, _) = crate::llama::test_support::split(&requests[0]);

        assert!(
            request_line.contains("is%3Aclosed"),
            "unexpected request: {request_line}"
        );
    }

    #[tokio::test]
    async fn reads_every_account_and_says_which_is_which() {
        // Answering out of one account was answering about half of a two-account
        // user's work, confidently and with nothing to say so.
        let (host, server) = serve(vec![
            ("HTTP/1.1 200 OK", SEARCH_RESULTS),
            ("HTTP/1.1 200 OK", SEARCH_RESULTS),
        ]);
        let context = connected(&host).await;
        connect(&context, "hubot").await;

        let result = dispatch(&context, &call("fetch_github_prs", json!({}))).await;
        let prs = result["pull_requests"]
            .as_array()
            .unwrap_or_else(|| panic!("expected a list, got {result}"));

        assert_eq!(prs.len(), 2, "both accounts should have been read");
        assert_eq!(result["count"], json!(2));
        assert_eq!(prs[0]["account"], json!("octocat"));
        assert_eq!(prs[1]["account"], json!("hubot"));

        // And a per-account tally, so the model can say what it looked at even
        // when an account has nothing to report.
        let accounts = result["accounts"]
            .as_array()
            .unwrap_or_else(|| panic!("expected a report per account, got {result}"));

        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0]["account"], json!("octocat"));
        assert_eq!(accounts[0]["count"], json!(1));

        assert_eq!(
            server.await.expect("the stub should finish").len(),
            2,
            "each account is a read of its own, with its own token"
        );
    }

    #[tokio::test]
    async fn answers_with_the_accounts_it_could_read() {
        // One connection needing attention is worth saying; it is not worth
        // withholding the work the other account did answer for.
        let (host, server) = serve(vec![
            (
                "HTTP/1.1 401 Unauthorized",
                r#"{"message":"Bad credentials"}"#,
            ),
            ("HTTP/1.1 200 OK", SEARCH_RESULTS),
        ]);
        let context = connected(&host).await;
        connect(&context, "hubot").await;

        let result = dispatch(&context, &call("fetch_github_prs", json!({}))).await;
        let prs = result["pull_requests"]
            .as_array()
            .unwrap_or_else(|| panic!("expected a list, got {result}"));

        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0]["account"], json!("hubot"));

        let accounts = result["accounts"]
            .as_array()
            .unwrap_or_else(|| panic!("expected a report per account, got {result}"));

        assert!(
            accounts[0]["error"]
                .as_str()
                .is_some_and(|message| message.contains("Reconnect GitHub")),
            "the refused account should say so: {result}"
        );

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn calls_an_account_what_the_user_called_it() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", SEARCH_RESULTS)]);
        let context = connected(&host).await;
        let account = integrations::accounts(&context.pool, integrations::GITHUB)
            .await
            .expect("should read")[0]
            .id;

        integrations::set_label(&context.pool, account, Some("Work"))
            .await
            .expect("should label");

        let result = dispatch(&context, &call("fetch_github_prs", json!({}))).await;

        assert_eq!(
            result["pull_requests"][0]["account"],
            json!("Work"),
            "a name the user gave the account is the one they will recognise"
        );

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn asks_github_for_merged_pull_requests() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", SEARCH_RESULTS)]);
        let context = connected(&host).await;

        dispatch(
            &context,
            &call("fetch_github_prs", json!({ "state": "merged" })),
        )
        .await;

        let requests = server.await.expect("the stub should finish");
        let (request_line, _) = crate::llama::test_support::split(&requests[0]);

        assert!(
            request_line.contains("is%3Amerged"),
            "unexpected request: {request_line}"
        );
    }

    #[tokio::test]
    async fn tells_the_model_when_github_is_not_connected() {
        let context = context("http://127.0.0.1:1").await;

        let result = dispatch(&context, &call("fetch_github_prs", json!({}))).await;

        assert!(
            result["error"]
                .as_str()
                .is_some_and(|message| message.contains("not connected")),
            "got {result}"
        );
    }

    #[tokio::test]
    async fn tells_the_model_when_the_token_was_rejected() {
        let (host, server) = serve(vec![(
            "HTTP/1.1 401 Unauthorized",
            r#"{"message":"Bad credentials"}"#,
        )]);
        let context = connected(&host).await;

        let result = dispatch(&context, &call("fetch_github_prs", json!({}))).await;

        assert!(
            result["error"]
                .as_str()
                .is_some_and(|message| message.contains("Reconnect GitHub")),
            "got {result}"
        );

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn copes_with_missing_arguments() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", SEARCH_RESULTS)]);
        let context = connected(&host).await;

        let result = dispatch(&context, &call("fetch_github_prs", Value::Null)).await;

        assert!(result["pull_requests"].is_array(), "got {result}");
        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn tells_the_model_when_arguments_make_no_sense() {
        let context = context("http://127.0.0.1:1").await;

        let result = dispatch(
            &context,
            &call("fetch_github_prs", json!({ "state": "sideways" })),
        )
        .await;

        assert!(
            result["error"].is_string(),
            "expected an error payload, got {result}"
        );
    }

    #[tokio::test]
    async fn tells_the_model_when_a_tool_does_not_exist() {
        let context = context("http://127.0.0.1:1").await;

        let result = dispatch(&context, &call("send_everything_to_the_cloud", json!({}))).await;

        assert!(
            result["error"]
                .as_str()
                .is_some_and(|message| message.contains("no tool called")),
            "got {result}"
        );
    }
}
