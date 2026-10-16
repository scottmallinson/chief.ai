//! Filling in the corpus from the user's own work.
//!
//! `ensure_shape` creates seven files with a heading each and nothing in them,
//! and a draft written against an empty `writing_style.md` sounds like a model.
//! COSTA's finding, and the reason this step exists: stale or absent context
//! produces generic drafts, so the profile has to be seeded from something real
//! before Proposed Actions is worth building.
//!
//! Three rules hold this together, and all three are about not taking something
//! that is the user's:
//!
//! 1. **Nothing runs without being asked.** [`plan`] says what would be read and
//!    what would be written, in those words, and returns without touching
//!    anything. The interface shows it and waits. This is the first feature that
//!    reads the user's own writing in bulk to build a profile of it, and a
//!    profile built quietly is the wrong way to do that however local it stays.
//! 2. **A file the user has touched is theirs.** A starter file is replaced only
//!    while it is still byte-for-byte the starter Chief wrote. See
//!    [`is_untouched`].
//! 3. **Counted, not inferred.** The writing style and the team are both
//!    arithmetic over what the user wrote and who they met. A 3B model asked to
//!    describe a voice made up habits the samples did not show, so no model is
//!    called here at all. Each file says what it was counted from and when, so
//!    a reader can tell a fact from a stale one: see [`counted_on`].

use serde::Serialize;

use crate::corpus;
use crate::github;
use crate::llama;
use crate::recipe;
use crate::session::{GithubSession, OutlookSession};

/// Where the two profile files live.
pub const WRITING_STYLE: &str = "context/agents/profile/writing_style.md";
pub const TEAM_STRUCTURE: &str = "context/agents/org/team_structure.md";

/// How many of the user's own pull requests are sampled for a writing style.
///
/// A sample, not a corpus. Enough for a voice to show through, few enough that
/// the one call this makes stays inside the prompt budget.
const SAMPLE: u8 = 20;

/// How many days of calendar are counted for the team.
const TEAM_DAYS: i64 = 28;

/// How many people the team file names. Past this it is an address book.
const TEAM_SIZE: usize = 12;

/// How many meetings somebody has to appear in before they are on the team.
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
    #[error("there is nothing to learn a writing style from yet")]
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
    /// Which corpus files it would write.
    pub writes: Vec<String>,
    /// Files it would leave alone because they are no longer the starter Chief
    /// wrote. Named so the user can see why nothing happened to them.
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
/// Content, not modification time. mtime says *when* a file was last written
/// and content says *what is in it*, and only the second is the question being
/// asked: a git checkout, a Dropbox sync or a restored backup all move mtime
/// without changing a byte, and an editor that saves an unmodified buffer does
/// the same. Judged on mtime, Chief would refuse to seed a file nobody had ever
/// opened. Judged on content it refuses exactly when there is something of the
/// user's to lose, which is the rule this is trying to keep.
///
/// A file that is absent counts as untouched: there is nothing there to lose.
pub async fn is_untouched(corpus: &corpus::Corpus, path: &str) -> bool {
    let Some(starter) = corpus::starter(path) else {
        // Not a file Chief created, so not one it may overwrite.
        return false;
    };

    match corpus.read(path).await {
        Ok(contents) => contents == starter,
        Err(corpus::Error::NoSuchFile(_)) => true,
        Err(_) => false,
    }
}

/// What seeding would do, without doing any of it.
pub async fn plan(context: &recipe::Context) -> Plan {
    let mut writes = Vec::new();
    let mut keeps = Vec::new();

    for path in [WRITING_STYLE, TEAM_STRUCTURE] {
        if is_untouched(&context.corpus, path).await {
            writes.push(path.to_string());
        } else {
            keeps.push(path.to_string());
        }
    }

    Plan {
        reads: vec![
            format!("The descriptions you wrote on your last {SAMPLE} pull requests"),
            format!("Who you met with over the last {TEAM_DAYS} days, and how often"),
        ],
        writes,
        keeps,
    }
}

/// Seed the profile from the user's own work.
///
/// Every file is written only if it is still the starter, so running this twice
/// is not destructive — the second run reports the same files as kept.
pub async fn bootstrap(context: &recipe::Context) -> Result<Report, Error> {
    let mut report = Report::default();

    for (path, drafted) in [
        (WRITING_STYLE, writing_style(context).await?),
        (TEAM_STRUCTURE, team_structure(context).await),
    ] {
        let Some(markdown) = drafted else {
            continue;
        };

        if is_untouched(&context.corpus, path).await {
            context.corpus.write(path, &markdown).await?;
            report.written.push(path.to_string());
        } else {
            report.kept.push(path.to_string());
        }
    }

    Ok(report)
}

