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
use crate::session::GithubSession;

/// What the tools need to do their work: the user's local database, and a
/// client for the services they have connected.
///
/// Passing this explicitly, rather than reaching into the Tauri app, keeps the
/// tools runnable in tests against an in-memory database and a stub server.
#[derive(Debug, Clone)]
pub struct Context {
    pub pool: SqlitePool,
    pub github: github::Client,
}

/// Names of the tools we advertise, so the dispatcher and the catalogue cannot
/// drift apart.
const FETCH_GITHUB_PRS: &str = "fetch_github_prs";

/// Which pull requests the model is asking about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PullRequestState {
    #[default]
    Open,
    Closed,
    All,
}

impl From<PullRequestState> for PrState {
    fn from(state: PullRequestState) -> Self {
        match state {
            PullRequestState::Open => PrState::Open,
            PullRequestState::Closed => PrState::Closed,
            PullRequestState::All => PrState::All,
        }
    }
}

/// How many pull requests to read in one go.
const PR_LIMIT: u8 = 25;

/// Arguments to [`FETCH_GITHUB_PRS`], as the model produced them.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct FetchGithubPrsArgs {
    state: PullRequestState,
}

/// The tools offered to the model on every turn.
pub fn catalog() -> Vec<Tool> {
    vec![Tool::function(ToolFunction {
        name: FETCH_GITHUB_PRS.to_string(),
        description: "Fetch the user's GitHub pull requests, including which are still \
             unmerged and who they are waiting on. Call this whenever the user asks about \
             pull requests, code review, or what they have shipped."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "state": {
                    "type": "string",
                    "enum": ["open", "closed", "all"],
                    "description": "Which pull requests to return. Defaults to open.",
                },
            },
            "required": [],
        }),
    })]
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

/// Read the user's pull requests from GitHub with their own token.
///
/// Reads the first connected account. Asking across several accounts is a
/// question the tool schema cannot yet express, and inventing an argument the
/// model would have to guess at would make answers worse, not better.
async fn fetch_github_prs(
    context: &Context,
    state: PullRequestState,
) -> Result<Value, github::Error> {
    let account = integrations::accounts(&context.pool, integrations::GITHUB)
        .await?
        .into_iter()
        .next()
        .ok_or(github::Error::NotConnected)?;

    let pull_requests = GithubSession::new(&context.pool, &context.github, account.id)
        .pull_requests(state.into(), PR_LIMIT)
        .await?;

    Ok(github::as_tool_result(&pull_requests))
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
        }
    }

    /// The same, with one GitHub account connected.
    async fn connected(host: &str) -> Context {
        let context = context(host).await;
        integrations::save(
            &context.pool,
            integrations::NewAccount {
                service: integrations::GITHUB,
                account_key: "octocat",
                identity: Some("octocat"),
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

        context
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
            json!(["open", "closed", "all"])
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
