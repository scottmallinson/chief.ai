//! Deterministic work, then one sentence from the model.
//!
//! Small models are poor at planning and good at writing. So a recipe runs in
//! three phases and the model is only present for the last of them:
//!
//! 1. **Gather** — plain Rust. Read the calendar, the pull requests, the work
//!    log, the corpus. No model, no planning, no tool calls, nothing that can
//!    decide to do something else.
//! 2. **Assemble** — build one prompt, through a [`crate::context::Budget`] that
//!    refuses to go over rather than letting the engine truncate.
//! 3. **Render** — exactly one call, no tools. If a recipe needs two calls it is
//!    two recipes.
//!
//! That third rule is the one worth defending. An agent loop asked to compose a
//! brief will call a tool, narrate, call another, and arrive somewhere unrepeatable
//! forty seconds later. The same brief assembled in Rust and written once is the
//! same brief every morning, and the only part that varies is the prose.
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
use crate::{clock, corpus, github, integrations};

/// How long a brief may run to. Long enough for a handful of bullets and their
/// context, short enough that a slow machine finishes it.
const BRIEF_TOKENS: u32 = 500;

/// How many of anything is gathered for one brief.
pub(crate) const PER_SOURCE: u8 = 10;

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
}

/// What can go wrong making a brief.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("nothing is connected yet, so there is nothing to brief you on")]
    NothingToSay,
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
async fn gather(context: &Context) -> Gathered {
    let mut found = Gathered::default();

    let (from, to) = today();

    // Subscriptions first: they need no sign-in, so on most machines this is
    // the only calendar there is. Merged into the same agenda as Outlook, so
    // the brief never has to know which kind an entry came from.
    found
        .agenda
        .extend(subscribed_events(context).await.iter().map(describe_event));

    for account in outlook_accounts(context).await {
        let session = OutlookSession::new(&context.pool, &context.microsoft, account);

        if let Ok(events) = session.events(&from, &to, PER_SOURCE).await {
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
            found.assigned.extend(issues.iter().map(describe_issue));
        }
    }

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

/// Today's events from every calendar the user subscribed to.
///
/// A subscription that will not load fails that one calendar and nothing else,
/// the same rule every other source in `gather` follows: a brief with the
/// calendar and no mail is worth having.
pub(crate) async fn subscribed_events(context: &Context) -> Vec<microsoft::Event> {
    subscribed_events_by_account(context)
        .await
        .into_iter()
        .map(|(_, event)| event)
        .collect()
}

/// The same, keeping the account each event was read from.
///
/// The brief does not care — a reader must never have to know which calendar a
/// meeting came from — but ingestion does: the work log's dedupe index is
/// `(source, account_id, external_id)`, so a person with a work and a personal
/// calendar holding the same meeting keeps both rows rather than having one
/// overwrite the other.
pub(crate) async fn subscribed_events_by_account(
    context: &Context,
) -> Vec<(i64, microsoft::Event)> {
    let Ok(accounts) =
        crate::integrations::accounts(&context.pool, crate::integrations::CALENDAR).await
    else {
        return Vec::new();
    };

    let now = chrono::Local::now();
    let offset = now.offset().local_minus_utc();
    let midnight = now.date_naive().and_time(chrono::NaiveTime::MIN);
    let end = midnight + chrono::Duration::days(1);

    let mut found = Vec::new();

    for account in accounts {
        let Ok(Some(credentials)) =
            crate::integrations::credentials(&context.pool, account.id).await
        else {
            continue;
        };

        match context
            .calendar
            .events(&credentials.access_token, midnight, end, offset)
            .await
        {
            Ok(events) => found.extend(events.into_iter().map(|event| (account.id, event))),
            // Reported without the address, which is a credential.
            Err(error) => eprintln!("a calendar subscription could not be read: {error}"),
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
enum Route {
    /// Atlassian's REST API, with HTTP Basic over the pasted token.
    Rest { site: String, email: String },
    /// The MCP server, over the client this account registered for itself.
    Mcp,
}

fn route(account: &integrations::Account, credentials: &integrations::Credentials) -> Route {
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

/// Phase two: one prompt, and never more than the budget allows.
///
/// The order is load-bearing twice over. The instructions and the background go
/// first because they change least, and llama.cpp reuses the cached prefix of a
/// prompt it has already seen — a brief regenerated an hour later pays prefill
/// only on what actually moved. And when the budget runs out it runs out at the
/// bottom, which is where the least important material is.
fn assemble(found: &Gathered, present: &str) -> Result<String, Error> {
    let mut budget = Budget::new();
    let mut prompt = String::new();

    let mut push = |budget: &mut Budget, name: &str, block: &str| -> bool {
        if budget.add(name, block).is_err() {
            return false;
        }
        prompt.push_str(block);
        prompt.push_str("\n\n");
        true
    };

    push(&mut budget, "instructions", INSTRUCTIONS);
    push(&mut budget, "clock", present);

    for (path, text) in &found.background {
        // A background file that does not fit is skipped rather than ending the
        // brief: the calendar matters more than the writing-style notes.
        push(&mut budget, path, &format!("## {path}\n{text}"));
    }

    // The two trackers, under the heading they share. Built here rather than
    // in `gather` so `Gathered::sources` can still say which of them answered.
    let tracked = [found.assigned.as_slice(), found.jira.as_slice()].concat();

    for (heading, items) in [
        ("Today's meetings", &found.agenda),
        (
            "Waiting on you — somebody has asked for your review",
            &found.waiting,
        ),
        ("Your open pull requests", &found.mine),
        ("Unread mail", &found.inbox),
        ("Assigned to you and not finished", &tracked),
        ("Pages you have been writing", &found.pages),
        ("Recently logged work", &found.shipped),
    ] {
        if items.is_empty() {
            continue;
        }

        let block = format!(
            "## {heading}\n{}",
            items
                .iter()
                .map(|item| format!("- {item}"))
                .collect::<Vec<_>>()
                .join("\n")
        );

        push(&mut budget, heading, &block);
    }

    Ok(prompt.trim_end().to_string())
}

/// What the model is asked to do with all of it.
///
/// Rigidly shaped on purpose. A 3B model given "write a brief" writes an essay;
/// given a fixed number of bullets and a named order it writes a brief.
const INSTRUCTIONS: &str = "\
You are Chief, an AI chief of staff. Write the user's brief for today from the \
material below, and from nothing else.

Rules:
- At most five bullets. Fewer if there is less to say.
- Lead with whatever is time-bound: a meeting happens whether or not it is read about.
- Name people and pull requests exactly as they appear below.
- State only what the material says. Do not guess, and do not invent numbers.
- No preamble, no sign-off, no headings. Bullets only.";

/// Phase three: exactly one call, no tools.
///
/// **Streamed, even though nobody is watching it arrive.** The engine client
/// bounds a request by silence rather than by a deadline, precisely so a slow
/// answer is not cut off — but on a non-streamed request there is nothing to
/// hear until the whole answer is ready, so the inactivity timeout becomes a
/// deadline for the entire generation. That is the failure `SILENCE_TIMEOUT`
/// was written to avoid, and the brief was the one call still exposed to it:
/// the longest generation Chief makes, and reported as "the model engine
/// stopped responding part-way through the answer" when nothing had gone
/// wrong except that a small model was still working. Measured on Llama 3.2
/// 1B, where a cold brief takes longer than the ninety seconds allowed.
///
/// The tokens are dropped. Nothing here has anywhere to put them — a brief is
/// written to a file, not to a transcript — so this asks for a stream to be
/// timed correctly rather than to show progress. See REC-65.
async fn render(engine: &llama::Client, prompt: &str) -> Result<String, Error> {
    let request = ChatRequest::new(crate::agent::DEFAULT_MODEL, vec![Message::user(prompt)])
        .with_options(Options::new().with_answer_length(BRIEF_TOKENS));

    let reply = engine.chat_stream(&request, |_| {}).await?;

    Ok(reply.content.trim().to_string())
}

/// Make today's brief and write it into the corpus.
///
/// Regenerating replaces: a brief is what today looks like now, not a history
/// of what it looked like at each point during it.
pub async fn daily_brief(context: &Context) -> Result<Brief, Error> {
    let found = gather(context).await;

    // Nothing connected and nothing logged is not a brief with no bullets, it
    // is a machine that has not been set up. Saying so beats asking a model to
    // write about an empty page.
    if found.is_empty() {
        return Err(Error::NothingToSay);
    }

    // `today`, not `present`: the brief gets the clock and not the ranges.
    // See `clock::today` — a 1B model handed that date list returned it as the
    // bullets it had been asked for.
    let prompt = assemble(&found, &clock::today())?;
    let markdown = render(&context.engine, &prompt).await?;

    let date = today_date();
    let path = format!("briefs/{date}.md");

    // A brief the user has touched is theirs. The corpus is offered as a folder
    // they can edit, and regenerating used to overwrite it without a word —
    // measured, a hand-written section simply vanished. Checked here rather
    // than in the corpus, because only a brief knows when Chief last wrote it.
    if edited_by_hand(context, &date, &path).await {
        return Err(Error::EditedByHand { path });
    }

    context.corpus.write(&path, &markdown).await?;
    record(&context.pool, &date, &found.sources()).await?;

    Ok(Brief {
        date,
        path,
        markdown,
        sources: found.sources(),
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
            SET generated_at = excluded.generated_at, sources = excluded.sources",
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
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Midnight to midnight, in this machine's own time zone.
pub(crate) fn today() -> (String, String) {
    let midnight = chrono::Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap_or_default();

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
    let when = event
        .start
        .split('T')
        .nth(1)
        .unwrap_or("")
        .get(0..5)
        .unwrap_or("");
    let who = if event.attendees.is_empty() {
        String::new()
    } else {
        format!(" with {}", event.attendees.join(", "))
    };

    format!("{when} {}{who}", event.subject).trim().to_string()
}

fn describe_message(message: &microsoft::MailMessage) -> String {
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
    use super::*;
    use crate::work_log;

    const PRESENT: &str = "The current date and time is 08:00 on Friday 28 August 2026.";

    fn gathered() -> Gathered {
        Gathered {
            agenda: vec!["10:00 1:1 with Sam with Sam Patel".to_string()],
            waiting: vec!["scottmallinson/chief.ai #44 — Ana asked for your review".to_string()],
            mine: vec!["scottmallinson/chief.ai #12 — Add the corpus".to_string()],
            shipped: vec!["Shipped the loopback listener".to_string()],
            inbox: vec!["Dana Reid: Re: the migration".to_string()],
            assigned: vec!["REC-42 Read Linear [In Progress]".to_string()],
            jira: vec!["PROJ-7 Migrate the tenant [In Review]".to_string()],
            pages: vec!["Tenant migration runbook [ENG]".to_string()],
            background: vec![(
                "context/agents/org/team_structure.md".to_string(),
                "# Team\nSam owns auth.".to_string(),
            )],
        }
    }

    #[test]
    fn the_prompt_carries_every_source_that_had_something() {
        let prompt = assemble(&gathered(), PRESENT).expect("should assemble");

        assert!(prompt.contains("1:1 with Sam"), "{prompt}");
        assert!(prompt.contains("#12"), "{prompt}");
        assert!(prompt.contains("Dana Reid"), "{prompt}");
        assert!(prompt.contains("Shipped the loopback listener"), "{prompt}");
        assert!(prompt.contains("Sam owns auth."), "{prompt}");
    }

    #[test]
    fn a_source_with_nothing_in_it_takes_no_room_at_all() {
        let mut sparse = gathered();
        sparse.inbox.clear();
        sparse.waiting.clear();

        let prompt = assemble(&sparse, PRESENT).expect("should assemble");

        assert!(!prompt.contains("Unread mail"), "{prompt}");
        assert!(!prompt.contains("Pull requests waiting"), "{prompt}");
    }

    #[test]
    fn the_stable_material_comes_first_so_the_prefix_can_be_reused() {
        // llama.cpp reuses the cached prefix of a prompt it has seen. The
        // instructions never change and the agenda changes hourly, so this
        // order is what makes a regenerated brief cheap.
        let prompt = assemble(&gathered(), PRESENT).expect("should assemble");

        let instructions = prompt.find("You are Chief").expect("instructions");
        let background = prompt.find("Sam owns auth").expect("background");
        let agenda = prompt.find("Today's meetings").expect("agenda");

        assert!(instructions < background, "{prompt}");
        assert!(background < agenda, "{prompt}");
    }

    #[test]
    fn the_prompt_stays_inside_the_budget_however_much_was_gathered() {
        let mut flood = gathered();
        flood.agenda = (0..500).map(|n| format!("a meeting number {n}")).collect();
        flood.inbox = (0..500).map(|n| format!("a message number {n}")).collect();

        let prompt = assemble(&flood, PRESENT).expect("an over-full gather still assembles");

        assert!(
            context::estimate_tokens(&prompt) <= context::DEFAULT_CEILING,
            "the prompt is {} tokens, over the {} ceiling",
            context::estimate_tokens(&prompt),
            context::DEFAULT_CEILING
        );

        // And it kept the part that matters rather than the part that arrived
        // first alphabetically.
        assert!(
            prompt.contains("You are Chief"),
            "instructions must survive"
        );
    }

    #[test]
    fn the_instructions_pin_the_shape_a_small_model_needs() {
        // A 3B model given "write a brief" writes an essay.
        assert!(INSTRUCTIONS.contains("At most five bullets"));
        assert!(INSTRUCTIONS.contains("Do not guess"));
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
        let prompt = assemble(&gathered(), PRESENT).expect("should assemble");

        assert_eq!(
            prompt
                .matches("## Assigned to you and not finished")
                .count(),
            1,
            "{prompt}"
        );
        assert!(prompt.contains("REC-42"), "{prompt}");
        assert!(prompt.contains("PROJ-7"), "{prompt}");
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
        };

        assert_eq!(describe_event(&event), "10:00 1:1 with Sam with Sam Patel");
    }

    // ---- end to end, against stub servers ----

    use crate::db::test_support::migrated_pool;
    use crate::llama::test_support::{delta, events, serve};

    /// What the stub engine writes back, in the shape `llama-server` streams
    /// it — the brief asks for a stream so that a slow answer is not mistaken
    /// for a stalled one. See `render`.
    fn answer() -> String {
        events(&[delta("- 10:00 with Sam\n- PR #12 is waiting")])
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
        };

        let found = gather(&context).await;

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

        work_log::insert(
            &pool,
            work_log::NewWorkLogEntry {
                source: "github".to_string(),
                content: "Merged the loopback listener".to_string(),
                summary: Some("Shipped the loopback listener".to_string()),
                timestamp: None,
                account_id: None,
                external_id: None,
            },
        )
        .await
        .expect("should log");

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

    #[tokio::test]
    async fn writes_the_brief_into_the_corpus_with_one_model_call() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", answer())]);
        let (context, _scratch) = ready(&host, "writes").await;

        let brief = daily_brief(&context).await.expect("should write a brief");

        assert!(brief.markdown.contains("10:00 with Sam"), "{brief:?}");
        assert!(brief.path.starts_with("briefs/"), "{brief:?}");

        // It is in the corpus, as a file the user can open.
        assert_eq!(
            context.corpus.read(&brief.path).await.expect("read"),
            brief.markdown
        );

        // Exactly one call. A recipe that needs two is two recipes.
        let requests = server.await.expect("the stub should finish");
        assert_eq!(requests.len(), 1, "one model call per recipe");
    }

    /// The defect REC-64 is about, guarded where it actually happened.
    ///
    /// `clock::today` is asserted in its own module, but nothing there stops
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
        assert_eq!(
            context.corpus.read(&again.path).await.expect("read"),
            "- a later brief",
            "a brief is what today looks like now, not a history of it"
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

        assert_eq!(
            context.corpus.read(&again.path).await.expect("read"),
            "- a later brief"
        );
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
        };

        let error = daily_brief(&context)
            .await
            .expect_err("nothing is connected");

        assert!(matches!(error, Error::NothingToSay), "{error:?}");
    }

    #[test]
    fn today_runs_midnight_to_midnight() {
        let (from, to) = today();

        assert!(from.contains("T00:00:00"), "{from}");
        assert!(to > from);
    }
}
