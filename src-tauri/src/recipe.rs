//! Deterministic work, then a few sentences from the model.
//!
//! Small models are poor at planning and good at writing. So a recipe runs in
//! three phases and the model is only present for the last of them:
//!
//! 1. **Gather** — plain Rust. Read the calendar, the pull requests, the work
//!    log, the corpus. No model, no planning, no tool calls, nothing that can
//!    decide to do something else.
//! 2. **Assemble** — build one prompt, through a [`crate::context::Budget`] that
//!    refuses to go over rather than letting the engine truncate.
//! 3. **Render** — one short call per part of the brief, no tools. The parts and
//!    their headings are Rust's; the model words a few lines at a time.
//!
//! That third rule is the one worth defending. An agent loop asked to compose a
//! brief will call a tool, narrate, call another, and arrive somewhere unrepeatable
//! forty seconds later. The same brief assembled in Rust and written part by part
//! is the same brief every morning, and the only part that varies is the prose.
//! It used to be one call for the whole day, which meant a small model chose
//! five things to say and the rest of the day went unmentioned; a brief is now
//! complete, and several short calls are how a small model writes a long list
//! without being cut off or dropping lines.
//!
//! There is one recipe. The shape above is documented rather than abstracted
//! into a trait, for the reason [`crate::oauth::Provider`] gives about flows: an
//! abstraction drawn from one example is a guess. Meeting prep is the second,
//! and that is when the shared parts will be obvious rather than imagined.

use serde::Serialize;
use sqlx::SqlitePool;

use crate::context::{self, Budget};
use crate::corpus::Corpus;
use crate::llama::{self, ChatRequest, Message, Options};
use crate::microsoft;
use crate::retrieval;
use crate::session::{GithubSession, OutlookSession};
use crate::{check, clock, corpus, github, integrations, sections};

/// How long a brief may run to. Long enough for a handful of bullets and their
/// context, short enough that a slow machine finishes it.
const BRIEF_TOKENS: u32 = 500;

/// Pushes the model off lines it has already written. Measured need: Llama 3.2
/// 1B at temperature 0.2 listed the same four pull requests until it hit the
/// length limit. Mild, because a brief legitimately reuses names: on the 1B,
/// 80 runs on an 8 GB Intel Mac kept 83% of the meeting times and pull request
/// numbers in the prompt with no penalty, 86% at 1.05, 64% at 1.10 and 53% at
/// 1.15, where it also dropped meetings and invented some. Qwen3 1.7B never
/// looped in 40 runs. `break_loop` is the real guard; this is only the nudge.
const BRIEF_REPEAT_PENALTY: f32 = 1.05;

/// How many of anything is gathered for one brief, from each source.
///
/// Every line gathered is in the brief. If a source returns exactly this many
/// it may have had more, and the brief says so in words rather than passing the
/// newest few off as everything.
pub(crate) const PER_SOURCE: u8 = 25;

/// How many meetings are gathered for one day.
///
/// **A calendar is the one source that is not a queue.** A review request or an
/// unread message can wait and the newest are the ones worth having; a meeting
/// happens at its time, and a manager's day of back-to-back calls is routinely
/// more than ten. Graph returns them in start order, so a low cap used to cut
/// off the whole afternoon without a word.
pub(crate) const CALENDAR_LIMIT: u8 = 20;

/// A day with this many lines or fewer is written in code, not by the model.
pub const SHORT_BRIEF_LINES: usize = 3;

/// A meeting that began this recently is still the one being sat in, and stays
/// in the brief. Events carry no end time here, so this stands in for one.
const MEETING_RUNS: chrono::Duration = chrono::Duration::minutes(30);

/// Where the corpus keeps files that are loaded whatever the question.
///
/// The blueprint's "knowledge agents", and the reason that folder is separate:
/// everything under it is context Chief should have in hand before it is asked,
/// rather than something to go and find.
const AGENTS: &str = "context/agents/";

/// What a recipe needs. Passed explicitly rather than reached for through the
/// Tauri app, so a whole brief runs in a test against an in-memory database and
/// stub servers — the pattern `daemon::run_once` already establishes.
pub struct Context {
    pub pool: SqlitePool,
    pub github: github::Client,
    pub microsoft: microsoft::Client,
    /// Calendars subscribed to by URL, which need no sign-in at all.
    pub calendar: crate::calendar::Client,
    /// Linear, read with a pasted key rather than an OAuth grant.
    pub linear: crate::linear::Client,
    /// Jira and Confluence, over Atlassian's Remote MCP server.
    pub atlassian: crate::atlassian::Client,
    /// The same two, with an API token, for the organisations that restrict
    /// the MCP server.
    pub atlassian_rest: crate::atlassian::rest::Rest,
    pub engine: llama::Client,
    pub corpus: Corpus,
    /// A day with this many lines or fewer is written in code without asking a
    /// model; [`SHORT_BRIEF_LINES`] outside tests. A field rather than the
    /// constant so the tests that read the prompt can still reach the model.
    pub short_brief_lines: usize,
}

/// The sources the readers below need, and nothing else.
///
/// Implemented by this module's [`Context`] and by `tools::Context`, so the
/// brief and the questions about "my work" read Linear, Jira and the
/// subscribed calendars through the same code. A second copy of the Atlassian
/// route choice, which already has a bug behind it, is not worth having.
pub(crate) trait Reach {
    fn pool(&self) -> &SqlitePool;
    fn calendar(&self) -> &crate::calendar::Client;
    fn linear(&self) -> &crate::linear::Client;
    fn atlassian(&self) -> &crate::atlassian::Client;
    fn atlassian_rest(&self) -> &crate::atlassian::rest::Rest;
}

impl Reach for Context {
    fn pool(&self) -> &SqlitePool {
        &self.pool
    }
    fn calendar(&self) -> &crate::calendar::Client {
        &self.calendar
    }
    fn linear(&self) -> &crate::linear::Client {
        &self.linear
    }
    fn atlassian(&self) -> &crate::atlassian::Client {
        &self.atlassian
    }
    fn atlassian_rest(&self) -> &crate::atlassian::rest::Rest {
        &self.atlassian_rest
    }
}

