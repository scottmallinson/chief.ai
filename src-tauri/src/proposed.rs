//! The drafts Chief prepared for things it noticed, and what became of them.
//!
//! **Nothing here sends anything.** That boundary is the whole reason drafting
//! and sending are separate steps with separate reviews, and it is asserted
//! rather than trusted: see `drafts_without_touching_the_network` in
//! `daemon.rs`. A proposal is a file in the corpus and a row saying it exists.
//!
//! The body lives in the corpus rather than in the database, so a draft the
//! user opened in their own editor is the one they see on screen. The row is
//! only the state COSTA's `task-action-state.json` keeps: which item it came
//! from, where the body is, and whether anybody has acted on it.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::db::Error;

/// How many proposals a single read returns when the caller does not say.
const DEFAULT_LIMIT: i64 = 50;

/// The largest page we will build.
const MAX_LIMIT: i64 = 500;

/// What has happened to a proposal.
pub const DRAFTED: &str = "drafted";
pub const DISMISSED: &str = "dismissed";

/// A draft Chief prepared, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub id: i64,
    /// Where the thing it is about came from, e.g. `github`.
    pub source: String,
    /// Which connected account, and `0` when it came from none.
    pub account_id: i64,
    /// Identifies the item this was drafted for, so it is only drafted once.
    pub dedupe_key: String,
    /// What it is about, for the card.
    pub title: String,
    /// The item as Chief read it, so the card can say why this was proposed.
    pub context: String,
    /// Where the body is in the corpus.
    pub path: String,
    /// [`DRAFTED`] or [`DISMISSED`].
    pub status: String,
    pub created_at: String,
    /// When the user did something about it.
    pub acted_at: Option<String>,
}

/// The columns that make up a [`Proposal`], so every query agrees.
const COLUMNS: &str =
    "id, source, account_id, dedupe_key, title, context, path, status, created_at, acted_at";

/// A proposal about to be stored.
#[derive(Debug, Clone)]
pub struct NewProposal {
    pub source: String,
    pub account_id: i64,
    pub dedupe_key: String,
    pub title: String,
    pub context: String,
    pub path: String,
}

/// Read proposals, newest first.
pub async fn fetch(pool: &SqlitePool, limit: Option<i64>) -> Result<Vec<Proposal>, Error> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let rows = sqlx::query_as::<_, Proposal>(&format!(
        "SELECT {COLUMNS}
         FROM proposed_actions
         ORDER BY created_at DESC, id DESC
         LIMIT ?1"
    ))
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Has this source item already been proposed for?
///
/// Asked **before** the model is, so a pass that has caught up costs nothing —
/// the same shape as `work_log::has_logged`.
pub async fn has_proposed(
    pool: &SqlitePool,
    source: &str,
    account_id: i64,
    dedupe_key: &str,
) -> Result<bool, Error> {
    let found: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM proposed_actions
         WHERE source = ?1 AND account_id = ?2 AND dedupe_key = ?3
         LIMIT 1",
    )
    .bind(source)
    .bind(account_id)
    .bind(dedupe_key)
    .fetch_optional(pool)
    .await?;

    Ok(found.is_some())
}

/// Store a proposal, or do nothing if that item already has one.
///
/// `None` means it was already there. The unique index makes this true even
/// against a pass that raced another, which the `has_proposed` check alone
/// would not.
pub async fn insert_new(
    pool: &SqlitePool,
    proposal: NewProposal,
) -> Result<Option<Proposal>, Error> {
    let stored = sqlx::query_as::<_, Proposal>(&format!(
        "INSERT INTO proposed_actions
             (source, account_id, dedupe_key, title, context, path, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, '{DRAFTED}', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         ON CONFLICT (source, account_id, dedupe_key) DO NOTHING
         RETURNING {COLUMNS}"
    ))
    .bind(&proposal.source)
    .bind(proposal.account_id)
    .bind(&proposal.dedupe_key)
    .bind(&proposal.title)
    .bind(&proposal.context)
    .bind(&proposal.path)
    .fetch_optional(pool)
    .await?;

    Ok(stored)
}

/// Mark a proposal dismissed. The row stays, so it is never drafted again.
pub async fn dismiss(pool: &SqlitePool, id: i64) -> Result<(), Error> {
    sqlx::query(&format!(
        "UPDATE proposed_actions
         SET status = '{DISMISSED}', acted_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?1"
    ))
    .bind(id)
    .execute(pool)
    .await?;

    Ok(())
}

/// A proposal with its body, as the review card shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Drafted {
    #[serde(flatten)]
    pub proposal: Proposal,
    /// The draft as it is on disk right now, so a file the user edited in their
    /// own editor is the one on screen.
    pub body: String,
}

