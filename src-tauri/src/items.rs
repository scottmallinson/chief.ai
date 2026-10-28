//! What is assigned to the user, from whichever trackers they have connected,
//! and how it is going.
//!
//! GitHub issues, Jira issues and Linear issues are three shapes for one thing:
//! a piece of work somebody owns. The user has one, two or all three, so
//! nothing here assumes any of them. Each connected source is read into the
//! same [`Item`], an unconnected one is simply absent, and the report below
//! reads the same however many contributed.
//!
//! **Worked out in code, then worded.** What moved in the period, what is
//! blocked and since when, when anything was last touched and whether an item
//! looks likely to make its date are all computed from fields the trackers
//! return, and every number and date is written into the facts before a model
//! sees them. The model words a section of several items a few lines at a
//! time, and a batch that adds a date, a number or a name the facts do not
//! have is replaced by the facts as found. A section of one line, and the
//! whole report when the engine cannot be reached, is the facts as found.
//!
//! **Blocked means a reason Chief can name**, and the reason is printed beside
//! it: another unfinished issue the tracker says blocks it, or a pull request
//! that belongs to it with failing checks or changes requested. An issue
//! nobody has touched is not blocked, it is waiting, and the report does not
//! say otherwise.
//!
//! **A pull request belongs to an issue** when the tracker says so (Linear
//! lists the pull requests attached to an issue) or when the issue's key
//! appears in the pull request's title or description as a whole word, the
//! way Jira's own GitHub integration matches them. Neither is a guess about
//! what a question meant; both are reading a field.

use chrono::NaiveDate;

use crate::atlassian;
use crate::github;
use crate::integrations;
use crate::linear;
use crate::recipe::{self, Reach};
use crate::session::GithubSession;
use crate::sources::{self, Found};

/// Where an item lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tracker {
    Github,
    Jira,
    Linear,
}

impl Tracker {
    const fn name(self) -> &'static str {
        match self {
            Self::Github => "GitHub",
            Self::Jira => "Jira",
            Self::Linear => "Linear",
        }
    }
}

/// Something that happened to an item, dated by the tracker.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Change {
    /// When, as the tracker wrote it: an RFC 3339 timestamp or a bare date.
    pub at: String,
    pub kind: ChangeKind,
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum ChangeKind {
    /// It went from one status to another.
    Moved { from: String, to: String },
    /// Something was made to block it. Whether it still does is the item's
    /// `blocked_by`, which only lists what is unfinished.
    Blocked { by: String },
}

/// How far into the work an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Not started, or the tracker does not say.
    Waiting,
    /// Somebody is on it.
    Moving,
}

/// One piece of assigned work, whichever tracker holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub tracker: Tracker,
    /// `ENG-12`, or `owner/name#12` for GitHub.
    pub key: String,
    pub title: String,
    /// The tracker's own word for where it is.
    pub status: String,
    pub stage: Stage,
    /// 1 is most urgent; 9 is unset.
    pub rank: u8,
    /// The tracker's own word for the priority, when it has one.
    pub priority: Option<String>,
    pub due: Option<NaiveDate>,
    pub url: Option<String>,
    /// Keys of the unfinished items that block this one, as the tracker says.
    pub blocked_by: Vec<String>,
    /// Pull requests the tracker attached, as `owner/name#number`.
    pub pull_requests: Vec<String>,
    /// When the tracker last recorded anything on it.
    pub updated: Option<NaiveDate>,
    /// Dated status moves and blocks, when the tracker gave a history.
    pub changes: Vec<Change>,
    /// Whether `changes` is the whole history in the window or merely absent.
    /// An empty list means nothing moved only where this is true.
    pub history: bool,
}

/// One of the user's open pull requests, with what holds it up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pull {
    /// `owner/name#number`.
    pub id: String,
    pub title: String,
    pub body: String,
    pub draft: bool,
    /// When it last changed: a push, a review or a comment.
    pub updated: Option<NaiveDate>,
    /// What is stopping it merging: failing checks, requested changes,
    /// unresolved comments. Empty when nothing is.
    pub holds: Vec<String>,
}

impl Pull {
    /// How the report names it: the id, then what is wrong or that it is a draft.
    fn label(&self) -> String {
        let mut notes = self.holds.clone();
        if self.draft {
            notes.insert(0, "draft".to_string());
        }

        if notes.is_empty() {
            self.id.clone()
        } else {
            format!("{} ({})", self.id, notes.join(", "))
        }
    }
}

/// The words a pull request's hold-ups are reported in.
#[must_use]
pub fn holds(blockers: &github::Blockers) -> Vec<String> {
    let mut holds = Vec::new();

    if blockers.checks == github::Checks::Failing {
        holds.push("checks failing".to_string());
    }
    if blockers.decision == github::Decision::ChangesRequested {
        holds.push("changes requested".to_string());
    }
    match blockers.open_threads {
        0 => {}
        1 => holds.push("1 unresolved comment".to_string()),
        many => holds.push(format!("{many} unresolved comments")),
    }

    holds
}

/// Linear's priority numbers in words; 0 is "none" and so unset.
fn linear_priority(number: u8) -> (u8, Option<String>) {
    match number {
        1 => (1, Some("Urgent".to_string())),
        2 => (2, Some("High".to_string())),
        3 => (3, Some("Medium".to_string())),
        4 => (4, Some("Low".to_string())),
        _ => (9, None),
    }
}

/// A Jira priority's rank, from the five names a default site has. A site
/// that renamed them gets 9 (unset) for the sort and keeps its own word for
/// display.
fn jira_rank(name: &str) -> u8 {
    match name.to_lowercase().as_str() {
        "highest" | "blocker" | "critical" => 1,
        "high" | "major" => 2,
        "medium" | "normal" => 3,
        "low" | "minor" => 4,
        "lowest" | "trivial" => 5,
        _ => 9,
    }
}

fn day(text: Option<&str>) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text?.get(..10)?, "%Y-%m-%d").ok()
}

