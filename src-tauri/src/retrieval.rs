//! Finding what the user has already done, from this machine's own disk.
//!
//! This is the read half of D9: a question about the user's work is answered
//! from `work_logs` through the FTS5 index migration 8 built, rather than by
//! calling the services the work came from. No request leaves the machine, and
//! the answer arrives in the time a query takes.
//!
//! A [`Hit`] carries the `url` because the interface renders a link from it;
//! the injected context must not, because a link is most of what a row costs
//! and the model has no use for it. Leaving it out is what makes a 300-token
//! budget hold an answer about a week rather than a handful of rows.

use sqlx::SqlitePool;

use crate::context::Budget;
use crate::db::Error;

/// The most rows a single search will return.
///
/// Far more than a 300-token context can hold, on purpose: the ceiling belongs
/// to whoever assembles the prompt, so that trimming to fit is one decision in
/// one place rather than a limit smeared across the query too.
const MAX_LIMIT: i64 = 100;

/// One row the index matched.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Hit {
    pub id: i64,
    /// ISO-8601, UTC.
    pub timestamp: String,
    /// Which service it came from, e.g. `github`.
    pub source: String,
    /// What kind of thing it is, e.g. `pr`.
    pub category: String,
    pub title: String,
    pub summary: Option<String>,
    /// The canonical link. **For the interface, never for the prompt.**
    pub url: Option<String>,
}

/// Turn what a person typed into an FTS5 `MATCH` expression.
///
/// Pure, so the two decisions in it can be tested without a database.
///
/// **Terms are joined with `OR`, not `AND`.** FTS5's default is `AND`, and it
/// is the wrong default here: a read question is a recall problem. Asked "what
/// did I ship on the login refactor", a person is not asserting that every one
/// of those words appears in the row they want — and under `AND` a row titled
/// *Refactored OAuth handler* matches `refactor` but not `login`, so the
/// search returns nothing at all. `bm25()` then does the work `AND` was doing
/// badly: a row matching more terms ranks above one matching fewer, instead of
/// being the only row allowed to exist.
///
/// **Every term is quoted.** FTS5 reads bare input as a query language, so
/// `NOT`, `*`, `^`, `:` and an unbalanced `"` are all operators or syntax
/// errors waiting in ordinary prose. Quoting makes each token a string
/// literal, and the tokenizer still stems inside it — which is the whole point
/// of `porter`, and would be lost by escaping the input some other way.
///
/// Returns `None` when nothing usable is left, because `MATCH ''` is an error
/// rather than an empty result.
pub fn to_match_query(question: &str) -> Option<String> {
    let terms: Vec<String> = question
        .split(|c: char| !c.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{}\"", term.to_lowercase()))
        .collect();

    (!terms.is_empty()).then(|| terms.join(" OR "))
}

/// Search the work log, best match first.
///
/// Ordered by `bm25()` — which is negative and ascending, so the strongest
/// match sorts first — and then by recency, so two equally good matches put
/// the newer one in front.
pub async fn search(pool: &SqlitePool, question: &str, limit: i64) -> Result<Vec<Hit>, Error> {
    let Some(query) = to_match_query(question) else {
        return Ok(Vec::new());
    };

    let hits = sqlx::query_as::<_, Hit>(
        "SELECT w.id, w.timestamp, w.source, w.category, w.title, w.summary, w.url
           FROM work_logs_fts AS f
           JOIN work_logs AS w ON w.id = f.rowid
          WHERE work_logs_fts MATCH ?1
          ORDER BY bm25(work_logs_fts), w.timestamp DESC
          LIMIT ?2",
    )
    .bind(query)
    .bind(limit.clamp(1, MAX_LIMIT))
    .fetch_all(pool)
    .await?;

    Ok(hits)
}

