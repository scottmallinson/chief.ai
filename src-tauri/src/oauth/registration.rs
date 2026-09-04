//! Which OAuth registration a service signs in with.
//!
//! Every OAuth flow needs a client id. It is **not** a secret — a device-flow
//! id and a public-client id are both public by design — but it does have to
//! exist, and there is no dynamic registration on either GitHub or Entra to
//! mint one on the spot. So the only question is where the id comes from, and
//! Chief has to answer it for three different people:
//!
//! - **Somebody who installed a release** and wants sign-in to work with
//!   nothing to configure. The release bakes Chief's own id in.
//! - **An organisation** that would rather sign in against its own OAuth app
//!   or its own Entra registration than approve Chief's. It pastes that id in
//!   Settings, and it is kept here.
//! - **A developer running from source**, who exports the environment
//!   variable and expects that to win for the run they are doing.
//!
//! Hence three layers, resolved in that order of deliberateness: the
//! environment beats what is stored, and what is stored beats what was built
//! in. The stored one lives in `settings` rather than `integrations`, because
//! it is a fact about *how to sign in* and it has to be readable before any
//! account exists — which is exactly the case a build with no id at all is in.
//!
//! **The id is shown back to the user.** A credential Chief hides — a Linear
//! key, a calendar address — is hidden because holding it grants access. A
//! client id grants nothing on its own, and the user cannot tell which
//! registration they are signing in against unless Chief says.

use sqlx::SqlitePool;

use crate::db;
use crate::settings;

/// How long a pasted id may be. Nothing legitimate is close: a GitHub OAuth
/// client id is around 20 characters and an Entra one is a 36-character GUID.
const MAX_LENGTH: usize = 200;

/// Where the id in use came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// This machine's environment, which beats everything else.
    Environment,
    /// One the user pasted into Settings.
    Stored,
    /// The one the release was compiled with.
    BuiltIn,
    /// There is none, so this service cannot be signed into at all.
    Missing,
}

/// One service's sign-in registration, as the settings screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Registration {
    pub service: String,
    /// The id that would be used. Public by design; see the module docs.
    pub client_id: Option<String>,
    pub source: Source,
    /// Whether there is a built-in id to fall back to, so the interface knows
    /// whether clearing a stored one leaves the user with anything.
    pub has_built_in: bool,
    /// Whether the environment is overriding, so a stored id that is being
    /// ignored is not presented as the one in use.
    pub overridden_by_environment: bool,
}

/// Why a pasted client id was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Invalid {
    #[error("a client id cannot be blank")]
    Blank,
    #[error("that is too long to be a client id")]
    TooLong,
    #[error(
        "a client id is letters, digits and '-', '.', '_' or '~'. Paste just the id, not a URL or \
         a whole line of configuration."
    )]
    Characters,
}

/// Where a service's own client id is kept.
fn key(service: &str) -> String {
    format!("oauth.{service}.client_id")
}

/// Check a pasted id before it is stored, and return it trimmed.
///
/// **This is a URL safety check, not a formatting nicety.** The id is
/// interpolated into an authorize URL and posted to a token endpoint, so a
/// value carrying `&`, a space or a newline could add parameters to a request
/// the user never made. The permitted set is the unreserved characters of RFC
/// 3986, which covers every id GitHub and Entra issue.
pub fn check(client_id: &str) -> Result<String, Invalid> {
    let trimmed = client_id.trim();

    if trimmed.is_empty() {
        return Err(Invalid::Blank);
    }

    if trimmed.len() > MAX_LENGTH {
        return Err(Invalid::TooLong);
    }

    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'))
    {
        return Err(Invalid::Characters);
    }

    Ok(trimmed.to_string())
}

/// What the user stored for `service`, if anything.
///
/// A blank row reads as nothing stored rather than as an empty id, so a value
/// that somehow got emptied falls back to the built-in one instead of taking
/// sign-in down.
pub async fn stored(pool: &SqlitePool, service: &str) -> Result<Option<String>, db::Error> {
    Ok(settings::get(pool, &key(service))
        .await?
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty()))
}

/// Keep `client_id` as the registration `service` signs in with.
pub async fn store(pool: &SqlitePool, service: &str, client_id: &str) -> Result<(), db::Error> {
    Ok(settings::set(pool, &key(service), client_id).await?)
}

/// Forget the stored id, so the built-in one is used again.
pub async fn forget(pool: &SqlitePool, service: &str) -> Result<(), db::Error> {
    Ok(settings::clear(pool, &key(service)).await?)
}