/// A Linear issue as an item.
#[must_use]
pub fn from_linear(issue: &linear::Item) -> Item {
    let (rank, priority) = linear_priority(issue.priority);

    Item {
        tracker: Tracker::Linear,
        key: issue.identifier.clone(),
        title: issue.title.clone(),
        status: issue.state.clone(),
        stage: if issue.state_kind == "started" {
            Stage::Moving
        } else {
            Stage::Waiting
        },
        rank,
        priority,
        due: day(issue.due.as_deref()),
        url: issue.url.clone(),
        blocked_by: issue.blocked_by.clone(),
        pull_requests: issue.pull_requests.clone(),
        updated: day(issue.updated_at.as_deref()),
        changes: issue.changes.clone(),
        history: true,
    }
}

/// A Jira issue as an item.
#[must_use]
pub fn from_jira(issue: &atlassian::Issue) -> Item {
    let detail = &issue.detail;

    Item {
        tracker: Tracker::Jira,
        key: issue.key.clone(),
        title: issue.summary.clone(),
        status: issue.status.clone(),
        stage: if detail.status_kind == "indeterminate" {
            Stage::Moving
        } else {
            Stage::Waiting
        },
        rank: detail.priority.as_deref().map_or(9, jira_rank),
        priority: detail.priority.clone(),
        due: day(detail.due.as_deref()),
        url: issue.url.clone(),
        blocked_by: detail.blocked_by.clone(),
        pull_requests: Vec::new(),
        updated: day(detail.updated.as_deref()),
        changes: detail.changes.clone(),
        history: detail.history,
    }
}

/// A GitHub issue assigned to the user, as an item. GitHub says nothing about
/// progress, so it is waiting.
#[must_use]
pub fn from_github(issue: &github::Issue) -> Item {
    Item {
        tracker: Tracker::Github,
        key: format!("{}#{}", issue.repository, issue.number),
        title: issue.title.clone(),
        status: "issue".to_string(),
        stage: Stage::Waiting,
        rank: 9,
        priority: None,
        due: None,
        url: Some(issue.url.clone()),
        blocked_by: Vec::new(),
        pull_requests: Vec::new(),
        updated: day(Some(&issue.updated_at)),
        changes: Vec::new(),
        history: false,
    }
}

/// Whether `key` appears in `text` as a whole word, so `ENG-1` is not found
/// in `ENG-12` but `ENG-12` is found in `feat(ENG-12): ...`. The case is the
/// tracker's own, which is how Jira writes a key.
fn mentions(text: &str, key: &str) -> bool {
    let bytes = text.as_bytes();
    let wanted = key.len();
    let word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_';

    text.match_indices(key).any(|(at, _)| {
        let before = at == 0 || !word(bytes[at - 1]);
        let after = at + wanted >= bytes.len() || !word(bytes[at + wanted]);

        before && after
    })
}

/// The pull requests that belong to an item: the ones the tracker attached,
/// then the ones that name its key. GitHub's own issues are not matched by key,
/// since `owner/name#1` inside prose is a mention and not a link.
fn pulls_for<'a>(item: &Item, pulls: &'a [Pull]) -> Vec<&'a Pull> {
    pulls
        .iter()
        .filter(|pull| {
            item.pull_requests.contains(&pull.id)
                || (item.tracker != Tracker::Github
                    && (mentions(&pull.title, &item.key) || mentions(&pull.body, &item.key)))
        })
        .collect()
}

/// What the report says about one item's due date.
fn due_words(due: NaiveDate, today: NaiveDate) -> String {
    let days = (due - today).num_days();

    match days {
        0 => format!("due today ({due})"),
        1 => format!("due tomorrow ({due})"),
        d if d > 1 => format!("due in {d} days ({due})"),
        -1 => format!("1 day overdue (was due {due})"),
        d => format!("{} days overdue (was due {due})", -d),
    }
}

/// How an item stands, for the section it goes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    Blocked,
    Due,
    Moving,
    Waiting,
}

/// How many days ahead "due soon" reaches.
const SOON_DAYS: i64 = 7;

/// The most lines a section shows; the rest are counted, never dropped silently.
const LINES: usize = 12;

struct Placed<'a> {
    item: &'a Item,
    pulls: Vec<&'a Pull>,
    standing: Standing,
}

fn stand<'a>(item: &'a Item, pulls: &'a [Pull], today: NaiveDate) -> Placed<'a> {
    let linked = pulls_for(item, pulls);

    let held = linked.iter().any(|pull| !pull.holds.is_empty());
    let soon = item
        .due
        .is_some_and(|due| (due - today).num_days() <= SOON_DAYS);

    let standing = if !item.blocked_by.is_empty() || held {
        Standing::Blocked
    } else if soon {
        Standing::Due
    } else if item.stage == Stage::Moving {
        Standing::Moving
    } else {
        Standing::Waiting
    };

    Placed {
        item,
        pulls: linked,
        standing,
    }
}

/// The period a report looks back over, and the day it is looking from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub today: NaiveDate,
    /// How many days back "progress" reaches.
    pub days: i64,
}

/// How long without a sign of life before an item with a date coming is
/// called at risk.
const STALE_DAYS: i64 = 7;

/// How far ahead a quiet item still counts against its date.
const RISK_HORIZON_DAYS: i64 = 14;

/// "today", "yesterday" or "N days ago", worked out here so no model has to
/// subtract.
fn ago(date: NaiveDate, today: NaiveDate) -> String {
    ago_days((today - date).num_days())
}

fn ago_days(days: i64) -> String {
    match days {
        0 => "today".to_string(),
        1 => "yesterday".to_string(),
        d if d > 1 => format!("{d} days ago"),
        _ => "in the future".to_string(),
    }
}

fn plural(count: i64, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

/// The newest dated sign that anyone touched the item or a pull request that
/// belongs to it.
fn last_activity(item: &Item, pulls: &[&Pull]) -> Option<NaiveDate> {
    item.updated
        .into_iter()
        .chain(item.changes.iter().filter_map(|c| day(Some(&c.at))))
        .chain(pulls.iter().filter_map(|pull| pull.updated))
        .max()
}

/// Whether an item looks likely to make its date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Forecast {
    Overdue,
    AtRisk,
    OnTrack,
    Unclear,
}

