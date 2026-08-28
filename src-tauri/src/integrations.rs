//! Credentials for the accounts the user has connected.
//!
//! One row per *account*, not per service, so a work and a personal mailbox can
//! both be connected. Tokens live here on this machine and are never sent
//! anywhere except to the service they belong to.
//!
//! Every read and write of a credential goes through this module. That is the
//! seam for moving secrets to the OS keychain later: one implementation
//! changes, and no provider has to know.

use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::Error;

/// The services Chief knows how to connect.
pub const GITHUB: &str = "github";

/// Outlook mail and calendar, through Microsoft Graph.
pub const MICROSOFT: &str = "microsoft";

/// How a credential was obtained, so routing is explicit rather than inferred
/// from which columns happen to be NULL.
pub const OAUTH: &str = "oauth";

/// A connected account, as the settings screen sees it. Carries no secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: i64,
    pub service: String,
    /// The provider's stable identifier for this account.
    pub account_key: String,
    /// What the user called it, if they named it.
    pub label: Option<String>,
    /// What to show when there is no label: a login, an address, a site.
    pub identity: Option<String>,
    /// When the credential was stored, ISO-8601.
    pub connected_at: String,
}

/// A credential to store.
#[derive(Debug, Clone)]
pub struct NewAccount<'a> {
    pub service: &'a str,
    pub account_key: &'a str,
    pub identity: Option<&'a str>,
    pub credential_kind: &'a str,
    pub access_token: &'a str,
    pub refresh_token: Option<&'a str>,
    pub expires_at: Option<&'a str>,
    pub scopes: Option<&'a str>,
    pub client_id: Option<&'a str>,
    pub client_secret: Option<&'a str>,
}

/// What is stored for a connected account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub id: i64,
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// ISO-8601, or `None` for a token that does not expire.
    pub expires_at: Option<String>,
    /// `None` when the registration is Chief's own.
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
}

/// The columns that make up an [`Account`], so every query agrees.
const ACCOUNT_COLUMNS: &str = "id, service, account_key, label, identity, created_at";

type AccountRow = (i64, String, String, Option<String>, Option<String>, String);

fn account_from(row: AccountRow) -> Account {
    let (id, service, account_key, label, identity, connected_at) = row;

    Account {
        id,
        service,
        account_key,
        label,
        identity,
        connected_at,
    }
}

/// Claim the row an upgrade left behind, so a real login lands on it.
///
/// Migration v3 had no provider identifier to seed `account_key` with, so it
/// wrote the service name as a placeholder and attributed every existing work
/// log entry to that row. A genuine sign-in supplies a login, which never
/// matches — so without this the upsert below inserts a *second* row, strands
/// the migration's backfill on the first, and the daemon logs the whole history
/// again under the new id.
///
/// Renaming the key is all it takes: the upsert then collides with this row and
/// replaces its credential in place, keeping the id the work log points at.
///
/// Two conditions keep it from taking a row that is not a leftover. The service
/// must have exactly one account, because the placeholder only ever exists
/// alone — a second account means the first sign-in already adopted it. And a
/// row that names itself is a real connection whose key happens to equal the
/// service name: only a genuine sign-in stores an identity matching the key,
/// where the migration could store neither.
///
/// # This is a heuristic, and it can reattribute someone else's work
///
/// Nothing in the schema says "this row came from a migration". So the three
/// things above — `account_key` equal to the service name, no identity that
/// matches that key, and a single account for the service — are the whole test,
/// and none of them is *whose* credential the leftover held. The migration
/// could not record that: it is pure SQL, and the login is only knowable by
/// asking the provider.
///
/// The consequence is silent. Upgrade from v2, then sign in as a **different**
/// GitHub login from the one the old credential held, and this statement
/// relabels the migrated row to the new login rather than splitting the two
/// apart. Every work log entry migration v3 attributed to that row is now
/// attributed to a person who did not do that work, and nothing on screen says
/// so.
///
/// That was chosen, not overlooked, and it stands. The alternative is to refuse
/// adoption, which is worse for far more users: the upsert then inserts a
/// second row, the backfilled `work_logs.account_id` values are stranded on an
/// account no provider will ever match again, and the daemon re-logs the user's
/// entire merge history under the new id. A duplicated work log on every single
/// upgrade is a heavier cost than a misattributed one for the rarer user who
/// came back as somebody else.
///
/// The honest close is not a cleverer heuristic — it is a column. Had migration
/// v3 written a flag marking the row as its own, adoption would be decided
/// rather than inferred, and the two conditions above could go. Even that does
/// not recover the old login, so telling "the same person reconnecting" from "a
/// different account" would additionally need the identity captured *before*
/// the upgrade, while the old credential could still be asked about itself.
/// Neither can be added to a database that has already migrated, which is why
/// this heuristic exists at all.
const ADOPT_PLACEHOLDER: &str = "\
UPDATE integration_accounts SET account_key = ?2
 WHERE service = ?1
   AND account_key = ?1
   AND (identity IS NULL OR identity <> account_key)
   AND (SELECT count(*) FROM integration_accounts WHERE service = ?1) = 1";

