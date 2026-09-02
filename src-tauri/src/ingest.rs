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
use crate::work_log::WorkLogRecord;

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

/// A merged or open pull request, as a row.
///
/// `summary` is a template rather than prose. It says what state the thing is
/// in, which is the question a feed row answers and the thing that changes
/// between passes — and it is why ingestion writes through `upsert` rather than
/// `insert_new`: a pull request that was open when it was first seen and is
/// merged by the next pass has to change, or the feed keeps asserting something
/// that stopped being true.
#[must_use]
pub fn from_pull_request(pull_request: &PullRequest, account_id: i64) -> WorkLogRecord {
    let state = describe_state(pull_request);

    WorkLogRecord {
        timestamp: pull_request
            .merged_at
            .clone()
            .unwrap_or_else(|| pull_request.updated_at.clone()),
        source: "github".to_string(),
        category: "pr".to_string(),
        title: format!(
            "{} #{}: {}",
            pull_request.repository, pull_request.number, pull_request.title
        ),
        content: format!(
            "{} pull request #{} in {}: {}",
            capitalise(&state),
            pull_request.number,
            pull_request.repository,
            pull_request.title
        ),
        summary: Some(state),
        url: Some(canonical(&pull_request.url)),
        raw_ref: None,
        external_id: pull_request.external_id(),
        account_id,
    }
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

        let record = from_pull_request(&merged, 1);

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
            from_pull_request(&draft, 1).summary.as_deref(),
            Some("draft")
        );
    }

    #[test]
    fn the_title_carries_the_repository_and_number() {
        let record = from_pull_request(&pull_request(), 1);

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
        let build: fn(&PullRequest, i64) -> WorkLogRecord = from_pull_request;
        let record = build(&pull_request(), 1);

        assert_eq!(record.source, "github");
        assert!(
            record.summary.is_some(),
            "a row must carry its state without asking anyone"
        );
    }
}
