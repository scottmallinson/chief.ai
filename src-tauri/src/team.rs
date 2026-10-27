//! Pull requests waiting on review for the people and repositories the user
//! chose to watch.
//!
//! This is for someone whose work is other people's work: a manager or a
//! director reading what is stuck rather than what they have written. It is
//! **opt-in** and it is a file, like `people.md`: nothing is read from GitHub
//! until the user has listed somebody or somewhere in `team.md` in their
//! corpus.
//!
//! ```text
//! ## People
//! - dvance
//! - Priya Rao = prao
//!
//! ## Repositories
//! - acme/api
//! ```
//!
//! **People first.** A manager answers for their reports and for what those
//! reports deliver, and a report may land one feature across five repositories,
//! some of which the manager has never listed. A person's open pull requests
//! are found across every repository the token can see, so nobody has to know
//! where the work is. A repository list is for a codebase a team owns.
//!
//! **Only what the user's own token can see.** The read goes to GitHub with the
//! token the user connected and asks the same search any person with that
//! account could run. A repository or an author the token cannot read answers
//! with no pull requests, which is GitHub's behaviour and cannot be told apart
//! from an empty one, so an empty answer says both.
//!
//! **Counted and sorted in code.** No model is involved: the order, the ages
//! and the reasons a pull request is waiting come from the data, so every line
//! is true or absent.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::github::{Decision, TeamPull, TeamScope};
use crate::{corpus, people, recipe, session::GithubSession};

/// Where the list lives, relative to the corpus.
pub const FILE: &str = "team.md";

/// The most repositories read in one answer, so a long list is not a long wait
/// on somebody else's API.
const MAX_REPOS: usize = 5;

/// The most people read in one answer, for the same reason.
const MAX_PEOPLE: usize = 10;

/// Pull requests read per repository. A repository that fills this says so.
const PER_REPO: u8 = 50;

/// Lines shown in one section before the rest are counted.
const LINES: usize = 10;

/// What a new file starts with. The example is a comment, so Chief reads it as
/// nothing and somebody only counts once the user has written them out.
const STARTER: &str = "# Team\n\n\
Who and what Chief should read for pull requests waiting on review. Nothing is read until you \
list someone, and only what your own GitHub account can see.\n\n\
List the people you manage by GitHub handle, with a name if you like: `- handle` or \
`- Name = handle`. Their open pull requests are found in every repository you can see. List \
repositories too if a team owns a codebase: `- owner/name`.\n\n\
<!-- An example, which Chief ignores because it is a comment:\n\
## People\n\
- dvance\n\
- Priya Rao = prao\n\
## Repositories\n\
- acme/api\n\
-->\n\n\
## People\n\n\
## Repositories\n";

/// Somebody the user listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub handle: String,
    /// The name the user wrote beside the handle, if they did.
    pub name: Option<String>,
}

/// What the file lists.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Watched {
    pub people: Vec<Member>,
    pub repositories: Vec<String>,
    /// List lines that were neither a handle nor `owner/name`, so they are
    /// reported rather than quietly doing nothing.
    pub unread: Vec<String>,
}

impl Watched {
    fn is_empty(&self) -> bool {
        self.people.is_empty() && self.repositories.is_empty()
    }
}

/// Read the file. Total: any text parses.
///
/// A line is read by what it is, not only by the heading above it, so a file
/// of bare `owner/name` lines keeps working: `acme/api` is a repository and
/// `dvance` is a person wherever they are written.
#[must_use]
pub fn parse(text: &str) -> Watched {
    let mut watched = Watched::default();
    let mut commented = false;

    for line in text.lines() {
        let line = people::outside_comments(line, &mut commented);
        let Some(item) = line.trim().strip_prefix("- ") else {
            continue;
        };
        let item = item.trim();
        let (name, target) = match item.split_once(" = ") {
            Some((name, target)) => (Some(name.trim()), target.trim()),
            None => (None, item),
        };
        let target = target.trim_start_matches('@');

        if crate::github::is_repository(target) && name.is_none() {
            if !watched
                .repositories
                .iter()
                .any(|known| known.eq_ignore_ascii_case(target))
            {
                watched.repositories.push(target.to_string());
            }
        } else if crate::github::is_login(target) {
            if !watched
                .people
                .iter()
                .any(|known| known.handle.eq_ignore_ascii_case(target))
            {
                watched.people.push(Member {
                    handle: target.to_string(),
                    name: name
                        .filter(|name| !name.is_empty())
                        .map(ToString::to_string),
                });
            }
        } else {
            watched.unread.push(item.to_string());
        }
    }

    watched
}

/// Where a pull request stands, for grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    Waiting,
    ChangesRequested,
    InReview,
    Approved,
}