/// What the dates and the state say about whether an item will make its date.
///
/// **A rule, not a feeling, and it says which.** The model is handed this
/// sentence and asked to word it, so a forecast is only ever one of the few
/// below and the reason is in it. `None` only for an item with no date, which
/// has nothing to be late against.
fn outlook(placed: &Placed<'_>, window: Window) -> Option<(Forecast, String)> {
    let item = placed.item;
    let due = item.due?;
    let left = (due - window.today).num_days();
    let quiet = last_activity(item, &placed.pulls).map(|at| (window.today - at).num_days());
    let days = plural(left, "day");

    let cause = if item.blocked_by.is_empty() {
        "its pull request is held up"
    } else {
        "it is blocked"
    };

    Some(if left < 0 {
        (
            Forecast::Overdue,
            format!("Outlook: overdue by {}.", plural(-left, "day")),
        )
    } else if placed.standing == Standing::Blocked {
        (
            Forecast::AtRisk,
            format!("Outlook: at risk, because {cause} and it is due in {days}."),
        )
    } else if item.stage == Stage::Waiting && left <= SOON_DAYS {
        (
            Forecast::AtRisk,
            format!("Outlook: at risk, because it has not started and is due in {days}."),
        )
    } else if let Some(quiet) = quiet.filter(|q| *q >= STALE_DAYS && left <= RISK_HORIZON_DAYS) {
        (
            Forecast::AtRisk,
            format!(
                "Outlook: at risk, because nothing has changed for {} and it is due in {days}.",
                plural(quiet, "day")
            ),
        )
    } else if item.stage == Stage::Moving && quiet.is_some_and(|q| q < STALE_DAYS) {
        (
            Forecast::OnTrack,
            format!(
                "Outlook: on track, because it is in progress with activity {} and due in {days}.",
                ago_days(quiet.unwrap_or(0))
            ),
        )
    } else {
        (
            Forecast::Unclear,
            format!(
                "Outlook: unclear, because it is due in {days} and nothing shows how it is going."
            ),
        )
    })
}

/// Everything known about one item, as the sentences a report and a model are
/// both given. Every number and date in it is worked out here.
fn facts(placed: &Placed<'_>, window: Window) -> String {
    let item = placed.item;
    let today = window.today;
    let mut text = format!("{} {}", item.key, item.title);

    if !item.status.is_empty() {
        text.push_str(&format!(" [{}]", item.status));
    }

    let mut head = Vec::new();
    if let Some(priority) = &item.priority {
        head.push(priority.clone());
    }
    if let Some(due) = item.due {
        head.push(due_words(due, today));
    }
    if !head.is_empty() {
        text.push_str(&format!(", {}", head.join(", ")));
    }
    text.push('.');

    // What moved in the window. Said only where the tracker gave a history:
    // an absent history is not a quiet week.
    let since = today - chrono::Duration::days(window.days);
    if item.history {
        let moves: Vec<String> = item
            .changes
            .iter()
            .filter_map(|change| {
                let ChangeKind::Moved { from, to } = &change.kind else {
                    return None;
                };
                let on = day(Some(&change.at))?;

                (on >= since).then(|| {
                    let from = if from.is_empty() { "nothing" } else { from };
                    format!("Moved from {from} to {to} on {on} ({}).", ago(on, today))
                })
            })
            .take(3)
            .collect();

        if moves.is_empty() {
            text.push_str(&format!(
                " No status change in the last {}.",
                plural(window.days, "day")
            ));
        } else {
            text.push_str(&format!(" {}", moves.join(" ")));
        }
    }

    if let Some(at) = last_activity(item, &placed.pulls) {
        text.push_str(&format!(" Last activity {at} ({}).", ago(at, today)));
    }

    for key in &item.blocked_by {
        let since = item
            .changes
            .iter()
            .rev()
            .find_map(|change| match &change.kind {
                ChangeKind::Blocked { by } if by == key => day(Some(&change.at)),
                _ => None,
            });

        match since {
            Some(on) => text.push_str(&format!(
                " Blocked by {key} since {on} ({}).",
                plural((today - on).num_days().max(0), "day")
            )),
            None => text.push_str(&format!(
                " Blocked by {key}, and the tracker does not say since when."
            )),
        }
    }

    for pull in &placed.pulls {
        text.push_str(&format!(" Pull request {}", pull.label()));
        if let Some(at) = pull.updated {
            text.push_str(&format!(", last updated {at} ({})", ago(at, today)));
        }
        text.push('.');
    }

    if let Some((_, outlook)) = outlook(placed, window) {
        text.push(' ');
        text.push_str(&outlook);
    }

    text
}

fn order(left: &Placed<'_>, right: &Placed<'_>) -> std::cmp::Ordering {
    (left.item.rank, left.item.due, &left.item.key).cmp(&(
        right.item.rank,
        right.item.due,
        &right.item.key,
    ))
}

/// The first thing the report says: what needs the user, counted in code.
///
/// Counted from the same rules as the lines below it, so the sentence and the
/// list cannot disagree. `None` when nothing needs attention, which is said by
/// saying nothing rather than by a reassuring line.
#[must_use]
pub fn headline(items: &[Item], pulls: &[Pull], window: Window) -> Option<String> {
    let placed: Vec<Placed<'_>> = items
        .iter()
        .map(|item| stand(item, pulls, window.today))
        .collect();

    let keys = |wanted: &dyn Fn(&Placed<'_>) -> bool| -> Vec<&str> {
        placed
            .iter()
            .filter(|p| wanted(p))
            .map(|p| p.item.key.as_str())
            .collect()
    };
    let forecast = |p: &Placed<'_>| outlook(p, window).map(|(forecast, _)| forecast);

    let blocked = keys(&|p| p.standing == Standing::Blocked);
    let overdue = keys(&|p| forecast(p) == Some(Forecast::Overdue));
    let at_risk =
        keys(&|p| forecast(p) == Some(Forecast::AtRisk) && p.standing != Standing::Blocked);

    let mut parts = Vec::new();
    for (count, what) in [
        (&blocked, "blocked"),
        (&overdue, "overdue"),
        (&at_risk, "at risk of missing their date"),
    ] {
        if !count.is_empty() {
            parts.push(format!("{} {what} ({})", count.len(), count.join(", ")));
        }
    }

    (!parts.is_empty()).then(|| format!("Needs attention: {}.", parts.join("; ")))
}

