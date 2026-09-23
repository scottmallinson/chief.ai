//! Staying open after the window closes.
//!
//! The daemon, the watcher and the brief are written for a process that is
//! still running when the user is not looking at it: the brief is meant to be
//! waiting when they open Chief, not started by opening it. Closing the window
//! used to end the process, so all of that ran only while the window happened
//! to be open. Closing now hides the window, the tray icon brings it back, and
//! Quit in the tray menu ends Chief the way closing used to.
//!
//! Nothing here changes what Chief does in the background or who it talks to —
//! only whether the process is still there to do it. The engine still gives its
//! memory back after [`crate::engine`]'s idle timeout, so what stays resident
//! is a small Rust process and a hidden webview, not a model.
//!
//! **A process nobody can see is worse than one that quit**, so three rules:
//!
//! - **Closing is asked about once.** The first close with no stored choice
//!   keeps the window open and asks, in the window, which the user wants. If
//!   the renderer cannot answer — a white screen, a crash — the second close
//!   takes the default rather than asking again, so a broken page costs one
//!   click, never the ability to close the window.
//! - **No tray, no hiding.** Linux loads its tray library at run time and the
//!   crate panics when there is none; a machine where the icon could not be
//!   made quits on close exactly as before.
//! - **Launching Chief again shows the one already running**, through the
//!   single-instance plugin on Windows and Linux and `Reopen` on macOS. That is
//!   also the way back on a Linux desktop whose tray exists but is not shown —
//!   GNOME without an extension — where nothing here can tell the icon is
//!   invisible.
//!
//! A launch at login — see [`crate::login`] — passes [`LAUNCHED_AT_LOGIN`]
//! and starts in the tray rather than opening a window, unless there is no
//! tray, in which case the window is the only way to reach Chief and it opens
//! as usual.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, CloseRequestApi, Emitter, Manager, Runtime, Window};

use crate::daemon;
use crate::db;
use crate::login::LAUNCHED_AT_LOGIN;
use crate::settings;

/// Where the choice is stored: `true` to keep running, `false` to quit.
///
/// Absent means the user has not been asked, which is a third state rather
/// than a default — it is what makes the first close a question.
const KEEP_RUNNING_KEY: &str = "window.keep_running";

/// The window this is about. Tauri names the one in `tauri.conf.json` `main`.
const MAIN: &str = "main";

/// Emitted to the window the first time it is closed with no choice stored.
const ASK_EVENT: &str = "close-to-tray-question";

/// The tray menu's items, by id.
const OPEN: &str = "open";
const SYNC: &str = "sync";
const QUIT: &str = "quit";

/// What closing the window does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnClose {
    /// Hide the window and keep running.
    Hide,
    /// Keep the window open and ask which of the other two the user wants.
    Ask,
    /// Let the window close, which ends Chief.
    Quit,
}

/// Decide what a close does. Pure, so every combination is tested.
///
/// `asked` is whether this process has already asked once and not been
/// answered: the default then applies, so a renderer that cannot show the
/// question does not leave a window that will not close.
pub fn decide(tray: bool, keep_running: Option<bool>, asked: bool) -> OnClose {
    if !tray {
        return OnClose::Quit;
    }

    match keep_running {
        Some(true) => OnClose::Hide,
        Some(false) => OnClose::Quit,
        None if asked => OnClose::Hide,
        None => OnClose::Ask,
    }
}

/// Whether this launch should start in the tray without showing the window.
///
/// Only a launch at login, and only with a tray to start in: hidden with no
/// icon, Chief would be running where nobody could reach it.
pub fn starts_hidden<I, S>(args: I, tray: bool) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    tray && args
        .into_iter()
        .any(|arg| arg.as_ref() == LAUNCHED_AT_LOGIN)
}

