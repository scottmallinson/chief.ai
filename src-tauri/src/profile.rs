//! Counting facts about the user from their own work.
//!
//! A draft written with nothing known about the user sounds like a model, and a
//! draft written from stale facts sounds like somebody else. The facts are
//! counted from what the user wrote and who they worked with, and kept as rows
//! (see `facts.rs`) so each one carries where it came from and when.
//!
//! Three rules hold this together, and all three are about not taking something
//! that is the user's:
//!
//! 1. **Nothing runs without being asked.** [`plan`] says what would be read and
//!    what would be written, in those words, and returns without touching
//!    anything. The interface shows it and waits. This is the first feature that
//!    reads the user's own writing in bulk to build a profile of it, and a
//!    profile built quietly is the wrong way to do that however local it stays.
//! 2. **A fact the user has touched is theirs.** A recount replaces only the
//!    rows Chief counted; one the user edited or removed is left as they left
//!    it. See `facts::recount`.
//! 3. **Counted, not inferred.** The writing style, the reviewers and the team
//!    are arithmetic over what the user wrote and who they met. A 3B model asked
//!    to describe a voice made up habits the samples did not show, so no model
//!    is called here at all.
//!
//! The two markdown files in the corpus are not read for drafting any more.
//! Briefs still read them as background, so they are left where they are.

use serde::Serialize;

use crate::corpus;
use crate::db;
use crate::facts::{self, NewFact};
use crate::github;
use crate::llama;
use crate::recipe;
use crate::session::{GithubSession, OutlookSession};

/// The two corpus files drafting used to read. Still the user's, still read by
/// briefs; named here so the settings card can say drafts no longer use them.
pub const WRITING_STYLE: &str = "context/agents/profile/writing_style.md";
pub const TEAM_STRUCTURE: &str = "context/agents/org/team_structure.md";

/// How many of the user's own pull requests are sampled for a writing style.
///
/// A sample, not a corpus. Enough for a voice to show through.
const SAMPLE: u8 = 20;

/// How many of the user's pull requests have their reviews read.
///
/// One GitHub request each, so this is the cost of the step: thirty requests,
/// once, when the user asks.
const REVIEWED: u8 = 30;

/// The most reviewers kept per repository. A draft takes fewer than this.
const REVIEWERS: usize = 8;

/// How many days of calendar are counted for the team.
const TEAM_DAYS: i64 = 28;

/// How many people the meeting count names. Past this it is an address book.
const TEAM_SIZE: usize = 12;

/// How many meetings somebody has to appear in before they are counted.
///
/// Two, not one. A single meeting is an interview, a vendor call, or somebody
/// who was cc'd — a recurring one is a working relationship.
const RECURRENCE: usize = 2;

/// What can go wrong seeding the profile.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Engine(#[from] llama::Error),
    #[error(transparent)]
    Corpus(#[from] corpus::Error),
    #[error(transparent)]
    Storage(#[from] db::Error),
    #[error("there is nothing to learn from yet")]
    NothingToLearnFrom,
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// What seeding the profile would read and write, said before it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// What Chief would read, in the user's words rather than in API names.
    pub reads: Vec<String>,
    /// Which facts it would count.
    pub writes: Vec<String>,
    /// What it would leave alone, and why, so "nothing happened to that one"
    /// always has a visible reason.
    pub keeps: Vec<String>,
}

/// What was actually written.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub written: Vec<String>,
    pub kept: Vec<String>,
}

/// Is this file still exactly what `ensure_shape` put there?
///
/// Content, not modification time: a git checkout, a Dropbox sync or a restored
/// backup all move mtime without changing a byte. A file that is absent counts
/// as untouched.
pub async fn is_untouched(corpus: &corpus::Corpus, path: &str) -> bool {
    let Some(starter) = corpus::starter(path) else {
        // Not a file Chief created.
        return false;
    };

    match corpus.read(path).await {
        Ok(contents) => contents == starter,
        Err(corpus::Error::NoSuchFile(_)) => true,
        Err(_) => false,
    }
}

/// The user's facts that a recount will not touch, as one sentence, if any.
async fn owned_facts(context: &recipe::Context) -> Option<String> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM facts WHERE origin != 'counted'")
        .fetch_one(&context.pool)
        .await
        .unwrap_or(0);

    match count {
        0 => None,
        1 => Some("1 fact you changed or removed stays as you left it".to_string()),
        n => Some(format!(
            "{n} facts you changed or removed stay as you left them"
        )),
    }
}

