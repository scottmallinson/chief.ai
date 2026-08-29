//! What this machine can actually run.
//!
//! Chief ships one model for every installation, and that model has to be
//! chosen before a gigabyte of it is downloaded. The decision cannot wait for a
//! measurement, because measuring throughput needs the weights the decision is
//! about — so it is made twice. This module owns the first half: read what the
//! machine says about itself, and pick the [`Tier`] that follows from it.
//!
//! The second half is measurement, once the engine is up, and it can only ever
//! confirm or lower a tier. Nothing here reports anything anywhere; the answer
//! is written to this machine's own database and shown to the person using it.

use std::fmt;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::llama::{self, ChatRequest, Message, Options};

/// Facts about the machine, as it reports them.
///
/// Deliberately two numbers. More would invite the tier rule to become a
/// scoring function nobody can predict the output of, and these are the two
/// that decide whether a 3B model at Q4 is pleasant or unusable: how much
/// memory it can hold without evicting the user's real work, and how many cores
/// are available to prefill a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Machine {
    /// Total physical memory, in mebibytes. Total rather than available: what
    /// is free right now says more about the browser than about the machine,
    /// and the tier is recorded once rather than recomputed per launch.
    pub memory_mb: u64,
    /// Logical cores usable by this process.
    pub cores: usize,
}

/// The memory below which the standard model is not worth offering.
///
/// Llama 3.2 3B at Q4_K_M is about 1.9 GB of weights, plus a KV cache that
/// reaches a few hundred megabytes at an 8192-token window — call it 2.4 GB
/// resident. On a 16 GB machine already running a browser, a chat client and an
/// editor that fits with room to spare. On 8 GB it does not, and the failure is
/// not a slow answer but the operating system swapping while the user is typing.
const STANDARD_MEMORY_MB: u64 = 15 * 1024;

/// The core count below which prefill is too slow to be worth the larger model.
///
/// Decoding is bound by memory bandwidth, but *prefill* — reading the prompt
/// before the first word appears — is bound by compute, and it is what a person
/// experiences as the wait. Two cores turn a few thousand tokens of context
/// into a minute of silence.
const STANDARD_CORES: usize = 4;

/// Which model this machine gets, and how the engine is configured for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Enough memory and enough cores for the model Chief would rather run.
    Standard,
    /// Everything else. A smaller model, a smaller window, and a smaller cache.
    Light,
}

impl Tier {
    /// The tier this machine qualifies for.
    ///
    /// A pure function of two numbers, so the rule can be tested without a
    /// machine that has the shape being tested for. Both thresholds must be met:
    /// memory and cores fail in different ways and neither substitutes for the
    /// other.
    #[must_use]
    pub const fn for_machine(machine: Machine) -> Self {
        if machine.memory_mb >= STANDARD_MEMORY_MB && machine.cores >= STANDARD_CORES {
            Self::Standard
        } else {
            Self::Light
        }
    }

    /// The context window the engine is started with on this tier.
    ///
    /// This is the *engine's* window, not Chief's prompt budget. A larger window
    /// buys room for a longer conversation and a longer answer; it does not
    /// license a larger injection, because prefill cost scales with what is put
    /// in it.
    #[must_use]
    pub const fn context_size(self) -> u32 {
        match self {
            Self::Standard => 8192,
            Self::Light => 4096,
        }
    }

    /// How much memory the engine may hold in prompt caches, in mebibytes.
    ///
    /// llama.cpp defaults this to 8192 MiB, which is a reasonable ceiling on a
    /// server and a hazard on the machine Chief is written for: it is more than
    /// twice the headroom a 16 GB workplace laptop has after the browser and the
    /// chat client, and it is spent on top of the resident model. Bounding it is
    /// the difference between a cache that saves prefill and one that causes the
    /// swapping it was meant to avoid.
    #[must_use]
    pub const fn cache_ram_mb(self) -> u32 {
        match self {
            Self::Standard => 1024,
            Self::Light => 512,
        }
    }

