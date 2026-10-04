//! Total boundary conversion; opaque payloads are never decoded at a worker.
pub mod agent;
pub mod link;
mod records;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    Direction,
    Limits,
    Credential,
    Unsupported,
}
