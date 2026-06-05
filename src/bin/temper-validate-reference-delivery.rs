use temper_production::reference_delivery_validator::{self, ParseOutcome, RunError};

fn main() {
    match reference_delivery_validator::parse(std::env::args().skip(1)) {
        Ok(ParseOutcome::Help) => println!("usage: {}", reference_delivery_validator::USAGE),
        Ok(ParseOutcome::Run(args)) => match reference_delivery_validator::run(&args) {
            Ok(output) => println!("{output}"),
            Err(RunError::ValidationFailed(output)) => {
                eprintln!("{output}");
                std::process::exit(1);
            }
            Err(error) => {
                eprintln!("temper-validate-reference-delivery: {error}");
                std::process::exit(1);
            }
        },
        Err(error) => {
            eprintln!("temper-validate-reference-delivery: {error}");
            std::process::exit(2);
        }
    }
}
