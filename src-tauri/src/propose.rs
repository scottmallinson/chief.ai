//! Drafting the thing before the user asks for it.
//!
//! **Nothing here sends anything, and that is the point.** Drafting and sending
//! are separate steps with separate reviews, so this module reads, writes to
//! the corpus, and stops. `never_reaches_the_network_to_draft` asserts it
//! rather than trusting it: the boundary is only real if something checks.
//!
//! The item Chief proposes for is one of the user's own open pull requests that
//! has been sitting without a review. The draft is a short message asking for
//! one — the thing you would have written yourself on Friday afternoon, ready
//! on Friday morning.
//!
//! The prompt is assembled in COSTA's order, literally: **org structure, then
//! writing style, then the item**. Who these people are frames how to address
//! them, how the user writes frames the words, and only then does the thing
//! being written about arrive. Reversing it produces a draft about a pull
//! request that happens to mention a person.

use crate::context::Budget;
use crate::corpus;
use crate::github;
use crate::llama::{self, ChatRequest, Message, Options};
use crate::profile;
use crate::proposed::{self, NewProposal};
use crate::recipe;
use crate::session::GithubSession;

/// How many open pull requests a pass looks at before giving up on finding one
/// that has not been proposed for.
const SCAN: u8 = 20;

/// How old, in days, before a pull request is worth chasing.
///
/// Not a rule about review culture, a rule about not being annoying: a pull
/// request opened this morning does not need a nudge, and a Chief that drafts
/// one for it will be turned off by lunchtime.
const STALE_DAYS: i64 = 3;

/// What the draft itself may cost.
const DRAFT_TOKENS: u32 = 300;

/// What the whole prompt may cost, background included.
const PROMPT_TOKENS: u32 = 1_800;

