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

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, Runtime};
use tokio::io::AsyncBufReadExt;
use tokio::process::{Child, ChildStderr, Command};
use tokio::task::JoinHandle;

use crate::llama::{self, Health};
use crate::probe::{Machine, Tier};
use crate::weights;

/// The name the loaded model answers to.
///
/// `llama-server` serves exactly one model, so this identifies rather than
/// chooses — but pinning it means requests do not have to know the file name of
/// whatever GGUF is on disk.
pub const MODEL_ALIAS: &str = "chief";

/// How far back the model can see. Tier-dependent rather than fixed: the window
/// costs memory in the KV cache, and the machine that needs the smaller model
/// needs the smaller window for the same reason. See [`crate::probe::Tier`].
///
/// This is the engine's window, not Chief's prompt budget.
///
/// The smallest run of tokens worth reusing from a cached prompt via KV
/// shifting. Too small and the engine spends more on bookkeeping than it saves;
/// 256 is the value llama.cpp's own guidance settles on.
const CACHE_REUSE_CHUNK: u32 = 256;

/// How long the engine may sit unused before its memory is given back.
///
/// A resident model holds a couple of gigabytes on a machine written to have
/// three or four spare, and an app left open all day is used for minutes of it.
/// Deliberately shorter than the work-log daemon's interval, so an idle machine
/// spends most of each half hour with the memory returned rather than none of
/// it — the daemon's own pass wakes the engine and is the reason it is measured
/// in minutes rather than seconds.
const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How often idleness is looked at. Cheap, so the granularity costs nothing.
const IDLE_CHECK: Duration = Duration::from_secs(60);

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

/// How many lines of the engine's own output to keep.
///
/// llama.cpp is chatty on the way up — backends, tensor counts, chat template
/// — and the server then runs for hours, so what is kept has to be a window
/// rather than a log. Twenty lines carries a dynamic-link failure, a flag the
/// build does not know, or a model file the loader rejected, with enough of
/// what came before it to read in context.
const TAIL_LINES: usize = 20;

/// How much of any one line to keep.
///
/// Nothing the loader writes is this long. A line that runs past it has no
/// newline in it for a reason — a progress meter redrawing itself with carriage
/// returns — and keeping the whole of one would make the count above pointless.
const TAIL_LINE_CHARS: usize = 500;

/// How long to wait for the last of a dead server's output before giving up on
/// it. Long enough for a pipe to drain, short enough that a pipe something
/// else is holding open cannot delay an error the user is waiting on.
const DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

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
    #[error("the inference engine stopped before it could answer. {0}")]
    Stopped(String),
    #[error(transparent)]
    Client(#[from] llama::Error),
    #[error("could not work out where Chief keeps its files: {0}")]
    Paths(String),
}

impl Error {
    /// What to say about a server that died on the way up.
    ///
    /// All that has been observed at this point is that the process is gone,
    /// so that is all the message asserts; the engine's own last words carry
    /// the diagnosis. This used to name a cause instead — that the model might
    /// not fit in this machine's memory — and the report that prompted the
    /// change was somebody on macOS 12 dutifully downloading a smaller model
    /// to fix a symbol missing from a system library. A guess written as a
    /// finding is worse than no answer: it sends the reader somewhere else.
    fn stopped(last_words: Option<String>) -> Self {
        Self::Stopped(match last_words {
            Some(words) => format!("It said:\n{words}"),
            // Silence is consistent with the machine killing it for its size,
            // and with a dozen other things. Offer that, do not claim it.
            None => "It wrote nothing on the way out, so there is nothing here \
                     that says why. Too little memory for the model would do \
                     that; so would an engine this machine cannot run at all."
                .to_string(),
        })
    }
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// The last few lines the engine wrote, and nothing older.
///
/// A supervised server writes for as long as it runs, so this is deliberately
/// a fixed-size window: it is here to explain a death, not to keep a log.
#[derive(Debug, Default)]
struct Tail(VecDeque<String>);

impl Tail {
    /// Keep a line, dropping the oldest once the window is full.
    fn push(&mut self, line: &str) {
        let line = line.trim_end();

        // Blank lines are punctuation in llama.cpp's output, and a run of them
        // would push out everything actually worth quoting.
        if line.is_empty() {
            return;
        }

        let kept = match line.char_indices().nth(TAIL_LINE_CHARS) {
            Some((cut, _)) => format!("{}…", &line[..cut]),
            None => line.to_string(),
        };

        while self.0.len() >= TAIL_LINES {
            self.0.pop_front();
        }

        self.0.push_back(kept);
    }

