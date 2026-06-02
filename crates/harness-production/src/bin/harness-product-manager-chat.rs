use harness_production::product_chat_args::{self, ParseOutcome};

fn main() {
    let args = std::env::args().skip(1);
    match product_chat_args::parse(args) {
        Ok(ParseOutcome::Help) => {
            println!("usage: {}", product_chat_args::USAGE);
        }
        Ok(ParseOutcome::Repl(chat_args)) => {
            if let Err(error) = harness_production::product_chat_repl::run_repl(&chat_args) {
                eprintln!("harness-product-manager-chat: {error}");
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("harness-product-manager-chat: {error}");
            std::process::exit(2);
        }
    }
}