/// What seeding would do, without doing any of it.
pub async fn plan(context: &recipe::Context) -> Plan {
    let mut keeps = Vec::new();

    if let Some(owned) = owned_facts(context).await {
        keeps.push(owned);
    }

    for path in [WRITING_STYLE, TEAM_STRUCTURE] {
        if !is_untouched(&context.corpus, path).await {
            keeps.push(format!(
                "{path}: your briefs still read it, but drafts no longer do"
            ));
        }
    }

    Plan {
        reads: vec![
            format!("The descriptions you wrote on your last {SAMPLE} pull requests"),
            format!(
                "Who reviewed your last {REVIEWED} pull requests (one request to GitHub for each)"
            ),
            format!("Who you met with over the last {TEAM_DAYS} days, and how often"),
        ],
        writes: vec![
            "How you write: greeting, sign-off, length, emoji, spelling".to_string(),
            "Who reviews your pull requests, repository by repository".to_string(),
            "Who you meet with most".to_string(),
        ],
        keeps,
    }
}

/// Count the facts from the user's own work.
///
/// Every kind is replaced as a whole, so running this twice is not destructive:
/// the second run counts the same things again and leaves what the user wrote.
pub async fn bootstrap(context: &recipe::Context) -> Result<Report, Error> {
    let today = chrono::Local::now().date_naive();
    let mut report = Report::default();
    let mut kept = 0;

    let written = sample(context).await;
    if !written.is_empty() {
        let rows = style_facts(&written, today);
        let done = facts::recount(
            &context.pool,
            &[
                facts::GREETING,
                facts::SIGN_OFF,
                facts::LENGTH,
                facts::EMOJI,
                facts::SPELLING,
            ],
            rows,
        )
        .await?;
        kept += done.kept;
        if done.written > 0 {
            report
                .written
                .push(format!("{} facts about how you write", done.written));
        }
    }

    if let Some(rows) = reviewer_facts(context, today).await {
        let done = facts::recount(&context.pool, &[facts::REVIEWS], rows).await?;
        kept += done.kept;
        if done.written > 0 {
            report.written.push(format!(
                "{} facts about who reviews your pull requests",
                done.written
            ));
        }
    }

    let team = team_facts(context, today).await;
    if !team.is_empty() {
        let done = facts::recount(&context.pool, &[facts::MEETS], team).await?;
        kept += done.kept;
        if done.written > 0 {
            report
                .written
                .push(format!("{} facts about who you meet with", done.written));
        }
    }

    if kept > 0 {
        report.kept.push(format!(
            "{kept} facts you had changed or removed, left as they were"
        ));
    }

    if report.written.is_empty() && report.kept.is_empty() {
        return Err(Error::NothingToLearnFrom);
    }

    Ok(report)
}

/// The user's own prose, as a sample to count.
async fn sample(context: &recipe::Context) -> Vec<String> {
    let mut written = Vec::new();

    for account in recipe::github_accounts(context).await {
        let session = GithubSession::new(&context.pool, &context.github, account);

        if let Ok(pull_requests) = session
            .pull_requests(github::Involvement::Authored, github::State::Closed, SAMPLE)
            .await
        {
            written.extend(
                pull_requests
                    .iter()
                    .filter_map(|pull_request| pull_request.body.clone())
                    .map(|body| crate::context::slim(&body))
                    .filter(|body| !body.trim().is_empty()),
            );
        }
    }

    written
}

