//! Tools the agent can call.
//!
//! A tool is a schema the model sees plus a function we run on its behalf.
//! Everything runs locally: a tool either reads the user's own database or
//! calls a service the user has explicitly connected, with their own token.
//!
//! `fetch_github_prs` currently answers with a fixed sample. Step 5 replaces
//! that body with a real request once GitHub OAuth stores a token.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::ollama::{Tool, ToolCall, ToolFunction};

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
pub async fn dispatch(call: &ToolCall) -> Value {
    match call.function.name.as_str() {
        FETCH_GITHUB_PRS => match parse(&call.function.arguments) {
            Ok(args) => fetch_github_prs(args.state).await,
            Err(message) => json!({ "error": message }),
        },
        unknown => json!({
            "error": format!("there is no tool called '{unknown}'"),
        }),
    }
}

fn parse(arguments: &Value) -> Result<FetchGithubPrsArgs, String> {
    // Models sometimes send an empty value instead of an empty object.
    if arguments.is_null() {
        return Ok(FetchGithubPrsArgs::default());
    }

    serde_json::from_value(arguments.clone())
        .map_err(|error| format!("could not read the arguments: {error}"))
}

/// Placeholder pull requests.
///
/// Replaced in step 5 by a real request to `api.github.com` made from here with
/// the user's own token.
async fn fetch_github_prs(state: PullRequestState) -> Value {
    let sample = json!([
        {
            "number": 4,
            "title": "Scaffold the desktop app and its toolchain",
            "repository": "scottmallinson/chief.ai",
            "state": "closed",
            "merged": true,
            "waiting_on": null,
        },
        {
            "number": 12,
            "title": "Add the tool calling orchestrator",
            "repository": "scottmallinson/chief.ai",
            "state": "open",
            "merged": false,
            "waiting_on": "review",
        },
    ]);

    let pull_requests: Vec<Value> = sample
        .as_array()
        .expect("the sample is an array")
        .iter()
        .filter(|pr| match state {
            PullRequestState::All => true,
            PullRequestState::Open => pr["state"] == "open",
            PullRequestState::Closed => pr["state"] == "closed",
        })
        .cloned()
        .collect();

    json!({
        "pull_requests": pull_requests,
        "note": "Sample data. GitHub is not connected yet.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ollama::ToolCallFunction;

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            function: ToolCallFunction {
                name: name.to_string(),
                arguments,
            },
        }
    }

    #[test]
    fn advertises_the_github_tool_in_ollamas_format() {
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
    async fn returns_open_pull_requests_by_default() {
        let result = dispatch(&call("fetch_github_prs", json!({}))).await;
        let prs = result["pull_requests"]
            .as_array()
            .expect("should be a list");

        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0]["number"], json!(12));
    }

    #[tokio::test]
    async fn honours_the_requested_state() {
        let result = dispatch(&call("fetch_github_prs", json!({ "state": "all" }))).await;
        assert_eq!(result["pull_requests"].as_array().unwrap().len(), 2);

        let result = dispatch(&call("fetch_github_prs", json!({ "state": "closed" }))).await;
        let prs = result["pull_requests"]
            .as_array()
            .expect("should be a list");

        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0]["merged"], json!(true));
    }

    #[tokio::test]
    async fn copes_with_missing_arguments() {
        let result = dispatch(&call("fetch_github_prs", Value::Null)).await;

        assert!(result["pull_requests"].is_array(), "got {result}");
    }

    #[tokio::test]
    async fn tells_the_model_when_arguments_make_no_sense() {
        let result = dispatch(&call("fetch_github_prs", json!({ "state": "sideways" }))).await;

        assert!(
            result["error"].is_string(),
            "expected an error payload, got {result}"
        );
    }

    #[tokio::test]
    async fn tells_the_model_when_a_tool_does_not_exist() {
        let result = dispatch(&call("send_everything_to_the_cloud", json!({}))).await;

        assert!(
            result["error"]
                .as_str()
                .is_some_and(|message| message.contains("no tool called")),
            "got {result}"
        );
    }
}