/// One headed list of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub heading: String,
    pub lines: Vec<String>,
    /// What was left out for length, said rather than dropped.
    pub note: Option<String>,
    /// Whether a model is asked to word it. The pull requests that belong to
    /// no item have nothing to forecast, so a model could only reword them.
    pub worded: bool,
}

/// The report as parts: what is assigned, grouped by how it is going.
///
/// `window` is passed in so the dates are tested rather than read from a clock.
#[must_use]
pub fn parts(items: &[Item], pulls: &[Pull], window: Window) -> Vec<Part> {
    let today = window.today;
    let mut placed: Vec<Placed<'_>> = items.iter().map(|item| stand(item, pulls, today)).collect();
    placed.sort_by(order);

    let mut parts = Vec::new();

    for (standing, heading) in [
        (Standing::Blocked, "Blocked"),
        (Standing::Due, "Overdue or due this week"),
        (Standing::Moving, "In progress"),
        (Standing::Waiting, "Not started"),
    ] {
        let here: Vec<&Placed<'_>> = placed.iter().filter(|p| p.standing == standing).collect();
        if here.is_empty() {
            continue;
        }

        parts.push(Part {
            heading: format!("{heading} ({})", here.len()),
            lines: here.iter().take(LINES).map(|p| facts(p, window)).collect(),
            note: (here.len() > LINES).then(|| format!("{} more not shown.", here.len() - LINES)),
            // A section of one line is its own list: a model could only reword
            // it, and every rewording measured that went wrong did so on a
            // short one.
            worded: here.len() > 1,
        });
    }

    // Open pull requests that belong to nothing assigned.
    let belonging: Vec<&str> = placed
        .iter()
        .flat_map(|p| p.pulls.iter().map(|pull| pull.id.as_str()))
        .collect();
    let loose: Vec<&Pull> = pulls
        .iter()
        .filter(|pull| !belonging.contains(&pull.id.as_str()))
        .collect();

    if !loose.is_empty() {
        parts.push(Part {
            heading: format!("Pull requests tied to no item ({})", loose.len()),
            lines: loose
                .iter()
                .take(LINES)
                .map(|pull| format!("{} {}", pull.label(), pull.title))
                .collect(),
            note: (loose.len() > LINES).then(|| format!("{} more not shown.", loose.len() - LINES)),
            worded: false,
        });
    }

    parts
}

/// The parts as the plain report: what Chief says when no model writes it.
#[must_use]
pub fn render(parts: &[Part]) -> String {
    if parts.is_empty() {
        return "Nothing is assigned to you and open.".to_string();
    }

    let mut sections = Vec::new();
    for part in parts {
        sections.push(format!("## {}", part.heading));
        sections.extend(part.lines.iter().map(|line| format!("- {line}")));
        sections.extend(part.note.clone());
    }

    sections.join("\n")
}

/// The report with no model: the parts, rendered.
#[cfg(test)]
#[must_use]
pub fn report(items: &[Item], pulls: &[Pull], today: NaiveDate) -> String {
    render(&parts(
        items,
        pulls,
        Window {
            today,
            days: DEFAULT_DAYS,
        },
    ))
}

/// How far back progress is read unless the question says otherwise.
pub const DEFAULT_DAYS: i64 = 7;

/// What the readers found, and what they could not read.
pub struct Gathered {
    pub items: Vec<Item>,
    pub pulls: Vec<Pull>,
    /// Sources that answered, with how many items each gave.
    pub read: Vec<(Tracker, usize)>,
    /// Sources that are connected and could not be read, and why.
    pub unread: Vec<(Tracker, String)>,
    /// Whether any tracker or GitHub account is connected at all.
    pub connected: bool,
}

/// Linear issues with their detail, from every connected workspace.
pub async fn linear_items(context: &impl Reach) -> Found<linear::Item> {
    let mut found = Found::default();

    let Ok(accounts) = integrations::accounts(context.pool(), integrations::LINEAR).await else {
        return found;
    };

    for account in accounts {
        found.connected = true;

        let Ok(Some(credentials)) = integrations::credentials(context.pool(), account.id).await
        else {
            found.fail("Linear has no key saved");
            continue;
        };

        match context.linear().items(&credentials.access_token).await {
            Ok(read) => found.ok(read),
            Err(error) => found.fail(error),
        }
    }

    found
}

/// Read everything that is assigned to the user, from every connected source.
pub async fn gather(context: &recipe::Context) -> Gathered {
    let mut gathered = Gathered {
        items: Vec::new(),
        pulls: Vec::new(),
        read: Vec::new(),
        unread: Vec::new(),
        connected: false,
    };

    let linear = linear_items(context).await;
    let jira = sources::jira(context, None).await;

    for (tracker, connected, answered, failed, items) in [
        (
            Tracker::Linear,
            linear.connected,
            linear.answered,
            linear.failed,
            linear.items.iter().map(from_linear).collect::<Vec<_>>(),
        ),
        (
            Tracker::Jira,
            jira.connected,
            jira.answered,
            jira.failed,
            jira.items.iter().map(from_jira).collect::<Vec<_>>(),
        ),
    ] {
        gathered.connected |= connected;
        if answered {
            gathered.read.push((tracker, items.len()));
            gathered.items.extend(items);
        }
        for why in failed {
            gathered.unread.push((tracker, why));
        }
    }

    let accounts = recipe::github_accounts(context).await;
    gathered.connected |= !accounts.is_empty();

    let mut issues = 0;
    let mut answered = false;

    for account in accounts {
        let session = GithubSession::new(&context.pool, &context.github, account);

        match session.assigned_issues(recipe::PER_SOURCE).await {
            Ok(found) => {
                answered = true;
                issues += found.len();
                gathered.items.extend(found.iter().map(from_github));
            }
            Err(error) => gathered.unread.push((Tracker::Github, error.to_string())),
        }

        let blockers = session
            .blockers(recipe::PER_SOURCE)
            .await
            .unwrap_or_default();

        match session
            .pull_requests(github::Involvement::Authored, github::State::Open, 30)
            .await
        {
            Ok(found) => {
                answered = true;
                for pull in found {
                    let id = pull.external_id();
                    let holds = blockers
                        .iter()
                        .find(|one| one.external_id() == id)
                        .map(holds)
                        .unwrap_or_default();

                    gathered.pulls.push(Pull {
                        id,
                        title: pull.title,
                        body: pull.body.unwrap_or_default(),
                        draft: pull.draft,
                        updated: day(Some(&pull.updated_at)),
                        holds,
                    });
                }
            }
            Err(error) => gathered.unread.push((Tracker::Github, error.to_string())),
        }
    }

    if answered {
        gathered.read.push((Tracker::Github, issues));
    }

    gathered
}

