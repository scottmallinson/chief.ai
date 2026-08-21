//! The local inference engine.
//!
//! Chief ships llama.cpp's `llama-server` alongside its own binary and runs it
//! as a child process on loopback. That is the whole reason for this module:
//! nobody has to install a model runtime, the installer carries it, and the
//! process lives and dies with the app.
//!
//! It also removes two things the app used to have to worry about. The context
//! window is a launch flag rather than a per-request option, so no request can
//! make the engine reload the weights; and a server that owns one model keeps
//! it resident for its lifetime, so there is no keep-alive to negotiate and no
//! warm-up to schedule — `llama-server` warms itself as it starts.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;

use tauri::{AppHandle, Manager, Runtime};
use tokio::process::{Child, Command};

use crate::llama::{self, Health};
use crate::weights;

/// The name the loaded model answers to.
///
/// `llama-server` serves exactly one model, so this identifies rather than
/// chooses — but pinning it means requests do not have to know the file name of
/// whatever GGUF is on disk.
pub const MODEL_ALIAS: &str = "chief";

/// The context window, in tokens, fixed for the life of the server.
///
/// Room for a conversation plus a page of tool results. Larger costs memory for
/// the key/value cache and buys nothing Chief asks for.
const CONTEXT_SIZE: u32 = 4096;

/// Point Chief at a `llama-server` you are running yourself, instead of the one
/// it ships. Must still be loopback; the client refuses anything else.
const BASE_URL_VAR: &str = "CHIEF_LLAMA_BASE_URL";

/// Run a `llama-server` from somewhere other than beside the app binary. For
/// working on the engine itself without rebuilding the bundle.
const SERVER_VAR: &str = "CHIEF_LLAMA_SERVER";

/// How long to wait for a freshly spawned server to start answering before
/// giving up on it. Reading a couple of gigabytes off a cold disk is slow, and
/// the alternative to waiting is telling the user it failed when it did not.
const START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// How often to ask a starting server whether it is up yet.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// What can go wrong running the engine.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "Chief's inference engine is missing from this installation. Reinstalling should restore it."
    )]
    Missing,
    #[error("the model has not been downloaded yet.")]
    NoWeights,
    #[error("could not start the inference engine: {0}")]
    Spawn(String),
    #[error("the inference engine started but never began answering.")]
    NeverReady,
    #[error(
        "the inference engine stopped while loading the model. It may not fit in this machine's memory."
    )]
    Stopped,
    #[error(transparent)]
    Client(#[from] llama::Error),
    #[error("could not work out where Chief keeps its files: {0}")]
    Paths(String),
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// The executable's name on this platform.
fn server_file_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// The variable this platform's loader reads to find shared libraries.
///
/// llama.cpp ships its backends as separate libraries next to the server, so
/// wherever the bundle puts them has to be on this path or the engine will not
/// start.
fn library_path_variable() -> &'static str {
    if cfg!(target_os = "macos") {
        "DYLD_LIBRARY_PATH"
    } else if cfg!(windows) {
        "PATH"
    } else {
        "LD_LIBRARY_PATH"
    }
}

/// Put `directories` in front of whatever the loader already searches.
fn library_path(directories: &[PathBuf], inherited: Option<OsString>) -> OsString {
    let mut entries: Vec<OsString> = directories.iter().map(OsString::from).collect();

    if let Some(inherited) = inherited.filter(|value| !value.is_empty()) {
        entries.push(inherited);
    }

    std::env::join_paths(entries).unwrap_or_default()
}

/// The command line the server is started with.
///
/// Kept separate from the spawning so the flags Chief actually relies on —
/// loopback only, a fixed context window, and the Jinja chat templates that
/// tool calling needs — are covered by a test rather than by hoping.
fn arguments(weights: &Path, port: u16) -> Vec<OsString> {
    vec![
        OsString::from("--model"),
        weights.into(),
        OsString::from("--alias"),
        OsString::from(MODEL_ALIAS),
        // The engine must not be reachable from the network, only from here.
        OsString::from("--host"),
        OsString::from("127.0.0.1"),
        OsString::from("--port"),
        OsString::from(port.to_string()),
        OsString::from("--ctx-size"),
        OsString::from(CONTEXT_SIZE.to_string()),
        // Tool calling goes through the model's own chat template, which
        // llama.cpp only applies in Jinja mode.
        OsString::from("--jinja"),
    ]
}