    /// Everything kept, oldest first — or nothing, if it never said anything.
    fn text(&self) -> Option<String> {
        (!self.0.is_empty()).then(|| {
            self.0
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    fn clear(&mut self) {
        self.0.clear();
    }
}

/// What the running server has told us about itself.
///
/// The stream has to be read continuously whether or not anyone wants it: a
/// pipe nobody drains fills up and stops the process writing to it. So the
/// reading is a task, and this is the pair of things that task leaves behind —
/// the window of lines it has kept, and a handle for knowing when it is done.
#[derive(Debug, Default)]
struct Output {
    tail: Arc<Mutex<Tail>>,
    reader: Mutex<Option<JoinHandle<()>>>,
}

impl Output {
    /// Start reading a freshly started server's stderr.
    ///
    /// Every line is written straight back out to Chief's own stderr. Piping
    /// the stream is what takes it away from the terminal a developer running
    /// `tauri dev` watches the model load in, and there is no reason both
    /// cannot have it — left inherited, that output reaches nobody at all in a
    /// packaged install, which is precisely where it was needed.
    fn watch(&self, stderr: ChildStderr) {
        let tail = Arc::clone(&self.tail);

        // This server's account of itself, not the last one's.
        if let Ok(mut tail) = tail.lock() {
            tail.clear();
        }

        let reader = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(stderr).lines();

            // Ends of its own accord when the pipe closes, which is when the
            // process it belongs to has gone.
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("llama-server: {line}");

                if let Ok(mut tail) = tail.lock() {
                    tail.push(&line);
                }
            }
        });

        if let Ok(mut slot) = self.reader.lock() {
            *slot = Some(reader);
        }
    }

