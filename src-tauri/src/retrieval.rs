//! Finding what the user has already done, from this machine's own disk.
//!
//! This is the read half of D9: a question about the user's work is answered
//! from `work_logs` through the FTS5 index migration 8 built, rather than by
//! calling the services the work came from. No request leaves the machine, and
//! the answer arrives in the time a query takes.
//!
//! What this module deliberately does **not** do is format anything for a
//! prompt. A [`Hit`] carries the `url` because the interface renders a link
//! from it; the injected context must not, because a GitHub link costs 13–15
//! tokens of the ~15 a whole row is worth and the model has no use for it.
//! Stripping it is what turns a 300-token budget from eight rows into twenty.

use sqlx::SqlitePool;

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

/// Read the most recent entries, for a question with no search terms in it.
///
/// "What did I do today" names nothing to match on, so the index has nothing
/// to rank; the answer is a window over time instead. Kept here beside
/// [`search`] because both answer the same question for the caller — what does
/// this machine already know — and neither touches the network.
pub async fn recent(pool: &SqlitePool, since: &str, limit: i64) -> Result<Vec<Hit>, Error> {
    let hits = sqlx::query_as::<_, Hit>(
        "SELECT id, timestamp, source, category, title, summary, url
           FROM work_logs
          WHERE timestamp >= ?1
          ORDER BY timestamp DESC, id DESC
          LIMIT ?2",
    )
    .bind(since)
    .bind(limit.clamp(1, MAX_LIMIT))
    .fetch_all(pool)
    .await?;

    Ok(hits)
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
    async fn recent_reads_a_window_rather_than_the_index() {
        let pool = migrated_pool().await;

        for (id, stamp) in [
            ("1", "2026-08-30T09:00:00.000Z"),
            ("2", "2026-09-01T09:00:00.000Z"),
        ] {
            let mut entry = record(id, "Shipped something", "done");
            entry.timestamp = stamp.to_string();
            work_log::upsert(&pool, entry).await.expect("should store");
        }

        let hits = recent(&pool, "2026-08-31T00:00:00.000Z", 10)
            .await
            .expect("recent");

        assert_eq!(hits.len(), 1, "only the entry after the cutoff");
        assert_eq!(hits[0].timestamp, "2026-09-01T09:00:00.000Z");
    }

    #[test]
    fn builds_an_or_query_of_quoted_terms() {
        assert_eq!(
            to_match_query("Refactoring Auth").expect("two terms"),
            "\"refactoring\" OR \"auth\""
        );
    }
}
