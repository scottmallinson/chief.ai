//! OAuth machinery shared by every provider.
//!
//! Nothing here talks to a particular service. A provider describes itself and
//! these pieces do the protocol, so adding the second and third provider costs
//! a description rather than a flow.

// pub mod loopback;
pub mod pkce;
