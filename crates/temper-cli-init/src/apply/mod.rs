// SPDX-License-Identifier: MPL-2.0

//! `temper apply` facade, split by arguments, orchestration, and presentation.

mod args;
mod presentation;
mod run;

pub use args::{
    APPLY_USAGE, ApplyCredentialMode, ApplyOptions, apply_main, apply_main_with_options,
};
pub use run::run_apply;

#[cfg(test)]
mod tests;
