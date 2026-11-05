//! What Chief knows about the user and the people around them, as rows.
//!
//! A fact says one thing, where it came from and when it was last true. Rows
//! replace the two free-text profile files for drafting: a line of prose has no
//! source, no date and no count, so the only thing that could read it was string
//! matching, and nothing could say when it had gone out of date.
//!
//! Three rules decide who owns a row, and they are the whole editing story:
//!
//! - **`counted`** rows are Chief's. A recount replaces them.
//! - **`yours`** rows were typed or edited by the user. No recount touches them,
//!   so a correction stays corrected.
//! - **`hidden`** is a counted row the user deleted. It is kept, invisible, so
//!   the next recount does not bring back what was just removed.
//!
//! Everything that picks rows for a prompt lives here, in SQL, so a draft pays
//! for the handful of rows about its own repository and not for everything
//! Chief has ever counted.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::db::Error;

pub const COUNTED: &str = "counted";
pub const YOURS: &str = "yours";

/// Who a fact is about when it is about the user.
pub const YOU: &str = "you";

/// Kinds of fact about the user's own writing.
pub const GREETING: &str = "greeting";
pub const SIGN_OFF: &str = "sign-off";
pub const LENGTH: &str = "length";
pub const EMOJI: &str = "emoji";
pub const SPELLING: &str = "spelling";

/// Kinds of fact about other people.
///
/// `reviews` is scoped to a repository: how many of the user's pull requests
/// there this person reviewed. `name` is what to call the login in a message.
/// `meets` is a count of shared meetings and has no scope.
pub const REVIEWS: &str = "reviews";
pub const NAME: &str = "name";
pub const MEETS: &str = "meets";

const STYLE: [&str; 5] = [GREETING, SIGN_OFF, LENGTH, EMOJI, SPELLING];

/// Past this many days a counted fact is flagged as possibly out of date.
pub const STALE_DAYS: i64 = 90;

/// One row, as stored and as shown in Settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Fact {
    pub id: i64,
    pub about: String,
    pub what: String,
    pub scope: String,
    pub value: String,
    pub count: i64,
    pub total: i64,
    pub source: String,
    pub seen_on: String,
    pub origin: String,
}

/// A fact before it has a row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewFact {
    pub about: String,
    pub what: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub count: i64,
    #[serde(default)]
    pub total: i64,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub seen_on: String,
}

impl NewFact {
    /// Keys are compared lower-case, so `Acme/API` and `acme/api` are one.
    fn normalised(mut self) -> Self {
        self.about = self.about.trim().trim_start_matches('@').to_lowercase();
        self.what = self.what.trim().to_lowercase();
        self.scope = self.scope.trim().to_lowercase();
        self.value = self.value.trim().to_string();
        self
    }
}

const COLUMNS: &str = "id, about, what, scope, value, count, total, source, seen_on, origin";

/// Every fact the user can see, newest first within a person.
pub async fn list(pool: &SqlitePool) -> Result<Vec<Fact>, Error> {
    Ok(sqlx::query_as::<_, Fact>(&format!(
        "SELECT {COLUMNS} FROM facts WHERE origin != 'hidden' \
         ORDER BY about = 'you' DESC, about, what, count DESC, scope"
    ))
    .fetch_all(pool)
    .await?)
}

/// What a recount did, said back to the user.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Recounted {
    pub written: usize,
    /// Rows the recount left alone because the user edited or removed them.
    pub kept: usize,
}

