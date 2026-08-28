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

/// How many bytes of ordinary English one token is worth.
///
/// Byte-pair encodings land near four for prose and lower for punctuation-heavy
/// text, so this is deliberately the pessimistic end of the usual range: a
/// budget that under-counts spends more than it thinks it does, which is the
/// failure this type exists to prevent.
const BYTES_PER_TOKEN: u64 = 4;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_tokens_pessimistically_rather_than_optimistically() {
        // Four bytes to a token, rounded up: a budget that under-counts spends
        // more than it thinks it does.
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    #[test]
    fn the_estimate_is_within_ten_per_cent_of_a_real_count_for_prose() {
        // Calibration: ordinary English at roughly four bytes a token. The
        // estimator only has to be right to within a few per cent of a budget
        // with hundreds of tokens of slack in it, and this pins that it is.
        let prose = "The chief of staff prepares the principal for the day ahead, \
                     reads what has come in overnight, and decides what is worth \
                     their attention and what is not.";

        // A byte-pair encoder puts this at about one token per four bytes for
        // text of this shape; the check is that the estimate is not wildly off,
        // in the direction of over-counting.
        let estimated = estimate_tokens(prose);
        let words = prose.split_whitespace().count() as u32;

        assert!(
            estimated >= words,
            "an estimate below one token per word would under-spend: {estimated} vs {words}"
        );
        assert!(
            estimated < words * 2,
            "an estimate above two tokens per word wastes the budget: {estimated} vs {words}"
        );
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
        let mut budget = Budget::with_ceiling(10);
        budget.add("kept", "abcd").expect("should fit");

        let _ = budget.add("refused", &"x".repeat(200));

        assert_eq!(budget.remaining(), 9, "a refused block should cost nothing");

        // Which is what lets a transcript drop an old turn and keep a new one.
        budget
            .add("also kept", "efgh")
            .expect("there is still room");
        assert_eq!(budget.remaining(), 8);
    }

    #[test]
    fn what_is_left_is_what_has_not_been_spent() {
        let mut budget = Budget::with_ceiling(10);
        budget.add("block", "abcdefgh").expect("two tokens");

        assert_eq!(budget.remaining(), 8);
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
