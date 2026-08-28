//! The background work log.
//!
//! Periodically, for every connected account: ask GitHub what the user merged,
//! ask the local model to turn each one into a single-sentence achievement, and
//! write it to `work_logs`.
//! Both halves stay on the user's terms — GitHub is an account they connected,
//! and the summarising model runs on this machine.
//!
//! Every pass is idempotent: entries carry the pull request's identifier and the
//! account that read it, so a merge already in the log is never written again.

use std::time::Duration;

use tauri::{AppHandle, Manager, Runtime};

use crate::agent::Attention;
use crate::db;
use crate::github::{self, PullRequest, State};
use crate::integrations;
use crate::llama::{self, ChatRequest, Message, Options};
use crate::session::GithubSession;
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

/// A sentence, and the model should stop there rather than write an essay
/// nobody reads. Nothing else about how the engine runs is touched: the
/// context window belongs to the server, not to a request.
const SUMMARY_OPTIONS: Options = Options::new().with_answer_length(80);

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
    Summary(#[from] llama::Error),
}

/// Everything a pass needs. Passed in so the whole thing runs in tests against
/// an in-memory database and stub servers.
pub struct Context {
    pub pool: sqlx::SqlitePool,
    pub github: github::Client,
    pub engine: llama::Client,
    /// Whether the user is waiting on an answer right now.
    pub attention: Attention,
}

/// Start the daemon. Returns immediately; the work happens on the async runtime.
pub fn spawn<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_PASS_DELAY).await;

        loop {
            // The engine gives its memory back when nothing is using it, so a
            // pass may have to start it. Failing to is not fatal and not worth
            // reporting twice: the pass below will say so in its own terms.
            {
                let engine = app.state::<crate::engine::Engine>();
                let client = app.state::<llama::Client>();
                let _ = engine.ensure_running(client.inner()).await;
            }

            match context(&app).await {
                Ok(context) => {
                    // A pass failing is not fatal: GitHub may be unreachable or
                    // the engine may still be loading. Try again next time.
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
        engine: app.state::<llama::Client>().inner().clone(),
        attention: app.state::<Attention>().inner().clone(),
    })
}

/// Run one pass over every connected account. Returns how many new entries
/// were written.
///
/// Does nothing at all when nothing is connected — there is no work to read and
/// nothing to report. A person with a work and a personal account is two
/// separate readings of GitHub, because the credentials are separate — and
/// separate is what they stay when one of them fails.
pub async fn run_once(context: &Context) -> Result<usize, Error> {
    let accounts = integrations::accounts(&context.pool, integrations::GITHUB).await?;
    let mut written = 0;

    for account in accounts {
        // The user's question is worth more than the log being current, and the
        // engine decodes one request at a time. Stopping between accounts as
        // well as between items keeps a second account from starting a read the
        // pass is about to abandon anyway.
        if context.attention.is_engaged() {
            break;
        }

        // A failure belongs to the account it happened to, not to the pass. A
        // token the user revoked on a personal account says nothing about
        // whether their work account can be read, and ending the pass on the
        // first refusal would freeze every later account's log until they
        // noticed which one was at fault. Reported against the id, the way a
        // name that could not be read is, and tried again next pass.
        match run_one_account(context, &account).await {
            Ok(entries) => written += entries,
            Err(error) => eprintln!("could not read account {}: {error}", account.id),
        }
    }

    Ok(written)
}