/// What `/items` says.
pub struct Report {
    pub markdown: String,
    /// What was read, composed here and not written by a model.
    pub provenance: Option<String>,
}

/// What was read and worked out, before anybody words it.
pub struct Analysis {
    /// Said first, and worked out in code.
    pub headline: Option<String>,
    pub parts: Vec<Part>,
    /// Sources that are connected and could not be read, said in code.
    pub unread: Vec<String>,
    pub provenance: Option<String>,
    pub window: Window,
}

impl Analysis {
    /// The report with no model: every fact, as found.
    #[must_use]
    pub fn markdown(&self) -> String {
        let mut markdown = render(&self.parts);

        if let Some(headline) = &self.headline {
            markdown = format!("{headline}\n\n{markdown}");
        }

        for note in &self.unread {
            markdown.push('\n');
            markdown.push_str(note);
        }

        markdown
    }
}

/// What the model is asked to do with each batch of lines.
fn instruction(days: i64, lines: &str) -> String {
    format!(
        "These lines are the user's assigned work, read just now:\n{lines}\n\nWrite one or \
         two short plain sentences for each line, starting with a dash and keeping its key. \
         Say what has moved in the last {days} days or that nothing has, what blocks it and \
         since when, and, where the line has an Outlook, whether it will make its date, \
         keeping the Outlook's reason. Use only what the line says. Do not add dates, \
         numbers, people or reasons, and do not say what you cannot do."
    )
}

/// The report, worded a few lines at a time and checked against the lines.
///
/// **Checked, and not streamed.** Text already shown cannot be taken back, so
/// each batch is held until `check::unsupported_by` has compared it with the
/// lines it was written from, and a batch that adds a date, a number, a name
/// or a run of unfamiliar words is replaced by the lines as found. The cost
/// is that the answer arrives whole.
///
/// # Errors
/// When the engine cannot be reached or stops answering.
pub async fn write(
    client: &crate::llama::Client,
    analysis: &Analysis,
    request: impl Fn(String) -> crate::llama::ChatRequest,
) -> Result<crate::sections::Written, crate::llama::Error> {
    let days = analysis.window.days;

    let sections: Vec<crate::sections::Section> = analysis
        .parts
        .iter()
        .map(|part| crate::sections::Section {
            heading: part.heading.clone(),
            lines: part.lines.clone(),
            worded: part.worded,
        })
        .collect();

    let mut written = crate::sections::write(
        client,
        &sections,
        |heading| format!("## {heading}"),
        |_, lines| instruction(days, lines),
        request,
        |answer, lines| {
            let shown = lines.join("\n");

            crate::check::unsupported_by(
                answer,
                &crate::check::Material {
                    shown: &shown,
                    items: &shown,
                    words: &shown,
                },
            )
        },
        |_| {},
    )
    .await?;

    if let Some(headline) = &analysis.headline {
        written.text = format!("{headline}\n\n{}", written.text);
    }

    // A count of what was left out is not a line a model words.
    for part in &analysis.parts {
        if let Some(note) = &part.note {
            written
                .text
                .push_str(&format!("\n{}: {note}", part.heading));
        }
    }
    for note in &analysis.unread {
        written.text.push('\n');
        written.text.push_str(note);
    }

    Ok(written)
}

/// Read everything and work out how it is going. `None` when nothing that
/// holds work is connected.
pub async fn analyse(context: &recipe::Context, days: i64) -> Option<Analysis> {
    let gathered = gather(context).await;

    if !gathered.connected {
        return None;
    }

    let window = Window {
        today: chrono::Local::now().date_naive(),
        days,
    };

    let unread = gathered
        .unread
        .iter()
        .map(|(tracker, why)| format!("{} could not be read: {why}.", tracker.name()))
        .collect();

    let provenance = (!gathered.read.is_empty()).then(|| {
        let parts: Vec<String> = gathered
            .read
            .iter()
            .map(|(tracker, count)| format!("{} ({count})", tracker.name()))
            .collect();

        format!("Read just now: {}.", parts.join(", "))
    });

    Some(Analysis {
        headline: headline(&gathered.items, &gathered.pulls, window),
        parts: parts(&gathered.items, &gathered.pulls, window),
        unread,
        provenance,
        window,
    })
}

