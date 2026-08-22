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

/// Reject a service Chief does not know, rather than failing later and less
/// clearly.
fn known(service: &str) -> Result<(), github::Error> {
    if service == Client::SERVICE {
        return Ok(());
    }

    Err(github::Error::Status {
        status: 400,
        body: format!("there is no integration called '{service}'"),
    })
}

/// Begin signing in and return what the user must enter.
#[tauri::command]
pub async fn start_login(
    service: String,
    client: State<'_, Client>,
    pending: State<'_, Pending>,
) -> Result<DeviceLogin, github::Error> {
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
) -> Result<Vec<Account>, github::Error> {
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
pub async fn connections<R: Runtime>(app: AppHandle<R>) -> Result<Vec<Account>, github::Error> {
    let pool = db::pool(&app).await?;

    Ok(integrations::all_accounts(&pool).await?)
}

/// Forget one account's credential.
#[tauri::command]
pub async fn disconnect<R: Runtime>(
    account_id: i64,
    app: AppHandle<R>,
) -> Result<Vec<Account>, github::Error> {
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
) -> Result<Vec<Account>, github::Error> {
    let pool = db::pool(&app).await?;
    integrations::set_label(&pool, account_id, label.as_deref()).await?;

    Ok(integrations::all_accounts(&pool).await?)
}
