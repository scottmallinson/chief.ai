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
