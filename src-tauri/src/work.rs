//! A live look at the user's work, for the questions that are about all of it.
//!
//! "What should I prioritise?", "what did I complete yesterday?" and "draft my
//! standup" are not about one tool. Left to the tool loop, a small model asked
//! for context it could have looked up, or said it had no access to a log it
//! did. This reads what those questions are made of — what is open, what other
//! people are waiting on, what was finished lately, today's meetings and unread
//! mail — as it is now, and hands it over as short lines.
//!
//! **No tool is assumed.** The work lives wherever the user keeps it: pull
//! requests and issues on GitHub, tickets in Linear or Jira, meetings in Outlook
//! or a subscribed calendar. Each source that is connected is read and the rest
//! are simply not part of the answer, so a user with only Linear gets a Linear
//! answer and one with all of them gets one overview.
//!
//! **Live, not logged.** The work log only ever grows: a pull request that has
//! since merged still sits in it as "open", and a review done at nine still
//! reads as waiting at noon. That is the stale "Still open" and "Waiting on
//! review" in a standup. A source that cannot be read is named as such rather
//! than reported as empty, because "you have nothing open" and "Linear could not
//! be reached" are opposite statements.
//!
//! **Lines, not JSON.** A pull request as the tools return it carries a URL, an
//! update time and an account on every entry; none of that is wanted to answer
//! these questions and all of it is prompt. One line each.

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Weekday};
use serde_json::{json, Value};

use crate::github;
use crate::integrations;
use crate::microsoft;
use crate::recipe;
use crate::session::{GithubSession, OutlookSession};
use crate::sources::{self, Found};
use crate::tools::{self, Context, PullRequestState, Whose};

/// The most lines a *question* is handed from any one source. A question about
/// "my work" is answered in a handful of items; the rest would be prefill and a
/// longer wait for an answer nobody reads to the end. What is not shown is
/// counted and said. A standup reads without this limit, since it is written a
/// few lines at a time.
pub const MOST: usize = 8;

/// The most a question about a *period* is handed from any one source. Asked
/// about a month, eight lines is an answer that stops at the eighth merge and
/// says so; the person asked for the month. Each line is about 25 tokens, so
/// this is a few seconds of prefill rather than a longer prompt than the model
/// can read.
pub const PERIOD: usize = 60;

/// How many merged pull requests one account's search returns: GitHub's page.
const MERGED_PAGE: u8 = 100;

/// One kind of thing, read from every connected source that holds it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Part {
    /// What was found, one line each.
    pub lines: Vec<String>,
    /// The sources that answered. Empty means nothing that holds this kind of
    /// thing is connected, or everything that does failed.
    pub read: Vec<&'static str>,
    /// The sources that are connected and could not be read, and why.
    pub unread: Vec<(&'static str, String)>,
    /// Lines found and not kept, because of the limit.
    pub more: usize,
}

impl Part {
    /// Whether something was asked for and nothing answered. The caller's cue
    /// to say so, or to fall back on the log, rather than report an empty day.
    pub fn is_unread(&self) -> bool {
        self.read.is_empty() && !self.unread.is_empty()
    }
}

/// The user's work right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Work {
    /// Their own pull requests, issues and tickets that are not finished.
    pub open: Part,
    /// Pull requests other people are waiting on them to review.
    pub reviews: Part,
    /// What they finished since the cut-off, each saying how long ago.
    pub done: Part,
    /// Today's meetings that have not finished, in the order they happen.
    pub meetings: Part,
    /// Unread mail, newest first.
    pub mail: Part,
    /// The day `done` reaches back to, so an answer can say how far it looked.
    pub done_since: Option<NaiveDate>,
}

/// What to read, and how much of it.
#[derive(Debug, Clone, Copy)]
pub struct Scope {
    /// Finished work counts from this day.
    pub done_since: NaiveDate,
    /// Whether to read today's meetings and unread mail too. A standup is not
    /// about the calendar, and a Graph call nobody reads is a wait for nothing.
    pub day: bool,
    /// The most lines kept from any one source.
    pub limit: usize,
}