/// Who reviewed the user's recent pull requests, counted per repository.
///
/// `None` when no GitHub account answered at all, so a machine with nothing
/// connected does not wipe the reviewers it counted before. A pull request
/// whose reviews cannot be read (a repository since deleted, a rate limit) is
/// skipped; the people it would have shown are simply not counted this time.
async fn reviewer_facts(
    context: &recipe::Context,
    today: chrono::NaiveDate,
) -> Option<Vec<NewFact>> {
    let mut answered = false;
    let mut rows: Vec<NewFact> = Vec::new();

    for account in recipe::github_accounts(context).await {
        let session = GithubSession::new(&context.pool, &context.github, account);

        let Ok(own) = session.login().await else {
            continue;
        };
        let Ok(pull_requests) = session
            .pull_requests(github::Involvement::Authored, github::State::All, REVIEWED)
            .await
        else {
            continue;
        };
        answered = true;

        // (repository, reviewer) -> pull requests reviewed; repository -> sampled.
        let mut reviewed: Vec<(String, String, i64)> = Vec::new();
        let mut sampled: Vec<(String, i64)> = Vec::new();

        for pull_request in pull_requests.iter().filter(|one| !one.draft) {
            let repository = pull_request.repository.to_lowercase();

            let Ok(reviewers) = session
                .reviewers(&pull_request.repository, pull_request.number)
                .await
            else {
                continue;
            };

            match sampled.iter_mut().find(|(seen, _)| *seen == repository) {
                Some((_, total)) => *total += 1,
                None => sampled.push((repository.clone(), 1)),
            }

            for reviewer in reviewers.into_iter().filter(|login| *login != own) {
                match reviewed
                    .iter_mut()
                    .find(|(repo, who, _)| *repo == repository && *who == reviewer)
                {
                    Some((_, _, count)) => *count += 1,
                    None => reviewed.push((repository.clone(), reviewer, 1)),
                }
            }
        }

        for (repository, total) in &sampled {
            let mut here: Vec<&(String, String, i64)> = reviewed
                .iter()
                .filter(|(repo, _, _)| repo == repository)
                .collect();
            here.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
            here.truncate(REVIEWERS);

            rows.extend(here.into_iter().map(|(repo, who, count)| NewFact {
                about: who.clone(),
                what: facts::REVIEWS.to_string(),
                scope: repo.clone(),
                value: String::new(),
                count: *count,
                total: *total,
                source: "GitHub reviews on your pull requests".to_string(),
                seen_on: today.to_string(),
            }));
        }
    }

    answered.then_some(rows)
}

/// The people the user meets most, counted rather than inferred.
async fn team_facts(context: &recipe::Context, today: chrono::NaiveDate) -> Vec<NewFact> {
    let now = chrono::Local::now();
    let from = (now - chrono::Duration::days(TEAM_DAYS))
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string();
    let to = now.format("%Y-%m-%dT%H:%M:%S").to_string();

    let mut meetings: Vec<Vec<String>> = Vec::new();

    for account in recipe::outlook_accounts(context).await {
        let session = OutlookSession::new(&context.pool, &context.microsoft, account);

        if let Ok(events) = session.events(&from, &to, recipe::PER_SOURCE).await {
            meetings.extend(events.iter().map(|event| event.attendees.clone()));
        }
    }

    rank(&meetings)
        .into_iter()
        .map(|(name, count)| NewFact {
            about: name.clone(),
            what: facts::MEETS.to_string(),
            scope: String::new(),
            value: name,
            count: i64::try_from(count).unwrap_or(i64::MAX),
            total: 0,
            source: format!("your calendar over the last {TEAM_DAYS} days"),
            seen_on: today.to_string(),
        })
        .collect()
}

const GREETINGS: [&str; 6] = ["hi", "hey", "hello", "dear", "morning", "hiya"];

const SIGN_OFFS: [&str; 9] = [
    "thanks",
    "thank you",
    "many thanks",
    "cheers",
    "regards",
    "best",
    "best regards",
    "kind regards",
    "ta",
];

const BRITISH: [&str; 10] = [
    "colour",
    "behaviour",
    "organis",
    "optimis",
    "summaris",
    "centre",
    "licence",
    "analyse",
    "favour",
    "initialis",
];

const AMERICAN: [&str; 10] = [
    "color",
    "behavior",
    "organiz",
    "optimiz",
    "summariz",
    "center",
    "license",
    "analyze",
    "favor",
    "initializ",
];

fn plain(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

fn first_word(body: &str) -> Option<String> {
    let line = body.lines().find(|line| !line.trim().is_empty())?;
    let word = plain(line.split_whitespace().next()?);
    GREETINGS.contains(&word.as_str()).then_some(word)
}

fn last_words(body: &str) -> Option<String> {
    let line = body.lines().rev().find(|line| !line.trim().is_empty())?;
    let said = plain(line);
    SIGN_OFFS.contains(&said.as_str()).then_some(said)
}

fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F300..=0x1FAFF | 0x2600..=0x27BF)
}

fn sentence_case(word: &str) -> String {
    let mut letters = word.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + letters.as_str(),
        None => String::new(),
    }
}

/// The commonest of `found`, when most of `total` agree on it.
fn habit(found: &[String], total: usize) -> Option<(String, usize)> {
    let mut counts: Vec<(String, usize)> = Vec::new();

    for item in found {
        match counts.iter_mut().find(|(seen, _)| seen == item) {
            Some((_, count)) => *count += 1,
            None => counts.push((item.clone(), 1)),
        }
    }

    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    counts
        .into_iter()
        .next()
        .filter(|(_, count)| *count >= 2 && count * 2 >= total)
}

