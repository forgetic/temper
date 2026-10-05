//! Payload schemas selected by the negotiated channel version.
#[cfg(test)]
#[path = "payload/tests_v2.rs"]
mod tests_v2;
pub mod v1;
pub mod v2;
