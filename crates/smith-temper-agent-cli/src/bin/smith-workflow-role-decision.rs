use std::io::{self, Read, Write};
use std::path::PathBuf;

use smith_temper_agent::{
    AuthChoice, ProviderConfig, WORKFLOW_ROLE_DECISION_CAPTURE_DIR_ENV,
    WorkflowRoleDecisionRequest, WorkflowRoleDecisionResponder,
};

#[cfg(feature = "test-provider-base-url-override")]
const TEST_PROVIDER_BASE_URL_ENV: &str = "SMITH_TEST_PROVIDER_BASE_URL";

#[cfg(feature = "test-provider-base-url-override")]
fn apply_test_provider_base_url_override(provider: ProviderConfig) -> ProviderConfig {
    match std::env::var(TEST_PROVIDER_BASE_URL_ENV) {
        Ok(base_url) if !base_url.trim().is_empty() => provider.with_base_url_override(base_url),
        _ => provider,
    }
}

fn main() {
    match run() {
        Ok(()) => {}
        Err(message) => {
            eprintln!("smith-workflow-role-decision: {message}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<(), String> {
    let options = DecisionOptions::parse(std::env::args().skip(1).collect())?;
    if options.help {
        print_usage();
        return Ok(());
    }

    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|error| format!("reading request from stdin failed: {error}"))?;
    let request: WorkflowRoleDecisionRequest = serde_json::from_str(&input)
        .map_err(|error| format!("invalid WorkflowRoleDecisionRequest JSON: {error}"))?;

    let provider = ProviderConfig::from_auth(options.auth, options.codex_model, options.auth_file)
        .map_err(|error| error.to_string())?;
    #[cfg(feature = "test-provider-base-url-override")]
    let provider = apply_test_provider_base_url_override(provider);
    let responder = WorkflowRoleDecisionResponder::new(provider);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("building runtime failed: {error}"))?;
    let reply = runtime
        .block_on(responder.respond(&request))
        .map_err(|error| error.to_string())?;

    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(&mut stdout, &reply)
        .map_err(|error| format!("writing WorkflowRoleDecisionReply JSON failed: {error}"))?;
    stdout
        .write_all(b"\n")
        .map_err(|error| format!("writing stdout failed: {error}"))?;
    Ok(())
}

#[derive(Debug)]
struct DecisionOptions {
    auth: AuthChoice,
    codex_model: Option<String>,
    auth_file: Option<PathBuf>,
    help: bool,
}

impl DecisionOptions {
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
        "Usage:\n  smith-workflow-role-decision \\\n    [--auth deepseek|chatgpt-oauth|anthropic-oauth] \\\n    [--codex-model MODEL] [--auth-file PATH] \\\n    < workflow-role-decision-request.json > workflow-role-decision-reply.json\n\nReads one Temper WorkflowRoleDecisionRequest JSON value on stdin and writes one WorkflowRoleDecisionReply JSON value on stdout. Logs and errors go to stderr. The process receives no Forge handle, Forge token, or workflow mutation tool. Set {WORKFLOW_ROLE_DECISION_CAPTURE_DIR_ENV} to an existing writable directory to write one bounded, redacted JSON capture per decision; captures are disabled by default."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_role_decision_parses_provider_options() {
        let options = DecisionOptions::parse(vec![
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
    fn workflow_role_decision_rejects_unknown_auth() {
        let error = DecisionOptions::parse(vec!["--auth".into(), "unknown".into()])
            .expect_err("unknown auth fails");
        assert!(error.contains("unsupported auth"));
    }

    #[cfg(feature = "test-provider-base-url-override")]
    #[test]
    fn test_provider_base_url_override_honors_non_empty_value() {
        let provider = ProviderConfig::new("test", "model", "http://original", "key")
            .with_base_url_override("http://127.0.0.1:4200");
        assert_eq!(provider.base_url_for_test(), "http://127.0.0.1:4200");
    }
}
