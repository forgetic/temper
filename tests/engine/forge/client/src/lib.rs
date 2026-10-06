//! A client domain world: durable scripted parent, independently typed fake
//! forge, protocol translation, referee observations, and one seeded loop.
mod referee;
pub mod translate;
mod world;
pub use world::{LIMITS, REPO, Settings, Stats, World};
