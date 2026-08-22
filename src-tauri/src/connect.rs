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
use crate::github::{self, Client, DeviceLogin, PendingLogin};
use crate::integrations::{self, Account, NewAccount, OAUTH};
use crate::oauth::Provider;

/// The sign-in waiting to be completed, if any.
#[derive(Default)]
pub struct Pending(Mutex<Option<PendingLogin>>);

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
    if service == Client::SERVICE {
        return Ok(());
    }

    Err(Error::NoSuchService(service.to_string()))
}

/// Begin signing in and return what the user must enter.
#[tauri::command]
pub async fn start_login(
    service: String,
    client: State<'_, Client>,
    pending: State<'_, Pending>,
) -> Result<DeviceLogin, Error> {
    known(&service)?;

    let client_id = github::client_id()?;
    let started = client.start_login(&client_id).await?;
    let login = started.login.clone();

    *pending.0.lock().await = Some(started);

    Ok(login)
}

/// Wait for the user to finish in the browser, then store the credential.
#[tauri::command]
pub async fn finish_login<R: Runtime>(
    service: String,
    app: AppHandle<R>,
    client: State<'_, Client>,
    pending: State<'_, Pending>,
) -> Result<Vec<Account>, Error> {
    known(&service)?;

    let started = pending
        .0
        .lock()
        .await
        .take()
        .ok_or(github::Error::NotConnected)?;

    let client_id = github::client_id()?;
    let (access_token, refresh_token) = client.finish_login(&client_id, &started).await?;

    // Ask who this is before storing, so two accounts on one service do not
    // collide on a placeholder key.
    let login = client.viewer(&access_token).await?;

    let pool = db::pool(&app).await?;
    integrations::save(
        &pool,
        NewAccount {
            service: Client::SERVICE,
            account_key: &login,
            identity: Some(&login),
            credential_kind: OAUTH,
            access_token: &access_token,
            refresh_token: refresh_token.as_deref(),
            expires_at: None,
            scopes: Some(&client.scopes().join(" ")),
            client_id: None,
            client_secret: None,
        },
    )
    .await?;

    Ok(integrations::all_accounts(&pool).await?)
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
    fn accepts_the_service_it_has() {
        assert!(known(crate::integrations::GITHUB).is_ok());
    }
}