/// Read one account's merged work and log whatever is new.
async fn run_one_account(
    context: &Context,
    account: &integrations::Account,
) -> Result<usize, Error> {
    let session = GithubSession::new(&context.pool, &context.github, account.id);

    // An account carried over from before Chief stored identities has none, and
    // the settings screen has nothing to show but the placeholder key. Fill it
    // in on the next read rather than making the user reconnect for a name.
    //
    // Naming is not the work, so a failure here is reported and stepped over.
    // `name_account` asks GitHub who this is without renewing, where the read
    // below does — letting it abort the pass would strand an account whose only
    // problem is a token that has rotated, which is exactly the migrated row
    // this backfill exists for.
    if account.identity.is_none() {
        if let Err(error) = session.name_account().await {
            eprintln!("could not name account {}: {error}", account.id);
        }
    }

    let merged = session.pull_requests(State::Merged, BATCH).await?;

    let mut written = 0;

    for pull_request in merged {
        // Summarising uses the same local model the user is talking to, and
        // the engine decodes one request at a time: carrying on here would put
        // their question behind ours. The rest keeps until the next pass.
        if context.attention.is_engaged() {
            break;
        }

        if log_one(context, account.id, &pull_request).await? {
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
async fn log_one(
    context: &Context,
    account_id: i64,
    pull_request: &PullRequest,
) -> Result<bool, Error> {
    let external_id = pull_request.external_id();

    if work_log::has_logged(&context.pool, "github", account_id, &external_id).await? {
        return Ok(false);
    }

    let content = describe(pull_request);

    // If the model is unreachable we write nothing, rather than filling the log
    // with unsummarised rows that would never be revisited — the entry is
    // picked up on a later pass instead.
    let summary = summarise(&context.engine, &content).await?;

    let entry = NewWorkLogEntry {
        source: "github".to_string(),
        content,
        timestamp: pull_request
            .merged_at
            .clone()
            .or_else(|| Some(pull_request.updated_at.clone())),
        summary: Some(summary),
        external_id: Some(external_id),
        account_id: Some(account_id),
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
async fn summarise(client: &llama::Client, activity: &str) -> Result<String, llama::Error> {
    let request = ChatRequest::new(
        SUMMARY_MODEL,
        vec![
            Message::system(SUMMARY_PROMPT),
            Message::user(activity.to_string()),
        ],
    )
    .with_options(SUMMARY_OPTIONS);

    let reply = client.chat(&request).await?;

    Ok(tidy(&reply.content))
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
    use crate::llama::test_support::{answer, serve};

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

    const VIEWER: &str = r#"{"login":"octocat","name":"The Octocat"}"#;

    /// Connect one GitHub account and return its id.
    ///
    /// `identity` is what the provider said the account is. Passing `Some`
    /// keeps a pass from asking GitHub who this is, which is a request a test
    /// counting requests would otherwise have to stub.
    async fn connect(pool: &sqlx::SqlitePool, account_key: &str, identity: Option<&str>) -> i64 {
        integrations::save(
            pool,
            integrations::NewAccount {
                service: integrations::GITHUB,
                account_key,
                identity,
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
        .expect("should store a credential")
        .id
    }

    async fn context_for(github_host: &str, engine_host: &str, connected: bool) -> Context {
        let pool = migrated_pool().await;

        if connected {
            connect(&pool, "octocat", Some("octocat")).await;
        }

        Context {
            pool,
            github: github::Client::against(github_host).expect("should build a github client"),
            engine: llama::Client::with_base_url(engine_host).expect("loopback is allowed"),
            attention: Attention::default(),
        }
    }

    #[tokio::test]
    async fn steps_aside_while_the_user_is_waiting_on_an_answer() {
        // Neither host exists, so a pass that read anything at all would fail
        // rather than quietly report nothing: this proves it never started.
        let context = context_for("http://127.0.0.1:1", "http://127.0.0.1:1", true).await;
        let _waiting = context.attention.begin();

        let written = run_once(&context).await.expect("a pass should not fail");

        assert_eq!(written, 0, "the user's question comes first");
        assert!(work_log::fetch(&context.pool, None)
            .await
            .expect("read")
            .is_empty());
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
        let (engine_host, engine_server) = serve(vec![
            (
                "HTTP/1.1 200 OK",
                answer("Shipped the tool calling orchestrator."),
            ),
            ("HTTP/1.1 200 OK", answer("Shipped the local database.")),
        ]);

        let context = context_for(&github_host, &engine_host, true).await;
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
        engine_server.await.expect("engine stub should finish");
    }

    #[tokio::test]
    async fn asks_github_only_for_merged_work() {
        let (github_host, github_server) = serve(vec![("HTTP/1.1 200 OK", NO_PRS)]);
        let context = context_for(&github_host, "http://127.0.0.1:1", true).await;

        run_once(&context).await.expect("a pass should not fail");

        let requests = github_server.await.expect("github stub should finish");
        let (request_line, _) = crate::llama::test_support::split(&requests[0]);

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
        let (engine_host, engine_server) = serve(vec![
            ("HTTP/1.1 200 OK", answer("Shipped the orchestrator.")),
            ("HTTP/1.1 200 OK", answer("Shipped the database.")),
        ]);

        let context = context_for(&github_host, &engine_host, true).await;

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
        engine_server.await.expect("engine stub should finish");
    }

    #[tokio::test]
    async fn logs_the_same_merge_once_for_each_account_that_made_it() {
        // Both accounts read the same stub, so both see the same two merges.
        // They are still four entries: whose work it was is part of what the
        // log records, and the dedupe key says so too.
        let (github_host, github_server) = serve(vec![
            ("HTTP/1.1 200 OK", MERGED_PRS),
            ("HTTP/1.1 200 OK", MERGED_PRS),
        ]);
        let (engine_host, engine_server) = serve(vec![
            ("HTTP/1.1 200 OK", answer("Shipped the orchestrator.")),
            ("HTTP/1.1 200 OK", answer("Shipped the database.")),
            ("HTTP/1.1 200 OK", answer("Shipped the orchestrator.")),
            ("HTTP/1.1 200 OK", answer("Shipped the database.")),
        ]);

        let context = context_for(&github_host, &engine_host, true).await;
        let second = connect(&context.pool, "hubot", Some("hubot")).await;

        assert_eq!(run_once(&context).await.expect("a pass should not fail"), 4);
        assert!(
            work_log::has_logged(
                &context.pool,
                "github",
                second,
                "scottmallinson/chief.ai#12"
            )
            .await
            .expect("read"),
            "the second account should have logged it too"
        );

        github_server.await.expect("github stub should finish");
        engine_server.await.expect("engine stub should finish");
    }

    #[tokio::test]
    async fn reads_every_account_even_when_one_is_refused() {
        // A credential is refused per account: a personal token the user
        // revoked months ago says nothing about whether their work account can
        // be read. Letting the first one end the pass would mean the log never
        // moves again, on the account they care about, until they notice the
        // other one and disconnect it.
        let (github_host, github_server) = serve(vec![
            (
                "HTTP/1.1 401 Unauthorized",
                r#"{"message":"Bad credentials"}"#,
            ),
            ("HTTP/1.1 200 OK", MERGED_PRS),
        ]);
        let (engine_host, engine_server) = serve(vec![
            ("HTTP/1.1 200 OK", answer("Shipped the orchestrator.")),
            ("HTTP/1.1 200 OK", answer("Shipped the database.")),
        ]);

        // The refused account is the one read first, by id.
        let context = context_for(&github_host, &engine_host, true).await;
        let healthy = connect(&context.pool, "hubot", Some("hubot")).await;

        let written = run_once(&context)
            .await
            .expect("one account's refusal is not a failed pass");

        assert_eq!(written, 2, "the healthy account should have been read");
        assert!(
            work_log::has_logged(
                &context.pool,
                "github",
                healthy,
                "scottmallinson/chief.ai#12"
            )
            .await
            .expect("read"),
            "the healthy account's work should be in the log"
        );

        github_server.await.expect("github stub should finish");
        engine_server.await.expect("engine stub should finish");
    }

    #[tokio::test]
    async fn names_an_account_that_arrived_without_an_identity() {
        // The row a v1 database was migrated from has no identity, because the
        // old table never stored one. A pass fills it in rather than asking the
        // user to reconnect for a name.
        let (github_host, github_server) = serve(vec![
            ("HTTP/1.1 200 OK", VIEWER),
            ("HTTP/1.1 200 OK", NO_PRS),
        ]);
        let context = context_for(&github_host, "http://127.0.0.1:1", false).await;
        connect(&context.pool, "github", None).await;

        run_once(&context).await.expect("a pass should not fail");

        let account = integrations::accounts(&context.pool, integrations::GITHUB)
            .await
            .expect("should read")
            .into_iter()
            .next()
            .expect("the account should be there");

        assert_eq!(account.identity.as_deref(), Some("octocat"));

        github_server.await.expect("github stub should finish");
    }

    #[tokio::test]
    async fn reads_an_account_it_could_not_name() {
        // Asking who an account is does not renew a rotated token, but reading
        // its work does. A refused name must therefore step aside for the read
        // rather than take the account down with it.
        let (github_host, github_server) = serve(vec![
            (
                "HTTP/1.1 401 Unauthorized",
                r#"{"message":"Bad credentials"}"#,
            ),
            ("HTTP/1.1 200 OK", NO_PRS),
        ]);
        let context = context_for(&github_host, "http://127.0.0.1:1", false).await;
        connect(&context.pool, "github", None).await;

        run_once(&context)
            .await
            .expect("a name we could not read is not a failed pass");

        let requests = github_server.await.expect("github stub should finish");
        assert_eq!(requests.len(), 2, "the read should still have happened");
    }

    #[tokio::test]
    async fn writes_nothing_when_the_model_is_unreachable() {
        let (github_host, github_server) = serve(vec![
            ("HTTP/1.1 200 OK", MERGED_PRS),
            ("HTTP/1.1 200 OK", MERGED_PRS),
        ]);
        let context = context_for(&github_host, "http://127.0.0.1:1", true).await;
        let account = integrations::accounts(&context.pool, integrations::GITHUB)
            .await
            .expect("should read")
            .into_iter()
            .next()
            .expect("the account should be connected");

        // Reading the account fails, and says so in the terms the caller needs.
        let error = run_one_account(&context, &account)
            .await
            .expect_err("an unreachable model should surface");

        assert!(matches!(error, Error::Summary(_)), "got {error:?}");

        // The pass over the accounts reports that and carries on, so an engine
        // still loading does not take the whole thing down.
        assert_eq!(
            run_once(&context)
                .await
                .expect("a pass reports a failure rather than becoming one"),
            0
        );
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
