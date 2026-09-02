//! Connecting and disconnecting the accounts Chief reads from.
//!
//! Sign-in happens in two commands so the user can see their code while we
//! wait: `start_login` returns what to enter, `finish_login` blocks until they
//! have entered it. The device code itself never reaches the renderer — it
//! stays in backend state.
//!
//! Every command takes the service by name rather than being named after one,
//! so a new provider costs an arm in `dispatch` and no new commands.

use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::Mutex;

use crate::calendar;
use crate::corpus;
use crate::db;
use crate::github::{self, Client, DeviceLogin};
use crate::integrations::{
    self, Account, NewAccount, API_KEY, CALENDAR, GITHUB, LINEAR, MICROSOFT, OAUTH, SUBSCRIPTION,
};
use crate::linear;
use crate::microsoft;
use crate::oauth::Provider;
use crate::proposed;
use crate::sync_state;
use crate::work_log;

/// A sign-in that has been started, whichever shape it takes.
///
/// The two providers Chief has sign in differently and neither is a special
/// case of the other: GitHub shows a code the user types somewhere else, while
/// Microsoft sends them to a browser and waits on a port. Holding both here,
/// rather than making one pretend to be the other, is what lets `finish_login`
/// stay one command.
pub enum Flow {
    Device(github::PendingLogin),
    Browser(Box<microsoft::PendingLogin>),
}

/// The sign-in waiting to be completed, if any.
#[derive(Default)]
pub struct Pending(Mutex<Option<Flow>>);

/// What the user has to do next, phrased for the interface to render.
///
/// Tagged rather than a union of optional fields, so a renderer that forgets a
/// case fails to compile rather than showing an empty dialog.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Login {
    /// Type this code, over there.
    Device(DeviceLogin),
    /// Open this, and Chief will wait for the browser to come back.
    Browser { url: String },
}