/// Replace Chief's own rows of these kinds with `rows`.
///
/// One transaction: a recount that fails half-way leaves the old rows, not a
/// gap. Rows the recount no longer finds are dropped, which is how a person who
/// stopped reviewing ages out. A key the user owns, or removed, is skipped.
pub async fn recount(
    pool: &SqlitePool,
    kinds: &[&str],
    rows: Vec<NewFact>,
) -> Result<Recounted, Error> {
    let mut tx = pool.begin().await?;

    for kind in kinds {
        sqlx::query("DELETE FROM facts WHERE what = ? AND origin = 'counted'")
            .bind(kind)
            .execute(&mut *tx)
            .await?;
    }

    let mut done = Recounted::default();

    for row in rows.into_iter().map(NewFact::normalised) {
        let inserted = sqlx::query(
            "INSERT INTO facts (about, what, scope, value, count, total, source, seen_on, origin) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'counted') \
             ON CONFLICT (about, what, scope) DO NOTHING",
        )
        .bind(&row.about)
        .bind(&row.what)
        .bind(&row.scope)
        .bind(&row.value)
        .bind(row.count)
        .bind(row.total)
        .bind(&row.source)
        .bind(&row.seen_on)
        .execute(&mut *tx)
        .await?;

        if inserted.rows_affected() == 1 {
            done.written += 1;
        } else {
            done.kept += 1;
        }
    }

    tx.commit().await?;

    Ok(done)
}

/// Add a fact, or correct one. The row becomes the user's.
///
/// With an `id`, that row is replaced, so changing who or where a fact is about
/// moves it instead of leaving the old one behind.
pub async fn save(
    pool: &SqlitePool,
    id: Option<i64>,
    fact: NewFact,
    today: &str,
) -> Result<(), Error> {
    let fact = fact.normalised();
    let mut tx = pool.begin().await?;

    if let Some(id) = id {
        sqlx::query("DELETE FROM facts WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }

    sqlx::query(
        "INSERT INTO facts (about, what, scope, value, count, total, source, seen_on, origin) \
         VALUES (?, ?, ?, ?, ?, ?, 'you', ?, 'yours') \
         ON CONFLICT (about, what, scope) DO UPDATE SET \
         value = excluded.value, count = excluded.count, total = excluded.total, \
         source = 'you', seen_on = excluded.seen_on, origin = 'yours'",
    )
    .bind(&fact.about)
    .bind(&fact.what)
    .bind(&fact.scope)
    .bind(&fact.value)
    .bind(fact.count)
    .bind(fact.total)
    .bind(today)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(())
}

/// Delete a fact. Chief's own row is hidden rather than dropped, so a recount
/// does not put it back; the user's own row is simply gone.
pub async fn remove(pool: &SqlitePool, id: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM facts WHERE id = ? AND origin = 'yours'")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE facts SET origin = 'hidden' WHERE id = ? AND origin = 'counted'")
        .bind(id)
        .execute(pool)
        .await?;

    Ok(())
}

/// The rows that describe how the user writes.
pub async fn style(pool: &SqlitePool) -> Result<Vec<Fact>, Error> {
    let mut rows = sqlx::query_as::<_, Fact>(&format!(
        "SELECT {COLUMNS} FROM facts WHERE about = 'you' AND origin != 'hidden' \
         AND what IN ('greeting', 'sign-off', 'length', 'emoji', 'spelling')"
    ))
    .fetch_all(pool)
    .await?;

    rows.sort_by_key(|row| STYLE.iter().position(|kind| *kind == row.what));

    Ok(rows)
}

/// Somebody the facts tie to a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// Their handle, which is what the facts are keyed on.
    pub login: String,
    /// What to call them, when the user has said.
    pub name: Option<String>,
    pub count: i64,
    pub total: i64,
    pub source: String,
    pub seen_on: String,
    pub origin: String,
}

/// The people who review the user's pull requests in `repository`, most
/// frequent first. Matched on the whole `owner/name`, so `acme/api` never pulls
/// in `acme/api-gateway`.
pub async fn people_for(
    pool: &SqlitePool,
    repository: &str,
    limit: i64,
) -> Result<Vec<Person>, Error> {
    let rows = sqlx::query_as::<_, Fact>(&format!(
        "SELECT {COLUMNS} FROM facts WHERE what = '{REVIEWS}' AND scope = ? \
         AND origin != 'hidden' ORDER BY count DESC, about LIMIT ?"
    ))
    .bind(repository.to_lowercase())
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut people = Vec::new();

    for row in rows {
        let name: Option<String> = sqlx::query_scalar(
            "SELECT value FROM facts WHERE about = ? AND what = ? AND scope = '' \
             AND origin != 'hidden'",
        )
        .bind(&row.about)
        .bind(NAME)
        .fetch_optional(pool)
        .await?
        .filter(|name: &String| !name.trim().is_empty());

        people.push(Person {
            login: row.about,
            name,
            count: row.count,
            total: row.total,
            source: row.source,
            seen_on: row.seen_on,
            origin: row.origin,
        });
    }

    Ok(people)
}