/// The user's own prose, as a sample for the model to describe.
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

/// The writing style, counted from the sample.
async fn writing_style(context: &recipe::Context) -> Result<Option<String>, Error> {
    // Nothing to write about is not an empty style file, it is a machine with
    // no connected account or nobody who writes pull request descriptions.
    if !is_untouched(&context.corpus, WRITING_STYLE).await {
        return Ok(None);
    }

    let written = sample(context).await;
    if written.is_empty() {
        return Err(Error::NothingToLearnFrom);
    }

    Ok(Some(render_style(
        &written,
        chrono::Local::now().date_naive(),
    )))
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

/// One line per habit, each a count and never an impression.
///
/// A habit the sample does not settle is left out rather than guessed at: a
/// missing line costs a draft some personality, an invented one costs the user
/// a message that does not sound like them.
fn style_rules(bodies: &[String]) -> Vec<String> {
    let total = bodies.len();
    let mut rules = Vec::new();

    let greetings: Vec<String> = bodies.iter().filter_map(|body| first_word(body)).collect();
    if let Some((word, count)) = habit(&greetings, total) {
        rules.push(format!(
            "Greeting: opens with \"{}\" ({count} of {total}).",
            sentence_case(&word)
        ));
    } else if greetings.is_empty() && total >= 3 {
        rules.push(format!("Greeting: none (0 of {total})."));
    }

    let sign_offs: Vec<String> = bodies.iter().filter_map(|body| last_words(body)).collect();
    if let Some((words, count)) = habit(&sign_offs, total) {
        rules.push(format!(
            "Sign-off: ends with \"{}\" ({count} of {total}).",
            sentence_case(&words)
        ));
    } else if sign_offs.is_empty() && total >= 3 {
        rules.push(format!("Sign-off: none (0 of {total})."));
    }

    let mut lengths: Vec<usize> = bodies
        .iter()
        .map(|body| body.split_whitespace().count())
        .collect();
    lengths.sort_unstable();
    rules.push(format!(
        "Length: about {} words (median of {total}).",
        lengths[lengths.len() / 2]
    ));

    let with_emoji = bodies
        .iter()
        .filter(|body| body.chars().any(is_emoji))
        .count();
    rules.push(if with_emoji == 0 {
        format!("Emoji: none (0 of {total}).")
    } else {
        format!("Emoji: used in {with_emoji} of {total}.")
    });

    let lowered = bodies.join(" ").to_lowercase();
    let british = BRITISH.iter().any(|stem| lowered.contains(stem));
    let american = AMERICAN.iter().any(|stem| lowered.contains(stem));
    match (british, american) {
        (true, false) => rules.push("Spelling: British.".to_string()),
        (false, true) => rules.push("Spelling: American.".to_string()),
        _ => {}
    }

    rules
}

fn render_style(bodies: &[String], today: chrono::NaiveDate) -> String {
    let rules = style_rules(bodies)
        .iter()
        .map(|rule| format!("- {rule}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "{STYLE_PREAMBLE}\n\nSource: your last {} pull request descriptions, counted {today}.\n\n{rules}\n",
        bodies.len()
    )
}

/// When a profile file says it was counted. A file the user wrote has no such
/// line, and so no date, and is never called stale.
pub fn counted_on(text: &str) -> Option<chrono::NaiveDate> {
    let line = text.lines().find(|line| line.starts_with("Source:"))?;
    let date = line.trim_end_matches('.').rsplit("counted ").next()?;
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

/// The people the user actually works with, counted rather than inferred.
///
/// No model call. Who somebody meets, and how often, is arithmetic — asking a
/// 3B model to do it would be slower, less accurate and would spend the one
/// call this step is allowed.
///
/// Reviewers and repository collaborators belong here too and are deliberately
/// absent: GitHub's search response carries neither, so gathering them is one
/// API call per pull request, which is the per-artefact fan-out this step is
/// specifically not allowed to do.
async fn team_structure(context: &recipe::Context) -> Option<String> {
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

    let team = rank(&meetings);

    if team.is_empty() {
        return None;
    }

    let people = team
        .iter()
        .map(|(name, count)| {
            format!("## {name}\n\nIn {count} of your meetings this month. What do they own?\n")
        })
        .collect::<Vec<_>>()
        .join("\n");

    Some(format!(
        "{TEAM_PREAMBLE}\n\nSource: meetings over the last {TEAM_DAYS} days, counted {}.\n\n{people}",
        now.date_naive()
    ))
}

/// What seeding would read and write. Runs nothing.
#[tauri::command]
pub async fn profile_plan<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<Plan, Error> {
    let context = recipe::context(&app)
        .await
        .map_err(|error| Error::Corpus(corpus::Error::Index(error.to_string())))?;

    Ok(plan(&context).await)
}

/// Seed the profile. Only ever called because the user pressed the button.
///
/// Makes no model call, so it does not wake the engine: counting a sample
/// costs milliseconds, and reading two gigabytes of weights to do it would not.
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

/// Said at the top of the file rather than in a dialog nobody keeps.
const STYLE_PREAMBLE: &str = "\
# Writing style

Chief counted this from the descriptions on your own pull requests. It is a
draft: edit it, and Chief will not touch it again.";

const TEAM_PREAMBLE: &str = "\
# Team

Chief drafted this from who you have been meeting with. The headings are the
people; what they own is for you to fill in. Edit it and Chief will leave it
alone.";

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

    /// The same search, but nobody wrote a description.
    const WROTE_NOTHING: &str = r#"{
        "total_count": 1,
        "items": [
            {
                "number": 44, "title": "A title and nothing else",
                "state": "closed", "draft": false, "body": null,
                "html_url": "https://github.com/x/y/pull/44",
                "repository_url": "https://api.github.com/repos/x/y",
                "updated_at": "2026-08-18T10:00:00Z",
                "pull_request": { "merged_at": "2026-08-18T09:58:00Z" }
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

    // ---- rule two: a file the user has touched is theirs ----

    #[tokio::test]
    async fn counts_an_untouched_starter_as_ours_to_replace() {
        let (corpus, _scratch) = corpus_at("untouched").await;

        assert!(is_untouched(&corpus, WRITING_STYLE).await);
    }

    #[tokio::test]
    async fn counts_a_file_that_is_not_there_as_ours_to_write() {
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

        assert!(
            !is_untouched(&corpus, "journal/monday.md").await,
            "only a starter Chief wrote is Chief's to replace"
        );
    }

    // ---- rule one: nothing runs without being asked ----

    #[tokio::test]
    async fn says_what_it_would_read_and_write_without_doing_either() {
        let (host, server) = serve(Vec::<(&str, &str)>::new());
        let (context, _scratch) = context_for("127.0.0.1:1", &host, "plan").await;

        let plan = plan(&context).await;

        assert_eq!(plan.writes, vec![WRITING_STYLE, TEAM_STRUCTURE]);
        assert!(plan.keeps.is_empty());
        assert!(
            plan.reads.iter().any(|line| line.contains("pull requests")),
            "the user is agreeing to this sentence: {:?}",
            plan.reads
        );
        assert_eq!(
            server.await.expect("server"),
            Vec::<String>::new(),
            "planning must not run the thing it is describing"
        );
    }

    #[tokio::test]
    async fn says_which_files_it_would_leave_alone() {
        let (engine_host, _engine) = serve(Vec::<(&str, &str)>::new());
        let (context, _scratch) = context_for("127.0.0.1:1", &engine_host, "plankeep").await;
        context
            .corpus
            .write(WRITING_STYLE, "mine now")
            .await
            .expect("should write");

        let plan = plan(&context).await;

        assert_eq!(plan.keeps, vec![WRITING_STYLE]);
        assert_eq!(plan.writes, vec![TEAM_STRUCTURE]);
    }

    // ---- rule three: one call over a sample ----

    #[tokio::test]
    async fn writes_a_style_counted_from_the_users_own_descriptions() {
        let (github_host, github) = serve(vec![("HTTP/1.1 200 OK", WROTE)]);
        let (engine_host, engine) = serve(Vec::<(&str, &str)>::new());
        let (context, _scratch) = context_for(&github_host, &engine_host, "style").await;

        let report = bootstrap(&context).await.expect("should draft");

        assert!(report.written.contains(&WRITING_STYLE.to_string()));

        let written = context
            .corpus
            .read(WRITING_STYLE)
            .await
            .expect("should read");
        assert!(written.contains("- Length: about"), "{written}");
        assert!(written.contains("- Emoji: none (0 of 2)."), "{written}");
        assert!(
            written.contains("It is a\ndraft: edit it"),
            "the file has to say it is a draft: {written}"
        );
        assert!(
            counted_on(&written).is_some(),
            "a count with no date cannot be called stale: {written}"
        );

        let _ = github.await;
        assert_eq!(
            engine.await.expect("engine"),
            Vec::<String>::new(),
            "counting a sample is not a model call"
        );
    }

    fn bodies(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|text| text.to_string()).collect()
    }

    #[test]
    fn counts_a_greeting_only_when_most_descriptions_share_it() {
        let rules = style_rules(&bodies(&[
            "Hi Sam,\nFixes the timeout.",
            "Hi Priya,\nRuns checks once.",
            "Hi team,\nDrops the flag.",
            "Drops the other flag.",
        ]));

        assert!(
            rules.contains(&"Greeting: opens with \"Hi\" (3 of 4).".to_string()),
            "{rules:?}"
        );
    }

    #[test]
    fn leaves_a_habit_out_rather_than_guessing_when_the_sample_is_split() {
        let rules = style_rules(&bodies(&[
            "Hi Sam,\nFixes it.",
            "Hey Priya,\nFixes that.",
            "Hello team,\nFixes this.",
            "Fixes the other.",
            "Fixes another.",
        ]));

        assert!(
            !rules.iter().any(|rule| rule.starts_with("Greeting")),
            "no greeting is common enough to be a rule: {rules:?}"
        );
    }

    #[test]
    fn counts_a_sign_off_and_says_so_when_there_is_none() {
        let signed = style_rules(&bodies(&[
            "Fixes it.\n\nThanks",
            "Fixes that.\n\nThanks!",
            "Fixes this.",
        ]));
        assert!(
            signed.contains(&"Sign-off: ends with \"Thanks\" (2 of 3).".to_string()),
            "{signed:?}"
        );

        let unsigned = style_rules(&bodies(&["Fixes it.", "Fixes that.", "Fixes this."]));
        assert!(
            unsigned.contains(&"Sign-off: none (0 of 3).".to_string()),
            "{unsigned:?}"
        );
    }

    #[test]
    fn counts_length_as_the_median_of_the_descriptions() {
        let rules = style_rules(&bodies(&["one two", "one two three four", "one"]));

        assert!(
            rules.contains(&"Length: about 2 words (median of 3).".to_string()),
            "{rules:?}"
        );
    }

    #[test]
    fn counts_emoji_and_spelling() {
        let rules = style_rules(&bodies(&[
            "Fixes the colour handling \u{1F389}",
            "Optimise the centre panel",
        ]));

        assert!(
            rules.contains(&"Emoji: used in 1 of 2.".to_string()),
            "{rules:?}"
        );
        assert!(
            rules.contains(&"Spelling: British.".to_string()),
            "{rules:?}"
        );

        let mixed = style_rules(&bodies(&["The colour", "The color"]));
        assert!(
            !mixed.iter().any(|rule| rule.starts_with("Spelling")),
            "a split sample is not a spelling: {mixed:?}"
        );
    }

    #[test]
    fn reads_back_the_date_a_file_was_counted_and_none_from_a_hand_written_one() {
        let text = render_style(
            &bodies(&["Fixes it."]),
            chrono::NaiveDate::from_ymd_opt(2026, 10, 13).expect("date"),
        );

        assert_eq!(
            counted_on(&text),
            chrono::NaiveDate::from_ymd_opt(2026, 10, 13)
        );
        assert_eq!(counted_on("# Writing style\n\nI write short."), None);
    }

    #[tokio::test]
    async fn says_so_rather_than_inventing_a_style_from_nothing() {
        let (github_host, _github) = serve(vec![("HTTP/1.1 200 OK", WROTE_NOTHING)]);
        let (engine_host, engine) = serve(Vec::<(&str, &str)>::new());
        let (context, _scratch) = context_for(&github_host, &engine_host, "nothing").await;

        let error = bootstrap(&context).await.expect_err("nothing was written");

        assert!(matches!(error, Error::NothingToLearnFrom), "{error:?}");
        assert_eq!(
            engine.await.expect("engine"),
            Vec::<String>::new(),
            "there was nothing to send"
        );
    }

    #[tokio::test]
    async fn never_overwrites_what_the_user_wrote() {
        let (github_host, _github) = serve(vec![("HTTP/1.1 200 OK", WROTE)]);
        let (engine_host, engine) = serve(Vec::<(&str, &str)>::new());
        let (context, _scratch) = context_for(&github_host, &engine_host, "keep").await;

        let mine = "# Writing style\n\nI always sign off with 'cheers'.\n";
        context
            .corpus
            .write(WRITING_STYLE, mine)
            .await
            .expect("write");

        bootstrap(&context).await.expect("should not fail");

        assert_eq!(
            context.corpus.read(WRITING_STYLE).await.expect("read"),
            mine,
            "the user's file is the user's"
        );
        assert_eq!(
            engine.await.expect("engine"),
            Vec::<String>::new(),
            "and there was no engine to ask"
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
