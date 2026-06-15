//! Argument parsing and top-level runner for `temper provision-forgejo`.

mod model;
mod parse;
mod runner;

pub use model::{
    ADMIN_TOKEN_ENV, ArgsError, ParseOutcome, ProvisionArgs, RunError, USAGE, WORKFLOW_FILE_ENV,
};
pub use parse::{parse, parse_with_env};
pub use runner::run;

#[cfg(test)]
mod tests;
