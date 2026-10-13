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
//! The prompt is assembled in COSTA's order, literally: **the people tied to the
//! item, then writing style, then examples, then the item**. Who these people are frames how to address
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
    #[error("the draft named {}, who are not in anything Chief was given, so it was not kept", .0.join(", "))]
    InventedName(Vec<String>),
}

const INSTRUCTIONS: &str = "\
Write a short message asking a colleague to review the pull request described \
below. It will be read and edited by the person you are writing for, not sent \
for them.

Rules:
- Three sentences at most, and no subject line.
- Say what the change does, in their words, from the description below.
- Ask for a review. Do not apologise, and do not chase twice in one message.
- Name only people listed under \"people on this repository\". If none are \
listed, greet without a name. Never invent a name, a time or a promise.
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

/// What a pass wrote, and whether a model was involved.
enum Written {
    /// No profile to write from, so nothing was asked of the model.
    Template(String),
    Model(String),
}

/// The one model call, assembled in COSTA's order, then checked.
async fn compose(
    context: &recipe::Context,
    pull_request: &github::PullRequest,
) -> Result<String, Error> {
    let heading = heading(pull_request);

    match write(context, pull_request).await? {
        Written::Template(text) => Ok(format!("{heading}\n\n{NO_PROFILE_NOTE}\n\n{text}\n")),
        Written::Model(text) => Ok(format!("{heading}\n\n{text}\n")),
    }
}

const NO_PROFILE_NOTE: &str = "\
Chief has no profile of how you write or who you work with yet, so this is a \
plain template and no model wrote it. Seed one in Settings and the next draft \
will sound like you.";

async fn write(
    context: &recipe::Context,
    pull_request: &github::PullRequest,
) -> Result<Written, Error> {
    if has_no_profile(context).await {
        return Ok(Written::Template(neutral_template(pull_request)));
    }

    let drafted = ask_model(context, pull_request).await?;
    let invented = unsupported_names(&drafted.text, &drafted.prompt);

    if invented.is_empty() {
        Ok(Written::Model(drafted.text))
    } else {
        Err(Error::InventedName(invented))
    }
}

/// The benchmark's entry: what the model wrote, and whether Chief would have
/// refused to keep it. No heading and no file.
#[cfg(test)]
pub(crate) async fn draft_for_bench(
    context: &recipe::Context,
    pull_request: &github::PullRequest,
) -> Result<(String, bool), Error> {
    if has_no_profile(context).await {
        return Ok((neutral_template(pull_request), false));
    }

    let drafted = ask_model(context, pull_request).await?;
    let blocked = !unsupported_names(&drafted.text, &drafted.prompt).is_empty();

    Ok((drafted.text, blocked))
}

/// Neither profile file has been touched, so there is nothing of the user's to
/// write from.
async fn has_no_profile(context: &recipe::Context) -> bool {
    profile::is_untouched(&context.corpus, profile::WRITING_STYLE).await
        && profile::is_untouched(&context.corpus, profile::TEAM_STRUCTURE).await
}

/// Three lines, no names, no model. A greeting with no name in it cannot be
/// wrong about who the reader is.
fn neutral_template(pull_request: &github::PullRequest) -> String {
    format!(
        "Hi,\n\nCould you take a look at {} #{}, \"{}\"? {}\n\nThanks",
        pull_request.repository, pull_request.number, pull_request.title, pull_request.url
    )
}

struct Drafted {
    prompt: String,
    text: String,
}

/// How long before a counted profile is flagged as possibly out of date.
const STALE_PROFILE_DAYS: i64 = 90;

/// How many of the user's own past requests go in as examples of their voice.
const EXAMPLES: usize = 3;

/// The most characters of one example. A message asking for a review is a few
/// sentences, so an example that runs longer is not one.
const EXAMPLE_CHARS: usize = 400;

/// The most people carried in, however many the file lists for a repository.
const PEOPLE: usize = 4;

/// Sections of the team file, each starting at its `## ` line.
fn sections(team: &str) -> Vec<&str> {
    let mut starts: Vec<usize> = team
        .match_indices("## ")
        .filter(|(at, _)| *at == 0 || team.as_bytes()[*at - 1] == b'\n')
        .map(|(at, _)| at)
        .collect();
    starts.push(team.len());

    starts
        .windows(2)
        .map(|pair| team[pair[0]..pair[1]].trim_end())
        .collect()
}

/// The people the file ties to this repository: those whose section names it.
///
/// Matched on the whole `owner/name`, case-insensitively and only at a word
/// edge, so `acme/api` does not pull in `acme/api-gateway`.
fn people_for(team: &str, repository: &str) -> Vec<String> {
    let wanted = repository.to_lowercase();

    sections(team)
        .into_iter()
        .filter(|section| {
            let lower = section.to_lowercase();
            lower.match_indices(&wanted).any(|(at, _)| {
                let mut after = lower[at + wanted.len()..].chars();
                match after.next() {
                    Some('-' | '_') => false,
                    Some('.') => !after.next().is_some_and(char::is_alphanumeric),
                    Some(c) => !c.is_alphanumeric(),
                    None => true,
                }
            })
        })
        .take(PEOPLE)
        .map(str::to_string)
        .collect()
}

