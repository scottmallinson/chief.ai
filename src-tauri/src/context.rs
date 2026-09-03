//! What may be put in a prompt, and how much of it.
//!
//! The governing cost of a feature here is not the model's size but the length
//! of the prompt: decoding is bound by memory bandwidth and scales with the
//! weights, while **prefill** — reading the prompt before the first word comes
//! back — is bound by compute and scales with what was put in it. On a CPU a
//! few thousand tokens of context is tens of seconds before anything appears.
//!
//! So the budget is a type rather than a convention. [`Budget`] refuses the
//! block that would take a prompt over its ceiling, instead of handing the
//! engine something it will silently truncate: a truncated prompt fails as a
//! confidently wrong answer, and a refused one fails as an error somebody can
//! fix.

/// How many bytes of text one token is worth.
///
/// Three, not four, and the difference matters. Four is the familiar figure for
/// English prose and it is wrong for the material this budget actually has to
/// hold back. Measured against this model's own tokenizer:
///
/// | sample                      | bytes/token |
/// |-----------------------------|-------------|
/// | a GitHub tool result (JSON) | 3.09        |
/// | corpus markdown             | 3.83        |
/// | English prose               | 4.93        |
///
/// URLs, punctuation and repeated JSON keys fragment far worse than prose, so a
/// four-byte assumption under-counted a tool result by 23% — and a tool result
/// is the largest, least bounded thing that becomes prompt. Under-counting is
/// the one direction this type must never err in: it spends more than it thinks
/// it has, which is the failure the budget exists to prevent. Three over-counts
/// prose, which wastes a little room and is safe.
const BYTES_PER_TOKEN: u64 = 3;

/// Roughly what a file of `bytes` costs to put in a prompt.
///
/// An estimate rather than a count: a real tokenizer here would mean carrying
/// the model's vocabulary around to answer a question whose answer only has to
/// be right to within a few per cent of a budget with hundreds of tokens of
/// slack in it.
#[must_use]
pub fn estimate_tokens_in_bytes(bytes: u64) -> u32 {
    u32::try_from(bytes.div_ceil(BYTES_PER_TOKEN)).unwrap_or(u32::MAX)
}

/// Roughly what this text costs to put in a prompt.
#[must_use]
pub fn estimate_tokens(text: &str) -> u32 {
    estimate_tokens_in_bytes(text.len() as u64)
}

/// What went wrong assembling a prompt.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error(
        "the prompt needs about {wanted} tokens and there is room for {ceiling}: \
         '{block}' does not fit"
    )]
    OverBudget {
        block: String,
        wanted: u32,
        ceiling: u32,
    },
}

/// The most context any one prompt may carry.
///
/// Chosen against the slower of the two hardware tiers rather than the faster:
/// a budget that only holds on a good machine is not a budget. It is the same
/// on both tiers — a larger context window buys room for a longer conversation
/// and a longer answer, not a larger injection, because prefill is paid on what
/// is put in.
pub const DEFAULT_CEILING: u32 = 2_400;

/// The most context a *read question* may inject.
///
/// Deliberately a different number from [`DEFAULT_CEILING`], and deliberately
/// named beside it so neither silently absorbs the other. A brief assembles
/// corpus files, a calendar and pull requests and needs the larger figure; a
/// read question needs a handful of rows and should cost almost nothing, so
/// that "what did I ship this week" is answered in the time a query takes.
///
/// **300 is only useful because the URL is left out.** A link is a large part
/// of what a row costs, so leaving it out fits about half as many rows again —
/// measured, 15 against 10 — which is the difference between an answer about a
/// week and a handful of rows. The interface renders the link from
/// `work_logs.url` instead, which also means the model cannot invent one.
///
/// **How many rows that is depends on the estimator, not just the format.**
/// Measured through [`estimate_tokens`], which is 3 bytes per token and still
/// uncalibrated (plan §9), 300 tokens holds **15** rows of realistic length. An
/// earlier note here said 20, from a real-tokenizer estimate; the estimator
/// that actually enforces the gate is more pessimistic than that, and it is the
/// one that decides. `retrieval::leaving_the_link_out_is_what_makes_the_ceiling_workable`
/// asserts the ratio rather than either number, so calibrating the estimator
/// later cannot quietly invalidate it.
pub const RETRIEVAL_CEILING: u32 = 300;

