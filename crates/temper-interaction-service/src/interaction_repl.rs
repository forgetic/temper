//! Terminal REPL transport for generic compiled interaction profiles.

use std::io::{self, Write};

use serde_json::json;
use temper_interaction::{CommandActionManifest, CompiledProfileManifest};

use crate::interaction_args::InteractionReplArgs;
use crate::interaction_bindings::{
    build_service, load_bindings, load_compiled_spec, InteractionDeploymentError,
};
use crate::interaction_commands::{
    parse_repl_command, render_command_help, render_proposals, resolve_proposal_selector,
    BuiltinReplCommand, ParsedReplCommand,
};
use crate::interaction_service::InteractionService;

pub fn run_repl(args: &InteractionReplArgs) -> Result<(), InteractionDeploymentError> {
    let runtime = temper_io_engine::build_runtime().map_err(InteractionDeploymentError::Config)?;
    let args = args.clone();
    temper_io_engine::runtime::block_on_runtime_with(&runtime, move |cx, _handle| async move {
        run_repl_async(&cx, &args).await
    })
}

async fn run_repl_async(
    cx: &temper_io_engine::Cx,
    args: &InteractionReplArgs,
) -> Result<(), InteractionDeploymentError> {
    let spec = load_compiled_spec(&args.spec_path)?;
    let bindings = load_bindings(&args.bindings_path)?;
    let service = build_service(cx, &spec, &bindings, |key| std::env::var(key).ok())?;
    let profile = service
        .profile_manifest(&args.profile_id)
        .cloned()
        .ok_or_else(|| {
            InteractionDeploymentError::Config(format!(
                "profile `{}` is not bound by deployment config",
                args.profile_id
            ))
        })?;
    let conversation = service
        .create_conversation(
            Some(args.profile_id.clone()),
            args.transcript_issue,
            json!({}),
        )
        .await?;
    println!(
        "Opened {} conversation:\n  {}",
        profile.profile.id, conversation.transcript.url
    );
    println!("{}", render_command_help(&profile));
    repl_loop(&service, &profile, conversation.id).await
}

async fn repl_loop(
    service: &InteractionService,
    profile: &CompiledProfileManifest,
    conversation_id: String,
) -> Result<(), InteractionDeploymentError> {
    let stdin = io::stdin();
    loop {
        print!("you> ");
        io::stdout().flush()?;
        let mut line = String::new();
        if stdin.read_line(&mut line)? == 0 {
            println!();
            return Ok(());
        }
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        if let Some(command) = parse_repl_command(profile, input) {
            if handle_local_command(service, profile, &conversation_id, command).await? {
                return Ok(());
            }
            continue;
        }
        let response = service
            .send_turn(&conversation_id, input.to_string())
            .await?;
        println!(
            "\n{}> {}\n",
            agent_name(profile),
            response.reply.message.trim()
        );
        println!("{}", render_proposals(profile, &response.latest_proposals));
    }
}

async fn handle_local_command(
    service: &InteractionService,
    profile: &CompiledProfileManifest,
    conversation_id: &str,
    command: ParsedReplCommand<'_>,
) -> Result<bool, InteractionDeploymentError> {
    match command {
        ParsedReplCommand::Builtin(BuiltinReplCommand::Help) => {
            println!("{}", render_command_help(profile));
        }
        ParsedReplCommand::Builtin(BuiltinReplCommand::Proposals) => {
            let proposals = service.latest_proposals(conversation_id).await?.proposals;
            println!("{}", render_proposals(profile, &proposals));
        }
        ParsedReplCommand::Builtin(BuiltinReplCommand::Transcript) => {
            let conversation = service.get_conversation(conversation_id).await?;
            println!("{}", conversation.transcript.url);
        }
        ParsedReplCommand::Builtin(BuiltinReplCommand::Quit) => return Ok(true),
        ParsedReplCommand::Manifest { command, argument } => match &command.action {
            CommandActionManifest::AcceptProposal {
                proposal_kind,
                acceptance_action,
            } => {
                let Some(argument) = argument else {
                    println!("usage: {} <proposal-id|number>", command_alias(command));
                    return Ok(false);
                };
                let proposals = service.latest_proposals(conversation_id).await?.proposals;
                let proposal_id =
                    match resolve_proposal_selector(&proposals, proposal_kind, argument) {
                        Ok(id) => id,
                        Err(message) => {
                            println!("{message}");
                            return Ok(false);
                        }
                    };
                let outcome = service
                    .accept_proposal_with_action(
                        conversation_id,
                        proposal_id,
                        Some(acceptance_action),
                    )
                    .await?;
                match outcome.target {
                    Some(target) => println!(
                        "accepted {} proposal: {}",
                        target.kind,
                        target.url.unwrap_or_else(|| "(no url)".into())
                    ),
                    None => println!("accepted proposal"),
                }
            }
        },
        ParsedReplCommand::Unknown(command) => {
            println!("unknown command '{command}'; try /help");
        }
    }
    Ok(false)
}

fn command_alias(command: &temper_interaction::CommandManifest) -> &str {
    command
        .aliases
        .first()
        .map(String::as_str)
        .unwrap_or(command.id.as_str())
}

fn agent_name(profile: &CompiledProfileManifest) -> String {
    profile
        .profile
        .agent_participant
        .display_name
        .clone()
        .unwrap_or_else(|| profile.profile.id.to_string())
}
