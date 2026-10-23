//! A live look at the user's work, for the questions that are about all of it.
//!
//! "What should I prioritise?", "what did I complete yesterday?" and "draft my
//! standup" are not about one tool. Left to the tool loop, a small model asked
//! for context it could have looked up, or said it had no access to a log it
//! did. This reads the four things those questions are made of — the user's open
//! pull requests, the reviews requested of them, what they merged lately and
//! today's meetings — as they are now, and hands them over as short lines.
//!
//! **Live, not logged.** The work log only ever grows: a pull request that has
//! since merged still sits in it as "open", and a review done at nine still reads
//! as waiting at noon. That is the stale "Still open" and "Waiting on review" in
//! a standup, and it is why `intent::waiting` already asks GitHub. A source that
//! cannot be read is named as such rather than reported as empty, because "you
//! have nothing open" and "GitHub could not be reached" are opposite statements.
//!
//! **Lines, not JSON.** A pull request as the tools return it carries a URL, an
//! update time and an account on every entry; none of that is wanted to answer
//! these questions and all of it is prompt. One line each, capped at [`MOST`].

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Weekday};
use serde_json::{json, Value};

use crate::tools::{self, Context, PullRequestState, Range, Whose};

/// The most lines read from any one source. A question about "my work" is
/// answered in a handful of items; the rest would be prefill and a longer wait
/// for an answer nobody reads to the end.
pub const MOST: usize = 8;

/// One source, read or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// What was found. Empty means the source answered and had nothing.
    Read(Vec<String>),
    /// Why it could not be read.
    Unread(String),
}

/// The user's work right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Work {
    /// Their own pull requests that are open or in draft.
    pub open: Part,
    /// Pull requests other people are waiting on them to review.
    pub reviews: Part,
    /// Their pull requests merged since the cut-off, each saying how long ago.
    pub merged: Part,
    /// Today's meetings, in the order they happen.
    pub meetings: Part,
}

/// Read everything, one source after another. Meetings only when asked for.
///
/// Sequential rather than joined: four reads are a few hundred milliseconds
/// against a model that takes tens of seconds, and a fixed order is what lets a
/// stub server be scripted.
pub async fn read(context: &Context, merged_since: NaiveDate, with_meetings: bool) -> Work {
    let today = Local::now().date_naive();

    Work {
        open: pull_requests(context, PullRequestState::Open, Whose::Mine, |pr| {
            Some(describe(
                pr,
                if pr["draft"] == true { ", draft" } else { "" },
            ))
        })
        .await,
        reviews: pull_requests(context, PullRequestState::Open, Whose::Reviewing, |pr| {
            Some(describe(pr, ""))
        })
        .await,
        merged: pull_requests(context, PullRequestState::Merged, Whose::Mine, |pr| {
            let merged = merged_on(pr["merged_at"].as_str()?)?;

            (merged >= merged_since)
                .then(|| describe(pr, &format!(", merged {}", when(merged, today))))
        })
        .await,
        // A standup is not about the calendar, and a Graph call nobody reads is
        // a wait for nothing.
        meetings: if with_meetings {
            meetings(context).await
        } else {
            Part::Read(Vec::new())
        },
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
pub fn last_week(today: NaiveDate) -> NaiveDate {
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

/// `title (repo#number, note)`.
fn describe(pr: &Value, note: &str) -> String {
    format!(
        "{} ({}#{}{note})",
        pr["title"].as_str().unwrap_or("Untitled"),
        pr["repository"].as_str().unwrap_or("unknown"),
        pr["number"],
    )
}

/// One GitHub read, turned into lines by `line`, which may drop an entry.
async fn pull_requests(
    context: &Context,
    state: PullRequestState,
    whose: Whose,
    line: impl Fn(&Value) -> Option<String>,
) -> Part {
    match tools::fetch_github_prs(&context.pool, &context.github, state, whose).await {
        Ok(read) => Part::Read(
            read["pull_requests"]
                .as_array()
                .map(|entries| entries.iter().filter_map(&line).take(MOST).collect())
                .unwrap_or_default(),
        ),
        Err(error) => {
            eprintln!("could not read pull requests for the work overview: {error}");
            Part::Unread(error.to_string())
        }
    }
}

/// Today's meetings as `HH:MM subject`.
async fn meetings(context: &Context) -> Part {
    match tools::fetch_calendar(context, Range::Today).await {
        Ok(read) => Part::Read(
            read["events"]
                .as_array()
                .map(|events| events.iter().map(meeting).take(MOST).collect())
                .unwrap_or_default(),
        ),
        Err(error) => Part::Unread(error.to_string()),
    }
}

fn meeting(event: &Value) -> String {
    let subject = event["subject"].as_str().unwrap_or("Untitled");

    if event["all_day"] == true {
        return format!("all day: {subject}");
    }

    // Graph answers in the zone it was asked for, as `2026-08-28T10:00:00`.
    match event["start"].as_str().and_then(|start| start.get(11..16)) {
        Some(time) => format!("{time} {subject}"),
        None => subject.to_string(),
    }
}

impl Work {
    /// Everything, as a tool result.
    ///
    /// A source that could not be read says so under `not_read`, never as an
    /// empty list. Nothing at all readable is the caller's error to raise, so
    /// the model is not handed a result made only of excuses.
    pub fn as_tool_result(&self) -> Option<Value> {
        let parts = [
            ("my_open_pull_requests", &self.open),
            ("review_requests_waiting_on_me", &self.reviews),
            ("my_recently_merged_pull_requests", &self.merged),
            ("my_meetings_today", &self.meetings),
        ];

        let mut result = serde_json::Map::new();
        let mut not_read = Vec::new();

        for (name, part) in parts {
            match part {
                Part::Read(lines) => {
                    result.insert(name.to_string(), json!(lines));
                }
                Part::Unread(why) => not_read.push(json!({ "source": name, "why": why })),
            }
        }

        if result.is_empty() {
            return None;
        }

        if !not_read.is_empty() {
            result.insert("not_read".to_string(), Value::Array(not_read));
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
    on first, then your own open pull requests, mentioning a meeting that limits the time. For \
    what was done, use the merged list and its dates. A list that is empty means there is \
    nothing; one under not_read could not be read, so say that and do not guess.";

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_meeting_is_its_time_and_its_subject() {
        let event = json!({ "subject": "1:1 with Sam", "start": "2026-10-23T10:00:00.0000000" });

        assert_eq!(meeting(&event), "10:00 1:1 with Sam");
        assert_eq!(
            meeting(
                &json!({ "subject": "Bank holiday", "start": "2026-10-23T00:00:00", "all_day": true })
            ),
            "all day: Bank holiday"
        );
    }

    #[test]
    fn a_source_that_could_not_be_read_is_not_reported_as_empty() {
        let work = Work {
            open: Part::Read(vec![]),
            reviews: Part::Unread("GitHub could not be reached".to_string()),
            merged: Part::Read(vec!["Fix it (o/r#1, merged yesterday)".to_string()]),
            meetings: Part::Unread("No calendar is connected".to_string()),
        };

        let result = work.as_tool_result().expect("something was read");

        assert_eq!(result["my_open_pull_requests"], json!([]));
        assert!(result.get("review_requests_waiting_on_me").is_none());
        assert_eq!(result["not_read"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn nothing_readable_at_all_is_not_a_result() {
        let unread = || Part::Unread("not connected".to_string());
        let work = Work {
            open: unread(),
            reviews: unread(),
            merged: unread(),
            meetings: unread(),
        };

        assert!(work.as_tool_result().is_none());
    }
}