/// The answer to `/items` with no model. `None` when nothing that holds work
/// is connected.
pub async fn read(context: &recipe::Context) -> Option<Report> {
    let analysis = analyse(context, DEFAULT_DAYS).await?;

    Some(Report {
        markdown: analysis.markdown(),
        provenance: analysis.provenance,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 28).expect("a date")
    }

    fn item(key: &str, stage: Stage) -> Item {
        Item {
            tracker: Tracker::Jira,
            key: key.to_string(),
            title: format!("Work on {key}"),
            status: "In Progress".to_string(),
            stage,
            rank: 9,
            priority: None,
            due: None,
            url: None,
            blocked_by: Vec::new(),
            pull_requests: Vec::new(),
            updated: None,
            changes: Vec::new(),
            history: false,
        }
    }

    fn pull(id: &str, title: &str, holds: &[&str]) -> Pull {
        Pull {
            id: id.to_string(),
            title: title.to_string(),
            body: String::new(),
            draft: false,
            updated: None,
            holds: holds.iter().map(ToString::to_string).collect(),
        }
    }

    #[test]
    fn a_key_matches_only_as_a_whole_word() {
        assert!(mentions("feat(ENG-12): ship it", "ENG-12"));
        assert!(mentions("ENG-12", "ENG-12"));
        assert!(mentions("Fixes ENG-12.", "ENG-12"));
        assert!(!mentions("ENG-123 and more", "ENG-12"));
        assert!(!mentions("PRE-ENG-12", "ENG-12"));
        assert!(!mentions("ENG-1", "ENG-12"));
    }

    #[test]
    fn says_what_blocks_an_item_and_names_it() {
        let mut blocked = item("ENG-12", Stage::Moving);
        blocked.blocked_by = vec!["ENG-3".to_string()];

        let text = report(&[blocked], &[], today());

        assert!(text.contains("## Blocked (1)"), "{text}");
        assert!(text.contains("Blocked by ENG-3,"), "{text}");
        assert!(!text.contains("## In progress"), "{text}");
    }

    #[test]
    fn an_item_nobody_has_touched_is_waiting_and_not_blocked() {
        let text = report(&[item("ENG-13", Stage::Waiting)], &[], today());

        assert!(text.contains("## Not started (1)"), "{text}");
        assert!(!text.contains("Blocked"), "{text}");
    }

    #[test]
    fn a_linked_pull_request_that_is_held_up_blocks_its_item() {
        let moving = item("ENG-12", Stage::Moving);
        let held = pull("o/r#4", "feat(ENG-12): ship it", &["checks failing"]);

        let text = report(&[moving], &[held], today());

        assert!(text.contains("## Blocked (1)"), "{text}");
        assert!(
            text.contains("Pull request o/r#4 (checks failing)"),
            "{text}"
        );
    }

    #[test]
    fn a_linked_pull_request_that_is_fine_does_not_block() {
        let moving = item("ENG-12", Stage::Moving);
        let fine = pull("o/r#4", "feat(ENG-12): ship it", &[]);

        let text = report(&[moving], &[fine], today());

        assert!(text.contains("## In progress (1)"), "{text}");
        assert!(text.contains("Pull request o/r#4"), "{text}");
    }

    #[test]
    fn a_pull_request_naming_a_longer_key_is_not_this_items() {
        let moving = item("ENG-12", Stage::Moving);
        let other = pull(
            "o/r#5",
            "feat(ENG-123): something else",
            &["checks failing"],
        );

        let text = report(&[moving], &[other], today());

        assert!(text.contains("## In progress (1)"), "{text}");
        assert!(text.contains("tied to no item"), "{text}");
    }

    #[test]
    fn a_pull_request_the_tracker_attached_is_linked_without_naming_the_key() {
        let mut attached = item("REC-5", Stage::Moving);
        attached.tracker = Tracker::Linear;
        attached.pull_requests = vec!["o/r#9".to_string()];
        let unnamed = pull("o/r#9", "Tidy the parser", &["changes requested"]);

        let text = report(&[attached], &[unnamed], today());

        assert!(text.contains("## Blocked (1)"), "{text}");
    }

    #[test]
    fn a_pull_request_with_no_item_is_listed_on_its_own() {
        let loose = pull("o/r#7", "Unrelated tidy-up", &[]);

        let text = report(&[item("ENG-12", Stage::Moving)], &[loose], today());

        assert!(
            text.contains("## Pull requests tied to no item (1)"),
            "{text}"
        );
        assert!(text.contains("o/r#7 Unrelated tidy-up"), "{text}");
    }

    #[test]
    fn a_linked_pull_request_is_not_listed_a_second_time() {
        let linked = pull("o/r#4", "feat(ENG-12): ship it", &[]);

        let text = report(&[item("ENG-12", Stage::Moving)], &[linked], today());

        assert!(!text.contains("tied to no item"), "{text}");
    }

    #[test]
    fn dates_say_how_late_or_how_soon() {
        let mut late = item("ENG-1", Stage::Moving);
        late.due = NaiveDate::from_ymd_opt(2026, 10, 25);
        let mut soon = item("ENG-2", Stage::Waiting);
        soon.due = NaiveDate::from_ymd_opt(2026, 10, 30);
        let mut far = item("ENG-3", Stage::Waiting);
        far.due = NaiveDate::from_ymd_opt(2026, 12, 1);

        let text = report(&[late, soon, far], &[], today());

        assert!(text.contains("## Overdue or due this week (2)"), "{text}");
        assert!(
            text.contains("3 days overdue (was due 2026-10-25)"),
            "{text}"
        );
        assert!(text.contains("due in 2 days (2026-10-30)"), "{text}");
        // A far date puts nothing in the due section.
        assert!(text.contains("## Not started (1)"), "{text}");
    }

    #[test]
    fn the_most_urgent_comes_first() {
        let mut low = item("ENG-1", Stage::Waiting);
        low.rank = 4;
        low.priority = Some("Low".to_string());
        let mut urgent = item("ENG-2", Stage::Waiting);
        urgent.rank = 1;
        urgent.priority = Some("Urgent".to_string());

        let text = report(&[low, urgent], &[], today());

        assert!(text.find("ENG-2").expect("urgent") < text.find("ENG-1").expect("low"));
    }

    #[test]
    fn a_long_section_counts_what_it_does_not_show() {
        let many: Vec<Item> = (0..LINES + 3)
            .map(|n| item(&format!("ENG-{n}"), Stage::Waiting))
            .collect();

        let text = report(&many, &[], today());

        assert!(text.contains("3 more not shown."), "{text}");
    }

    #[test]
    fn says_so_when_nothing_is_assigned() {
        assert_eq!(
            report(&[], &[], today()),
            "Nothing is assigned to you and open."
        );
    }

    #[test]
    fn linear_and_jira_read_into_the_same_shape() {
        let from_linear = from_linear(&linear::Item {
            identifier: "REC-1".to_string(),
            title: "t".to_string(),
            state: "In Progress".to_string(),
            state_kind: "started".to_string(),
            priority: 2,
            due: Some("2026-10-30".to_string()),
            url: None,
            updated_at: None,
            blocked_by: vec!["REC-0".to_string()],
            pull_requests: Vec::new(),
            changes: Vec::new(),
        });
        let from_jira = from_jira(&atlassian::Issue {
            key: "PROJ-1".to_string(),
            summary: "t".to_string(),
            status: "In Progress".to_string(),
            url: None,
            resolved: None,
            detail: atlassian::Detail {
                priority: Some("High".to_string()),
                due: Some("2026-10-30".to_string()),
                status_kind: "indeterminate".to_string(),
                blocked_by: vec!["PROJ-0".to_string()],
                ..atlassian::Detail::default()
            },
        });

        assert_eq!(from_linear.stage, from_jira.stage);
        assert_eq!(from_linear.rank, from_jira.rank);
        assert_eq!(from_linear.priority, from_jira.priority);
        assert_eq!(from_linear.due, from_jira.due);
        assert_eq!(from_linear.blocked_by.len(), from_jira.blocked_by.len());
    }

    fn window() -> Window {
        Window {
            today: today(),
            days: 7,
        }
    }

    fn on(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, day).expect("a date")
    }

    fn moved(at: &str, from: &str, to: &str) -> Change {
        Change {
            at: at.to_string(),
            kind: ChangeKind::Moved {
                from: from.to_string(),
                to: to.to_string(),
            },
        }
    }

    fn lines(items: &[Item], pulls: &[Pull]) -> String {
        parts(items, pulls, window())
            .iter()
            .flat_map(|part| part.lines.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn says_what_moved_inside_the_window_and_when() {
        let mut one = item("ENG-12", Stage::Moving);
        one.history = true;
        one.changes = vec![
            moved("2026-10-26T10:00:00.000Z", "To Do", "In Progress"),
            moved("2026-10-01T10:00:00.000Z", "Backlog", "To Do"),
        ];

        let text = lines(&[one], &[]);

        assert!(
            text.contains("Moved from To Do to In Progress on 2026-10-26 (2 days ago)."),
            "{text}"
        );
        // A move before the window is not this week's progress.
        assert!(!text.contains("Backlog"), "{text}");
    }

    #[test]
    fn says_so_when_nothing_moved_but_only_where_a_history_was_read() {
        let mut read = item("ENG-12", Stage::Moving);
        read.history = true;
        let unread = item("ENG-13", Stage::Moving);

        let text = lines(&[read, unread], &[]);

        assert!(
            text.contains(
                "ENG-12 Work on ENG-12 [In Progress]. No status change in the last 7 days."
            ),
            "{text}"
        );
        // No history is not a quiet week: nothing is claimed about movement.
        assert!(
            !text.contains("ENG-13 Work on ENG-13 [In Progress]. No status change"),
            "{text}"
        );
    }

    #[test]
    fn says_since_when_and_for_how_long_an_item_has_been_blocked() {
        let mut blocked = item("ENG-12", Stage::Moving);
        blocked.blocked_by = vec!["ENG-3".to_string(), "ENG-4".to_string()];
        blocked.changes = vec![Change {
            at: "2026-10-15T08:00:00.000Z".to_string(),
            kind: ChangeKind::Blocked {
                by: "ENG-3".to_string(),
            },
        }];

        let text = lines(&[blocked], &[]);

        assert!(
            text.contains("Blocked by ENG-3 since 2026-10-15 (13 days)."),
            "{text}"
        );
        // A blocker the tracker never dated is not given a made-up date.
        assert!(
            text.contains("Blocked by ENG-4, and the tracker does not say since when."),
            "{text}"
        );
    }

    #[test]
    fn last_activity_is_the_newest_sign_from_the_item_or_its_pull_requests() {
        let mut moving = item("ENG-12", Stage::Moving);
        moving.updated = Some(on(20));
        let mut linked = pull("o/r#4", "feat(ENG-12): ship it", &[]);
        linked.updated = Some(on(27));

        let text = lines(&[moving], &[linked]);

        assert!(
            text.contains("Last activity 2026-10-27 (yesterday)."),
            "{text}"
        );
        assert!(
            text.contains("Pull request o/r#4, last updated 2026-10-27 (yesterday)"),
            "{text}"
        );
    }

    fn outlook_of(configure: impl FnOnce(&mut Item)) -> String {
        let mut one = item("ENG-12", Stage::Moving);
        configure(&mut one);

        lines(&[one], &[])
    }

    #[test]
    fn forecasts_only_with_a_date_and_says_why() {
        let none = outlook_of(|_| {});
        assert!(!none.contains("Outlook"), "{none}");

        let late = outlook_of(|one| one.due = Some(on(25)));
        assert!(late.contains("Outlook: overdue by 3 days."), "{late}");

        let blocked = outlook_of(|one| {
            one.due = Some(on(30));
            one.blocked_by = vec!["ENG-3".to_string()];
        });
        assert!(
            blocked.contains("Outlook: at risk, because it is blocked and it is due in 2 days."),
            "{blocked}"
        );

        let unstarted = outlook_of(|one| {
            one.stage = Stage::Waiting;
            one.due = Some(on(30));
        });
        assert!(
            unstarted
                .contains("Outlook: at risk, because it has not started and is due in 2 days."),
            "{unstarted}"
        );

        let quiet = outlook_of(|one| {
            one.due = Some(on(31));
            one.updated = Some(on(15));
        });
        assert!(
            quiet.contains(
                "at risk, because nothing has changed for 13 days and it is due in 3 days."
            ),
            "{quiet}"
        );

        let fine = outlook_of(|one| {
            one.due = Some(on(31));
            one.updated = Some(on(27));
        });
        assert!(
            fine.contains("Outlook: on track, because it is in progress with activity yesterday and due in 3 days."),
            "{fine}"
        );

        let unknown = outlook_of(|one| one.due = Some(on(31)));
        assert!(unknown.contains("Outlook: unclear"), "{unknown}");
    }

    #[test]
    fn an_item_with_a_date_a_month_away_is_not_called_at_risk_for_being_quiet() {
        let text = outlook_of(|one| {
            one.due = Some(NaiveDate::from_ymd_opt(2026, 12, 20).expect("a date"));
            one.updated = Some(on(1));
        });

        assert!(text.contains("Outlook: unclear"), "{text}");
    }

    fn analysis_of(items: &[Item], pulls: &[Pull]) -> Analysis {
        Analysis {
            headline: headline(items, pulls, window()),
            parts: parts(items, pulls, window()),
            unread: vec!["Jira could not be read: no answer.".to_string()],
            provenance: None,
            window: window(),
        }
    }

    fn ask(prompt: String) -> crate::llama::ChatRequest {
        crate::llama::ChatRequest::new("model", vec![crate::llama::Message::user(prompt)])
    }

    fn said(text: &str) -> String {
        crate::llama::test_support::events(&[crate::llama::test_support::delta(text)])
    }

    fn blocked_item() -> Item {
        blocked_item_named("ENG-12")
    }

    fn blocked_item_named(key: &str) -> Item {
        let mut one = item(key, Stage::Moving);
        one.due = Some(on(30));
        one.blocked_by = vec!["ENG-3".to_string()];
        one.changes = vec![Change {
            at: "2026-10-15T08:00:00.000Z".to_string(),
            kind: ChangeKind::Blocked {
                by: "ENG-3".to_string(),
            },
        }];
        one
    }

    #[tokio::test]
    async fn words_the_facts_and_keeps_what_could_not_be_read() {
        let (host, server) = crate::llama::test_support::serve(vec![(
            "HTTP/1.1 200 OK",
            said("- ENG-12 has been blocked by ENG-3 since 2026-10-15 (13 days) and is at risk.\n- ENG-14 has been blocked by ENG-3 since 2026-10-15 (13 days) and is at risk."),
        )]);
        let client = crate::llama::Client::with_base_url(&host).expect("client");

        let written = write(
            &client,
            &analysis_of(&[blocked_item(), blocked_item_named("ENG-14")], &[]),
            ask,
        )
        .await
        .expect("written");

        assert!(
            written.text.contains("blocked by ENG-3 since 2026-10-15"),
            "{}",
            written.text
        );
        assert!(written.refused.is_empty(), "{:?}", written.refused);
        assert!(
            written.text.ends_with("Jira could not be read: no answer."),
            "{}",
            written.text
        );

        // The model was handed the worked-out facts, not the raw fields.
        let sent = server.await.expect("server").join("");
        assert!(
            sent.contains("Outlook: at risk, because it is blocked"),
            "{sent}"
        );
    }

    /// Proved by making the check in `write` return nothing: the invented
    /// date and the invented person were then printed as the report.
    #[tokio::test]
    async fn a_date_or_a_person_the_facts_do_not_have_is_refused_and_the_facts_are_shown() {
        let (host, _server) = crate::llama::test_support::serve(vec![(
            "HTTP/1.1 200 OK",
            said("- ENG-12 is blocked by ENG-3 since 2026-10-02 and Priya is chasing it."),
        )]);
        let client = crate::llama::Client::with_base_url(&host).expect("client");

        let written = write(
            &client,
            &analysis_of(&[blocked_item(), blocked_item_named("ENG-14")], &[]),
            ask,
        )
        .await
        .expect("written");

        assert!(!written.text.contains("Priya"), "{}", written.text);
        assert!(!written.text.contains("2026-10-02"), "{}", written.text);
        assert!(
            written
                .text
                .contains("Blocked by ENG-3 since 2026-10-15 (13 days)."),
            "{}",
            written.text
        );
        assert!(!written.refused.is_empty());
    }

    /// A section of one line is only reworded, and a rewording is where the
    /// measured failures came from.
    #[tokio::test]
    async fn a_section_of_one_line_is_written_in_code() {
        let (host, server) =
            crate::llama::test_support::serve(Vec::<(&'static str, String)>::new());
        let client = crate::llama::Client::with_base_url(&host).expect("client");

        let written = write(&client, &analysis_of(&[blocked_item()], &[]), ask)
            .await
            .expect("written");

        assert_eq!(written.requests, 0);
        assert!(
            written.text.contains("Blocked by ENG-3 since 2026-10-15"),
            "{}",
            written.text
        );
        drop(server);
    }

    #[test]
    fn the_headline_counts_what_needs_attention_from_the_same_rules_as_the_lines() {
        let mut late = item("ENG-1", Stage::Moving);
        late.due = Some(on(25));
        let mut risky = item("ENG-2", Stage::Waiting);
        risky.due = Some(on(30));
        let mut fine = item("ENG-3", Stage::Moving);
        fine.due = Some(on(31));
        fine.updated = Some(on(27));

        let headline = headline(&[blocked_item(), late, risky, fine], &[], window());

        assert_eq!(
            headline.as_deref(),
            Some("Needs attention: 1 blocked (ENG-12); 1 overdue (ENG-1); 1 at risk of missing their date (ENG-2).")
        );
        assert_eq!(
            super::headline(&[item("ENG-9", Stage::Moving)], &[], window()),
            None
        );
    }

    #[tokio::test]
    async fn pull_requests_tied_to_no_item_are_not_sent_to_a_model() {
        // No reply is queued: any request would find nothing listening.
        let (host, server) =
            crate::llama::test_support::serve(Vec::<(&'static str, String)>::new());
        let client = crate::llama::Client::with_base_url(&host).expect("client");
        let loose = pull("o/r#7", "Unrelated tidy-up", &[]);

        let written = write(&client, &analysis_of(&[], &[loose]), ask)
            .await
            .expect("written");

        assert_eq!(written.requests, 0);
        assert!(
            written.text.contains("o/r#7 Unrelated tidy-up"),
            "{}",
            written.text
        );
        drop(server);
    }
}
