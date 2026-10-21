//! What the engine can see of the graphics hardware, for Settings to show.
//!
//! Chief does not decide from it. Nothing can promise a graphics driver is
//! stable, and the failure that matters is one that freezes the whole machine,
//! which no fallback sees and no trial run can avoid: the trial run is the
//! hang. So on the platforms that ship a Vulkan build the model runs on the
//! CPU until the person ticks the box, and this module only tells them what
//! they would be switching on. `llama-server --list-devices` returns in tens of
//! milliseconds without loading a model, and is asked only when Settings opens.
//!
//! Choosing between a card and integrated graphics by size or by name was
//! tried and dropped: the engine does not print whether a device is
//! integrated, so it was a guess about a flag the driver already holds.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::process::Command;

use crate::{engine, settings};

/// The setting the switch in Settings writes: `true` or `false`, absent for
/// "decide automatically".
const KEY: &str = "engine.use_gpu";

/// How long to wait for the engine to list its devices. A healthy answer takes
/// tens of milliseconds; a driver that stalls while it is being enumerated is
/// exactly the case that must end in the CPU rather than in a wait.
const LIST_TIMEOUT: Duration = Duration::from_secs(15);

/// One device from `--list-devices`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// What `--device` takes, such as `Vulkan0`.
    pub id: String,
    /// What the driver calls it.
    pub name: String,
    /// Memory the device reports, in mebibytes.
    pub memory_mb: u64,
}

/// Read the engine's device list.
///
/// Lines look like `  Vulkan0: Intel(R) Arc(TM) 140V GPU (18000 MiB, 17000
/// MiB free)`, under a header, with `(none)` when there is nothing. Anything
/// that does not match is skipped rather than guessed at, so a build that
/// changes the wording ends in no devices, which is the CPU.
pub fn parse_devices(output: &str) -> Vec<Device> {
    output.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<Device> {
    // Only the indented lines are devices; the header and `(none)` are not
    // `id: description` anyway, but the indentation is what the engine writes.
    if !line.starts_with(' ') {
        return None;
    }

    let (id, rest) = line.trim().split_once(": ")?;
    if id.is_empty() || id.contains(' ') {
        return None;
    }

    let rest = rest.trim_end().strip_suffix(')')?;
    let open = rest.rfind(" (")?;
    let (name, sizes) = (rest[..open].trim(), &rest[open + 2..]);

    let (total, free) = sizes.split_once(" MiB, ")?;
    free.strip_suffix(" MiB free")?;

    Some(Device {
        id: id.to_string(),
        name: name.to_string(),
        memory_mb: total.trim().parse().ok()?,
    })
}

/// One sentence for Settings about what the engine found.
pub fn describe(devices: &[Device]) -> String {
    match devices {
        [] => "No graphics card was found.".to_string(),
        [one] => format!("Found {}.", one.name),
        many => format!(
            "Found {}.",
            many.iter()
                .map(|device| device.name.as_str())
                .collect::<Vec<_>>()
                .join(" and ")
        ),
    }
}

/// Ask the engine which devices it sees.
///
/// `None` when it could not be asked: the server would not start, did not
/// answer in time, or exited with an error. The caller treats that as unclear.
pub async fn list(
    server: &Path,
    backend_dir: Option<&Path>,
    library_dirs: &[PathBuf],
) -> Option<Vec<Device>> {
    let mut command = Command::new(server);
    command
        .arg("--list-devices")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true);

    if let Some(dir) = backend_dir {
        command.current_dir(dir);
    }

    if !library_dirs.is_empty() {
        let variable = engine::library_path_variable();
        command.env(
            variable,
            engine::library_path(library_dirs, std::env::var_os(variable)),
        );
    }

    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let output = tokio::time::timeout(LIST_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;

    output
        .status
        .success()
        .then(|| parse_devices(&String::from_utf8_lossy(&output.stdout)))
}

/// What the user chose in Settings, if they chose.
pub async fn stored(pool: &SqlitePool) -> Result<Option<bool>, sqlx::Error> {
    Ok(settings::get(pool, KEY)
        .await?
        .and_then(|value| value.parse().ok()))
}

async fn remember(pool: &SqlitePool, choice: Option<bool>) -> Result<(), sqlx::Error> {
    match choice {
        Some(choice) => settings::set(pool, KEY, &choice.to_string()).await,
        None => settings::clear(pool, KEY).await,
    }
}

/// What the switch in Settings shows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Whether the model is set to run on a graphics card.
    pub use_gpu: bool,
    /// The user's own choice, or `None` while Chief decides.
    pub chosen: Option<bool>,
    /// Whether the environment is overriding both, for a developer.
    pub overridden: bool,
    /// What Chief found, when it has looked. Absent on platforms where the
    /// engine's own default is used without looking.
    pub summary: Option<String>,
}