    /// The last thing the server said, once there is nothing more coming.
    ///
    /// A process being gone does not mean its output has been read — the pipe
    /// is drained by a task of its own, a scheduling hop behind — so the task
    /// is waited for first. That wait is the difference between quoting the
    /// engine and reporting that it said nothing, and only one of those is
    /// true.
    async fn last_words(&self) -> Option<String> {
        // Taken out from under the lock before anything is awaited: the guard
        // must not be held across a suspension point.
        let reader = self.reader.lock().ok().and_then(|mut slot| slot.take());

        if let Some(reader) = reader {
            let _ = tokio::time::timeout(DRAIN_TIMEOUT, reader).await;
        }

        self.tail.lock().ok().and_then(|tail| tail.text())
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
fn arguments(weights: &Path, port: u16, tier: Tier) -> Vec<OsString> {
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
        OsString::from(tier.context_size().to_string()),
        // llama.cpp caches the KV state of a prompt prefix already; what it does
        // not do by default is reuse a cache entry whose prefix only partly
        // matches. Chief assembles every prompt with the stable parts first and
        // the volatile parts last precisely so that reuse can happen, and this
        // is the flag that lets it: prefill on a repeated prefix is paid once
        // rather than on every turn, which on a CPU is the difference between a
        // pause and a wait.
        OsString::from("--cache-reuse"),
        OsString::from(CACHE_REUSE_CHUNK.to_string()),
        // One slot, because Chief asks one question at a time.
        //
        // llama.cpp defaults to four, and hands each request whichever slot is
        // least recently used — so consecutive questions land on different
        // slots, each with its own KV cache, and the prefix reuse the flag
        // above exists for never happens. Observed on a 2014 Mac mini: a
        // question landed on a slot that had never been used and paid 39.6 s to
        // prefill 881 tokens; the next matched only 38% of its slot's prefix
        // and paid 77.5 s for 1,430. One slot means every question meets the
        // cache the last one left.
        OsString::from("--parallel"),
        OsString::from("1"),
        // And a ceiling on what those caches may hold. The engine's own default
        // is 8192 MiB — more than twice the headroom this app is written to live
        // within, spent on top of a resident model. Left alone it would cause
        // the swapping the cache exists to avoid.
        OsString::from("--cache-ram"),
        OsString::from(tier.cache_ram_mb().to_string()),
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
    /// The model this tier runs, so the setup screen can name it and the
    /// download can fetch it without working the tier out a second time.
    model: weights::Model,
    /// What this machine qualifies for, measured once when the engine is
    /// discovered. It decides the context window and the cache ceiling the
    /// server is started with, so it is read here rather than at every launch.
    tier: Tier,
    /// Whether this process is the one that starts and stops the server. False
    /// when the user pointed Chief at their own, which is not ours to kill.
    owned: bool,
    child: Mutex<Option<Child>>,
    /// When the engine was last asked for anything. Read by the idle
    /// supervisor, which gives the memory back when nothing has wanted it for
    /// a while.
    last_used: Mutex<Instant>,
    /// How many pieces of background work are using the engine right now.
    ///
    /// `last_used` is a moment, and a moment is enough for a question, which is
    /// over in seconds. It is not enough for a work-log pass: that starts the
    /// engine once and then generates up to a summary per merged pull request,
    /// which on a small model can outlast the idle timeout — and the supervisor
    /// would then stop the engine halfway through its own daemon's work.
    /// [`Attention`](crate::agent::Attention) cannot serve here because the
    /// daemon *reads* it to stand aside for the user; raising it would make the
    /// daemon yield to itself.
    working: Arc<AtomicUsize>,
    /// Whether the stray-engine warning has already been given.
    warned_about_stray: AtomicBool,
    /// What the child has said for itself. Kept beside the handle rather than
    /// inside it because the handle is cleared the moment the process is found
    /// to have exited — which is exactly when its last words are wanted.
    output: Output,
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

        let tier = Tier::for_machine(Machine::detect());
        let model = weights::for_tier(tier);

        Ok(Self {
            base_url,
            library_dirs: library_dirs(app, server.as_deref()),
            server,
            weights: model.path(&data_dir),
            tier,
            model,
            owned,
            child: Mutex::new(None),
            last_used: Mutex::new(Instant::now()),
            working: Arc::new(AtomicUsize::new(0)),
            warned_about_stray: AtomicBool::new(false),
            output: Output::default(),
        })
    }

    /// Where the engine is, or will be, listening.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Where the model file belongs on this machine.
    /// The model this machine runs, for naming it and for fetching it.
    #[must_use]
    pub fn model(&self) -> weights::Model {
        self.model
    }

    /// What this machine qualified for.
    #[must_use]
    pub fn tier(&self) -> Tier {
        self.tier
    }

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
        // Anything that wants the engine counts as use, whether or not it ends
        // up starting it — otherwise a busy hour of answered questions would
        // look idle to the supervisor below.
        self.touch();

        if !self.owned {
            return Ok(());
        }

        if client.health().await != Health::Down {
            // Something is answering on our port that this process did not
            // start. Almost always our own engine, orphaned by a crash or a
            // force-quit: `RunEvent::Exit` never ran, so nothing killed it, and
            // the health check above means we will now never start one either.
            //
            // It is not killed, because a server the user is running themselves
            // looks identical from here and is not ours to take. But it is said
            // once, because the consequence is otherwise invisible: the idle
            // supervisor holds no handle for it, so its memory is never given
            // back for as long as this installation runs.
            if self.child_state() == ChildState::None {
                self.warn_once_about_the_stray();
            }

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
            .args(arguments(&self.weights, port, self.tier))
            .stdin(Stdio::null())
            // Read rather than inherited. A server that dies on the way up says
            // why on this stream, and inherited it goes to whatever terminal
            // launched the app — which for anyone running an installed copy is
            // nowhere, leaving Chief to report an exit it cannot explain.
            .stderr(Stdio::piped())
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
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = command
            .spawn()
            .map_err(|error| Error::Spawn(error.to_string()))?;

        if let Some(stderr) = child.stderr.take() {
            self.output.watch(stderr);
        }

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
            // answer, and whatever killed it did so in seconds. Waiting out the
            // timeout would only delay saying so — and it did say why.
            if self.child_state() == ChildState::Exited {
                return Err(Error::stopped(self.output.last_words().await));
            }

            if std::time::Instant::now() >= deadline {
                return Err(Error::NeverReady);
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Whether a server this process started is up right now.
    ///
    /// Only a question about *our* child: a server the user runs themselves is
    /// never stopped, so it is always considered up.
    #[must_use]
    pub fn is_running(&self) -> bool {
        !self.owned || self.child_state() == ChildState::Running
    }

    /// Say once that something else is serving our port.
    ///
    /// Once, rather than on every question and every daemon pass, which is what
    /// makes it worth a flag rather than a bare `eprintln!`.
    fn warn_once_about_the_stray(&self) {
        if self.warned_about_stray.swap(true, Ordering::SeqCst) {
            return;
        }

        eprintln!(
            "an inference engine is already answering at {} that Chief did not start. \
             Chief will use it, but cannot stop it when it goes idle, so its memory \
             stays held. Quit anything else serving that address and restart Chief to \
             have it manage its own.",
            self.base_url
        );
    }

    /// Hold the engine open for as long as the guard lives.
    ///
    /// For background work that runs longer than the idle timeout. The engine
    /// is not stopped while any guard is alive, whatever the clock says.
    pub fn working(&self) -> Working {
        self.working.fetch_add(1, Ordering::SeqCst);

        Working(Arc::clone(&self.working))
    }

    /// Is anything holding the engine open?
    fn is_working(&self) -> bool {
        self.working.load(Ordering::SeqCst) > 0
    }

    /// Note that something wanted the engine just now.
    pub fn touch(&self) {
        if let Ok(mut last) = self.last_used.lock() {
            *last = Instant::now();
        }
    }

    /// How long the engine has gone unwanted.
    fn idle_for(&self) -> Duration {
        self.last_used
            .lock()
            .map_or(Duration::ZERO, |last| last.elapsed())
    }

    /// Give the memory back if nothing has wanted the engine for a while.
    ///
    /// Returns whether it stopped anything, which is what the test asserts on.
    /// A server the user is running themselves is never stopped, and neither is
    /// one that is still being waited on: `waiting` is the caller's answer to
    /// "is somebody owed an answer right now", which the supervisor takes from
    /// [`crate::agent::Attention`].
    fn stop_if_idle(&self, waiting: bool, timeout: Duration) -> bool {
        if !self.owned || waiting || self.is_working() || self.idle_for() < timeout {
            return false;
        }

        if self.child_state() != ChildState::Running {
            return false;
        }

        self.stop();
        true
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
/// Background work in flight. While one of these is alive the engine is not
/// stopped for being idle, however long the work takes.
pub struct Working(Arc<AtomicUsize>);

impl Drop for Working {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Watch for the engine going unused, and give its memory back when it does.
///
/// Chief runs `llama-server` as a child process, which means stopping it
/// returns every byte it held — the whole benefit of a separate process, and
/// one the app was not using: the model stayed resident for the life of the
/// window whether or not anybody asked it anything.
///
/// Nothing here has to wake it again. [`Engine::ensure_running`] is idempotent
/// and is called on the way into a question and on the way into a work-log
/// pass, so the next thing that wants the engine starts it.
pub fn supervise<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();

    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(IDLE_CHECK).await;

            let engine = app.state::<Engine>();
            let waiting = app.state::<crate::agent::Attention>().is_engaged();

            engine.stop_if_idle(waiting, IDLE_TIMEOUT);
        }
    });
}

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
        let arguments = arguments(Path::new("/models/model.gguf"), 11435, Tier::Standard);
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
        let arguments = arguments(Path::new("/models/model.gguf"), 11435, Tier::Standard);
        let rendered: Vec<String> = arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();

        let size = rendered
            .iter()
            .position(|argument| argument == "--ctx-size")
            .expect("the context size should be set");
        assert_eq!(
            rendered[size + 1],
            Tier::Standard.context_size().to_string()
        );
    }