/// One fact per habit, each a count and never an impression.
///
/// A habit the sample does not settle is left out rather than guessed at: a
/// missing fact costs a draft some personality, an invented one costs the user
/// a message that does not sound like them.
fn style_facts(bodies: &[String], today: chrono::NaiveDate) -> Vec<NewFact> {
    let total = bodies.len();
    let source = format!("your last {total} pull request descriptions");
    let mut rows = Vec::new();

    let mut add = |what: &str, value: String, count: usize| {
        rows.push(NewFact {
            about: facts::YOU.to_string(),
            what: what.to_string(),
            scope: String::new(),
            value,
            count: i64::try_from(count).unwrap_or(i64::MAX),
            total: i64::try_from(total).unwrap_or(i64::MAX),
            source: source.clone(),
            seen_on: today.to_string(),
        });
    };

    let greetings: Vec<String> = bodies.iter().filter_map(|body| first_word(body)).collect();
    if let Some((word, count)) = habit(&greetings, total) {
        add(facts::GREETING, sentence_case(&word), count);
    } else if greetings.is_empty() && total >= 3 {
        add(facts::GREETING, String::new(), 0);
    }

    let sign_offs: Vec<String> = bodies.iter().filter_map(|body| last_words(body)).collect();
    if let Some((words, count)) = habit(&sign_offs, total) {
        add(facts::SIGN_OFF, sentence_case(&words), count);
    } else if sign_offs.is_empty() && total >= 3 {
        add(facts::SIGN_OFF, String::new(), 0);
    }

    let mut lengths: Vec<usize> = bodies
        .iter()
        .map(|body| body.split_whitespace().count())
        .collect();
    lengths.sort_unstable();
    add(facts::LENGTH, lengths[lengths.len() / 2].to_string(), 0);

    let with_emoji = bodies
        .iter()
        .filter(|body| body.chars().any(is_emoji))
        .count();
    add(facts::EMOJI, String::new(), with_emoji);

    let lowered = bodies.join(" ").to_lowercase();
    let british = BRITISH.iter().any(|stem| lowered.contains(stem));
    let american = AMERICAN.iter().any(|stem| lowered.contains(stem));
    match (british, american) {
        (true, false) => add(facts::SPELLING, "British".to_string(), 0),
        (false, true) => add(facts::SPELLING, "American".to_string(), 0),
        _ => {}
    }

    rows
}

/// What seeding would read and write. Runs nothing.
#[tauri::command]
pub async fn profile_plan<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<Plan, Error> {
    let context = recipe::context(&app)
        .await
        .map_err(|error| Error::Corpus(corpus::Error::Index(error.to_string())))?;

    Ok(plan(&context).await)
}

/// Count the facts. Only ever called because the user pressed the button.
///
/// Makes no model call, so it does not wake the engine: counting costs
/// milliseconds and a handful of GitHub requests, and reading two gigabytes of
/// weights to do it would not.
#[tauri::command]
pub async fn bootstrap_profile<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Report, Error> {
    let context = recipe::context(&app)
        .await
        .map_err(|error| Error::Corpus(corpus::Error::Index(error.to_string())))?;

    bootstrap(&context).await
}