/// What is in use, and why.
#[tauri::command]
pub async fn gpu_status<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<Status, String> {
    use tauri::Manager;

    let pool = crate::db::pool(&app)
        .await
        .map_err(|error| error.to_string())?;
    let chosen = stored(&pool).await.map_err(|error| error.to_string())?;

    Ok(app.state::<engine::Engine>().gpu_status(chosen).await)
}

/// Choose whether the model runs on a graphics card, or `None` to let Chief
/// decide again. The engine is restarted so the choice is the one in use.
#[tauri::command]
pub async fn set_use_gpu<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    enabled: Option<bool>,
) -> Result<(), String> {
    use tauri::Manager;

    let pool = crate::db::pool(&app)
        .await
        .map_err(|error| error.to_string())?;
    remember(&pool, enabled)
        .await
        .map_err(|error| error.to_string())?;

    app.state::<engine::Engine>().choose_gpu(enabled);
    engine::start(&app);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, name: &str, memory_mb: u64) -> Device {
        Device {
            id: id.to_string(),
            name: name.to_string(),
            memory_mb,
        }
    }

    #[test]
    fn reads_what_the_engine_prints() {
        let output = "Available devices:\n  \
            Vulkan0: Intel(R) Arc(TM) 140V GPU (16GB) (18000 MiB, 17000 MiB free)\n  \
            Vulkan1: NVIDIA GeForce RTX 4060 Laptop GPU (8188 MiB, 7000 MiB free)\n";

        assert_eq!(
            parse_devices(output),
            [
                device("Vulkan0", "Intel(R) Arc(TM) 140V GPU (16GB)", 18000),
                device("Vulkan1", "NVIDIA GeForce RTX 4060 Laptop GPU", 8188),
            ]
        );
    }

    #[test]
    fn reads_no_devices_as_none() {
        assert!(parse_devices("Available devices:\n  (none)\n").is_empty());
        assert!(parse_devices("").is_empty());
    }

    #[test]
    fn skips_what_it_does_not_recognise_rather_than_guessing() {
        for line in [
            "Available devices:",
            "  (none)",
            "  Vulkan0 Intel Arc (18000 MiB, 17000 MiB free)",
            "  Vulkan0: Intel Arc (lots of MiB, 17000 MiB free)",
            "  Vulkan0: Intel Arc (18000 MiB)",
            "Vulkan0: Intel Arc (18000 MiB, 17000 MiB free)",
            "ggml_vulkan: Found 1 Vulkan devices:",
        ] {
            assert!(parse_devices(line).is_empty(), "{line:?}");
        }
    }

    #[test]
    fn says_what_it_found() {
        assert_eq!(describe(&[]), "No graphics card was found.");
        assert_eq!(
            describe(&[device(
                "Vulkan0",
                "Intel(R) Arc(TM) 140V GPU (16GB)",
                18_425
            )]),
            "Found Intel(R) Arc(TM) 140V GPU (16GB)."
        );
        assert_eq!(
            describe(&[
                device("Vulkan0", "Intel(R) Iris(R) Xe Graphics", 8192),
                device("Vulkan1", "NVIDIA GeForce RTX 4060", 8188),
            ]),
            "Found Intel(R) Iris(R) Xe Graphics and NVIDIA GeForce RTX 4060."
        );
    }

    #[tokio::test]
    async fn remembers_a_choice_and_forgets_it() {
        let pool = crate::db::test_support::migrated_pool().await;

        assert_eq!(stored(&pool).await.expect("read"), None);

        remember(&pool, Some(true)).await.expect("write");
        assert_eq!(stored(&pool).await.expect("read"), Some(true));

        remember(&pool, Some(false)).await.expect("write");
        assert_eq!(stored(&pool).await.expect("read"), Some(false));

        remember(&pool, None).await.expect("clear");
        assert_eq!(stored(&pool).await.expect("read"), None);
    }

    /// A server that prints a list the way the engine does.
    #[cfg(unix)]
    fn fake_server(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join("llama-server");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    #[cfg(unix)]
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("chief-gpu-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn asks_the_engine_and_reads_the_answer() {
        let dir = scratch("list");
        let server = fake_server(
            &dir,
            "[ \"$1\" = --list-devices ] || exit 2\n\
             echo 'Available devices:'\n\
             echo '  Vulkan0: Test Card (8192 MiB, 8000 MiB free)'",
        );

        let devices = list(&server, None, &[]).await.expect("an answer");

        assert_eq!(devices, [device("Vulkan0", "Test Card", 8192)]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn treats_an_engine_that_fails_as_not_answering() {
        let dir = scratch("fails");
        let server = fake_server(&dir, "echo 'Available devices:'; exit 1");

        assert_eq!(list(&server, None, &[]).await, None);
    }

    #[tokio::test]
    async fn treats_an_engine_that_is_not_there_as_not_answering() {
        assert_eq!(
            list(Path::new("/nonexistent/llama-server"), None, &[]).await,
            None
        );
    }
}