/// The drafts Chief has prepared, with their bodies.
///
/// A proposal whose file has gone is dropped rather than shown empty: the
/// corpus is a folder the user is invited to manage, and deleting the file is
/// a reasonable way to say no.
#[tauri::command]
pub async fn list_proposals<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Vec<Drafted>, Error> {
    let context = crate::recipe::context(&app)
        .await
        .map_err(|error| Error::Sqlx(sqlx::Error::Protocol(error.to_string())))?;

    let mut drafted = Vec::new();

    for proposal in fetch(&context.pool, None).await? {
        if proposal.status == DISMISSED {
            continue;
        }

        if let Ok(body) = context.corpus.read(&proposal.path).await {
            drafted.push(Drafted { proposal, body });
        }
    }

    Ok(drafted)
}

/// Put the user's edit back on disk. The file is the draft.
#[tauri::command]
pub async fn save_proposal<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    path: String,
    body: String,
) -> Result<(), Error> {
    let context = crate::recipe::context(&app)
        .await
        .map_err(|error| Error::Sqlx(sqlx::Error::Protocol(error.to_string())))?;

    context
        .corpus
        .write(&path, &body)
        .await
        .map_err(|error| Error::Sqlx(sqlx::Error::Protocol(error.to_string())))
}

/// Say no to a proposal. The row stays so it is never drafted again.
#[tauri::command]
pub async fn dismiss_proposal<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    id: i64,
) -> Result<(), Error> {
    dismiss(&crate::db::pool(&app).await?, id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::migrated_pool;

    fn about(key: &str) -> NewProposal {
        NewProposal {
            source: "github".to_string(),
            account_id: 1,
            dedupe_key: key.to_string(),
            title: format!("Chase a review on {key}"),
            context: "Open for eleven days with no review".to_string(),
            path: format!("proposed/2026-08-29-{}.md", key.replace(['/', '#'], "-")),
        }
    }

    #[tokio::test]
    async fn stores_a_proposal_and_reads_it_back() {
        let pool = migrated_pool().await;

        let stored = insert_new(&pool, about("scottmallinson/chief.ai#44"))
            .await
            .expect("should insert")
            .expect("should be new");

        assert_eq!(stored.status, DRAFTED);
        assert_eq!(stored.title, "Chase a review on scottmallinson/chief.ai#44");
        assert_eq!(fetch(&pool, None).await.expect("should read").len(), 1);
    }

    #[tokio::test]
    async fn never_proposes_twice_for_the_same_item() {
        let pool = migrated_pool().await;

        insert_new(&pool, about("scottmallinson/chief.ai#44"))
            .await
            .expect("should insert")
            .expect("should be new");

        let again = insert_new(&pool, about("scottmallinson/chief.ai#44"))
            .await
            .expect("should not fail");

        assert!(again.is_none(), "the same item must not be drafted twice");
        assert_eq!(fetch(&pool, None).await.expect("read").len(), 1);
    }

    #[tokio::test]
    async fn tells_a_caller_before_it_spends_a_model_call() {
        let pool = migrated_pool().await;
        let key = "scottmallinson/chief.ai#44";

        assert!(!has_proposed(&pool, "github", 1, key).await.expect("read"));

        insert_new(&pool, about(key)).await.expect("insert");

        assert!(has_proposed(&pool, "github", 1, key).await.expect("read"));
    }

    #[tokio::test]
    async fn keeps_two_accounts_apart() {
        let pool = migrated_pool().await;
        let key = "scottmallinson/chief.ai#44";

        insert_new(&pool, about(key)).await.expect("insert");

        let other = NewProposal {
            account_id: 2,
            ..about(key)
        };

        assert!(
            insert_new(&pool, other).await.expect("insert").is_some(),
            "a work and a personal account are two people's inboxes"
        );
    }

    #[tokio::test]
    async fn a_dismissed_proposal_is_never_drafted_again() {
        let pool = migrated_pool().await;
        let key = "scottmallinson/chief.ai#44";

        let stored = insert_new(&pool, about(key))
            .await
            .expect("insert")
            .expect("new");

        dismiss(&pool, stored.id).await.expect("should dismiss");

        assert!(
            has_proposed(&pool, "github", 1, key).await.expect("read"),
            "dismissing has to keep the slot, or the next pass drafts it again"
        );

        let read = fetch(&pool, None).await.expect("read");
        assert_eq!(read[0].status, DISMISSED);
        assert!(read[0].acted_at.is_some(), "and record when that happened");
    }

    #[tokio::test]
    async fn reads_the_newest_first() {
        let pool = migrated_pool().await;

        for key in ["a#1", "b#2", "c#3"] {
            insert_new(&pool, about(key)).await.expect("insert");
        }

        let read = fetch(&pool, None).await.expect("read");

        assert_eq!(read.len(), 3);
        assert_eq!(read[0].dedupe_key, "c#3");
    }
}