/// The `llama-server` this installation should run, if there is one.
///
/// Tauri copies a bundled sidecar next to the app binary, which covers both a
/// release install and `tauri dev`. A `llama-server` on `PATH` is the last
/// resort, so someone who already has one can work on Chief without fetching a
/// second copy.
fn find_server() -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os(SERVER_VAR).map(PathBuf::from) {
        return configured.is_file().then_some(configured);
    }

    let beside_us = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(server_file_name())));

    if let Some(sidecar) = beside_us.filter(|path| path.is_file()) {
        return Some(sidecar);
    }

    // Nothing to resolve against a directory, so the loader searches `PATH`.
    on_path().then(|| PathBuf::from(server_file_name()))
}

/// Is there a `llama-server` on `PATH`?
fn on_path() -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };

    std::env::split_paths(&path).any(|dir| dir.join(server_file_name()).is_file())
}

/// Where this installation keeps the shared libraries the server loads.
///
/// The bundle puts them in the resource directory, which on Linux and Windows
/// is the directory the sidecar lands in as well — so on those two the loader
/// would find them unaided. macOS splits `Contents/MacOS` from
/// `Contents/Resources`, which is why the path is set explicitly rather than
/// left to the server's own `$ORIGIN`.
fn library_dirs<R: Runtime>(app: &AppHandle<R>, server: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(resources) = app.path().resource_dir() {
        candidates.push(resources.join("lib"));
        candidates.push(resources);
    }

    if let Some(beside_the_server) = server.and_then(Path::parent) {
        candidates.push(beside_the_server.join("lib"));
        candidates.push(beside_the_server.to_path_buf());
    }

    // A development build runs out of the source tree, where the libraries sit
    // where `scripts/fetch-llama-server.mjs` put them and nothing has been
    // bundled yet. Compiled in only for debug builds, so no release binary
    // carries a path from the machine that built it.
    #[cfg(debug_assertions)]
    candidates.push(PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/binaries/lib"
    )));

    candidates.retain(|dir| dir.is_dir());
    candidates.dedup();
    candidates
}

/// What became of a server this process started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChildState {
    /// Started by us and still running.
    Running,
    /// Started by us and gone.
    Exited,
    /// We never started one: an external server, or one that was already up.
    None,
}

/// A supervised `llama-server`, and everything needed to start one.
#[derive(Debug)]
pub struct Engine {
    base_url: String,
    server: Option<PathBuf>,
    library_dirs: Vec<PathBuf>,
    weights: PathBuf,
    /// Whether this process is the one that starts and stops the server. False
    /// when the user pointed Chief at their own, which is not ours to kill.
    owned: bool,
    child: Mutex<Option<Child>>,
}

impl Engine {
    /// Work out what this installation has to run and where it keeps things.
    pub fn discover<R: Runtime>(app: &AppHandle<R>) -> Result<Self, Error> {
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|error| Error::Paths(error.to_string()))?;

        let external = std::env::var(BASE_URL_VAR)
            .ok()
            .filter(|url| !url.is_empty());
        let owned = external.is_none();
        let base_url =
            external.unwrap_or_else(|| format!("http://127.0.0.1:{}", llama::DEFAULT_PORT));

        let server = owned.then(find_server).flatten();

