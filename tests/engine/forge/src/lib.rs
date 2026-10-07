//! A connector world with a durable scripted root, an independent fake forge
//! and the client protocol translation. The fake's observations and stored
//! rows are separate from the connector's state.
#[path = "../client/src/translate.rs"]
pub mod translate;
mod world;
pub use world::{LIMITS, REPO, World, fake_config};
