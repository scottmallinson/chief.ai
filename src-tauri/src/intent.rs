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
use crate::context::RETRIEVAL_CEILING;
use crate::facts;
use crate::ingest;
use crate::integrations;
use crate::people;
use crate::recipe;
use crate::retrieval;
use crate::tools;
use crate::work;

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
    /// What has been logged, over the stretch that was asked about.
    Log(Window),
    /// What somebody else is blocked on: the reviews requested of the user.
    ///
    /// A separate intent rather than a `Log` window, because it is a different
    /// question about different rows. REC-41 established that "waiting on you"
    /// and "your open work" are not the same thing and that conflating them was
    /// a wrong answer rather than a thin one; this is that distinction carried
    /// into the router.
    Waiting,
    /// Pull requests waiting on review in the repositories listed in
    /// `team.md`. Opt-in, and read live: it is somebody else's work, which the
    /// work log does not hold.
    Team,
    /// Today's meetings that include someone listed in `team.md`, each with
    /// what that person merged lately and has open. Opt-in, like [`Intent::Team`].
    OneOnOne,
    /// What is assigned to the user in whichever of GitHub, Jira and Linear
    /// they have connected, and how it is going. Read live, no model.
    Items,
}

/// How far back a question is asking about.
///
/// **The reason a window exists at all.** `Log` used to answer every phrasing
/// with the last ten rows whenever they happened, so "what did I ship this
/// week" and "what did I ship" got the same answer — which for somebody who
/// had shipped nothing this week was ten things from last month presented as
/// this week's work. A window is cheap here because `retrieval::in_window`
/// already takes one; nothing but the router was passing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// No window: the last few things, whenever they happened.
    Recent,
    Today,
    ThisWeek,
    LastWeek,
}

impl Window {
    /// The heading the answer is filed under, which has to match the window or
    /// the answer is mislabelled rather than merely wide.
    const fn heading(self) -> &'static str {
        match self {
            Self::Recent => "Recently logged",
            Self::Today => "Today",
            Self::ThisWeek => "This week",
            Self::LastWeek => "Last week",
        }
    }

    /// The half-open bounds, as the **UTC** instants the work log stores.
    ///
    /// `None` for [`Window::Recent`], which is the caller's signal to read the
    /// newest rows rather than a range.
    ///
    /// Weeks start on Monday, matching `clock::describe` — the model and the
    /// router must not disagree about which days "this week" covers.
    pub(crate) fn bounds<Tz: chrono::TimeZone>(
        self,
        now: &chrono::DateTime<Tz>,
    ) -> Option<(String, String)> {
        let today = now.date_naive();

        let (from, days) = match self {
            Self::Recent => return None,
            Self::Today => (today, 1),
            Self::ThisWeek => (monday_of(today), 7),
            Self::LastWeek => (monday_of(today) - chrono::Duration::days(7), 7),
        };

        Some(between(now, from, from + chrono::Duration::days(days)))
    }
}

/// The Monday of the week a date falls in.
fn monday_of(date: chrono::NaiveDate) -> chrono::NaiveDate {
    use chrono::Datelike;

    let since_monday = i64::from(date.weekday().num_days_from_monday());

    date - chrono::Duration::days(since_monday)
}

/// The slash commands, matched before anything else and matched exactly.
const COMMANDS: [(&str, Intent); 8] = [
    ("/brief", Intent::Brief),
    ("/prep", Intent::Prep),
    ("/log", Intent::Log(Window::Recent)),
    ("/week", Intent::Log(Window::ThisWeek)),
    ("/waiting", Intent::Waiting),
    ("/team", Intent::Team),
    ("/1on1", Intent::OneOnOne),
    ("/items", Intent::Items),
];

/// The natural-language phrases that route, written out in full.
///
/// Whole-question equality, not containment. Every phrase here is one somebody
/// would type meaning exactly this and nothing else; anything less certain is
/// deliberately left to the model, which has the context to tell the
/// difference and is allowed to be wrong out loud.
const PHRASES: [(&str, Intent); 34] = [
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
    ("my work log", Intent::Log(Window::Recent)),
    ("show me my work log", Intent::Log(Window::Recent)),
    ("what did i ship", Intent::Log(Window::Recent)),
    ("what have i shipped", Intent::Log(Window::Recent)),
    // The time-qualified forms. The composer offers the first of these, and
    // until now it did not match: the list held "what did i ship" and the
    // opener normalises to "what did i ship this week", so Chief's own
    // suggested question missed its own router and went to the network.
    ("what did i ship today", Intent::Log(Window::Today)),
    ("what have i shipped today", Intent::Log(Window::Today)),
    ("what did i ship this week", Intent::Log(Window::ThisWeek)),
    (
        "what have i shipped this week",
        Intent::Log(Window::ThisWeek),
    ),
    ("what did i ship last week", Intent::Log(Window::LastWeek)),
    (
        "what have i shipped last week",
        Intent::Log(Window::LastWeek),
    ),
    // "Waiting on me" is the reviews somebody has asked for, and it is the
    // other question the composer offers. It became answerable from this
    // machine when the pass started ingesting review requests.
    ("what is waiting on me", Intent::Waiting),
    ("whats waiting on me", Intent::Waiting),
    ("what is waiting on me right now", Intent::Waiting),
    ("who is waiting on me", Intent::Waiting),
    ("what needs my review", Intent::Waiting),
    ("what am i blocking", Intent::Waiting),
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

/// Which services feed a routed intent, so an empty answer can tell the two
/// empties apart.
///
/// "Nothing happened today" and "nothing is connected, so Chief has never had
/// anything to look at" read identically in the work log and mean opposite
/// things to the user. Only the second is a fact this module can state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Feeds {
    /// Outlook, or a calendar subscribed to by address.
    Calendar,
    /// GitHub, which is where pull requests and reviews come from.
    Github,
    /// GitHub, Jira or Linear: wherever assigned work lives.
    Work,
    /// Nothing in particular — the corpus, which is always there.
    Corpus,
}

impl Intent {
    /// What this intent reads from.
    const fn feeds(self) -> Feeds {
        match self {
            // The brief is a file on disk. An empty one is not a connection
            // problem and saying it were would be a wrong answer.
            Intent::Brief => Feeds::Corpus,
            Intent::Prep | Intent::OneOnOne => Feeds::Calendar,
            Intent::Log(_) | Intent::Waiting | Intent::Team => Feeds::Github,
            Intent::Items => Feeds::Work,
        }
    }
}

impl Feeds {
    /// Whether anything that feeds this is connected at all.
    async fn connected(self, pool: &sqlx::SqlitePool) -> bool {
        let services: &[&str] = match self {
            Feeds::Calendar => &[integrations::MICROSOFT, integrations::CALENDAR],
            Feeds::Github => &[integrations::GITHUB],
            Feeds::Work => &[
                integrations::GITHUB,
                integrations::ATLASSIAN,
                integrations::LINEAR,
            ],
            Feeds::Corpus => return true,
        };

        for service in services {
            match integrations::accounts(pool, service).await {
                Ok(accounts) if !accounts.is_empty() => return true,
                // A database that cannot be read is not evidence that nothing
                // is connected, so it steps aside rather than asserting.
                Err(_) => return true,
                Ok(_) => {}
            }
        }

        false
    }

    /// What to say when nothing that feeds this is connected.
    const fn nothing_connected(self) -> &'static str {
        match self {
            Feeds::Calendar => {
                "No calendar is connected, so Chief has nothing to read. Connect Outlook, or add \
                 a calendar subscription, in Settings."
            }
            Feeds::Github => {
                "No GitHub account is connected, so Chief has nothing to read. Connect GitHub in \
                 Settings."
            }
            Feeds::Work => {
                "Nothing that holds assigned work is connected. Connect GitHub, Jira or Linear in \
                 Settings."
            }
            Feeds::Corpus => "",
        }
    }
}

/// A question Chief knows the material for but cannot write itself.
///
/// **Deliberately not an [`Intent`].** Every `Intent` means "this is
/// answerable from local storage without a model", and the whole router is
/// built on that being true of all of them. A standup is the opposite shape:
/// the facts are on this machine, and turning them into something a person
/// would say out loud is exactly the part a model is for. Folding it into
/// `Intent` would have made one variant mean something different from every
/// other one, and `strategy_for` would have had to special-case it.
///
/// So this is a second, narrower recogniser feeding the tool loop rather than
/// bypassing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ground {
    /// "Draft my standup" and its neighbours.
    Standup,
}

