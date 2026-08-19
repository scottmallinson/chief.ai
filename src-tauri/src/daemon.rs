//! The background work log.
//!
//! Periodically: ask GitHub what the user merged, ask the local model to turn
//! each one into a single-sentence achievement, and write it to `work_logs`.
//! Both halves stay on the user's terms — GitHub is an account they connected,
//! and the summarising model runs on this machine.
//!
//! Every pass is idempotent: entries carry the pull request's identifier, so a
//! merge already in the log is never written again.

use std::time::Duration;

use tauri::{AppHandle, Manager, Runtime};

use crate::db;
use crate::github::{self, PullRequest, State};
use crate::integrations;
use crate::ollama::{self, ChatRequest, Message};
use crate::work_log::{self, NewWorkLogEntry};

/// How long to settle after launch before the first pass, so startup is not
/// competing with a model.
const FIRST_PASS_DELAY: Duration = Duration::from_secs(20);

/// How often to look for new work afterwards.
const PASS_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// How many merged pull requests to consider in one pass.
const BATCH: u8 = 25;

/// Summaries should be a sentence, so this needs very few tokens.
const SUMMARY_MODEL: &str = crate::agent::DEFAULT_MODEL;

const SUMMARY_PROMPT: &str = "\
You turn a developer's activity into a work log. Summarise the activity into a
single sentence describing what they achieved, in the past tense. Do not add
commentary, quotes, bullet points, or a preamble — reply with the sentence and
nothing else.";

/// What can go wrong during a pass.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Github(#[from] github::Error),
    #[error(transparent)]
    Storage(#[from] db::Error),
    #[error("could not summarise the activity: {0}")]
    Summary(#[from] ollama::Error),
}

/// Everything a pass needs. Passed in so the whole thing runs in tests against
/// an in-memory database and stub servers.
pub struct Context {
    pub pool: sqlx::SqlitePool,
    pub github: github::Client,
    pub ollama: ollama::Client,
}

/// Start the daemon. Returns immediately; the work happens on the async runtime.
pub fn spawn<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_PASS_DELAY).await;

        loop {
            match context(&app).await {
                Ok(context) => {
                    // A pass failing is not fatal: GitHub may be unreachable or
                    // Ollama may not be running. Try again next time.
                    if let Err(error) = run_once(&context).await {
                        eprintln!("work log pass failed: {error}");
                    }
                }
                Err(error) => eprintln!("work log pass could not start: {error}"),
            }

            tokio::time::sleep(PASS_INTERVAL).await;
        }
    });
}

async fn context<R: Runtime>(app: &AppHandle<R>) -> Result<Context, db::Error> {
    Ok(Context {
        pool: db::pool(app).await?,
        github: app.state::<github::Client>().inner().clone(),
        ollama: app.state::<ollama::Client>().inner().clone(),
    })
}

/// Run one pass. Returns how many new entries were written.
///
/// Does nothing at all when GitHub is not connected — there is no work to read
/// and nothing to report.
pub async fn run_once(context: &Context) -> Result<usize, Error> {
    let Some(token) = integrations::token(&context.pool, integrations::GITHUB).await? else {
        return Ok(0);
    };

    let merged = context
        .github
        .pull_requests(&token, State::Merged, BATCH)
        .await?;

    let mut written = 0;

    for pull_request in merged {
        if log_one(context, &pull_request).await? {
            written += 1;
        }
    }

    Ok(written)
}

/// Summarise one merged pull request and add it to the log.
///
/// Returns whether anything was written. A pull request already in the log is
/// skipped before the model is asked, so a pass costs nothing once it has
/// caught up.
async fn log_one(context: &Context, pull_request: &PullRequest) -> Result<bool, Error> {
    let external_id = pull_request.external_id();

    if work_log::has_logged(&context.pool, "github", &external_id).await? {
        return Ok(false);
    }

    let content = describe(pull_request);

    // If the model is unreachable we write nothing, rather than filling the log
    // with unsummarised rows that would never be revisited — the entry is
    // picked up on a later pass instead.
    let summary = summarise(&context.ollama, &content).await?;

    let entry = NewWorkLogEntry {
        source: "github".to_string(),
        content,
        timestamp: pull_request
            .merged_at
            .clone()
            .or_else(|| Some(pull_request.updated_at.clone())),
        summary: Some(summary),
        external_id: Some(external_id),
    };

    Ok(work_log::insert_new(&context.pool, entry).await?.is_some())
}