/// What can go wrong making a brief.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("nothing is connected yet, so there is nothing to brief you on")]
    NothingToSay,
    /// Accounts are connected and not one of them could be read. Said apart
    /// from [`Error::NothingToSay`], which would send somebody who is on a
    /// train to set up what they already have.
    #[error(
        "Chief could not reach anything it is connected to, so there is nothing to brief you \
         on yet. Check the network connection; it will try again."
    )]
    NothingReachable,
    #[error(
        "today's brief has been edited since Chief wrote it, so it has been left alone. \
         Delete or rename {path} to have a fresh one written."
    )]
    EditedByHand { path: String },
    #[error(transparent)]
    Budget(#[from] context::Error),
    #[error(transparent)]
    Engine(#[from] llama::Error),
    #[error(transparent)]
    Corpus(#[from] corpus::Error),
    #[error(transparent)]
    Storage(#[from] crate::db::Error),
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// A brief, once it has been written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Brief {
    /// The day it covers, `YYYY-MM-DD`.
    pub date: String,
    /// Where it was written in the corpus.
    pub path: String,
    /// The brief itself.
    pub markdown: String,
    /// Which sources had anything to say, for the screen and for diagnosis.
    pub sources: Vec<String>,
    /// What checking the model's answer against the material found. Never sent
    /// to the window; the benchmark reads it to count retries and fallbacks.
    #[serde(skip)]
    pub checked: Checked,
}

/// What [`check::unsupported_by`] made of the model's answer to a brief.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checked {
    /// What the answer said that the material does not: times, numbers and
    /// names. Empty when it said nothing of the kind, or when no model was
    /// asked at all.
    pub first: Vec<String>,
    /// Whether the brief is the one Chief wrote from the material itself,
    /// because the model's could not be trusted.
    pub fell_back: bool,
    /// Whether there was so little to say that the brief was written from the
    /// material without asking a model; see [`SHORT_BRIEF_LINES`].
    pub by_code: bool,
}

/// Everything phase one found, before a model has seen any of it.
#[derive(Debug, Default)]
struct Gathered {
    agenda: Vec<String>,
    /// Things somebody else is blocked on: a review requested of the user.
    /// Until REC-41 this held the user's *own* open pull requests under a
    /// heading that said "waiting on you", which is a different question and
    /// the wrong answer to it.
    waiting: Vec<String>,
    /// The user's own open pull requests. Their work in flight, not a request.
    mine: Vec<String>,
    /// Issues assigned to the user on GitHub. Their own queue, like Linear's
    /// and Jira's, and rendered under the same heading — but its own field, so
    /// a brief built from GitHub alone does not report its source as Linear.
    issues: Vec<String>,
    shipped: Vec<String>,
    inbox: Vec<String>,
    /// What Linear says is assigned and unfinished — the one question the
    /// other sources cannot answer, because they hold the *outputs* of work.
    assigned: Vec<String>,
    /// The same question, asked of Jira.
    ///
    /// **Kept apart from [`Gathered::assigned`] only so [`Gathered::sources`]
    /// can name which tracker answered.** They share a heading in the prompt —
    /// a brief listing "Assigned in Linear" and "Assigned in Jira" separately
    /// would make the reader do the merge Chief is for — but a brief built
    /// from Jira alone must not report its source as Linear, which is what one
    /// merged field would have said.
    jira: Vec<String>,
    /// Confluence pages this person has been writing.
    ///
    /// Reachable only with an API token: the MCP path does not request
    /// Confluence's scopes, because it would be asking for access no code
    /// path used. The token carries whatever its owner already has, so there
    /// is nothing extra to ask for and the read simply works.
    pages: Vec<String>,
    background: Vec<(String, String)>,
}

impl Gathered {
    /// Which sources had anything at all.
    fn sources(&self) -> Vec<String> {
        let mut named = Vec::new();

        for (name, empty) in [
            ("calendar", self.agenda.is_empty()),
            ("review requests", self.waiting.is_empty()),
            ("pull requests", self.mine.is_empty()),
            ("work log", self.shipped.is_empty()),
            ("inbox", self.inbox.is_empty()),
            ("GitHub issues", self.issues.is_empty()),
            ("Linear", self.assigned.is_empty()),
            ("Jira", self.jira.is_empty()),
            ("Confluence", self.pages.is_empty()),
            ("corpus", self.background.is_empty()),
        ] {
            if !empty {
                named.push(name.to_string());
            }
        }

        named
    }

    fn is_empty(&self) -> bool {
        self.sources().is_empty()
    }
}

/// Phase one: read everything, decide nothing.
///
/// Every source is optional and a source that fails is skipped rather than
/// fatal — a brief with a calendar and no mail is worth having, and an Outlook
/// token that expired overnight should not cost the user their morning.
async fn gather(context: &Context, now: chrono::DateTime<chrono::Local>) -> Gathered {
    let mut found = Gathered::default();

    let (from, to) = today_at(&now);

    // Subscriptions first: they need no sign-in, so on most machines this is
    // the only calendar there is. Merged into the same agenda as Outlook, so
    // the brief never has to know which kind an entry came from.
    found.agenda.extend(
        subscribed_events_at(context, now)
            .await
            .iter()
            .map(describe_event),
    );

    for account in outlook_accounts(context).await {
        let session = OutlookSession::new(&context.pool, &context.microsoft, account);

        if let Ok(events) = session.events(&from, &to, CALENDAR_LIMIT).await {
            found.agenda.extend(events.iter().map(describe_event));
        }

        if let Ok(messages) = session.messages(PER_SOURCE).await {
            found.inbox.extend(
                messages
                    .iter()
                    .filter(|one| one.unread)
                    .map(describe_message),
            );
        }
    }

    for account in github_accounts(context).await {
        let session = GithubSession::new(&context.pool, &context.github, account);

        // The user's own work in flight.
        if let Ok(open) = session
            .pull_requests(
                github::Involvement::Authored,
                github::State::Open,
                PER_SOURCE,
            )
            .await
        {
            found.mine.extend(open.iter().map(describe_pull_request));
        }

        // What somebody else is actually blocked on. Each source fails on its
        // own, so a rate-limited search costs one bucket rather than the brief.
        if let Ok(reviewing) = session
            .pull_requests(
                github::Involvement::Reviewing,
                github::State::Open,
                PER_SOURCE,
            )
            .await
        {
            found
                .waiting
                .extend(reviewing.iter().map(describe_pull_request));
        }

        if let Ok(issues) = session.assigned_issues(PER_SOURCE).await {
            found.issues.extend(issues.iter().map(describe_issue));
        }
    }

    // One agenda, in the order the day happens. Each calendar is in order
    // already, but they are read one after another, so a family calendar read
    // before an Outlook one would put the 17:30 parents' evening above the
    // 09:00 stand-up. The same meeting on two calendars is one line, and the
    // list is held to a day's worth so that one very full calendar cannot push
    // the whole block over the prompt budget, which would drop it entirely.
    tidy_agenda(&mut found.agenda);

    // **Through `retrieval`, which knows what a row says.** This read
    // `work_log::fetch` and took `summary`, which was a model-written sentence
    // until deterministic ingestion made it a state word — after which every
    // bullet of the brief's material was the single word "merged". The model
    // was told to name pull requests exactly as they appear and given nothing
    // to name, so it invented them. `retrieval::line` is the same renderer the
    // chat answers use, so a logged item reads the same wherever it appears.
    //
    // **Every logged row, not one category.** An entry the user typed by hand
    // carries the default category and belongs under this heading as much as a
    // merged pull request does; filtering to shipped rows would have dropped
    // it silently. Which rows a brief should carry is a question about the
    // brief rather than about this defect, and REC-9 is where it belongs.
    if let Ok(hits) = retrieval::latest(&context.pool, PER_SOURCE.into()).await {
        found
            .shipped
            .extend(hits.iter().filter_map(retrieval::line));
    }

    found.assigned = assigned_issues(context).await;
    let (jira, pages) = atlassian(context).await;
    found.jira = jira;
    found.pages = pages;
    found.background = background(context).await;

    found
}

/// The time an agenda line starts. `None` for an all-day line or one with no
/// time on it.
fn starts_at(line: &str) -> Option<chrono::NaiveTime> {
    chrono::NaiveTime::parse_from_str(line.get(0..5)?, "%H:%M").ok()
}

/// The meetings still worth listing: everything lasting the day, and every
/// timed meeting that has not finished.
///
/// Pure, and given the clock, so "at 14:10 the morning has gone" is tested
/// without waiting for it. `agenda` must already be in the order the day
/// happens, as [`tidy_agenda`] leaves it. **Nothing else is left out**: a
/// meeting that has finished is no longer news, but one still to come is
/// listed however many there are.
pub(crate) fn still_to_come(
    agenda: Vec<String>,
    now: &chrono::DateTime<chrono::Local>,
) -> Vec<String> {
    // Subtracting from a time wraps past midnight, which would put 09:00 in the
    // past at 00:01. The day starts at midnight, so the cutoff cannot go before it.
    let cutoff = now
        .time()
        .signed_duration_since(chrono::NaiveTime::MIN)
        .checked_sub(&MEETING_RUNS)
        .map_or(chrono::NaiveTime::MIN, |since_midnight| {
            chrono::NaiveTime::MIN + since_midnight.max(chrono::Duration::zero())
        });

    // An event with no time lasts the day and is always in.
    agenda
        .into_iter()
        .filter(|line| starts_at(line).map_or(true, |start| start >= cutoff))
        .collect()
}

/// Put an agenda in the order the day happens, without repeats, and no longer
/// than [`CALENDAR_LIMIT`]. Lines start with the time, so text order is time
/// order; a day-long event has no time and goes first.
pub(crate) fn tidy_agenda(agenda: &mut Vec<String>) {
    agenda.sort_by_key(|line| (!line.starts_with(ALL_DAY), line.clone()));
    agenda.dedup();
    agenda.truncate(usize::from(CALENDAR_LIMIT));
}

/// What stands in for a time on an event that lasts the day.
const ALL_DAY: &str = "all day";

/// Today's events from every calendar the user subscribed to.
///
/// A subscription that will not load fails that one calendar and nothing else,
/// the same rule every other source in `gather` follows: a brief with the
/// calendar and no mail is worth having.
pub(crate) async fn subscribed_events_at(
    context: &impl Reach,
    now: chrono::DateTime<chrono::Local>,
) -> Vec<microsoft::Event> {
    read_subscriptions_at(context, now)
        .await
        .into_iter()
        .flat_map(|(_, read)| read.unwrap_or_default())
        .collect()
}

/// Each subscribed calendar's day, with `None` for one that would not load.
///
/// The difference between "nothing today" and "could not be read" is what lets
/// a caller that keeps a copy decide whether to trust an empty list.
pub(crate) async fn read_subscriptions_at(
    context: &impl Reach,
    now: chrono::DateTime<chrono::Local>,
) -> Vec<(i64, Option<Vec<microsoft::Event>>)> {
    let Ok(accounts) =
        crate::integrations::accounts(context.pool(), crate::integrations::CALENDAR).await
    else {
        return Vec::new();
    };

    let offset = now.offset().local_minus_utc();
    let midnight = now.date_naive().and_time(chrono::NaiveTime::MIN);
    let end = midnight + chrono::Duration::days(1);

    let mut found = Vec::new();

    for account in accounts {
        let Ok(Some(credentials)) =
            crate::integrations::credentials(context.pool(), account.id).await
        else {
            continue;
        };

        match context
            .calendar()
            .events(&credentials.access_token, midnight, end, offset)
            .await
        {
            Ok(events) => found.push((account.id, Some(events))),
            // Reported without the address, which is a credential.
            Err(error) => {
                eprintln!("a calendar subscription could not be read: {error}");
                found.push((account.id, None));
            }
        }
    }

    found
}

/// What every connected Linear workspace says is assigned and unfinished.
///
/// A workspace that will not answer fails that one source, the same rule every
/// other source in `gather` follows.
async fn assigned_issues(context: &Context) -> Vec<String> {
    let Ok(accounts) =
        crate::integrations::accounts(&context.pool, crate::integrations::LINEAR).await
    else {
        return Vec::new();
    };

    let mut found = Vec::new();

    for account in accounts {
        let Ok(Some(credentials)) =
            crate::integrations::credentials(&context.pool, account.id).await
        else {
            continue;
        };

        match context.linear.assigned(&credentials.access_token).await {
            Ok(assigned) => found.extend(assigned.issues.iter().map(crate::linear::describe)),
            // Reported without the key, which is a credential.
            Err(error) => eprintln!("a Linear workspace could not be read: {error}"),
        }
    }

    found
}

/// What every connected Atlassian account holds, by whichever route it was
/// connected.
///
/// **The credential decides the transport, not a setting.** An account
/// connected through the MCP server carries a client minted for it and reads
/// over JSON-RPC; one connected with a pasted token carries the token and
/// reads Atlassian's REST API directly. `credential_kind` says which, which is
/// the column's whole purpose — "explicit, so a pasted credential is not
/// inferred from which columns happen to be NULL".
///
/// A site that will not answer fails that one source, the same rule every
/// other source in `gather` follows.
/// Where one account's reads go, decided from what it stored.
///
/// Pure, and separate from the loop, because a branch inside an `async fn`
/// that reaches two servers is provable only by standing both of them up —
/// and one of those is an MCP server. `Error::from_transport` was pulled out
/// of `atlassian.rs` for the same reason: a test for the matcher and a test
/// for the wording, with nothing joining them, left the branch itself free to
/// be deleted.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// Atlassian's REST API, with HTTP Basic over the pasted token.
    Rest { site: String, email: String },
    /// The MCP server, over the client this account registered for itself.
    Mcp,
}

pub(crate) fn route(
    account: &integrations::Account,
    credentials: &integrations::Credentials,
) -> Route {
    if credentials.kind != crate::integrations::API_KEY {
        return Route::Mcp;
    }

    Route::Rest {
        // `account_key` holds the bare host, so the scheme is put back rather
        // than stored — there is no other scheme this may be.
        site: format!("https://{}", account.account_key),
        // Basic auth is `email:token`, and for this provider `identity` is
        // documented as what to send as the user half.
        email: account.identity.clone().unwrap_or_default(),
    }
}

async fn atlassian(context: &Context) -> (Vec<String>, Vec<String>) {
    let Ok(accounts) =
        crate::integrations::accounts(&context.pool, crate::integrations::ATLASSIAN).await
    else {
        return (Vec::new(), Vec::new());
    };

    let mut issues = Vec::new();
    let mut pages = Vec::new();

    for account in accounts {
        let Ok(Some(credentials)) =
            crate::integrations::credentials(&context.pool, account.id).await
        else {
            continue;
        };

        let Route::Rest { site, email } = route(&account, &credentials) else {
            let session = crate::session::AtlassianSession::new(
                &context.pool,
                &context.atlassian,
                account.id,
            );

            match session.assigned().await {
                Ok(found) => issues.extend(found.iter().map(crate::atlassian::describe)),
                Err(error) => eprintln!("an Atlassian account could not be read: {error}"),
            }

            continue;
        };

        {
            match context
                .atlassian_rest
                .assigned(&site, &email, &credentials.access_token)
                .await
            {
                Ok(found) => issues.extend(found.iter().map(crate::atlassian::describe)),
                // Reported without the token, which is a credential.
                Err(error) => eprintln!("an Atlassian site could not be read: {error}"),
            }

            match context
                .atlassian_rest
                .pages(&site, &email, &credentials.access_token)
                .await
            {
                Ok(found) => pages.extend(found.iter().map(crate::atlassian::rest::describe)),
                // Confluence is a separate product and a separate permission:
                // a token that reads Jira may legitimately not reach it, and
                // that must not cost the Jira half of the same account.
                Err(error) => {
                    eprintln!("an Atlassian site's Confluence could not be read: {error}")
                }
            }
        }
    }

    (issues, pages)
}

/// The always-loaded corpus files, newest first.
///
/// Read in full and slimmed, because these are short files written for exactly
/// this purpose. What keeps them from overrunning anything is the budget in
/// phase two, not a limit here.
async fn background(context: &Context) -> Vec<(String, String)> {
    let Ok(entries) = corpus::indexed(&context.pool).await else {
        return Vec::new();
    };

    let mut loaded = Vec::new();

    for entry in entries
        .iter()
        .filter(|entry| entry.path.starts_with(AGENTS))
    {
        if let Ok(text) = context.corpus.read(&entry.path).await {
            let slimmed = context::slim(&text);

            if !slimmed.is_empty() {
                loaded.push((entry.path.clone(), slimmed));
            }
        }
    }

    loaded
}

impl Gathered {
    /// The brief's parts, in the order they are written: what is time-bound
    /// first, then what other people are waiting on, then the rest.
    fn sections(&self) -> Vec<(&'static str, Vec<String>)> {
        // The trackers share a heading, for the reason `Gathered::jira` gives.
        let tracked = [
            self.issues.as_slice(),
            self.assigned.as_slice(),
            self.jira.as_slice(),
        ]
        .concat();

        [
            ("Today's meetings", self.agenda.clone()),
            (
                "Waiting on you — somebody has asked for your review",
                self.waiting.clone(),
            ),
            ("Your open pull requests", self.mine.clone()),
            ("Assigned to you and not finished", tracked),
            ("Unread mail", self.inbox.clone()),
            ("Pages you have been writing", self.pages.clone()),
            ("Recently logged work", self.shipped.clone()),
        ]
        .into_iter()
        .filter(|(_, lines)| !lines.is_empty())
        .collect()
    }

    /// The parts that came back as full as a read can be, so there may be more
    /// behind them. Said in the brief in words, because the alternative is a
    /// list that looks complete and is not.
    fn possibly_more(&self) -> Vec<&'static str> {
        let full = usize::from(PER_SOURCE);

        [
            ("meetings", self.agenda.len() >= usize::from(CALENDAR_LIMIT)),
            ("review requests", self.waiting.len() >= full),
            ("open pull requests", self.mine.len() >= full),
            ("GitHub issues", self.issues.len() >= full),
            ("Linear issues", self.assigned.len() >= full),
            ("Jira issues", self.jira.len() >= full),
            ("unread mail", self.inbox.len() >= full),
        ]
        .into_iter()
        .filter_map(|(label, full)| full.then_some(label))
        .collect()
    }

    /// Every line, in the order they are written.
    fn every_line(&self) -> usize {
        self.sections().iter().map(|(_, lines)| lines.len()).sum()
    }
}