/// The questions that get material put in front of them.
///
/// Whole-question matching, like [`PHRASES`], and for the same reason: a
/// keyword found somewhere inside a sentence is how "can you write up the
/// standup process for onboarding?" would silently become a standup draft.
const GROUNDED: &[(&str, Ground)] = &[
    ("draft my standup", Ground::Standup),
    ("draft my stand up", Ground::Standup),
    ("write my standup", Ground::Standup),
    ("write my stand up", Ground::Standup),
    ("my standup", Ground::Standup),
    ("standup", Ground::Standup),
    ("stand up", Ground::Standup),
    ("what is my standup", Ground::Standup),
    ("whats my standup", Ground::Standup),
];

/// Which grounded question this is, if it is one.
#[must_use]
pub fn ground(question: &str) -> Option<Ground> {
    let normalised = normalise(question);

    GROUNDED
        .iter()
        .find(|(phrase, _)| *phrase == normalised)
        .map(|(_, ground)| *ground)
}

/// One section of a standup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandupPart {
    pub heading: &'static str,
    pub lines: Vec<String>,
}

/// What a standup is made of, read just now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standup {
    /// The sections with something in them, in the order they are said.
    pub parts: Vec<StandupPart>,
    /// Whether any section is from the last sync because nothing that holds it
    /// could be asked. The answer says so rather than passing it off as current.
    pub from_log: bool,
    /// Whether something could not be read and there was no log to fall back
    /// on, which is a different statement from having nothing to report.
    pub unread: bool,
    /// Whether anything that holds the user's work is connected at all.
    pub connected: bool,
    /// The sources that answered, for the line under the answer.
    pub read: Vec<&'static str>,
    /// The sources that are connected and did not answer.
    pub missed: Vec<&'static str>,
}

/// The user's work for a standup, **live from whatever they have connected**.
///
/// The three sections used to come from the work log. Its rows only ever grow,
/// so a pull request that had since merged was still "open" and a review done
/// at nine was still "waiting" at noon — the stale lines in a standup. Each
/// section is now asked of every connected source that holds it (GitHub,
/// Linear, Jira), and only a section nothing could answer falls back to the
/// log, saying so.
///
/// **What is done reaches back to the last working day**, not seven days: a
/// standup is yesterday and today, and a week of merges is a long answer for a
/// small model to write out. On a Monday that is Friday, which is why it is not
/// simply yesterday.
pub async fn standup(context: &tools::Context) -> Standup {
    let now = chrono::Local::now();
    let since = work::last_working_day(now.date_naive());
    let read = work::read(
        context,
        &work::Scope {
            done_since: since,
            day: false,
            // A standup is written a few lines at a time, so it can be all of
            // them.
            limit: usize::MAX,
        },
    )
    .await;

    let (from, to) = between(&now, since, now.date_naive() + chrono::Duration::days(1));
    let mut standup = Standup {
        parts: Vec::new(),
        from_log: false,
        unread: false,
        connected: false,
        read: Vec::new(),
        missed: Vec::new(),
    };

    for (heading, live, stored) in [
        (
            "Done",
            read.done,
            retrieval::shipped_in_window(&context.pool, &from, &to, LOG_ENTRIES).await,
        ),
        (
            "Still open",
            read.open,
            retrieval::latest_in_category(&context.pool, ingest::IN_FLIGHT, LOG_ENTRIES).await,
        ),
        (
            "Waiting on my review",
            read.reviews,
            retrieval::latest_in_category(&context.pool, ingest::REVIEW, LOG_ENTRIES).await,
        ),
    ] {
        standup.connected |= !live.read.is_empty() || !live.unread.is_empty();

        for source in &live.read {
            if !standup.read.contains(source) {
                standup.read.push(source);
            }
        }

        for (source, _) in &live.unread {
            if !standup.missed.contains(source) {
                standup.missed.push(source);
            }
        }

        let lines = if live.read.is_empty() {
            let logged = stored
                .ok()
                .as_deref()
                .and_then(|hits| retrieval::to_context(hits, RETRIEVAL_CEILING))
                .map(|body| body.lines().map(str::to_string).collect::<Vec<_>>())
                .unwrap_or_default();

            if logged.is_empty() {
                standup.unread |= live.is_unread();
            } else {
                standup.from_log = true;
            }

            logged
        } else {
            live.lines
        };

        if !lines.is_empty() {
            standup.parts.push(StandupPart { heading, lines });
        }
    }

    standup
}

/// Answer a routed intent from local data, without a model.
///
/// `None` means there was nothing to say, and the question goes to the tool
/// loop after all. That is the safety valve on the whole idea: a routed intent
/// that would answer "you have nothing" is indistinguishable, to the reader,
/// from Chief having misunderstood them — so it steps aside and lets the model
/// answer instead. The cost of being wrong is one model call, which is what the
/// question cost before this module existed.
///
/// **With one exception, which is what REC-62 is about.** Stepping aside was
/// written as though the model were the safer answer. Measured in the running
/// app it is not: asked what was on a calendar with nothing connected, a 3B
/// model invented "[Outlook] Meeting with John Doe at 10:00 AM" — confident,
/// plausible and entirely fabricated, past a system prompt that tells it not
/// to. So when the source a question reads from is not connected *at all*,
/// that is said plainly instead. It is a fact about this machine rather than a
/// judgement about the user's day, and it names the thing that would fix it.
///
/// A source that *is* connected and simply has nothing still steps aside: that
/// is the ambiguous case the original reasoning was about, and it is untouched.
pub async fn answer(intent: Intent, context: &recipe::Context) -> Option<Answered> {
    let found = match intent {
        Intent::Brief => brief(context).await,
        Intent::Prep => prep(context).await,
        Intent::Log(window) => log(context, window).await,
        Intent::Waiting => waiting(context).await,
        Intent::Team => team(context).await,
        Intent::OneOnOne => one_on_one(context).await,
        Intent::Items => items(context).await,
    }
    .filter(|answered| !answered.markdown.trim().is_empty());

    if let Some(answered) = found {
        return Some(answered);
    }

    let feeds = intent.feeds();

    if feeds == Feeds::Corpus || feeds.connected(&context.pool).await {
        return None;
    }

    Some(Answered {
        markdown: feeds.nothing_connected().to_string(),
        // Nothing was read, so there is nothing to cite. A provenance line
        // here would be claiming a source for a statement about there being
        // none.
        provenance: None,
    })
}

/// An answer, and the line under it saying where it came from.
///
/// The provenance is **built in Rust from the rows**, never written by a model.
/// It is the only thing left telling the reader how fresh an answer is, now
/// that a read is a query rather than a call somebody else's API times — so it
/// is the one piece of text on the screen that has to be true, and a line the
/// model composed could be wrong in exactly the way nothing else would catch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    pub markdown: String,
    /// `None` when the answer used no stored rows. `/brief` reads a corpus
    /// file, so it has nothing local behind it and shows no footer at all
    /// rather than an empty one.
    pub provenance: Option<String>,
}

/// Today's brief, as written. Reading, never regenerating.
async fn brief(context: &recipe::Context) -> Option<Answered> {
    let path = format!("briefs/{}.md", recipe::today_date());

    // No provenance: this is a file the user can open, not rows Chief read.
    // Claiming a work log behind it would be a claim about where it came from
    // that happens not to be true.
    context
        .corpus
        .read(&path)
        .await
        .ok()
        .map(|markdown| Answered {
            markdown,
            provenance: None,
        })
}

/// Today's meetings, in the order they happen.
///
/// **Read from the work log, not from Microsoft Graph.** This function used to
/// call `session.events` while the user waited, which made "what is on my
/// calendar" — the DLE specification's own example of a read — a question that
/// left the machine and took as long as somebody else's API decided. D9 says a
/// read is answered from local storage; the daemon's ingestion pass is what
/// puts the meetings there.
async fn prep(context: &recipe::Context) -> Option<Answered> {
    let (from, to) = day_window(&chrono::Local::now());

    let hits = retrieval::in_window(
        &context.pool,
        &from,
        &to,
        Some("calendar"),
        recipe::CALENDAR_LIMIT.into(),
    )
    .await
    .ok()?;

    // A meeting held before says so, and what was shipped in between. Each is
    // a small read of the log; a read that fails costs that meeting its note.
    let zone = chrono::Local;
    let mut notes = Vec::with_capacity(hits.len());
    for hit in &hits {
        let since = retrieval::since_last(&context.pool, &hit.title, &from, &to)
            .await
            .ok()
            .flatten();

        let history = since.and_then(|since| retrieval::note(&since, &zone));
        let attendees = attendee_notes(context, hit).await;

        notes.push(match (history, attendees) {
            (Some(history), Some(attendees)) => Some(format!("{history}\n{attendees}")),
            (history, attendees) => history.or(attendees),
        });
    }

    retrieval::to_context_with_notes(&hits, &notes, RETRIEVAL_CEILING).map(|body| Answered {
        markdown: format!("## Today\n{body}"),
        provenance: retrieval::provenance(&hits),
    })
}