/// Who turns up often enough to be the team, most-met first.
///
/// Ties break alphabetically so the file is stable: a team file that reorders
/// itself between runs looks like it changed when it did not.
fn rank(meetings: &[Vec<String>]) -> Vec<(String, usize)> {
    let mut seen: Vec<(String, usize)> = Vec::new();

    for attendees in meetings {
        for person in attendees {
            match seen.iter_mut().find(|(name, _)| name == person) {
                Some((_, count)) => *count += 1,
                None => seen.push((person.clone(), 1)),
            }
        }
    }

    seen.retain(|(_, count)| *count >= RECURRENCE);
    seen.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    seen.truncate(TEAM_SIZE);

    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::Corpus;
    use crate::db::test_support::migrated_pool;
    use crate::integrations;
    use crate::llama::test_support::serve;
    use crate::microsoft;

    /// Two pull requests with descriptions, as GitHub's search returns them.
    const WROTE: &str = r#"{
        "total_count": 2,
        "items": [
            {
                "number": 44, "title": "Stop a long answer being thrown away",
                "state": "closed", "draft": false,
                "html_url": "https://github.com/scottmallinson/chief.ai/pull/44",
                "repository_url": "https://api.github.com/repos/scottmallinson/chief.ai",
                "updated_at": "2026-08-18T10:00:00Z",
                "body": "The read timeout bounded the whole exchange rather than the gap between chunks, so a long answer died at five minutes. Measured, not guessed.",
                "pull_request": { "merged_at": "2026-08-18T09:58:00Z" }
            },
            {
                "number": 43, "title": "Run checks once",
                "state": "closed", "draft": false,
                "html_url": "https://github.com/scottmallinson/chief.ai/pull/43",
                "repository_url": "https://api.github.com/repos/scottmallinson/chief.ai",
                "updated_at": "2026-08-17T10:00:00Z",
                "body": "Release now calls the same workflow CI does, so a release is green by construction rather than by trust.",
                "pull_request": { "merged_at": "2026-08-17T09:58:00Z" }
            }
        ]
    }"#;

    struct Scratch(std::path::PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn corpus_at(name: &str) -> (Corpus, Scratch) {
        let root =
            std::env::temp_dir().join(format!("chief-profile-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        let corpus = Corpus::at(root.clone());
        corpus
            .ensure_shape()
            .await
            .expect("should create the shape");

        (corpus, Scratch(root))
    }

    /// A context with one connected GitHub account, a corpus, and stub hosts.
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

        let (corpus, scratch) = corpus_at(name).await;

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

        (context, scratch)
    }

    // ---- the files are still the user's ----

    #[tokio::test]
    async fn counts_an_untouched_starter_as_untouched() {
        let (corpus, _scratch) = corpus_at("untouched").await;

        assert!(is_untouched(&corpus, WRITING_STYLE).await);
    }

    #[tokio::test]
    async fn counts_a_file_that_is_not_there_as_untouched() {
        let (corpus, _scratch) = corpus_at("absent").await;
        let root = corpus.root().join(WRITING_STYLE);
        std::fs::remove_file(&root).expect("should remove");

        assert!(is_untouched(&corpus, WRITING_STYLE).await);
    }

    #[tokio::test]
    async fn refuses_a_file_the_user_has_edited() {
        let (corpus, _scratch) = corpus_at("edited").await;
        corpus
            .write(
                WRITING_STYLE,
                "# Writing style\n\nI always sign off with 'cheers'.\n",
            )
            .await
            .expect("should write");

        assert!(!is_untouched(&corpus, WRITING_STYLE).await);
    }

    #[tokio::test]
    async fn refuses_a_file_chief_never_created() {
        let (corpus, _scratch) = corpus_at("foreign").await;
        corpus
            .write("journal/monday.md", "mine")
            .await
            .expect("write");

        assert!(!is_untouched(&corpus, "journal/monday.md").await);
    }

    #[tokio::test]
    async fn never_touches_the_markdown_files_when_it_counts() {
        let (github_host, _github) = serve(vec![
            ("HTTP/1.1 200 OK", WROTE),
            ("HTTP/1.1 200 OK", OWN_LOGIN),
            ("HTTP/1.1 200 OK", WROTE),
            ("HTTP/1.1 200 OK", REVIEWS_44),
            ("HTTP/1.1 200 OK", REVIEWS_43),
        ]);
        let (engine_host, _engine) = serve(Vec::<(&str, &str)>::new());
        let (context, _scratch) = context_for(&github_host, &engine_host, "files").await;

        let mine = "# Writing style\n\nI always sign off with 'cheers'.\n";
        context
            .corpus
            .write(WRITING_STYLE, mine)
            .await
            .expect("write");

        bootstrap(&context).await.expect("should count");

        assert_eq!(
            context.corpus.read(WRITING_STYLE).await.expect("read"),
            mine,
            "briefs still read this file; counting is not allowed near it"
        );
    }

    // ---- rule one: nothing runs without being asked ----

    #[tokio::test]
    async fn says_what_it_would_read_and_write_without_doing_either() {
        let (host, server) = serve(Vec::<(&str, &str)>::new());
        let (context, _scratch) = context_for("127.0.0.1:1", &host, "plan").await;

        let plan = plan(&context).await;

        assert_eq!(plan.writes.len(), 3, "{:?}", plan.writes);
        assert!(plan.keeps.is_empty(), "{:?}", plan.keeps);
        assert!(
            plan.reads.iter().any(|line| line.contains("pull requests")),
            "the user is agreeing to this sentence: {:?}",
            plan.reads
        );
        assert!(
            plan.reads
                .iter()
                .any(|line| line.contains("one request to GitHub for each")),
            "the cost of reading reviews has to be said before it is paid: {:?}",
            plan.reads
        );
        assert_eq!(
            server.await.expect("server"),
            Vec::<String>::new(),
            "planning must not run the thing it is describing"
        );
        let counted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM facts")
            .fetch_one(&context.pool)
            .await
            .expect("count");
        assert_eq!(counted, 0, "and must not write anything either");
    }

    #[tokio::test]
    async fn says_what_it_would_leave_alone() {
        let (engine_host, _engine) = serve(Vec::<(&str, &str)>::new());
        let (context, _scratch) = context_for("127.0.0.1:1", &engine_host, "plankeep").await;
        context
            .corpus
            .write(WRITING_STYLE, "mine now")
            .await
            .expect("should write");
        facts::save(
            &context.pool,
            None,
            NewFact {
                about: facts::YOU.to_string(),
                what: facts::GREETING.to_string(),
                scope: String::new(),
                value: "Hey".to_string(),
                count: 0,
                total: 0,
                source: String::new(),
                seen_on: String::new(),
            },
            "2026-10-16",
        )
        .await
        .expect("a fact of the user's own");

        let plan = plan(&context).await;

        assert!(
            plan.keeps
                .iter()
                .any(|line| line.starts_with("1 fact you changed")),
            "{:?}",
            plan.keeps
        );
        assert!(
            plan.keeps
                .iter()
                .any(|line| line.contains(WRITING_STYLE) && line.contains("no longer")),
            "{:?}",
            plan.keeps
        );
    }

    // ---- counting ----

    /// The user's own handle, as GitHub's `/user` answers.
    const OWN_LOGIN: &str = r#"{"login": "Octocat"}"#;

    /// Reviews on the two pull requests in [`WROTE`]: Priya on both, a bot, the
    /// user themselves, and a review nobody has submitted.
    const REVIEWS_44: &str = r#"[
        {"state": "APPROVED", "user": {"login": "Priya-N", "type": "User"}},
        {"state": "COMMENTED", "user": {"login": "Priya-N", "type": "User"}},
        {"state": "COMMENTED", "user": {"login": "dependabot[bot]", "type": "Bot"}},
        {"state": "COMMENTED", "user": {"login": "octocat", "type": "User"}},
        {"state": "PENDING", "user": {"login": "ghost", "type": "User"}}
    ]"#;

    const REVIEWS_43: &str = r#"[
        {"state": "CHANGES_REQUESTED", "user": {"login": "priya-n", "type": "User"}},
        {"state": "APPROVED", "user": {"login": "tomasz", "type": "User"}}
    ]"#;

    async fn counted(context: &recipe::Context) -> Vec<facts::Fact> {
        facts::list(&context.pool).await.expect("list")
    }

    #[tokio::test]
    async fn counts_style_and_reviewers_from_the_users_own_work() {
        // Order of requests: the description sample, the user's handle, the
        // pull requests whose reviews are read, then one request for each.
        let (github_host, github) = serve(vec![
            ("HTTP/1.1 200 OK", WROTE),
            ("HTTP/1.1 200 OK", OWN_LOGIN),
            ("HTTP/1.1 200 OK", WROTE),
            ("HTTP/1.1 200 OK", REVIEWS_44),
            ("HTTP/1.1 200 OK", REVIEWS_43),
        ]);
        let (engine_host, engine) = serve(Vec::<(&str, &str)>::new());
        let (context, _scratch) = context_for(&github_host, &engine_host, "style").await;

        let report = bootstrap(&context).await.expect("should count");

        assert!(report
            .written
            .iter()
            .any(|line| line.contains("how you write")));

        let rows = counted(&context).await;
        let line = |what: &str| {
            rows.iter()
                .find(|row| row.what == what && row.about == facts::YOU)
                .map(facts::style_line)
                .unwrap_or_default()
        };
        assert!(line(facts::LENGTH).starts_with("Length: about"), "{rows:?}");
        assert_eq!(line(facts::EMOJI), "Emoji: none (0 of 2).");
        assert!(
            rows.iter().all(counted_with_a_date),
            "a count with no date cannot be called stale: {rows:?}"
        );

        let reviews: Vec<(String, i64, i64)> = rows
            .iter()
            .filter(|row| row.what == facts::REVIEWS)
            .map(|row| (row.about.clone(), row.count, row.total))
            .collect();
        assert_eq!(
            reviews,
            [("priya-n".to_string(), 2, 2), ("tomasz".to_string(), 1, 2)],
            "Priya reviewed both, counted once each however many reviews she left; \
             the bot, the user and an unsubmitted review are not people who reviewed"
        );
        assert!(rows
            .iter()
            .all(|row| row.scope.is_empty() || row.scope == "scottmallinson/chief.ai"));

        let _ = github.await;
        assert_eq!(
            engine.await.expect("engine"),
            Vec::<String>::new(),
            "counting is not a model call"
        );
    }

    fn counted_with_a_date(row: &facts::Fact) -> bool {
        chrono::NaiveDate::parse_from_str(&row.seen_on, "%Y-%m-%d").is_ok()
    }

    #[tokio::test]
    async fn a_pull_request_whose_reviews_cannot_be_read_costs_only_itself() {
        let (github_host, _github) = serve(vec![
            ("HTTP/1.1 200 OK", WROTE),
            ("HTTP/1.1 200 OK", OWN_LOGIN),
            ("HTTP/1.1 200 OK", WROTE),
            ("HTTP/1.1 404 Not Found", r#"{"message": "Not Found"}"#),
            ("HTTP/1.1 200 OK", REVIEWS_43),
        ]);
        let (engine_host, _engine) = serve(Vec::<(&str, &str)>::new());
        let (context, _scratch) = context_for(&github_host, &engine_host, "gone").await;

        bootstrap(&context).await.expect("should count");

        let reviewers: Vec<(String, i64, i64)> = counted(&context)
            .await
            .into_iter()
            .filter(|row| row.what == facts::REVIEWS)
            .map(|row| (row.about, row.count, row.total))
            .collect();
        assert_eq!(
            reviewers,
            [("priya-n".to_string(), 1, 1), ("tomasz".to_string(), 1, 1)],
            "the one that could not be read is neither counted nor held against anyone"
        );
    }

    #[tokio::test]
    async fn keeps_the_reviewers_it_counted_when_github_cannot_be_reached() {
        let (engine_host, _engine) = serve(Vec::<(&str, &str)>::new());
        let (context, _scratch) = context_for("127.0.0.1:1", &engine_host, "offline").await;
        facts::recount(
            &context.pool,
            &[facts::REVIEWS],
            vec![NewFact {
                about: "priya-n".to_string(),
                what: facts::REVIEWS.to_string(),
                scope: "acme/api".to_string(),
                value: String::new(),
                count: 5,
                total: 6,
                source: "GitHub reviews on your pull requests".to_string(),
                seen_on: "2026-10-01".to_string(),
            }],
        )
        .await
        .expect("an earlier count");

        let outcome = bootstrap(&context).await;

        assert!(
            matches!(outcome, Err(Error::NothingToLearnFrom)),
            "{outcome:?}"
        );
        assert_eq!(
            counted(&context).await.len(),
            1,
            "an unreachable GitHub is not evidence that nobody reviews"
        );
    }

    #[tokio::test]
    async fn says_so_rather_than_inventing_a_style_from_nothing() {
        let (engine_host, engine) = serve(Vec::<(&str, &str)>::new());
        let (context, _scratch) = context_for("127.0.0.1:1", &engine_host, "nothing").await;

        let error = bootstrap(&context).await.expect_err("nothing to count");

        assert!(matches!(error, Error::NothingToLearnFrom), "{error:?}");
        assert!(counted(&context).await.is_empty());
        assert_eq!(engine.await.expect("engine"), Vec::<String>::new());
    }

    #[tokio::test]
    async fn never_overwrites_what_the_user_wrote() {
        let (github_host, _github) = serve(vec![
            ("HTTP/1.1 200 OK", WROTE),
            ("HTTP/1.1 200 OK", OWN_LOGIN),
            ("HTTP/1.1 200 OK", WROTE),
            ("HTTP/1.1 200 OK", REVIEWS_44),
            ("HTTP/1.1 200 OK", REVIEWS_43),
        ]);
        let (engine_host, _engine) = serve(Vec::<(&str, &str)>::new());
        let (context, _scratch) = context_for(&github_host, &engine_host, "keep").await;

        facts::save(
            &context.pool,
            None,
            NewFact {
                about: facts::YOU.to_string(),
                what: facts::EMOJI.to_string(),
                scope: String::new(),
                value: "never".to_string(),
                count: 0,
                total: 0,
                source: String::new(),
                seen_on: String::new(),
            },
            "2026-10-16",
        )
        .await
        .expect("save");

        let report = bootstrap(&context).await.expect("should count");

        let emoji = counted(&context)
            .await
            .into_iter()
            .find(|row| row.what == facts::EMOJI)
            .expect("emoji row");
        assert_eq!(emoji.origin, facts::YOURS);
        assert_eq!(emoji.value, "never", "the user's word beat the count");
        assert!(
            report.kept.iter().any(|line| line.starts_with("1 facts")),
            "and the report says so: {report:?}"
        );
    }

    fn bodies(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|text| text.to_string()).collect()
    }

    /// The style, as the lines a prompt would carry.
    fn rules(texts: &[&str]) -> Vec<String> {
        style_facts(
            &bodies(texts),
            chrono::NaiveDate::from_ymd_opt(2026, 10, 13).expect("date"),
        )
        .iter()
        .map(|row| facts::style_text(&row.what, &row.value, row.count, row.total))
        .collect()
    }

    #[test]
    fn counts_a_greeting_only_when_most_descriptions_share_it() {
        let rules = rules(&[
            "Hi Sam,\nFixes the timeout.",
            "Hi Priya,\nRuns checks once.",
            "Hi team,\nDrops the flag.",
            "Drops the other flag.",
        ]);

        assert!(
            rules.contains(&"Greeting: opens with \"Hi\" (3 of 4).".to_string()),
            "{rules:?}"
        );
    }

    #[test]
    fn leaves_a_habit_out_rather_than_guessing_when_the_sample_is_split() {
        let rules = rules(&[
            "Hi Sam,\nFixes it.",
            "Hey Priya,\nFixes that.",
            "Hello team,\nFixes this.",
            "Fixes the other.",
            "Fixes another.",
        ]);

        assert!(
            !rules.iter().any(|rule| rule.starts_with("Greeting")),
            "no greeting is common enough to be a rule: {rules:?}"
        );
    }

    #[test]
    fn counts_a_sign_off_and_says_so_when_there_is_none() {
        let signed = rules(&[
            "Fixes it.\n\nThanks",
            "Fixes that.\n\nThanks!",
            "Fixes this.",
        ]);
        assert!(
            signed.contains(&"Sign-off: ends with \"Thanks\" (2 of 3).".to_string()),
            "{signed:?}"
        );

        let unsigned = rules(&["Fixes it.", "Fixes that.", "Fixes this."]);
        assert!(
            unsigned.contains(&"Sign-off: none (0 of 3).".to_string()),
            "{unsigned:?}"
        );
    }

    #[test]
    fn counts_length_as_the_median_of_the_descriptions() {
        let rules = rules(&["one two", "one two three four", "one"]);

        assert!(
            rules.contains(&"Length: about 2 words (median of 3).".to_string()),
            "{rules:?}"
        );
    }

    #[test]
    fn counts_emoji_and_spelling() {
        let rules = rules(&[
            "Fixes the colour handling \u{1F389}",
            "Optimise the centre panel",
        ]);

        assert!(
            rules.contains(&"Emoji: used in 1 of 2.".to_string()),
            "{rules:?}"
        );
        assert!(
            rules.contains(&"Spelling: British.".to_string()),
            "{rules:?}"
        );

        let mixed = self::rules(&["The colour", "The color"]);
        assert!(
            !mixed.iter().any(|rule| rule.starts_with("Spelling")),
            "a split sample is not a spelling: {mixed:?}"
        );
    }

    // ---- the team is counted, not inferred ----

    #[test]
    fn keeps_only_the_people_you_meet_more_than_once() {
        let counted = rank(&[
            vec!["Ana Silva".to_string(), "Sam Patel".to_string()],
            vec!["Ana Silva".to_string()],
            vec!["A Vendor".to_string()],
            vec!["Sam Patel".to_string()],
            vec!["Ana Silva".to_string()],
        ]);

        assert_eq!(
            counted,
            vec![("Ana Silva".to_string(), 3), ("Sam Patel".to_string(), 2)],
            "one meeting is an interview, not a working relationship"
        );
    }

    #[test]
    fn names_nobody_when_every_meeting_was_a_one_off() {
        assert!(rank(&[vec!["A Vendor".to_string()]]).is_empty());
    }

    #[test]
    fn stops_at_a_team_rather_than_an_address_book() {
        let crowd: Vec<Vec<String>> = (0..40)
            .map(|n| vec![format!("Person {n:02}"), format!("Person {n:02}")])
            .collect();

        assert_eq!(rank(&crowd).len(), TEAM_SIZE);
    }
}