/// The most a **chat prompt** may cost, once assembled.
///
/// The third named ceiling, and the one a person waits behind. `/brief` writes
/// a file nobody is watching, so [`DEFAULT_CEILING`]'s extra 400 tokens buys a
/// better brief at no cost anyone feels; a question in the composer is somebody
/// sitting still while prefill runs, and on a CPU each of those 400 tokens is
/// somewhere between 12 and 22 seconds of it.
///
/// **The DLE specification's `≤2,000 tokens total prompt` is this number, and
/// it is the half of that requirement that is real.** The other half — TTFT
/// ≤1.5 s at 2,000 tokens — is off by roughly two orders of magnitude on a cold
/// prefill: at the 18–34 tokens a second this machine manages, 2,000 tokens is
/// 60 to 110 seconds. The figure only means anything for the volatile suffix
/// past a cached prefix, which is why `agent::conversation` puts the stable
/// parts first and why `engine::arguments` passes `--cache-reuse`. It is
/// measured by `chief doctor` and asserted by nothing: a CI runner is not the
/// machine the number is about.
pub const PROMPT_CEILING: u32 = 2_000;

/// A ledger for what a prompt is allowed to cost.
///
/// Not an assembler: it records what has been spent and refuses the block that
/// would go over, leaving the caller to build whatever shape it needs — a list
/// of chat messages here, a single structured prompt when recipes arrive. What
/// is shared is the arithmetic and the refusal, which is the part that must not
/// be reimplemented per caller.
///
/// Callers are expected to add the stable blocks first. llama.cpp reuses the
/// cached prefix of a prompt it has seen before, so a system prompt that does
/// not change between turns is prefill paid once rather than every time —
/// putting the volatile parts last is what makes that possible, and it is free.
#[derive(Debug, Clone)]
pub struct Budget {
    ceiling: u32,
    spent: u32,
}

impl Budget {
    /// A budget with the default ceiling.
    #[must_use]
    pub fn new() -> Self {
        Self::with_ceiling(DEFAULT_CEILING)
    }

    #[must_use]
    pub const fn with_ceiling(ceiling: u32) -> Self {
        Self { ceiling, spent: 0 }
    }

    /// Charge a block against the budget, or refuse it.
    ///
    /// A refused block costs nothing: the caller can carry on adding smaller
    /// things, which is what lets a transcript drop its oldest turns and keep
    /// the newest. Named, so the error says which block did not fit rather than
    /// only that something did not.
    pub fn add(&mut self, name: &str, text: &str) -> Result<(), Error> {
        let wanted = self.spent + estimate_tokens(text);

        if wanted > self.ceiling {
            return Err(Error::OverBudget {
                block: name.to_string(),
                wanted,
                ceiling: self.ceiling,
            });
        }

        self.spent = wanted;

        Ok(())
    }

    /// What is left.
    #[must_use]
    pub const fn remaining(&self) -> u32 {
        self.ceiling.saturating_sub(self.spent)
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self::new()
    }
}

/// Cut `text` down to roughly `tokens` worth, and say that it was cut.
///
/// The beginning is what survives. For a pasted message that is where a person
/// puts what they want done with the thing they pasted; for a tool result it is
/// the first and most relevant rows. `note` is not decoration: a model reading
/// part of something and told nothing will answer as though it read all of it.
#[must_use]
pub fn fit(text: &str, tokens: u32, note: &str) -> String {
    // Leave room for the note itself, and never fall to nothing.
    let room = tokens.saturating_sub(estimate_tokens(note)).max(32);
    let mut cut = text.len().min(room as usize * BYTES_PER_TOKEN as usize);

    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }

    if cut >= text.len() {
        return text.to_string();
    }

    format!("{}{note}", &text[..cut])
}