/// The most attendees a meeting spends lines on.
const ATTENDEE_LINES: usize = 3;

/// What the facts say about the people a meeting names, for those the user has
/// confirmed in `people.md`.
///
/// Read from the corpus and the facts table, never the network, so `/prep`
/// stays free. Somebody who is not confirmed, or whom the facts hold nothing
/// about, gets no line: a name beside a stranger's review count is worse than
/// no name. A file that cannot be read costs the meeting these lines only.
async fn attendee_notes(context: &recipe::Context, hit: &retrieval::Hit) -> Option<String> {
    let text = context.corpus.read(people::FILE).await.ok()?;
    let people = people::parse(&text);
    let meeting = format!(
        "{} {}",
        hit.title,
        hit.summary.as_deref().unwrap_or_default()
    );
    let today = chrono::Local::now().date_naive();

    let mut lines = Vec::new();
    for person in people::attending(&people, &meeting) {
        let Ok(Some(reviewer)) = facts::reviewer(&context.pool, &person.handle).await else {
            continue;
        };

        lines.push(facts::reviewer_line(
            &person.name,
            &person.handle,
            &reviewer,
            today,
        ));
    }

    if lines.len() > ATTENDEE_LINES {
        let more = lines.len() - ATTENDEE_LINES;
        lines.truncate(ATTENDEE_LINES);
        lines.push(format!("  and {more} more people you have confirmed."));
    }

    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Local midnight today and tomorrow, as the **UTC** instants the work log
/// stores.
///
/// `recipe::today` produces local wall-clock strings, and `work_logs.timestamp`
/// is written by SQLite's `strftime('%Y-%m-%dT%H:%M:%fZ', 'now')`, which is
/// UTC. Comparing one against the other directly reads the wrong day by
/// whatever the offset is — invisible in London in winter and wrong by a day's
/// edge everywhere else, which is exactly the kind of bug that only appears
/// for somebody else.
///
/// Generic over the time zone, so the conversion is tested at a fixed offset
/// rather than against whatever clock the test machine keeps — the same reason
/// `clock::describe` is.
pub(crate) fn day_window<Tz: chrono::TimeZone>(now: &chrono::DateTime<Tz>) -> (String, String) {
    let today = now.date_naive();

    between(now, today, today + chrono::Duration::days(1))
}

/// Two local dates as the half-open pair of UTC instants between them.
///
/// Shared by [`day_window`] and [`Window::bounds`] so a day and a week are
/// converted the same way. Getting this wrong is invisible in London in winter
/// and wrong at every boundary everywhere else.
fn between<Tz: chrono::TimeZone>(
    now: &chrono::DateTime<Tz>,
    from: chrono::NaiveDate,
    to: chrono::NaiveDate,
) -> (String, String) {
    let zone = now.timezone();

    // A local midnight can be ambiguous or absent on the day a clock changes.
    // `earliest` takes the first instant that exists, and the fallback treats
    // the naive time as UTC rather than refusing to answer at all: being an
    // hour out once a year beats showing nothing.
    let as_utc = |date: chrono::NaiveDate| {
        let naive = date.and_hms_opt(0, 0, 0).unwrap_or_default();

        zone.from_local_datetime(&naive)
            .earliest()
            .map_or_else(
                || naive.and_utc(),
                |local| local.with_timezone(&chrono::Utc),
            )
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string()
    };

    (as_utc(from), as_utc(to))
}

/// What the daemon has logged lately, newest first.
///
/// Reads the same structured rows `prep` does, so a logged item is described
/// the same way wherever it appears — and, like `prep`, touches nothing but
/// this machine's own disk.
async fn log(context: &recipe::Context, window: Window) -> Option<Answered> {
    // **Shipped work only.** "What did I ship" is a question about what
    // landed, and once the pass started reading the user's open pull requests
    // too, an unfiltered read answered it with work in flight — measured
    // against the real repository, an open pull request was reported as that
    // day's shipped work. `ingest::SHIPPED` is what merged rows are filed
    // under, and what every row written before open ones existed already is.
    let hits = match window.bounds(&chrono::Local::now()) {
        Some((from, to)) => retrieval::shipped_in_window(&context.pool, &from, &to, LOG_ENTRIES)
            .await
            .ok()?,
        None => retrieval::shipped_latest(&context.pool, LOG_ENTRIES)
            .await
            .ok()?,
    };

    retrieval::to_context(&hits, RETRIEVAL_CEILING).map(|body| Answered {
        markdown: format!("## {}\n{body}", window.heading()),
        provenance: retrieval::provenance(&hits),
    })
}

/// The reviews somebody has asked the user for.
///
/// **Read from GitHub as it is asked, not from the work log.** A review request
/// closes the moment somebody reviews, merges or withdraws it, and nothing in
/// the log hears about that: the rows only ever grow, so a request dealt with
/// at nine still read as waiting at noon. This is one search per connected
/// account — the same call the tool makes — and no model is involved.
///
/// **The log is the fallback, and says so.** With GitHub out of reach, the last
/// sync is better than nothing, but it is not "just now", and the line under
/// the answer stops claiming it is.
///
/// No window. A review request is open until somebody deals with it, and one
/// from three weeks ago is more of a problem than one from this morning, not
/// less.
async fn waiting(context: &recipe::Context) -> Option<Answered> {
    match tools::fetch_github_prs(
        &context.pool,
        &context.github,
        tools::PullRequestState::Open,
        tools::Whose::Reviewing,
        None,
    )
    .await
    {
        Ok(read) => live_reviews(&read),
        Err(error) => {
            eprintln!("could not read review requests live: {error}");
            stored_reviews(context).await
        }
    }
}

/// What is waiting on review in the repositories the user listed.
async fn team(context: &recipe::Context) -> Option<Answered> {
    crate::team::read(context).await.map(|report| Answered {
        markdown: report.markdown,
        provenance: report.provenance,
    })
}

/// What is assigned to the user across the trackers they have connected.
async fn items(context: &recipe::Context) -> Option<Answered> {
    crate::items::read(context).await.map(|report| Answered {
        markdown: report.markdown,
        provenance: report.provenance,
    })
}

/// A sheet for each of today's meetings that includes somebody the user manages.
///
/// The meetings are read from the work log, like `/prep`; `None` when there are
/// none, so a machine with no calendar is told that.
async fn one_on_one(context: &recipe::Context) -> Option<Answered> {
    let (from, to) = day_window(&chrono::Local::now());
    let hits = retrieval::in_window(
        &context.pool,
        &from,
        &to,
        Some("calendar"),
        recipe::CALENDAR_LIMIT.into(),
    )
    .await
    .ok()?;

    if hits.is_empty() {
        return None;
    }

    let report = crate::team::one_on_ones(context, &hits, chrono::Utc::now()).await;

    Some(Answered {
        markdown: report.markdown,
        provenance: report.provenance,
    })
}

/// The live read, as lines. `None` for an empty one, like the stored read.
fn live_reviews(read: &serde_json::Value) -> Option<Answered> {
    let lines: Vec<String> = read["pull_requests"]
        .as_array()?
        .iter()
        .take(usize::try_from(LOG_ENTRIES).unwrap_or(usize::MAX))
        .map(|pr| {
            format!(
                "- {} ({}#{}, {})",
                pr["title"].as_str().unwrap_or("Untitled"),
                pr["repository"].as_str().unwrap_or("unknown"),
                pr["number"],
                pr["account"].as_str().unwrap_or("GitHub"),
            )
        })
        .collect();

    (!lines.is_empty()).then(|| Answered {
        markdown: format!("## Waiting on you\n{}", lines.join("\n")),
        provenance: Some("Read from GitHub just now.".to_string()),
    })
}

/// What the last sync knew, when GitHub could not be asked.
async fn stored_reviews(context: &recipe::Context) -> Option<Answered> {
    let hits = retrieval::latest_in_category(&context.pool, "review", LOG_ENTRIES)
        .await
        .ok()?;

    retrieval::to_context(&hits, RETRIEVAL_CEILING).map(|body| Answered {
        markdown: format!("## Waiting on you\n{body}"),
        provenance: retrieval::provenance(&hits)
            .map(|line| format!("GitHub could not be reached, so this may be out of date. {line}")),
    })
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
) -> Option<Answered>
where
    F: FnMut(Update),
{
    let answered = answer(route(question)?, context).await?;

    // The body streams; the footer does not. It is appended once the answer is
    // whole, which is also why it costs no prefill and is not measured against
    // `RETRIEVAL_CEILING` — it was never in a prompt.
    on_update(Update::Delta {
        text: answered.markdown.clone(),
    });

    Some(answered)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every phrase a user might type meaning one of these, and the route it
    /// must take. Failing here is a question silently answered wrongly.
    /// The questions the composer offers, read from the file it renders them
    /// from.
    ///
    /// **One list, two languages.** `ChatView` used to hold its own array of
    /// opener strings, and all three of them missed this router: "What did I
    /// ship this week?" normalises to a phrase the list did not have, so the
    /// one screen that suggests what to ask suggested three things the router
    /// was built to catch and caught none of. A test cannot check a list it
    /// cannot see, so the list moved to a file both sides read.
    const OPENERS: &str = include_str!("../../src/lib/openers.json");

    /// The guard REC-58 exists for.
    ///
    /// Proved by removing "what did i ship this week" from `PHRASES`:
    ///
    /// ```text
    /// the composer offers "What did I ship this week?" as something Chief
    /// answers from this machine, and the router does not recognise it
    ///   left: false
    ///  right: true
    /// ```
    ///
    /// Which is the reported defect exactly: a flagship read going to the
    /// network and coming back as prose a 3B model invented over a page of
    /// search results.
    #[test]
    fn every_question_the_composer_offers_routes_as_it_claims_to() {
        let openers: Vec<serde_json::Value> =
            serde_json::from_str(OPENERS).expect("the openers file should be a list");

        assert!(
            !openers.is_empty(),
            "an empty list would pass every assertion below without checking anything"
        );

        for opener in openers {
            let question = opener["question"]
                .as_str()
                .expect("each opener is a question");
            let routed = opener["routed"]
                .as_bool()
                .expect("each opener says whether it routes");

            assert!(
                route(question).is_some() || ground(question).is_some(),
                "the composer offers {question:?}, so Chief has to have a plan for it — either \
                 the router answers it from local data or `GROUNDED` puts the material in front \
                 of the model. Neither does, so it reaches a 3B model with nothing but its own \
                 words, which is how REC-62's invented standup happened"
            );

            assert_eq!(
                route(question).is_some(),
                routed,
                "the composer offers {question:?} as something Chief answers from \
                 this machine, and the router does not recognise it"
            );
        }
    }

    #[test]
    fn routes_every_slash_command() {
        assert_eq!(route("/brief"), Some(Intent::Brief));
        assert_eq!(route("/prep"), Some(Intent::Prep));
        assert_eq!(route("/log"), Some(Intent::Log(Window::Recent)));
        assert_eq!(route("/week"), Some(Intent::Log(Window::ThisWeek)));
        assert_eq!(route("/waiting"), Some(Intent::Waiting));
        assert_eq!(route("/team"), Some(Intent::Team));
        assert_eq!(route("/1on1"), Some(Intent::OneOnOne));
        assert_eq!(route("/items"), Some(Intent::Items));
    }

    #[test]
    fn routes_a_command_whatever_follows_it() {
        assert_eq!(route("/brief please"), Some(Intent::Brief));
        assert_eq!(
            route("/log for this week"),
            Some(Intent::Log(Window::Recent))
        );
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
        assert_eq!(route("What did I ship?"), Some(Intent::Log(Window::Recent)));
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
            // Added with the phrases below them. A longer list is more surface
            // for a false positive, not less, and a false positive silently
            // replaces somebody's question with a canned answer.
            "what did I ship this week compared with last?",
            "who is waiting on me to approve their expenses?",
            "what needs my review by Friday?",
            "am I blocking anyone on the design?",
            "what did the team ship this week?",
            "what is waiting on me in Jira?",
        ] {
            assert_eq!(route(asked), None, "{asked} must reach the model");
        }
    }

    /// A time-qualified read is answered about the time it asked for.
    ///
    /// Proved by pointing every phrase at `Window::Recent`:
    ///
    /// ```text
    /// "what did i ship this week" must be answered about this week
    ///   left: Log(Recent)
    ///  right: Log(ThisWeek)
    /// ```
    ///
    /// Which is not a cosmetic difference: for somebody who shipped nothing
    /// this week, the last ten rows whenever they happened are last month's
    /// work presented as this week's.
    #[test]
    fn a_question_about_a_stretch_of_time_carries_that_stretch() {
        for (asked, expected) in [
            ("what did i ship", Window::Recent),
            ("what did i ship today", Window::Today),
            ("what did i ship this week", Window::ThisWeek),
            ("what did i ship last week", Window::LastWeek),
        ] {
            assert_eq!(
                route(asked),
                Some(Intent::Log(expected)),
                "{asked:?} must be answered about {expected:?}"
            );
        }
    }

    /// The windows, at a fixed offset rather than the test machine's clock.
    ///
    /// Wednesday 2 September 2026 at 14:00, five hours behind UTC — an offset
    /// chosen so a local midnight is a *different date* in UTC, which is the
    /// case a naive conversion gets wrong.
    #[test]
    fn a_week_is_monday_to_monday_in_the_users_own_zone() {
        use chrono::TimeZone;

        let zone = chrono::FixedOffset::west_opt(5 * 3600).expect("a real offset");
        let now = zone
            .with_ymd_and_hms(2026, 9, 2, 14, 0, 0)
            .single()
            .expect("a real instant");

        assert_eq!(Window::Recent.bounds(&now), None, "recent has no window");

        let (from, to) = Window::Today.bounds(&now).expect("today has one");
        assert_eq!(from, "2026-09-02T05:00:00.000Z");
        assert_eq!(to, "2026-09-03T05:00:00.000Z");

        // Wednesday's week starts on Monday the 31st of August.
        let (from, to) = Window::ThisWeek.bounds(&now).expect("this week has one");
        assert_eq!(from, "2026-08-31T05:00:00.000Z");
        assert_eq!(to, "2026-09-07T05:00:00.000Z");

        let (from, to) = Window::LastWeek.bounds(&now).expect("last week has one");
        assert_eq!(from, "2026-08-24T05:00:00.000Z");
        assert_eq!(to, "2026-08-31T05:00:00.000Z");
    }

    /// A Monday is its own Monday, which is the boundary a `- days` gets wrong.
    #[test]
    fn a_week_asked_about_on_a_monday_starts_that_morning() {
        use chrono::TimeZone;

        let zone = chrono::FixedOffset::east_opt(0).expect("a real offset");
        let monday = zone
            .with_ymd_and_hms(2026, 8, 31, 9, 0, 0)
            .single()
            .expect("a real instant");

        let (from, _) = Window::ThisWeek.bounds(&monday).expect("this week has one");

        assert_eq!(from, "2026-08-31T00:00:00.000Z");
    }

    #[test]
    fn refuses_a_command_that_is_only_part_of_a_word() {
        assert_eq!(route("/briefing"), None);
        assert_eq!(route("/logout"), None);
    }

    /// The local day converted to the UTC window the work log is stored in.
    ///
    /// Tested at a fixed offset rather than against the machine's own clock,
    /// which is the same reason `clock::describe` is generic: a test that
    /// passes only in UTC proves nothing for anybody east or west of it.
    #[test]
    fn a_local_day_becomes_the_utc_window_the_log_is_stored_in() {
        // UTC+10: local midnight is 14:00 the previous day in UTC.
        let east = chrono::FixedOffset::east_opt(10 * 3600).expect("a real offset");
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-02T09:00:00+10:00")
            .expect("a real instant")
            .with_timezone(&east);

        let (from, to) = day_window(&now);

        assert_eq!(from, "2026-09-01T14:00:00.000Z");
        assert_eq!(to, "2026-09-02T14:00:00.000Z");
    }

    /// And the other direction, so the test cannot pass by ignoring the offset.
    #[test]
    fn a_western_offset_shifts_the_window_the_other_way() {
        let west = chrono::FixedOffset::west_opt(7 * 3600).expect("a real offset");
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-02T09:00:00-07:00")
            .expect("a real instant")
            .with_timezone(&west);

        let (from, to) = day_window(&now);

        assert_eq!(from, "2026-09-02T07:00:00.000Z");
        assert_eq!(to, "2026-09-03T07:00:00.000Z");
    }

    #[test]
    fn refuses_an_empty_question() {
        assert_eq!(route(""), None);
        assert_eq!(route("   "), None);
    }
}

