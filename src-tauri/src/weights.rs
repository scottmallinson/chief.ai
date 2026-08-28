//! The model file the engine loads.
//!
//! llama.cpp runs a single GGUF file off disk, so Chief owns one: it knows
//! where the file belongs, whether it is there, and how to fetch it the first
//! time with a progress bar rather than a terminal.
//!
//! ## The one thing here that leaves the machine
//!
//! Downloading the weights is the only outbound request in this module, and it
//! is the narrowest one that can exist: a fixed URL on [`SOURCE_HOST`], sent
//! with no token, no cookie and no body. Nothing about the user — not their
//! work, not their questions, not their identity — is part of it, and it
//! happens once, when they press the button. Every hop is required to be
//! HTTPS, and a redirect that tries to leave HTTPS fails the download.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::probe::Tier;

/// One model Chief can run, and everything needed to fetch and name it.
///
/// A catalogue rather than a single pinned file, because the machine decides.
/// Every entry here must be a model llama.cpp recognises for **native tool
/// calling**: Chief's orchestrator asks the model which tool to run and with
/// what arguments through the model's own chat template, so a template with no
/// tool-use structures is not a smaller option, it is a different application.
/// That is why both entries are Llama 3.2 — same family, same template, one
/// third the size — rather than the lightest model that would load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    /// What a person calls it.
    pub name: &'static str,
    /// The quantisation, which is what makes it fit. Q4_K_M is the usual
    /// balance between size and quality.
    pub quantisation: &'static str,
    /// The file on disk. Named after the release it came from, so a future
    /// upgrade lands beside it rather than silently replacing it.
    pub file_name: &'static str,
    /// Where the weights come from. Pinned to a revision rather than a branch,
    /// so the file Chief downloads today is the file it downloaded yesterday.
    pub source: &'static str,
    /// Roughly what it holds once loaded, in mebibytes: the weights plus a
    /// working KV cache. Shown to the user before a download starts, because
    /// "2 GB" is the number they need and "Q4_K_M" is not.
    pub approx_resident_mb: u32,
}

/// The model Chief would rather run, on a machine with room for it.
pub const STANDARD: Model = Model {
    name: "Llama 3.2 3B Instruct",
    quantisation: "Q4_K_M",
    file_name: "Llama-3.2-3B-Instruct-Q4_K_M.gguf",
    source: "https://huggingface.co/bartowski/Llama-3.2-3B-Instruct-GGUF/resolve/5ab33fa94d1d04e903623ae72c95d1696f09f9e8/Llama-3.2-3B-Instruct-Q4_K_M.gguf",
    approx_resident_mb: 2400,
};

/// The same family a third of the size, for a machine that cannot hold the
/// other one. It answers less well; it answers.
pub const LIGHT: Model = Model {
    name: "Llama 3.2 1B Instruct",
    quantisation: "Q4_K_M",
    file_name: "Llama-3.2-1B-Instruct-Q4_K_M.gguf",
    source: "https://huggingface.co/bartowski/Llama-3.2-1B-Instruct-GGUF/resolve/067b946cf014b7c697f3654f621d577a3e3afd1c/Llama-3.2-1B-Instruct-Q4_K_M.gguf",
    approx_resident_mb: 1100,
};

/// Everything Chief can run.
///
/// Test-only: production reaches a model through [`for_tier`], and this exists
/// so the invariants every entry must hold — a pinned HTTPS source on the model
/// host, a file name nothing else shares — are asserted across the whole
/// catalogue rather than against whichever one someone remembered.
#[cfg(test)]
const CATALOGUE: [Model; 2] = [STANDARD, LIGHT];

/// The model this machine gets.
#[must_use]
pub const fn for_tier(tier: Tier) -> Model {
    match tier {
        Tier::Standard => STANDARD,
        Tier::Light => LIGHT,
    }
}

/// The only host these weights are ever fetched from.
const SOURCE_HOST: &str = "huggingface.co";

/// Every GGUF file starts with these four bytes. Checking them catches the
/// classic failure — an error page, a login wall or a truncated transfer saved
/// under the model's name — before the engine tries to load it.
const GGUF_MAGIC: &[u8; 4] = b"GGUF";

/// The folder inside the app's data directory that holds models.
const FOLDER: &str = "models";

