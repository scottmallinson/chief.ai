//! Routing a question without asking a model what it is.
//!
//! The blueprint calls this the Overseer. It is code, not a second model: on a
//! machine where prefill runs at 18–34 tokens a second, the largest
//! optimisation available is not calling the model at all.
//!
//! **Conservative by construction.** A false positive silently replaces the
//! user's question with a canned answer, which is a worse failure than a miss —
//! a miss just costs what every question cost before this module existed.
//! Matching is therefore on the *whole* normalised question against a written
//! list of phrases, never on a keyword found somewhere inside one. That is what
//! keeps "how do I brief a client?" and "a brief history of Rust" out of the
//! brief, and it is why the list is long and dull rather than clever.
//!
//! A routed intent costs **zero model calls**. Everything here is answered from
//! the local database, the corpus, or a connected account's own API.

use crate::agent::Update;
use crate::recipe;
use crate::session::OutlookSession;
use crate::work_log;

/// How many entries `/log` answers with. Enough to see the shape of a week.
const LOG_ENTRIES: i64 = 10;

/// What the user asked for, when it is something we can answer without a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// Today's brief, as already written. Never regenerated: writing one is a
    /// model call, and `/brief` is meant to be the free way to read it.
    Brief,
    /// What today looks like — the meetings, in order.
    Prep,
    /// What has been logged lately.
    Log,
}

/// The slash commands, matched before anything else and matched exactly.
const COMMANDS: [(&str, Intent); 3] = [
    ("/brief", Intent::Brief),
    ("/prep", Intent::Prep),
    ("/log", Intent::Log),
];

/// The natural-language phrases that route, written out in full.
///
/// Whole-question equality, not containment. Every phrase here is one somebody
/// would type meaning exactly this and nothing else; anything less certain is
/// deliberately left to the model, which has the context to tell the
/// difference and is allowed to be wrong out loud.
const PHRASES: [(&str, Intent); 22] = [
    ("brief me", Intent::Brief),
    ("brief me for today", Intent::Brief),
    ("my brief", Intent::Brief),
    ("my brief for today", Intent::Brief),
    ("what is my brief", Intent::Brief),
    ("whats my brief", Intent::Brief),
    ("todays brief", Intent::Brief),
    ("daily brief", Intent::Brief),
    ("my daily brief", Intent::Brief),
    ("show me my brief", Intent::Brief),
    ("prep me", Intent::Prep),
    ("prep me for today", Intent::Prep),
    ("what is on my calendar", Intent::Prep),
    ("whats on my calendar", Intent::Prep),
    ("what is on my calendar today", Intent::Prep),
    ("whats on my calendar today", Intent::Prep),
    ("what meetings do i have", Intent::Prep),
    ("what meetings do i have today", Intent::Prep),
    ("my work log", Intent::Log),
    ("show me my work log", Intent::Log),
    ("what did i ship", Intent::Log),
    ("what have i shipped", Intent::Log),
];

/// What this question is asking for, if it is asking for one of these.
///
/// `None` means the tool loop handles it, exactly as it did before.
#[must_use]
pub fn route(question: &str) -> Option<Intent> {
    let asked = normalise(question);

    // Commands first, and on the first word only, so `/brief` takes the route
    // whatever the user typed after it.
    if let Some(command) = asked.split_whitespace().next() {
        if let Some((_, intent)) = COMMANDS.iter().find(|(name, _)| *name == command) {
            return Some(*intent);
        }
    }

    PHRASES
        .iter()
        .find(|(phrase, _)| *phrase == asked)
        .map(|(_, intent)| *intent)
}

