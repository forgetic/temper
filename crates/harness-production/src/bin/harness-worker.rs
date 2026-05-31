use harness_production::worker_args::{self, ParseOutcome};

fn main() {
    let args = std::env::args().skip(1);
    match worker_args::parse(args) {
        Ok(ParseOutcome::Help) => {
            println!("usage: {}", worker_args::USAGE);
        }
        Ok(ParseOutcome::Run(worker_args)) => match harness_production::worker::run(&worker_args) {
            Ok(report) => {
                eprintln!("harness-worker: completed after {} tick(s)", report.ticks);
            }
            Err(error) => {
                eprintln!("harness-worker: {error}");
                std::process::exit(1);
            }
        },
        Err(error) => {
            eprintln!("harness-worker: {error}");
            std::process::exit(2);
        }
    }
}
