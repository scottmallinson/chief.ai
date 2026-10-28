//! What is assigned to the user, from whichever trackers they have connected,
//! and how it is going.
//!
//! GitHub issues, Jira issues and Linear issues are three shapes for one thing:
//! a piece of work somebody owns. The user has one, two or all three, so
//! nothing here assumes any of them. Each connected source is read into the
//! same [`Item`], an unconnected one is simply absent, and the report below
//! reads the same however many contributed.
//!
//! **Counted in code, and no model is called.** Which items are blocked, which
//! are late and which are moving come from fields the trackers return, and a
//! sentence a model wrote about them could only be wrong where this is not.
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
}

/// One of the user's open pull requests, with what holds it up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pull {
    /// `owner/name#number`.
    pub id: String,
    pub title: String,
    pub body: String,
    pub draft: bool,
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

fn line(placed: &Placed<'_>, today: NaiveDate) -> String {
    let item = placed.item;
    let mut text = format!("- {} {}", item.key, item.title);

    if !item.status.is_empty() {
        text.push_str(&format!(" [{}]", item.status));
    }

    let mut facts = Vec::new();
    if let Some(priority) = &item.priority {
        facts.push(priority.clone());
    }
    if let Some(due) = item.due {
        facts.push(due_words(due, today));
    }
    if !facts.is_empty() {
        text.push_str(&format!(", {}", facts.join(", ")));
    }
    text.push('.');

    if !item.blocked_by.is_empty() {
        text.push_str(&format!(" Blocked by {}.", item.blocked_by.join(", ")));
    }

    if !placed.pulls.is_empty() {
        let labels: Vec<String> = placed.pulls.iter().map(|pull| pull.label()).collect();
        text.push_str(&format!(" Pull requests: {}.", labels.join(", ")));
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

/// The report: what is assigned, grouped by how it is going.
///
/// `today` is passed in so the dates are tested rather than read from a clock.
#[must_use]
pub fn report(items: &[Item], pulls: &[Pull], today: NaiveDate) -> String {
    let mut placed: Vec<Placed<'_>> = items.iter().map(|item| stand(item, pulls, today)).collect();
    placed.sort_by(order);

    let mut sections = Vec::new();

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

        sections.push(format!("## {heading} ({})", here.len()));
        sections.extend(here.iter().take(LINES).map(|p| line(p, today)));
        if here.len() > LINES {
            sections.push(format!("{} more not shown.", here.len() - LINES));
        }
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
        sections.push(format!(
            "## Pull requests tied to no item ({})",
            loose.len()
        ));
        sections.extend(
            loose
                .iter()
                .take(LINES)
                .map(|pull| format!("- {} {}", pull.label(), pull.title)),
        );
        if loose.len() > LINES {
            sections.push(format!("{} more not shown.", loose.len() - LINES));
        }
    }

    if sections.is_empty() {
        return "Nothing is assigned to you and open.".to_string();
    }

    sections.join("\n")
}

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

/// The answer to `/items`. `None` when nothing that holds work is connected.
pub async fn read(context: &recipe::Context) -> Option<Report> {
    let gathered = gather(context).await;

    if !gathered.connected {
        return None;
    }

    let today = chrono::Local::now().date_naive();
    let mut markdown = report(&gathered.items, &gathered.pulls, today);

    for (tracker, why) in &gathered.unread {
        markdown.push_str(&format!("\n{} could not be read: {why}.", tracker.name()));
    }

    let provenance = (!gathered.read.is_empty()).then(|| {
        let parts: Vec<String> = gathered
            .read
            .iter()
            .map(|(tracker, count)| format!("{} ({count})", tracker.name()))
            .collect();

        format!("Read just now: {}.", parts.join(", "))
    });

    Some(Report {
        markdown,
        provenance,
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
        }
    }

    fn pull(id: &str, title: &str, holds: &[&str]) -> Pull {
        Pull {
            id: id.to_string(),
            title: title.to_string(),
            body: String::new(),
            draft: false,
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
        assert!(text.contains("Blocked by ENG-3."), "{text}");
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
            text.contains("Pull requests: o/r#4 (checks failing)."),
            "{text}"
        );
    }

    #[test]
    fn a_linked_pull_request_that_is_fine_does_not_block() {
        let moving = item("ENG-12", Stage::Moving);
        let fine = pull("o/r#4", "feat(ENG-12): ship it", &[]);

        let text = report(&[moving], &[fine], today());

        assert!(text.contains("## In progress (1)"), "{text}");
        assert!(text.contains("Pull requests: o/r#4."), "{text}");
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
}