        Ok(Self {
            base_url,
            library_dirs: library_dirs(app, server.as_deref()),
            server,
            weights: weights::path(&data_dir),
            owned,
            child: Mutex::new(None),
        })
    }

    /// Where the engine is, or will be, listening.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Where the model file belongs on this machine.
    pub fn weights(&self) -> &Path {
        &self.weights
    }

    /// Whether the weights have been downloaded.
    pub fn has_weights(&self) -> bool {
        self.weights.is_file()
    }

    /// Whether there is a `llama-server` for this process to run.
    ///
    /// True when the user is running their own: there is nothing missing from
    /// the installation, it is simply not ours to start.
    pub fn is_available(&self) -> bool {
        !self.owned || self.server.is_some()
    }

    /// Start the engine unless it is already answering.
    ///
    /// Safe to call as often as you like: a server that is up, one this process
    /// already started, and one the user is running themselves are all left
    /// alone.
    pub async fn ensure_running(&self, client: &llama::Client) -> Result<(), Error> {
        if !self.owned {
            return Ok(());
        }

        if client.health().await != Health::Down {
            return Ok(());
        }

        if self.child_state() == ChildState::Running {
            // Started, but not answering yet — it is still reading the weights.
            return Ok(());
        }

        if !self.has_weights() {
            return Err(Error::NoWeights);
        }

        let server = self.server.as_ref().ok_or(Error::Missing)?;
        let port = self.port();

        let mut command = Command::new(server);
        command
            .args(arguments(&self.weights, port))
            .stdin(Stdio::null())
            // Outlives a panic in this process only for as long as it takes the
            // runtime to reap it; the app also stops it explicitly on exit.
            .kill_on_drop(true);

        if !self.library_dirs.is_empty() {
            let variable = library_path_variable();
            command.env(
                variable,
                library_path(&self.library_dirs, std::env::var_os(variable)),
            );
        }

        #[cfg(windows)]
        {
            // Without this, a console window flashes up behind the app.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
        }

        let child = command
            .spawn()
            .map_err(|error| Error::Spawn(error.to_string()))?;

        if let Ok(mut slot) = self.child.lock() {
            *slot = Some(child);
        }

        Ok(())
    }

    /// Start the engine and wait until it can answer.
    ///
    /// Used where the caller is about to tell the user whether it worked, so
    /// "starting" is not a useful thing to report back.
    pub async fn start_and_wait(&self, client: &llama::Client) -> Result<(), Error> {
        self.ensure_running(client).await?;

        let deadline = std::time::Instant::now() + START_TIMEOUT;

        loop {
            if client.health().await == Health::Ready {
                return Ok(());
            }

            // A server we started that is no longer there is never going to
            // answer, and a model too big for the machine dies in seconds.
            // Waiting out the timeout would only delay saying so.
            if self.child_state() == ChildState::Exited {
                return Err(Error::Stopped);
            }

            if std::time::Instant::now() >= deadline {
                return Err(Error::NeverReady);
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Stop the server this process started. A server the user is running
    /// themselves is left alone.
    pub fn stop(&self) {
        let Ok(mut slot) = self.child.lock() else {
            return;
        };

        if let Some(child) = slot.as_mut() {
            // Sending the signal is all that can be done synchronously, and it
            // is all that is needed: the process is going away with us.
            let _ = child.start_kill();
        }

        *slot = None;
    }

    /// What became of the server this process started.
    ///
    /// Reading it clears a handle whose process has gone, so nothing tries to
    /// reuse it — which means [`ChildState::Exited`] is reported once, to
    /// whoever asks first.
    fn child_state(&self) -> ChildState {
        let Ok(mut slot) = self.child.lock() else {
            return ChildState::None;
        };

        match slot.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(None) => ChildState::Running,
                // Exited, or unknowable — either way there is nothing to reuse.
                Ok(Some(_)) | Err(_) => {
                    *slot = None;
                    ChildState::Exited
                }
            },
            None => ChildState::None,
        }
    }

    fn port(&self) -> u16 {
        reqwest::Url::parse(&self.base_url)
            .ok()
            .and_then(|url| url.port())
            .unwrap_or(llama::DEFAULT_PORT)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start the engine in the background, so the window does not wait for a
/// couple of gigabytes to come off disk.
///
/// Failure is expected and reported rather than raised: on a fresh machine the
/// model has not been downloaded yet, which is what the setup screen is for.
pub fn start<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();

    tauri::async_runtime::spawn(async move {
        let engine = app.state::<Engine>();
        let client = app.state::<llama::Client>();

        if let Err(error) = engine.ensure_running(client.inner()).await {
            eprintln!("the inference engine did not start: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_engine_on_loopback() {
        let arguments = arguments(Path::new("/models/model.gguf"), 11435);
        let rendered: Vec<String> = arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();

        let host = rendered
            .iter()
            .position(|argument| argument == "--host")
            .expect("the host should be pinned");
        assert_eq!(rendered[host + 1], "127.0.0.1");
    }

    #[test]
    fn fixes_the_context_window_when_the_server_starts() {
        let arguments = arguments(Path::new("/models/model.gguf"), 11435);
        let rendered: Vec<String> = arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();

        let size = rendered
            .iter()
            .position(|argument| argument == "--ctx-size")
            .expect("the context size should be set");
        assert_eq!(rendered[size + 1], CONTEXT_SIZE.to_string());
    }

    #[test]
    fn asks_for_the_templates_that_tool_calling_needs() {
        let arguments = arguments(Path::new("/models/model.gguf"), 11435);

        assert!(
            arguments.iter().any(|argument| argument == "--jinja"),
            "without --jinja the model cannot call tools: {arguments:?}"
        );
    }

    #[test]
    fn serves_the_model_under_the_name_requests_use() {
        let arguments = arguments(Path::new("/models/model.gguf"), 11435);
        let rendered: Vec<String> = arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();

        let alias = rendered
            .iter()
            .position(|argument| argument == "--alias")
            .expect("the model should be aliased");
        assert_eq!(rendered[alias + 1], MODEL_ALIAS);

        let model = rendered
            .iter()
            .position(|argument| argument == "--model")
            .expect("the weights should be named");
        assert_eq!(rendered[model + 1], "/models/model.gguf");
    }

    #[test]
    fn starts_the_server_on_the_port_the_client_will_use() {
        let arguments = arguments(Path::new("/models/model.gguf"), 4242);
        let rendered: Vec<String> = arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();

        let port = rendered
            .iter()
            .position(|argument| argument == "--port")
            .expect("the port should be set");
        assert_eq!(rendered[port + 1], "4242");
    }

    #[test]
    fn looks_in_the_bundles_own_directories_before_the_systems() {
        let ours = vec![PathBuf::from("/opt/chief/lib"), PathBuf::from("/opt/chief")];
        let combined = library_path(&ours, Some(OsString::from("/usr/lib")));
        let searched: Vec<PathBuf> = std::env::split_paths(&combined).collect();

        assert_eq!(
            searched,
            [
                PathBuf::from("/opt/chief/lib"),
                PathBuf::from("/opt/chief"),
                PathBuf::from("/usr/lib"),
            ]
        );
    }

    #[test]
    fn copes_with_a_loader_path_that_is_not_set() {
        let ours = vec![PathBuf::from("/opt/chief/lib")];
        let combined = library_path(&ours, None);
        let searched: Vec<PathBuf> = std::env::split_paths(&combined).collect();

        assert_eq!(searched, [PathBuf::from("/opt/chief/lib")]);
    }

    #[test]
    fn nothing_was_started_before_anything_starts_it() {
        let engine = Engine {
            base_url: "http://127.0.0.1:11435".to_string(),
            server: None,
            library_dirs: Vec::new(),
            weights: PathBuf::from("/models/model.gguf"),
            owned: true,
            child: Mutex::new(None),
        };

        assert_eq!(engine.child_state(), ChildState::None);
    }

    #[test]
    fn reads_the_port_out_of_the_address_the_client_will_use() {
        let engine = Engine {
            base_url: "http://127.0.0.1:8080".to_string(),
            server: None,
            library_dirs: Vec::new(),
            weights: PathBuf::from("/models/model.gguf"),
            owned: false,
            child: Mutex::new(None),
        };

        assert_eq!(engine.port(), 8080);
    }

    #[test]
    fn falls_back_to_the_default_port_when_the_address_names_none() {
        let engine = Engine {
            base_url: "http://localhost".to_string(),
            server: None,
            library_dirs: Vec::new(),
            weights: PathBuf::from("/models/model.gguf"),
            owned: false,
            child: Mutex::new(None),
        };

        assert_eq!(engine.port(), llama::DEFAULT_PORT);
    }

    #[test]
    fn names_the_executable_the_way_this_platform_does() {
        let name = server_file_name();

        if cfg!(windows) {
            assert_eq!(name, "llama-server.exe");
        } else {
            assert_eq!(name, "llama-server");
        }
    }
}
