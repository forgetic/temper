//! The engine's model world (engine-model.md, section 14; testing-pyramid.md,
//! 2.2): the engine's top-level model, with all its sub-models, in one loop,
//! deterministically from a seed.

pub mod codec;
pub mod deployment;
pub mod mirror;
pub mod people;
pub mod referee;
pub mod script;
pub mod store;
pub mod translate;
pub mod workers;
mod world;

pub use temper_world::Span;
pub use world::{ENDINGS, Settings, Stats, World};