fn standing(pull: &TeamPull) -> Standing {
    match pull.decision {
        Decision::Approved => Standing::Approved,
        Decision::ChangesRequested => Standing::ChangesRequested,
        Decision::Required => Standing::Waiting,
        Decision::None if pull.reviews == 0 => Standing::Waiting,
        Decision::None => Standing::InReview,
    }
}

fn open_for(created_at: &str, now: DateTime<Utc>) -> Option<String> {
    let created = DateTime::parse_from_rfc3339(created_at).ok()?;
    let days = (now - created.with_timezone(&Utc)).num_days().max(0);

    Some(match days {
        0 => "opened today".to_string(),
        1 => "open 1 day".to_string(),
        days => format!("open {days} days"),
    })
}

/// Somebody as the user would know them: the name they confirmed in
/// `people.md`, with the handle, or the handle alone.
fn who(login: &str, names: &HashMap<String, String>) -> String {
    match (login.is_empty(), names.get(&login.to_lowercase())) {
        (true, _) => "a deleted account".to_string(),
        (false, Some(name)) => format!("{name} (@{login})"),
        (false, None) => format!("@{login}"),
    }
}

/// What a line leads with: under a person's heading it is the repository, and
/// under a repository's it is the author.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    Author,
    Repository,
}

fn line(
    pull: &TeamPull,
    now: DateTime<Utc>,
    names: &HashMap<String, String>,
    show: Show,
) -> String {
    let mut parts = Vec::new();
    if show == Show::Author {
        parts.push(who(&pull.author, names));
    }
    parts.extend(open_for(&pull.created_at, now));

    match (standing(pull), pull.requested.is_empty()) {
        (Standing::Waiting | Standing::InReview, false) => {
            let asked: Vec<String> = pull
                .requested
                .iter()
                .map(|entry| match entry.strip_prefix('@') {
                    Some(login) => who(login, names),
                    None => entry.clone(),
                })
                .collect();

            parts.push(format!("review requested from {}", asked.join(", ")));
        }
        (Standing::Waiting, true) => parts.push("no reviewer requested".to_string()),
        _ => {}
    }

    let number = match show {
        Show::Author => format!("#{}", pull.number),
        Show::Repository => format!("{}#{}", pull.repository, pull.number),
    };

    format!("- {number} {}: {}", pull.title, parts.join(", "))
}

/// One repository's pull requests as sections, oldest first within each.
///
/// Drafts are counted and not listed: a draft is not waiting on anybody.
#[must_use]
pub fn describe(
    pulls: &[TeamPull],
    now: DateTime<Utc>,
    names: &HashMap<String, String>,
    show: Show,
) -> Vec<String> {
    let mut lines = Vec::new();
    let drafts = pulls.iter().filter(|pull| pull.draft).count();

    for (heading, wanted) in [
        ("Waiting for review", Standing::Waiting),
        (
            "Changes requested, waiting on the author",
            Standing::ChangesRequested,
        ),
        ("Reviewed, no decision yet", Standing::InReview),
        ("Approved, not merged", Standing::Approved),
    ] {
        let mut section: Vec<&TeamPull> = pulls
            .iter()
            .filter(|pull| !pull.draft && standing(pull) == wanted)
            .collect();

        if section.is_empty() {
            continue;
        }

        section.sort_by(|left, right| left.created_at.cmp(&right.created_at));

        lines.push(format!("{heading} ({}):", section.len()));
        lines.extend(
            section
                .iter()
                .take(LINES)
                .map(|pull| line(pull, now, names, show)),
        );
        if section.len() > LINES {
            lines.push(format!(
                "- and {} more, newer than these.",
                section.len() - LINES
            ));
        }
    }

    match drafts {
        0 => {}
        1 => lines.push("1 draft is not listed.".to_string()),
        many => lines.push(format!("{many} drafts are not listed.")),
    }

    lines
}

/// What `/team` says.
pub struct Report {
    pub markdown: String,
    /// `None` when nothing was read from GitHub.
    pub provenance: Option<String>,
}

fn how_to_turn_on(unread: &[String]) -> String {
    let mut text = format!(
        "The team view is off. It reads open pull requests for the people and repositories you \
         list in `{FILE}` in your Chief folder, using your own GitHub account, and nothing until \
         you do. Add a line such as `- dvance` (a GitHub handle) under People, then ask again."
    );

    if !unread.is_empty() {
        text.push_str(&format!(
            "\n\nThese lines are neither a GitHub handle nor `owner/name`, so they were ignored: \
             {}.",
            unread.join(", ")
        ));
    }

    text
}

