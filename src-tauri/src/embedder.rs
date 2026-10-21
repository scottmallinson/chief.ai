//! The embedding server the router asks.
//!
//! EmbeddingGemma 2 places a question by meaning (see [`crate::router`]), and
//! `llama-server --embedding` serves it. It is a second process beside the
//! engine, on its own loopback port, and it is **optional in every direction**:
//! with no weights, no server binary, or a server that will not start, nothing
//! is registered here and [`crate::router::configured`] answers `None`, so the
//! tool loop runs exactly as it did before the router existed.
//!
//! Nothing about it is a second way off the machine. The weights are fetched
//! by [`crate::weights`] under the same rules as the main model's, the server
//! listens on `127.0.0.1` only, and the client that talks to it refuses any
//! address that is not loopback.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tauri::{AppHandle, Manager, Runtime};
use tokio::process::{Child, Command};

use crate::engine;
use crate::weights;

/// The port the embedding server listens on. The engine is on 11435.
pub const PORT: u16 = 11436;

/// Set to `off` to run without the router.
pub const OFF_VAR: &str = "CHIEF_ROUTER";

/// How long to wait for the server to answer `/health` after starting it.
const READY_WITHIN: Duration = Duration::from_secs(60);

/// Where the running server is, once it has answered. Set once per process.
static URL: OnceLock<String> = OnceLock::new();

/// The child this process started, so it can be stopped on exit.
static CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// The address of the embedding server, if one is up.
pub fn url() -> Option<&'static str> {
    URL.get().map(String::as_str)
}

/// Is the router switched off by name?
fn disabled(setting: Option<&OsString>) -> bool {
    setting.is_some_and(|value| value.to_string_lossy().eq_ignore_ascii_case("off"))
}

/// The flags the embedding server is started with.
///
/// The router embeds a sentence at a time, so a small window and one slot are
/// all it needs, and they are what the benchmark measured (169 MB median, 311
/// MB peak resident). `--host` is pinned to loopback.
pub(crate) fn arguments(weights: &Path, port: u16) -> Vec<OsString> {
    [
        "--model",
        &weights.to_string_lossy(),
        "--embedding",
        "--pooling",
        "mean",
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--ctx-size",
        "512",
        "--batch-size",
        "512",
        "--ubatch-size",
        "512",
        "--parallel",
        "1",
    ]
    .iter()
    .map(OsString::from)
    .collect()
}

/// Start the embedding server in the background, if there is one to start.
///
/// Called once at launch and again after the weights have been downloaded.
/// Idempotent: a second call while one is running does nothing.
pub fn start<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();

    tauri::async_runtime::spawn(async move {
        if let Err(reason) = bring_up(&app).await {
            // Expected on a fresh install and on a machine without the weights.
            eprintln!("the router is off: {reason}");
        }
    });
}

async fn bring_up<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if disabled(std::env::var_os(OFF_VAR).as_ref()) {
        return Err(format!("{OFF_VAR}=off"));
    }

    // Somebody pointed Chief at a server of their own; leave it to them.
    if std::env::var_os(crate::router::ROUTER_URL_VAR).is_some_and(|url| !url.is_empty()) {
        return Err(format!("{} is set", crate::router::ROUTER_URL_VAR));
    }

    if URL.get().is_some() {
        return Ok(());
    }

    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let weights_file = weights::EMBEDDER.path(&data_dir);
    if !weights_file.is_file() {
        return Err("the embedding weights have not been downloaded".to_string());
    }

    let server = engine::find_server().ok_or("no llama-server to run it")?;
    let library_dirs = engine::library_dirs(app, Some(&server));

    let mut command = Command::new(&server);
    command
        .args(arguments(&weights_file, PORT))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    if let Some(dir) = engine::backend_dir(&library_dirs) {
        command.current_dir(dir);
    }

    if !library_dirs.is_empty() {
        let variable = engine::library_path_variable();
        command.env(
            variable,
            engine::library_path(&library_dirs, std::env::var_os(variable)),
        );
    }

    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let child = command.spawn().map_err(|error| error.to_string())?;
    if let Ok(mut slot) = CHILD.lock() {
        *slot = Some(child);
    }

    let address = format!("http://127.0.0.1:{PORT}");
    let client = crate::llama::Client::with_base_url(&address).map_err(|e| e.to_string())?;

    let deadline = tokio::time::Instant::now() + READY_WITHIN;
    while tokio::time::Instant::now() < deadline {
        if client.health().await == crate::llama::Health::Ready {
            let _ = URL.set(address);
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    stop();
    Err("the embedding server did not answer in time".to_string())
}

/// Stop the server this process started.
pub fn stop() {
    if let Ok(mut slot) = CHILD.lock() {
        if let Some(child) = slot.as_mut() {
            let _ = child.start_kill();
        }
        *slot = None;
    }
}

/// Where the embedding weights belong, for the setup download.
#[must_use]
pub fn weights_path(data_dir: &Path) -> PathBuf {
    weights::EMBEDDER.path(data_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered() -> Vec<String> {
        arguments(Path::new("/models/e.gguf"), PORT)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn keeps_the_embedding_server_on_loopback() {
        let args = rendered();
        let host = args
            .iter()
            .position(|a| a == "--host")
            .expect("host is pinned");

        assert_eq!(args[host + 1], "127.0.0.1");
    }

    #[test]
    fn listens_on_a_port_the_engine_is_not_using() {
        let args = rendered();
        let port = args
            .iter()
            .position(|a| a == "--port")
            .expect("port is set");

        assert_eq!(args[port + 1], PORT.to_string());
        assert_ne!(PORT, crate::llama::DEFAULT_PORT);
    }

    #[test]
    fn serves_embeddings_rather_than_chat() {
        assert!(rendered().iter().any(|a| a == "--embedding"));
    }

    #[test]
    fn is_switched_off_by_name_and_by_nothing_else() {
        assert!(disabled(Some(&OsString::from("off"))));
        assert!(disabled(Some(&OsString::from("OFF"))));
        assert!(!disabled(Some(&OsString::from("on"))));
        assert!(!disabled(Some(&OsString::from(""))));
        assert!(!disabled(None));
    }
}
