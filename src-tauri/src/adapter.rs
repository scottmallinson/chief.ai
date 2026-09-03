//! The seam between how a question is answered and which model is loaded.
//!
//! The DLE specification's Tier 2 proposes a `ModelAdapter` trait, and two of
//! its three parts are worth having. This module builds those and refuses the
//! third, with the refusal written down so it is not proposed a third time.
//!
//! **What is refused: `supports_native_tools()`.** That is D1's `supports_tools`
//! flag under a different name, and the plan records it — at revision 4 — as
//! specified, not built, and *correctly* not built. The premise was that a
//! small model cannot be trusted with a tool loop; measured, the 1B called
//! tools correctly on 3 of 3 attempts where a tool fitted the question. The
//! real failure is a malformed generation, and [`crate::agent`] already handles
//! that by retrying without the catalogue. A capability flag would be a
//! permanent branch in the prompt path standing in for a problem that turned
//! out not to exist.
//!
//! **Also refused: a `GrammarConstrainedJson` strategy**, on different grounds.
//! It would have no users: `weights::CATALOGUE` holds two models and both are
//! Llama, which llama.cpp tool-calls natively. A path no catalogue entry
//! reaches cannot be tested against anything real, and a test that only ever
//! exercises a stub is how a guard comes to assert nothing. It is worth
//! building on the day a model that needs it enters the catalogue.
//!
//! **And the prompt is a list of messages, never a rendered string.** The
//! specification's `format_prompt(system, context, query) -> String` would
//! break tool calling outright: Chief speaks the OpenAI chat-completions
//! contract to `llama-server --jinja`, and a tool call goes through the
//! *model's own* chat template. A flat string bypasses the template, which is
//! the one thing that must not happen — so the return type is the guard.

use crate::context;
use crate::intent::Intent;
use crate::llama::Message;
use crate::probe::Tier;

/// How a question gets answered.
///
/// Chosen from the intent rather than from the model, which is the whole point:
/// what a question *is* decides how it is served, and which weights happen to
/// be loaded decides nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStrategy {
    /// The answer is assembled on this machine and injected — or, when the
    /// router can answer outright, never reaches a model at all. Every read
    /// question takes this path: that is D9.
    DirectContextInjection,
    /// The model is given the catalogue and decides what to look up. Anything
    /// the router does not recognise, which is everything open-ended.
    NativeToolCall,
}

/// Which strategy a question takes.
///
/// `None` — no recognised intent — is the tool loop, and deliberately so: a
/// question Chief does not recognise is one the model should be allowed to
/// work at, and the router's whole design is that a miss costs what the
/// question cost before the router existed.
#[must_use]
pub const fn strategy_for(intent: Option<Intent>) -> ExecutionStrategy {
    match intent {
        Some(Intent::Brief | Intent::Prep | Intent::Log) => {
            ExecutionStrategy::DirectContextInjection
        }
        None => ExecutionStrategy::NativeToolCall,
    }
}

/// What this machine's model can be asked to read.
///
/// Wraps [`Tier`] rather than introducing a parallel notion of the same fact.
/// D7 makes the context window a per-tier decision and `engine::arguments`
/// passes it to the server; this is the same number seen from the prompt side,
/// so a tier whose window ever fell below the prompt ceiling would shrink the
/// prompt rather than silently overrun the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Adapter {
    tier: Tier,
}

impl Adapter {
    #[must_use]
    pub const fn for_tier(tier: Tier) -> Self {
        Self { tier }
    }

    /// The most a prompt may cost on this machine.
    ///
    /// The lesser of what Chief is willing to spend and what the engine was
    /// started with. Today the ceiling is the smaller on both tiers, which is
    /// the answer being right rather than the comparison being pointless: the
    /// window is a per-tier decision and this is what keeps the two from
    /// drifting into disagreement.
    #[must_use]
    pub const fn max_prefill_tokens(self) -> u32 {
        let window = self.tier.context_size();

        if window < context::PROMPT_CEILING {
            window
        } else {
            context::PROMPT_CEILING
        }
    }

