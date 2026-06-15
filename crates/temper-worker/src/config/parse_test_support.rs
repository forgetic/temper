use super::{ParseOutcome, WorkerConfig, parse};

pub(super) fn parse_ok(args: &[&str]) -> WorkerConfig {
    match parse(args.iter().map(|arg| (*arg).to_string())).expect("parse succeeds") {
        ParseOutcome::Run(config) => config,
        ParseOutcome::Help => panic!("expected run config"),
    }
}

pub(super) fn parse_err(args: &[&str]) -> String {
    parse(args.iter().map(|arg| (*arg).to_string())).expect_err("parse fails")
}