/// One source's answer, before it is folded into a [`Part`].
struct Reading {
    name: &'static str,
    /// `Some` when the source answered, however empty.
    lines: Option<Vec<String>>,
    /// Why each connected account that did not answer could not be read.
    failed: Vec<String>,
}

impl Reading {
    /// Nothing of this kind is connected.
    fn absent(name: &'static str) -> Self {
        Self {
            name,
            lines: None,
            failed: Vec::new(),
        }
    }

    fn of<T>(name: &'static str, found: Found<T>, line: impl Fn(&T) -> Option<String>) -> Self {
        Self {
            name,
            lines: found
                .answered
                .then(|| found.items.iter().filter_map(&line).collect()),
            failed: found.failed,
        }
    }
}

fn part(readings: Vec<Reading>, limit: usize) -> Part {
    let mut part = Part::default();

    for reading in readings {
        if let Some(mut lines) = reading.lines {
            part.read.push(reading.name);
            part.more += lines.len().saturating_sub(limit);
            lines.truncate(limit);
            part.lines.extend(lines);
        }

        for why in reading.failed {
            part.unread.push((reading.name, why));
        }
    }

    part
}

/// Read everything, one source after another.
///
/// Sequential rather than joined: the reads are a few hundred milliseconds
/// each against a model that takes tens of seconds, and a fixed order is what
/// lets a stub server be scripted.
pub async fn read(context: &Context, scope: &Scope) -> Work {
    let now = Local::now();
    let today = now.date_naive();
    let limit = scope.limit;

    let open = part(
        vec![
            pull_requests(context, PullRequestState::Open, Whose::Mine, |pr| {
                Some(describe(
                    pr,
                    if pr["draft"] == true { ", draft" } else { "" },
                ))
            })
            .await,
            github_issues(context).await,
            Reading::of("Linear", sources::linear(context, None).await, |issue| {
                Some(linear_line(issue, ""))
            }),
            Reading::of("Jira", sources::jira(context, None).await, |issue| {
                Some(crate::atlassian::describe(issue))
            }),
        ],
        limit,
    );

    let reviews = part(
        vec![
            pull_requests(context, PullRequestState::Open, Whose::Reviewing, |pr| {
                Some(describe(pr, ""))
            })
            .await,
        ],
        limit,
    );

    let since = scope.done_since;
    let done = part(
        vec![
            merged_pull_requests(context, since, |pr| {
                let merged = merged_on(pr["merged_at"].as_str()?)?;

                (merged >= since)
                    .then(|| describe(pr, &format!(", merged {}", when(merged, today))))
            })
            .await,
            Reading::of(
                "Linear",
                sources::linear(context, Some(since)).await,
                |issue| {
                    let day = merged_on(issue.completed_at.as_deref()?)?;

                    (day >= since)
                        .then(|| linear_line(issue, &format!("done {}", when(day, today))))
                },
            ),
            Reading::of("Jira", sources::jira(context, Some(since)).await, |issue| {
                let day = resolved_on(issue.resolved.as_deref()?)?;

                (day >= since).then(|| {
                    format!(
                        "{} {} (done {})",
                        issue.key,
                        issue.summary,
                        when(day, today)
                    )
                })
            }),
        ],
        limit,
    );

    let (meetings, mail) = if scope.day {
        (
            part(day_meetings(context, now).await, limit),
            part(vec![unread_mail(context).await], limit),
        )
    } else {
        (Part::default(), Part::default())
    };

    Work {
        open,
        reviews,
        done,
        meetings,
        mail,
        done_since: Some(since),
    }
}