/// The `Source:` line of a counted profile, with a warning when it is old.
fn provenance(text: &str, today: chrono::NaiveDate) -> Option<String> {
    let source = text.lines().find(|line| line.starts_with("Source:"))?;

    match profile::counted_on(text) {
        Some(counted) if (today - counted).num_days() > STALE_PROFILE_DAYS => Some(format!(
            "{source} That was {} days ago and may be out of date.",
            (today - counted).num_days()
        )),
        _ => Some(source.to_string()),
    }
}

/// The body of a draft Chief kept, without the heading it wrote above it.
fn body_of(kept: &str) -> String {
    let text = match kept.find("\nhttps://") {
        Some(at) => kept[at + 1..].split_once('\n').map_or("", |(_, rest)| rest),
        None => kept,
    };
    let text = text.trim();

    text.chars().take(EXAMPLE_CHARS).collect()
}

/// A few of the user's own kept drafts, the ones for this repository first.
///
/// A dismissed draft is the user saying no, so it is never held up as an
/// example of how they would put it.
async fn examples_for(context: &recipe::Context, repository: &str) -> Vec<String> {
    let Ok(all) = proposed::fetch(&context.pool, Some(50)).await else {
        return Vec::new();
    };
    let own = format!("{repository}#");

    let (same, other): (Vec<_>, Vec<_>) = all
        .into_iter()
        .filter(|one| one.source == "github" && one.status != proposed::DISMISSED)
        .partition(|one| one.dedupe_key.starts_with(&own));

    let mut found = Vec::new();

    for one in same.into_iter().chain(other) {
        if found.len() == EXAMPLES {
            break;
        }
        if let Ok(text) = context.corpus.read(&one.path).await {
            let body = body_of(&text);
            if !body.is_empty() {
                found.push(body);
            }
        }
    }

    found
}

/// Names in a draft that nothing Chief was given contains.
///
/// A person is a capitalised word in the middle of a sentence, the word after
/// a greeting, a word opening a sentence that is followed by a comma, or an
/// `@handle`. Each must appear somewhere in the prompt. This is a heuristic
/// and errs towards refusing: a draft thrown away costs a model call, a
/// message to somebody who does not exist costs the user's credibility.
fn unsupported_names(draft: &str, prompt: &str) -> Vec<String> {
    const GREETINGS: [&str; 5] = ["hi", "hey", "hello", "dear", "morning"];
    const NOT_NAMES: [&str; 12] = [
        "thanks", "thank", "cheers", "please", "also", "could", "would", "can", "this", "the",
        "it", "we",
    ];

    let known = prompt.to_lowercase();
    let mut invented: Vec<String> = Vec::new();
    let mut opens_sentence;
    let mut after_greeting;

    for line in draft.lines() {
        opens_sentence = true;
        after_greeting = false;

        for token in line.split_whitespace() {
            let word = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '@' && c != '\'');
            let lower = word.to_lowercase();
            let comma_after = token.ends_with(',');

            let mut candidate = false;

            if word.starts_with('@') && word.len() > 1 {
                candidate = true;
            } else if word.chars().next().is_some_and(char::is_uppercase)
                && word.chars().any(char::is_lowercase)
                && word != "I"
                && !word.starts_with("I'")
            {
                let is_greeting = GREETINGS.contains(&lower.as_str());
                candidate = after_greeting
                    || (!opens_sentence && !is_greeting)
                    || (opens_sentence
                        && comma_after
                        && !is_greeting
                        && !NOT_NAMES.contains(&lower.as_str()));
            }

            if candidate && !known.contains(&lower) && !invented.contains(&word.to_string()) {
                invented.push(word.to_string());
            }

            after_greeting = GREETINGS.contains(&lower.as_str());
            opens_sentence = token.ends_with(['.', '!', '?', ':']);
        }
    }

    invented
}

