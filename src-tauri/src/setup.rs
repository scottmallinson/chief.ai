//! First-run readiness.
//!
//! Chief needs a model before it can answer anything. The engine itself is part
//! of the installation, so there is only one thing left to fetch — the weights
//! — and this module answers two questions for the frontend: are they here, and
//! is the engine answering?

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime, State};

use crate::engine::{self, Engine};
use crate::llama::{Client, Health};
use crate::weights::{self, DownloadProgress};

/// The event carrying download progress to the setup screen.
pub const DOWNLOAD_PROGRESS_EVENT: &str = "model-download-progress";

/// What the setup screen needs to know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Readiness {
    /// The model Chief runs, named for a person to read.
    pub model: String,
    /// What this machine qualified for — `standard` or `light`. Shown before a
    /// download starts, because a person about to spend two gigabytes and a
    /// share of their memory is entitled to know which of them they are getting
    /// and that something looked at their machine before deciding.
    pub tier: String,
    /// Roughly what the model holds once loaded, in mebibytes. "2400" is the
    /// number that answers "will this slow my laptop down"; "Q4_K_M" is not.
    pub model_size_mb: u32,
    /// Whether the weights have been downloaded to this machine.
    pub model_installed: bool,
    /// Whether the engine is answering, still loading, or not running.
    pub engine: Health,
    /// Why the engine is not answering, phrased for the person reading it.
    pub problem: Option<String>,
}

/// What can go wrong getting this machine ready.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Download(#[from] weights::Error),
    #[error(transparent)]
    Engine(#[from] engine::Error),
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// Why the engine is not up, given what we know about this machine.
///
/// Only worth saying while it is actually down: once it is answering, the
/// reason it was not is stale. Ordered by how fundamental the problem is, so
/// the user is told about a broken installation before a missing download.
fn problem(engine: Health, available: bool, model_installed: bool) -> Option<String> {
    if engine != Health::Down {
        return None;
    }

    Some(
        if !available {
            engine::Error::Missing
        } else if !model_installed {
            engine::Error::NoWeights
        } else {
            engine::Error::NeverReady
        }
        .to_string(),
    )
}

/// Is the machine ready to answer questions?
#[tauri::command]
pub async fn check_readiness(
    engine: State<'_, Engine>,
    client: State<'_, Client>,
) -> Result<Readiness, Error> {
    let health = client.health().await;
    let model_installed = engine.has_weights();

    Ok(Readiness {
        model: engine.model().describe(),
        tier: engine.tier().to_string(),
        model_size_mb: engine.model().approx_resident_mb,
        model_installed,
        engine: health,
        problem: problem(health, engine.is_available(), model_installed),
    })
}

/// Download the model, emitting progress as it goes, then start the engine on
/// it so the user does not have to press a second button.
#[tauri::command]
pub async fn download_model<R: Runtime>(
    app: AppHandle<R>,
    engine: State<'_, Engine>,
    client: State<'_, Client>,
) -> Result<(), Error> {
    weights::download(
        &engine.model(),
        engine.weights(),
        |progress: DownloadProgress| {
            // A dropped event only costs a progress tick, so it is not worth
            // failing the download over.
            let _ = app.emit(DOWNLOAD_PROGRESS_EVENT, &progress);
        },
    )
    .await?;

    engine.ensure_running(client.inner()).await?;

    Ok(())
}

/// Start the engine, and do not answer until it can answer.
#[tauri::command]
pub async fn start_engine(
    engine: State<'_, Engine>,
    client: State<'_, Client>,
) -> Result<(), Error> {
    engine.start_and_wait(client.inner()).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_the_model_is_missing_when_it_has_not_been_downloaded() {
        let reason = problem(Health::Down, true, false).expect("should explain itself");

        assert!(reason.contains("not been downloaded"), "got {reason}");
    }

    #[test]
    fn says_the_engine_did_not_start_when_the_model_is_there() {
        let reason = problem(Health::Down, true, true).expect("should explain itself");

        assert!(reason.contains("never began answering"), "got {reason}");
    }

    #[test]
    fn a_broken_installation_is_reported_before_a_missing_download() {
        // Downloading two gigabytes would not help if there is nothing to run
        // it, so that is the thing to say.
        let reason = problem(Health::Down, false, false).expect("should explain itself");

        assert!(
            reason.contains("missing from this installation"),
            "got {reason}"
        );
    }

    #[test]
    fn stops_explaining_once_the_engine_is_up() {
        assert_eq!(problem(Health::Ready, true, true), None);
        assert_eq!(
            problem(Health::Loading, true, true),
            None,
            "a model that is still loading is not a problem to report"
        );
    }

    #[test]
    fn describes_readiness_for_the_setup_screen() {
        let readiness = Readiness {
            model: weights::STANDARD.describe(),
            tier: crate::probe::Tier::Standard.to_string(),
            model_size_mb: weights::STANDARD.approx_resident_mb,
            model_installed: false,
            engine: Health::Down,
            problem: problem(Health::Down, true, false),
        };

        let body = serde_json::to_value(&readiness).expect("should serialize");

        assert_eq!(body["modelInstalled"], serde_json::json!(false));
        assert_eq!(body["tier"], serde_json::json!("standard"));
        assert_eq!(body["modelSizeMb"], serde_json::json!(2400));
        assert_eq!(body["engine"], serde_json::json!("down"));
        assert_eq!(
            body["model"],
            serde_json::json!(weights::STANDARD.describe())
        );
    }
}
