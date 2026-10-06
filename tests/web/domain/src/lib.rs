//! The client world drives a real domain and view from an accessible person.
//! Its scripted engine owns durable facts; a referee reads only those facts
//! and the rendered tree. The shell owns time, session storage and reloads.

pub mod engine;
pub mod model;
pub mod referee;
pub mod scenario;
pub mod tab;
pub mod world;

pub use scenario::Scenario;
pub use world::{Fault, Settings, Stats, World};