/// What the facts say about one reviewer across every repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reviewer {
    /// Reviews of the user's recent pull requests, summed over repositories.
    pub count: i64,
    /// How many pull requests were counted. Every repository's row carries the
    /// same window, so this is the largest, not a sum.
    pub total: i64,
    /// The oldest count behind the figures, so a stale one is said to be.
    pub seen_on: String,
    /// True when every row was typed by the user and none was counted.
    pub told: bool,
}

/// How often `login` reviews the user's pull requests, or `None` when the facts
/// hold nothing about them. A person who is not in the facts is not described
/// as someone who never reviews.
pub async fn reviewer(pool: &SqlitePool, login: &str) -> Result<Option<Reviewer>, Error> {
    let rows = sqlx::query_as::<_, Fact>(&format!(
        "SELECT {COLUMNS} FROM facts WHERE what = '{REVIEWS}' AND about = ? \
         AND origin != 'hidden'"
    ))
    .bind(login.to_lowercase())
    .fetch_all(pool)
    .await?;

    let Some(oldest) = rows.iter().map(|row| row.seen_on.clone()).min() else {
        return Ok(None);
    };

    Ok(Some(Reviewer {
        count: rows.iter().map(|row| row.count).sum(),
        total: rows.iter().map(|row| row.total).max().unwrap_or(0),
        seen_on: oldest,
        told: rows.iter().all(|row| row.origin == YOURS && row.total == 0),
    }))
}

/// How a meeting's attendee reads under it, with a warning when the count is old.
pub fn reviewer_line(
    name: &str,
    login: &str,
    reviewer: &Reviewer,
    today: chrono::NaiveDate,
) -> String {
    let mut line = if reviewer.told {
        format!("  {name} (@{login}) reviews your pull requests. You told Chief this.")
    } else {
        format!(
            "  {name} (@{login}) reviewed {} of your last {} pull requests (counted {}).",
            reviewer.count, reviewer.total, reviewer.seen_on
        )
    };

    if let Some(days) =
        age_in_days(&reviewer.seen_on, today).filter(|days| *days > STALE_DAYS && !reviewer.told)
    {
        line.push_str(&format!(
            " That was {days} days ago and may be out of date."
        ));
    }

    line
}

/// Is there anything to write a draft from?
///
/// Style or a reviewer; a meeting count on its own does not make a draft sound
/// like anybody.
pub async fn has_profile(pool: &SqlitePool) -> Result<bool, Error> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM facts WHERE origin != 'hidden' AND what != 'meets' \
         AND what != 'name')",
    )
    .fetch_one(pool)
    .await?)
}

/// How a style fact reads in a prompt. Counts are said, never an impression;
/// a row the user wrote has no sample behind it and says none.
pub fn style_line(fact: &Fact) -> String {
    style_text(&fact.what, &fact.value, fact.count, fact.total)
}

/// [`style_line`] for the fields alone, so a count can be read before it is a row.
pub fn style_text(what: &str, value: &str, count: i64, total: i64) -> String {
    let of = if total > 0 {
        format!(" ({count} of {total})")
    } else {
        String::new()
    };
    let value = value.trim();

    match what {
        GREETING if value.is_empty() => format!("Greeting: none{of}."),
        GREETING => format!("Greeting: opens with \"{value}\"{of}."),
        SIGN_OFF if value.is_empty() => format!("Sign-off: none{of}."),
        SIGN_OFF => format!("Sign-off: ends with \"{value}\"{of}."),
        LENGTH if total > 0 => format!("Length: about {value} words (median of {total})."),
        LENGTH => format!("Length: about {value} words."),
        EMOJI if count == 0 && total > 0 => format!("Emoji: none{of}."),
        EMOJI if total > 0 => format!("Emoji: used in {count} of {total}."),
        EMOJI if value.is_empty() => "Emoji: used.".to_string(),
        EMOJI => format!("Emoji: {value}."),
        SPELLING => format!("Spelling: {value}."),
        other => format!("{other}: {value}."),
    }
}