/// The cut-off for "what I did lately" in a standup: the last working day.
///
/// A standup on Tuesday reports Monday; one on Monday reports Friday, which is
/// why this is not simply yesterday — on a Monday morning "yesterday" is a
/// Sunday with nothing in it, and the one standup of the week with the most to
/// say would open by saying there was nothing.
pub fn last_working_day(today: NaiveDate) -> NaiveDate {
    let back = match today.weekday() {
        Weekday::Mon => 3,
        Weekday::Sun => 2,
        _ => 1,
    };

    today - Duration::days(back)
}

/// The cut-off for a general look at the user's work: the last week.
#[cfg(test)]
fn last_week(today: NaiveDate) -> NaiveDate {
    today - Duration::days(6)
}

/// "today", "yesterday" or "N days ago".
fn when(day: NaiveDate, today: NaiveDate) -> String {
    match (today - day).num_days() {
        0 => "today".to_string(),
        1 => "yesterday".to_string(),
        days if days > 1 => format!("{days} days ago"),
        // A timestamp in the future is a clock disagreeing, not news.
        _ => "today".to_string(),
    }
}

/// The local date an RFC 3339 instant falls on.
fn merged_on(instant: &str) -> Option<NaiveDate> {
    DateTime::parse_from_rfc3339(instant)
        .ok()
        .map(|parsed| parsed.with_timezone(&Local).date_naive())
}

/// The local date Jira's `resolutiondate` falls on. Jira writes the offset
/// without a colon (`+0100`), which RFC 3339 does not allow.
fn resolved_on(instant: &str) -> Option<NaiveDate> {
    DateTime::parse_from_str(instant, "%Y-%m-%dT%H:%M:%S%.f%z")
        .ok()
        .or_else(|| DateTime::parse_from_rfc3339(instant).ok())
        .map(|parsed| parsed.with_timezone(&Local).date_naive())
}

/// `title (repo#number, note)`.
fn describe(pr: &Value, note: &str) -> String {
    format!(
        "{} ({}#{}{note})",
        pr["title"].as_str().unwrap_or("Untitled"),
        pr["repository"].as_str().unwrap_or("unknown"),
        pr["number"],
    )
}

/// `REC-42 Title [state, note]`. A Linear issue says how urgent it is.
fn linear_line(issue: &crate::linear::Issue, note: &str) -> String {
    let urgency = match issue.priority {
        p if (p - 1.0).abs() < f64::EPSILON => "urgent",
        p if (p - 2.0).abs() < f64::EPSILON => "high priority",
        _ => "",
    };
    let marks: Vec<&str> = [issue.state.as_str(), urgency, note]
        .into_iter()
        .filter(|mark| !mark.is_empty())
        .collect();

    if marks.is_empty() {
        format!("{} {}", issue.identifier, issue.title)
    } else {
        format!(
            "{} {} [{}]",
            issue.identifier,
            issue.title,
            marks.join(", ")
        )
    }
}

/// One GitHub read, turned into lines by `line`, which may drop an entry.
///
/// No account is "not part of the setup"; an account that fails is said.
async fn pull_requests(
    context: &Context,
    state: PullRequestState,
    whose: Whose,
    line: impl Fn(&Value) -> Option<String>,
) -> Reading {
    match tools::fetch_github_prs(&context.pool, &context.github, state, whose, None).await {
        Ok(read) => Reading {
            name: "GitHub",
            lines: Some(
                read["pull_requests"]
                    .as_array()
                    .map(|entries| entries.iter().filter_map(&line).collect())
                    .unwrap_or_default(),
            ),
            failed: Vec::new(),
        },
        Err(github::Error::NotConnected) => Reading::absent("GitHub"),
        Err(error) => {
            eprintln!("could not read pull requests for the work overview: {error}");
            Reading {
                name: "GitHub",
                lines: None,
                failed: vec![error.to_string()],
            }
        }
    }
}

