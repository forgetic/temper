//! The fake forge's tests that need ordinary Rust (testing-pyramid.md, 4.2):
//! its memory, measured by a counting allocator, against its worst case
//! (programming-model.md, 6.3), with every container filled to its limit.
//!
//! The fake has no world of its own: the worlds of those it stands beside
//! run it. Its step tests are in its crate.