/// How a reviewer reads in a prompt, with a warning when the count is old.
pub fn person_line(person: &Person, repository: &str, today: chrono::NaiveDate) -> String {
    let who = match &person.name {
        Some(name) => format!("{name} (@{})", person.login),
        None => format!("@{}", person.login),
    };

    let mut line = if person.origin == YOURS && person.total == 0 {
        format!("- {who} reviews pull requests in {repository}. You told Chief this.")
    } else {
        format!(
            "- {who} reviewed {} of your last {} pull requests in {repository}. Source: {}, counted {}.",
            person.count, person.total, person.source, person.seen_on
        )
    };

    if let Some(days) = age_in_days(&person.seen_on, today).filter(|days| *days > STALE_DAYS) {
        if person.origin != YOURS {
            line.push_str(&format!(
                " That was {days} days ago and may be out of date."
            ));
        }
    }

    line
}

/// The one line above a block of style facts saying where they came from, with
/// a warning when the oldest counted one is stale.
pub fn style_source(rows: &[Fact], today: chrono::NaiveDate) -> Option<String> {
    let counted = rows.iter().filter(|row| row.origin == COUNTED);
    let oldest = counted.clone().map(|row| row.seen_on.as_str()).min()?;
    let source = counted.map(|row| row.source.as_str()).next()?;

    let mut line = format!("Source: {source}, counted {oldest}.");

    if let Some(days) = age_in_days(oldest, today).filter(|days| *days > STALE_DAYS) {
        line.push_str(&format!(
            " That was {days} days ago and may be out of date."
        ));
    }

    Some(line)
}

fn age_in_days(seen_on: &str, today: chrono::NaiveDate) -> Option<i64> {
    let seen = chrono::NaiveDate::parse_from_str(seen_on, "%Y-%m-%d").ok()?;
    Some((today - seen).num_days())
}

// ── Commands ──────────────────────────────────────────────────────────────

/// Every fact, for the Settings card.
#[tauri::command]
pub async fn list_facts<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<Vec<Fact>, Error> {
    list(&crate::db::pool(&app).await?).await
}

/// Add a fact or correct one. Only ever called because the user pressed save.
#[tauri::command]
pub async fn save_fact<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    id: Option<i64>,
    fact: NewFact,
) -> Result<(), Error> {
    let today = chrono::Local::now().date_naive().to_string();
    save(&crate::db::pool(&app).await?, id, fact, &today).await
}