/// The user's pull requests merged on or after `since`, from every GitHub
/// account. The date is part of the search, so a month is a month however busy
/// it was, up to the hundred a page holds.
async fn merged_pull_requests(
    context: &Context,
    since: NaiveDate,
    line: impl Fn(&Value) -> Option<String>,
) -> Reading {
    let accounts = match integrations::accounts(&context.pool, integrations::GITHUB).await {
        Ok(accounts) if !accounts.is_empty() => accounts,
        _ => return Reading::absent("GitHub"),
    };

    let mut lines = Vec::new();
    let mut failed = Vec::new();
    let mut answered = false;

    for account in &accounts {
        match GithubSession::new(&context.pool, &context.github, account.id)
            .merged_since(since, MERGED_PAGE)
            .await
        {
            Ok(found) => {
                answered = true;
                let name = account
                    .label
                    .clone()
                    .or_else(|| account.identity.clone())
                    .unwrap_or_else(|| account.account_key.clone());

                lines.extend(
                    github::as_tool_entries(&name, &found)
                        .iter()
                        .filter_map(&line),
                );
            }
            Err(error) => {
                eprintln!("could not read merged pull requests for the work overview: {error}");
                failed.push(error.to_string());
            }
        }
    }

    Reading {
        name: "GitHub",
        lines: answered.then_some(lines),
        failed,
    }
}

/// Issues assigned to the user on GitHub, which are work like any tracker's.
async fn github_issues(context: &Context) -> Reading {
    let Ok(accounts) = integrations::accounts(&context.pool, integrations::GITHUB).await else {
        return Reading::absent("GitHub");
    };

    if accounts.is_empty() {
        return Reading::absent("GitHub");
    }

    let mut found: Found<github::Issue> = Found {
        connected: true,
        ..Found::default()
    };

    for account in &accounts {
        match GithubSession::new(&context.pool, &context.github, account.id)
            .assigned_issues(recipe::PER_SOURCE)
            .await
        {
            Ok(issues) => {
                found.answered = true;
                found.items.extend(issues);
            }
            Err(error) => found.failed.push(error.to_string()),
        }
    }

    Reading::of("GitHub", found, |issue| {
        Some(format!(
            "{} ({}#{}, issue)",
            issue.title, issue.repository, issue.number
        ))
    })
}

/// Today's meetings from Outlook and from subscribed calendars, in the order
/// the day happens, minus the ones that have finished.
async fn day_meetings(context: &Context, now: DateTime<Local>) -> Vec<Reading> {
    let (from, to) = recipe::today_at(&now);
    let mut lines = Vec::new();
    let mut outlook = Reading::absent("Outlook");

    let accounts = integrations::accounts(&context.pool, integrations::MICROSOFT)
        .await
        .unwrap_or_default();

    for account in &accounts {
        let session = OutlookSession::new(&context.pool, &context.microsoft, account.id);

        match session.events(&from, &to, recipe::CALENDAR_LIMIT).await {
            Ok(events) => {
                outlook.lines.get_or_insert_with(Vec::new);
                lines.extend(events.iter().map(recipe::describe_event));
            }
            Err(error) => outlook.failed.push(error.to_string()),
        }
    }

    let subscribed = sources::subscribed(context, now).await;
    let calendar_failed = subscribed.failed.clone();
    let calendar_answered = subscribed.answered;
    lines.extend(subscribed.items.iter().map(recipe::describe_event));

    recipe::tidy_agenda(&mut lines);
    let lines = recipe::still_to_come(lines, &now);

    // The lines are one agenda; saying which calendar each came from would be
    // the reader doing the merge Chief is for. They are credited to the first
    // source that answered, and every failure is still named.
    let mut readings = Vec::new();
    let mut agenda = Some(lines);

    if outlook.lines.is_some() {
        outlook.lines = agenda.take();
    }
    readings.push(outlook);

    readings.push(Reading {
        name: "Calendar",
        lines: if calendar_answered {
            Some(agenda.take().unwrap_or_default())
        } else {
            None
        },
        failed: calendar_failed,
    });

    readings
}

