use std::io::{self, Read, Write};
use std::path::PathBuf;

use temper_agent::{AuthChoice, ProductManagerResponder, ProviderConfig};
use temper_process_protocol::ConversationRequest;

pub fn main<I>(args: I) -> std::process::ExitCode
where
    I: Iterator<Item = String>,
{
    match run(args.collect()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("temper-product-manager-responder: {message}");
            std::process::ExitCode::from(2)
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let options = ResponderOptions::parse(args)?;
    if options.help {
        print_usage();
        return Ok(());
    }

    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|error| format!("reading request from stdin failed: {error}"))?;
    let request: ConversationRequest = serde_json::from_str(&input)
        .map_err(|error| format!("invalid ConversationRequest JSON: {error}"))?;

    let provider = ProviderConfig::from_auth(options.auth, options.codex_model, options.auth_file)
        .map_err(|error| error.to_string())?
        .apply_base_url_override_from_env();
    let responder = ProductManagerResponder::new(provider);
    // The decision path drives anvil's native agent loop, which must run
    // inside a skein engine task (runtime clock + I/O spawning).
    let reply = temper_agent_io_engine::block_on_with(move |_cx, handle| async move {
        responder.respond(handle, &request).await
    })
    .map_err(|error| error.to_string())?;

    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(&mut stdout, &reply)
        .map_err(|error| format!("writing ConversationReply JSON failed: {error}"))?;
    stdout
        .write_all(b"\n")
        .map_err(|error| format!("writing stdout failed: {error}"))?;
    Ok(())
}

#[derive(Debug)]
struct ResponderOptions {
    auth: AuthChoice,
    codex_model: Option<String>,
    auth_file: Option<PathBuf>,
    help: bool,
}

impl ResponderOptions {
    fn parse(args: Vec<String>) -> Result<Self, String> {
        let mut auth = AuthChoice::ChatGptOAuth;
        let mut codex_model = None;
        let mut auth_file = None;
        let mut help = false;
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
                "--help" | "-h" | "help" => help = true,
                other => return Err(format!("unknown option `{other}`; run with --help")),
            }
        }
        Ok(Self {
            auth,
            codex_model,
            auth_file,
            help,
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
        "Usage:\n  anvil-product-manager-responder \\\n    [--auth deepseek|chatgpt-oauth|anthropic-oauth] \\\n    [--codex-model MODEL] [--auth-file PATH] \\\n    < conversation-request.json > conversation-reply.json\n\nReads one Temper ConversationRequest JSON value on stdin and writes one ConversationReply JSON value on stdout. Logs and errors go to stderr."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_manager_responder_parses_provider_options() {
        let options = ResponderOptions::parse(vec![
            "--auth".into(),
            "anthropic-oauth".into(),
            "--codex-model".into(),
            "gpt-test".into(),
            "--auth-file".into(),
            "/tmp/auth.json".into(),
        ])
        .expect("options parse");

        assert_eq!(options.auth, AuthChoice::AnthropicOAuth);
        assert_eq!(options.codex_model.as_deref(), Some("gpt-test"));
        assert_eq!(options.auth_file, Some(PathBuf::from("/tmp/auth.json")));
    }

    #[test]
    fn product_manager_responder_rejects_unknown_auth() {
        let error = ResponderOptions::parse(vec!["--auth".into(), "unknown".into()])
            .expect_err("unknown auth fails");
        assert!(error.contains("unsupported auth"));
    }

    #[test]
    fn provider_base_url_override_leaves_config_unchanged_without_override() {
        let provider = ProviderConfig::new("test", "model", "http://original", "key");
        assert_eq!(provider.base_url(), "http://original");
    }
}