/// Store a credential, replacing any previous one for the same account.
///
/// Adoption and the write are one transaction, so a daemon pass reading
/// accounts alongside cannot see the moment between them.
pub async fn save(pool: &SqlitePool, account: NewAccount<'_>) -> Result<Account, Error> {
    let mut transaction = pool.begin().await?;

    sqlx::query(ADOPT_PLACEHOLDER)
        .bind(account.service)
        .bind(account.account_key)
        .execute(&mut *transaction)
        .await?;

    let row = sqlx::query_as::<_, AccountRow>(&format!(
        "INSERT INTO integration_accounts
             (service, account_key, identity, credential_kind, access_token,
              refresh_token, expires_at, scopes, client_id, client_secret)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (service, account_key) DO UPDATE SET
             identity      = COALESCE(excluded.identity, integration_accounts.identity),
             credential_kind = excluded.credential_kind,
             access_token  = excluded.access_token,
             refresh_token = excluded.refresh_token,
             expires_at    = excluded.expires_at,
             scopes        = excluded.scopes,
             client_id     = excluded.client_id,
             client_secret = excluded.client_secret,
             created_at    = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         RETURNING {ACCOUNT_COLUMNS}"
    ))
    .bind(account.service)
    .bind(account.account_key)
    .bind(account.identity)
    .bind(account.credential_kind)
    .bind(account.access_token)
    .bind(account.refresh_token)
    .bind(account.expires_at)
    .bind(account.scopes)
    .bind(account.client_id)
    .bind(account.client_secret)
    .fetch_one(&mut *transaction)
    .await?;

    transaction.commit().await?;

    Ok(account_from(row))
}

/// Every account connected for one service, oldest first.
pub async fn accounts(pool: &SqlitePool, service: &str) -> Result<Vec<Account>, Error> {
    let rows = sqlx::query_as::<_, AccountRow>(&format!(
        "SELECT {ACCOUNT_COLUMNS} FROM integration_accounts
          WHERE service = ?1 ORDER BY id"
    ))
    .bind(service)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(account_from).collect())
}

/// Every connected account, whatever the service.
pub async fn all_accounts(pool: &SqlitePool) -> Result<Vec<Account>, Error> {
    let rows = sqlx::query_as::<_, AccountRow>(&format!(
        "SELECT {ACCOUNT_COLUMNS} FROM integration_accounts ORDER BY service, id"
    ))
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(account_from).collect())
}