/// Strip what costs tokens and carries nothing.
///
/// The blueprint calls this `slimtoken`. Markdown written by people is full of
/// runs of blank lines and trailing spaces that mean something to an editor and
/// nothing to a model — and every one of them is prefill on a CPU. What it does
/// *not* do is touch the words, reflow paragraphs or strip markdown syntax:
/// headings and list markers are structure the model reads, and a minifier that
/// changed meaning to save fifty tokens would be a bad trade.
#[must_use]
pub fn slim(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0;

    for line in text.lines() {
        let trimmed = line.trim_end();

        if trimmed.trim().is_empty() {
            blank_run += 1;
            // One blank line separates; more is whitespace.
            if blank_run > 1 {
                continue;
            }
            out.push('\n');
            continue;
        }

        blank_run = 0;
        out.push_str(trimmed);
        out.push('\n');
    }

    // No leading or trailing blank lines: pure cost at the join.
    out.trim_matches('\n').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_tokens_pessimistically_rather_than_optimistically() {
        // Expressed against the estimator rather than against a byte count, so
        // recalibrating the ratio does not require rewriting the test.
        assert_eq!(estimate_tokens(""), 0);
        assert!(estimate_tokens("a") >= 1, "anything at all costs something");

        // Rounded up, never down: a partial token is a token.
        let one = estimate_tokens_in_bytes(BYTES_PER_TOKEN);
        assert_eq!(estimate_tokens_in_bytes(BYTES_PER_TOKEN + 1), one + 1);

        // Monotonic — more text never estimates as less.
        assert!(estimate_tokens("aa") >= estimate_tokens("a"));
    }

    #[test]
    fn the_estimate_never_under_counts_the_material_the_budget_has_to_hold_back() {
        // Measured against this model's own tokenizer, not assumed. The numbers
        // are what a real GitHub tool result and a real corpus file cost, and
        // the estimate has to be at or above them — under-counting is the one
        // direction that overflows a context window.
        for (name, bytes, real) in [
            ("GitHub tool result", 7457_u64, 2411_u32),
            ("corpus markdown", 352, 92),
            ("prose", 616, 125),
        ] {
            let estimated = estimate_tokens_in_bytes(bytes);

            assert!(
                estimated >= real,
                "{name}: estimated {estimated} for {real} real tokens — under-counting overflows"
            );
        }
    }

    #[test]
    fn the_estimate_is_not_so_cautious_that_it_wastes_the_budget() {
        // Over-counting is safe but not free: doubling the estimate would halve
        // what fits. Prose is the worst case, and it must stay within reason.
        assert!(estimate_tokens_in_bytes(616) < 125 * 2);
    }

    #[test]
    fn refuses_an_over_budget_assembly_rather_than_truncating_it() {
        let mut budget = Budget::with_ceiling(10);

        budget
            .add("small", "abcd")
            .expect("four bytes is one token");

        let error = budget
            .add("large", &"x".repeat(200))
            .expect_err("two hundred bytes does not fit in ten tokens");

        assert!(matches!(error, Error::OverBudget { .. }));

        // And the block that did not fit is named, so the failure is fixable.
        assert!(error.to_string().contains("large"), "{error}");
    }

    #[test]
    fn a_refused_block_costs_nothing_so_a_smaller_one_can_still_fit() {
        let mut budget = Budget::with_ceiling(100);
        budget.add("kept", "a short block").expect("should fit");
        let after_first = budget.remaining();

        let _ = budget.add("refused", &"x".repeat(10_000));

        assert_eq!(
            budget.remaining(),
            after_first,
            "a refused block should cost nothing"
        );

        // Which is what lets a transcript drop an old turn and keep a new one.
        budget
            .add("also kept", "another block")
            .expect("there is still room");
        assert!(budget.remaining() < after_first);
    }

    #[test]
    fn what_is_left_is_what_has_not_been_spent() {
        let mut budget = Budget::with_ceiling(100);
        let block = "some text of a known size";
        budget.add("block", block).expect("should fit");

        assert_eq!(budget.remaining(), 100 - estimate_tokens(block));
    }

    #[test]
    fn the_minifier_drops_what_costs_tokens_and_carries_nothing() {
        let messy = "# Heading   \n\n\n\nSome text.   \n\n\n- a point\n- another   \n\n\n";

        assert_eq!(
            slim(messy),
            "# Heading\n\nSome text.\n\n- a point\n- another"
        );
    }

    #[test]
    fn the_minifier_leaves_the_structure_a_model_reads() {
        // Headings, list markers and code fences are meaning, not decoration.
        let text = "# Title\n\n- one\n- two\n\n```rust\nlet x = 1;\n```";

        assert_eq!(slim(text), text, "structure should survive untouched");
    }

    #[test]
    fn the_minifier_is_idempotent() {
        let messy = "#  Heading \n\n\n\ntext\n\n\n";
        let once = slim(messy);

        assert_eq!(slim(&once), once, "slimming twice should change nothing");
    }

    #[test]
    fn the_ceiling_holds_on_the_tier_that_cannot_afford_more() {
        // Both tiers get the same injection budget. The larger context window
        // on the standard tier buys a longer conversation and a longer answer,
        // not more corpus, because prefill is paid on what is put in.
        assert!(
            DEFAULT_CEILING < crate::probe::Tier::Light.context_size(),
            "the budget has to leave room for the question and the answer"
        );
    }
}
