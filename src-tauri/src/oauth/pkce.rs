//! PKCE (RFC 7636) and the `state` nonce.
//!
//! Pure: no I/O, no clock, no provider, so every branch is testable against the
//! specification's own vectors. Only `S256` exists here — RFC 8252 §8.1 makes
//! PKCE mandatory for a native public client, and `plain` protects nothing.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

/// The secret half of a PKCE exchange: sent with the code, never before it.
#[derive(Clone, PartialEq, Eq)]
pub struct Verifier(String);

/// The one genuinely secret value here, so it never renders itself: derived
/// `Debug` would carry it into any error or trace that formats a struct
/// holding one.
impl std::fmt::Debug for Verifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Verifier(<redacted>)")
    }
}

impl Verifier {
    /// A fresh verifier from the operating system's CSPRNG.
    ///
    /// 32 random bytes base64url-encode to 43 characters — the shortest RFC
    /// 7636 §4.1 permits, and a full 256 bits of entropy.
    pub fn generate() -> Self {
        Self(URL_SAFE_NO_PAD.encode(random_bytes()))
    }

    /// Rebuild a verifier held between the two halves of a sign-in.
    pub fn from_stored(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// What goes in the token request.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// What goes in the authorization request: BASE64URL(SHA256(verifier)).
    pub fn challenge(&self) -> String {
        URL_SAFE_NO_PAD.encode(Sha256::digest(self.0.as_bytes()))
    }
}

/// A single-use value echoed back by the authorization server, checked before
/// the code is exchanged so another site cannot feed us one.
///
/// Deliberately not `PartialEq`: `matches` is the one way to compare a state,
/// and `==` would be a second route that short-circuits on the first differing
/// byte.
#[derive(Debug, Clone)]
pub struct State(String);

impl State {
    pub fn generate() -> Self {
        Self(URL_SAFE_NO_PAD.encode(random_bytes()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether a returned value is the one we sent. Compared without an early
    /// exit, which costs nothing at this length and avoids thinking about it.
    pub fn matches(&self, returned: &str) -> bool {
        self.0.len() == returned.len()
            && self
                .0
                .bytes()
                .zip(returned.bytes())
                .fold(0_u8, |differences, (ours, theirs)| {
                    differences | (ours ^ theirs)
                })
                == 0
    }
}

fn random_bytes() -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).expect("the operating system should provide randomness");

    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7636 Appendix B — the specification's own worked example.
    const RFC_VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const RFC_CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    #[test]
    fn derives_the_challenge_from_rfc_7636_appendix_b() {
        assert_eq!(
            Verifier::from_stored(RFC_VERIFIER).challenge(),
            RFC_CHALLENGE
        );
    }

    #[test]
    fn generates_a_verifier_of_the_length_rfc_7636_allows() {
        let length = Verifier::generate().as_str().len();
        assert!((43..=128).contains(&length), "got {length} characters");
    }

    #[test]
    fn generates_a_different_verifier_every_time() {
        assert_ne!(Verifier::generate(), Verifier::generate());
    }

    #[test]
    fn uses_only_the_unreserved_characters() {
        let verifier = Verifier::generate();

        assert!(
            verifier
                .as_str()
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'-' | b'.' | b'_' | b'~')),
            "got {}",
            verifier.as_str()
        );
    }

    #[test]
    fn a_state_matches_only_itself() {
        let state = State::generate();

        assert!(state.matches(state.as_str()));
        assert!(!state.matches("something-else"));
        assert!(!State::generate().matches(state.as_str()));
    }
}
