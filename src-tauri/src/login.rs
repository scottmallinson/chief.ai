//! Opening Chief at login, and making sure the entry that does it cannot
//! outlive Chief.
//!
//! **Off until the user turns it on.** An app that adds itself to somebody's
//! login is doing something to their machine rather than inside itself, so
//! nothing here runs except from the Settings switch. The operating system's
//! own entry is the only record of the choice, so the switch reads it back
//! rather than trusting a stored copy of it.
//!
//! **An entry left behind by an uninstall must do nothing.** Chief's code is
//! gone by the time that matters, so each platform covers it where it can:
//!
//! - **Windows** is covered by the installers, the only code that runs at
//!   uninstall. The entry is a value under the user's `Run` key, written by
//!   `tauri-plugin-autostart`. Tauri's NSIS uninstaller already deletes it;
//!   `windows/hooks.nsh` also deletes the Task Manager `StartupApproved` value
//!   the plugin writes beside it, and `windows/login-entry.wxs` does both for
//!   the MSI, which deletes neither. None of them touch it on an update, or
//!   every upgrade would quietly turn the setting off.
//! - **Linux** has nothing that runs at uninstall for a file in the user's
//!   home: a `.deb` or an `.rpm` must not reach into home directories, and an
//!   AppImage is deleted rather than uninstalled. So Chief writes the entry
//!   itself with `TryExec`, which the XDG autostart spec says makes the
//!   desktop ignore the entry once the program is not there.
//! - **macOS** has no uninstaller at all — the app is dragged to the Bin. So
//!   the Launch Agent starts Chief by bundle identifier through `open` rather
//!   than by path: once the app is gone, `open` finds nothing and exits, and
//!   an app moved to another folder is still found.
//!
//! What stays behind on those two is a small file that does nothing. Turning
//! the switch off before deleting Chief removes it.

use tauri::{AppHandle, Runtime};
use tauri_plugin_autostart::ManagerExt;

/// The argument the login entry launches Chief with, so a launch at login can
/// be told apart from somebody opening it.
pub const LAUNCHED_AT_LOGIN: &str = "--launched-at-login";

/// The plugin that reads and removes the entry, and writes it on Windows.
///
/// Registered in `lib.rs` but never granted to the renderer: the switch goes
/// through [`set_launch_at_login`], like every other setting.
pub fn plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri_plugin_autostart::Builder::new()
        .arg(LAUNCHED_AT_LOGIN)
        .build()
}

/// Whether Chief is set to launch at login, as the operating system has it.
#[tauri::command]
pub fn launch_at_login<R: Runtime>(app: AppHandle<R>) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

/// Turn launching at login on or off.
#[tauri::command]
pub fn set_launch_at_login<R: Runtime>(app: AppHandle<R>, enabled: bool) -> Result<(), String> {
    if enabled {
        write(&app)
    } else {
        app.autolaunch()
            .disable()
            .map_err(|error| error.to_string())
    }
}

/// Point an existing Linux entry at wherever Chief is now.
///
/// An AppImage carries its version in its file name, so updating one leaves
/// the entry naming a file that has been replaced, and `TryExec` then makes it
/// silently do nothing while the switch still says it is on. Only the two
/// lines naming the program are rewritten: a desktop that switched the entry
/// off did it with a key of its own in the same file, and that is the user's.
pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    #[cfg(target_os = "linux")]
    {
        let Ok(path) = linux::entry(app) else { return };
        let Ok(existing) = std::fs::read_to_string(&path) else {
            return;
        };
        let Some(exe) = linux::program(app) else {
            return;
        };

        if let Some(updated) = retarget(&existing, &exe) {
            if let Err(error) = std::fs::write(&path, updated) {
                eprintln!("the login entry could not be pointed at this copy of Chief: {error}");
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    let _ = app;
}

/// Write the entry for this platform.
fn write<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let path = linux::entry(app)?;
        let exe = linux::program(app).ok_or("Chief could not tell where it is installed")?;
        let name = app.package_info().name.clone();

        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
        }
        std::fs::write(&path, desktop_entry(&name, &exe)).map_err(|error| error.to_string())
    }

    #[cfg(target_os = "macos")]
    {
        use tauri::Manager;

        let home = app.path().home_dir().map_err(|error| error.to_string())?;
        let dir = home.join("Library").join("LaunchAgents");
        // The plugin reads and removes the entry by this name, so it has to
        // be the one written here.
        let path = dir.join(format!("{}.plist", app.package_info().name));

        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        std::fs::write(&path, launch_agent(&app.config().identifier))
            .map_err(|error| error.to_string())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        app.autolaunch().enable().map_err(|error| error.to_string())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::path::PathBuf;

    use tauri::{AppHandle, Manager, Runtime};

    /// Where the entry lives. The plugin looks for it at exactly this path.
    pub fn entry<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
        let home = app.path().home_dir().map_err(|error| error.to_string())?;

        Ok(home
            .join(".config")
            .join("autostart")
            .join(format!("{}.desktop", app.package_info().name)))
    }

    /// The file to run: the AppImage itself when Chief is one, because the
    /// executable inside it lives on a mount that is gone once Chief exits.
    pub fn program<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
        let path = match app.env().appimage {
            Some(appimage) => PathBuf::from(appimage),
            None => std::env::current_exe().ok()?,
        };

        path.to_str().map(str::to_string)
    }
}

