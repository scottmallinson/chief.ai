//! First-run readiness.
//!
//! Chief needs a local model before it can answer anything. Rather than asking
//! the user to open a terminal, the app checks for Ollama itself and can pull
//! the model on their behalf — every call here goes to `localhost`.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime, State};

use crate::agent::DEFAULT_MODEL;
use crate::ollama::{self, Client, PullProgress};

/// The event carrying download progress to the setup screen.
pub const PULL_PROGRESS_EVENT: &str = "model-pull-progress";

/// What the setup screen needs to know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Readiness {
    /// Whether Ollama answered on this machine.
    pub ollama_running: bool,
    /// Ollama's version, when it is running.
    pub ollama_version: Option<String>,
    /// The model Chief will use.
    pub model: String,
    /// Whether that model is already installed.
    pub model_installed: bool,
    /// Why Ollama could not be reached, phrased for the person reading it.
    pub problem: Option<String>,
}

impl Readiness {
    /// Nothing works until both of these are true.
    fn stopped(problem: &ollama::Error) -> Self {
        Self {
            ollama_running: false,
            ollama_version: None,
            model: DEFAULT_MODEL.to_string(),
            model_installed: false,
            problem: Some(problem.to_string()),
        }
    }
}

/// A model is installed if its name matches, with or without an explicit tag.
fn installed(models: &[String], wanted: &str) -> bool {
    let bare = wanted.split_once(':').map_or(wanted, |(name, _)| name);

    models.iter().any(|model| {
        model == wanted || model == bare || model.split_once(':').is_some_and(|(n, _)| n == bare)
    })
}

/// Is the machine ready to answer questions?
#[tauri::command]
pub async fn check_readiness(client: State<'_, Client>) -> Result<Readiness, ollama::Error> {
    let version = match client.version().await {
        Ok(version) => version,
        // Not running, or not installed at all. Either way there is nothing to
        // report but the reason, and the screen offers the fix.
        Err(problem) => return Ok(Readiness::stopped(&problem)),
    };

    let models = client.installed_models().await.unwrap_or_default();

    Ok(Readiness {
        ollama_running: true,
        ollama_version: Some(version),
        model_installed: installed(&models, DEFAULT_MODEL),
        model: DEFAULT_MODEL.to_string(),
        problem: None,
    })
}

/// Download the model, emitting progress as it goes.
#[tauri::command]
pub async fn pull_model<R: Runtime>(
    app: AppHandle<R>,
    client: State<'_, Client>,
) -> Result<(), ollama::Error> {
    client
        .pull(DEFAULT_MODEL, |progress: PullProgress| {
            // A dropped event only costs a progress tick, so it is not worth
            // failing the download over.
            let _ = app.emit(PULL_PROGRESS_EVENT, &progress);
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_a_model_however_it_is_tagged() {
        let installed_models = vec!["llama3.2:3b".to_string()];

        assert!(installed(&installed_models, "llama3.2:3b"));
        assert!(installed(&installed_models, "llama3.2"));
    }

    #[test]
    fn does_not_match_a_different_model() {
        let installed_models = vec!["llama3.1:8b".to_string(), "mistral:latest".to_string()];

        assert!(!installed(&installed_models, "llama3.2:3b"));
    }

    #[test]
    fn nothing_is_installed_on_a_fresh_machine() {
        assert!(!installed(&[], "llama3.2:3b"));
    }
}