/// What the model is asked to do with one part of the day.
///
/// Rigidly shaped on purpose. A 3B model given "write a brief" writes an essay;
/// given a few lines, a named order and nothing else to say, it writes bullets.
const INSTRUCTIONS: &str = "\
You are Chief, an AI chief of staff. Write one part of the user's brief for today \
from the lines below, and from nothing else.

Rules:
- One short bullet per line given, in the order given, each starting with a dash.
- Name people, pull requests, tickets and times exactly as they appear below.
- State only what the lines say. Do not guess, do not invent numbers, and do not \
add or leave out anything.
- No preamble, no sign-off, no headings. Bullets only.";

/// The instructions, the clock and the background: what every request of one
/// brief starts with.
///
/// **First and unchanged from one request to the next**, because llama.cpp
/// reuses the cached prefix of a prompt it has already seen, so each part after
/// the first pays prefill only for its own lines. Built through a
/// [`Budget`], so background that does not fit is skipped rather than ending
/// the brief — the calendar matters more than the writing-style notes.
///
/// `present` is the clock, or nothing when the same text is wanted without it.
fn prefix(found: &Gathered, present: &str) -> Result<String, Error> {
    let mut budget = Budget::new();
    let mut prompt = String::new();

    let mut push = |budget: &mut Budget, name: &str, block: &str| {
        if budget.add(name, block).is_ok() {
            prompt.push_str(block);
            prompt.push_str("\n\n");
        }
    };

    push(&mut budget, "instructions", INSTRUCTIONS);
    push(&mut budget, "clock", present);

    for (path, text) in &found.background {
        push(&mut budget, path, &format!("## {path}\n{text}"));
    }

    Ok(prompt)
}

