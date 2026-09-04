//! OAuth machinery shared by every provider.
//!
//! Nothing here talks to a particular service. A provider describes itself and
//! these pieces do the protocol, so adding the second and third provider costs
//! a description rather than a flow.

pub mod loopback;
pub mod pkce;
pub mod registration;

/// A fresh pair of tokens, however they were obtained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Seconds from now, when the provider says so.
    pub expires_in: Option<u64>,
}

/// Where a provider's OAuth endpoints live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub authorize: &'static str,
    pub token: &'static str,
    /// `Some` when the provider supports the device grant.
    pub device_code: Option<&'static str>,
}

/// What a service has to say about itself for the shared machinery to sign in
/// on its behalf and keep the credential alive.
///
/// Deliberately small. Everything here is a fact about the provider; nothing is
/// a flow. Flows live in the provider's own module until a second provider
/// wants the same one, because an abstraction drawn from one example is a
/// guess.
pub trait Provider {
    /// The value stored in `integration_accounts.service`.
    const SERVICE: &'static str;

    /// This provider's error type, so callers keep the errors they already
    /// surface to the user rather than a lowest common denominator.
    type Error: std::error::Error + From<crate::db::Error>;

    fn endpoints(&self) -> Endpoints;

    /// Public by design. Never a secret; see `client_secret`.
    ///
    /// The answer without reading the database: this machine's environment,
    /// then whatever the build carried. Callers that have a pool should go
    /// through [`registration::resolve`] instead, so an id the user supplied
    /// in Settings is not skipped over.
    fn client_id(&self) -> Result<String, Self::Error>;

    /// What this machine's environment says, if anything.
    ///
    /// Separate from [`Provider::client_id`] because the layers rank: an
    /// environment variable is a deliberate act for one run and beats a stored
    /// id, while a built-in one is only a default and loses to it. Resolving
    /// the two together would make that impossible to express.
    fn environment_client_id(&self) -> Option<String>;

    fn scopes(&self) -> &'static [&'static str];

    /// Only ever a value the provider documents as non-confidential, or one
    /// minted for this installation alone. A secret compiled into a binary
    /// Chief distributes is not a secret, and must never be returned here.
    fn client_secret(&self) -> Option<String> {
        None
    }

    /// Swap a refresh token for a fresh pair.
    fn refresh(
        &self,
        client_id: &str,
        refresh_token: &str,
    ) -> impl std::future::Future<Output = Result<Tokens, Self::Error>> + Send;

    /// Whether an error means the stored credential is no longer usable.
    fn is_token_rejected(error: &Self::Error) -> bool;
}