    #[test]
    fn the_window_follows_the_tier_rather_than_a_single_number() {
        let render = |tier| {
            arguments(Path::new("/models/model.gguf"), 11435, tier)
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };

        let value_after = |rendered: &[String], flag: &str| {
            let at = rendered
                .iter()
                .position(|argument| argument == flag)
                .unwrap_or_else(|| panic!("{flag} should be set"));
            rendered[at + 1].clone()
        };

        let standard = render(Tier::Standard);
        let light = render(Tier::Light);

        // Tied to the tier rather than to a literal, so a hard-coded window
        // fails here even if somebody hard-codes the one this tier happens to
        // want. D7 makes the window a per-tier decision and this is what keeps
        // it one.
        assert_eq!(
            value_after(&standard, "--ctx-size"),
            Tier::Standard.context_size().to_string()
        );
        assert_eq!(
            value_after(&light, "--ctx-size"),
            Tier::Light.context_size().to_string()
        );
        assert_eq!(value_after(&standard, "--ctx-size"), "8192");
        assert_eq!(value_after(&light, "--ctx-size"), "4096");

        // The machine that gets the smaller model gets the smaller cache for
        // the same reason, and neither may reach the engine's own 8192 MiB
        // default — which is more memory than this app is written to use in
        // total, spent on top of a resident model.
        let standard_cache: u32 = value_after(&standard, "--cache-ram")
            .parse()
            .expect("the cache ceiling should be a number");
        let light_cache: u32 = value_after(&light, "--cache-ram")
            .parse()
            .expect("the cache ceiling should be a number");

        assert!(standard_cache < 8192, "got {standard_cache} MiB");
        assert!(light_cache < standard_cache);
    }

