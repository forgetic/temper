// SPDX-License-Identifier: MPL-2.0

//! Hidden responder subcommands — the agent's external process-responders.
//!
//! Each reads one request JSON value on stdin and writes one reply JSON value on
//! stdout, driving anvil's native agent loop on a skein engine task. They share
//! the same provider option parsing.

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;
use temper_agent::{
    AuthChoice, ProductManagerResponder, ProviderConfig, WorkflowRoleDecisionRequest,
    WorkflowRoleDecisionResponder,
};
use temper_process_protocol::ConversationRequest;

/// `temper product-manager-responder`.
pub fn product_manager(args: std::env::Args) -> ExitCode {
    finish(
        "product-manager-responder",
        run_product_manager(args.collect()),
    )
}

/// `temper workflow-role-decision`.
pub fn workflow_role_decision(args: std::env::Args) -> ExitCode {
    finish(
        "workflow-role-decision",
        run_workflow_role_decision(args.collect()),
    )
}

fn finish(name: &str, result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("temper {name}: {message}");
            ExitCode::from(2)
        }
    }
}

fn run_product_manager(args: Vec<String>) -> Result<(), String> {
    let options = ResponderOptions::parse(args)?;
    if options.help {
        println!("{PRODUCT_MANAGER_USAGE}");
        return Ok(());
    }
    let request: ConversationRequest = read_request("ConversationRequest")?;
    let provider = options.provider()?;
    let responder = ProductManagerResponder::new(provider);
    let reply = temper_agent_io::block_on_with(move |_cx, handle| async move {
        responder.respond(handle, &request).await
    })
    .map_err(|error| error.to_string())?;
    write_reply(&reply)
}

fn run_workflow_role_decision(args: Vec<String>) -> Result<(), String> {
    let options = ResponderOptions::parse(args)?;
    if options.help {
        println!("{WORKFLOW_ROLE_DECISION_USAGE}");
        return Ok(());
    }
    let request: WorkflowRoleDecisionRequest = read_request("WorkflowRoleDecisionRequest")?;
    let provider = options.provider()?;
    let responder = WorkflowRoleDecisionResponder::new(provider);
    let reply = temper_agent_io::block_on_with(move |_cx, handle| async move {
        responder.respond(handle, &request).await
    })
    .map_err(|error| error.to_string())?;
    write_reply(&reply)
}

fn read_request<T: serde::de::DeserializeOwned>(name: &str) -> Result<T, String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|error| format!("reading request from stdin failed: {error}"))?;
    serde_json::from_str(&input).map_err(|error| format!("invalid {name} JSON: {error}"))
}

fn write_reply<T: Serialize>(reply: &T) -> Result<(), String> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(&mut stdout, reply)
        .map_err(|error| format!("writing reply JSON failed: {error}"))?;
    stdout
        .write_all(b"\n")
        .map_err(|error| format!("writing stdout failed: {error}"))
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
                    auth = parse_auth_choice(
                        &iter
                            .next()
                            .ok_or_else(|| "--auth requires a value".to_string())?,
                    )?;
                }
                "--codex-model" => {
                    codex_model = Some(
                        iter.next()
                            .ok_or_else(|| "--codex-model requires a value".to_string())?,
                    );
                }
                "--auth-file" => {
                    auth_file = Some(PathBuf::from(
                        iter.next()
                            .ok_or_else(|| "--auth-file requires a value".to_string())?,
                    ));
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

    fn provider(&self) -> Result<ProviderConfig, String> {
        ProviderConfig::from_auth(self.auth, self.codex_model.clone(), self.auth_file.clone())
            .map_err(|error| error.to_string())
            .map(ProviderConfig::apply_base_url_override_from_env)
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

const PRODUCT_MANAGER_USAGE: &str = "\
temper product-manager-responder [--auth deepseek|chatgpt-oauth|anthropic-oauth] \
[--codex-model MODEL] [--auth-file PATH] < request.json > reply.json

Reads one ConversationRequest JSON value on stdin and writes one
ConversationReply JSON value on stdout. Logs and errors go to stderr.";

const WORKFLOW_ROLE_DECISION_USAGE: &str = "\
temper workflow-role-decision [--auth deepseek|chatgpt-oauth|anthropic-oauth] \
[--codex-model MODEL] [--auth-file PATH] < request.json > reply.json

Reads one WorkflowRoleDecisionRequest JSON value on stdin and writes one
WorkflowRoleDecisionReply JSON value on stdout. The process receives no Forge
handle, token, or workflow mutation tool.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_provider_options() {
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
    fn rejects_unknown_auth() {
        let error = ResponderOptions::parse(vec!["--auth".into(), "unknown".into()])
            .expect_err("unknown auth fails");
        assert!(error.contains("unsupported auth"));
    }
}