/// Read what happened inside a window, in the order it happened.
///
/// "What is on my calendar today" names nothing to match on, so the index has
/// nothing to rank; the answer is a window over time. Ascending, because a day
/// of meetings is read forwards.
///
/// `from` and `to` are compared as strings against `work_logs.timestamp`,
/// which the schema stores as ISO-8601 **UTC**. Callers converting a local day
/// into that window must convert to UTC first — see `intent::day_window`.
pub async fn in_window(
    pool: &SqlitePool,
    from: &str,
    to: &str,
    category: Option<&str>,
    limit: i64,
) -> Result<Vec<Hit>, Error> {
    let hits = sqlx::query_as::<_, Hit>(
        "SELECT id, timestamp, source, category, title, summary, url
           FROM work_logs
          WHERE timestamp >= ?1 AND timestamp < ?2
            AND (?3 IS NULL OR category = ?3)
          ORDER BY timestamp ASC, id ASC
          LIMIT ?4",
    )
    .bind(from)
    .bind(to)
    .bind(category)
    .bind(limit.clamp(1, MAX_LIMIT))
    .fetch_all(pool)
    .await?;

    Ok(hits)
}

/// The newest entries, whatever they are.
///
/// What `/log` answers with: no window and no search terms, just the last few
/// things that happened, newest first.
pub async fn latest(pool: &SqlitePool, limit: i64) -> Result<Vec<Hit>, Error> {
    let hits = sqlx::query_as::<_, Hit>(
        "SELECT id, timestamp, source, category, title, summary, url
           FROM work_logs
          ORDER BY timestamp DESC, id DESC
          LIMIT ?1",
    )
    .bind(limit.clamp(1, MAX_LIMIT))
    .fetch_all(pool)
    .await?;

    Ok(hits)
}

/// One row, as a line for a person or a prompt to read.
///
/// `- [GitHub] Add PKCE auth handler — merged`
///
/// **No URL, ever.** That is the whole reason [`crate::context::RETRIEVAL_CEILING`]
/// of 300 tokens is a useful budget rather than a crippling one: a link is most
/// of what a row costs, for a string the model cannot use and should not be
/// able to repeat back. The interface renders it from `work_logs.url` instead.
///
/// Returns `None` for a row with nothing to say. **An entry the user typed by
/// hand has an empty `title`** — migration 8 gave the column a default rather
/// than inventing one mid-flight — so the summary carries the line instead. A
/// row with neither is dropped, because `- [GitHub]` on its own is worse than
/// one fewer row.
#[must_use]
pub fn describe(hit: &Hit) -> Option<String> {
    let source = title_case(&hit.source);
    let title = hit.title.trim();
    let summary = hit
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|summary| !summary.is_empty());

    match (title.is_empty(), summary) {
        (false, Some(summary)) if summary != title => {
            Some(format!("- [{source}] {title} — {summary}"))
        }
        (false, _) => Some(format!("- [{source}] {title}")),
        (true, Some(summary)) => Some(format!("- [{source}] {summary}")),
        (true, None) => None,
    }
}

/// `github` reads as `GitHub` to a person, and `linear` as `Linear`.
///
/// A lookup rather than capitalising the first letter, because the two that
/// matter are not simply capitalised and getting them wrong in every answer
/// would be a small permanent papercut.
fn title_case(source: &str) -> String {
    match source {
        "github" => "GitHub".to_string(),
        "linear" => "Linear".to_string(),
        "outlook" => "Outlook".to_string(),
        "calendar" => "Calendar".to_string(),
        other => other.to_string(),
    }
}

