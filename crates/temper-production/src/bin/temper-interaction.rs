use temper_production::interaction_args::{self, ParseOutcome};

fn main() {
    let args = std::env::args().skip(1);
    match interaction_args::parse(args) {
        Ok(ParseOutcome::Help) => {
            println!("usage: {}", interaction_args::USAGE);
        }
        Ok(ParseOutcome::Repl(args)) => {
            if let Err(error) = temper_production::interaction_repl::run_repl(&args) {
                eprintln!("temper-interaction: {error}");
                std::process::exit(1);
            }
        }
        Ok(ParseOutcome::Serve(args)) => {
            if let Err(error) = temper_production::interaction_serve::run_serve(&args) {
                eprintln!("temper-interaction: {error}");
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("temper-interaction: {error}");
            std::process::exit(2);
        }
    }
}
