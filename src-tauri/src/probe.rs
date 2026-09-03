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
use std::num::NonZeroUsize;
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
    /// Physical cores, not logical ones.
    ///
    /// Hyper-threading does not help inference — the work is dense arithmetic
    /// over memory the two siblings share — which is why llama.cpp itself
    /// defaults its thread count to the physical count. Counting logical cores
    /// would tell Chief this machine is twice the machine it is: the 2014 Mac
    /// mini this was measured on reports four, has two, and ran the engine on
    /// two threads throughout while the tier believed it had four.
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

        // `available_parallelism` counts logical cores, which on anything with
        // hyper-threading is twice the number that decides how fast a prompt is
        // read. Fall back to it only when the physical count is unavailable —
        // over-counting is better than assuming a single core.
        let cores = system
            .physical_core_count()
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, NonZeroUsize::get));

        Self {
            memory_mb: system.total_memory() / (1024 * 1024),
            cores,
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
            warm_first_token_ms: None,
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
            warm_first_token_ms: None,
            total_ms: 20_000,
            characters: 100,
        };
        let quick = super::Measurement {
            first_token_ms: 300,
            warm_first_token_ms: None,
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
            warm_first_token_ms: None,
            total_ms: 5_000,
            characters: 400,
        };

        assert!((measurement.characters_per_second() - 100.0).abs() < 0.001);
    }

    #[test]
    fn an_answer_that_never_started_reports_no_rate_rather_than_dividing_by_zero() {
        let measurement = super::Measurement {
            first_token_ms: 3_000,
            warm_first_token_ms: None,
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
    /// `chief doctor` states the arithmetic rather than a guess, because the
    /// specification asked for a cap llama.cpp does not have. No wall clock is
    /// involved, so this cannot fail on a slow runner.
    #[test]
    fn reports_what_the_context_window_costs() {
        use crate::weights::{KV_BYTES_F16, KV_BYTES_Q8, STANDARD};

        assert_eq!(super::kv_cache_mb(STANDARD, 8192, KV_BYTES_F16), 896);
        assert_eq!(super::kv_cache_mb(STANDARD, 4096, KV_BYTES_F16), 448);
        assert_eq!(super::kv_cache_mb(STANDARD, 8192, KV_BYTES_Q8), 448);
    }

    /// A measurement stored before the warm figure existed must still read
    /// back. The last one lives in `settings` as JSON, and an upgrade throwing
    /// it away would make the settings screen re-measure for no reason.
    #[test]
    fn a_measurement_stored_before_the_warm_figure_still_reads() {
        let stored = r#"{"firstTokenMs":1200,"totalMs":5000,"characters":300}"#;

        let measurement: super::Measurement =
            serde_json::from_str(stored).expect("an older measurement should still read");

        assert_eq!(measurement.first_token_ms, 1_200);
        assert_eq!(
            measurement.warm_first_token_ms, None,
            "absent is not zero: a figure never taken must not read as instant"
        );
    }

    /// The two figures are separate on the way out as well as in, so the
    /// interface can say which is which.
    #[test]
    fn the_warm_figure_survives_being_written_down() {
        let taken = super::Measurement {
            first_token_ms: 9_000,
            warm_first_token_ms: Some(1_400),
            total_ms: 20_000,
            characters: 300,
        };

        let round_tripped: super::Measurement =
            serde_json::from_str(&serde_json::to_string(&taken).expect("should write"))
                .expect("should read");

        assert_eq!(round_tripped, taken);
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
    ///
    /// A **cold** prefix: nothing in the engine's cache matched, so this is the
    /// whole prompt being read.
    pub first_token_ms: u64,
    /// The same, asked a second time against the prefix the first left in the
    /// cache.
    ///
    /// **This is the number the DLE specification's ≤1.5 s target could
    /// sensibly be about.** Cold, 2,000 tokens at the 18–34 tokens a second a
    /// CPU manages is 60 to 110 seconds — two orders of magnitude off. Warm,
    /// only the part of the prompt that changed is read, which is what
    /// `--cache-reuse` and the stable-parts-first assembly exist for. The gap
    /// between the two is what those two things are worth on this machine.
    ///
    /// `None` for a measurement taken before this field existed, which is why
    /// it is `serde(default)`: the last one is kept in `settings` as JSON, and
    /// an upgrade must not throw it away.
    #[serde(default)]
    pub warm_first_token_ms: Option<u64>,
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

    let cold = time_one(client, &request).await?;

    // The same prompt again, against the prefix the first pass left behind.
    // Asking twice is what makes the warm figure exist at all: there is no way
    // to observe a cache hit except by producing one, and the two together are
    // what `--cache-reuse` is worth on this machine. It costs a second answer
    // of at most `MEASURE_TOKENS`, which is the price of the only number the
    // specification's target could sensibly have meant.
    let warm = time_one(client, &request).await.ok();

    Ok(Measurement {
        first_token_ms: cold.first_token_ms,
        warm_first_token_ms: warm.map(|warm| warm.first_token_ms),
        total_ms: cold.total_ms,
        characters: cold.characters,
    })
}

/// One timed answer.
async fn time_one(client: &llama::Client, request: &ChatRequest) -> Result<Timing, llama::Error> {
    let started = Instant::now();
    let mut first_token_at = None;
    let mut characters = 0usize;

    client
        .chat_stream(request, |token| {
            if first_token_at.is_none() {
                first_token_at = Some(started.elapsed());
            }
            characters += token.chars().count();
        })
        .await?;

    let total = started.elapsed();

    Ok(Timing {
        first_token_ms: u64::try_from(first_token_at.unwrap_or(total).as_millis())
            .unwrap_or(u64::MAX),
        total_ms: u64::try_from(total.as_millis()).unwrap_or(u64::MAX),
        characters,
    })
}

/// What one timed answer produced, before the two are folded together.
#[derive(Debug, Clone, Copy)]
struct Timing {
    first_token_ms: u64,
    total_ms: u64,
    characters: usize,
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
    /// What the KV cache costs at this context window, in mebibytes.
    ///
    /// Shown because the DLE specification asked for resident memory to be
    /// *capped* and llama.cpp has no such thing: it is chosen, and this is the
    /// half of the choice the context window makes. `chief doctor` states the
    /// arithmetic so the trade is somebody's to make rather than a surprise.
    pub kv_cache_mb: u32,
    /// Weights plus that cache — what the engine actually holds.
    pub resident_mb: u32,
    /// The same with `--cache-type-k/v q8_0`, which Chief does not pass yet.
    /// Reported so the lever is visible before it is needed.
    pub resident_mb_q8: u32,
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

    let context_size = tier.context_size();

    Ok(Report {
        tier: tier.to_string(),
        model: model.describe(),
        model_size_mb: model.approx_resident_mb,
        context_size,
        memory_mb: machine.memory_mb,
        cores: machine.cores,
        characters_per_second: measurement.map(Measurement::characters_per_second),
        measurement,
        kv_cache_mb: kv_cache_mb(model, context_size, crate::weights::KV_BYTES_F16),
        resident_mb: crate::weights::resident_mb(model, context_size, crate::weights::KV_BYTES_F16),
        resident_mb_q8: crate::weights::resident_mb(
            model,
            context_size,
            crate::weights::KV_BYTES_Q8,
        ),
    })
}

/// What a full KV cache costs at this window, in mebibytes.
#[must_use]
pub const fn kv_cache_mb(model: crate::weights::Model, context_size: u32, kv_bytes: u32) -> u32 {
    let bytes = crate::weights::kv_bytes_per_token(model, kv_bytes) * (context_size as u64);

    (bytes / (1024 * 1024)) as u32
}
