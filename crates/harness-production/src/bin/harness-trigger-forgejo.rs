use harness_production::trigger_args::{self, ParseOutcome};

fn main() {
    let args = std::env::args().skip(1);
    match trigger_args::parse(args) {
        Ok(ParseOutcome::Help) => {
            println!("usage: {}", trigger_args::USAGE);
        }
        Ok(ParseOutcome::Run(trigger_args)) => {
            if let Err(error) = trigger_args::run(&trigger_args) {
                eprintln!("harness-trigger-forgejo: {error}");
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("harness-trigger-forgejo: {error}");
            std::process::exit(2);
        }
    }
}