/// Unread mail as `sender: subject`, from every Outlook account.
async fn unread_mail(context: &Context) -> Reading {
    let accounts = integrations::accounts(&context.pool, integrations::MICROSOFT)
        .await
        .unwrap_or_default();

    let mut found: Found<microsoft::MailMessage> = Found {
        connected: !accounts.is_empty(),
        ..Found::default()
    };

    for account in &accounts {
        match OutlookSession::new(&context.pool, &context.microsoft, account.id)
            .messages(recipe::PER_SOURCE)
            .await
        {
            Ok(messages) => {
                found.answered = true;
                found
                    .items
                    .extend(messages.into_iter().filter(|message| message.unread));
            }
            Err(error) => found.failed.push(error.to_string()),
        }
    }

    Reading::of("Outlook", found, |message| {
        Some(recipe::describe_message(message))
    })
}

impl Work {
    /// Everything, as a tool result.
    ///
    /// A source that could not be read says so under `not_read`, never as an
    /// empty list, and lines that were found and not shown are counted under
    /// `not_shown`. Nothing at all readable is the caller's error to raise, so
    /// the model is not handed a result made only of excuses.
    pub fn as_tool_result(&self) -> Option<Value> {
        let parts = [
            ("my_open_work", &self.open),
            ("review_requests_waiting_on_me", &self.reviews),
            ("my_recently_completed_work", &self.done),
            ("my_meetings_today", &self.meetings),
            ("my_unread_mail", &self.mail),
        ];

        let mut result = serde_json::Map::new();
        let mut not_read = Vec::new();
        let mut not_shown = serde_json::Map::new();
        let mut sources_read: Vec<&str> = Vec::new();

        for (name, part) in parts {
            if !part.read.is_empty() {
                result.insert(name.to_string(), json!(part.lines));

                for source in &part.read {
                    if !sources_read.contains(source) {
                        sources_read.push(source);
                    }
                }
            }

            for (source, why) in &part.unread {
                not_read.push(json!({ "list": name, "source": source, "why": why }));
            }

            if part.more > 0 {
                not_shown.insert(name.to_string(), json!(part.more));
            }
        }

        if result.is_empty() {
            return None;
        }

        result.insert("read_from".to_string(), json!(sources_read));

        if let (Some(since), false) = (self.done_since, self.done.read.is_empty()) {
            result.insert(
                "my_recently_completed_work_covers".to_string(),
                json!(format!("{} to today", since.format("%Y-%m-%d"))),
            );
        }

        if !not_read.is_empty() {
            result.insert("not_read".to_string(), Value::Array(not_read));
        }

        if !not_shown.is_empty() {
            result.insert("not_shown".to_string(), Value::Object(not_shown));
        }

        result.insert("how_to_use".to_string(), json!(HOW_TO_USE));

        Some(Value::Object(result))
    }
}

