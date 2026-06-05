use temper_forgejo_provision::provision_args::{self, ParseOutcome};

fn main() {
    let args = std::env::args().skip(1);
    match provision_args::parse(args) {
        Ok(ParseOutcome::Help) => {
            println!("usage: {}", provision_args::USAGE);
        }
        Ok(ParseOutcome::Run(provision_args)) => match provision_args::run(&provision_args) {
            Ok(status) => println!("{status}"),
            Err(error) => {
                eprintln!("temper-provision-forgejo: {error}");
                std::process::exit(1);
            }
        },
        Err(error) => {
            eprintln!("temper-provision-forgejo: {error}");
            std::process::exit(2);
        }
    }
}