/// One request: the prefix, then the part's own lines.
fn part_prompt(prefix: &str, heading: &str, lines: &str) -> String {
    let lines = lines
        .lines()
        .map(|line| format!("- {line}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!("{prefix}## {heading}\n{lines}")
}

/// Phase three: a short call for each part of the day, no tools.
///
/// **Not streamed to anyone, but streamed.** The engine client bounds a request
/// by silence rather than by a deadline, precisely so a slow answer is not cut
/// off — but on a non-streamed request there is nothing to hear until the whole
/// answer is ready, so the inactivity timeout becomes a deadline for the entire
/// generation. That is the failure `SILENCE_TIMEOUT` was written to avoid, and
/// it is why `sections::write` streams even though nobody is watching. See
/// REC-65.
fn request(prompt: String) -> ChatRequest {
    ChatRequest::new(crate::agent::DEFAULT_MODEL, vec![Message::user(prompt)]).with_options(
        Options::new()
            .with_answer_length(BRIEF_TOKENS)
            .with_repeat_penalty(BRIEF_REPEAT_PENALTY),
    )
}

/// The brief, written a part at a time.
///
/// **Whatever the model says is checked against what it was given.** A time, a
/// number or a name that is not in the lines is something it made up, and a
/// brief with one invented meeting in it is worse than a plainer brief, because
/// the reader plans a morning round it. So a part whose answer names something
/// the lines do not is not used: that part is written from its lines directly,
/// with no model in it.
///
/// **There is no second attempt.** The first version asked the model again,
/// telling it what to leave out. Of ten answers that really had invented
/// something, none came back clean (Llama 3.2 3B and Qwen3 1.7B, five repeats
/// each), and each retry cost another full generation on a machine where that
/// is the expensive part.
///
/// **Nothing is left out.** A part is asked about eight lines at a time so that
/// no request can run into the length limit, and any line the answer does not
/// mention is added back as it was found. What this cannot see is an invented
/// claim in ordinary words ("all staff must attend"); see [`check`] for what it
/// does and does not look for.
async fn write_checked(
    engine: &llama::Client,
    short_brief_lines: usize,
    found: &Gathered,
    present: &str,
) -> Result<(String, Checked), Error> {
    let with_clock = prefix(found, present)?;
    let without_clock = prefix(found, "")?;
    let by_code = found.every_line() <= short_brief_lines;

    // A part of a line is its own list: the model could only reword it, and
    // every rewording measured so far that went wrong did so on a short one
    // ("A meeting is scheduled" for a school notice). A day this short is
    // written whole in code, and asks no model.
    let sections: Vec<sections::Section> = found
        .sections()
        .into_iter()
        .map(|(heading, lines)| sections::Section {
            worded: !by_code && lines.len() > 1,
            heading: heading.to_string(),
            lines,
        })
        .collect();

    let written = sections::write(
        engine,
        &sections,
        |heading| format!("## {heading}"),
        |heading, lines| part_prompt(&with_clock, heading, lines),
        request,
        |answer, lines| {
            let items = lines.join("\n");
            // The prompt without its clock: a time must be somebody's, not the
            // moment the brief happened to be written.
            let shown = part_prompt(&without_clock, "", &items);
            let words = shown.strip_prefix(INSTRUCTIONS).unwrap_or(&shown);

            check::unsupported_by(
                answer,
                &check::Material {
                    shown: &shown,
                    items: &items,
                    words,
                },
            )
        },
        |_| {},
    )
    .await?;

    let checked = Checked {
        first: written.refused.clone(),
        fell_back: !written.refused.is_empty(),
        by_code: written.requests == 0,
    };

    Ok((written.text, checked))
}

/// Make today's brief and write it into the corpus.
///
/// Regenerating replaces: a brief is what today looks like now, not a history
/// of what it looked like at each point during it.
pub async fn daily_brief(context: &Context) -> Result<Brief, Error> {
    daily_brief_at(context, chrono::Local::now()).await
}

/// The brief for the day `now` falls in.
///
/// **The clock is read once, here, and everything below is worked out from it.**
/// A brief used to ask the clock four separate times — for the window it
/// gathered, for the date in the prompt, and for the file it was filed under,
/// which was read *after* the model had finished. A small model on a slow
/// machine takes minutes, so a brief started at 23:59 was written under
/// tomorrow's name, with yesterday's meetings in it, and the day it had been
/// filed under then counted as already briefed.
pub(crate) async fn daily_brief_at(
    context: &Context,
    now: chrono::DateTime<chrono::Local>,
) -> Result<Brief, Error> {
    let date = today_date_at(&now);
    let path = format!("briefs/{date}.md");

    // A brief the user has touched is theirs, and finding out costs nothing —
    // where finding out *after* the model has written one costs a minute on a
    // slow machine, every time the daemon comes round to refresh it.
    if edited_by_hand(context, &date, &path).await {
        return Err(Error::EditedByHand { path });
    }

    let mut found = gather(context, now).await;

    // Nothing connected and nothing logged is not a brief with no bullets, it
    // is a machine that has not been set up. Saying so beats asking a model to
    // write about an empty page.
    if found.is_empty() {
        // Nothing found is two different situations. Never set up, and set up
        // but unreachable — a laptop on a train — read the same here and need
        // opposite advice.
        let connected = integrations::all_accounts(&context.pool)
            .await
            .is_ok_and(|accounts| !accounts.is_empty());

        return Err(if connected {
            Error::NothingReachable
        } else {
            Error::NothingToSay
        });
    }

    // `today`, not `present`: the brief gets the clock and not the ranges.
    // See `clock::stamp_at` — a 1B model handed that date list returned it as the
    // bullets it had been asked for.
    //
    // **The sources are named before anything is cut**, so a calendar whose
    // meetings have all finished still counts as having answered.
    let sources = found.sources();
    let possibly_more = found.possibly_more();
    found.agenda = still_to_come(std::mem::take(&mut found.agenda), &now);

    let mut checked = Checked::default();
    let mut markdown = if found.is_empty() {
        // Everything on the day has been and gone. Nothing to ask a model.
        "- Nothing else is scheduled for today.".to_string()
    } else {
        let (written, outcome) = write_checked(
            &context.engine,
            context.short_brief_lines,
            &found,
            &clock::stamp_at(&now),
        )
        .await?;
        checked = outcome;
        written
    };

    // **Written here, not by the model**: a model asked to say what it was not
    // shown invents the number.
    if !possibly_more.is_empty() {
        markdown = format!(
            "{markdown}\n\nThere may be more than this: Chief read the newest {} of each, and \
             that many came back for {}.",
            PER_SOURCE,
            possibly_more.join(", ")
        );
    }

    // A brief the user has touched is theirs. The corpus is offered as a folder
    // they can edit, and regenerating used to overwrite it without a word —
    // measured, a hand-written section simply vanished. Checked here rather
    // than in the corpus, because only a brief knows when Chief last wrote it.
    if edited_by_hand(context, &date, &path).await {
        return Err(Error::EditedByHand { path });
    }

    context.corpus.write(&path, &markdown).await?;
    record(&context.pool, &date, &sources).await?;

    Ok(Brief {
        date,
        path,
        markdown,
        sources,
        checked,
    })
}

/// Has the file changed since Chief last wrote it?
///
/// Compared against the moment recorded in `briefs`, with a couple of seconds
/// of slack: writing the file and recording the row are two operations, and
/// their timestamps differ by a little even when nothing has touched it since.
async fn edited_by_hand(context: &Context, date: &str, path: &str) -> bool {
    let Ok(Some(written)) = written_at(&context.pool, date).await else {
        // Never written, so nothing of the user's to lose.
        return false;
    };

    let Some(modified) = context.corpus.modified_at(path).await else {
        return false;
    };

    let (Ok(written), Ok(modified)) = (
        chrono::DateTime::parse_from_rfc3339(&written),
        chrono::DateTime::parse_from_rfc3339(&modified),
    ) else {
        // Unreadable timestamps are not evidence of an edit, and refusing to
        // write on that basis would stop briefs entirely.
        return false;
    };

    modified - written > chrono::Duration::seconds(2)
}

/// Note that a brief was written, and from what.
async fn record(pool: &SqlitePool, date: &str, sources: &[String]) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO briefs (date, generated_at, sources)
         VALUES (?1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?2)
         ON CONFLICT (date) DO UPDATE
            SET generated_at = excluded.generated_at,
                sources = excluded.sources",
    )
    .bind(date)
    .bind(sources.join(", "))
    .execute(pool)
    .await
    .map_err(|error| Error::Storage(crate::db::Error::Sqlx(error)))?;

    Ok(())
}

/// When today's brief was last written, if it has been.
pub async fn written_at(pool: &SqlitePool, date: &str) -> Result<Option<String>, crate::db::Error> {
    sqlx::query_scalar("SELECT generated_at FROM briefs WHERE date = ?1")
        .bind(date)
        .fetch_optional(pool)
        .await
        .map_err(crate::db::Error::Sqlx)
}

/// Today, as the date a brief is filed under.
pub(crate) fn today_date() -> String {
    today_date_at(&chrono::Local::now())
}

pub(crate) fn today_date_at(now: &chrono::DateTime<chrono::Local>) -> String {
    now.format("%Y-%m-%d").to_string()
}

/// Midnight to midnight, in this machine's own time zone.
pub(crate) fn today_at(now: &chrono::DateTime<chrono::Local>) -> (String, String) {
    let midnight = now.date_naive().and_hms_opt(0, 0, 0).unwrap_or_default();

    (
        midnight.format("%Y-%m-%dT%H:%M:%S").to_string(),
        (midnight + chrono::Duration::days(1))
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string(),
    )
}

pub(crate) async fn github_accounts(context: &Context) -> Vec<i64> {
    integrations::accounts(&context.pool, integrations::GITHUB)
        .await
        .map(|accounts| accounts.iter().map(|account| account.id).collect())
        .unwrap_or_default()
}

pub(crate) async fn outlook_accounts(context: &Context) -> Vec<i64> {
    integrations::accounts(&context.pool, integrations::MICROSOFT)
        .await
        .map(|accounts| accounts.iter().map(|account| account.id).collect())
        .unwrap_or_default()
}

pub(crate) fn describe_event(event: &microsoft::Event) -> String {
    let when = if event.all_day {
        ALL_DAY
    } else {
        event
            .start
            .split('T')
            .nth(1)
            .unwrap_or("")
            .get(0..5)
            .unwrap_or("")
    };
    // An attendee the title already names is not named again: "1:1 with Priya"
    // with Priya invited read "1:1 with Priya with Priya" in every brief
    // written in code.
    let others: Vec<&str> = event
        .attendees
        .iter()
        .map(String::as_str)
        .filter(|name| !named_in(&event.subject, name))
        .collect();
    let who = if others.is_empty() {
        String::new()
    } else {
        format!(" with {}", others.join(", "))
    };

    format!("{when} {}{who}", event.subject).trim().to_string()
}