/// The registration `service` would sign in with, and where it came from.
///
/// The two candidates are passed in rather than read here, because only the
/// provider's own module knows the name of its environment variable and what
/// its build baked in.
pub async fn resolve(
    pool: &SqlitePool,
    service: &str,
    from_environment: Option<String>,
    built_in: Option<String>,
) -> Result<Registration, db::Error> {
    let kept = stored(pool, service).await?;

    let (client_id, source) = match (from_environment, kept) {
        (Some(id), _) => (Some(id), Source::Environment),
        (None, Some(id)) => (Some(id), Source::Stored),
        (None, None) => match built_in.clone() {
            Some(id) => (Some(id), Source::BuiltIn),
            None => (None, Source::Missing),
        },
    };

    Ok(Registration {
        service: service.to_string(),
        client_id,
        overridden_by_environment: source == Source::Environment,
        source,
        has_built_in: built_in.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::{check, forget, resolve, store, stored, Invalid, Source};
    use crate::db::test_support::migrated_pool;

    const SERVICE: &str = "github";

    #[tokio::test]
    async fn falls_back_to_what_the_build_carries() {
        let pool = migrated_pool().await;

        let found = resolve(&pool, SERVICE, None, Some("built-in".into()))
            .await
            .expect("resolve");

        assert_eq!(found.client_id.as_deref(), Some("built-in"));
        assert_eq!(found.source, Source::BuiltIn);
        assert!(found.has_built_in);
    }

    #[tokio::test]
    async fn a_stored_id_is_used_instead_of_the_one_built_in() {
        let pool = migrated_pool().await;

        store(&pool, SERVICE, "theirs").await.expect("store");

        let found = resolve(&pool, SERVICE, None, Some("built-in".into()))
            .await
            .expect("resolve");

        assert_eq!(found.client_id.as_deref(), Some("theirs"));
        assert_eq!(found.source, Source::Stored);
    }

    #[tokio::test]
    async fn the_environment_beats_a_stored_id_and_says_so() {
        let pool = migrated_pool().await;

        store(&pool, SERVICE, "theirs").await.expect("store");

        let found = resolve(
            &pool,
            SERVICE,
            Some("from-env".into()),
            Some("built-in".into()),
        )
        .await
        .expect("resolve");

        assert_eq!(found.client_id.as_deref(), Some("from-env"));
        assert_eq!(found.source, Source::Environment);
        assert!(found.overridden_by_environment);
    }

    /// The state a release with no id compiled in is in — the defect REC-60
    /// is about. It has to be reportable, because the interface has to offer
    /// the field that fixes it.
    #[tokio::test]
    async fn a_build_with_nothing_anywhere_says_so_rather_than_guessing() {
        let pool = migrated_pool().await;

        let found = resolve(&pool, SERVICE, None, None).await.expect("resolve");

        assert_eq!(found.client_id, None);
        assert_eq!(found.source, Source::Missing);
        assert!(!found.has_built_in);
    }

    #[tokio::test]
    async fn forgetting_a_stored_id_returns_to_the_built_in_one() {
        let pool = migrated_pool().await;

        store(&pool, SERVICE, "theirs").await.expect("store");
        forget(&pool, SERVICE).await.expect("forget");

        let found = resolve(&pool, SERVICE, None, Some("built-in".into()))
            .await
            .expect("resolve");

        assert_eq!(found.client_id.as_deref(), Some("built-in"));
        assert_eq!(found.source, Source::BuiltIn);
    }

    #[tokio::test]
    async fn one_service_cannot_read_another_service_id() {
        let pool = migrated_pool().await;

        store(&pool, "github", "for-github").await.expect("store");

        assert_eq!(stored(&pool, "microsoft").await.expect("read"), None);
    }

    /// A row that has somehow been emptied is nothing stored, not an empty id:
    /// signing in with `client_id=` would fail at the provider with a message
    /// about the request rather than about this machine.
    #[tokio::test]
    async fn a_blank_row_reads_as_nothing_stored() {
        let pool = migrated_pool().await;

        crate::settings::set(&pool, "oauth.github.client_id", "   ")
            .await
            .expect("write");

        assert_eq!(stored(&pool, SERVICE).await.expect("read"), None);
    }

    /// The guard. A client id is interpolated into an authorize URL, so
    /// anything outside RFC 3986's unreserved set could add parameters to a
    /// request the user never made.
    ///
    /// Proved by accepting whatever was pasted:
    ///
    /// ```text
    /// assertion `left == right` failed: a client id goes into a URL
    ///   left: Ok("abc&scope=admin:org")
    ///  right: Err(Characters)
    /// ```
    #[test]
    fn refuses_anything_that_could_change_the_request_it_is_put_into() {
        for pasted in [
            "abc&scope=admin:org",
            "abc def",
            "abc\nclient_secret=x",
            "https://github.com/login?client_id=abc",
            "abc#fragment",
            "abc%2f",
        ] {
            assert_eq!(
                check(pasted),
                Err(Invalid::Characters),
                "a client id goes into a URL"
            );
        }
    }

    #[test]
    fn accepts_the_shapes_github_and_entra_actually_issue() {
        for pasted in [
            "Ov23liAbCdEf01234567",
            "Iv1.0123456789abcdef",
            "7f3c1a02-9d4e-4b71-88a1-0d9f2c3b4e56",
        ] {
            assert_eq!(check(pasted), Ok(pasted.to_string()));
        }
    }

    #[test]
    fn takes_the_whitespace_off_a_pasted_id() {
        assert_eq!(check("  Ov23liAbCdEf  \n"), Ok("Ov23liAbCdEf".to_string()));
    }

    #[test]
    fn refuses_a_blank_field_and_an_implausible_length() {
        assert_eq!(check("   "), Err(Invalid::Blank));
        assert_eq!(check(&"a".repeat(201)), Err(Invalid::TooLong));
    }
}