/// What to do with the result, said once in the result itself.
///
/// In the result rather than in the tool's description: the description is paid
/// for on every question, this only on the ones that call it.
const HOW_TO_USE: &str = "Answer from these lines only, in plain text. To prioritise, give at \
    most five items in order with a few words of reason each: reviews other people are waiting \
    on first, then urgent or in-progress open work, mentioning a meeting that limits the time. \
    For what was done, use the completed list and its dates. A list that is empty means there is \
    nothing; one under not_read could not be read, so say that and do not guess. Say how many \
    more are under not_shown. If my_recently_completed_work_covers is shorter than the period \
    the user asked about, call this tool again with more days.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::migrated_pool;
    use crate::llama::test_support::serve;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("a date")
    }

    #[test]
    fn a_standup_looks_back_to_the_last_working_day() {
        // 2026-10-26 is a Monday.
        assert_eq!(last_working_day(date(2026, 10, 26)), date(2026, 10, 23));
        assert_eq!(last_working_day(date(2026, 10, 27)), date(2026, 10, 26));
        assert_eq!(last_working_day(date(2026, 10, 25)), date(2026, 10, 23));
        assert_eq!(last_working_day(date(2026, 10, 24)), date(2026, 10, 23));
    }

    #[test]
    fn says_how_long_ago_in_words_the_model_cannot_get_wrong() {
        let today = date(2026, 10, 23);

        assert_eq!(when(today, today), "today");
        assert_eq!(when(date(2026, 10, 22), today), "yesterday");
        assert_eq!(when(date(2026, 10, 20), today), "3 days ago");
        assert_eq!(when(date(2026, 10, 24), today), "today");
    }

    #[test]
    fn jira_writes_its_offset_without_a_colon_and_that_still_reads() {
        assert!(resolved_on("2026-10-22T10:15:30.000+0100").is_some());
        assert!(resolved_on("2026-10-22T10:15:30.000+01:00").is_some());
        assert_eq!(resolved_on("last tuesday"), None);
    }

    #[test]
    fn a_linear_issue_says_how_urgent_it_is_and_what_became_of_it() {
        let issue = |priority: f64, state: &str| crate::linear::Issue {
            identifier: "REC-42".to_string(),
            title: "Read Linear".to_string(),
            state: state.to_string(),
            priority,
            completed_at: None,
        };

        assert_eq!(
            linear_line(&issue(1.0, "In Progress"), ""),
            "REC-42 Read Linear [In Progress, urgent]"
        );
        assert_eq!(
            linear_line(&issue(2.0, "Todo"), ""),
            "REC-42 Read Linear [Todo, high priority]"
        );
        assert_eq!(
            linear_line(&issue(0.0, "Done"), "done yesterday"),
            "REC-42 Read Linear [Done, done yesterday]"
        );
        assert_eq!(linear_line(&issue(0.0, ""), ""), "REC-42 Read Linear");
    }

    fn reading(name: &'static str, lines: Option<&[&str]>, failed: &[&str]) -> Reading {
        Reading {
            name,
            lines: lines.map(|lines| lines.iter().map(ToString::to_string).collect()),
            failed: failed.iter().map(ToString::to_string).collect(),
        }
    }

    /// Sources that answered are credited, the ones that failed are named with
    /// why, and the ones that are not connected are not mentioned at all.
    #[test]
    fn a_part_credits_what_answered_and_names_what_did_not() {
        let part = part(
            vec![
                reading("GitHub", Some(&["a (o/r#1)", "b (o/r#2)"]), &[]),
                reading("Linear", None, &["Linear could not be reached"]),
                reading("Jira", None, &[]),
            ],
            usize::MAX,
        );

        assert_eq!(part.lines.len(), 2);
        assert_eq!(part.read, ["GitHub"]);
        assert_eq!(
            part.unread,
            [("Linear", "Linear could not be reached".to_string())]
        );
        assert!(!part.is_unread(), "GitHub answered");
    }

    #[test]
    fn a_part_nothing_answered_is_unread_and_a_part_nothing_holds_is_not() {
        let failed = part(
            vec![reading("Linear", None, &["Linear could not be reached"])],
            usize::MAX,
        );
        assert!(failed.is_unread());

        let absent = part(vec![reading("Linear", None, &[])], usize::MAX);
        assert!(!absent.is_unread());
        assert!(absent.read.is_empty() && absent.unread.is_empty());
    }

    /// What is cut from the lines handed to a question is counted, so the
    /// answer can say so.
    ///
    /// Proved by dropping the `more` count: the model is not told.
    #[test]
    fn lines_beyond_the_limit_are_counted_not_lost_silently() {
        let lines: Vec<String> = (1..=11).map(|n| format!("item {n}")).collect();
        let all: Vec<&str> = lines.iter().map(String::as_str).collect();

        let part = part(vec![reading("Linear", Some(&all), &[])], 8);

        assert_eq!(part.lines.len(), 8);
        assert_eq!(part.more, 3);

        let work = Work {
            open: part,
            ..Work::default()
        };
        let result = work.as_tool_result().expect("something was read");

        assert_eq!(result["not_shown"]["my_open_work"], 3);
    }

    #[test]
    fn a_source_that_could_not_be_read_is_not_reported_as_empty() {
        let work = Work {
            open: part(vec![reading("Linear", Some(&[]), &[])], 8),
            reviews: part(
                vec![reading("GitHub", None, &["GitHub could not be reached"])],
                8,
            ),
            done: part(
                vec![reading(
                    "GitHub",
                    Some(&["Fix it (o/r#1, merged yesterday)"]),
                    &[],
                )],
                8,
            ),
            ..Work::default()
        };

        let result = work.as_tool_result().expect("something was read");

        assert_eq!(result["my_open_work"], json!([]));
        assert!(result.get("review_requests_waiting_on_me").is_none());
        assert_eq!(result["not_read"][0]["source"], "GitHub");
        assert_eq!(result["read_from"], json!(["Linear", "GitHub"]));
        assert!(
            result.get("my_meetings_today").is_none(),
            "a calendar that was never asked about is not an empty one"
        );
    }

    #[test]
    fn nothing_readable_at_all_is_not_a_result() {
        let work = Work {
            open: part(vec![reading("Linear", None, &["not connected"])], 8),
            ..Work::default()
        };

        assert!(work.as_tool_result().is_none());
        assert!(Work::default().as_tool_result().is_none());
    }

    async fn context_with_linear(linear_host: &str) -> Context {
        Context {
            pool: migrated_pool().await,
            github: github::Client::against("http://127.0.0.1:1").expect("client"),
            microsoft: microsoft::Client::against("http://127.0.0.1:1").expect("client"),
            calendar: crate::calendar::Client::new().expect("client"),
            linear: crate::linear::Client::against(linear_host).expect("client"),
            atlassian: crate::atlassian::Client::new().expect("client"),
            atlassian_rest: crate::atlassian::rest::Rest::new().expect("client"),
        }
    }

    async fn connect_linear(context: &Context) {
        integrations::save(
            &context.pool,
            integrations::NewAccount {
                service: integrations::LINEAR,
                account_key: "acme",
                identity: Some("scott@example.com"),
                credential_kind: integrations::API_KEY,
                access_token: "lin_api_test",
                refresh_token: None,
                expires_at: None,
                scopes: None,
                client_id: None,
                client_secret: None,
            },
        )
        .await
        .expect("should store a key");
    }

    /// **A month is a month.** GitHub's search sorts by last update, so "the
    /// latest 25 merged, minus the old ones" answered a question about a month
    /// with whatever fitted, and the follow-up "how far back did you look?" had
    /// no honest answer. The date is now in the search, and a merge three weeks
    /// old is kept.
    ///
    /// Proved by dropping the `merged:>=` qualifier from the query:
    ///
    /// ```text
    /// the date was not part of the search
    /// ```
    #[tokio::test]
    async fn merged_work_is_asked_of_github_from_the_date_and_a_month_back_is_kept() {
        let today = Local::now().date_naive();
        let since = today - Duration::days(29);
        let merged = (Local::now() - Duration::days(21)).to_rfc3339();
        let page = format!(
            r#"{{ "total_count": 1, "items": [{{
                "number": 31, "title": "Three weeks ago",
                "repository_url": "https://api.github.com/repos/acme/api",
                "state": "closed", "draft": false,
                "html_url": "https://github.com/acme/api/pull/31",
                "updated_at": "{merged}",
                "pull_request": {{ "merged_at": "{merged}" }} }}] }}"#
        );
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", page)]);
        let context = Context {
            github: github::Client::against(&host).expect("client"),
            ..context_with_linear("http://127.0.0.1:1").await
        };
        integrations::save(
            &context.pool,
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

        let reading = merged_pull_requests(&context, since, |pr| {
            let day = merged_on(pr["merged_at"].as_str()?)?;

            Some(describe(pr, &format!(", merged {}", when(day, today))))
        })
        .await;

        assert_eq!(
            reading.lines,
            Some(vec![
                "Three weeks ago (acme/api#31, merged 21 days ago)".to_string()
            ])
        );

        let sent = server.await.expect("server").join("");
        assert!(
            sent.contains(&format!("merged%3A%3E%3D{}", since.format("%Y-%m-%d"))),
            "the date was not part of the search: {sent}"
        );
    }

    /// **GitHub is not assumed.** A user who keeps their work in Linear and
    /// nowhere else gets an overview of it: open issues now, finished ones with
    /// when, and no mention of a GitHub they never connected. It used to answer
    /// "no GitHub account is connected" to everyone without one.
    ///
    /// Proved by making `read` skip Linear:
    ///
    /// ```text
    /// the open work is Linear's: []
    /// ```
    #[tokio::test]
    async fn a_user_with_only_linear_gets_a_linear_overview() {
        let yesterday = (Local::now() - Duration::days(1)).to_rfc3339();
        let open = r#"{ "data": {
            "viewer": { "name": "Scott", "email": "scott@example.com" },
            "issues": { "nodes": [
                { "identifier": "REC-42", "title": "Read Linear", "priority": 1,
                  "state": { "name": "In Progress" } } ] } } }"#;
        let done = format!(
            r#"{{ "data": {{
            "viewer": {{ "name": "Scott", "email": "scott@example.com" }},
            "issues": {{ "nodes": [
                {{ "identifier": "REC-40", "title": "Shipped it", "priority": 0,
                   "state": {{ "name": "Done" }}, "completedAt": "{yesterday}" }} ] }} }} }}"#
        );
        let (host, server) = serve(vec![
            ("HTTP/1.1 200 OK", open.to_string()),
            ("HTTP/1.1 200 OK", done),
        ]);
        let context = context_with_linear(&host).await;
        connect_linear(&context).await;
        let today = Local::now().date_naive();

        let work = read(
            &context,
            &Scope {
                done_since: last_week(today),
                day: false,
                limit: MOST,
            },
        )
        .await;

        assert_eq!(work.open.read, ["Linear"], "the open work is Linear's");
        assert_eq!(
            work.open.lines,
            ["REC-42 Read Linear [In Progress, urgent]"]
        );
        assert_eq!(
            work.done.lines,
            ["REC-40 Shipped it [Done, done yesterday]"]
        );
        assert!(
            work.reviews.read.is_empty() && work.reviews.unread.is_empty(),
            "no GitHub is connected, so reviews are not mentioned: {:?}",
            work.reviews
        );

        let result = work.as_tool_result().expect("something was read");
        assert_eq!(result["read_from"], json!(["Linear"]));
        assert_eq!(
            result["my_recently_completed_work_covers"],
            format!("{} to today", last_week(today).format("%Y-%m-%d")),
            "the answer can say how far back it looked"
        );
        assert!(
            !result.to_string().contains("GitHub"),
            "nothing about a tool the user does not have: {result}"
        );

        let sent = server.await.expect("server").join("");
        assert!(
            sent.contains("isMe"),
            "the open read asks for the key holder's issues"
        );
        assert!(
            sent.contains("completedAt"),
            "and the done read for what they finished"
        );
    }

    #[tokio::test]
    async fn a_linear_key_that_is_refused_is_said_not_read_as_an_empty_day() {
        let (host, _server) = serve(vec![
            ("HTTP/1.1 401 Unauthorized", "{}".to_string()),
            ("HTTP/1.1 401 Unauthorized", "{}".to_string()),
        ]);
        let context = context_with_linear(&host).await;
        connect_linear(&context).await;

        let work = read(
            &context,
            &Scope {
                done_since: last_week(Local::now().date_naive()),
                day: false,
                limit: MOST,
            },
        )
        .await;

        assert!(work.open.is_unread(), "{:?}", work.open);
        assert!(work.done.is_unread(), "{:?}", work.done);
        assert_eq!(work.open.unread[0].0, "Linear");
        assert!(work.as_tool_result().is_none());
    }
}
