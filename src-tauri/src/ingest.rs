//! Turning what a service returned into rows the work log can hold.
//!
//! **Nothing here asks the model anything.** The daemon used to spend one
//! generation per merged pull request to write a one-sentence achievement, and
//! that was the wrong shape once D9 made the work log the thing reads are
//! answered from: a feed row and a search hit both want a title, a category and
//! a link, all of which the provider already told us. Deriving them costs a
//! `format!` where generating them costs seconds of a machine that decodes one
//! request at a time.
//!
//! It also unblocks the cadence. `engine::IDLE_TIMEOUT` is ten minutes, so a
//! pass that wakes the model every fifteen would mean the engine never releases
//! its couple of gigabytes — plan §3's third lever undone by the second. A pass
//! that never touches the engine can run as often as is useful.
//!
//! The specification's word for this is *minification*, and the part worth
//! naming is what gets thrown away: tracking parameters, and anything that is
//! not the canonical thing a person would open.

use crate::github::PullRequest;
use crate::microsoft::Event;
use crate::work_log::WorkLogRecord;

/// Why a pull request is in the log: the user's own work, or a review somebody
/// has asked them for.
///
/// The two are different questions and used to be one row shape, because the
/// pass only ever read `author:@me is:merged` and had nothing else to
/// distinguish. `category` carries it, so "what did I ship" and "what is
/// waiting on me" can be answered from the same table without either of them
/// having to guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A pull request the user opened.
    Mine,
    /// One somebody asked them to review.
    Review,
}

impl Kind {
    /// What the row is filed under. `intent` and `retrieval` filter on this.
    const fn category(self) -> &'static str {
        match self {
            Self::Mine => "pr",
            Self::Review => "review",
        }
    }
}

/// Query parameters that are somebody's analytics rather than the address of a
/// thing.
///
/// A prefix list rather than an exact one, because the `utm_` family is open
/// ended and new members keep arriving. Everything else is left alone: a query
/// string can be load bearing, and guessing wrong turns a working link into a
/// 404 that nobody traces back to here.
const TRACKING_PREFIXES: [&str; 6] = ["utm_", "ga_", "mc_", "pk_", "_hs", "vero_"];

/// Query parameters that are tracking despite carrying no family prefix.
const TRACKING_NAMES: [&str; 5] = ["gclid", "fbclid", "mkt_tok", "igshid", "ref_src"];

/// Strip the tracking out of a link, keeping the thing it points at.
///
/// Deliberately string surgery rather than a URL parser: the input is a link a
/// provider gave us, not something a person typed, and pulling in a parser to
/// drop query parameters would be a dependency for a `split`. A link that does
/// not look like one is returned untouched, because the alternative — dropping
/// it — loses the only route back to the item.
#[must_use]
pub fn canonical(url: &str) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return url.to_string();
    };

    // The fragment rides on the last parameter, so it is separated first and
    // put back afterwards rather than being swept up as part of a value.
    let (query, fragment) = match query.split_once('#') {
        Some((query, fragment)) => (query, Some(fragment)),
        None => (query, None),
    };

    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter(|pair| {
            let name = pair.split('=').next().unwrap_or_default().to_lowercase();

            !TRACKING_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
                && !TRACKING_NAMES.iter().any(|tracker| name == *tracker)
        })
        .collect();

    let mut canonical = base.to_string();

    if !kept.is_empty() {
        canonical.push('?');
        canonical.push_str(&kept.join("&"));
    }

    if let Some(fragment) = fragment {
        canonical.push('#');
        canonical.push_str(fragment);
    }

    canonical
}