/// Read a stored choice. Anything unrecognised reads as not having chosen.
fn parse(stored: Option<&str>) -> Option<bool> {
    match stored {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

/// The stored choice, if one has been made.
async fn stored(pool: &sqlx::SqlitePool) -> Result<Option<bool>, sqlx::Error> {
    Ok(parse(
        settings::get(pool, KEEP_RUNNING_KEY).await?.as_deref(),
    ))
}

/// Store a choice.
async fn remember(pool: &sqlx::SqlitePool, keep_running: bool) -> Result<(), sqlx::Error> {
    settings::set(pool, KEEP_RUNNING_KEY, &keep_running.to_string()).await
}

/// What this process knows about closing, for the window-event handler.
///
/// The handler is synchronous and SQLite is not, so the stored choice is read
/// once at startup and kept here, and every write goes through both.
#[derive(Debug, Default)]
pub struct Background {
    tray: AtomicBool,
    keep_running: Mutex<Option<bool>>,
    asked: AtomicBool,
}

impl Background {
    fn keep_running(&self) -> Option<bool> {
        *self
            .keep_running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn set_keep_running(&self, keep_running: Option<bool>) {
        *self
            .keep_running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = keep_running;
    }

    /// What this close does, recording that the question has now been asked.
    fn on_close(&self) -> OnClose {
        let asked = self.asked.swap(true, Ordering::SeqCst);
        decide(self.tray.load(Ordering::SeqCst), self.keep_running(), asked)
    }
}

/// Put the icon in the tray and load the stored choice.
///
/// Called from `setup`, which runs on the main thread — that matters, because
/// the tray is built on the main thread and a panic building it therefore
/// arrives here, where it can be caught, rather than in the event loop.
pub fn install<R: Runtime>(app: &AppHandle<R>) {
    let state = Background::default();

    // `catch_unwind` because on Linux the tray library is loaded at run time
    // and its absence is a panic rather than an error. The panic hook has
    // already written the reason to stderr by the time this sees it.
    let built = std::panic::catch_unwind(AssertUnwindSafe(|| tray(app)));
    let tray = match built {
        Ok(Ok(())) => true,
        Ok(Err(error)) => {
            eprintln!("no tray icon, so closing the window will quit Chief: {error}");
            false
        }
        Err(_) => {
            eprintln!("the tray icon could not be made, so closing the window will quit Chief");
            false
        }
    };
    state.tray.store(tray, Ordering::SeqCst);
    app.manage(state);

    // The window from `tauri.conf.json` is already open by now, so a launch
    // at login hides it rather than never showing it. Showing by default and
    // hiding on a condition means a mistake here costs a window opening at
    // login, not a Chief nobody can find.
    if starts_hidden(std::env::args(), tray) {
        if let Some(window) = app.get_webview_window(MAIN) {
            let _ = window.hide();
        }
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let choice = match db::pool(&app).await {
            Ok(pool) => stored(&pool).await,
            Err(error) => {
                eprintln!("the close-to-tray setting could not be read: {error}");
                return;
            }
        };

        match choice {
            Ok(choice) => app.state::<Background>().set_keep_running(choice),
            Err(error) => eprintln!("the close-to-tray setting could not be read: {error}"),
        }
    });
}

fn tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, OPEN, "Open Chief", true, None::<&str>)?,
            &MenuItem::with_id(app, SYNC, "Sync now", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, QUIT, "Quit Chief", true, None::<&str>)?,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id(MAIN)
        .tooltip("Chief")
        .menu(&menu)
        // A left click opens the window and a right click the menu, on the
        // platforms that tell them apart. Linux does not, and shows the menu —
        // which is why Open is its first item.
        .show_menu_on_left_click(false)
        .on_menu_event(menu_event)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}

fn menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    match event.id().as_ref() {
        OPEN => show(app),
        SYNC => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = daemon::sync_now(app).await {
                    eprintln!("a sync asked for from the tray failed: {error}");
                }
            });
        }
        // Through `exit`, so `RunEvent::Exit` still stops the engine.
        QUIT => app.exit(0),
        _ => {}
    }
}

/// Bring the window back, from wherever it went.
pub fn show<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Handle a request to close a window.
pub fn close_requested<R: Runtime>(window: &Window<R>, api: &CloseRequestApi) {
    if window.label() != MAIN {
        return;
    }

    match window.state::<Background>().on_close() {
        // The last window closing ends the process, as it always has.
        OnClose::Quit => {}
        OnClose::Hide => {
            api.prevent_close();
            let _ = window.hide();
        }
        OnClose::Ask => {
            api.prevent_close();
            let _ = window.emit(ASK_EVENT, ());
        }
    }
}

/// What the settings screen shows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Behaviour {
    /// Whether this machine has a tray to keep running in.
    pub tray: bool,
    /// The stored choice, or `None` if the user has not made one.
    pub keep_running: Option<bool>,
    /// What this platform calls the tray, for the words on screen.
    pub tray_name: &'static str,
}

const TRAY_NAME: &str = if cfg!(target_os = "macos") {
    "menu bar"
} else {
    "system tray"
};

