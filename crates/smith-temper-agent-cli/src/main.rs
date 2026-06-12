use std::path::PathBuf;

use smith_temper_agent::{AuthChoice, ProviderConfig};

fn main() {
    match run() {
        Ok(()) => {}
        Err(message) => {
            eprintln!("smith-temper-agent-cli: {message}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        print_usage();
        return Ok(());
    };
    match command.as_str() {
        "version" => {
            print_version();
            Ok(())
        }
        "preflight" => preflight(args.collect()),
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        other => Err(format!("unknown command `{other}`; run with `help`")),
    }
}

fn print_version() {
    println!("smith-temper-agent-cli {}", env!("CARGO_PKG_VERSION"));
}

fn preflight(args: Vec<String>) -> Result<(), String> {
    let options = PreflightOptions::parse(args)?;
    let config = ProviderConfig::from_auth(options.auth, options.codex_model, options.auth_file)
        .map_err(|error| error.to_string())?;
    println!(
        "provider preflight ok: model={} config={:?}",
        config.model_id(),
        config
    );
    Ok(())
}

struct PreflightOptions {
    auth: AuthChoice,
    codex_model: Option<String>,
    auth_file: Option<PathBuf>,
}

impl PreflightOptions {
    fn parse(args: Vec<String>) -> Result<Self, String> {
        let mut auth = AuthChoice::ChatGptOAuth;
        let mut codex_model = None;
        let mut auth_file = None;
        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--auth" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| "--auth requires a value".to_string())?;
                    auth = parse_auth_choice(&value)?;
                }
                "--codex-model" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| "--codex-model requires a value".to_string())?;
                    codex_model = Some(value);
                }
                "--auth-file" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| "--auth-file requires a value".to_string())?;
                    auth_file = Some(PathBuf::from(value));
                }
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => return Err(format!("unknown preflight option `{other}`")),
            }
        }
        Ok(Self {
            auth,
            codex_model,
            auth_file,
        })
    }
}

fn parse_auth_choice(value: &str) -> Result<AuthChoice, String> {
    match value {
        "deepseek" => Ok(AuthChoice::DeepSeek),
        "chatgpt-oauth" => Ok(AuthChoice::ChatGptOAuth),
        "anthropic-oauth" => Ok(AuthChoice::AnthropicOAuth),
        other => Err(format!(
            "unsupported auth `{other}`; expected deepseek, chatgpt-oauth, or anthropic-oauth"
        )),
    }
}

fn print_usage() {
    println!(
        "Usage:\n  smith-temper-agent-cli preflight \\\n    [--auth deepseek|chatgpt-oauth|anthropic-oauth] \\\n    [--codex-model MODEL] [--auth-file PATH]\n  smith-temper-agent-cli version\n\nThe preflight command reads credentials from env or auth files and prints only redacted provider config.\nThe version command prints the Smith Temper agent CLI package version."
    );
}