#[cfg(test)]
mod delivery {
    use super::*;
    use crate::corpus::Corpus;
    use crate::db::test_support::migrated_pool;
    use crate::llama::test_support::serve;
    use crate::work_log;
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
            atlassian: crate::atlassian::Client::new().expect("client"),
            atlassian_rest: crate::atlassian::rest::Rest::new().expect("client"),
            engine: llama::Client::with_base_url(&host).expect("client"),
            corpus,
            short_brief_lines: 0,
        };

        (context, Scratch(root), server)
    }

    /// Connect an account for `service`, so a routed intent that finds nothing
    /// is the ambiguous empty rather than the stateable one.
    async fn connect(context: &recipe::Context, service: &str) {
        integrations::save(
            &context.pool,
            integrations::NewAccount {
                service,
                account_key: "someone",
                identity: Some("someone"),
                credential_kind: integrations::OAUTH,
                access_token: "token",
                refresh_token: None,
                expires_at: None,
                scopes: None,
                client_id: None,
                client_secret: None,
            },
        )
        .await
        .expect("should store a credential");
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
                text: "## Recently logged\n- [GitHub] Shipped the release workflow".to_string()
            }]
        );
        assert_eq!(
            server.await.expect("server"),
            Vec::<String>::new(),
            "a routed intent must cost zero model calls"
        );
    }

    /// `/prep` used to call Microsoft Graph while the user waited. It must not.
    ///
    /// The Microsoft client points at `127.0.0.1:1`, where nothing listens, so
    /// **a call would fail and `prep` would return `None`** — an answer here is
    /// therefore proof no call was made. The two assertions before it are what
    /// stop that being vacuous: without a stored meeting `prep` returns `None`
    /// for the innocent reason too, and the test would pass while proving
    /// nothing.
    ///
    /// Proved by pointing `prep` back at `session.events` and watching it fail:
    ///
    /// ```text
    /// /prep must answer from the work log, not from Graph
    /// ```
    #[tokio::test]
    async fn prep_answers_from_the_work_log_rather_than_the_network() {
        let (context, _scratch, server) = silent("prep").await;

        let (from, _to) = day_window(&chrono::Local::now());
        let mut meeting = work_log::WorkLogRecord {
            timestamp: from,
            source: "calendar".to_string(),
            category: "calendar".to_string(),
            title: "Standup".to_string(),
            content: "Standup".to_string(),
            summary: Some("09:30, with Dana".to_string()),
            url: None,
            raw_ref: None,
            external_id: "cal:1".to_string(),
            account_id: 1,
        };
        // Nudged past local midnight so it lands inside today rather than on
        // the boundary the window excludes at the far end.
        meeting.timestamp = bump(&meeting.timestamp);

        work_log::upsert(&context.pool, meeting)
            .await
            .expect("should store");

        let stored = retrieval::latest(&context.pool, 10).await.expect("read");
        assert_eq!(stored.len(), 1, "the fixture must have stored a meeting");

        let answered = answer(Intent::Prep, &context).await;

        assert!(
            answered.is_some(),
            "/prep must answer from the work log, not from Graph"
        );
        assert!(
            answered
                .as_ref()
                .is_some_and(|answered| answered.markdown.contains("Standup")),
            "the answer should be the stored meeting: {answered:?}"
        );
        assert_eq!(
            server.await.expect("server"),
            Vec::<String>::new(),
            "and it must cost zero model calls"
        );
    }

    /// A meeting held before says when, and what was shipped since, and still
    /// costs no model call.
    ///
    /// Proved by dropping the note from `prep` and watching it fail:
    ///
    /// ```text
    /// the recurring meeting should say when it was last held
    /// ```
    #[tokio::test]
    async fn prep_says_what_happened_since_a_recurring_meeting_last_ran() {
        let (context, _scratch, server) = silent("prep-history").await;

        let (from, _to) = day_window(&chrono::Local::now());
        let at = |stamp: &str, id: &str, category: &str, title: &str| work_log::WorkLogRecord {
            timestamp: stamp.to_string(),
            source: if category == "calendar" {
                "calendar"
            } else {
                "github"
            }
            .to_string(),
            category: category.to_string(),
            title: title.to_string(),
            content: title.to_string(),
            summary: Some("09:30".to_string()),
            url: None,
            raw_ref: None,
            external_id: id.to_string(),
            account_id: 1,
        };

        // Two days before the window opens: held then, and one thing shipped
        // after it.
        let earlier = chrono::DateTime::parse_from_rfc3339(&from)
            .expect("a day boundary")
            .checked_sub_signed(chrono::Duration::days(2))
            .expect("in range")
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        let after = bump(&earlier);

        for entry in [
            at(&earlier, "cal:old", "calendar", "1:1 with Dana"),
            at(&after, "o/r#9", "pr", "o/r #9: Retry the sync"),
            at(&bump(&from), "cal:new", "calendar", "1:1 with Dana"),
        ] {
            work_log::upsert(&context.pool, entry)
                .await
                .expect("should store");
        }

        let answered = answer(Intent::Prep, &context)
            .await
            .expect("today has a meeting");

        assert!(
            answered.markdown.contains("Last held"),
            "the recurring meeting should say when it was last held: {}",
            answered.markdown
        );
        assert!(
            answered.markdown.contains("o/r #9: Retry the sync"),
            "and what was shipped since: {}",
            answered.markdown
        );
        assert_eq!(
            answered.markdown.matches("Last held").count(),
            1,
            "only the meeting that has history gets a note: {}",
            answered.markdown
        );
        assert_eq!(
            server.await.expect("server"),
            Vec::<String>::new(),
            "and it must cost zero model calls"
        );
    }

    /// A meeting names the people the user has confirmed, and only them.
    ///
    /// Proved by reading `Suggested` lines as confirmed, which names Priya:
    ///
    /// ```text
    /// somebody the user has not confirmed must not be named
    /// ```
    #[tokio::test]
    async fn prep_names_attendees_the_user_confirmed_and_nobody_else() {
        let (context, _scratch, server) = silent("prep-people").await;

        let (from, _to) = day_window(&chrono::Local::now());
        work_log::upsert(
            &context.pool,
            work_log::WorkLogRecord {
                timestamp: bump(&from),
                source: "calendar".to_string(),
                category: "calendar".to_string(),
                title: "Design sync".to_string(),
                content: "10:00 Design sync with Dana Vance, Priya Rao".to_string(),
                summary: Some("10:00 with Dana Vance, Priya Rao".to_string()),
                url: None,
                raw_ref: None,
                external_id: "cal:design".to_string(),
                account_id: 1,
            },
        )
        .await
        .expect("should store");

        let row = |about: &str| facts::NewFact {
            about: about.to_string(),
            what: facts::REVIEWS.to_string(),
            scope: "o/r".to_string(),
            value: String::new(),
            count: 7,
            total: 30,
            source: "GitHub".to_string(),
            seen_on: chrono::Local::now().format("%Y-%m-%d").to_string(),
        };
        facts::recount(
            &context.pool,
            &[facts::REVIEWS],
            vec![row("dvance"), row("prao")],
        )
        .await
        .expect("facts");

        context
            .corpus
            .write(
                people::FILE,
                "## Confirmed\n- Dana Vance = dvance\n\n## Suggested\n- Priya Rao = prao\n",
            )
            .await
            .expect("file");

        let answered = answer(Intent::Prep, &context).await.expect("a meeting");

        assert!(
            answered
                .markdown
                .contains("Dana Vance (@dvance) reviewed 7 of your last 30 pull requests"),
            "a confirmed attendee should be described: {}",
            answered.markdown
        );
        assert!(
            !answered.markdown.contains("prao"),
            "somebody the user has not confirmed must not be named: {}",
            answered.markdown
        );
        assert_eq!(
            server.await.expect("server"),
            Vec::<String>::new(),
            "and it must cost zero model calls"
        );
    }

    const TEAM_ANSWER: &str = r#"{"data":{"search":{"nodes":[
      {"number":12,"title":"Retry the sync","url":"u","createdAt":"2026-10-20T09:00:00Z",
       "isDraft":false,"reviewDecision":"REVIEW_REQUIRED","author":{"login":"dvance"},
       "repository":{"nameWithOwner":"acme/api"},"reviews":{"totalCount":0},
       "reviewRequests":{"nodes":[{"requestedReviewer":{"login":"prao"}}]}}
    ]}}}"#;

    /// Nothing is read from any repository until the user has listed one.
    ///
    /// Proved by reading the repository named in the starter's commented
    /// example as if it were listed:
    ///
    /// ```text
    /// an unlisted repository must cost no request
    /// ```
    #[tokio::test]
    async fn team_reads_nothing_until_the_user_lists_a_repository() {
        let (mut context, _scratch, _engine) = silent("team-off").await;
        // A spare reply that is never fetched, so a request would show.
        let (host, github) = serve(vec![("HTTP/1.1 200 OK", TEAM_ANSWER)]);
        context.github = github::Client::against(&host).expect("client");
        connect(&context, integrations::GITHUB).await;

        let first = answer(Intent::Team, &context).await.expect("an answer");
        assert!(
            first.markdown.contains("The team view is off"),
            "{}",
            first.markdown
        );
        assert!(
            context.corpus.read(crate::team::FILE).await.is_ok(),
            "the file is created so there is something to open"
        );

        // The file now holds only the commented example.
        let second = answer(Intent::Team, &context).await.expect("an answer");
        assert!(second.markdown.contains("The team view is off"));
        assert!(
            !github.is_finished(),
            "an unlisted repository must cost no request"
        );
    }

    const LINEAR_ITEMS: &str = r#"{"data":{"viewer":{"name":"S","email":"s@example.com"},
      "issues":{"nodes":[
       {"identifier":"REC-50","title":"Ship the report","priority":1,"dueDate":"2020-01-01",
        "state":{"name":"In Progress","type":"started"},
        "inverseRelations":{"nodes":[]},
        "attachments":{"nodes":[{"url":"https://github.com/o/r/pull/131"}]}},
       {"identifier":"REC-51","title":"Plan the quarter","priority":0,
        "state":{"name":"Todo","type":"unstarted"},
        "inverseRelations":{"nodes":[{"type":"blocks","issue":
          {"identifier":"REC-49","completedAt":null,"canceledAt":null}}]},
        "attachments":{"nodes":[]}}
      ]}}}"#;

    #[tokio::test]
    async fn items_reports_what_is_assigned_and_why_it_is_stuck_without_a_model() {
        let (mut context, _scratch, engine) = silent("items").await;
        let (linear_host, _linear) = serve(vec![("HTTP/1.1 200 OK", LINEAR_ITEMS)]);
        let (github_host, github) = serve(vec![
            ("HTTP/1.1 200 OK", r#"{"total_count":0,"items":[]}"#),
            (
                "HTTP/1.1 200 OK",
                r#"{"data":{"viewer":{"pullRequests":{"nodes":[{"number":131,"isDraft":false,
                  "repository":{"nameWithOwner":"o/r"},"reviewDecision":"CHANGES_REQUESTED",
                  "commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"FAILURE"}}}]},
                  "reviewThreads":{"nodes":[]}}]}}}}"#,
            ),
            (
                "HTTP/1.1 200 OK",
                r#"{"total_count":1,"items":[{"number":131,"title":"Ship the report","state":"open",
                  "draft":false,"html_url":"https://github.com/o/r/pull/131",
                  "repository_url":"https://api.github.com/repos/o/r",
                  "updated_at":"2026-10-27T09:00:00Z"}]}"#,
            ),
        ]);
        context.linear = crate::linear::Client::against(&linear_host).expect("client");
        context.github = github::Client::against(&github_host).expect("client");
        connect(&context, integrations::LINEAR).await;
        connect(&context, integrations::GITHUB).await;

        let answered = answer(Intent::Items, &context).await.expect("an answer");

        // The pull request is held up, so the issue it belongs to is blocked
        // even though Linear itself lists no blocker for it.
        assert!(
            answered.markdown.contains("## Blocked (2)")
                && answered
                    .markdown
                    .contains("Pull requests: o/r#131 (checks failing, changes requested).")
                && answered.markdown.contains("Blocked by REC-49."),
            "{}",
            answered.markdown
        );
        assert!(
            !answered.markdown.contains("tied to no item"),
            "{}",
            answered.markdown
        );
        assert_eq!(
            answered.provenance.as_deref(),
            Some("Read just now: Linear (2), GitHub (0).")
        );
        assert_eq!(github.await.expect("github").len(), 3);
        assert_eq!(
            engine.await.expect("engine"),
            Vec::<String>::new(),
            "and it must cost zero model calls"
        );
    }

    #[tokio::test]
    async fn items_says_so_when_nothing_that_holds_work_is_connected() {
        let (context, _scratch, _engine) = silent("items-none").await;

        let answered = answer(Intent::Items, &context).await.expect("an answer");

        assert!(
            answered.markdown.contains("Connect GitHub, Jira or Linear"),
            "{}",
            answered.markdown
        );
    }

    #[tokio::test]
    async fn items_still_answers_from_linear_when_github_cannot_be_read() {
        let (mut context, _scratch, _engine) = silent("items-partial").await;
        let (linear_host, _linear) = serve(vec![("HTTP/1.1 200 OK", LINEAR_ITEMS)]);
        context.linear = crate::linear::Client::against(&linear_host).expect("client");
        connect(&context, integrations::LINEAR).await;
        // GitHub is connected but its host does not answer.
        connect(&context, integrations::GITHUB).await;

        let answered = answer(Intent::Items, &context).await.expect("an answer");

        assert!(
            answered.markdown.contains("REC-50"),
            "{}",
            answered.markdown
        );
        assert!(
            answered.markdown.contains("GitHub could not be read"),
            "{}",
            answered.markdown
        );
    }

    #[tokio::test]
    async fn team_lists_what_is_waiting_in_a_repository_the_user_listed() {
        let (mut context, _scratch, engine) = silent("team-on").await;
        let (host, github) = serve(vec![
            ("HTTP/1.1 200 OK", TEAM_ANSWER),
            ("HTTP/1.1 200 OK", TEAM_ANSWER),
        ]);
        context.github = github::Client::against(&host).expect("client");
        connect(&context, integrations::GITHUB).await;
        context
            .corpus
            .write(
                crate::team::FILE,
                "## People\n- Dana Vance = dvance\n\n## Repositories\n- acme/api\n",
            )
            .await
            .expect("file");

        let answered = answer(Intent::Team, &context).await.expect("an answer");

        assert!(
            answered.markdown.contains("### Dana Vance (@dvance)")
                && answered
                    .markdown
                    .contains("- acme/api#12 Retry the sync: open ")
                && answered.markdown.contains("### acme/api")
                && answered
                    .markdown
                    .contains("- #12 Retry the sync: Dana Vance (@dvance), open ")
                && answered.markdown.contains("review requested from @prao"),
            "{}",
            answered.markdown
        );
        assert!(answered.provenance.is_some());
        assert_eq!(
            github.await.expect("github").len(),
            2,
            "one request for the person and one for the repository"
        );
        assert_eq!(
            engine.await.expect("engine"),
            Vec::<String>::new(),
            "and it must cost zero model calls"
        );
    }

    const MERGED_ANSWER: &str = r#"{"data":{"search":{"nodes":[
      {"number":30,"title":"Cache the calendar","url":"u","createdAt":"2026-10-01T09:00:00Z",
       "mergedAt":"2026-10-26T09:00:00Z","isDraft":false,"reviewDecision":"APPROVED",
       "author":{"login":"dvance"},"repository":{"nameWithOwner":"acme/web"},
       "reviews":{"totalCount":1},"reviewRequests":{"nodes":[]}}
    ]}}}"#;

    async fn meeting(context: &recipe::Context, title: &str) {
        let (from, _to) = day_window(&chrono::Local::now());
        work_log::upsert(
            &context.pool,
            work_log::WorkLogRecord {
                timestamp: bump(&from),
                source: "calendar".to_string(),
                category: "calendar".to_string(),
                title: title.to_string(),
                content: format!("10:00 {title}"),
                summary: Some("10:00".to_string()),
                url: None,
                raw_ref: None,
                external_id: format!("cal:{title}"),
                account_id: 1,
            },
        )
        .await
        .expect("should store");
    }

    #[tokio::test]
    async fn one_on_one_shows_what_a_listed_person_merged_and_has_open() {
        let (mut context, _scratch, engine) = silent("one-on-one").await;
        let (host, github) = serve(vec![
            ("HTTP/1.1 200 OK", MERGED_ANSWER),
            ("HTTP/1.1 200 OK", TEAM_ANSWER),
        ]);
        context.github = github::Client::against(&host).expect("client");
        connect(&context, integrations::GITHUB).await;
        meeting(&context, "1:1 with Dana Vance").await;
        context
            .corpus
            .write(crate::team::FILE, "## People\n- Dana Vance = dvance\n")
            .await
            .expect("file");

        let answered = answer(Intent::OneOnOne, &context)
            .await
            .expect("a meeting with a listed person");

        assert!(
            answered.markdown.contains("### ")
                && answered.markdown.contains("1:1 with Dana Vance")
                && answered.markdown.contains("Dana Vance (@dvance)")
                && answered
                    .markdown
                    .contains("Merged in the last 14 days (1):")
                && answered
                    .markdown
                    .contains("- acme/web#30 Cache the calendar, merged ")
                && answered.markdown.contains("Waiting for review (1):")
                && answered
                    .markdown
                    .contains("- acme/api#12 Retry the sync: open "),
            "{}",
            answered.markdown
        );
        assert_eq!(
            github.await.expect("github").len(),
            2,
            "one request for what they merged and one for what is open"
        );
        assert_eq!(
            engine.await.expect("engine"),
            Vec::<String>::new(),
            "and it must cost zero model calls"
        );
    }

    /// A meeting only matches by a name; a handle on its own is not what a
    /// calendar says, and somebody who is not listed is not read at all.
    ///
    /// Proved by matching on the handle when a person has no name:
    ///
    /// ```text
    /// a meeting with nobody listed must cost no request
    /// ```
    #[tokio::test]
    async fn one_on_one_reads_nothing_for_a_meeting_with_nobody_listed() {
        let (mut context, _scratch, _engine) = silent("one-on-one-none").await;
        // Spare replies that are never fetched, so a request would show.
        let (host, github) = serve(vec![
            ("HTTP/1.1 200 OK", MERGED_ANSWER),
            ("HTTP/1.1 200 OK", TEAM_ANSWER),
        ]);
        context.github = github::Client::against(&host).expect("client");
        connect(&context, integrations::GITHUB).await;
        meeting(&context, "1:1 with Priya Rao").await;
        meeting(&context, "prao sync").await;
        context
            .corpus
            .write(
                crate::team::FILE,
                "## People\n- Dana Vance = dvance\n- prao\n",
            )
            .await
            .expect("file");

        let answered = answer(Intent::OneOnOne, &context).await.expect("an answer");

        assert!(
            answered
                .markdown
                .contains("None of today's meetings include someone you listed"),
            "{}",
            answered.markdown
        );
        assert!(
            !github.is_finished(),
            "a meeting with nobody listed must cost no request"
        );
    }

    /// "What is waiting on me" is the reviews, not the user's own work.
    ///
    /// REC-41 established that these are different questions and that answering
    /// one with the other was a wrong answer rather than a thin one. The
    /// distinction lives in `category` now, so this asserts the answer holds
    /// the review and not the pull request sitting beside it in the same table.
    ///
    /// Proved by pointing `waiting` at `retrieval::latest`:
    ///
    /// ```text
    /// the user's own work is not a thing waiting on them
    /// ```
    #[tokio::test]
    async fn waiting_answers_with_the_reviews_and_not_the_users_own_work() {
        let (context, _scratch, server) = silent("waiting").await;

        for (category, external_id, title, summary) in [
            (
                "review",
                "o/r#71",
                "Tighten the calendar parser",
                "review requested",
            ),
            ("pr", "o/r#70", "Add the corpus watcher", "merged"),
        ] {
            work_log::upsert(
                &context.pool,
                work_log::WorkLogRecord {
                    timestamp: "2026-09-02T09:00:00.000Z".to_string(),
                    source: "github".to_string(),
                    category: category.to_string(),
                    title: title.to_string(),
                    content: title.to_string(),
                    summary: Some(summary.to_string()),
                    url: None,
                    raw_ref: None,
                    external_id: external_id.to_string(),
                    account_id: 1,
                },
            )
            .await
            .expect("should store");
        }

        let answered = answer(Intent::Waiting, &context)
            .await
            .expect("a review request is something to say");

        assert!(
            answered.markdown.contains("Tighten the calendar parser"),
            "the review should be the answer: {}",
            answered.markdown
        );
        assert!(
            !answered.markdown.contains("Add the corpus watcher"),
            "the user's own work is not a thing waiting on them: {}",
            answered.markdown
        );
        assert_eq!(
            server.await.expect("server"),
            Vec::<String>::new(),
            "and it must cost zero model calls"
        );
    }

    /// The guard on a false answer found by running the real thing.
    ///
    /// Measured against scottmallinson/chief.ai on 2026-09-03: the pass had
    /// ingested pull request #66, opened that morning and still open, and
    /// "what did I ship today" listed it as that day's shipped work beside
    /// #65, which really had merged. `Log` filtered by window and not by
    /// state, which was accidentally correct for as long as the pass read only
    /// merged work and stopped being correct the moment it read more.
    ///
    /// Proved by passing `None` as the category:
    ///
    /// ```text
    /// an open pull request is work in flight, not work shipped
    /// ```
    #[tokio::test]
    async fn shipped_means_merged_and_never_merely_open() {
        let (context, _scratch, _server) = silent("shipped").await;

        for (category, external_id, title, summary) in [
            (
                crate::ingest::SHIPPED,
                "o/r#65",
                "Landed the work log",
                "merged",
            ),
            (
                crate::ingest::IN_FLIGHT,
                "o/r#66",
                "Still open today",
                "open",
            ),
            (
                crate::ingest::REVIEW,
                "o/r#71",
                "Somebody else's ask",
                "review requested",
            ),
            (crate::ingest::MEETING, "cal:1", "Standup", "09:30"),
            // Migration 8's default, which is what an entry the user typed
            // into their own work log carries. It is their work and it counts:
            // filtering to `SHIPPED` alone dropped it, which an existing test
            // caught before this one did.
            ("note", "note:1", "Wrote the release notes by hand", "done"),
        ] {
            work_log::upsert(
                &context.pool,
                work_log::WorkLogRecord {
                    timestamp: bump(&day_window(&chrono::Local::now()).0),
                    source: "github".to_string(),
                    category: category.to_string(),
                    title: title.to_string(),
                    content: title.to_string(),
                    summary: Some(summary.to_string()),
                    url: None,
                    raw_ref: None,
                    external_id: external_id.to_string(),
                    account_id: 1,
                },
            )
            .await
            .expect("should store");
        }

        for window in [Window::Today, Window::ThisWeek, Window::Recent] {
            let answered = answer(Intent::Log(window), &context)
                .await
                .unwrap_or_else(|| panic!("{window:?} should have the merged one to report"));

            assert!(
                answered.markdown.contains("Landed the work log"),
                "{window:?} should report what merged: {}",
                answered.markdown
            );
            assert!(
                answered
                    .markdown
                    .contains("Wrote the release notes by hand"),
                "{window:?} should count what the user logged themselves: {}",
                answered.markdown
            );

            for excluded in ["Still open today", "Somebody else's ask", "Standup"] {
                assert!(
                    !answered.markdown.contains(excluded),
                    "an open pull request is work in flight, not work shipped, and \
                     neither a review request nor a meeting is either — {excluded:?} \
                     in {window:?}: {}",
                    answered.markdown
                );
            }
        }
    }

    fn tools_context(context: &recipe::Context) -> tools::Context {
        tools::Context::from_recipe(context)
    }

    async fn log_open(context: &recipe::Context, external_id: &str, title: &str) {
        work_log::upsert(
            &context.pool,
            work_log::WorkLogRecord {
                timestamp: "2026-09-02T09:00:00.000Z".to_string(),
                source: "github".to_string(),
                category: ingest::IN_FLIGHT.to_string(),
                title: title.to_string(),
                content: title.to_string(),
                summary: Some("open".to_string()),
                url: None,
                raw_ref: None,
                external_id: external_id.to_string(),
                account_id: 1,
            },
        )
        .await
        .expect("should store");
    }

    fn search_result(number: u32, title: &str, merged_at: Option<String>) -> String {
        serde_json::json!({
            "total_count": 1,
            "items": [{
                "number": number,
                "title": title,
                "repository_url": "https://api.github.com/repos/o/r",
                "state": "open",
                "draft": false,
                "html_url": format!("https://github.com/o/r/pull/{number}"),
                "updated_at": "2026-10-23T09:00:00Z",
                "pull_request": { "merged_at": merged_at },
            }]
        })
        .to_string()
    }

    /// A standup is read from GitHub as it is asked, so a pull request that has
    /// since merged is not still "open" because a row nothing closes says so.
    ///
    /// Proved by reading the "Still open" section from the log first:
    ///
    /// ```text
    /// a pull request that has since merged is still being reported open
    /// ```
    #[tokio::test]
    async fn a_standup_is_read_from_github_now_and_not_from_rows_that_never_close() {
        let (mut context, _scratch, _model) = silent("standup-live").await;
        connect(&context, integrations::GITHUB).await;
        log_open(&context, "o/r#40", "Already merged since").await;

        let merged_just_now = chrono::Utc::now().to_rfc3339();
        let open = search_result(41, "Still being reviewed", None);
        let none = r#"{"total_count":0,"items":[]}"#.to_string();
        let merged = search_result(42, "Shipped this morning", Some(merged_just_now));
        let (host, github) = serve(vec![
            ("HTTP/1.1 200 OK", open),
            // What is holding up the open ones: nothing, here.
            (
                "HTTP/1.1 200 OK",
                r#"{"data":{"viewer":{"pullRequests":{"nodes":[]}}}}"#.to_string(),
            ),
            // The issues assigned to the user, which are work like any other.
            ("HTTP/1.1 200 OK", "[]".to_string()),
            ("HTTP/1.1 200 OK", none),
            ("HTTP/1.1 200 OK", merged),
        ]);
        context.github = github::Client::against(&host).expect("client");

        let found = standup(&tools_context(&context)).await;

        let section = |heading: &str| {
            found
                .parts
                .iter()
                .find(|part| part.heading == heading)
                .map(|part| part.lines.join("\n"))
                .unwrap_or_default()
        };
        assert!(
            section("Still open").contains("Still being reviewed"),
            "{found:?}"
        );
        assert!(
            !section("Still open").contains("Already merged since"),
            "a pull request that has since merged is still being reported open: {found:?}"
        );
        assert!(
            section("Done").contains("Shipped this morning (o/r#42, merged today)"),
            "{found:?}"
        );
        assert!(
            section("Waiting on my review").is_empty(),
            "an empty section is left out"
        );
        assert!(!found.from_log && !found.unread);

        let requests = github.await.expect("server");
        assert_eq!(
            requests.len(),
            5,
            "open, what holds the open ones up, assigned issues, review requests and merged: five reads"
        );
    }

    /// GitHub out of reach: the last sync answers, and the standup says so.
    #[tokio::test]
    async fn a_standup_falls_back_to_the_log_and_says_so_when_github_is_unreachable() {
        let (context, _scratch, _model) = silent("standup-offline").await;
        connect(&context, integrations::GITHUB).await;
        log_open(&context, "o/r#40", "Logged and open").await;

        let found = standup(&tools_context(&context)).await;

        assert!(found.from_log, "{found:?}");
        assert!(
            found.parts.iter().any(|part| part.heading == "Still open"
                && part.lines.join("\n").contains("Logged and open")),
            "{found:?}"
        );
    }

    /// Unreachable and nothing logged is not "nothing to report".
    #[tokio::test]
    async fn an_unreadable_standup_with_an_empty_log_is_not_an_empty_standup() {
        let (context, _scratch, _model) = silent("standup-unread").await;
        connect(&context, integrations::GITHUB).await;

        let found = standup(&tools_context(&context)).await;

        assert!(found.parts.is_empty());
        assert!(found.unread, "{found:?}");
    }

    const REVIEW_REQUESTS: &str = r#"{
        "total_count": 1,
        "items": [{
            "number": 88,
            "title": "Fresh review request",
            "repository_url": "https://api.github.com/repos/o/r",
            "state": "open",
            "draft": false,
            "html_url": "https://github.com/o/r/pull/88",
            "updated_at": "2026-09-02T09:00:00Z"
        }]
    }"#;

    async fn log_review(context: &recipe::Context, external_id: &str, title: &str) {
        work_log::upsert(
            &context.pool,
            work_log::WorkLogRecord {
                timestamp: "2026-09-02T09:00:00.000Z".to_string(),
                source: "github".to_string(),
                category: "review".to_string(),
                title: title.to_string(),
                content: title.to_string(),
                summary: Some("review requested".to_string()),
                url: None,
                raw_ref: None,
                external_id: external_id.to_string(),
                account_id: 1,
            },
        )
        .await
        .expect("should store");
    }

    /// "What is waiting on me" asks GitHub, so a request somebody has since
    /// dealt with is not reported from a row nothing ever closes.
    ///
    /// Proved by making `waiting` read `stored_reviews` first:
    ///
    /// ```text
    /// a review request already dealt with is still being reported
    /// ```
    #[tokio::test]
    async fn waiting_reads_github_now_and_not_a_row_that_has_since_closed() {
        let (mut context, _scratch, _model) = silent("waiting-live").await;
        connect(&context, integrations::GITHUB).await;
        log_review(&context, "o/r#71", "Already reviewed by someone").await;

        let (host, github) = serve(vec![("HTTP/1.1 200 OK", REVIEW_REQUESTS)]);
        context.github = github::Client::against(&host).expect("client");

        let answered = answer(Intent::Waiting, &context)
            .await
            .expect("a review request is something to say");

        assert!(
            answered.markdown.contains("Fresh review request"),
            "the live request should be the answer: {}",
            answered.markdown
        );
        assert!(
            !answered.markdown.contains("Already reviewed by someone"),
            "a review request already dealt with is still being reported: {}",
            answered.markdown
        );
        assert_eq!(
            answered.provenance.as_deref(),
            Some("Read from GitHub just now.")
        );

        let requests = github.await.expect("server");
        assert!(
            requests[0].contains("review-requested"),
            "it should ask for review requests: {}",
            requests[0]
        );
    }

    /// With GitHub unreachable the last sync still answers, and says it is not
    /// current.
    #[tokio::test]
    async fn waiting_falls_back_to_the_log_and_says_so_when_github_is_unreachable() {
        let (context, _scratch, _model) = silent("waiting-offline").await;
        connect(&context, integrations::GITHUB).await;
        log_review(&context, "o/r#71", "Tighten the calendar parser").await;

        let answered = answer(Intent::Waiting, &context)
            .await
            .expect("the stored request is better than nothing");

        assert!(answered.markdown.contains("Tighten the calendar parser"));
        assert!(
            answered
                .provenance
                .as_deref()
                .is_some_and(|line| line.starts_with("GitHub could not be reached")),
            "{:?}",
            answered.provenance
        );
    }

    #[tokio::test]
    async fn waiting_steps_aside_when_there_is_nothing_waiting() {
        let (context, _scratch, _server) = silent("waiting-empty").await;
        connect(&context, integrations::GITHUB).await;

        assert!(answer(Intent::Waiting, &context).await.is_none());
    }

    /// One minute past whatever instant this is, so a fixture lands inside a
    /// window rather than on its edge.
    fn bump(stamp: &str) -> String {
        chrono::DateTime::parse_from_rfc3339(stamp)
            .map(|at| {
                (at + chrono::Duration::minutes(1))
                    .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                    .to_string()
            })
            .unwrap_or_else(|_| stamp.to_string())
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

        assert_eq!(
            answered.as_ref().map(|answered| answered.markdown.as_str()),
            Some("- Standup at 09:30")
        );
        assert_eq!(
            answered.and_then(|answered| answered.provenance),
            None,
            "a brief is a file the user can open, not rows Chief assembled"
        );
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
        // Connected, and simply with nothing in it. This is the ambiguous
        // empty the step-aside was written for and it is untouched.
        connect(&context, integrations::GITHUB).await;

        let answered = deliver("/log", &context, |_| {}).await;

        assert_eq!(
            answered, None,
            "an empty log reads as a misunderstanding, so the model gets the question"
        );
    }

    /// The defect REC-62 is about.
    ///
    /// Asked what was on a calendar with nothing connected, the model
    /// answered "[Outlook] Meeting with John Doe at 10:00 AM" — invented, and
    /// past a system prompt that says never to invent meetings. Whether the
    /// user had a quiet day is not something this module can know; whether
    /// anything is connected is, so that is what it says.
    ///
    /// Proved by stepping aside as before:
    ///
    /// ```text
    /// assertion failed: a question with no source behind it must not reach
    /// the model, which will invent one
    /// ```
    #[tokio::test]
    async fn says_nothing_is_connected_rather_than_letting_the_model_invent_one() {
        let (context, _scratch, _server) = silent("unconnected").await;

        let answered = answer(Intent::Prep, &context).await.expect(
            "a question with no source behind it must not reach the model, which will invent one",
        );

        assert!(
            answered.markdown.contains("No calendar is connected"),
            "it names what is missing: {}",
            answered.markdown
        );
        assert!(
            answered.markdown.contains("Settings"),
            "and where to fix it: {}",
            answered.markdown
        );
        assert_eq!(
            answered.provenance, None,
            "nothing was read, so there is no source to cite"
        );
    }

    /// A calendar reads from either provider, so one of them is enough to make
    /// the empty ambiguous again.
    #[tokio::test]
    async fn a_subscribed_calendar_alone_is_a_connected_calendar() {
        let (context, _scratch, _server) = silent("subscribed").await;
        connect(&context, integrations::CALENDAR).await;

        assert!(
            answer(Intent::Prep, &context).await.is_none(),
            "a connected calendar with nothing in it steps aside as before"
        );
    }

    /// The brief is a file, not a connection. Saying "nothing is connected"
    /// when a brief has simply not been written would be a wrong answer about
    /// a different thing.
    #[tokio::test]
    async fn an_unwritten_brief_is_never_reported_as_a_missing_connection() {
        let (context, _scratch, _server) = silent("brief-unconnected").await;

        assert_eq!(answer(Intent::Brief, &context).await, None);
    }

    #[tokio::test]
    async fn steps_aside_when_no_brief_has_been_written_today() {
        let (context, _scratch, _server) = silent("nobrief").await;

        assert_eq!(deliver("/brief", &context, |_| {}).await, None);
    }
}