/// Whether closing the window keeps Chief running.
#[tauri::command]
pub async fn window_behaviour<R: Runtime>(app: AppHandle<R>) -> Result<Behaviour, String> {
    let pool = db::pool(&app).await.map_err(|error| error.to_string())?;
    let keep_running = stored(&pool).await.map_err(|error| error.to_string())?;

    Ok(Behaviour {
        tray: app.state::<Background>().tray.load(Ordering::SeqCst),
        keep_running,
        tray_name: TRAY_NAME,
    })
}

/// Choose whether closing the window keeps Chief running.
#[tauri::command]
pub async fn set_keep_running<R: Runtime>(
    app: AppHandle<R>,
    keep_running: bool,
) -> Result<(), String> {
    let pool = db::pool(&app).await.map_err(|error| error.to_string())?;
    remember(&pool, keep_running)
        .await
        .map_err(|error| error.to_string())?;
    app.state::<Background>()
        .set_keep_running(Some(keep_running));

    Ok(())
}

/// Finish the close the question interrupted, now that there is an answer.
#[tauri::command]
pub fn close_window<R: Runtime>(app: AppHandle<R>) {
    let state = app.state::<Background>();

    match decide(
        state.tray.load(Ordering::SeqCst),
        state.keep_running(),
        true,
    ) {
        OnClose::Quit => app.exit(0),
        OnClose::Hide | OnClose::Ask => {
            if let Some(window) = app.get_webview_window(MAIN) {
                let _ = window.hide();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{decide, parse, remember, starts_hidden, stored, Background, OnClose};
    use crate::db::test_support::migrated_pool;
    use crate::login::LAUNCHED_AT_LOGIN;
    use std::sync::atomic::Ordering;

    #[test]
    fn a_machine_with_no_tray_quits_whatever_was_chosen() {
        for keep_running in [None, Some(true), Some(false)] {
            for asked in [false, true] {
                assert_eq!(
                    decide(false, keep_running, asked),
                    OnClose::Quit,
                    "hiding with no tray leaves a process nobody can reach \
                     ({keep_running:?}, asked: {asked})"
                );
            }
        }
    }

    #[test]
    fn a_stored_choice_is_followed_without_asking() {
        for asked in [false, true] {
            assert_eq!(decide(true, Some(true), asked), OnClose::Hide);
            assert_eq!(decide(true, Some(false), asked), OnClose::Quit);
        }
    }

    #[test]
    fn with_no_choice_the_first_close_asks() {
        assert_eq!(decide(true, None, false), OnClose::Ask);
    }

    #[test]
    fn a_question_nobody_answered_is_not_asked_again() {
        let state = Background::default();
        state.tray.store(true, Ordering::SeqCst);

        assert_eq!(state.on_close(), OnClose::Ask);
        assert_eq!(
            state.on_close(),
            OnClose::Hide,
            "a renderer that cannot show the question must not stop the window closing"
        );
    }

    #[test]
    fn anything_unrecognised_reads_as_not_having_chosen() {
        assert_eq!(parse(Some("true")), Some(true));
        assert_eq!(parse(Some("false")), Some(false));
        assert_eq!(parse(Some("yes")), None);
        assert_eq!(parse(Some("")), None);
        assert_eq!(parse(None), None);
    }

    #[tokio::test]
    async fn a_choice_survives_being_stored() {
        let pool = migrated_pool().await;

        assert_eq!(stored(&pool).await.expect("read"), None);

        remember(&pool, false).await.expect("write");
        assert_eq!(stored(&pool).await.expect("read"), Some(false));

        remember(&pool, true).await.expect("write");
        assert_eq!(stored(&pool).await.expect("read"), Some(true));
    }

    #[test]
    fn a_launch_at_login_starts_in_the_tray() {
        assert!(starts_hidden(["/usr/bin/chief", LAUNCHED_AT_LOGIN], true));
    }

    #[test]
    fn somebody_opening_chief_sees_the_window() {
        assert!(!starts_hidden(["/usr/bin/chief"], true));
        assert!(!starts_hidden(["/usr/bin/chief", "--launched"], true));
    }

    #[test]
    fn with_no_tray_a_launch_at_login_still_opens_the_window() {
        assert!(
            !starts_hidden(["/usr/bin/chief", LAUNCHED_AT_LOGIN], false),
            "hidden with no tray icon is a Chief nobody can reach"
        );
    }
}