/// How many were read, and a note when the pull requests filled the page.
fn read_note(count: usize) -> Option<String> {
    (count >= usize::from(PER_REPO)).then(|| {
        format!("Only the {PER_REPO} oldest open pull requests were read; there may be more.")
    })
}

/// Read what the user listed and say what is waiting.
///
/// `None` when no GitHub account is connected, so the caller can say that
/// instead of this.
pub async fn read(context: &recipe::Context) -> Option<Report> {
    let text = match context.corpus.read(FILE).await {
        Ok(text) => text,
        Err(corpus::Error::NoSuchFile(_)) => {
            // Created so there is a file to open and an example to copy from.
            if let Err(error) = context.corpus.write(FILE, STARTER).await {
                eprintln!("team: could not write {FILE}: {error}");
            }
            return Some(Report {
                markdown: how_to_turn_on(&[]),
                provenance: None,
            });
        }
        Err(error) => {
            eprintln!("team: could not read {FILE}: {error}");
            return Some(Report {
                markdown: format!("Chief could not read `{FILE}`, so nothing was read."),
                provenance: None,
            });
        }
    };

    let watched = parse(&text);
    if watched.is_empty() {
        return Some(Report {
            markdown: how_to_turn_on(&watched.unread),
            provenance: None,
        });
    }

    let account = recipe::github_accounts(context).await.first().copied()?;
    let session = GithubSession::new(&context.pool, &context.github, account);

    // Only people the user named, here or as confirmed in people.md, are named;
    // the rest are handles.
    let mut names: HashMap<String, String> = match context.corpus.read(people::FILE).await {
        Ok(file) => people::parse(&file)
            .confirmed
            .into_iter()
            .map(|person| (person.handle.to_lowercase(), person.name))
            .collect(),
        Err(_) => HashMap::new(),
    };
    for member in &watched.people {
        if let Some(name) = &member.name {
            names.insert(member.handle.to_lowercase(), name.clone());
        }
    }

    let now = Utc::now();
    let mut sections = vec!["## Pull requests waiting".to_string()];

    for member in watched.people.iter().take(MAX_PEOPLE) {
        sections.push(format!("### {}", who(&member.handle, &names)));

        match session
            .team_pulls(TeamScope::Author(&member.handle), PER_REPO)
            .await
        {
            Ok(pulls) if pulls.is_empty() => {
                sections.push("Nothing open, or none that your GitHub account can see.".to_string())
            }
            Ok(pulls) => {
                sections.extend(describe(&pulls, now, &names, Show::Repository));
                sections.extend(read_note(pulls.len()));
            }
            Err(error) => sections.push(format!("Could not be read: {error}")),
        }
    }

    for repository in watched.repositories.iter().take(MAX_REPOS) {
        sections.push(format!("### {repository}"));

        match session
            .team_pulls(TeamScope::Repository(repository), PER_REPO)
            .await
        {
            Ok(pulls) if pulls.is_empty() => sections.push(
                "Nothing open, or your GitHub account cannot see this repository.".to_string(),
            ),
            Ok(pulls) => {
                sections.extend(describe(&pulls, now, &names, Show::Author));
                sections.extend(read_note(pulls.len()));
            }
            Err(error) => sections.push(format!("Could not be read: {error}")),
        }
    }

    if watched.people.len() > MAX_PEOPLE {
        sections.push(format!(
            "Only the first {MAX_PEOPLE} of {} listed people were read.",
            watched.people.len()
        ));
    }
    if watched.repositories.len() > MAX_REPOS {
        sections.push(format!(
            "Only the first {MAX_REPOS} of {} listed repositories were read.",
            watched.repositories.len()
        ));
    }
    if !watched.unread.is_empty() {
        sections.push(format!(
            "Ignored, neither a GitHub handle nor `owner/name`: {}.",
            watched.unread.join(", ")
        ));
    }

    Some(Report {
        markdown: sections.join("\n"),
        provenance: Some(format!(
            "Read from GitHub just now, for who and what is listed in {FILE}."
        )),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::TeamPull;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-27T12:00:00Z")
            .expect("a time")
            .with_timezone(&Utc)
    }

    fn pull(number: i64, created: &str, decision: Decision) -> TeamPull {
        TeamPull {
            repository: "acme/api".to_string(),
            number,
            title: format!("Change {number}"),
            url: String::new(),
            author: "dvance".to_string(),
            created_at: created.to_string(),
            draft: false,
            decision,
            reviews: 0,
            requested: Vec::new(),
        }
    }

    #[test]
    fn reads_people_and_repositories_by_what_each_line_is_outside_comments() {
        let watched = parse(
            "# Team\n## People\n- dvance\n- Priya Rao = @prao\n<!-- - hidden -->\n- DVANCE\n\
             ## Repositories\n- acme/api\n- ACME/API\n- not valid\n- acme/web\n",
        );

        assert_eq!(
            watched.people,
            vec![
                Member {
                    handle: "dvance".to_string(),
                    name: None
                },
                Member {
                    handle: "prao".to_string(),
                    name: Some("Priya Rao".to_string())
                },
            ]
        );
        assert_eq!(watched.repositories, vec!["acme/api", "acme/web"]);
        assert_eq!(watched.unread, vec!["not valid"]);
    }

    #[test]
    fn a_file_of_bare_repositories_still_works() {
        let watched = parse("- acme/api\n- acme/web\n");

        assert_eq!(watched.repositories, vec!["acme/api", "acme/web"]);
        assert!(watched.people.is_empty());
    }

    #[test]
    fn the_starters_example_is_not_anybody() {
        assert_eq!(parse(STARTER), Watched::default());
    }

    #[test]
    fn groups_by_what_the_pull_request_is_waiting_for_oldest_first() {
        let mut approved = pull(4, "2026-10-01T09:00:00Z", Decision::Approved);
        approved.reviews = 1;
        let mut draft = pull(5, "2026-10-02T09:00:00Z", Decision::None);
        draft.draft = true;
        let mut asked = pull(2, "2026-10-20T12:00:00Z", Decision::Required);
        asked.requested = vec!["@prao".to_string(), "team platform".to_string()];

        let lines = describe(
            &[
                pull(1, "2026-10-26T09:00:00Z", Decision::None),
                asked,
                pull(3, "2026-10-10T09:00:00Z", Decision::ChangesRequested),
                approved,
                draft,
            ],
            now(),
            &HashMap::new(),
            Show::Author,
        );

        assert_eq!(
            lines,
            vec![
                "Waiting for review (2):",
                "- #2 Change 2: @dvance, open 7 days, review requested from @prao, team platform",
                "- #1 Change 1: @dvance, open 1 day, no reviewer requested",
                "Changes requested, waiting on the author (1):",
                "- #3 Change 3: @dvance, open 17 days",
                "Approved, not merged (1):",
                "- #4 Change 4: @dvance, open 26 days",
                "1 draft is not listed.",
            ]
        );
    }

    /// A pull request with no decision recorded but a review already given is
    /// not "waiting for review".
    ///
    /// Proved by dropping the `reviews == 0` condition:
    ///
    /// ```text
    /// a reviewed pull request must not be reported as waiting for a review
    /// ```
    #[test]
    fn a_reviewed_pull_request_is_not_reported_as_waiting_for_a_review() {
        let mut reviewed = pull(7, "2026-10-20T09:00:00Z", Decision::None);
        reviewed.reviews = 2;

        let lines = describe(&[reviewed], now(), &HashMap::new(), Show::Author);

        assert!(
            !lines
                .iter()
                .any(|line| line.starts_with("Waiting for review")),
            "a reviewed pull request must not be reported as waiting for a review: {lines:?}"
        );
        assert_eq!(lines[0], "Reviewed, no decision yet (1):");
    }

    #[test]
    fn names_only_people_the_user_confirmed() {
        let names = HashMap::from([("dvance".to_string(), "Dana Vance".to_string())]);
        let mut asked = pull(1, "2026-10-26T12:00:00Z", Decision::Required);
        asked.requested = vec!["@prao".to_string(), "@dvance".to_string()];

        let lines = describe(&[asked], now(), &names, Show::Author);

        assert_eq!(
            lines[1],
            "- #1 Change 1: Dana Vance (@dvance), open 1 day, review requested from @prao, \
             Dana Vance (@dvance)"
        );
    }

    #[test]
    fn counts_the_rest_instead_of_dropping_them() {
        let pulls: Vec<TeamPull> = (1..=12)
            .map(|number| pull(number, "2026-10-20T09:00:00Z", Decision::Required))
            .collect();

        let lines = describe(&pulls, now(), &HashMap::new(), Show::Author);

        assert_eq!(lines[0], "Waiting for review (12):");
        assert_eq!(lines.len(), 1 + LINES + 1);
        assert_eq!(
            lines.last().map(String::as_str),
            Some("- and 2 more, newer than these.")
        );
    }

    #[test]
    fn under_a_person_each_line_leads_with_the_repository_not_the_author() {
        let mut asked = pull(2, "2026-10-20T12:00:00Z", Decision::Required);
        asked.requested = vec!["@prao".to_string()];

        let lines = describe(&[asked], now(), &HashMap::new(), Show::Repository);

        assert_eq!(
            lines[1],
            "- acme/api#2 Change 2: open 7 days, review requested from @prao"
        );
    }
}