/// The raw activity, as the model sees it and as the log keeps it.
fn describe(pull_request: &PullRequest) -> String {
    format!(
        "Merged pull request #{} in {}: {}",
        pull_request.number, pull_request.repository, pull_request.title
    )
}

/// Ask the local model for a one-sentence achievement.
async fn summarise(client: &ollama::Client, activity: &str) -> Result<String, ollama::Error> {
    let request = ChatRequest::new(
        SUMMARY_MODEL,
        vec![
            Message::system(SUMMARY_PROMPT),
            Message::user(activity.to_string()),
        ],
    );

    let reply = client.chat(&request).await?;

    Ok(tidy(&reply.message.content))
}

/// Small models like to wrap an answer in quotes or a preamble. Take the first
/// sentence-ish line and strip the decoration.
fn tidy(summary: &str) -> String {
    summary
        .trim()
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
        .trim_start_matches("Summary:")
        .trim_start_matches("Achievement:")
        .trim()
        .trim_matches('"')
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::migrated_pool;
    use crate::ollama::test_support::serve;

    const MERGED_PRS: &str = r#"{
        "total_count": 2,
        "items": [
            {
                "number": 12,
                "title": "Add the tool calling orchestrator",
                "repository_url": "https://api.github.com/repos/scottmallinson/chief.ai",
                "state": "closed",
                "draft": false,
                "html_url": "https://github.com/scottmallinson/chief.ai/pull/12",
                "updated_at": "2026-08-19T14:00:00Z",
                "pull_request": { "merged_at": "2026-08-19T13:58:00Z" }
            },
            {
                "number": 9,
                "title": "Add the local database",
                "repository_url": "https://api.github.com/repos/scottmallinson/chief.ai",
                "state": "closed",
                "draft": false,
                "html_url": "https://github.com/scottmallinson/chief.ai/pull/9",
                "updated_at": "2026-08-18T10:00:00Z",
                "pull_request": { "merged_at": "2026-08-18T09:58:00Z" }
            }
        ]
    }"#;

    const NO_PRS: &str = r#"{"total_count":0,"items":[]}"#;

    fn summary_reply(sentence: &str) -> String {
        format!(
            r#"{{"model":"llama3.2:3b","message":{{"role":"assistant","content":"{sentence}"}},"done":true}}"#
        )
    }

    async fn context_for(github_host: &str, ollama_host: &str, connected: bool) -> Context {
        let pool = migrated_pool().await;

        if connected {
            integrations::save(&pool, integrations::GITHUB, "gho_token", None)
                .await
                .expect("should store a token");
        }

        Context {
            pool,
            github: github::Client::against(github_host).expect("should build a github client"),
            ollama: ollama::Client::with_base_url(ollama_host).expect("loopback is allowed"),
        }
    }

    #[tokio::test]
    async fn does_nothing_when_github_is_not_connected() {
        let context = context_for("http://127.0.0.1:1", "http://127.0.0.1:1", false).await;

        let written = run_once(&context).await.expect("a pass should not fail");

        assert_eq!(written, 0);
        assert!(work_log::fetch(&context.pool, None)
            .await
            .expect("read")
            .is_empty());
    }

    #[tokio::test]
    async fn summarises_merged_pull_requests_into_the_log() {
        let (github_host, github_server) = serve(vec![("HTTP/1.1 200 OK", MERGED_PRS)]);
        let (ollama_host, ollama_server) = serve(vec![
            (
                "HTTP/1.1 200 OK",
                summary_reply("Shipped the tool calling orchestrator."),
            ),
            (
                "HTTP/1.1 200 OK",
                summary_reply("Shipped the local database."),
            ),
        ]);

        let context = context_for(&github_host, &ollama_host, true).await;
        let written = run_once(&context).await.expect("a pass should not fail");

        assert_eq!(written, 2);

        let entries = work_log::fetch(&context.pool, None).await.expect("read");
        assert_eq!(entries.len(), 2);

        // Newest first, by the time each was merged.
        assert_eq!(
            entries[0].summary.as_deref(),
            Some("Shipped the tool calling orchestrator.")
        );
        assert_eq!(entries[0].timestamp, "2026-08-19T13:58:00Z");
        assert_eq!(entries[0].source, "github");
        assert_eq!(
            entries[0].external_id.as_deref(),
            Some("scottmallinson/chief.ai#12")
        );
        assert!(
            entries[0]
                .content
                .contains("Add the tool calling orchestrator"),
            "the raw activity should be kept: {}",
            entries[0].content
        );

        github_server.await.expect("github stub should finish");
        ollama_server.await.expect("ollama stub should finish");
    }

    #[tokio::test]
    async fn asks_github_only_for_merged_work() {
        let (github_host, github_server) = serve(vec![("HTTP/1.1 200 OK", NO_PRS)]);
        let context = context_for(&github_host, "http://127.0.0.1:1", true).await;

        run_once(&context).await.expect("a pass should not fail");

        let requests = github_server.await.expect("github stub should finish");
        let (request_line, _) = crate::ollama::test_support::split(&requests[0]);

        assert!(
            request_line.contains("is%3Amerged"),
            "unexpected request: {request_line}"
        );
    }

    #[tokio::test]
    async fn running_again_logs_nothing_new() {
        let (github_host, github_server) = serve(vec![
            ("HTTP/1.1 200 OK", MERGED_PRS),
            ("HTTP/1.1 200 OK", MERGED_PRS),
        ]);
        // Only two model replies: a second pass must not ask again.
        let (ollama_host, ollama_server) = serve(vec![
            (
                "HTTP/1.1 200 OK",
                summary_reply("Shipped the orchestrator."),
            ),
            ("HTTP/1.1 200 OK", summary_reply("Shipped the database.")),
        ]);

        let context = context_for(&github_host, &ollama_host, true).await;

        assert_eq!(run_once(&context).await.expect("first pass"), 2);
        assert_eq!(
            run_once(&context).await.expect("second pass"),
            0,
            "work already in the log should be skipped"
        );
        assert_eq!(
            work_log::fetch(&context.pool, None)
                .await
                .expect("read")
                .len(),
            2
        );

        github_server.await.expect("github stub should finish");
        ollama_server.await.expect("ollama stub should finish");
    }

    #[tokio::test]
    async fn writes_nothing_when_the_model_is_unreachable() {
        let (github_host, github_server) = serve(vec![("HTTP/1.1 200 OK", MERGED_PRS)]);
        let context = context_for(&github_host, "http://127.0.0.1:1", true).await;

        let error = run_once(&context)
            .await
            .expect_err("an unreachable model should surface");

        assert!(matches!(error, Error::Summary(_)), "got {error:?}");
        assert!(
            work_log::fetch(&context.pool, None)
                .await
                .expect("read")
                .is_empty(),
            "an unsummarised entry should not be written"
        );

        github_server.await.expect("github stub should finish");
    }

    #[test]
    fn strips_the_decoration_small_models_add() {
        assert_eq!(tidy("  Shipped it.  "), "Shipped it.");
        assert_eq!(tidy("\"Shipped it.\""), "Shipped it.");
        assert_eq!(tidy("Summary: Shipped it."), "Shipped it.");
        assert_eq!(tidy("Achievement: Shipped it."), "Shipped it.");
        assert_eq!(tidy("Shipped it.\n\nLet me know!"), "Shipped it.");
        assert_eq!(tidy(""), "");
    }

    #[test]
    fn describes_activity_for_the_model() {
        let pr = PullRequest {
            number: 12,
            title: "Add the daemon".to_string(),
            repository: "scottmallinson/chief.ai".to_string(),
            state: "closed".to_string(),
            draft: false,
            url: "https://github.com/scottmallinson/chief.ai/pull/12".to_string(),
            updated_at: "2026-08-19T14:00:00Z".to_string(),
            merged_at: Some("2026-08-19T13:58:00Z".to_string()),
        };

        assert_eq!(
            describe(&pr),
            "Merged pull request #12 in scottmallinson/chief.ai: Add the daemon"
        );
    }
}