/// Delete a fact.
#[tauri::command]
pub async fn delete_fact<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    id: i64,
) -> Result<(), Error> {
    remove(&crate::db::pool(&app).await?, id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::migrated_pool;

    fn review(login: &str, repository: &str, count: i64) -> NewFact {
        NewFact {
            about: login.to_string(),
            what: REVIEWS.to_string(),
            scope: repository.to_string(),
            value: String::new(),
            count,
            total: 10,
            source: "GitHub reviews".to_string(),
            seen_on: "2026-10-12".to_string(),
        }
    }

    fn day(month: u32, day: u32) -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, month, day).expect("date")
    }

    #[tokio::test]
    async fn picks_only_the_people_for_the_repository_asked_about() {
        let pool = migrated_pool().await;
        recount(
            &pool,
            &[REVIEWS],
            vec![
                review("priya-n", "acme/api", 14),
                review("tomasz", "acme/web", 9),
                review("dev-p", "acme/api-gateway", 7),
            ],
        )
        .await
        .expect("recount");

        let people = people_for(&pool, "Acme/API", 4).await.expect("people");

        assert_eq!(
            people.iter().map(|p| p.login.as_str()).collect::<Vec<_>>(),
            ["priya-n"],
            "neither another repository nor one that merely starts the same way"
        );
        assert!(people_for(&pool, "acme/billing", 4)
            .await
            .expect("people")
            .is_empty());
    }

    #[tokio::test]
    async fn puts_the_most_frequent_reviewer_first_and_stops_at_the_limit() {
        let pool = migrated_pool().await;
        recount(
            &pool,
            &[REVIEWS],
            vec![
                review("a", "acme/api", 2),
                review("b", "acme/api", 9),
                review("c", "acme/api", 5),
            ],
        )
        .await
        .expect("recount");

        let people = people_for(&pool, "acme/api", 2).await.expect("people");

        assert_eq!(
            people.iter().map(|p| p.login.as_str()).collect::<Vec<_>>(),
            ["b", "c"]
        );
    }

    #[tokio::test]
    async fn a_recount_replaces_its_own_rows_and_drops_those_it_no_longer_finds() {
        let pool = migrated_pool().await;
        recount(&pool, &[REVIEWS], vec![review("old", "acme/api", 3)])
            .await
            .expect("first");
        recount(&pool, &[REVIEWS], vec![review("new", "acme/api", 4)])
            .await
            .expect("second");

        let logins: Vec<String> = list(&pool)
            .await
            .expect("list")
            .into_iter()
            .map(|fact| fact.about)
            .collect();

        assert_eq!(logins, ["new"], "someone who stopped reviewing ages out");
    }

    #[tokio::test]
    async fn a_recount_never_overwrites_what_the_user_wrote() {
        let pool = migrated_pool().await;
        recount(&pool, &[REVIEWS], vec![review("priya-n", "acme/api", 14)])
            .await
            .expect("counted");
        let id = list(&pool).await.expect("list")[0].id;

        save(
            &pool,
            Some(id),
            NewFact {
                count: 0,
                total: 0,
                ..review("priya-n", "acme/api", 0)
            },
            "2026-10-16",
        )
        .await
        .expect("edit");

        let done = recount(&pool, &[REVIEWS], vec![review("priya-n", "acme/api", 99)])
            .await
            .expect("recount");

        let rows = list(&pool).await.expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].origin, YOURS);
        assert_eq!(
            rows[0].count, 0,
            "the edit survived a recount that found 99"
        );
        assert_eq!(
            done,
            Recounted {
                written: 0,
                kept: 1
            }
        );
    }

    #[tokio::test]
    async fn a_row_the_user_deleted_stays_deleted_through_a_recount() {
        let pool = migrated_pool().await;
        recount(&pool, &[REVIEWS], vec![review("bot-ish", "acme/api", 8)])
            .await
            .expect("counted");
        let id = list(&pool).await.expect("list")[0].id;

        remove(&pool, id).await.expect("remove");
        recount(&pool, &[REVIEWS], vec![review("bot-ish", "acme/api", 8)])
            .await
            .expect("recount");

        assert!(list(&pool).await.expect("list").is_empty());
        assert!(people_for(&pool, "acme/api", 4)
            .await
            .expect("people")
            .is_empty());
    }

    #[tokio::test]
    async fn deleting_your_own_row_removes_it_outright() {
        let pool = migrated_pool().await;
        save(&pool, None, review("me-added", "acme/api", 0), "2026-10-16")
            .await
            .expect("save");
        let id = list(&pool).await.expect("list")[0].id;

        remove(&pool, id).await.expect("remove");

        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM facts")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(
            rows, 0,
            "nothing hidden is left behind for a row nobody counted"
        );
    }

    #[tokio::test]
    async fn a_failed_recount_leaves_the_old_rows() {
        let pool = migrated_pool().await;
        recount(&pool, &[REVIEWS], vec![review("keep", "acme/api", 3)])
            .await
            .expect("first");

        // A row with a NULL-violating shape cannot be built from NewFact, so
        // break the table instead: the second insert has nowhere to go.
        sqlx::query("DROP INDEX facts_key")
            .execute(&pool)
            .await
            .expect("drop");
        sqlx::query("ALTER TABLE facts RENAME COLUMN seen_on TO seen_on_renamed")
            .execute(&pool)
            .await
            .expect("rename");

        let failed = recount(&pool, &[REVIEWS], vec![review("replace", "acme/api", 4)]).await;
        assert!(failed.is_err());

        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM facts WHERE about = 'keep'")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(
            remaining, 1,
            "the delete rolled back with the failed insert"
        );
    }

    #[tokio::test]
    async fn a_name_you_gave_is_used_for_the_login() {
        let pool = migrated_pool().await;
        recount(&pool, &[REVIEWS], vec![review("priya-n", "acme/api", 14)])
            .await
            .expect("recount");
        save(
            &pool,
            None,
            NewFact {
                about: "@Priya-N".to_string(),
                what: NAME.to_string(),
                scope: String::new(),
                value: "Priya Nair".to_string(),
                count: 0,
                total: 0,
                source: String::new(),
                seen_on: String::new(),
            },
            "2026-10-16",
        )
        .await
        .expect("name");

        let people = people_for(&pool, "acme/api", 4).await.expect("people");

        assert_eq!(people[0].name.as_deref(), Some("Priya Nair"));
        let line = person_line(&people[0], "acme/api", day(10, 16));
        assert!(line.contains("Priya Nair (@priya-n)"), "{line}");
        assert!(line.contains("14 of your last 10"), "{line}");
    }

    #[tokio::test]
    async fn a_meeting_count_alone_is_not_a_profile() {
        let pool = migrated_pool().await;
        assert!(!has_profile(&pool).await.expect("empty"));

        recount(
            &pool,
            &[MEETS],
            vec![NewFact {
                about: "sam patel".to_string(),
                what: MEETS.to_string(),
                scope: String::new(),
                value: String::new(),
                count: 5,
                total: 0,
                source: "calendar".to_string(),
                seen_on: "2026-10-12".to_string(),
            }],
        )
        .await
        .expect("meets");
        assert!(
            !has_profile(&pool).await.expect("meets only"),
            "knowing who you meet does not tell a draft how to sound"
        );

        recount(&pool, &[REVIEWS], vec![review("priya-n", "acme/api", 1)])
            .await
            .expect("reviews");
        assert!(has_profile(&pool).await.expect("with a reviewer"));
    }

    #[test]
    fn says_a_count_and_never_an_impression() {
        let fact = |what: &str, value: &str, count, total| Fact {
            id: 0,
            about: YOU.to_string(),
            what: what.to_string(),
            scope: String::new(),
            value: value.to_string(),
            count,
            total,
            source: "x".to_string(),
            seen_on: "2026-10-10".to_string(),
            origin: COUNTED.to_string(),
        };

        assert_eq!(
            style_line(&fact(GREETING, "Hi", 17, 20)),
            "Greeting: opens with \"Hi\" (17 of 20)."
        );
        assert_eq!(
            style_line(&fact(GREETING, "", 0, 20)),
            "Greeting: none (0 of 20)."
        );
        assert_eq!(
            style_line(&fact(LENGTH, "40", 0, 20)),
            "Length: about 40 words (median of 20)."
        );
        assert_eq!(
            style_line(&fact(EMOJI, "", 0, 20)),
            "Emoji: none (0 of 20)."
        );
        assert_eq!(
            style_line(&fact(EMOJI, "", 3, 20)),
            "Emoji: used in 3 of 20."
        );
        assert_eq!(
            style_line(&fact(SPELLING, "British", 0, 0)),
            "Spelling: British."
        );
        assert_eq!(
            style_line(&fact(GREETING, "Hey", 0, 0)),
            "Greeting: opens with \"Hey\".",
            "a row you wrote has no sample to quote"
        );
    }

    #[test]
    fn flags_a_count_made_long_ago_and_not_a_recent_one() {
        let person = Person {
            login: "priya-n".to_string(),
            name: None,
            count: 2,
            total: 5,
            source: "GitHub reviews".to_string(),
            seen_on: "2026-10-01".to_string(),
            origin: COUNTED.to_string(),
        };

        assert!(!person_line(&person, "acme/api", day(10, 13)).contains("out of date"));
        let later = chrono::NaiveDate::from_ymd_opt(2027, 3, 1).expect("date");
        assert!(person_line(&person, "acme/api", later).contains("may be out of date"));

        let told = Person {
            origin: YOURS.to_string(),
            total: 0,
            ..person
        };
        assert!(
            !person_line(&told, "acme/api", later).contains("out of date"),
            "something you said does not go stale by the calendar"
        );
    }
}