/// What can go wrong connecting or disconnecting an account.
///
/// This module's own error rather than one provider's, because this module is
/// the one part of the integration layer that is not about a particular
/// service. Borrowing GitHub's meant reporting an unknown service as an HTTP
/// 400 from GitHub — an account of a request nobody made, to a host nobody
/// called.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("there is no integration called '{0}'")]
    NoSuchService(String),
    #[error(transparent)]
    Provider(#[from] github::Error),
    #[error(transparent)]
    Microsoft(#[from] microsoft::Error),
    #[error("that sign-in was for a different service")]
    WrongFlow,
    #[error(transparent)]
    Storage(#[from] db::Error),
    #[error(transparent)]
    Calendar(#[from] calendar::Error),
    #[error(transparent)]
    Linear(#[from] linear::Error),
    #[error(transparent)]
    Corpus(#[from] corpus::Error),
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// Reject a service Chief does not know, rather than failing later and less
/// clearly.
fn known(service: &str) -> Result<(), Error> {
    if service == GITHUB || service == MICROSOFT || service == CALENDAR || service == LINEAR {
        return Ok(());
    }

    Err(Error::NoSuchService(service.to_string()))
}

/// Subscribe to a calendar by its published address.
///
/// Validated by reading it once, before anything is stored: a mistyped address
/// should fail while the user is looking at the field, not produce an empty
/// brief tomorrow morning. The URL is a bearer credential — see `calendar.rs` —
/// so it is stored as one and never returned to the frontend.
#[tauri::command]
pub async fn add_calendar<R: Runtime>(
    app: AppHandle<R>,
    calendar: State<'_, calendar::Client>,
    url: String,
    label: Option<String>,
) -> Result<Account, Error> {
    let address = calendar::normalise(&url)?;

    // Prove it is reachable and really is a calendar before it is kept.
    calendar.read(&address).await?;

    let pool = db::pool(&app).await?;

    // Keyed on the address, so re-adding the same calendar updates the row it
    // already has rather than making a second one.
    let stored = integrations::save(
        &pool,
        NewAccount {
            service: CALENDAR,
            account_key: &address,
            identity: label.as_deref().filter(|it| !it.trim().is_empty()),
            credential_kind: SUBSCRIPTION,
            access_token: &address,
            refresh_token: None,
            expires_at: None,
            scopes: None,
            client_id: None,
            client_secret: None,
        },
    )
    .await?;

    Ok(stored)
}

/// Connect Linear with a personal API key.
///
/// Validated by reading with it before anything is stored, so a mistyped key
/// fails while the user is looking at the field rather than producing an empty
/// brief tomorrow morning. The key is a bearer credential — see `linear.rs` —
/// so it is stored as one and never returned to the frontend.
#[tauri::command]
pub async fn add_linear_key<R: Runtime>(
    app: AppHandle<R>,
    linear: State<'_, linear::Client>,
    key: String,
) -> Result<Account, Error> {
    let key = key.trim().to_string();

    // The key names its own owner, so one read both proves it works and says
    // whose workspace this is.
    let found = linear.assigned(&key).await?;
    let pool = db::pool(&app).await?;

    let stored = integrations::save(
        &pool,
        NewAccount {
            service: LINEAR,
            // Keyed on the person, so re-pasting a rotated key updates the row
            // it already has rather than making a second account.
            account_key: &found.viewer.email,
            identity: Some(found.viewer.name.as_str()).filter(|name| !name.is_empty()),
            credential_kind: API_KEY,
            access_token: &key,
            refresh_token: None,
            expires_at: None,
            scopes: None,
            client_id: None,
            client_secret: None,
        },
    )
    .await?;

    Ok(stored)
}

/// Begin signing in and return what the user must do next.
#[tauri::command]
pub async fn start_login(
    service: String,
    github_client: State<'_, Client>,
    microsoft_client: State<'_, microsoft::Client>,
    pending: State<'_, Pending>,
) -> Result<Login, Error> {
    known(&service)?;

    if service == CALENDAR {
        return Err(Error::NoSuchService(
            "a calendar subscription is added with its address, not by signing in".to_string(),
        ));
    }

    if service == LINEAR {
        return Err(Error::NoSuchService(
            "Linear is connected with a personal API key, not by signing in".to_string(),
        ));
    }

    let (flow, login) = if service == MICROSOFT {
        let client_id = microsoft::client_id()?;
        let (started, url) = microsoft_client.start_login(&client_id).await?;

        (Flow::Browser(Box::new(started)), Login::Browser { url })
    } else {
        let client_id = github::client_id()?;
        let started = github_client.start_login(&client_id).await?;
        let login = Login::Device(started.login.clone());

        (Flow::Device(started), login)
    };

    *pending.0.lock().await = Some(flow);

    Ok(login)
}

/// Wait for the user to finish signing in, then store the credential.
#[tauri::command]
pub async fn finish_login<R: Runtime>(
    service: String,
    app: AppHandle<R>,
    github_client: State<'_, Client>,
    microsoft_client: State<'_, microsoft::Client>,
    pending: State<'_, Pending>,
) -> Result<Vec<Account>, Error> {
    known(&service)?;

    let started = pending
        .0
        .lock()
        .await
        .take()
        .ok_or(github::Error::NotConnected)?;

    let pool = db::pool(&app).await?;

    // Whichever provider it is, the shape is the same: finish the handshake,
    // ask who this is before storing so two accounts on one service cannot
    // collide on a placeholder key, then save.
    match (service.as_str(), started) {
        (MICROSOFT, Flow::Browser(started)) => {
            let client_id = microsoft::client_id()?;
            let tokens = microsoft_client.finish_login(&client_id, *started).await?;
            let mailbox = microsoft_client.me(&tokens.access_token).await?;

            integrations::save(
                &pool,
                NewAccount {
                    service: MICROSOFT,
                    account_key: &mailbox,
                    identity: Some(&mailbox),
                    credential_kind: OAUTH,
                    access_token: &tokens.access_token,
                    refresh_token: tokens.refresh_token.as_deref(),
                    // Graph access tokens last about an hour, and it says so —
                    // storing when it runs out is what lets the session renew
                    // before a call rather than after one has been refused.
                    expires_at: tokens.expires_in.map(expires_at).as_deref(),
                    scopes: Some(&microsoft_client.scopes().join(" ")),
                    client_id: None,
                    client_secret: None,
                },
            )
            .await?;
        }
        (_, Flow::Device(started)) => {
            let client_id = github::client_id()?;
            let (access_token, refresh_token) =
                github_client.finish_login(&client_id, &started).await?;
            let login = github_client.viewer(&access_token).await?;

            integrations::save(
                &pool,
                NewAccount {
                    service: GITHUB,
                    account_key: &login,
                    identity: Some(&login),
                    credential_kind: OAUTH,
                    access_token: &access_token,
                    refresh_token: refresh_token.as_deref(),
                    expires_at: None,
                    scopes: Some(&github_client.scopes().join(" ")),
                    client_id: None,
                    client_secret: None,
                },
            )
            .await?;
        }
        // Asking to finish a Microsoft sign-in while a GitHub one is pending,
        // or the reverse. Better said plainly than by using the wrong client.
        _ => return Err(Error::WrongFlow),
    }

    Ok(integrations::all_accounts(&pool).await?)
}

/// When a token that lasts `seconds` more runs out, as an ISO-8601 instant.
fn expires_at(seconds: u64) -> String {
    let at = chrono::Utc::now() + chrono::Duration::seconds(i64::try_from(seconds).unwrap_or(0));

    at.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// Every connected account, whatever the service.
#[tauri::command]
pub async fn connections<R: Runtime>(app: AppHandle<R>) -> Result<Vec<Account>, Error> {
    let pool = db::pool(&app).await?;

    Ok(integrations::all_accounts(&pool).await?)
}

/// What unlinking an account would destroy.
///
/// Read before anything is deleted, so the confirmation can state it. The
/// counts are the point: a person unlinking an account is thinking about a
/// credential, and has no reason to know that four months of their work log
/// and eleven drafts are behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountData {
    /// Rows in the work log, which are also what search answers from.
    pub entries: i64,
    /// Drafts Chief prepared, whether or not the user dismissed them.
    pub proposals: i64,
}

/// What unlinking this account would take with it.
#[tauri::command]
pub async fn account_data<R: Runtime>(
    account_id: i64,
    app: AppHandle<R>,
) -> Result<AccountData, Error> {
    let pool = db::pool(&app).await?;

    Ok(AccountData {
        entries: work_log::count_for_account(&pool, account_id).await?,
        proposals: i64::try_from(proposed::for_account(&pool, account_id).await?.len())
            .unwrap_or(i64::MAX),
    })
}

/// Delete everything one account put on this machine.
///
/// **Keyed on `account_id`, never on `source`.** The specification this came
/// from proposed `purge_integration_data(source)`, which would wipe both of a
/// person's GitHub accounts when they unlinked one — migration v3 made the
/// account the unit precisely because holding a work and a personal one is
/// ordinary.
///
/// Order matters in one place: the proposals are read before their rows go,
/// because the row is the only thing that knows where the body is. A file that
/// has already been deleted by hand is not an error — the corpus is a folder
/// the user is invited to manage, and `list_proposals` already treats a missing
/// body as the user having said no.
///
/// Every path goes through [`corpus::Corpus::resolve`], so a stored path that
/// is malformed or has been tampered with cannot reach outside the corpus root.
/// A path that is refused leaves the file alone rather than failing the purge:
/// the credential and the rows still have to go.
async fn purge_account_data(
    pool: &sqlx::SqlitePool,
    corpus: &corpus::Corpus,
    account_id: i64,
) -> Result<(), Error> {
    for proposal in proposed::for_account(pool, account_id).await? {
        let Ok(path) = corpus.resolve(&proposal.path) else {
            continue;
        };

        // `NotFound` is the user having deleted it themselves.
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!("could not delete {}: {error}", path.display()),
        }
    }

    proposed::forget_account(pool, account_id).await?;
    work_log::forget_account(pool, account_id).await?;
    sync_state::forget(pool, account_id).await?;

    Ok(())
}

/// Forget one account: its credential, and everything it put here.
///
/// **"Disconnect" now means what a person reading it assumes.** It used to
/// delete the credential row and nothing else, leaving the account's work log
/// entries, its drafts and their markdown behind — in an app whose one promise
/// is about where the user's data lives and what happens to it. The confirmation
/// in front of this states the counts, from [`account_data`].
///
/// The credential goes **last**. If deleting the data fails half way, the
/// account is still connected and still visible, which is a state the user can
/// act on; the other order leaves orphaned rows nothing can name.
#[tauri::command]
pub async fn disconnect<R: Runtime>(
    account_id: i64,
    app: AppHandle<R>,
) -> Result<Vec<Account>, Error> {
    let pool = db::pool(&app).await?;
    let home = app
        .path()
        .home_dir()
        .map_err(|error| corpus::Error::Index(error.to_string()))?;
    let corpus = corpus::Corpus::at(corpus::root(&pool, &home).await?);

    purge_account_data(&pool, &corpus, account_id).await?;
    integrations::forget(&pool, account_id).await?;

    Ok(integrations::all_accounts(&pool).await?)
}

/// Name an account, or clear its name.
#[tauri::command]
pub async fn label_account<R: Runtime>(
    account_id: i64,
    label: Option<String>,
    app: AppHandle<R>,
) -> Result<Vec<Account>, Error> {
    let pool = db::pool(&app).await?;
    integrations::set_label(&pool, account_id, label.as_deref()).await?;

    Ok(integrations::all_accounts(&pool).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::test_support::migrated_pool;
    use crate::proposed::NewProposal;
    use crate::work_log::WorkLogRecord;

    /// A corpus in a directory of this test's own, removed when it drops.
    struct Scratch {
        corpus: corpus::Corpus,
        root: std::path::PathBuf,
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn scratch(name: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("chief-purge-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("drafts")).expect("should make a corpus");

        Scratch {
            corpus: corpus::Corpus::at(root.clone()),
            root,
        }
    }

    fn entry(account_id: i64, title: &str) -> WorkLogRecord {
        WorkLogRecord {
            timestamp: "2026-09-01T09:00:00.000Z".to_string(),
            source: GITHUB.to_string(),
            category: "pr".to_string(),
            title: title.to_string(),
            content: format!("{title} was merged"),
            summary: Some("merged".to_string()),
            url: None,
            raw_ref: None,
            external_id: format!("owner/repo#{title}"),
            account_id,
        }
    }

    async fn draft(
        pool: &sqlx::SqlitePool,
        scratch: &Scratch,
        account_id: i64,
        name: &str,
    ) -> String {
        let path = format!("drafts/{name}.md");
        std::fs::write(
            scratch.corpus.resolve(&path).expect("a corpus path"),
            format!("A draft about {name}.\n"),
        )
        .expect("should write");

        crate::proposed::insert_new(
            pool,
            NewProposal {
                source: GITHUB.to_string(),
                account_id,
                dedupe_key: format!("owner/repo#{name}"),
                title: name.to_string(),
                context: "waiting on review".to_string(),
                path: path.clone(),
            },
        )
        .await
        .expect("should store");

        path
    }

    async fn titles(pool: &sqlx::SqlitePool) -> Vec<String> {
        sqlx::query_scalar("SELECT title FROM work_logs ORDER BY id")
            .fetch_all(pool)
            .await
            .expect("should read")
    }

    /// **The criterion the specification's own design fails.**
    ///
    /// It proposed `purge_integration_data(source)`, which would take both of
    /// a person's GitHub accounts when they unlinked one. Migration v3 made
    /// the account the unit for exactly this reason.
    ///
    /// Proved by keying the deletes on `source` instead:
    ///
    /// ```text
    /// the other account's work is not this account's to delete
    ///   left: []
    ///  right: ["kept"]
    /// ```
    #[tokio::test]
    async fn purging_one_account_leaves_the_other_on_the_same_service_alone() {
        let pool = migrated_pool().await;
        let scratch = scratch("two-accounts");

        crate::work_log::upsert(&pool, entry(1, "mine"))
            .await
            .expect("should store");
        crate::work_log::upsert(&pool, entry(2, "kept"))
            .await
            .expect("should store");

        let going = draft(&pool, &scratch, 1, "going").await;
        let staying = draft(&pool, &scratch, 2, "staying").await;

        purge_account_data(&pool, &scratch.corpus, 1)
            .await
            .expect("should purge");

        assert_eq!(
            titles(&pool).await,
            vec!["kept".to_string()],
            "the other account's work is not this account's to delete"
        );
        assert!(
            crate::proposed::for_account(&pool, 2)
                .await
                .expect("should read")
                .len()
                == 1,
            "the other account's drafts should still be there"
        );
        assert!(
            !scratch.corpus.resolve(&going).expect("a path").exists(),
            "the purged account's draft should be gone"
        );
        assert_eq!(
            std::fs::read_to_string(scratch.corpus.resolve(&staying).expect("a path"))
                .expect("should read"),
            "A draft about staying.\n",
            "the other account's draft should be byte-identical"
        );
    }

    /// The work log is what search answers from, so a row that is gone from
    /// the table and still in the index is a deleted item Chief will still
    /// quote back.
    ///
    /// Proved by removing migration 8's `work_logs_fts_delete` trigger, which
    /// leaves the row gone from the table and present in the index:
    ///
    /// ```text
    /// a purged row must not still be findable
    ///   left: 1
    ///  right: 0
    /// ```
    #[tokio::test]
    async fn a_purged_row_leaves_the_search_index_too() {
        let pool = migrated_pool().await;
        let scratch = scratch("index");

        crate::work_log::upsert(&pool, entry(1, "hydrofoil"))
            .await
            .expect("should store");

        purge_account_data(&pool, &scratch.corpus, 1)
            .await
            .expect("should purge");

        let stale: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM work_logs_fts WHERE work_logs_fts MATCH '\"hydrofoil\"'",
        )
        .fetch_one(&pool)
        .await
        .expect("the index should be searchable");

        assert_eq!(stale, 0, "a purged row must not still be findable");
    }

    /// Entries the user typed are not any account's to delete.
    ///
    /// They carry `account_id = 0`, the sentinel for "no account", which
    /// `AUTOINCREMENT` never issues — and unlike a merged pull request there
    /// is nothing to re-fetch them from.
    ///
    /// Proved by widening the delete to `account_id = ?1 OR external_id IS
    /// NULL` — the plausible mistake, since every hand-written entry has a
    /// null external id:
    ///
    /// ```text
    /// a note the user typed is not an account's to delete
    ///   left: 0
    ///  right: 1
    /// ```
    #[tokio::test]
    async fn nothing_the_user_typed_by_hand_is_ever_purged() {
        let pool = migrated_pool().await;
        let scratch = scratch("hand-written");

        crate::work_log::insert(
            &pool,
            crate::work_log::NewWorkLogEntry {
                source: "note".to_string(),
                content: "Spoke to Dana about the migration".to_string(),
                timestamp: None,
                summary: None,
                external_id: None,
                account_id: None,
            },
        )
        .await
        .expect("should store");

        crate::work_log::upsert(&pool, entry(1, "mine"))
            .await
            .expect("should store");

        purge_account_data(&pool, &scratch.corpus, 1)
            .await
            .expect("should purge");

        assert_eq!(
            titles(&pool).await.len(),
            1,
            "a note the user typed is not an account's to delete"
        );
    }

    /// A stored path that is malformed, or has been tampered with, must not
    /// reach outside the corpus — and must not stop the purge either. The
    /// credential and the rows still have to go.
    #[tokio::test]
    async fn a_path_that_climbs_out_of_the_corpus_deletes_nothing_and_stops_nothing() {
        let pool = migrated_pool().await;
        let scratch = scratch("traversal");

        let outside = scratch.root.join("secret.md");
        std::fs::write(&outside, "not the corpus\n").expect("should write");

        crate::proposed::insert_new(
            &pool,
            NewProposal {
                source: GITHUB.to_string(),
                account_id: 1,
                dedupe_key: "owner/repo#9".to_string(),
                title: "climbing".to_string(),
                context: "waiting on review".to_string(),
                // Resolves to `<root>/secret.md` if the traversal defence is
                // not consulted.
                path: "drafts/../secret.md".to_string(),
            },
        )
        .await
        .expect("should store");

        purge_account_data(&pool, &scratch.corpus, 1)
            .await
            .expect("a refused path is not a failed purge");

        assert!(
            outside.exists(),
            "a path outside the corpus is not this function's to delete"
        );
        assert!(
            crate::proposed::for_account(&pool, 1)
                .await
                .expect("should read")
                .is_empty(),
            "the row still has to go, whatever became of its body"
        );
    }

    /// A file the user already deleted is them having said no, which
    /// `list_proposals` treats the same way.
    #[tokio::test]
    async fn a_draft_whose_file_has_already_gone_is_not_an_error() {
        let pool = migrated_pool().await;
        let scratch = scratch("missing");

        let path = draft(&pool, &scratch, 1, "removed").await;
        std::fs::remove_file(scratch.corpus.resolve(&path).expect("a path"))
            .expect("should remove");

        purge_account_data(&pool, &scratch.corpus, 1)
            .await
            .expect("a missing body is not a failed purge");
    }

    /// The counts the confirmation states. They are read before anything is
    /// deleted, and they are the whole reason the confirmation is worth
    /// showing: somebody unlinking an account is thinking about a credential.
    #[tokio::test]
    async fn says_what_would_go_before_anything_does() {
        let pool = migrated_pool().await;
        let scratch = scratch("counts");

        crate::work_log::upsert(&pool, entry(1, "one"))
            .await
            .expect("should store");
        crate::work_log::upsert(&pool, entry(1, "two"))
            .await
            .expect("should store");
        draft(&pool, &scratch, 1, "draft").await;

        assert_eq!(
            crate::work_log::count_for_account(&pool, 1)
                .await
                .expect("should count"),
            2
        );
        assert_eq!(
            crate::proposed::for_account(&pool, 1)
                .await
                .expect("should read")
                .len(),
            1
        );

        // And counting is not deleting.
        assert_eq!(titles(&pool).await.len(), 2);
    }

    #[test]
    fn says_when_there_is_no_such_integration() {
        // No request was made and no host answered, so the message must not
        // claim one did.
        let error = known("slack").expect_err("slack is not an integration");

        assert_eq!(error.to_string(), "there is no integration called 'slack'");
    }

    #[test]
    fn accepts_the_services_it_has() {
        assert!(known(GITHUB).is_ok());
        assert!(known(MICROSOFT).is_ok());
    }

    #[test]
    fn the_two_sign_in_shapes_are_told_apart_by_a_tag() {
        // The renderer switches on `kind`. Pinned here because the interface
        // cannot compile-check a shape that crosses the IPC boundary.
        let device = serde_json::to_value(Login::Device(DeviceLogin {
            user_code: "WDJB-MJHT".to_string(),
            verification_uri: "https://github.com/login/device".to_string(),
            expires_in: 900,
        }))
        .expect("should serialize");

        assert_eq!(device["kind"], "device");
        assert_eq!(device["userCode"], "WDJB-MJHT");
        assert_eq!(device["expiresIn"], 900);

        let browser = serde_json::to_value(Login::Browser {
            url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?x=1".to_string(),
        })
        .expect("should serialize");

        assert_eq!(browser["kind"], "browser");
        assert!(
            browser["url"]
                .as_str()
                .expect("a url")
                .starts_with("https://login.microsoftonline.com/"),
            "the browser flow carries where to send them"
        );
    }

    #[test]
    fn an_expiry_is_written_as_an_instant_rather_than_a_duration() {
        // Graph says "3600 seconds"; the row has to hold when that is, or a
        // restart would read a duration measured from a moment nobody kept.
        let at = expires_at(3600);

        assert!(at.ends_with('Z'), "{at}");
        assert!(at.contains('T'), "{at}");
        assert!(
            chrono::DateTime::parse_from_rfc3339(&at).is_ok(),
            "should be a real instant: {at}"
        );
    }
}
