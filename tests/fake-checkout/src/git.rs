//! The shared checkout's git entry points and remote contract
//! (skein's docs/design/fake-checkout.md, section 4). Local operations keep objects and merge
//! metadata; temper's world-owned `Remote` implementations retain forge policy.
//! These explicit exports preserve their trait identity and terminal outcomes.

pub use skein_fake_checkout::git::{
    CommitFailure, Created, Fault, Merged, NotFetched, Pushed, Remote, Tree, Want, What, check_out, clone_repository,
    commit, commit_merging, create, fetch, merge, push, push_expected,
};