    #[test]
    fn lets_a_repeated_prompt_prefix_be_reused_rather_than_recomputed() {
        // Prefill is the dominant cost of a question on a CPU, and llama.cpp
        // will not reuse a partly matching prefix unless asked. Without this
        // flag every turn pays for the whole prompt again.
        let arguments = arguments(Path::new("/models/model.gguf"), 11435, Tier::Standard);
        let rendered: Vec<String> = arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();

        let at = rendered
            .iter()
            .position(|argument| argument == "--cache-reuse")
            .expect("prompt-prefix reuse should be enabled");

        let chunk: u32 = rendered[at + 1]
            .parse()
            .expect("the reuse chunk should be a number");
        assert!(chunk > 0, "a chunk of zero leaves reuse switched off");
    }

    #[test]
    fn keeps_every_question_on_one_slot_so_the_reuse_is_reachable() {
        // `--cache-reuse` is worth nothing if consecutive questions land on
        // different slots: llama.cpp defaults to four and picks the least
        // recently used, so each question meets a cache belonging to some
        // other conversation. Chief asks one question at a time, so one slot
        // is both the truth and what makes the flag above pay.
        for tier in [Tier::Standard, Tier::Light] {
            let rendered: Vec<String> = arguments(Path::new("/models/model.gguf"), 11435, tier)
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect();

            let at = rendered
                .iter()
                .position(|argument| argument == "--parallel")
                .expect("the slot count should be stated rather than defaulted");

            assert_eq!(rendered[at + 1], "1", "{tier:?} should run one slot");
        }
    }

    #[test]
    fn asks_for_the_templates_that_tool_calling_needs() {
        let arguments = arguments(Path::new("/models/model.gguf"), 11435, Tier::Standard);

        assert!(
            arguments.iter().any(|argument| argument == "--jinja"),
            "without --jinja the model cannot call tools: {arguments:?}"
        );
    }