/// Fold away everything that is not the question: case, the two apostrophes, the
/// punctuation a question ends with, and repeated spaces.
///
/// Apostrophes are dropped rather than normalised to one form because the two
/// characters and the elision itself all have to land on the same string —
/// "what's", "what’s" and "whats" are one question typed three ways.
fn normalise(question: &str) -> String {
    let folded: String = question
        .to_lowercase()
        .chars()
        .filter(|character| !matches!(character, '\'' | '\u{2019}'))
        .map(|character| {
            if character.is_alphanumeric() || character == '/' {
                character
            } else {
                ' '
            }
        })
        .collect();

    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Answer a routed intent from local data, without a model.
///
/// `None` means there was nothing to say, and the question goes to the tool
/// loop after all. That is the safety valve on the whole idea: a routed intent
/// that would answer "you have nothing" is indistinguishable, to the reader,
/// from Chief having misunderstood them — so it steps aside and lets the model
/// answer instead. The cost of being wrong is one model call, which is what the
/// question cost before this module existed.
pub async fn answer(intent: Intent, context: &recipe::Context) -> Option<String> {
    let markdown = match intent {
        Intent::Brief => brief(context).await,
        Intent::Prep => prep(context).await,
        Intent::Log => log(context).await,
    }?;

    (!markdown.trim().is_empty()).then_some(markdown)
}

/// Today's brief, as written. Reading, never regenerating.
async fn brief(context: &recipe::Context) -> Option<String> {
    let path = format!("briefs/{}.md", recipe::today_date());

    context.corpus.read(&path).await.ok()
}

/// Today's meetings, in the order they happen.
async fn prep(context: &recipe::Context) -> Option<String> {
    let (from, to) = recipe::today();
    let mut lines = Vec::new();

    for account in recipe::outlook_accounts(context).await {
        let session = OutlookSession::new(&context.pool, &context.microsoft, account);

        if let Ok(events) = session.events(&from, &to, recipe::PER_SOURCE).await {
            lines.extend(events.iter().map(recipe::describe_event));
        }
    }

    bullets("Today", &lines)
}

/// What the daemon has logged lately, newest first.
async fn log(context: &recipe::Context) -> Option<String> {
    let entries = work_log::fetch(&context.pool, Some(LOG_ENTRIES))
        .await
        .ok()?;

    let lines: Vec<String> = entries
        .iter()
        .map(|entry| {
            entry
                .summary
                .clone()
                .unwrap_or_else(|| entry.content.clone())
        })
        .collect();

    bullets("Recently logged", &lines)
}

/// A heading and a bullet each, or nothing at all when there is nothing to say.
fn bullets(heading: &str, lines: &[String]) -> Option<String> {
    if lines.is_empty() {
        return None;
    }

    let body = lines
        .iter()
        .map(|line| format!("- {line}"))
        .collect::<Vec<_>>()
        .join("\n");

    Some(format!("## {heading}\n{body}"))
}

/// Answer this question here, if it is one of ours.
///
/// `Some` means it was answered and the model was never asked — not to route,
/// and not to write. `None` means the tool loop takes it, exactly as before,
/// which is both the fall-through for a question we do not recognise and the
/// one for a question we do but have nothing to say about.
///
/// The whole answer arrives in a single [`Update::Delta`]. There is nothing to
/// stream: it was read off the disk, and pretending otherwise by dribbling it
/// out would be an animation of work that is already done.
pub async fn deliver<F>(
    question: &str,
    context: &recipe::Context,
    mut on_update: F,
) -> Option<String>
where
    F: FnMut(Update),
{
    let markdown = answer(route(question)?, context).await?;

    on_update(Update::Delta {
        text: markdown.clone(),
    });

    Some(markdown)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every phrase a user might type meaning one of these, and the route it
    /// must take. Failing here is a question silently answered wrongly.
    #[test]
    fn routes_every_slash_command() {
        assert_eq!(route("/brief"), Some(Intent::Brief));
        assert_eq!(route("/prep"), Some(Intent::Prep));
        assert_eq!(route("/log"), Some(Intent::Log));
    }

    #[test]
    fn routes_a_command_whatever_follows_it() {
        assert_eq!(route("/brief please"), Some(Intent::Brief));
        assert_eq!(route("/log for this week"), Some(Intent::Log));
    }

    #[test]
    fn routes_a_command_with_stray_whitespace_and_case() {
        assert_eq!(route("  /BRIEF  "), Some(Intent::Brief));
    }

    #[test]
    fn routes_every_written_phrase() {
        for (phrase, expected) in PHRASES {
            assert_eq!(route(phrase), Some(expected), "{phrase} should route");
        }
    }

    #[test]
    fn reads_the_two_apostrophes_and_the_elision_as_one_question() {
        for asked in [
            "what's my brief",
            "what\u{2019}s my brief",
            "whats my brief",
        ] {
            assert_eq!(route(asked), Some(Intent::Brief), "{asked} should route");
        }
    }

    #[test]
    fn ignores_the_punctuation_a_question_ends_with() {
        assert_eq!(route("What did I ship?"), Some(Intent::Log));
        assert_eq!(route("Brief me!"), Some(Intent::Brief));
    }

    /// The list this module exists to get right.
    ///
    /// Every one of these contains a trigger word and must still reach the
    /// model. A false positive here replaces somebody's actual question with a
    /// canned answer and never tells them it did.
    #[test]
    fn refuses_everything_that_merely_mentions_a_trigger_word() {
        for asked in [
            "how many shipping containers fit on a panamax vessel?",
            "write a haiku",
            "how do I brief a client?",
            "give me a brief history of Rust",
            "what is a work log?",
            "should I log this or is it too small?",
            "can you log that I shipped the release workflow?",
            "what did I ship in 2019?",
            "why did the brief say I have three meetings?",
            "add prep time to my calendar",
            "what meetings do I have next Tuesday?",
            "what is on my calendar for the whole of next month?",
            "summarise my brief and email it to Ana",
            "delete my work log",
            "is /brief a command you support?",
            "my briefcase is missing",
            "unblock me",
        ] {
            assert_eq!(route(asked), None, "{asked} must reach the model");
        }
    }

    #[test]
    fn refuses_a_command_that_is_only_part_of_a_word() {
        assert_eq!(route("/briefing"), None);
        assert_eq!(route("/logout"), None);
    }

    #[test]
    fn refuses_an_empty_question() {
        assert_eq!(route(""), None);
        assert_eq!(route("   "), None);
    }

    #[test]
    fn has_nothing_to_say_when_there_is_nothing_logged() {
        assert_eq!(bullets("Recently logged", &[]), None);
    }

    #[test]
    fn writes_one_bullet_per_line_under_a_heading() {
        let written = bullets(
            "Today",
            &["09:30 Standup".to_string(), "16:00 Retro".to_string()],
        );

        assert_eq!(
            written.as_deref(),
            Some("## Today\n- 09:30 Standup\n- 16:00 Retro")
        );
    }
}

#[cfg(test)]
mod delivery {
    use super::*;
    use crate::corpus::Corpus;
    use crate::db::test_support::migrated_pool;
    use crate::llama::test_support::serve;
    use crate::{github, llama, microsoft};

    struct Scratch(std::path::PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A context whose engine is a server that will answer nothing at all.
    ///
    /// `serve` with no replies accepts no connections and hands back what it
    /// received, so the empty list it returns *is* the assertion that routing
    /// never reached the model.
    async fn silent(
        name: &str,
    ) -> (
        recipe::Context,
        Scratch,
        tokio::task::JoinHandle<Vec<String>>,
    ) {
        let (host, server) = serve(Vec::<(&str, &str)>::new());
        let pool = migrated_pool().await;
        let root = std::env::temp_dir().join(format!("chief-intent-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        let corpus = Corpus::at(root.clone());
        corpus.ensure_shape().await.expect("should create");

        let context = recipe::Context {
            pool,
            github: github::Client::against("127.0.0.1:1").expect("client"),
            microsoft: microsoft::Client::against("127.0.0.1:1").expect("client"),
            calendar: crate::calendar::Client::new().expect("client"),
            linear: crate::linear::Client::new().expect("client"),
            engine: llama::Client::with_base_url(&host).expect("client"),
            corpus,
        };

        (context, Scratch(root), server)
    }

    fn logged(content: &str) -> work_log::NewWorkLogEntry {
        work_log::NewWorkLogEntry {
            source: "github".to_string(),
            content: content.to_string(),
            summary: Some("Shipped the release workflow".to_string()),
            timestamp: None,
            account_id: None,
            external_id: None,
        }
    }

    #[tokio::test]
    async fn answers_a_routed_question_without_asking_the_model() {
        let (context, _scratch, server) = silent("log").await;
        work_log::insert(&context.pool, logged("Merged #43"))
            .await
            .expect("should insert");

        let mut updates = Vec::new();
        let answered = deliver("/log", &context, |update| updates.push(update)).await;

        assert!(answered.is_some(), "/log should be answered here");
        assert_eq!(
            updates,
            vec![Update::Delta {
                text: "## Recently logged\n- Shipped the release workflow".to_string()
            }]
        );
        assert_eq!(
            server.await.expect("server"),
            Vec::<String>::new(),
            "a routed intent must cost zero model calls"
        );
    }

    #[tokio::test]
    async fn reads_todays_brief_rather_than_writing_one() {
        let (context, _scratch, server) = silent("brief").await;
        let path = format!("briefs/{}.md", recipe::today_date());
        context
            .corpus
            .write(&path, "- Standup at 09:30")
            .await
            .expect("should write");

        let answered = deliver("/brief", &context, |_| {}).await;

        assert_eq!(answered.as_deref(), Some("- Standup at 09:30"));
        assert_eq!(
            server.await.expect("server"),
            Vec::<String>::new(),
            "reading a brief must not regenerate it"
        );
    }

    #[tokio::test]
    async fn leaves_an_unrecognised_question_to_the_tool_loop() {
        let (context, _scratch, _server) = silent("miss").await;

        let mut updates = Vec::new();
        let answered = deliver("write a haiku", &context, |update| updates.push(update)).await;

        assert_eq!(answered, None, "the model should still get this");
        assert!(updates.is_empty(), "and nothing should have been shown");
    }

    #[tokio::test]
    async fn steps_aside_rather_than_answering_that_there_is_nothing() {
        let (context, _scratch, _server) = silent("empty").await;

        let answered = deliver("/log", &context, |_| {}).await;

        assert_eq!(
            answered, None,
            "an empty log reads as a misunderstanding, so the model gets the question"
        );
    }

    #[tokio::test]
    async fn steps_aside_when_no_brief_has_been_written_today() {
        let (context, _scratch, _server) = silent("nobrief").await;

        assert_eq!(deliver("/brief", &context, |_| {}).await, None);
    }
}