/// Escape a value for a desktop entry, where `\`, newlines and the like have
/// escapes of their own. A path with a newline in it would otherwise end the
/// line and start a key of its choosing.
#[cfg(any(target_os = "linux", test))]
fn string_value(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());

    for c in value.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(c),
        }
    }

    escaped
}

/// One quoted argument of an `Exec` line.
///
/// Quoted always, so a path with a space in it — an AppImage in `~/My Apps` —
/// is one argument. Inside quotes the spec reserves `"`, `` ` ``, `$` and `\`,
/// and `%` begins a field code anywhere, so each is escaped before the whole
/// is escaped again as a string value.
#[cfg(any(target_os = "linux", test))]
fn exec_argument(value: &str) -> String {
    let mut quoted = String::from("\"");

    for c in value.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }

    quoted.push('"');
    string_value(&quoted)
}

/// The `Exec` and `TryExec` lines for `exe`.
#[cfg(any(target_os = "linux", test))]
fn program_lines(exe: &str) -> (String, String) {
    (
        format!("Exec={} {LAUNCHED_AT_LOGIN}", exec_argument(exe)),
        format!("TryExec={}", string_value(exe)),
    )
}

/// The autostart entry for `exe`.
#[cfg(any(target_os = "linux", test))]
fn desktop_entry(name: &str, exe: &str) -> String {
    let (exec, try_exec) = program_lines(exe);

    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name={name}\n\
         Comment=Opens {name} in the background when you log in\n\
         {exec}\n\
         {try_exec}\n\
         StartupNotify=false\n\
         Terminal=false\n",
        name = string_value(name),
    )
}

/// `existing` with its `Exec` and `TryExec` pointed at `exe`, or `None` if
/// they already are. Every other line is kept as it was.
#[cfg(any(target_os = "linux", test))]
fn retarget(existing: &str, exe: &str) -> Option<String> {
    let (exec, try_exec) = program_lines(exe);
    let mut changed = false;
    let mut saw_try_exec = false;

    let mut lines: Vec<String> = existing
        .lines()
        .map(|line| {
            let replacement = if line.starts_with("Exec=") {
                &exec
            } else if line.starts_with("TryExec=") {
                saw_try_exec = true;
                &try_exec
            } else {
                return line.to_string();
            };

            changed |= line != replacement;
            replacement.clone()
        })
        .collect();

    // An entry written before `TryExec` was — by an earlier build — gains one,
    // straight after the line it guards.
    if !saw_try_exec {
        let at = lines.iter().position(|line| line.starts_with("Exec="))?;
        lines.insert(at + 1, try_exec);
        changed = true;
    }

    changed.then(|| lines.join("\n") + "\n")
}

/// Escape text for a property list.
#[cfg(any(target_os = "macos", test))]
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The Launch Agent that opens the app with `bundle_id`.
///
/// No path anywhere in it: `open -b` asks Launch Services for the app, so an
/// app that has been moved is still found and one that has been deleted is
/// not, and the agent exits having done nothing. `-g` keeps it from taking
/// focus from whatever the user opened first.
#[cfg(any(target_os = "macos", test))]
fn launch_agent(bundle_id: &str) -> String {
    let bundle_id = xml(bundle_id);

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20 <key>Label</key>\n\
         \x20 <string>{bundle_id}</string>\n\
         \x20 <key>AssociatedBundleIdentifiers</key>\n\
         \x20 <array><string>{bundle_id}</string></array>\n\
         \x20 <key>ProgramArguments</key>\n\
         \x20 <array>\n\
         \x20   <string>/usr/bin/open</string>\n\
         \x20   <string>-g</string>\n\
         \x20   <string>-b</string>\n\
         \x20   <string>{bundle_id}</string>\n\
         \x20   <string>--args</string>\n\
         \x20   <string>{LAUNCHED_AT_LOGIN}</string>\n\
         \x20 </array>\n\
         \x20 <key>RunAtLoad</key>\n\
         \x20 <true/>\n\
         </dict>\n\
         </plist>\n"
    )
}