    /// The value stored in `settings`, and shown by `chief doctor`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Light => "light",
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Machine {
    /// Ask this machine about itself.
    ///
    /// `sysinfo` is used rather than three hand-written platform calls because
    /// the Windows one would be compiled on Windows and nowhere else, and
    /// CLAUDE.md records that as where the last two bugs on `main` came from.
    #[must_use]
    pub fn detect() -> Self {
        let mut system = sysinfo::System::new();
        system.refresh_memory();

        Self {
            memory_mb: system.total_memory() / (1024 * 1024),
            cores: std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Machine, Tier};

    fn machine(memory_gb: u64, cores: usize) -> Machine {
        Machine {
            memory_mb: memory_gb * 1024,
            cores,
        }
    }

    #[test]
    fn a_typical_workplace_laptop_gets_the_standard_tier() {
        assert_eq!(Tier::for_machine(machine(16, 8)), Tier::Standard);
    }

    #[test]
    fn too_little_memory_drops_to_light_however_many_cores_there_are() {
        assert_eq!(Tier::for_machine(machine(8, 16)), Tier::Light);
    }

    #[test]
    fn too_few_cores_drops_to_light_however_much_memory_there_is() {
        assert_eq!(Tier::for_machine(machine(64, 2)), Tier::Light);
    }

    #[test]
    fn a_machine_reporting_slightly_under_sixteen_gigabytes_still_qualifies() {
        // Sixteen gigabytes of hardware never reports sixteen gigabytes to the
        // operating system, so a threshold of exactly 16 GiB would put every
        // machine this tier was written for on the wrong side of it.
        assert_eq!(Tier::for_machine(machine(15, 4)), Tier::Standard);
    }

    #[test]
    fn the_standard_tier_gets_the_larger_window_and_the_light_tier_the_smaller() {
        assert_eq!(Tier::Standard.context_size(), 8192);
        assert_eq!(Tier::Light.context_size(), 4096);
    }

    #[test]
    fn neither_tier_lets_the_prompt_cache_reach_the_engines_own_default() {
        // llama.cpp defaults to 8192 MiB. Both tiers must be well under it, or
        // the cache costs more than the prefill it saves.
        assert!(Tier::Standard.cache_ram_mb() < 8192);
        assert!(Tier::Light.cache_ram_mb() < Tier::Standard.cache_ram_mb());
    }

    #[tokio::test]
    async fn nothing_is_remembered_before_anything_is_measured() {
        let pool = crate::db::test_support::migrated_pool().await;

        assert_eq!(
            super::remembered(&pool).await.expect("should read"),
            None,
            "a machine that has never been measured has no measurement"
        );
    }

    #[tokio::test]
    async fn a_measurement_survives_being_written_and_read_back() {
        let pool = crate::db::test_support::migrated_pool().await;
        let taken = super::Measurement {
            first_token_ms: 1_200,
            total_ms: 5_200,
            characters: 400,
        };

        super::remember(&pool, taken).await.expect("should write");

        assert_eq!(
            super::remembered(&pool).await.expect("should read"),
            Some(taken)
        );
    }

    #[tokio::test]
    async fn measuring_again_replaces_the_last_one_rather_than_stacking() {
        let pool = crate::db::test_support::migrated_pool().await;
        let slow = super::Measurement {
            first_token_ms: 9_000,
            total_ms: 20_000,
            characters: 100,
        };
        let quick = super::Measurement {
            first_token_ms: 300,
            total_ms: 1_300,
            characters: 400,
        };

        super::remember(&pool, slow).await.expect("should write");
        super::remember(&pool, quick).await.expect("should write");

        assert_eq!(
            super::remembered(&pool).await.expect("should read"),
            Some(quick),
            "the settings screen should show what this machine does now"
        );

        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM settings")
            .fetch_one(&pool)
            .await
            .expect("should count");
        assert_eq!(rows, 1, "a key holds one value, not a history");
    }

    #[test]
    fn the_rate_counts_only_the_time_spent_writing() {
        // Four seconds of decoding after a one-second wait for the first
        // character: the rate is 400 characters over four seconds, not five.
        let measurement = super::Measurement {
            first_token_ms: 1_000,
            total_ms: 5_000,
            characters: 400,
        };

        assert!((measurement.characters_per_second() - 100.0).abs() < 0.001);
    }

    #[test]
    fn an_answer_that_never_started_reports_no_rate_rather_than_dividing_by_zero() {
        let measurement = super::Measurement {
            first_token_ms: 3_000,
            total_ms: 3_000,
            characters: 0,
        };

        assert!((measurement.characters_per_second() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn this_machine_reports_something_believable() {
        let machine = Machine::detect();

        assert!(machine.cores >= 1, "a machine has at least one core");
        assert!(
            machine.memory_mb >= 512,
            "a machine running this test has at least half a gigabyte, got {}",
            machine.memory_mb
        );
    }
}

/// What this machine did when it was asked to answer something.
///
/// Wall-clock rather than the engine's own counters, because those are not
/// something every build reports and a number that is sometimes absent is worse
/// than one that is always honest. The split that matters is preserved either
/// way: time to the *first* character is prefill — reading the prompt — and
/// everything after it is decoding, and on a CPU those two are bound by
/// different things and fail differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Measurement {
    /// Milliseconds from asking to the first character coming back. This is the
    /// number a person experiences as "did it hear me".
    pub first_token_ms: u64,
    /// Milliseconds for the whole answer.
    pub total_ms: u64,
    /// Characters written. A proxy for tokens that needs no tokenizer, and the
    /// ratio between them is close enough to constant for one model.
    pub characters: usize,
}

impl Measurement {
    /// Roughly how fast the answer was written, once it started.
    #[must_use]
    pub fn characters_per_second(self) -> f64 {
        let decoding = self.total_ms.saturating_sub(self.first_token_ms);

        if decoding == 0 || self.characters == 0 {
            return 0.0;
        }

        #[allow(clippy::cast_precision_loss)]
        {
            (self.characters as f64) / (decoding as f64 / 1000.0)
        }
    }
}

/// The prompt the measurement is taken with.
///
/// Short, so the measurement costs a second or two rather than a minute, and
/// fixed, so two runs on the same machine are comparable. It asks for prose
/// rather than a fact, because a model that answers "yes" has measured nothing.
const MEASURE_PROMPT: &str = "In two sentences, describe what a chief of staff does.";

/// How long the measuring answer may run before it is cut off.
const MEASURE_TOKENS: u32 = 64;

/// Ask the engine to answer something, and time it.
///
/// The engine must already be running; this does not start it. Callers that
/// might be looking at a stopped engine wake it first.
pub async fn measure(client: &llama::Client, model: &str) -> Result<Measurement, llama::Error> {
    let request = ChatRequest::new(model, vec![Message::user(MEASURE_PROMPT)])
        .with_options(Options::new().with_answer_length(MEASURE_TOKENS));

    let started = Instant::now();
    let mut first_token_at = None;
    let mut characters = 0usize;

    client
        .chat_stream(&request, |token| {
            if first_token_at.is_none() {
                first_token_at = Some(started.elapsed());
            }
            characters += token.chars().count();
        })
        .await?;

    let total = started.elapsed();

    Ok(Measurement {
        first_token_ms: u64::try_from(first_token_at.unwrap_or(total).as_millis())
            .unwrap_or(u64::MAX),
        total_ms: u64::try_from(total.as_millis()).unwrap_or(u64::MAX),
        characters,
    })
}

/// The key the last measurement is stored under.
const MEASUREMENT_KEY: &str = "probe.measurement";

/// Remember a measurement, so the next person to open the settings screen is
/// not made to wait for a fresh one.
pub async fn remember(pool: &SqlitePool, measurement: Measurement) -> Result<(), sqlx::Error> {
    let value = serde_json::to_string(&measurement).unwrap_or_default();

    crate::settings::set(pool, MEASUREMENT_KEY, &value).await
}

/// What was measured last, if anything ever was.
pub async fn remembered(pool: &SqlitePool) -> Result<Option<Measurement>, sqlx::Error> {
    let stored = crate::settings::get(pool, MEASUREMENT_KEY).await?;

    Ok(stored.and_then(|value| serde_json::from_str(&value).ok()))
}

/// Everything `chief doctor` has to say about this machine.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// `standard` or `light`.
    pub tier: String,
    /// The model that tier runs, named for a person to read.
    pub model: String,
    /// Roughly what it holds once loaded, in mebibytes.
    pub model_size_mb: u32,
    /// The window the engine was started with.
    pub context_size: u32,
    /// What the machine says it has.
    pub memory_mb: u64,
    /// Cores available to this process.
    pub cores: usize,
    /// What it did when it was last asked to answer something. `None` before
    /// anything has been measured, which is not a failure — it is a machine
    /// that has not been asked yet.
    pub measurement: Option<Measurement>,
    /// Roughly how fast the answer was written, once it started. Derived here
    /// rather than in the interface, so the arithmetic has one home.
    pub characters_per_second: Option<f64>,
}

/// Errors `chief doctor` surfaces.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Engine(#[from] llama::Error),
    #[error(transparent)]
    Starting(#[from] crate::engine::Error),
    #[error(transparent)]
    Storage(#[from] crate::db::Error),
    #[error("could not read what this machine measured last: {0}")]
    Stored(String),
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        Self::Stored(error.to_string())
    }
}

/// What this machine is, and what it can do.
///
/// `remeasure` decides whether to spend a generation finding out again. The
/// settings screen opens with `false` and shows whatever was last recorded;
/// pressing the button asks for `true`. Measuring wakes the engine, because a
/// measurement taken against a stopped engine would be timing a failure.
#[tauri::command]
pub async fn run_doctor<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    engine: tauri::State<'_, crate::engine::Engine>,
    client: tauri::State<'_, llama::Client>,
    attention: tauri::State<'_, crate::agent::Attention>,
    remeasure: bool,
) -> Result<Report, Error> {
    let machine = Machine::detect();
    let tier = engine.tier();
    let model = engine.model();
    let pool = crate::db::pool(&app).await?;

    let measurement = if remeasure {
        // Hold the door: this is a person waiting on an answer like any other,
        // and the idle supervisor must not stop the engine mid-measurement.
        let _waiting = attention.begin();

        engine.start_and_wait(client.inner()).await?;
        let taken = measure(client.inner(), crate::agent::DEFAULT_MODEL).await?;
        remember(&pool, taken).await?;
        Some(taken)
    } else {
        remembered(&pool).await?
    };

    Ok(Report {
        tier: tier.to_string(),
        model: model.describe(),
        model_size_mb: model.approx_resident_mb,
        context_size: tier.context_size(),
        memory_mb: machine.memory_mb,
        cores: machine.cores,
        characters_per_second: measurement.map(Measurement::characters_per_second),
        measurement,
    })
}