/// Whether `title` already says `name`, or any whole word of it ("Sam" for
/// "Sam Patel"), ignoring case. Whole words only, so "Ana" is not in "banana".
fn named_in(title: &str, name: &str) -> bool {
    let words: Vec<String> = title
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .collect();

    name.split_whitespace()
        .filter(|part| part.chars().count() > 1)
        .any(|part| words.contains(&part.to_lowercase()))
}

pub(crate) fn describe_message(message: &microsoft::MailMessage) -> String {
    let from = message.from.as_deref().unwrap_or("someone");

    format!("{from}: {}", message.subject)
}

fn describe_issue(issue: &github::Issue) -> String {
    format!("{} #{} — {}", issue.repository, issue.number, issue.title)
}

fn describe_pull_request(pull_request: &github::PullRequest) -> String {
    format!(
        "{} #{} — {}",
        pull_request.repository, pull_request.number, pull_request.title
    )
}

/// Assemble a recipe context from the running app.
pub async fn context<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<Context, Error> {
    use tauri::Manager;

    let pool = crate::db::pool(app).await?;
    let home = app
        .path()
        .home_dir()
        .map_err(|error| corpus::Error::Index(error.to_string()))?;

    Ok(Context {
        corpus: Corpus::at(corpus::root(&pool, &home).await?),
        pool,
        github: app.state::<github::Client>().inner().clone(),
        microsoft: app.state::<microsoft::Client>().inner().clone(),
        calendar: app.state::<crate::calendar::Client>().inner().clone(),
        linear: app.state::<crate::linear::Client>().inner().clone(),
        atlassian: app.state::<crate::atlassian::Client>().inner().clone(),
        atlassian_rest: app.state::<crate::atlassian::rest::Rest>().inner().clone(),
        engine: app.state::<llama::Client>().inner().clone(),
        short_brief_lines: SHORT_BRIEF_LINES,
    })
}

/// Write today's brief now.
///
/// Wakes the engine first, and holds the door while it runs: a brief takes a
/// model call, and the idle supervisor must not stop the engine underneath it.
#[tauri::command]
pub async fn generate_brief<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    engine: tauri::State<'_, crate::engine::Engine>,
    client: tauri::State<'_, llama::Client>,
    attention: tauri::State<'_, crate::agent::Attention>,
) -> Result<Brief, Error> {
    let _waiting = attention.begin();

    engine
        .start_and_wait(client.inner())
        .await
        .map_err(|error| Error::Engine(llama::Error::Transport(error.to_string())))?;

    let context = context(&app).await?;

    daily_brief(&context).await
}