    #[test]
    fn serves_the_model_under_the_name_requests_use() {
        let arguments = arguments(Path::new("/models/model.gguf"), 11435, Tier::Standard);
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
        let arguments = arguments(Path::new("/models/model.gguf"), 4242, Tier::Standard);
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

    /// An engine with no child process, for the idle rules — which decide
    /// whether to stop something, and can be asked that without one running.
    fn stopped_engine() -> Engine {
        Engine {
            base_url: "http://127.0.0.1:11435".to_string(),
            server: None,
            library_dirs: Vec::new(),
            weights: PathBuf::from("/models/model.gguf"),
            tier: Tier::Standard,
            model: weights::STANDARD,
            owned: true,
            child: Mutex::new(None),
            last_used: Mutex::new(Instant::now()),
            working: Arc::new(AtomicUsize::new(0)),
            warned_about_stray: AtomicBool::new(false),
            output: Output::default(),
        }
    }

    #[test]
    fn an_engine_nobody_has_used_is_not_stopped_before_its_time() {
        let engine = stopped_engine();

        assert!(
            !engine.stop_if_idle(false, Duration::from_secs(600)),
            "a fresh engine has not been idle for ten minutes"
        );
    }

    #[test]
    fn the_stray_engine_warning_is_given_once_and_not_on_every_question() {
        // ensure_running is called on the way into every question and every
        // daemon pass. A warning without this flag would be printed on all of
        // them.
        let engine = stopped_engine();

        assert!(!engine.warned_about_stray.load(Ordering::SeqCst));

        engine.warn_once_about_the_stray();
        assert!(engine.warned_about_stray.load(Ordering::SeqCst));

        // The second call is a no-op; what is asserted is that the flag latches
        // rather than toggling.
        engine.warn_once_about_the_stray();
        assert!(engine.warned_about_stray.load(Ordering::SeqCst));
    }

    #[test]
    fn work_in_flight_holds_the_engine_open_however_long_it_takes() {
        // The case this exists for: a work-log pass is one model call per
        // merged pull request and then a brief. On a small model that runs
        // past the idle timeout, and stopping the engine halfway through the
        // daemon's own work makes every pass from then on die at the same
        // place. Attention cannot serve here — the daemon reads it to stand
        // aside for the user, so raising it would make the daemon yield to
        // itself.
        let engine = stopped_engine();
        let working = engine.working();

        assert!(
            !engine.stop_if_idle(false, Duration::ZERO),
            "the engine must stay up while background work holds it"
        );

        drop(working);

        // And once the work is done it is idle again like anything else. There
        // is no child here, so nothing is stopped — what is being asserted is
        // that the guard no longer refuses on its own account.
        assert!(!engine.is_working(), "the guard should have been released");
    }

    #[test]
    fn two_pieces_of_work_both_have_to_finish() {
        let engine = stopped_engine();

        let first = engine.working();
        let second = engine.working();

        drop(first);
        assert!(engine.is_working(), "the second is still holding it");

        drop(second);
        assert!(!engine.is_working());
    }

    #[test]
    fn an_engine_still_being_waited_on_is_left_alone() {
        let engine = stopped_engine();

        // Idle by the clock, but somebody is owed an answer: a question that
        // takes longer than the timeout must not have the engine pulled out
        // from under it.
        assert!(
            !engine.stop_if_idle(true, Duration::ZERO),
            "the engine must not be stopped while a question is in flight"
        );
    }

    #[test]
    fn a_server_the_user_runs_themselves_is_never_stopped() {
        let mut engine = stopped_engine();
        engine.owned = false;

        assert!(
            !engine.stop_if_idle(false, Duration::ZERO),
            "an engine Chief did not start is not Chief's to stop"
        );
    }

    #[test]
    fn asking_for_the_engine_counts_as_using_it() {
        let engine = stopped_engine();
        std::thread::sleep(Duration::from_millis(20));

        let before = engine.idle_for();
        engine.touch();

        assert!(
            engine.idle_for() < before,
            "touching should reset how long the engine has gone unwanted"
        );
    }

    #[test]
    fn nothing_was_started_before_anything_starts_it() {
        let engine = Engine {
            base_url: "http://127.0.0.1:11435".to_string(),
            server: None,
            library_dirs: Vec::new(),
            weights: PathBuf::from("/models/model.gguf"),
            tier: Tier::Standard,
            model: weights::STANDARD,
            owned: true,
            child: Mutex::new(None),
            last_used: Mutex::new(Instant::now()),
            working: Arc::new(AtomicUsize::new(0)),
            warned_about_stray: AtomicBool::new(false),
            output: Output::default(),
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
            tier: Tier::Standard,
            model: weights::STANDARD,
            owned: false,
            child: Mutex::new(None),
            last_used: Mutex::new(Instant::now()),
            working: Arc::new(AtomicUsize::new(0)),
            warned_about_stray: AtomicBool::new(false),
            output: Output::default(),
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
            tier: Tier::Standard,
            model: weights::STANDARD,
            owned: false,
            child: Mutex::new(None),
            last_used: Mutex::new(Instant::now()),
            working: Arc::new(AtomicUsize::new(0)),
            warned_about_stray: AtomicBool::new(false),
            output: Output::default(),
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

    #[test]
    fn says_nothing_about_an_engine_that_said_nothing() {
        let tail = Tail::default();

        assert_eq!(tail.text(), None, "there is nothing to quote");

        // And the error made from that admits it rather than picking a cause.
        let reported = Error::stopped(tail.text()).to_string();
        assert!(
            reported.contains("wrote nothing"),
            "silence is the finding, and should be reported as one: {reported}"
        );
    }

    #[test]
    fn keeps_the_last_words_and_drops_the_older_ones() {
        let mut tail = Tail::default();

        // Twice the window, so the first half has to go.
        for line in 0..TAIL_LINES * 2 {
            tail.push(&format!("line {line}"));
        }

        let text = tail.text().expect("should have kept something");
        let lines: Vec<&str> = text.lines().collect();

        assert_eq!(lines.len(), TAIL_LINES, "the window is a fixed size");
        assert_eq!(lines.first(), Some(&"line 20"));
        assert_eq!(
            lines.last(),
            Some(&"line 39"),
            "what the engine said last is the part that explains it"
        );
    }

    #[test]
    fn does_not_let_one_line_stand_in_for_the_whole_window() {
        let mut tail = Tail::default();

        // A progress meter redrawing itself with carriage returns arrives as a
        // single line with no end to it.
        tail.push(&"=".repeat(TAIL_LINE_CHARS * 10));
        tail.push("dyld: Symbol not found");

        let text = tail.text().expect("should have kept something");

        assert!(
            text.chars().count() < TAIL_LINE_CHARS * 2,
            "a line without a newline in it must not defeat the bound: {} characters kept",
            text.chars().count()
        );
        assert!(
            text.contains("dyld: Symbol not found"),
            "and the truncation must not cost the line that matters: {text}"
        );
    }

    #[test]
    fn does_not_spend_the_window_on_blank_lines() {
        let mut tail = Tail::default();

        tail.push("ggml_backend_load_best: failed to load");
        for _ in 0..TAIL_LINES * 2 {
            tail.push("");
            tail.push("   ");
        }

        assert_eq!(
            tail.text().as_deref(),
            Some("ggml_backend_load_best: failed to load"),
            "llama.cpp punctuates with blank lines; they are not what it said"
        );
    }

    #[test]
    fn starts_again_for_each_server_it_watches() {
        let mut tail = Tail::default();

        tail.push("the last attempt's complaint");
        tail.clear();

        assert_eq!(
            tail.text(),
            None,
            "a new server's silence must not be reported as the old one's words"
        );
    }

    /// A directory of this test's own, so two tests cleaning up after
    /// themselves cannot take each other's files.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("chief-engine-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("should create a scratch directory");
        dir
    }

    /// A loopback address with nothing behind it, so a health check can only
    /// fail. Asked of the operating system rather than picked, so a real
    /// `llama-server` on the usual port cannot make this test pass.
    fn address_nothing_is_serving() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("should bind a port");
        let port = listener
            .local_addr()
            .expect("a bound listener has an address")
            .port();
        drop(listener);
        format!("http://127.0.0.1:{port}")
    }

    /// A stand-in for `llama-server` that dies the way a real one does when the
    /// machine cannot run it: whatever it has to say on stderr, no port opened,
    /// and gone in milliseconds.
    #[cfg(unix)]
    fn stand_in_server(dir: &Path, complaint: Option<&str>) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let script = match complaint {
            Some(complaint) => format!("#!/bin/sh\necho '{complaint}' >&2\nexit 1\n"),
            None => "#!/bin/sh\nexit 1\n".to_string(),
        };

        let path = dir.join("llama-server");
        std::fs::write(&path, script).expect("should write the stand-in");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("should make the stand-in executable");
        path
    }

