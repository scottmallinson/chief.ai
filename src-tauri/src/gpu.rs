//! Whether the model should run on a graphics card, worked out from what the
//! engine itself can see.
//!
//! The engine is asked, not guessed at: `llama-server --list-devices` prints
//! every device the bundled build could offload to, with its memory, and
//! returns in tens of milliseconds without loading a model. It is run once per
//! launch and the answer kept.
//!
//! **Nothing here loads the model to find out.** The failure this exists to
//! avoid is a driver that hangs rather than fails, which takes the whole
//! machine with it and cannot be caught by a trial run: the trial run is the
//! hang.
//!
//! The rule is deliberately cautious, because the two mistakes are not equal.
//! Running on the CPU when a card would have been faster costs speed, and the
//! switch in Settings gives it back. Running on a graphics driver that freezes
//! the computer costs the user their work. So a card is used only when it is
//! plainly a dedicated one, and anything unclear is the CPU.

use std::ffi::OsString;
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

/// The least memory a card must report to be worth putting the model on. The
/// weights alone are about 3.2 GB; with less than this most of the model would
/// be left on the CPU anyway, paying for transfers between the two.
const MIN_DEDICATED_MB: u64 = 4096;

/// A card reporting at least this share of the machine's memory is treated as
/// integrated. An integrated GPU has no memory of its own: the driver reports a
/// slice of system RAM, which on the machine this was written for is 18 GB of
/// 32 GB (56%), and the usual figure is about half. A card with memory of its
/// own reports its own, whatever the RAM beside it — so this errs towards the
/// CPU for a large card in a small machine, which is the cheaper mistake.
const SHARED_MEMORY_PERCENT: u64 = 40;

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

/// Whether a device has memory of its own, as far as its name and size say.
fn is_dedicated(device: &Device, system_mb: u64) -> bool {
    // There is no integrated NVIDIA part in a PC, so the name settles it.
    if device.name.to_ascii_lowercase().contains("nvidia") {
        return true;
    }

    device.memory_mb >= MIN_DEDICATED_MB
        && device.memory_mb * 100 < system_mb.saturating_mul(SHARED_MEMORY_PERCENT)
}

/// What was decided, and enough of why to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    /// The device to put the model on, or `None` for the CPU.
    pub device: Option<String>,
    /// One sentence for Settings, in plain words.
    pub summary: String,
}

/// Choose a device from what the engine listed.
///
/// With several dedicated cards the largest is named, so the choice does not
/// depend on the order they were listed in. Naming it also keeps an integrated
/// GPU in the same machine out of it.
pub fn classify(devices: &[Device], system_mb: u64) -> Detection {
    let best = devices
        .iter()
        .filter(|device| is_dedicated(device, system_mb))
        .max_by_key(|device| device.memory_mb);

    if let Some(card) = best {
        return Detection {
            device: Some(card.id.clone()),
            summary: format!(
                "Found {} with {} GB of its own memory.",
                card.name,
                card.memory_mb / 1024
            ),
        };
    }

    let summary = match devices.first() {
        None => "No graphics card was found, so the model runs on the processor.".to_string(),
        Some(first) => format!(
            "Found {}, which shares memory with the rest of the computer. \
             The model runs on the processor, which keeps the computer responsive.",
            first.name
        ),
    };

    Detection {
        device: None,
        summary,
    }
}

/// The answer when the engine could not be asked.
pub fn unknown() -> Detection {
    Detection {
        device: None,
        summary: "Chief could not tell what graphics card this computer has, \
                  so the model runs on the processor."
            .to_string(),
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

/// The `--device` arguments for a chosen device.
pub fn device_arguments(device: &str) -> Vec<OsString> {
    vec![OsString::from("--device"), OsString::from(device)]
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

    const GB: u64 = 1024;

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
    fn keeps_the_model_off_the_integrated_graphics_it_froze() {
        // The laptop this was written for: 32 GB, and the driver reports 18 GB
        // of it as the card's.
        let found = classify(
            &[device("Vulkan0", "Intel(R) Arc(TM) 140V GPU", 18_000)],
            32 * GB,
        );

        assert_eq!(found.device, None);
        assert!(found.summary.contains("shares memory"), "{}", found.summary);
    }

    #[test]
    fn uses_a_card_with_memory_of_its_own() {
        let found = classify(
            &[device("Vulkan0", "AMD Radeon RX 7800 XT", 16 * GB)],
            64 * GB,
        );

        assert_eq!(found.device.as_deref(), Some("Vulkan0"));
    }

    #[test]
    fn uses_an_nvidia_card_whatever_the_memory_beside_it() {
        // 24 GB of card in a 32 GB machine is 75% — over the shared-memory
        // line — but nothing that says NVIDIA shares anything.
        let found = classify(
            &[device("Vulkan0", "NVIDIA GeForce RTX 4090", 24 * GB)],
            32 * GB,
        );

        assert_eq!(found.device.as_deref(), Some("Vulkan0"));
    }

    #[test]
    fn names_the_dedicated_card_in_a_machine_that_also_has_integrated_graphics() {
        let found = classify(
            &[
                device("Vulkan0", "Intel(R) Iris(R) Xe Graphics", 8 * GB),
                device("Vulkan1", "AMD Radeon RX 6700M", 10 * GB),
            ],
            32 * GB,
        );

        // The integrated one is listed first and must not be the one named.
        assert_eq!(found.device.as_deref(), Some("Vulkan1"));
    }

    #[test]
    fn leaves_a_card_too_small_to_hold_the_model_alone() {
        let found = classify(&[device("Vulkan0", "AMD Radeon RX 550", 2 * GB)], 32 * GB);

        assert_eq!(found.device, None);
    }

    #[test]
    fn takes_the_cpu_when_there_is_nothing_to_use() {
        assert_eq!(classify(&[], 16 * GB).device, None);
        assert_eq!(unknown().device, None);
    }

    #[test]
    fn names_the_device_for_the_engine() {
        assert_eq!(
            device_arguments("Vulkan1"),
            [OsString::from("--device"), OsString::from("Vulkan1")]
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