/// A merged, open or review-requested pull request, as a row.
///
/// `summary` is a template rather than prose. It says what state the thing is
/// in, which is the question a feed row answers and the thing that changes
/// between passes — and it is why ingestion writes through `upsert` rather than
/// `insert_new`: a pull request that was open when it was first seen and is
/// merged by the next pass has to change, or the feed keeps asserting something
/// that stopped being true.
#[must_use]
pub fn from_pull_request(pull_request: &PullRequest, account_id: i64, kind: Kind) -> WorkLogRecord {
    let state = match kind {
        Kind::Mine => describe_state(pull_request),
        // What the user has to *do*, which for a review request is the whole
        // point of the row and is not a state GitHub reports on the item.
        Kind::Review => "review requested".to_string(),
    };

    WorkLogRecord {
        timestamp: pull_request
            .merged_at
            .clone()
            .unwrap_or_else(|| pull_request.updated_at.clone()),
        source: "github".to_string(),
        category: kind.category().to_string(),
        title: format!(
            "{} #{}: {}",
            pull_request.repository, pull_request.number, pull_request.title
        ),
        content: match kind {
            Kind::Mine => format!(
                "{} pull request #{} in {}: {}",
                capitalise(&state),
                pull_request.number,
                pull_request.repository,
                pull_request.title
            ),
            Kind::Review => format!(
                "Review requested on pull request #{} in {}: {}",
                pull_request.number, pull_request.repository, pull_request.title
            ),
        },
        summary: Some(state),
        url: Some(canonical(&pull_request.url)),
        raw_ref: None,
        external_id: pull_request.external_id(),
        account_id,
    }
}

/// One meeting, as a row.
///
/// **The rows `intent::prep` has always queried for and nothing ever wrote.**
/// D9 says a read is answered from local storage, and `prep` duly filters
/// `category = 'calendar'` — but the only production writer of `work_logs` was
/// `from_pull_request`, so the query never matched anything, `answer` returned
/// `None` every time, and "what is on my calendar" fell through to the tool
/// loop and Microsoft Graph exactly as it had before D9 was built.
///
/// `None` when the start cannot be read. A meeting with no placeable time
/// cannot be answered from a window query, and a row dated now would put it in
/// the middle of today's agenda claiming to be a meeting that is not.
#[must_use]
pub fn from_event<Tz: chrono::TimeZone>(
    event: &Event,
    account_id: i64,
    zone: &Tz,
) -> Option<WorkLogRecord> {
    let timestamp = as_utc(&event.start, zone)?;
    let when = clock_time(&event.start);
    let who = if event.attendees.is_empty() {
        String::new()
    } else {
        format!(" with {}", event.attendees.join(", "))
    };

    Some(WorkLogRecord {
        timestamp,
        source: "calendar".to_string(),
        category: "calendar".to_string(),
        title: event.subject.clone(),
        content: format!("{when} {}{who}", event.subject).trim().to_string(),
        summary: Some(format!("{when}{who}").trim().to_string()),
        // A subscription event has no address to open, and `Event` does not
        // carry Graph's `webLink`. Better nothing than a link that goes wrong.
        url: None,
        raw_ref: None,
        // `Event` carries no identifier of its own, so the row is keyed on the
        // two fields that make a meeting the meeting it is. Enough for a pass
        // to recognise the one it wrote last time, which is all `upsert` needs.
        external_id: format!("{}|{}", event.start, event.subject),
        account_id,
    })
}

/// A local wall-clock stamp as the UTC instant the work log stores.
///
/// Both calendar paths produce `%Y-%m-%dT%H:%M:%S` **in the user's own zone** —
/// `calendar.rs` formats an expanded occurrence that way, and Graph is asked
/// for its own offset — while `work_logs.timestamp` is UTC, because that is
/// what SQLite's `strftime(…, 'now')` writes. Storing one as the other reads
/// the wrong day by whatever the offset is: invisible in London in winter and
/// wrong at every day's edge everywhere else.
///
/// Generic over the zone so the conversion is tested at a fixed offset rather
/// than against whatever clock the test machine keeps — the same reason
/// `clock::describe` and `intent::day_window` are.
#[must_use]
pub fn as_utc<Tz: chrono::TimeZone>(local: &str, zone: &Tz) -> Option<String> {
    // Some providers append a fractional part; none of them append an offset,
    // which is the whole reason this function exists.
    let stem = local
        .split('.')
        .next()
        .unwrap_or(local)
        .trim_end_matches('Z');
    let naive = chrono::NaiveDateTime::parse_from_str(stem, "%Y-%m-%dT%H:%M:%S").ok()?;

    Some(
        zone.from_local_datetime(&naive)
            .earliest()
            // A local time that does not exist — the hour a clock skips
            // forward — is read as UTC rather than dropped: being an hour out
            // once a year beats losing the meeting.
            .map_or_else(|| naive.and_utc(), |at| at.to_utc())
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string(),
    )
}