    /// An engine whose server exits before it can answer, and whose client will
    /// find nothing listening where it looks.
    #[cfg(unix)]
    fn engine_that_dies_at_once(dir: &Path, complaint: Option<&str>) -> Engine {
        // Only that the file is there matters; the stand-in never opens it.
        let weights = dir.join("model.gguf");
        std::fs::write(&weights, b"GGUF").expect("should write the stand-in weights");

        Engine {
            base_url: address_nothing_is_serving(),
            server: Some(stand_in_server(dir, complaint)),
            library_dirs: Vec::new(),
            weights,
            tier: Tier::Standard,
            model: weights::STANDARD,
            owned: true,
            child: Mutex::new(None),
            last_used: Mutex::new(Instant::now()),
            working: Arc::new(AtomicUsize::new(0)),
            warned_about_stray: AtomicBool::new(false),
            output: Output::default(),
        }
    }

    /// The report this test exists for: on macOS 12 the bundled build is dead
    /// at dynamic-link time, and Chief told the user their model might not fit
    /// in this machine's memory — so they downloaded a smaller one, which of
    /// course changed nothing. The engine had said exactly what was wrong on a
    /// stream nobody was reading.
    ///
    /// Unix only: the stand-in is a shell script, and there is no portable way
    /// to conjure an executable that writes to stderr without building one.
    #[cfg(unix)]
    #[tokio::test]
    async fn reports_what_the_engine_said_rather_than_guessing_why_it_stopped() {
        const COMPLAINT: &str = "dyld: Symbol not found: (_cblas_sgemm$NEWLAPACK$ILP64)";

        let dir = scratch("last-words");
        let engine = engine_that_dies_at_once(&dir, Some(COMPLAINT));
        let client = llama::Client::with_base_url(engine.base_url()).expect("loopback is allowed");

        let error = engine
            .start_and_wait(&client)
            .await
            .expect_err("a server that exits at once cannot answer");
        let reported = error.to_string();

        assert!(
            reported.contains(COMPLAINT),
            "the engine's own words are the diagnosis and should be passed on: {reported}"
        );
        assert!(
            !reported.contains("memory"),
            "nothing observed here says anything about memory, so nothing should claim it: {reported}"
        );

        std::fs::remove_dir_all(&dir).expect("should clean up");
    }

    /// And the other half. An exit on its own establishes nothing, so when
    /// there is nothing to quote the message has to say so rather than fill
    /// the gap with the likeliest-sounding cause.
    #[cfg(unix)]
    #[tokio::test]
    async fn admits_it_does_not_know_when_the_engine_dies_without_a_word() {
        let dir = scratch("silence");
        let engine = engine_that_dies_at_once(&dir, None);
        let client = llama::Client::with_base_url(engine.base_url()).expect("loopback is allowed");

        let error = engine
            .start_and_wait(&client)
            .await
            .expect_err("a server that exits at once cannot answer");
        let reported = error.to_string();

        assert!(
            reported.contains("stopped before it could answer"),
            "the exit is the one thing known, and should be said: {reported}"
        );
        assert!(
            reported.contains("wrote nothing"),
            "having nothing to go on is itself the finding: {reported}"
        );

        std::fs::remove_dir_all(&dir).expect("should clean up");
    }
}