/// The stored secrets for one account.
pub async fn credentials(pool: &SqlitePool, id: i64) -> Result<Option<Credentials>, Error> {
    let row = sqlx::query_as::<
        _,
        (
            i64,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
    >(
        "SELECT id, access_token, refresh_token, expires_at, client_id, client_secret
           FROM integration_accounts WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(id, access_token, refresh_token, expires_at, client_id, client_secret)| Credentials {
            id,
            access_token,
            refresh_token,
            expires_at,
            client_id,
            client_secret,
        },
    ))
}

/// Replace an account's tokens after a renewal, leaving everything else alone.
pub async fn store_tokens(
    pool: &SqlitePool,
    id: i64,
    access_token: &str,
    refresh_token: Option<&str>,
    expires_at: Option<&str>,
) -> Result<(), Error> {
    sqlx::query(
        "UPDATE integration_accounts
            SET access_token = ?2, refresh_token = ?3, expires_at = ?4
          WHERE id = ?1",
    )
    .bind(id)
    .bind(access_token)
    .bind(refresh_token)
    .bind(expires_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// Name an account, or clear the name.
pub async fn set_label(pool: &SqlitePool, id: i64, label: Option<&str>) -> Result<(), Error> {
    sqlx::query("UPDATE integration_accounts SET label = ?2 WHERE id = ?1")
        .bind(id)
        .bind(label)
        .execute(pool)
        .await?;

    Ok(())
}

/// Record who an account belongs to, once the provider has told us.
///
/// The row migrated from v1 has no identity, because the old table never stored
/// one. It is filled in on the next successful read rather than by asking the
/// user to reconnect.
pub async fn set_identity(pool: &SqlitePool, id: i64, identity: &str) -> Result<(), Error> {
    sqlx::query("UPDATE integration_accounts SET identity = ?2 WHERE id = ?1")
        .bind(id)
        .bind(identity)
        .execute(pool)
        .await?;

    Ok(())
}

/// Forget one account's credential.
pub async fn forget(pool: &SqlitePool, id: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM integration_accounts WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::{migrated_pool, pool_at_version, upgrade};

    fn github_account<'a>(account_key: &'a str, access_token: &'a str) -> NewAccount<'a> {
        NewAccount {
            service: GITHUB,
            account_key,
            identity: Some("octocat@example.com"),
            credential_kind: OAUTH,
            access_token,
            refresh_token: None,
            expires_at: None,
            scopes: Some("repo read:user"),
            client_id: None,
            client_secret: None,
        }
    }

    #[tokio::test]
    async fn stores_and_reads_back_a_credential() {
        let pool = migrated_pool().await;

        let account = save(&pool, github_account("octocat", "gho_first"))
            .await
            .expect("should save");

        let stored = credentials(&pool, account.id)
            .await
            .expect("should read")
            .expect("should be connected");

        assert_eq!(stored.access_token, "gho_first");
        assert_eq!(account.service, GITHUB);
        assert_eq!(account.identity.as_deref(), Some("octocat@example.com"));
    }

    #[tokio::test]
    async fn holds_a_work_and_a_personal_account_side_by_side() {
        let pool = migrated_pool().await;

        save(&pool, github_account("octocat", "gho_personal"))
            .await
            .expect("should save the first");
        save(&pool, github_account("hubot", "gho_work"))
            .await
            .expect("should save the second");

        let accounts = accounts(&pool, GITHUB).await.expect("should read");

        assert_eq!(accounts.len(), 2, "both accounts should be kept");
    }

    #[tokio::test]
    async fn a_credential_that_says_nothing_does_not_erase_the_name() {
        // Identity is the one column the upsert does not overwrite
        // unconditionally, and nothing said so. A caller that stores a
        // credential without knowing who it belongs to — a token the user
        // pasted in, a provider that names an account only at sign-in — must
        // not blank the name the settings screen is showing.
        let pool = migrated_pool().await;

        save(&pool, github_account("octocat", "gho_first"))
            .await
            .expect("should save");

        let account = save(
            &pool,
            NewAccount {
                identity: None,
                ..github_account("octocat", "gho_second")
            },
        )
        .await
        .expect("should save again");

        assert_eq!(
            account.identity.as_deref(),
            Some("octocat@example.com"),
            "a credential with no identity should keep the stored one"
        );
    }

    #[tokio::test]
    async fn reconnecting_the_same_account_replaces_its_credential() {
        let pool = migrated_pool().await;

        save(&pool, github_account("octocat", "gho_first"))
            .await
            .expect("should save");
        let account = save(&pool, github_account("octocat", "gho_second"))
            .await
            .expect("should save again");

        let stored = credentials(&pool, account.id)
            .await
            .expect("should read")
            .expect("should be connected");

        assert_eq!(stored.access_token, "gho_second");
        assert_eq!(
            accounts(&pool, GITHUB).await.expect("should read").len(),
            1,
            "reconnecting should not add a second row"
        );
    }

    /// An upgraded database with one credential and one entry logged against
    /// it, exactly as migration v3 leaves them: the account carries the service
    /// name as a placeholder key, because the old table never stored a login.
    async fn upgraded_from_v2() -> SqlitePool {
        let pool = pool_at_version(2).await;

        sqlx::query(
            "INSERT INTO integrations (service_name, access_token, refresh_token)
             VALUES ('github', 'gho_old', 'ghr_old')",
        )
        .execute(&pool)
        .await
        .expect("a credential should be storable before the upgrade");
        sqlx::query(
            "INSERT INTO work_logs (source, content, external_id)
             VALUES ('github', 'Merged PR #4', 'owner/repo#4')",
        )
        .execute(&pool)
        .await
        .expect("an entry should be storable before the upgrade");

        upgrade(&pool).await;

        pool
    }

    #[tokio::test]
    async fn adopts_the_placeholder_an_upgrade_left_behind() {
        // The seam migration v3 documents and cannot close itself. Signing in
        // supplies the login the old table never had, which never matches the
        // placeholder — so without adoption the credential lands in a *second*
        // row, and every entry the migration attributed to the first is
        // stranded on an account nothing reads. The daemon then logs all of it
        // again, under the new id.
        let pool = upgraded_from_v2().await;

        let account = save(&pool, github_account("octocat", "gho_new"))
            .await
            .expect("should save");

        let accounts = accounts(&pool, GITHUB).await.expect("should read");
        assert_eq!(
            accounts.len(),
            1,
            "signing in should adopt the migrated row, not insert beside it"
        );
        assert_eq!(accounts[0].account_key, "octocat", "the key should be real");
        assert_eq!(accounts[0].identity.as_deref(), Some("octocat@example.com"));

        let attributed: i64 = sqlx::query_scalar("SELECT account_id FROM work_logs")
            .fetch_one(&pool)
            .await
            .expect("the migrated entry should still be there");
        assert_eq!(
            attributed, account.id,
            "the entry the migration attributed should still name this account"
        );

        let stored = credentials(&pool, account.id)
            .await
            .expect("should read")
            .expect("should be connected");
        assert_eq!(stored.access_token, "gho_new", "the credential is replaced");
    }

    #[tokio::test]
    async fn adopts_a_placeholder_that_a_pass_has_already_named() {
        // The daemon fills in the identity of a migrated account on its first
        // read, so the row an upgrade left is usually named by the time the
        // user reconnects. It is still a placeholder: the *key* is what the
        // constraint matches on, and it is still the service name.
        let pool = upgraded_from_v2().await;
        let migrated = accounts(&pool, GITHUB).await.expect("should read")[0].id;

        set_identity(&pool, migrated, "octocat")
            .await
            .expect("a pass should be able to name it");

        let account = save(&pool, github_account("octocat", "gho_new"))
            .await
            .expect("should save");

        assert_eq!(account.id, migrated, "the same row should be adopted");
        assert_eq!(
            accounts(&pool, GITHUB).await.expect("should read").len(),
            1,
            "a named placeholder is still a placeholder"
        );
    }

    #[tokio::test]
    async fn leaves_a_real_account_alone_when_its_key_looks_like_a_placeholder() {
        // Nothing stops a provider issuing an account key that happens to be
        // the service's own name, and adopting *that* row would hand one
        // person's log and credential to another account. A genuine connection
        // says who it is, and says the same thing twice — key and identity —
        // where the migration could say neither.
        let pool = migrated_pool().await;
        let real = save(
            &pool,
            NewAccount {
                identity: Some("github"),
                ..github_account("github", "gho_theirs")
            },
        )
        .await
        .expect("should save");

        let other = save(&pool, github_account("octocat", "gho_mine"))
            .await
            .expect("should save");

        assert_ne!(other.id, real.id, "a second account is a second row");
        assert_eq!(accounts(&pool, GITHUB).await.expect("should read").len(), 2);
        assert_eq!(
            credentials(&pool, real.id)
                .await
                .expect("should read")
                .expect("should be connected")
                .access_token,
            "gho_theirs",
            "the account already there should keep its credential"
        );
    }

    #[tokio::test]
    async fn reports_nothing_when_a_service_has_no_accounts() {
        let pool = migrated_pool().await;

        assert!(accounts(&pool, GITHUB)
            .await
            .expect("should read")
            .is_empty());
    }

    #[tokio::test]
    async fn timestamps_an_account_when_it_is_connected() {
        let pool = migrated_pool().await;

        let account = save(&pool, github_account("octocat", "gho_token"))
            .await
            .expect("should save");

        assert!(
            account.connected_at.contains('T') && account.connected_at.ends_with('Z'),
            "expected an ISO-8601 timestamp, got {}",
            account.connected_at
        );
    }

    #[tokio::test]
    async fn stores_renewed_tokens_without_disturbing_the_account() {
        let pool = migrated_pool().await;
        let account = save(&pool, github_account("octocat", "gho_old"))
            .await
            .expect("should save");

        store_tokens(
            &pool,
            account.id,
            "gho_new",
            Some("ghr_new"),
            Some("2026-08-22T12:00:00.000Z"),
        )
        .await
        .expect("should store renewed tokens");

        let stored = credentials(&pool, account.id)
            .await
            .expect("should read")
            .expect("should be connected");

        assert_eq!(stored.access_token, "gho_new");
        assert_eq!(stored.refresh_token.as_deref(), Some("ghr_new"));
        assert_eq!(
            stored.expires_at.as_deref(),
            Some("2026-08-22T12:00:00.000Z")
        );
    }

    #[tokio::test]
    async fn names_an_account_and_backfills_its_identity() {
        let pool = migrated_pool().await;
        let account = save(
            &pool,
            NewAccount {
                identity: None,
                ..github_account("octocat", "gho_token")
            },
        )
        .await
        .expect("should save");

        set_label(&pool, account.id, Some("Work"))
            .await
            .expect("should label");
        set_identity(&pool, account.id, "octocat")
            .await
            .expect("should backfill the identity");

        let named = accounts(&pool, GITHUB)
            .await
            .expect("should read")
            .into_iter()
            .next()
            .expect("the account should be there");

        assert_eq!(named.label.as_deref(), Some("Work"));
        assert_eq!(named.identity.as_deref(), Some("octocat"));
    }

    #[tokio::test]
    async fn forgetting_removes_only_that_account() {
        let pool = migrated_pool().await;
        let personal = save(&pool, github_account("octocat", "gho_personal"))
            .await
            .expect("should save");
        save(&pool, github_account("hubot", "gho_work"))
            .await
            .expect("should save");

        forget(&pool, personal.id).await.expect("should forget");

        let left = accounts(&pool, GITHUB).await.expect("should read");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].account_key, "hubot");
        assert!(credentials(&pool, personal.id)
            .await
            .expect("should read")
            .is_none());
    }
}