/// How the download is going, as the setup screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    /// What is happening, phrased for the person reading it.
    pub status: String,
    /// Bytes fetched so far.
    #[serde(default)]
    pub completed: u64,
    /// Bytes in the whole file, when the server says.
    #[serde(default)]
    pub total: u64,
}

/// What can go wrong getting the weights onto this machine.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("refusing to download the model from '{0}' — only {SOURCE_HOST} over HTTPS")]
    UntrustedSource(String),
    #[error("could not reach {SOURCE_HOST} to download the model: {0}")]
    Transport(String),
    #[error("downloading the model failed with HTTP {status}")]
    Status { status: u16 },
    #[error("could not write the model to {path}: {source}")]
    Storage {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the downloaded file is not a model. Try the download again.")]
    NotAModel,
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// A short, human name for the model, for the setup screen and settings.
impl Model {
    /// How the model is named to a person: "Llama 3.2 3B Instruct (Q4_K_M)".
    #[must_use]
    pub fn describe(&self) -> String {
        format!("{} ({})", self.name, self.quantisation)
    }

    /// Where this model belongs inside the app's data directory.
    ///
    /// Keyed on the file name, so switching tiers lands the new weights beside
    /// the old ones rather than on top of them.
    #[must_use]
    pub fn path(&self, data_dir: &Path) -> PathBuf {
        data_dir.join(FOLDER).join(self.file_name)
    }
}

/// Where a download in progress is written. Kept beside the finished file so
/// the rename that completes it never crosses a filesystem boundary.
fn part_path(destination: &Path) -> PathBuf {
    destination.with_extension("gguf.part")
}

/// Only ever talk HTTPS, however many hops the host redirects us through.
///
/// Hugging Face serves the file itself from a CDN, so redirects have to be
/// followed — but a redirect down to plain HTTP would put the transfer on the
/// network in the clear, and is refused.
fn https_only() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.url().scheme() != "https" {
            return attempt.error(std::io::Error::other("a redirect tried to leave HTTPS"));
        }

        if attempt.previous().len() >= 10 {
            return attempt.error(std::io::Error::other("too many redirects"));
        }

        attempt.follow()
    })
}

/// Download the weights to `destination`, reporting progress as it goes.
///
/// A part-finished download is resumed rather than restarted: this is a couple
/// of gigabytes, and a dropped connection three quarters of the way through
/// should not cost the whole thing.
pub async fn download<F>(model: &Model, destination: &Path, mut on_progress: F) -> Result<(), Error>
where
    F: FnMut(DownloadProgress),
{
    let url = reqwest::Url::parse(model.source)
        .map_err(|_| Error::UntrustedSource(model.source.to_string()))?;

    if url.scheme() != "https" || url.host_str() != Some(SOURCE_HOST) {
        return Err(Error::UntrustedSource(url.to_string()));
    }

    if let Some(folder) = destination.parent() {
        tokio::fs::create_dir_all(folder)
            .await
            .map_err(|source| storage(folder, source))?;
    }

    let partial = part_path(destination);
    let already = tokio::fs::metadata(&partial)
        .await
        .map(|file| file.len())
        .unwrap_or(0);

    on_progress(DownloadProgress {
        status: "Contacting the model host".to_string(),
        completed: already,
        total: 0,
    });

    let http = reqwest::Client::builder()
        .redirect(https_only())
        .build()
        .map_err(|error| Error::Transport(error.to_string()))?;

    let mut request = http.get(url);
    if already > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={already}-"));
    }

    let response = request
        .send()
        .await
        .map_err(|error| Error::Transport(error.to_string()))?;

    let status = response.status();
    if !status.is_success() {
        return Err(Error::Status {
            status: status.as_u16(),
        });
    }

    // The host is free to ignore a range request. When it does, what arrives is
    // the whole file from byte zero, so start the part file over rather than
    // appending a second copy onto the first.
    let resuming = already > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT;
    let mut completed = if resuming { already } else { 0 };
    let total = response.content_length().unwrap_or(0) + completed;

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resuming)
        .truncate(!resuming)
        .open(&partial)
        .await
        .map_err(|source| storage(&partial, source))?;

    let mut stream = response.bytes_stream();
    let mut since_report = 0_u64;

    {
        use futures_util::StreamExt;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| Error::Transport(error.to_string()))?;

            file.write_all(&chunk)
                .await
                .map_err(|source| storage(&partial, source))?;

            completed += chunk.len() as u64;
            since_report += chunk.len() as u64;

            // A progress event per network chunk would be thousands a second
            // for no more information than one every few megabytes.
            if since_report >= REPORT_EVERY {
                since_report = 0;
                on_progress(DownloadProgress {
                    status: "Downloading the model".to_string(),
                    completed,
                    total,
                });
            }
        }
    }

    file.flush()
        .await
        .map_err(|source| storage(&partial, source))?;
    drop(file);

    on_progress(DownloadProgress {
        status: "Checking the download".to_string(),
        completed,
        total,
    });

    if !looks_like_a_model(&partial).await? {
        // Keeping it would poison the resume: the next attempt would append to
        // whatever this is instead of fetching the model.
        let _ = tokio::fs::remove_file(&partial).await;

        return Err(Error::NotAModel);
    }

    tokio::fs::rename(&partial, destination)
        .await
        .map_err(|source| storage(destination, source))?;

    on_progress(DownloadProgress {
        status: "Done".to_string(),
        completed,
        total,
    });

    Ok(())
}