/// Today's brief, if one has been written.
///
/// Reads the corpus rather than regenerating: opening the screen should not
/// cost a model call, and a brief the user has edited by hand is the one they
/// should see.
#[tauri::command]
pub async fn todays_brief<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Option<Brief>, Error> {
    let context = context(&app).await?;
    let date = today_date();
    let path = format!("briefs/{date}.md");

    match context.corpus.read(&path).await {
        Ok(markdown) => Ok(Some(Brief {
            checked: Checked::default(),
            date,
            path,
            markdown,
            sources: Vec::new(),
        })),
        Err(corpus::Error::NoSuchFile(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    fn at_local(stamp: &str) -> chrono::DateTime<chrono::Local> {
        use chrono::TimeZone;

        let naive =
            chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%dT%H:%M:%S").expect("stamp");

        chrono::Local
            .from_local_datetime(&naive)
            .single()
            .expect("an unambiguous local time")
    }

    /// **Nothing is cut.** A brief used to hold three of each queue and say
    /// "Not listed: 4 more waiting on your review"; the reader wanted the
    /// overview, not a count of what was withheld.
    ///
    /// Proved by truncating each part to three lines in `Gathered::sections`:
    ///
    /// ```text
    /// every review is listed
    /// ```
    #[test]
    fn a_long_queue_is_listed_in_full() {
        let found = Gathered {
            waiting: (0..7).map(|n| format!("repo #{n} — review")).collect(),
            mine: (0..4).map(|n| format!("repo #{n} — mine")).collect(),
            ..Gathered::default()
        };

        let sections = found.sections();
        let lines = |heading: &str| {
            sections
                .iter()
                .find(|(title, _)| title.starts_with(heading))
                .map(|(_, lines)| lines.len())
        };

        assert_eq!(lines("Waiting on you"), Some(7), "every review is listed");
        assert_eq!(lines("Your open pull requests"), Some(4));
    }

    /// A read that came back as full as it can be may have had more behind it,
    /// and the brief says so in words rather than looking complete.
    #[test]
    fn a_read_that_came_back_full_is_said_to_be_possibly_incomplete() {
        let found = Gathered {
            waiting: (0..usize::from(PER_SOURCE))
                .map(|n| format!("repo #{n} — review"))
                .collect(),
            mine: vec!["repo #1 — mine".to_string()],
            ..Gathered::default()
        };

        assert_eq!(found.possibly_more(), ["review requests"]);
        assert!(gathered().possibly_more().is_empty());
    }

    /// Every meeting still to come is listed however many there are; only one
    /// that has finished is left out.
    #[test]
    fn every_meeting_still_to_come_is_listed_and_only_finished_ones_are_not() {
        let agenda: Vec<String> = [
            "all day Bank holiday",
            "09:00 Standup",
            "11:00 Planning",
            "13:00 Review",
            "14:00 1:1",
            "15:00 Retro",
            "16:00 Demo",
        ]
        .map(String::from)
        .to_vec();

        let kept = still_to_come(agenda, &at_local("2026-10-06T13:20:00"));

        assert_eq!(
            kept,
            [
                "all day Bank holiday",
                "13:00 Review",
                "14:00 1:1",
                "15:00 Retro",
                "16:00 Demo"
            ],
            "a meeting that began twenty minutes ago is the one being sat in"
        );
    }

    /// Subtracting half an hour from 00:01 wraps to 23:31, which put the whole
    /// day in the past. ANY-02 caught it; this keeps it caught at the source.
    #[test]
    fn the_start_of_the_day_does_not_wrap_into_the_night_before() {
        let kept = still_to_come(
            vec!["09:00 Planning".to_string()],
            &at_local("2026-10-06T00:01:00"),
        );

        assert_eq!(kept, vec!["09:00 Planning".to_string()]);
    }

    use super::*;
    use crate::work_log;

    const PRESENT: &str = "The current date and time is 08:00 on Friday 28 August 2026.";

    fn gathered() -> Gathered {
        Gathered {
            agenda: vec!["10:00 1:1 with Sam".to_string()],
            waiting: vec!["scottmallinson/chief.ai #44 — Ana asked for your review".to_string()],
            mine: vec!["scottmallinson/chief.ai #12 — Add the corpus".to_string()],
            shipped: vec!["Shipped the loopback listener".to_string()],
            inbox: vec!["Dana Reid: Re: the migration".to_string()],
            issues: vec!["scottmallinson/chief.ai #3 — Slow start on Windows".to_string()],
            assigned: vec!["REC-42 Read Linear [In Progress]".to_string()],
            jira: vec!["PROJ-7 Migrate the tenant [In Review]".to_string()],
            pages: vec!["Tenant migration runbook [ENG]".to_string()],
            background: vec![(
                "context/agents/org/team_structure.md".to_string(),
                "# Team\nSam owns auth.".to_string(),
            )],
        }
    }

    /// Everything gathered is written somewhere, under a heading of its own.
    #[test]
    fn every_source_that_had_something_has_a_part() {
        let found = gathered();
        let written: String = found
            .sections()
            .iter()
            .flat_map(|(_, lines)| lines.clone())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(written.contains("1:1 with Sam"), "{written}");
        assert!(written.contains("#12"), "{written}");
        assert!(written.contains("Dana Reid"), "{written}");
        assert!(
            written.contains("Shipped the loopback listener"),
            "{written}"
        );
        assert!(written.contains("Tenant migration runbook"), "{written}");
        assert!(
            prefix(&found, PRESENT)
                .expect("a prefix")
                .contains("Sam owns auth."),
            "the background rides in every request"
        );
    }

    #[test]
    fn a_source_with_nothing_in_it_takes_no_part_at_all() {
        let mut sparse = gathered();
        sparse.inbox.clear();
        sparse.waiting.clear();

        let headings: Vec<&str> = sparse.sections().iter().map(|(title, _)| *title).collect();

        assert!(!headings.contains(&"Unread mail"), "{headings:?}");
        assert!(
            !headings
                .iter()
                .any(|title| title.starts_with("Waiting on you")),
            "{headings:?}"
        );
    }

    #[test]
    fn the_stable_material_comes_first_so_the_prefix_can_be_reused() {
        // llama.cpp reuses the cached prefix of a prompt it has seen. The
        // instructions never change and the lines change hourly, so this order
        // is what makes every part after the first, and a regenerated brief,
        // cheap.
        let found = gathered();
        let head = prefix(&found, PRESENT).expect("a prefix");
        let prompt = part_prompt(&head, "Today's meetings", "10:00 1:1 with Sam");

        assert!(prompt.starts_with(&head), "{prompt}");

        let instructions = prompt.find("You are Chief").expect("instructions");
        let background = prompt.find("Sam owns auth").expect("background");
        let lines = prompt.find("1:1 with Sam").expect("the part's own lines");

        assert!(instructions < background, "{prompt}");
        assert!(background < lines, "{prompt}");
    }

    #[test]
    fn the_prefix_stays_inside_the_budget_however_much_background_there_is() {
        let mut flood = gathered();
        flood.background = (0..50)
            .map(|n| (format!("context/agents/{n}.md"), "word ".repeat(2000)))
            .collect();

        let head = prefix(&flood, PRESENT).expect("an over-full gather still builds a prefix");

        assert!(
            context::estimate_tokens(&head) <= context::DEFAULT_CEILING,
            "the prefix is {} tokens, over the {} ceiling",
            context::estimate_tokens(&head),
            context::DEFAULT_CEILING
        );
        assert!(head.contains("You are Chief"), "instructions must survive");
    }

    #[test]
    fn the_instructions_pin_the_shape_a_small_model_needs() {
        // A 3B model given "write a brief" writes an essay.
        assert!(INSTRUCTIONS.contains("One short bullet per line"));
        assert!(INSTRUCTIONS.contains("Do not guess"));
        assert!(INSTRUCTIONS.contains("leave out"));
    }

    #[test]
    fn sources_name_only_what_had_something_to_say() {
        assert_eq!(
            gathered().sources(),
            [
                "calendar",
                "review requests",
                "pull requests",
                "work log",
                "inbox",
                "GitHub issues",
                "Linear",
                "Jira",
                "Confluence",
                "corpus"
            ]
        );

        let empty = Gathered::default();
        assert!(empty.sources().is_empty());
        assert!(empty.is_empty());
    }

    /// The two trackers answer the same question and share a heading, which is
    /// why they are two fields: merged into one, a brief built from Jira alone
    /// would tell the reader it came from Linear.
    #[test]
    fn a_brief_built_from_jira_alone_does_not_claim_to_come_from_linear() {
        let found = Gathered {
            jira: vec!["PROJ-7 Migrate the tenant [In Review]".to_string()],
            ..Gathered::default()
        };

        assert_eq!(found.sources(), ["Jira"]);
    }

    /// And they still arrive under one heading, so the reader is not asked to
    /// merge two lists of the same thing.
    #[test]
    fn both_trackers_land_under_one_heading() {
        let sections = gathered().sections();
        let tracked: Vec<_> = sections
            .iter()
            .filter(|(title, _)| *title == "Assigned to you and not finished")
            .collect();

        assert_eq!(tracked.len(), 1, "{sections:?}");

        let lines = tracked[0].1.join("\n");
        assert!(lines.contains("REC-42"), "{lines}");
        assert!(lines.contains("PROJ-7"), "{lines}");
        assert!(lines.contains("#3"), "{lines}");
    }

    #[test]
    fn a_meeting_reads_as_a_time_a_subject_and_who_is_in_it() {
        let event = microsoft::Event {
            subject: "1:1 with Sam".to_string(),
            start: "2026-08-28T10:00:00.0000000".to_string(),
            end: "2026-08-28T10:30:00.0000000".to_string(),
            organiser: Some("Sam Patel".to_string()),
            attendees: vec!["Sam Patel".to_string()],
            online: true,
            all_day: false,
        };

        // The title already says who, so the attendee is not said again.
        assert_eq!(describe_event(&event), "10:00 1:1 with Sam");
    }

    /// The defect: "1:1 with Priya" with Priya invited was written, in code, as
    /// "1:1 with Priya with Priya" in every run of the calendar fixture.
    ///
    /// Proved by making `named_in` return false: the name is said twice.
    #[test]
    fn an_attendee_the_title_names_is_not_named_again_but_the_others_are() {
        let event = |subject: &str, attendees: &[&str]| microsoft::Event {
            subject: subject.to_string(),
            start: "2026-08-28T09:00:00.0000000".to_string(),
            end: "2026-08-28T09:30:00.0000000".to_string(),
            organiser: None,
            attendees: attendees.iter().map(|name| name.to_string()).collect(),
            online: false,
            all_day: false,
        };

        assert_eq!(
            describe_event(&event("1:1 with Priya", &["Priya"])),
            "09:00 1:1 with Priya"
        );
        assert_eq!(
            describe_event(&event("Team standup", &["Priya", "Tom", "Ana"])),
            "09:00 Team standup with Priya, Tom, Ana"
        );
        assert_eq!(
            describe_event(&event("Sync with priya", &["Priya Shah", "Tom"])),
            "09:00 Sync with priya with Tom"
        );
        // A name inside a longer word is not the name.
        assert_eq!(
            describe_event(&event("Banana review", &["Ana"])),
            "09:00 Banana review with Ana"
        );
    }

    // ---- end to end, against stub servers ----

    use crate::db::test_support::migrated_pool;
    use crate::llama::test_support::{delta, events, serve};

    /// What the stub engine writes back, in the shape `llama-server` streams
    /// it — the brief asks for a stream so that a slow answer is not mistaken
    /// for a stalled one. See `sections::write`. It words both lines `ready`
    /// logs, so nothing needs putting back.
    fn answer() -> String {
        events(&[delta(
            "- Shipped the loopback listener\n- Merged the planner",
        )])
    }

    struct Scratch(std::path::PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The guard on a brief that named nothing, found by running the real one.
    ///
    /// `gather` read `work_log::fetch` and took `summary`, which was a
    /// model-written sentence until deterministic ingestion made it a bare
    /// state word. From then on the whole of a brief's material read:
    ///
    /// ```text
    /// ## Recently logged work
    /// - merged
    /// - merged
    /// - open
    /// ```
    ///
    /// Measured against the real repository on 2026-09-03, the model was given
    /// exactly that under a rule saying "name people and pull requests exactly
    /// as they appear below" — and, with nothing to name, invented a meeting,
    /// two people and three pull request numbers. `WorkLogEntry` has no
    /// `title`: migration 8 added the column and `fetch` never selected it, so
    /// the only field carrying what a row is *about* was unreachable from here.
    ///
    /// Proved by taking `summary` again instead of `retrieval::line`:
    ///
    /// ```text
    /// a brief's material has to name the thing, not just its state:
    ///   ["merged"]
    /// ```
    #[tokio::test]
    async fn the_material_names_the_work_and_not_only_its_state() {
        let pool = migrated_pool().await;

        work_log::upsert(
            &pool,
            work_log::WorkLogRecord {
                timestamp: "2026-09-03T07:21:03Z".to_string(),
                source: "github".to_string(),
                category: crate::ingest::SHIPPED.to_string(),
                title: "scottmallinson/chief.ai #65: answer read questions from a local work log"
                    .to_string(),
                content: "Merged pull request #65".to_string(),
                summary: Some("merged".to_string()),
                url: None,
                raw_ref: None,
                external_id: "scottmallinson/chief.ai#65".to_string(),
                account_id: 1,
            },
        )
        .await
        .expect("should store");

        let root = std::env::temp_dir().join(format!("chief-material-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let corpus = Corpus::at(root.clone());
        corpus.ensure_shape().await.expect("should create");
        let _scratch = Scratch(root);

        let context = Context {
            pool,
            github: github::Client::against("127.0.0.1:1").expect("client"),
            microsoft: microsoft::Client::against("127.0.0.1:1").expect("client"),
            calendar: crate::calendar::Client::new().expect("client"),
            linear: crate::linear::Client::new().expect("client"),
            atlassian: crate::atlassian::Client::new().expect("client"),
            atlassian_rest: crate::atlassian::rest::Rest::new().expect("client"),
            engine: llama::Client::with_base_url("http://127.0.0.1:1").expect("client"),
            corpus,
            short_brief_lines: 0,
        };

        let found = gather(&context, chrono::Local::now()).await;

        assert!(
            found
                .shipped
                .iter()
                .any(|line| line.contains("#65") && line.contains("local work log")),
            "a brief's material has to name the thing, not just its state: {:?}",
            found.shipped
        );
    }

    /// A context with a work log entry, a corpus, and an engine that answers.
    async fn ready(engine_host: &str, name: &str) -> (Context, Scratch) {
        let pool = migrated_pool().await;
        let root = std::env::temp_dir().join(format!("chief-recipe-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        let corpus = Corpus::at(root.clone());
        corpus.ensure_shape().await.expect("should create");
        corpus::reindex(&pool, &corpus).await.expect("should index");

        // Two lines, so the part is worded by the model and not written whole in
        // code; a part of one line is its own list.
        for (content, summary) in [
            (
                "Merged the loopback listener",
                "Shipped the loopback listener",
            ),
            ("Merged the planner", "Merged the planner"),
        ] {
            work_log::insert(
                &pool,
                work_log::NewWorkLogEntry {
                    source: "github".to_string(),
                    content: content.to_string(),
                    summary: Some(summary.to_string()),
                    timestamp: None,
                    account_id: None,
                    external_id: None,
                },
            )
            .await
            .expect("should log");
        }

        (
            Context {
                pool,
                github: github::Client::against("127.0.0.1:1").expect("client"),
                microsoft: microsoft::Client::against("127.0.0.1:1").expect("client"),
                calendar: crate::calendar::Client::new().expect("client"),
                linear: crate::linear::Client::new().expect("client"),
                atlassian: crate::atlassian::Client::new().expect("client"),
                atlassian_rest: crate::atlassian::rest::Rest::new().expect("client"),
                engine: llama::Client::with_base_url(engine_host).expect("client"),
                corpus,
                short_brief_lines: 0,
            },
            Scratch(root),
        )
    }

    // ---------------------------------------------------------------------
    // Which transport an Atlassian account reads over.
    // ---------------------------------------------------------------------

    fn account_keyed(key: &str, identity: Option<&str>) -> integrations::Account {
        integrations::Account {
            id: 1,
            service: integrations::ATLASSIAN.to_string(),
            account_key: key.to_string(),
            label: None,
            identity: identity.map(str::to_string),
            connected_at: "2026-09-14T09:00:00Z".to_string(),
        }
    }

    fn credentials_of(kind: &str) -> integrations::Credentials {
        integrations::Credentials {
            id: 1,
            kind: kind.to_string(),
            access_token: "a-token".to_string(),
            refresh_token: None,
            expires_at: None,
            client_id: None,
            client_secret: None,
        }
    }

    /// **The credential decides the transport, and nothing else does.** A
    /// pasted token cannot be exchanged for an MCP session and a minted client
    /// cannot sign HTTP Basic, so getting this wrong is not a degraded read —
    /// it is a connected account that returns nothing, for a reason no error
    /// on screen would explain.
    #[test]
    fn a_pasted_token_reads_over_rest_and_a_minted_client_does_not() {
        assert_eq!(
            route(
                &account_keyed("acme.atlassian.net", Some("scott@example.com")),
                &credentials_of(integrations::API_KEY),
            ),
            Route::Rest {
                site: "https://acme.atlassian.net".to_string(),
                email: "scott@example.com".to_string(),
            }
        );

        for kind in [integrations::DCR, integrations::OAUTH] {
            assert_eq!(
                route(
                    &account_keyed("acme.atlassian.net", Some("scott@example.com")),
                    &credentials_of(kind),
                ),
                Route::Mcp,
                "{kind} is a sign-in, not a pasted credential"
            );
        }
    }

    fn said(text: &str) -> String {
        events(&[delta(text)])
    }

    /// A stream that ends because the model reached its length limit.
    fn ran_out(chunks: &[serde_json::Value]) -> String {
        let mut all = chunks.to_vec();
        all.push(serde_json::json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "length" }],
        }));
        events(&all)
    }

    /// The defect the third Mac batch found: LFM2.5 with its reasoning left on
    /// spends the whole length limit thinking and says nothing. The stream
    /// layer then writes its "[Cut short]" marker onto an empty answer, the
    /// marker is not empty, and the marker alone was filed as the day's brief
    /// in 7 of 7 runs. The part is now written from its lines instead.
    ///
    /// Proved by deleting `unfinished ||` from `sections::write`: the marker
    /// is the brief.
    #[tokio::test]
    async fn a_model_that_ran_out_before_saying_anything_is_replaced_by_the_material() {
        let thinking = serde_json::json!({
            "choices": [{ "index": 0, "delta": { "reasoning_content": "Let me count" } }],
        });
        let (host, _server) = serve(vec![("HTTP/1.1 200 OK", ran_out(&[thinking]))]);
        let (context, _scratch) = ready(&host, "ran-out-silent").await;

        let brief = daily_brief(&context).await.expect("a brief");

        assert!(brief.checked.fell_back, "{:?}", brief.checked);
        assert!(
            brief.markdown.contains("Shipped the loopback listener")
                && brief.markdown.contains("Merged the planner"),
            "{}",
            brief.markdown
        );
        assert!(!brief.markdown.contains("Cut short"), "{}", brief.markdown);
    }

    /// A brief cut off at the limit may end halfway through a claim, so it is
    /// not used; the one written from the material is.
    ///
    /// Proved by deleting `unfinished ||` from `sections::write`: the half-line and
    /// the apology are the brief.
    #[tokio::test]
    async fn a_brief_cut_off_at_the_length_limit_is_written_from_the_material_instead() {
        let (host, _server) = serve(vec![(
            "HTTP/1.1 200 OK",
            ran_out(&[delta("- Shipped the loopback lis")]),
        )]);
        let (context, _scratch) = ready(&host, "ran-out-partial").await;

        let brief = daily_brief(&context).await.expect("a brief");

        assert!(brief.checked.fell_back, "{:?}", brief.checked);
        assert!(
            brief.markdown.contains("Shipped the loopback listener"),
            "{}",
            brief.markdown
        );
        assert!(!brief.markdown.contains("Cut short"), "{}", brief.markdown);
    }

    /// A day this short is its own list, so no model is asked and nothing can be
    /// added to it. Measured: Llama 3.2 3B and LFM2.5 each turned a school
    /// notice into "a meeting is scheduled".
    ///
    /// Proved by setting `short_brief_lines` to 0 in the render: the stub is
    /// asked and its invented line is the brief.
    #[tokio::test]
    async fn a_day_with_a_few_lines_is_written_without_asking_the_model() {
        let (host, server) = serve(vec![(
            "HTTP/1.1 200 OK",
            said("- A meeting is scheduled with Sam\n- Shipped the loopback listener"),
        )]);
        let (mut context, _scratch) = ready(&host, "short-day").await;
        context.short_brief_lines = SHORT_BRIEF_LINES;

        let brief = daily_brief(&context).await.expect("a brief");

        assert!(brief.checked.by_code, "{:?}", brief.checked);
        assert!(!brief.checked.fell_back);
        assert!(
            brief.markdown.contains("loopback listener"),
            "{}",
            brief.markdown
        );
        assert!(!brief.markdown.contains("Sam"), "{}", brief.markdown);
        server.abort();
    }

    /// A brief that names nothing the material lacks costs one call and is
    /// used as written.
    #[tokio::test]
    async fn a_faithful_brief_is_used_as_written_and_not_asked_for_twice() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", answer())]);
        let (context, _scratch) = ready(&host, "faithful").await;

        let brief = daily_brief(&context).await.expect("a brief");

        assert_eq!(brief.checked, Checked::default());
        assert_eq!(server.await.expect("stub").len(), 1);
    }

    /// The defect this exists for, measured on Llama 3.2 3B: given a day with
    /// two emails and no meetings it wrote "Meeting with John Doe at 10:00".
    /// The answer is not used, and the model is not asked again.
    ///
    /// Proved by making `check::unsupported_by` return nothing: the invented
    /// meeting is filed as the brief.
    #[tokio::test]
    async fn a_brief_that_invents_a_meeting_is_written_from_the_material_instead() {
        let (host, server) = serve(vec![(
            "HTTP/1.1 200 OK",
            said("- Meeting with John Doe at 10:00\n- Shipped the loopback listener"),
        )]);
        let (context, _scratch) = ready(&host, "invents").await;

        let brief = daily_brief(&context).await.expect("a brief");

        assert!(brief.checked.fell_back, "{:?}", brief.checked);
        assert_eq!(
            brief.checked.first,
            vec!["time 10:00", "name John", "name Doe"]
        );
        assert!(
            brief.markdown.contains("loopback listener"),
            "{}",
            brief.markdown
        );
        assert!(!brief.markdown.contains("10:00") && !brief.markdown.contains("John"));
        assert_eq!(server.await.expect("stub").len(), 1, "never a second call");
    }

    /// A time in the prompt's own clock is the time of nothing the user has.
    #[tokio::test]
    async fn the_time_the_brief_was_written_is_not_a_meeting() {
        let (host, _server) = serve(vec![(
            "HTTP/1.1 200 OK",
            said("- Meeting at 06:00\n- Shipped the loopback listener"),
        )]);
        let (context, _scratch) = ready(&host, "clock").await;

        let brief = daily_brief_at(&context, at_local("2026-10-18T06:00:00"))
            .await
            .expect("a brief");

        assert!(brief.checked.fell_back, "{:?}", brief.checked);
    }

    /// **A long day is written in full.** It used to be five bullets, and when
    /// the model's were refused, six lines and "And 2 more". Nothing is
    /// summed up now.
    ///
    /// Proved by truncating the lines of each part to six in `sections::write`:
    ///
    /// ```text
    /// every review is in the brief: Review #7
    /// ```
    #[tokio::test]
    async fn a_long_day_is_written_in_full_and_never_summed_up_as_and_n_more() {
        let (host, _server) = serve(vec![("HTTP/1.1 200 OK", said("- ok"))]);
        let engine = llama::Client::with_base_url(&host).expect("client");
        let found = Gathered {
            agenda: vec!["09:00 Standup".to_string()],
            waiting: (1..=7).map(|n| format!("Review #{n}")).collect(),
            ..Gathered::default()
        };

        let (written, checked) = write_checked(&engine, 0, &found, PRESENT)
            .await
            .expect("a brief");

        for n in 1..=7 {
            assert!(
                written.contains(&format!("Review #{n}")),
                "every review is in the brief: Review #{n}\n{written}"
            );
        }
        assert!(written.contains("09:00 Standup"), "{written}");
        assert!(!written.contains("more"), "{written}");
        assert!(written.starts_with("## Today's meetings\n"), "{written}");
        assert!(!checked.fell_back, "{checked:?}");
    }

    /// More lines than one request carries are asked about in turn, each with
    /// its own lines, under one heading.
    #[tokio::test]
    async fn a_part_longer_than_one_request_is_asked_in_pieces() {
        let (host, server) = serve(vec![
            ("HTTP/1.1 200 OK", said("- first")),
            ("HTTP/1.1 200 OK", said("- second")),
        ]);
        let engine = llama::Client::with_base_url(&host).expect("client");
        let found = Gathered {
            waiting: (1..=11).map(|n| format!("Review #{n}")).collect(),
            ..Gathered::default()
        };

        let (written, _) = write_checked(&engine, 0, &found, PRESENT)
            .await
            .expect("a brief");

        let requests = server.await.expect("stub");
        assert_eq!(requests.len(), 2, "eleven lines are two requests");
        assert!(
            requests[0].contains("Review #8") && !requests[0].contains("Review #9"),
            "the first request carries the first eight"
        );
        assert!(
            requests[1].contains("Review #9") && !requests[1].contains("Review #8"),
            "the second carries the rest"
        );
        assert_eq!(
            written.matches("## ").count(),
            1,
            "one heading for one part: {written}"
        );
        for n in 1..=11 {
            assert!(written.contains(&format!("Review #{n}")), "{written}");
        }
    }

    #[tokio::test]
    async fn writes_the_brief_into_the_corpus_one_call_for_each_worded_part() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", answer())]);
        let (context, _scratch) = ready(&host, "writes").await;

        let brief = daily_brief(&context).await.expect("should write a brief");

        assert!(
            brief.markdown.contains("Shipped the loopback listener"),
            "{brief:?}"
        );
        assert!(brief.path.starts_with("briefs/"), "{brief:?}");

        // It is in the corpus, as a file the user can open.
        assert_eq!(
            context.corpus.read(&brief.path).await.expect("read"),
            brief.markdown
        );

        // One part of two lines is one short call.
        let requests = server.await.expect("the stub should finish");
        assert_eq!(requests.len(), 1, "one model call for one part");
    }

    /// The defect REC-64 is about, guarded where it actually happened.
    ///
    /// `clock::stamp_at` is asserted in its own module, but nothing there stops
    /// this call site being pointed back at `clock::present`. This reads the
    /// prompt the engine was really sent.
    ///
    /// Proved by restoring `clock::present()` here:
    ///
    /// ```text
    /// the brief prompt must carry no list of dates to echo
    /// ```
    #[tokio::test]
    async fn the_brief_prompt_carries_the_clock_and_no_list_of_dates() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", answer())]);
        let (context, _scratch) = ready(&host, "no-date-list").await;

        daily_brief(&context).await.expect("should write a brief");

        let requests = server.await.expect("the stub should finish");
        let sent = requests.first().expect("one model call").clone();

        assert!(
            sent.contains("The current date and time is"),
            "it still knows when now is: {sent}"
        );

        for leak in ["- yesterday:", "- tomorrow:", "last week", "Resolve every"] {
            assert!(
                !sent.contains(leak),
                "the brief prompt must carry no list of dates to echo: {sent}"
            );
        }
    }

    /// The defect REC-65 is about.
    ///
    /// The client bounds a request by silence rather than by a deadline, so a
    /// slow answer is never cut off — but silence only means anything on a
    /// stream. A non-streamed request says nothing until the whole answer is
    /// ready, which turns the inactivity timeout into a deadline for the
    /// entire generation, and the brief is the longest generation Chief makes.
    /// Reported from a running app on Llama 3.2 1B as "the model engine
    /// stopped responding part-way through the answer", when nothing had gone
    /// wrong except that the model was still working.
    ///
    /// Proved by asking for the answer in one piece again:
    ///
    /// ```text
    /// the brief must ask for a stream, or its ninety seconds of silence
    /// become a deadline for the whole answer
    /// ```
    #[tokio::test]
    async fn the_brief_asks_for_a_stream_so_a_slow_answer_is_not_a_stalled_one() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", answer())]);
        let (context, _scratch) = ready(&host, "streams").await;

        // The result is deliberately ignored. A brief asked for in one piece
        // cannot read this stub's reply and would fail here, which would end
        // the test before it reached the assertion that explains why — so what
        // was *sent* is the thing examined, whatever came back.
        let _ = daily_brief(&context).await;

        let requests = server.await.expect("the stub should finish");
        let sent = requests.first().expect("one model call").clone();

        assert!(
            sent.contains("\"stream\":true"),
            "the brief must ask for a stream, or its ninety seconds of silence become a deadline \
             for the whole answer: {sent}"
        );
    }

    #[tokio::test]
    async fn regenerating_the_same_day_replaces_rather_than_stacking() {
        let second = events(&[delta("- a later brief")]);
        let (host, _server) = serve(vec![
            ("HTTP/1.1 200 OK", answer()),
            ("HTTP/1.1 200 OK", second),
        ]);
        let (context, _scratch) = ready(&host, "replaces").await;

        let first = daily_brief(&context).await.expect("first");
        let again = daily_brief(&context).await.expect("second");

        assert_eq!(first.path, again.path, "the same day is the same file");
        let filed = context.corpus.read(&again.path).await.expect("read");
        assert_eq!(filed, again.markdown);
        assert!(
            filed.contains("a later brief") && !filed.contains(&first.markdown),
            "a brief is what today looks like now, not a history of it: {filed}"
        );

        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM briefs")
            .fetch_one(&context.pool)
            .await
            .expect("count");
        assert_eq!(rows, 1, "one row per day");
    }

    #[tokio::test]
    async fn a_brief_the_user_has_edited_is_not_overwritten() {
        // Measured in the product: a hand-written section was added to today's
        // brief, the brief was regenerated, and the section was gone with
        // nothing said. The corpus is offered as a folder you can edit.
        let second = events(&[delta("- a replacement")]);
        let (host, _server) = serve(vec![
            ("HTTP/1.1 200 OK", answer()),
            ("HTTP/1.1 200 OK", second),
        ]);
        let (context, _scratch) = ready(&host, "edited").await;

        let first = daily_brief(&context).await.expect("first brief");

        // The user opens it and types something. Two seconds on, so the change
        // is distinguishable from Chief's own write.
        tokio::time::sleep(std::time::Duration::from_millis(2_100)).await;
        let mine = format!("{}\n\nMy own note.\n", first.markdown);
        context
            .corpus
            .write(&first.path, &mine)
            .await
            .expect("should write");

        let refused = daily_brief(&context)
            .await
            .expect_err("an edited brief must not be overwritten");

        assert!(matches!(refused, Error::EditedByHand { .. }), "{refused:?}");
        assert!(
            refused.to_string().contains("Delete or rename"),
            "the message has to say how to get a fresh one: {refused}"
        );

        assert_eq!(
            context.corpus.read(&first.path).await.expect("read"),
            mine,
            "the user's text must still be there"
        );
    }

    #[tokio::test]
    async fn a_brief_chief_wrote_itself_is_replaced_as_before() {
        // The guard must not stop the ordinary case: a brief nobody has touched
        // is still regenerated.
        let second = events(&[delta("- a later brief")]);
        let (host, _server) = serve(vec![
            ("HTTP/1.1 200 OK", answer()),
            ("HTTP/1.1 200 OK", second),
        ]);
        let (context, _scratch) = ready(&host, "untouched").await;

        daily_brief(&context).await.expect("first");
        let again = daily_brief(&context).await.expect("should replace its own");

        assert!(context
            .corpus
            .read(&again.path)
            .await
            .expect("read")
            .contains("a later brief"));
    }

    #[tokio::test]
    async fn a_model_that_will_not_answer_leaves_no_brief_behind() {
        let (host, _server) = serve(vec![("HTTP/1.1 500 Internal Server Error", "{}")]);
        let (context, _scratch) = ready(&host, "fails").await;

        let error = daily_brief(&context).await.expect_err("the engine refused");

        assert!(matches!(error, Error::Engine(_)), "{error:?}");

        // Nothing half-written: a brief file that exists is one a model wrote.
        let date = today_date();
        assert!(
            context
                .corpus
                .read(&format!("briefs/{date}.md"))
                .await
                .is_err(),
            "a failed render must not leave a file"
        );
        assert_eq!(
            written_at(&context.pool, &date).await.expect("read"),
            None,
            "and must not claim the day was briefed"
        );
    }

    #[tokio::test]
    async fn says_so_when_there_is_nothing_to_brief_on() {
        let (host, _server) = serve(Vec::<(&str, &str)>::new());
        let pool = migrated_pool().await;
        let root = std::env::temp_dir().join(format!("chief-recipe-{}-empty", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _scratch = Scratch(root.clone());

        let context = Context {
            pool,
            github: github::Client::against("127.0.0.1:1").expect("client"),
            microsoft: microsoft::Client::against("127.0.0.1:1").expect("client"),
            calendar: crate::calendar::Client::new().expect("client"),
            linear: crate::linear::Client::new().expect("client"),
            atlassian: crate::atlassian::Client::new().expect("client"),
            atlassian_rest: crate::atlassian::rest::Rest::new().expect("client"),
            engine: llama::Client::with_base_url(&host).expect("client"),
            corpus: Corpus::at(root),
            short_brief_lines: 0,
        };

        let error = daily_brief(&context)
            .await
            .expect_err("nothing is connected");

        assert!(matches!(error, Error::NothingToSay), "{error:?}");
    }

    #[test]
    fn today_runs_midnight_to_midnight() {
        let (from, to) = today_at(&chrono::Local::now());

        assert!(from.contains("T00:00:00"), "{from}");
        assert!(to > from);
    }
}