    /// Assemble the prompt, as messages.
    ///
    /// Delegates to [`crate::agent::conversation_within`], which owns the
    /// system prompt and the trimming — the adapter decides *how much*, not
    /// *what*. The return type is the guard the specification's flat string
    /// would have removed.
    #[must_use]
    pub fn assemble(self, turns: Vec<crate::agent::Turn>, present: &str) -> Vec<Message> {
        crate::agent::conversation_within(turns, present, self.max_prefill_tokens())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::agent::{Turn, PRESENT};
    use crate::llama::Role;

    fn turn(content: &str) -> Turn {
        Turn {
            role: Role::User,
            content: content.to_string(),
        }
    }

    /// Proved by routing `Prep` to the tool loop:
    ///
    /// ```text
    /// Prep is a read
    ///   left: NativeToolCall
    ///  right: DirectContextInjection
    /// ```
    #[test]
    fn a_read_is_answered_from_this_machine_and_anything_else_goes_to_the_tools() {
        for intent in [Intent::Brief, Intent::Prep, Intent::Log] {
            assert_eq!(
                strategy_for(Some(intent)),
                ExecutionStrategy::DirectContextInjection,
                "{intent:?} is a read"
            );
        }

        assert_eq!(
            strategy_for(None),
            ExecutionStrategy::NativeToolCall,
            "a question Chief does not recognise is one the model should work at"
        );
    }

    /// Asserted for **both** tiers, and against `Tier::context_size` rather
    /// than a literal, so a hard-coded window fails here even if somebody
    /// hard-codes the one a tier happens to want.
    ///
    /// Proved by returning a number of its own instead of consulting either:
    ///
    /// ```text
    /// Standard must never exceed what Chief is willing to spend
    /// ```
    #[test]
    fn the_prefill_budget_follows_the_tier_rather_than_a_number_of_its_own() {
        for tier in [Tier::Standard, Tier::Light] {
            let adapter = Adapter::for_tier(tier);

            assert!(
                adapter.max_prefill_tokens() <= tier.context_size(),
                "{tier:?} must never be asked to read more than it was started with"
            );
            assert!(
                adapter.max_prefill_tokens() <= context::PROMPT_CEILING,
                "{tier:?} must never exceed what Chief is willing to spend"
            );
        }

        // And it is the ceiling that binds today, on both — which is the answer
        // being right rather than the comparison being idle.
        assert_eq!(
            Adapter::for_tier(Tier::Light).max_prefill_tokens(),
            context::PROMPT_CEILING
        );
    }

    /// **The return type is the guard.** A rendered `String` would bypass the
    /// model's own chat template, which is the only thing that makes a tool
    /// call work under `--jinja`.
    ///
    /// This is a compile-time assertion first — the binding below does not
    /// typecheck against a function returning a string — and a shape assertion
    /// second.
    #[test]
    fn the_prompt_is_messages_and_never_a_rendered_string() {
        let assemble: fn(Adapter, Vec<Turn>, &str) -> Vec<Message> = Adapter::assemble;
        let messages = assemble(
            Adapter::for_tier(Tier::Standard),
            vec![turn("What did I ship?")],
            PRESENT,
        );

        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages.last().expect("a question").role, Role::User);
        assert_eq!(
            messages.last().expect("a question").content,
            "What did I ship?"
        );
    }

    /// A tier with a window narrower than the ceiling gets the window.
    ///
    /// Neither tier is that today, so the branch would otherwise never run —
    /// and a comparison nothing exercises is a comparison nobody can trust.
    #[test]
    fn a_narrow_window_would_shrink_the_prompt_rather_than_overrun_itself() {
        assert!(
            Tier::Light.context_size() > context::PROMPT_CEILING,
            "if this ever stops holding, the adapter starts choosing the window \
             and the assertion above has to say so"
        );
    }
}