/// How much has to arrive before the screen is told about it again.
const REPORT_EVERY: u64 = 4 * 1024 * 1024;

/// Does this file start like a GGUF model?
async fn looks_like_a_model(path: &Path) -> Result<bool, Error> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|source| storage(path, source))?;

    let mut magic = [0_u8; 4];
    match file.read_exact(&mut magic).await {
        Ok(_) => Ok(&magic == GGUF_MAGIC),
        // Too short to be a model, which is an answer rather than a failure.
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(source) => Err(storage(path, source)),
    }
}

fn storage(path: &Path, source: std::io::Error) -> Error {
    Error::Storage {
        path: path.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_source_is_a_pinned_https_url_on_the_model_host() {
        for model in CATALOGUE {
            let url = reqwest::Url::parse(model.source).expect("should be a URL");

            assert_eq!(url.scheme(), "https", "{}", model.name);
            assert_eq!(url.host_str(), Some(SOURCE_HOST), "{}", model.name);
            assert!(
                url.path().ends_with(model.file_name),
                "the URL and the file name should agree for {}: {url}",
                model.name
            );

            // Pinned to a revision, not a branch. `/resolve/main/` would let
            // the file change underneath an installation that already has it.
            assert!(
                !url.path().contains("/resolve/main/"),
                "{} is pinned to a branch rather than a revision",
                model.name
            );
        }
    }

    #[test]
    fn no_two_models_share_a_file_name() {
        // They land in one folder, so a shared name would have the light tier
        // load the standard model's weights, or the reverse.
        for (at, model) in CATALOGUE.iter().enumerate() {
            for other in &CATALOGUE[at + 1..] {
                assert_ne!(model.file_name, other.file_name);
            }
        }
    }

    #[test]
    fn the_tier_that_needs_less_gets_less() {
        assert!(
            for_tier(Tier::Light).approx_resident_mb < for_tier(Tier::Standard).approx_resident_mb,
            "the light tier exists to hold less"
        );
    }

    #[test]
    fn the_weights_live_under_the_apps_own_data_directory() {
        let data_dir = Path::new("/home/someone/.local/share/chief");

        assert_eq!(
            STANDARD.path(data_dir),
            Path::new("/home/someone/.local/share/chief/models").join(STANDARD.file_name)
        );
    }

    #[test]
    fn a_download_in_progress_sits_beside_the_finished_file() {
        let destination = STANDARD.path(Path::new("/data"));
        let partial = part_path(&destination);

        assert_eq!(partial.parent(), destination.parent());
        assert!(partial.to_string_lossy().ends_with(".part"));
    }

    #[tokio::test]
    async fn recognises_a_model_by_its_magic_bytes() {
        let dir = std::env::temp_dir().join(format!("chief-magic-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir)
            .await
            .expect("should create");

        let model = dir.join("model.gguf");
        tokio::fs::write(&model, b"GGUF\0\0\0\x03")
            .await
            .expect("should write");
        assert!(looks_like_a_model(&model).await.expect("should read"));

        // What a login wall or an error page looks like on disk.
        let html = dir.join("not-a-model.gguf");
        tokio::fs::write(&html, b"<!doctype html><title>Sign in</title>")
            .await
            .expect("should write");
        assert!(!looks_like_a_model(&html).await.expect("should read"));

        // And a transfer that stopped almost immediately.
        let stub = dir.join("truncated.gguf");
        tokio::fs::write(&stub, b"GG").await.expect("should write");
        assert!(!looks_like_a_model(&stub).await.expect("should read"));

        tokio::fs::remove_dir_all(&dir)
            .await
            .expect("should clean up");
    }

    #[test]
    fn reports_progress_the_way_the_setup_screen_reads_it() {
        let progress = DownloadProgress {
            status: "Downloading the model".to_string(),
            completed: 40,
            total: 100,
        };

        let body = serde_json::to_value(&progress).expect("should serialize");

        assert_eq!(body["status"], serde_json::json!("Downloading the model"));
        assert_eq!(body["completed"], serde_json::json!(40));
        assert_eq!(body["total"], serde_json::json!(100));
    }

    #[test]
    fn describes_the_model_for_the_setup_screen() {
        assert_eq!(STANDARD.describe(), "Llama 3.2 3B Instruct (Q4_K_M)");
        assert_eq!(LIGHT.describe(), "Llama 3.2 1B Instruct (Q4_K_M)");
    }

    /// The real download, against the real host.
    ///
    /// Everything else here is offline, because the one outbound request in
    /// this app should not be something a routine `cargo test` makes. But the
    /// pinned URL, the HTTPS-only redirect policy and the resume path are all
    /// promises about a host nobody here controls, and a stub server cannot
    /// keep them: Hugging Face answers `/resolve/` with a redirect to a CDN
    /// that may or may not honour a `Range` header. So this test exists, and
    /// is run deliberately:
    ///
    /// ```text
    /// cargo test --manifest-path src-tauri/Cargo.toml \
    ///   weights::tests::downloads_the_real_weights -- --ignored --nocapture
    /// ```
    ///
    /// It seeds a part file first, so the request it makes is the resume one —
    /// the harder of the two paths, and the one a dropped connection uses.
    #[tokio::test]
    #[ignore = "downloads two gigabytes from huggingface.co"]
    async fn downloads_the_real_weights_and_resumes_a_partial_one() {
        let dir = std::env::temp_dir().join(format!("chief-weights-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir)
            .await
            .expect("should create");

        // The ignored test exercises the real host, so it uses the model a
        // standard machine would actually fetch.
        let model = STANDARD;
        let destination = model.path(&dir);
        let partial = part_path(&destination);
        tokio::fs::create_dir_all(destination.parent().expect("has a parent"))
            .await
            .expect("should create");

        // A download that got 8 MiB in and then dropped.
        const SEEDED: u64 = 8 * 1024 * 1024;
        let head = reqwest::Client::new()
            .get(model.source)
            .header(reqwest::header::RANGE, format!("bytes=0-{}", SEEDED - 1))
            .send()
            .await
            .expect("should reach the model host")
            .bytes()
            .await
            .expect("should read the first megabytes");

        assert_eq!(head.len() as u64, SEEDED, "the host should honour a range");
        tokio::fs::write(&partial, &head)
            .await
            .expect("should write");

        let mut last = None;
        download(&model, &destination, |progress| last = Some(progress))
            .await
            .expect("should download the weights");

        let finished = last.expect("should have reported progress");
        assert_eq!(finished.status, "Done");
        assert_eq!(
            finished.completed, finished.total,
            "a finished download should have reported the whole file"
        );

        let written = tokio::fs::metadata(&destination)
            .await
            .expect("should be on disk")
            .len();

        assert_eq!(
            written, finished.total,
            "the file should be the size reported"
        );
        assert!(
            written > SEEDED,
            "the resumed download should have added to the part file"
        );
        assert!(
            looks_like_a_model(&destination).await.expect("should read"),
            "what arrived should be a GGUF model, not a login wall"
        );
        assert!(
            !partial.exists(),
            "the part file should have been renamed into place"
        );

        tokio::fs::remove_dir_all(&dir)
            .await
            .expect("should clean up");
    }
}