/// `HH:MM` out of a wall-clock stamp, or nothing if it is not one.
fn clock_time(start: &str) -> &str {
    start
        .split('T')
        .nth(1)
        .unwrap_or("")
        .get(0..5)
        .unwrap_or("")
}

/// What state a pull request is in, in one word a person would use.
///
/// `merged_at` is checked before `state`, because GitHub reports a merged pull
/// request as `closed` and "closed" reads to a person as *abandoned*.
fn describe_state(pull_request: &PullRequest) -> String {
    if pull_request.merged_at.is_some() {
        return "merged".to_string();
    }

    if pull_request.draft {
        return "draft".to_string();
    }

    pull_request.state.to_lowercase()
}

/// First letter up, for a sentence that starts with it.
fn capitalise(word: &str) -> String {
    let mut characters = word.chars();

    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + characters.as_str()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull_request() -> PullRequest {
        PullRequest {
            number: 42,
            title: "Add the PKCE auth handler".to_string(),
            repository: "scottmallinson/chief.ai".to_string(),
            state: "open".to_string(),
            draft: false,
            url: "https://github.com/scottmallinson/chief.ai/pull/42".to_string(),
            updated_at: "2026-09-01T09:00:00Z".to_string(),
            merged_at: None,
            body: None,
        }
    }

    #[test]
    fn a_merged_pull_request_reads_as_merged_rather_than_closed() {
        let mut merged = pull_request();
        merged.state = "closed".to_string();
        merged.merged_at = Some("2026-09-02T09:00:00Z".to_string());

        let record = from_pull_request(&merged, 1, Kind::Mine);

        assert_eq!(record.summary.as_deref(), Some("merged"));
        assert_eq!(
            record.timestamp, "2026-09-02T09:00:00Z",
            "a merged pull request is dated by the merge, not the last touch"
        );
    }

    #[test]
    fn a_draft_says_so() {
        let mut draft = pull_request();
        draft.draft = true;

        assert_eq!(
            from_pull_request(&draft, 1, Kind::Mine).summary.as_deref(),
            Some("draft")
        );
    }

    #[test]
    fn the_title_carries_the_repository_and_number() {
        let record = from_pull_request(&pull_request(), 1, Kind::Mine);

        assert_eq!(
            record.title,
            "scottmallinson/chief.ai #42: Add the PKCE auth handler"
        );
    }

    #[test]
    fn tracking_parameters_are_stripped_and_the_link_still_resolves() {
        let stripped = canonical(
            "https://github.com/o/r/pull/42?utm_source=email&utm_medium=digest&gclid=abc",
        );

        assert_eq!(stripped, "https://github.com/o/r/pull/42");
    }

    #[test]
    fn a_parameter_that_is_not_tracking_survives() {
        // `diff=split` changes what the page shows, so dropping it would
        // silently change where the link goes.
        let kept = canonical("https://github.com/o/r/pull/42?diff=split&utm_source=email");

        assert_eq!(kept, "https://github.com/o/r/pull/42?diff=split");
    }

    #[test]
    fn a_fragment_is_kept_and_stays_at_the_end() {
        let kept = canonical("https://github.com/o/r/pull/42?utm_source=x#discussion_r1");

        assert_eq!(kept, "https://github.com/o/r/pull/42#discussion_r1");
    }

    #[test]
    fn a_link_with_nothing_to_strip_is_returned_untouched() {
        let plain = "https://github.com/o/r/pull/42";

        assert_eq!(canonical(plain), plain);
    }

    /// The guard on the whole point of this module.
    ///
    /// Proved by putting a `summarise` call back into `from_pull_request` —
    /// which cannot even be done without making it `async` and giving it a
    /// client, and that is the point: the signature is the guard. This test
    /// records the intent so the next person reads it before changing the
    /// shape.
    #[test]
    fn building_a_row_needs_no_engine_and_no_await() {
        // A compile-time assertion: `from_pull_request` takes no client and
        // returns no future, so a model call cannot be added without changing
        // every caller.
        let build: fn(&PullRequest, i64, Kind) -> WorkLogRecord = from_pull_request;
        let record = build(&pull_request(), 1, Kind::Mine);

        assert_eq!(record.source, "github");
        assert!(
            record.summary.is_some(),
            "a row must carry its state without asking anyone"
        );
    }
}