async fn ask_model(
    context: &recipe::Context,
    pull_request: &github::PullRequest,
) -> Result<Drafted, Error> {
    let mut budget = Budget::with_ceiling(PROMPT_TOKENS);
    let mut prompt = String::new();
    let today = chrono::Local::now().date_naive();

    let mut push = |budget: &mut Budget, name: &str, block: &str| {
        if budget.add(name, block).is_ok() {
            prompt.push_str(block);
            prompt.push_str("\n\n");
        }
    };

    push(&mut budget, "instructions", INSTRUCTIONS);

    // People, then writing style, then examples, then the item. See the module
    // note: the order is COSTA's and it is load-bearing, not decorative.
    if let Ok(team) = context.corpus.read(profile::TEAM_STRUCTURE).await {
        let tied = people_for(&team, &pull_request.repository);

        if !tied.is_empty() {
            let mut block = String::from("## people on this repository\n");
            if let Some(source) = provenance(&team, today) {
                block.push_str(&source);
                block.push('\n');
            }
            block.push_str(&tied.join("\n\n"));
            push(&mut budget, "team", &crate::context::slim(&block));
        }
    }

    if let Ok(text) = context.corpus.read(profile::WRITING_STYLE).await {
        let slimmed = crate::context::slim(&text);

        if !slimmed.trim().is_empty() {
            let mut block = String::from("## style\n");
            if let Some(source) = provenance(&text, today) {
                block.push_str(&source);
                block.push('\n');
            }
            block.push_str(&slimmed);
            push(&mut budget, "style", &block);
        }
    }

    let examples = examples_for(context, &pull_request.repository).await;

    if !examples.is_empty() {
        push(
            &mut budget,
            "examples",
            &format!(
                "## messages you wrote before\n{}",
                examples
                    .iter()
                    .map(|one| format!("- {}", one.replace('\n', " ")))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        );
    }

    push(
        &mut budget,
        "item",
        &format!("## The pull request\n{}", describe(pull_request)),
    );

    let request = ChatRequest::new(crate::agent::DEFAULT_MODEL, vec![Message::user(&prompt)])
        .with_options(Options::new().with_answer_length(DRAFT_TOKENS));

    let reply = context.engine.chat(&request).await?;
    let text = reply.content.trim().to_string();

    if text.is_empty() {
        return Err(Error::Engine(llama::Error::Transport(
            "the model returned an empty draft".to_string(),
        )));
    }

    Ok(Drafted { prompt, text })
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
        corpus
            .write(
                profile::WRITING_STYLE,
                "# Writing style\n\n- Greeting: opens with \"Hi\" (3 of 4).",
            )
            .await
            .expect("should write a style");

        let context = recipe::Context {
            pool,
            github: github::Client::against(github_host).expect("client"),
            microsoft: microsoft::Client::against("127.0.0.1:1").expect("client"),
            calendar: crate::calendar::Client::new().expect("client"),
            linear: crate::linear::Client::new().expect("client"),
            atlassian: crate::atlassian::Client::new().expect("client"),
            atlassian_rest: crate::atlassian::rest::Rest::new().expect("client"),
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
                "# Team\n\n## Ana Silva\n\nReviews scottmallinson/chief.ai engine work.",
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

    const TEAM: &str =
        "# Team structure\n\nSource: meetings over the last 28 days, counted 2026-10-01.\n\n\
## Priya Nair\n\nReviews acme/api.\n\n## Tomasz Wrona\n\nReviews acme/web.\n\n\
## Dev Patel\n\nReviews acme/api-gateway.";

    #[test]
    fn carries_only_the_people_tied_to_the_repository() {
        let tied = people_for(TEAM, "acme/api");

        assert_eq!(tied.len(), 1, "{tied:?}");
        assert!(tied[0].contains("Priya Nair"), "{tied:?}");
        assert!(people_for(TEAM, "acme/billing").is_empty());
    }

    #[test]
    fn flags_a_profile_counted_long_ago_and_not_a_recent_one() {
        let day = |d| chrono::NaiveDate::from_ymd_opt(2026, 10, d).expect("date");
        let later = chrono::NaiveDate::from_ymd_opt(2027, 3, 1).expect("date");

        assert!(
            !provenance(TEAM, day(13))
                .expect("source")
                .contains("out of date"),
            "a fortnight is not stale"
        );
        assert!(
            provenance(TEAM, later)
                .expect("source")
                .contains("may be out of date"),
            "five months is"
        );
        assert_eq!(provenance("# hand written", day(13)), None);
    }

    #[test]
    fn finds_a_name_the_prompt_never_mentioned() {
        let prompt = "## people on this repository\n## Priya Nair\nReviews acme/api.";

        assert_eq!(
            unsupported_names("Hi Priya, could you ask Tomasz to look at #57?", prompt),
            vec!["Tomasz".to_string()]
        );
        assert_eq!(
            unsupported_names("Hi Marcus,\n\nCould you review #57?", prompt),
            vec!["Marcus".to_string()]
        );
        assert_eq!(
            unsupported_names("Could you review #57, @dev-patel?", prompt),
            vec!["@dev-patel".to_string()]
        );
        assert!(
            unsupported_names("Hi Priya,\n\nCould you review #57?\n\nThanks", prompt).is_empty()
        );
        assert!(
            unsupported_names("Hi,\n\nCould I get a review on the API change?", prompt).is_empty()
        );
    }

    #[tokio::test]
    async fn keeps_nothing_when_the_draft_names_somebody_it_was_not_given() {
        const INVENTED: &str = r#"{"choices":[{"message":{"role":"assistant","content":"Hi Marcus, could you take a look at #44?"}}]}"#;
        let (github_host, _github) = serve(vec![("HTTP/1.1 200 OK", open_prs(11))]);
        let (engine_host, _engine) = serve(vec![("HTTP/1.1 200 OK", INVENTED.to_string())]);
        let (context, _scratch) = context_for(&github_host, &engine_host, "invented").await;

        let error = run_once(&context, &Attention::default())
            .await
            .expect_err("a draft naming Marcus is not kept");

        assert!(
            matches!(error, Error::InventedName(ref names) if names == &["Marcus"]),
            "{error}"
        );
        assert_eq!(
            proposed::fetch(&context.pool, None)
                .await
                .expect("read")
                .len(),
            0,
            "no row"
        );
        let listed = context.corpus.list().await.expect("list");
        assert!(
            !listed
                .iter()
                .any(|entry| entry.path.contains("chief-ai-44")),
            "no file: {listed:?}"
        );
    }

    #[tokio::test]
    async fn sends_only_the_people_for_this_repository() {
        let (github_host, _github) = serve(vec![("HTTP/1.1 200 OK", open_prs(11))]);
        let (engine_host, engine) = serve(vec![("HTTP/1.1 200 OK", DRAFTED.to_string())]);
        let (context, _scratch) = context_for(&github_host, &engine_host, "tied").await;
        context
            .corpus
            .write(
                profile::TEAM_STRUCTURE,
                "# Team\n\n## Ana Silva\n\nReviews scottmallinson/chief.ai.\n\n## Bo Chen\n\nReviews other/thing.",
            )
            .await
            .expect("write");

        run_once(&context, &Attention::default())
            .await
            .expect("run");

        let sent = engine.await.expect("engine").join("");
        assert!(sent.contains("Ana Silva"), "{sent}");
        assert!(
            !sent.contains("Bo Chen"),
            "an unrelated person is not sent: {sent}"
        );
    }

    #[tokio::test]
    async fn shows_the_kept_drafts_as_examples_and_never_a_dismissed_one() {
        let (github_host, _github) = serve(vec![("HTTP/1.1 200 OK", open_prs(11))]);
        let (engine_host, engine) = serve(vec![("HTTP/1.1 200 OK", DRAFTED.to_string())]);
        let (context, _scratch) = context_for(&github_host, &engine_host, "examples").await;

        for (key, path, text, dismiss) in [
            ("scottmallinson/chief.ai#1", "proposed/a.md", "# Ask\n\nChief drafted this.\n\nhttps://github.com/x/1\n\nQuick one for you, when free.", false),
            ("scottmallinson/chief.ai#2", "proposed/b.md", "# Ask\n\nChief drafted this.\n\nhttps://github.com/x/2\n\nUNWANTED WORDING", true),
        ] {
            context.corpus.write(path, text).await.expect("write");
            let one = proposed::insert_new(&context.pool, NewProposal {
                source: "github".into(), account_id: 1, dedupe_key: key.into(),
                title: "t".into(), context: "c".into(), path: path.into(),
            }).await.expect("insert").expect("new");
            if dismiss {
                proposed::dismiss(&context.pool, one.id).await.expect("dismiss");
            }
        }

        run_once(&context, &Attention::default())
            .await
            .expect("run");

        let sent = engine.await.expect("engine").join("");
        assert!(sent.contains("Quick one for you, when free."), "{sent}");
        assert!(!sent.contains("UNWANTED WORDING"), "{sent}");
        assert!(
            !sent.contains("Chief drafted this"),
            "the heading is not the voice: {sent}"
        );
    }

    #[tokio::test]
    async fn writes_a_neutral_template_without_a_model_when_there_is_no_profile() {
        let (github_host, _github) = serve(vec![("HTTP/1.1 200 OK", open_prs(11))]);
        let (engine_host, engine) = serve(Vec::<(&str, &str)>::new());
        let (context, _scratch) = context_for(&github_host, &engine_host, "bare").await;
        for path in [profile::WRITING_STYLE, profile::TEAM_STRUCTURE] {
            let starter = corpus::starter(path).expect("starter");
            context.corpus.write(path, starter).await.expect("reset");
        }

        let proposal = run_once(&context, &Attention::default())
            .await
            .expect("run")
            .expect("a template is still a draft");

        let body = context.corpus.read(&proposal.path).await.expect("read");
        assert!(body.contains("no profile"), "{body}");
        assert!(body.contains("Hi,\n\nCould you take a look at"), "{body}");
        assert_eq!(
            engine.await.expect("engine"),
            Vec::<String>::new(),
            "no model was asked"
        );
    }
}