/// Turn hits into a block that fits the budget, dropping the weakest first.
///
/// Returns `None` when there is nothing to say, which is the caller's signal
/// to step aside rather than to answer "you have nothing" — those two are
/// indistinguishable to a reader, and the plan records the distinction as
/// settled.
///
/// Hits are already ranked, so truncation drops from the end: the rows most
/// likely to be what was asked about are the ones that survive the ceiling.
#[must_use]
pub fn to_context(hits: &[Hit], ceiling: u32) -> Option<String> {
    let mut budget = Budget::with_ceiling(ceiling);
    let mut lines: Vec<String> = Vec::new();

    for hit in hits {
        let Some(line) = describe(hit) else {
            continue;
        };

        // A refused line costs nothing, but stopping at the first refusal
        // keeps the block in rank order rather than letting a short low-ranked
        // row jump ahead of a long high-ranked one it should sit behind.
        if budget.add("row", &line).is_err() {
            break;
        }

        lines.push(line);
    }

    (!lines.is_empty()).then(|| lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::migrated_pool;
    use crate::work_log::{self, WorkLogRecord};

    fn record(external_id: &str, title: &str, summary: &str) -> WorkLogRecord {
        WorkLogRecord {
            timestamp: "2026-09-01T09:00:00.000Z".to_string(),
            source: "github".to_string(),
            category: "pr".to_string(),
            title: title.to_string(),
            content: title.to_string(),
            summary: Some(summary.to_string()),
            url: Some(format!("https://github.com/o/r/pull/{external_id}")),
            raw_ref: None,
            external_id: external_id.to_string(),
            account_id: 1,
        }
    }

    #[tokio::test]
    async fn finds_a_row_whose_words_were_stemmed() {
        let pool = migrated_pool().await;

        work_log::upsert(&pool, record("1", "Refactored OAuth handler", "Landed it"))
            .await
            .expect("should store");

        // `refactoring` and `Refactored` share the stem `refactor`, which is
        // the whole reason the tokenizer is `porter` rather than `unicode61`
        // alone. Without stemming this returns nothing.
        let hits = search(&pool, "refactoring auth", 10).await.expect("search");

        assert_eq!(hits.len(), 1, "porter should stem refactoring to refactor");
        assert_eq!(hits[0].title, "Refactored OAuth handler");
    }

    /// The specification's own stemming criterion, and why it is not the one
    /// above.
    ///
    /// It asked that searching *"login refactor"* find *"Refactored OAuth
    /// handler"*. Under FTS5's default `AND` that returns **nothing** — the
    /// row has no `login` — so the criterion fails against a correct
    /// implementation. It passes here only because [`to_match_query`] joins
    /// with `OR`, which is the decision this test exists to pin down.
    #[tokio::test]
    async fn matches_on_any_term_rather_than_every_term() {
        let pool = migrated_pool().await;

        work_log::upsert(&pool, record("1", "Refactored OAuth handler", "Landed it"))
            .await
            .expect("should store");

        let hits = search(&pool, "login refactor", 10).await.expect("search");

        assert_eq!(
            hits.len(),
            1,
            "OR should find the row on `refactor` alone; AND would find nothing"
        );
    }

    #[tokio::test]
    async fn ranks_the_row_matching_more_terms_first() {
        let pool = migrated_pool().await;

        for (id, title) in [
            ("1", "Fixed the calendar parser"),
            ("2", "Fixed the calendar parser timezone handling"),
        ] {
            work_log::upsert(&pool, record(id, title, "done"))
                .await
                .expect("should store");
        }

        let hits = search(&pool, "calendar timezone", 10)
            .await
            .expect("search");

        assert_eq!(hits.len(), 2, "OR should return both");
        assert_eq!(
            hits[0].title, "Fixed the calendar parser timezone handling",
            "bm25 should put the row matching both terms first — this is the \
             ranking that replaces AND's filtering"
        );
    }

    #[tokio::test]
    async fn punctuation_and_operators_are_matched_literally() {
        let pool = migrated_pool().await;

        work_log::upsert(&pool, record("1", "Shipped the parser", "done"))
            .await
            .expect("should store");

        // Each of these is FTS5 query syntax rather than prose. Unquoted they
        // are an operator, a prefix search, a column filter or a syntax error;
        // quoted they are words, and none of them may raise.
        for question in [
            "NOT shipped",
            "shipped*",
            "title:shipped",
            "\"unbalanced shipped",
            "shipped^parser",
            "(shipped",
        ] {
            let hits = search(&pool, question, 10)
                .await
                .unwrap_or_else(|error| panic!("{question:?} should not raise: {error}"));

            assert_eq!(hits.len(), 1, "{question:?} should find the row");
        }
    }

    #[tokio::test]
    async fn a_question_with_no_words_searches_for_nothing() {
        let pool = migrated_pool().await;

        work_log::upsert(&pool, record("1", "Shipped the parser", "done"))
            .await
            .expect("should store");

        // `MATCH ''` is an error rather than an empty result, so this has to
        // be answered before the query is built.
        assert!(to_match_query("   ?!  ").is_none());
        assert!(search(&pool, "   ?!  ", 10)
            .await
            .expect("search")
            .is_empty());
    }

    #[tokio::test]
    async fn a_window_holds_only_what_falls_inside_it() {
        let pool = migrated_pool().await;

        for (id, stamp) in [
            ("1", "2026-08-30T09:00:00.000Z"),
            ("2", "2026-09-01T09:00:00.000Z"),
            ("3", "2026-09-02T09:00:00.000Z"),
        ] {
            let mut entry = record(id, "Shipped something", "done");
            entry.timestamp = stamp.to_string();
            work_log::upsert(&pool, entry).await.expect("should store");
        }

        let hits = in_window(
            &pool,
            "2026-09-01T00:00:00.000Z",
            "2026-09-02T00:00:00.000Z",
            None,
            10,
        )
        .await
        .expect("window");

        assert_eq!(hits.len(), 1, "the upper bound is exclusive");
        assert_eq!(hits[0].timestamp, "2026-09-01T09:00:00.000Z");
    }

    #[tokio::test]
    async fn a_window_can_ask_for_one_kind_of_thing() {
        let pool = migrated_pool().await;

        for (id, category) in [("1", "pr"), ("2", "calendar")] {
            let mut entry = record(id, "Something happened", "done");
            entry.category = category.to_string();
            work_log::upsert(&pool, entry).await.expect("should store");
        }

        let hits = in_window(
            &pool,
            "2026-09-01T00:00:00.000Z",
            "2026-09-02T00:00:00.000Z",
            Some("calendar"),
            10,
        )
        .await
        .expect("window");

        assert_eq!(hits.len(), 1, "only the calendar row");
        assert_eq!(hits[0].category, "calendar");
    }

    /// The one guard on the whole 300-token budget: a link must never reach
    /// the block that goes into a prompt.
    ///
    /// Proved by putting `hit.url` back into `describe` and watching this
    /// fail:
    ///
    /// ```text
    /// a retrieved row must never carry a link into a prompt:
    ///   "- [GitHub] Add PKCE auth handler (https://github.com/o/r/pull/1) — open"
    /// ```
    #[tokio::test]
    async fn a_described_row_never_carries_a_link() {
        let pool = migrated_pool().await;

        work_log::upsert(&pool, record("1", "Add PKCE auth handler", "open"))
            .await
            .expect("should store");

        let hits = latest(&pool, 10).await.expect("latest");

        assert!(
            !hits.is_empty(),
            "nothing was stored, so this would prove nothing"
        );
        assert!(
            hits.iter().all(|hit| hit.url.is_some()),
            "the rows must carry a url, or the assertion below is vacuous"
        );

        for hit in &hits {
            let line = describe(hit).expect("a row with a title describes");

            assert!(
                !line.contains("http://") && !line.contains("https://"),
                "a retrieved row must never carry a link into a prompt: {line:?}"
            );
        }
    }

    /// The claim the 300-token ceiling actually rests on: **leaving the link
    /// out fits about half as many again.**
    ///
    /// Asserted as a comparison rather than an absolute count, because an
    /// absolute count measures `context::BYTES_PER_TOKEN` as much as it
    /// measures the format. That constant is 3 and uncalibrated (plan §9), and
    /// it is *more* pessimistic than a real tokenizer for this text: the PR
    /// that introduced this ceiling claimed 20 rows from a real-tokenizer
    /// estimate, and the estimator that enforces the gate fits 15. Both halves
    /// of the comparison go through the same estimator, so the ratio holds
    /// whichever way the calibration eventually lands.
    ///
    /// Measured here: **15 rows without the link against 10 with it**, using a
    /// short `github.com/o/r/pull/1`. A real repository path is longer than
    /// that placeholder, so the gap widens in practice — which is why the
    /// assertion is a floor rather than an equality, and why the claim is "half
    /// as many again" rather than the "double" an earlier draft asserted.
    #[tokio::test]
    async fn leaving_the_link_out_is_what_makes_the_ceiling_workable() {
        let pool = migrated_pool().await;

        for id in 1..=30 {
            work_log::upsert(
                &pool,
                record(
                    &id.to_string(),
                    "Add the PKCE authorisation handler",
                    "merged",
                ),
            )
            .await
            .expect("should store");
        }

        let hits = latest(&pool, 30).await.expect("latest");
        assert_eq!(hits.len(), 30, "the fixture has to have thirty rows");
        assert!(
            hits.iter().all(|hit| hit.url.is_some()),
            "every row must carry a url, or the comparison below is vacuous"
        );

        let without = to_context(&hits, crate::context::RETRIEVAL_CEILING)
            .expect("a block")
            .lines()
            .count();

        // The same rows, formatted the way the specification asked for.
        let with: usize = {
            let mut budget =
                crate::context::Budget::with_ceiling(crate::context::RETRIEVAL_CEILING);
            hits.iter()
                .take_while(|hit| {
                    let line = format!(
                        "{} ({})",
                        describe(hit).unwrap_or_default(),
                        hit.url.as_deref().unwrap_or_default()
                    );
                    budget.add("row", &line).is_ok()
                })
                .count()
        };

        // 1.4x, against a measured 1.5x on the shortest link a row could
        // carry. Stated as a floor because a longer path only widens it.
        assert!(
            without * 5 >= with * 7,
            "leaving the link out should fit about half as many rows again, \
             but {without} without against {with} with"
        );
        assert!(
            without >= 12,
            "the ceiling has to hold enough rows to answer a question about a \
             week; {without} is not enough"
        );
    }

    /// Over-budget input is truncated rather than refused, and the ceiling
    /// still holds.
    ///
    /// Proved by returning every line regardless of the budget and watching
    /// this fail:
    ///
    /// ```text
    /// the block must never exceed the ceiling it was given
    /// ```
    #[tokio::test]
    async fn too_many_rows_are_cut_to_fit() {
        let pool = migrated_pool().await;

        for id in 1..=60 {
            work_log::upsert(
                &pool,
                record(
                    &id.to_string(),
                    "Add the PKCE authorisation handler for the desktop client",
                    "merged and released",
                ),
            )
            .await
            .expect("should store");
        }

        let hits = latest(&pool, 60).await.expect("latest");
        let block = to_context(&hits, crate::context::RETRIEVAL_CEILING).expect("a block");

        assert!(
            block.lines().count() < hits.len(),
            "sixty rows cannot fit 300 tokens, so some must have been cut"
        );
        assert!(
            crate::context::estimate_tokens(&block) <= crate::context::RETRIEVAL_CEILING,
            "the block must never exceed the ceiling it was given"
        );
    }

    #[test]
    fn nothing_to_say_is_nothing_rather_than_an_empty_block() {
        assert_eq!(to_context(&[], crate::context::RETRIEVAL_CEILING), None);
    }

    #[test]
    fn builds_an_or_query_of_quoted_terms() {
        assert_eq!(
            to_match_query("Refactoring Auth").expect("two terms"),
            "\"refactoring\" OR \"auth\""
        );
    }
}
