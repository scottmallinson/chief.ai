//! Connecting and disconnecting the services Chief reads from.
//!
//! Sign-in happens in two commands so the user can see their code while we
//! wait: `start_github_login` returns the code to type, `finish_github_login`
//! blocks until they have typed it. The device code itself never reaches the
//! renderer — it stays in backend state.

use tauri::{AppHandle, Runtime, State};
use tokio::sync::Mutex;

use crate::db;
use crate::github::{self, Client, DeviceLogin, PendingLogin};
use crate::integrations::{self, Connection};

/// The sign-in waiting to be completed, if any.
#[derive(Default)]
pub struct Pending(Mutex<Option<PendingLogin>>);

/// Begin signing in to GitHub and return the code the user must enter.
#[tauri::command]
pub async fn start_github_login(
    client: State<'_, Client>,
    pending: State<'_, Pending>,
) -> Result<DeviceLogin, github::Error> {
    let client_id = github::client_id()?;
    let started = client.start_login(&client_id).await?;
    let login = started.login.clone();

    *pending.0.lock().await = Some(started);

    Ok(login)
}

/// Wait for the user to finish in the browser, then store the token.
#[tauri::command]
pub async fn finish_github_login<R: Runtime>(
    app: AppHandle<R>,
    client: State<'_, Client>,
    pending: State<'_, Pending>,
) -> Result<Connection, github::Error> {
    let started = pending
        .0
        .lock()
        .await
        .take()
        .ok_or(github::Error::NotConnected)?;

    let client_id = github::client_id()?;
    let (access_token, refresh_token) = client.finish_login(&client_id, &started).await?;

    let pool = db::pool(&app).await?;
    integrations::save(
        &pool,
        integrations::GITHUB,
        &access_token,
        refresh_token.as_deref(),
    )
    .await?;

    Ok(integrations::status(&pool, integrations::GITHUB).await?)
}

/// Whether GitHub is connected, and since when.
#[tauri::command]
pub async fn github_connection<R: Runtime>(app: AppHandle<R>) -> Result<Connection, github::Error> {
    let pool = db::pool(&app).await?;

    Ok(integrations::status(&pool, integrations::GITHUB).await?)
}

/// Forget the stored GitHub credential.
#[tauri::command]
pub async fn disconnect_github<R: Runtime>(app: AppHandle<R>) -> Result<Connection, github::Error> {
    let pool = db::pool(&app).await?;
    integrations::forget(&pool, integrations::GITHUB).await?;

    Ok(integrations::status(&pool, integrations::GITHUB).await?)
}
