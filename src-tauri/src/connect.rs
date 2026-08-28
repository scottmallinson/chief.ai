//! Connecting and disconnecting the accounts Chief reads from.
//!
//! Sign-in happens in two commands so the user can see their code while we
//! wait: `start_login` returns what to enter, `finish_login` blocks until they
//! have entered it. The device code itself never reaches the renderer — it
//! stays in backend state.
//!
//! Every command takes the service by name rather than being named after one,
//! so a new provider costs an arm in `dispatch` and no new commands.

use tauri::{AppHandle, Runtime, State};
use tokio::sync::Mutex;

use crate::db;
use crate::github::{self, Client, DeviceLogin};
use crate::integrations::{self, Account, NewAccount, GITHUB, MICROSOFT, OAUTH};
use crate::microsoft;
use crate::oauth::Provider;

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
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// Reject a service Chief does not know, rather than failing later and less
/// clearly.
fn known(service: &str) -> Result<(), Error> {
    if service == GITHUB || service == MICROSOFT {
        return Ok(());
    }

    Err(Error::NoSuchService(service.to_string()))
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

/// Forget one account's credential.
#[tauri::command]
pub async fn disconnect<R: Runtime>(
    account_id: i64,
    app: AppHandle<R>,
) -> Result<Vec<Account>, Error> {
    let pool = db::pool(&app).await?;
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