#[cfg(test)]
mod tests {
    use super::{desktop_entry, launch_agent, retarget, LAUNCHED_AT_LOGIN};

    const APPIMAGE: &str = "/home/someone/Apps/Chief_0.6.0_amd64.AppImage";

    fn value<'a>(entry: &'a str, key: &str) -> &'a str {
        entry
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .unwrap_or_else(|| panic!("no {key} in:\n{entry}"))
    }

    /// The whole of the Linux cover: a desktop ignores an autostart entry
    /// whose `TryExec` names nothing, so an entry left by a removed package or
    /// a deleted AppImage does nothing at the next login.
    #[test]
    fn a_linux_entry_is_ignored_once_chief_is_gone() {
        let entry = desktop_entry("Chief", APPIMAGE);

        assert_eq!(
            value(&entry, "TryExec"),
            APPIMAGE,
            "without TryExec naming the program, an uninstalled Chief is still launched at login"
        );
        assert_eq!(
            value(&entry, "Exec"),
            format!("\"{APPIMAGE}\" {LAUNCHED_AT_LOGIN}")
        );
    }

    #[test]
    fn a_path_with_spaces_is_one_argument() {
        let entry = desktop_entry("Chief", "/home/someone/My Apps/Chief.AppImage");

        assert_eq!(
            value(&entry, "Exec"),
            format!("\"/home/someone/My Apps/Chief.AppImage\" {LAUNCHED_AT_LOGIN}")
        );
    }

    #[test]
    fn a_path_cannot_add_lines_to_the_entry() {
        let entry = desktop_entry("Chief", "/tmp/x\nHidden=true\nExec=/bin/evil");

        assert_eq!(entry.matches("\nExec=").count(), 1);
        assert!(!entry.contains("\nHidden="));
    }

    #[test]
    fn reserved_characters_are_escaped_in_exec() {
        let entry = desktop_entry("Chief", "/opt/$HOME/100%/a\"b");

        assert_eq!(
            value(&entry, "Exec"),
            format!("\"/opt/\\\\$HOME/100%%/a\\\\\"b\" {LAUNCHED_AT_LOGIN}")
        );
    }

    #[test]
    fn an_entry_already_pointing_here_is_left_alone() {
        let entry = desktop_entry("Chief", APPIMAGE);

        assert_eq!(retarget(&entry, APPIMAGE), None);
    }

    #[test]
    fn an_updated_appimage_is_followed_and_nothing_else_changes() {
        let switched_off = desktop_entry("Chief", APPIMAGE) + "X-GNOME-Autostart-enabled=false\n";
        let moved = "/home/someone/Apps/Chief_0.7.0_amd64.AppImage";

        let updated = retarget(&switched_off, moved).expect("the path changed");

        assert_eq!(value(&updated, "TryExec"), moved);
        assert!(value(&updated, "Exec").starts_with(&format!("\"{moved}\"")));
        assert!(
            updated.contains("X-GNOME-Autostart-enabled=false"),
            "a desktop's own switch in the file is the user's and must survive"
        );
    }

    #[test]
    fn an_entry_without_try_exec_gains_one() {
        let old = "[Desktop Entry]\nType=Application\nExec=/usr/bin/Chief --launched-at-login\n";

        let updated = retarget(old, "/usr/bin/Chief").expect("TryExec was missing");

        assert_eq!(value(&updated, "TryExec"), "/usr/bin/Chief");
    }

    /// The whole of the macOS cover: nothing in the agent names a path, so
    /// once the app is deleted `open` finds nothing and the agent does nothing.
    #[test]
    fn the_launch_agent_finds_chief_by_bundle_rather_than_by_path() {
        let agent = launch_agent("com.scottmallinson.chief");

        assert!(agent.contains("<string>/usr/bin/open</string>"));
        assert!(
            agent.contains("<string>-b</string>\n    <string>com.scottmallinson.chief</string>")
        );
        assert!(
            !agent.contains(".app/") && !agent.contains(".app</string>"),
            "a path to the app outlives the app; the bundle identifier does not"
        );
        assert!(agent.contains(&format!("<string>{LAUNCHED_AT_LOGIN}</string>")));
    }
}