/// What can go wrong drafting.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Engine(#[from] llama::Error),
    #[error(transparent)]
    Corpus(#[from] corpus::Error),
    #[error(transparent)]
    Storage(#[from] crate::db::Error),
    #[error(transparent)]
    Source(#[from] github::Error),
}

const INSTRUCTIONS: &str = "\
Write a short message asking a colleague to review the pull request described \
below. It will be read and edited by the person you are writing for, not sent \
for them.

Rules:
- Three sentences at most, and no subject line.
- Say what the change does, in their words, from the description below.
- Ask for a review. Do not apologise, and do not chase twice in one message.
- Write it as the person whose style is described above. If no style is \
described, write plainly.
- Output the message only. No preamble, no sign-off block, no notes.";

/// One pass: draft for at most one item, and stop.
///
/// **One per pass on purpose.** A draft is a model call, and a machine that
/// woke up to find nine stale pull requests would spend several minutes of the
/// engine on work nobody asked for — while the user, who did ask for something,
/// waits behind it. Nine passes is half a day, and these are not urgent.
///
/// Returns the proposal if one was drafted.
pub async fn run_once(
    context: &recipe::Context,
    attention: &crate::agent::Attention,
) -> Result<Option<proposed::Proposal>, Error> {
    for account in recipe::github_accounts(context).await {
        // The user's question outranks a draft nobody has asked for yet.
        if attention.is_engaged() {
            break;
        }

        let session = GithubSession::new(&context.pool, &context.github, account);
        let open = session
            .pull_requests(github::Involvement::Authored, github::State::Open, SCAN)
            .await?;

        for pull_request in open.iter().filter(|one| is_worth_chasing(one)) {
            if attention.is_engaged() {
                break;
            }

            let key = pull_request.external_id();

            // Asked before the model is, so a caught-up pass costs nothing.
            if proposed::has_proposed(&context.pool, "github", account, &key).await? {
                continue;
            }

            return Ok(Some(draft(context, account, pull_request).await?));
        }
    }

    Ok(None)
}

/// Has this been open long enough, and is it actually reviewable?
fn is_worth_chasing(pull_request: &github::PullRequest) -> bool {
    if pull_request.draft {
        return false;
    }

    let Ok(updated) = chrono::DateTime::parse_from_rfc3339(&pull_request.updated_at) else {
        // An unreadable date is not evidence of staleness.
        return false;
    };

    chrono::Utc::now().signed_duration_since(updated) > chrono::Duration::days(STALE_DAYS)
}

/// Draft for one item, and record it.
///
/// The order is what makes a failure safe. The model is asked first and its
/// answer held in memory; only once there is something to write does anything
/// touch the disk or the database. A refusal therefore leaves no half-written
/// draft and no orphan row, because neither was created yet.
async fn draft(
    context: &recipe::Context,
    account: i64,
    pull_request: &github::PullRequest,
) -> Result<proposed::Proposal, Error> {
    let body = compose(context, pull_request).await?;

    let path = format!(
        "proposed/{}-{}.md",
        recipe::today_date(),
        slug(&pull_request.external_id())
    );

    context.corpus.write(&path, &body).await?;

    let stored = proposed::insert_new(
        &context.pool,
        NewProposal {
            source: "github".to_string(),
            account_id: account,
            dedupe_key: pull_request.external_id(),
            title: format!(
                "Ask for a review on {} #{}",
                pull_request.repository, pull_request.number
            ),
            context: describe(pull_request),
            path: path.clone(),
        },
    )
    .await;

    match stored {
        Ok(Some(proposal)) => Ok(proposal),
        // Another pass got there first. The file is the same draft under the
        // same name, so there is nothing to undo.
        Ok(None) => Err(Error::Storage(crate::db::Error::NotLoaded)),
        Err(error) => {
            // No orphan file: the row is the record that a draft exists, and a
            // file with nothing pointing at it is litter in the user's folder.
            let _ = context.corpus.write(&path, "").await;
            Err(error.into())
        }
    }
}

/// How long a refined draft may run. A little more than the draft itself, so
/// "make it warmer" is not silently truncated into "make it shorter".
const REFINE_TOKENS: u32 = 400;

const REFINE_INSTRUCTIONS: &str = "\
Rewrite the message below as the instruction says. Output only the rewritten \
message: no preamble, no explanation of what you changed, no notes.";

/// Rewrite a draft to an instruction. One call, and nothing else.
///
/// **No tools, no history, no corpus** — the whole prompt is the instruction
/// and the draft. This is the cheapest call Chief makes and the one the user
/// feels most, because it is the one they make repeatedly while looking at the
/// result: "make it less formal", again, shorter this time. Loading the corpus
/// here would put a thousand tokens of prefill in front of a hundred-token
/// rewrite, and the user would feel every one of them.
#[tauri::command]
pub async fn refine_draft(
    engine: tauri::State<'_, crate::engine::Engine>,
    client: tauri::State<'_, llama::Client>,
    attention: tauri::State<'_, crate::agent::Attention>,
    body: String,
    instruction: String,
) -> Result<String, String> {
    let _waiting = attention.begin();

    engine
        .start_and_wait(client.inner())
        .await
        .map_err(|error| error.to_string())?;

    refine(client.inner(), &body, &instruction).await
}

/// The rewrite itself, so it can be tested without a Tauri app.
async fn refine(client: &llama::Client, body: &str, instruction: &str) -> Result<String, String> {
    let prompt = format!(
        "{REFINE_INSTRUCTIONS}\n\n## Instruction\n{}\n\n## Message\n{}",
        instruction.trim(),
        body.trim()
    );

    let request = ChatRequest::new(crate::agent::DEFAULT_MODEL, vec![Message::user(&prompt)])
        .with_options(Options::new().with_answer_length(REFINE_TOKENS));

    let reply = client
        .chat(&request)
        .await
        .map_err(|error| error.to_string())?;
    let rewritten = reply.content.trim().to_string();

    if rewritten.is_empty() {
        return Err("the model returned nothing to put in its place".to_string());
    }

    Ok(rewritten)
}

/// The one model call, assembled in COSTA's order.
async fn compose(
    context: &recipe::Context,
    pull_request: &github::PullRequest,
) -> Result<String, Error> {
    let mut budget = Budget::with_ceiling(PROMPT_TOKENS);
    let mut prompt = String::new();

    let mut push = |budget: &mut Budget, name: &str, block: &str| {
        if budget.add(name, block).is_ok() {
            prompt.push_str(block);
            prompt.push_str("\n\n");
        }
    };

    push(&mut budget, "instructions", INSTRUCTIONS);

    // Org structure, then writing style, then the item. See the module note:
    // the order is COSTA's and it is load-bearing, not decorative.
    for (name, path) in [
        ("team", profile::TEAM_STRUCTURE),
        ("style", profile::WRITING_STYLE),
    ] {
        if let Ok(text) = context.corpus.read(path).await {
            let slimmed = crate::context::slim(&text);

            if !slimmed.trim().is_empty() {
                push(&mut budget, name, &format!("## {name}\n{slimmed}"));
            }
        }
    }

    push(
        &mut budget,
        "item",
        &format!("## The pull request\n{}", describe(pull_request)),
    );

    let request = ChatRequest::new(crate::agent::DEFAULT_MODEL, vec![Message::user(&prompt)])
        .with_options(Options::new().with_answer_length(DRAFT_TOKENS));

    let reply = context.engine.chat(&request).await?;
    let drafted = reply.content.trim();

    if drafted.is_empty() {
        return Err(Error::Engine(llama::Error::Transport(
            "the model returned an empty draft".to_string(),
        )));
    }

    Ok(format!("{}\n\n{drafted}\n", heading(pull_request)))
}

/// What the file says about itself, before the draft.
fn heading(pull_request: &github::PullRequest) -> String {
    format!(
        "# Ask for a review on {} #{}\n\n\
         Chief drafted this and has sent nothing. Edit it, or delete the file.\n\n\
         {}",
        pull_request.repository, pull_request.number, pull_request.url
    )
}

/// The item as both the model and the card see it, so they agree.
fn describe(pull_request: &github::PullRequest) -> String {
    let described = pull_request
        .body
        .as_deref()
        .map(str::trim)
        .filter(|body| !body.is_empty())
        .unwrap_or("(no description)");

    format!(
        "{} #{}: {}\nOpen since {}.\n\n{described}",
        pull_request.repository, pull_request.number, pull_request.title, pull_request.updated_at
    )
}

/// A file name from an identifier, safe on every platform Chief ships to.
fn slug(external_id: &str) -> String {
    let cleaned: String = external_id
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();

    cleaned
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull_request(updated_at: &str) -> github::PullRequest {
        github::PullRequest {
            number: 44,
            title: "Stop a long answer being thrown away".to_string(),
            repository: "scottmallinson/chief.ai".to_string(),
            state: "open".to_string(),
            draft: false,
            url: "https://github.com/scottmallinson/chief.ai/pull/44".to_string(),
            updated_at: updated_at.to_string(),
            merged_at: None,
            body: Some("The read timeout bounded the whole exchange.".to_string()),
        }
    }

    fn days_ago(days: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339()
    }

    #[test]
    fn leaves_a_pull_request_opened_this_morning_alone() {
        assert!(!is_worth_chasing(&pull_request(&days_ago(0))));
        assert!(!is_worth_chasing(&pull_request(&days_ago(1))));
    }

    #[test]
    fn chases_one_that_has_been_sitting() {
        assert!(is_worth_chasing(&pull_request(&days_ago(11))));
    }

    #[test]
    fn never_chases_a_draft_pull_request() {
        let mut unfinished = pull_request(&days_ago(30));
        unfinished.draft = true;

        assert!(
            !is_worth_chasing(&unfinished),
            "nobody owes a review on something still being written"
        );
    }

    #[test]
    fn treats_an_unreadable_date_as_no_evidence() {
        assert!(!is_worth_chasing(&pull_request("not a date")));
    }

    #[test]
    fn makes_a_file_name_no_platform_will_argue_with() {
        assert_eq!(
            slug("scottmallinson/chief.ai#44"),
            "scottmallinson-chief-ai-44"
        );
    }

    #[test]
    fn says_what_the_draft_is_and_that_nothing_was_sent() {
        let said = heading(&pull_request(&days_ago(11)));

        assert!(said.contains("has sent nothing"), "{said}");
        assert!(said.contains("https://github.com/"), "{said}");
    }

    #[test]
    fn describes_a_pull_request_with_no_description() {
        let mut bare = pull_request(&days_ago(11));
        bare.body = None;

        assert!(describe(&bare).contains("(no description)"));
    }
}

#[cfg(test)]
mod passes {
    use super::*;
    use crate::agent::Attention;
    use crate::corpus::Corpus;
    use crate::db::test_support::migrated_pool;
    use crate::llama::test_support::serve;
    use crate::{integrations, microsoft};

    const DRAFTED: &str = r#"{"choices":[{"message":{"role":"assistant","content":"Could you take a look at #44 when you get a moment?"}}]}"#;

    struct Scratch(std::path::PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// One open pull request, eleven days old, as GitHub's search returns it.
    fn open_prs(days: i64) -> String {
        let updated = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();

        format!(
            r#"{{"total_count":1,"items":[{{
                "number": 44, "title": "Stop a long answer being thrown away",
                "state": "open", "draft": false,
                "html_url": "https://github.com/scottmallinson/chief.ai/pull/44",
                "repository_url": "https://api.github.com/repos/scottmallinson/chief.ai",
                "updated_at": "{updated}",
                "body": "The read timeout bounded the whole exchange.",
                "pull_request": {{}}
            }}]}}"#
        )
    }

    async fn context_for(
        github_host: &str,
        engine_host: &str,
        name: &str,
    ) -> (recipe::Context, Scratch) {
        let pool = migrated_pool().await;
        integrations::save(
            &pool,
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

        let root =
            std::env::temp_dir().join(format!("chief-propose-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let corpus = Corpus::at(root.clone());
        corpus
            .ensure_shape()
            .await
            .expect("should create the shape");

        let context = recipe::Context {
            pool,
            github: github::Client::against(github_host).expect("client"),
            microsoft: microsoft::Client::against("127.0.0.1:1").expect("client"),
            calendar: crate::calendar::Client::new().expect("client"),
            linear: crate::linear::Client::new().expect("client"),
            atlassian: crate::atlassian::Client::new().expect("client"),
            engine: llama::Client::with_base_url(engine_host).expect("client"),
            corpus,
        };

        (context, Scratch(root))
    }

    #[tokio::test]
    async fn drafts_for_a_pull_request_that_has_been_sitting() {
        let (github_host, _github) = serve(vec![("HTTP/1.1 200 OK", open_prs(11))]);
        let (engine_host, _engine) = serve(vec![("HTTP/1.1 200 OK", DRAFTED.to_string())]);
        let (context, _scratch) = context_for(&github_host, &engine_host, "draft").await;

        let proposal = run_once(&context, &Attention::default())
            .await
            .expect("should run")
            .expect("should have drafted one");

        assert_eq!(proposal.dedupe_key, "scottmallinson/chief.ai#44");
        assert_eq!(proposal.status, proposed::DRAFTED);

        let body = context.corpus.read(&proposal.path).await.expect("read");
        assert!(body.contains("Could you take a look at #44"), "{body}");
        assert!(body.contains("has sent nothing"), "{body}");
    }

    /// The boundary to step 15, asserted rather than trusted.
    #[tokio::test]
    async fn never_reaches_the_network_to_draft() {
        let (github_host, github) = serve(vec![("HTTP/1.1 200 OK", open_prs(11))]);
        let (engine_host, _engine) = serve(vec![("HTTP/1.1 200 OK", DRAFTED.to_string())]);
        let (context, _scratch) = context_for(&github_host, &engine_host, "network").await;

        run_once(&context, &Attention::default())
            .await
            .expect("should run");

        let requests = github.await.expect("github");

        // Checked before the loop, because a loop over an empty list passes
        // without asserting anything — and "GitHub was never called at all"
        // would be a broken stub rather than proof of a boundary.
        assert!(
            !requests.is_empty(),
            "the pass should have read GitHub; an empty list proves nothing"
        );

        // Everything this step says to GitHub is a read. The moment anything
        // here writes, this fails — which is the entire point of the step
        // boundary being a test rather than an intention.
        for request in requests {
            let (line, _) = crate::llama::test_support::split(&request);

            assert!(
                line.starts_with("GET "),
                "drafting must not write anything anywhere: {line}"
            );
        }
    }

    #[tokio::test]
    async fn drafts_one_item_per_pass_and_no_more() {
        let two = format!(
            r#"{{"total_count":2,"items":[{},{}]}}"#,
            item(44, 11),
            item(45, 12)
        );
        let (github_host, _github) = serve(vec![("HTTP/1.1 200 OK", two)]);
        // One reply only: a second call would hang rather than be answered,
        // so this asserts the count by construction as well as by the result.
        let (engine_host, engine) = serve(vec![("HTTP/1.1 200 OK", DRAFTED.to_string())]);
        let (context, _scratch) = context_for(&github_host, &engine_host, "onlyone").await;

        run_once(&context, &Attention::default())
            .await
            .expect("should run");

        assert_eq!(
            proposed::fetch(&context.pool, None)
                .await
                .expect("read")
                .len(),
            1,
            "a pass drafts one thing, however many are waiting"
        );
        assert_eq!(engine.await.expect("engine").len(), 1);
    }

    fn item(number: i64, days: i64) -> String {
        let updated = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();

        format!(
            r#"{{
                "number": {number}, "title": "Something", "state": "open", "draft": false,
                "html_url": "https://github.com/scottmallinson/chief.ai/pull/{number}",
                "repository_url": "https://api.github.com/repos/scottmallinson/chief.ai",
                "updated_at": "{updated}", "body": "A change.",
                "pull_request": {{}}
            }}"#
        )
    }

    #[tokio::test]
    async fn steps_aside_while_the_user_is_waiting_on_an_answer() {
        let (github_host, github) = serve(Vec::<(&str, &str)>::new());
        let (engine_host, engine) = serve(Vec::<(&str, &str)>::new());
        let (context, _scratch) = context_for(&github_host, &engine_host, "attention").await;

        let attention = Attention::default();
        let _waiting = attention.begin();

        let drafted = run_once(&context, &attention).await.expect("should run");

        assert!(
            drafted.is_none(),
            "their question is worth more than a draft"
        );
        assert_eq!(github.await.expect("github"), Vec::<String>::new());
        assert_eq!(engine.await.expect("engine"), Vec::<String>::new());
    }

    #[tokio::test]
    async fn never_drafts_for_the_same_item_twice() {
        let (github_host, _github) = serve(vec![
            ("HTTP/1.1 200 OK", open_prs(11)),
            ("HTTP/1.1 200 OK", open_prs(11)),
        ]);
        let (engine_host, engine) = serve(vec![("HTTP/1.1 200 OK", DRAFTED.to_string())]);
        let (context, _scratch) = context_for(&github_host, &engine_host, "twice").await;

        run_once(&context, &Attention::default())
            .await
            .expect("first");
        let second = run_once(&context, &Attention::default())
            .await
            .expect("second");

        assert!(second.is_none(), "the item already has a draft");
        assert_eq!(
            engine.await.expect("engine").len(),
            1,
            "and the check happens before the model is asked, so the second pass is free"
        );
    }

    #[tokio::test]
    async fn writes_nothing_at_all_when_the_model_refuses() {
        let (github_host, _github) = serve(vec![("HTTP/1.1 200 OK", open_prs(11))]);
        let (engine_host, _engine) = serve(vec![(
            "HTTP/1.1 500 Internal Server Error",
            "{\"error\":\"no\"}",
        )]);
        let (context, _scratch) = context_for(&github_host, &engine_host, "enginefail").await;

        run_once(&context, &Attention::default())
            .await
            .expect_err("the engine refused");

        assert_eq!(
            proposed::fetch(&context.pool, None)
                .await
                .expect("read")
                .len(),
            0,
            "no orphan row"
        );

        let listed = context.corpus.list().await.expect("list");
        assert!(
            !listed
                .iter()
                .any(|entry| entry.path.starts_with("proposed/2026")
                    || entry.path.contains("chief-ai-44")),
            "and no half-written draft: {listed:?}"
        );
    }

    const REWRITTEN: &str =
        r#"{"choices":[{"message":{"role":"assistant","content":"Mind taking a look at #44?"}}]}"#;

    #[tokio::test]
    async fn rewrites_a_draft_to_an_instruction() {
        let (engine_host, engine) = serve(vec![("HTTP/1.1 200 OK", REWRITTEN)]);
        let client = llama::Client::with_base_url(&engine_host).expect("client");

        let rewritten = refine(
            &client,
            "I would be most grateful if you could review #44.",
            "make it less formal",
        )
        .await
        .expect("should rewrite");

        assert_eq!(rewritten, "Mind taking a look at #44?");

        let sent = engine.await.expect("engine").join("");
        assert!(sent.contains("make it less formal"), "the instruction goes");
        assert!(sent.contains("most grateful"), "and so does the draft");
    }

    /// The cheapest call Chief makes, and it has to stay that way.
    #[tokio::test]
    async fn sends_nothing_but_the_instruction_and_the_draft() {
        let (engine_host, engine) = serve(vec![("HTTP/1.1 200 OK", REWRITTEN)]);
        let client = llama::Client::with_base_url(&engine_host).expect("client");

        refine(&client, "Please review #44.", "make it warmer")
            .await
            .expect("should rewrite");

        let sent = engine.await.expect("engine").join("");

        // No corpus, no history, no tools. Each of these would put hundreds of
        // tokens of prefill in front of a rewrite the user makes repeatedly.
        assert!(!sent.contains("writing_style"), "no corpus: {sent}");
        assert!(!sent.contains("team_structure"), "no corpus: {sent}");
        assert!(!sent.contains("\"tools\""), "no tool catalogue: {sent}");
        assert!(
            sent.matches("\"role\"").count() == 1,
            "one message, so no history: {sent}"
        );
    }

    #[tokio::test]
    async fn refuses_to_replace_a_draft_with_nothing() {
        let empty = r#"{"choices":[{"message":{"role":"assistant","content":"   "}}]}"#;
        let (engine_host, _engine) = serve(vec![("HTTP/1.1 200 OK", empty)]);
        let client = llama::Client::with_base_url(&engine_host).expect("client");

        let error = refine(&client, "Please review #44.", "make it shorter")
            .await
            .expect_err("an empty rewrite is not a rewrite");

        assert!(error.contains("nothing to put in its place"), "{error}");
    }

    #[tokio::test]
    async fn leaves_a_fresh_pull_request_alone() {
        let (github_host, _github) = serve(vec![("HTTP/1.1 200 OK", open_prs(0))]);
        let (engine_host, engine) = serve(Vec::<(&str, &str)>::new());
        let (context, _scratch) = context_for(&github_host, &engine_host, "fresh").await;

        let drafted = run_once(&context, &Attention::default())
            .await
            .expect("run");

        assert!(drafted.is_none());
        assert_eq!(engine.await.expect("engine"), Vec::<String>::new());
    }

    #[tokio::test]
    async fn reads_the_team_and_the_style_before_the_item() {
        let (github_host, _github) = serve(vec![("HTTP/1.1 200 OK", open_prs(11))]);
        let (engine_host, engine) = serve(vec![("HTTP/1.1 200 OK", DRAFTED.to_string())]);
        let (context, _scratch) = context_for(&github_host, &engine_host, "order").await;

        context
            .corpus
            .write(
                profile::TEAM_STRUCTURE,
                "# Team\n\n## Ana Silva\n\nReviews the engine work.",
            )
            .await
            .expect("write");
        context
            .corpus
            .write(
                profile::WRITING_STYLE,
                "# Writing style\n\n- Short sentences.",
            )
            .await
            .expect("write");

        run_once(&context, &Attention::default())
            .await
            .expect("run");

        let sent = engine.await.expect("engine").join("");
        let team = sent
            .find("Ana Silva")
            .expect("the team should be in the prompt");
        let style = sent
            .find("Short sentences")
            .expect("the style should be too");
        let item = sent.find("Stop a long answer").expect("and the item");

        // COSTA's ordering, literally. Who these people are frames how to
        // address them; how the user writes frames the words; the thing being
        // written about arrives last.
        assert!(team < style, "org structure comes before writing style");
        assert!(style < item, "writing style comes before the item");
    }
}
