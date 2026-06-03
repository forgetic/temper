//! Generic REPL command parsing and rendering from compiled command manifests.

use temper_interaction::{
    CommandActionManifest, CommandManifest, CompiledProfileManifest, Proposal, ProposalId,
    ProposalKind, ProposalPayloadValidator,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuiltinReplCommand {
    Help,
    Proposals,
    Transcript,
    Quit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParsedReplCommand<'a> {
    Builtin(BuiltinReplCommand),
    Manifest {
        command: &'a CommandManifest,
        argument: Option<&'a str>,
    },
    Unknown(&'a str),
}

pub fn parse_repl_command<'a>(
    profile: &'a CompiledProfileManifest,
    input: &'a str,
) -> Option<ParsedReplCommand<'a>> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    match trimmed {
        "/help" => return Some(ParsedReplCommand::Builtin(BuiltinReplCommand::Help)),
        "/proposals" => {
            return Some(ParsedReplCommand::Builtin(BuiltinReplCommand::Proposals));
        }
        "/transcript" | "/issue" => {
            return Some(ParsedReplCommand::Builtin(BuiltinReplCommand::Transcript));
        }
        "/quit" | "/exit" => return Some(ParsedReplCommand::Builtin(BuiltinReplCommand::Quit)),
        _ => {}
    }
    for command in &profile.commands {
        for alias in &command.aliases {
            if trimmed == alias {
                return Some(ParsedReplCommand::Manifest {
                    command,
                    argument: None,
                });
            }
            if let Some(argument) = trimmed
                .strip_prefix(alias)
                .and_then(|rest| rest.strip_prefix(' '))
            {
                return Some(ParsedReplCommand::Manifest {
                    command,
                    argument: Some(argument.trim()),
                });
            }
        }
    }
    Some(ParsedReplCommand::Unknown(trimmed))
}

pub fn render_command_help(profile: &CompiledProfileManifest) -> String {
    let mut lines = vec![
        format!("Profile: {}", profile.profile.id),
        "Built-in commands: /help, /proposals, /transcript, /quit".to_string(),
    ];
    if profile.commands.is_empty() {
        lines.push("Profile commands: (none)".to_string());
    } else {
        lines.push("Profile commands:".to_string());
        for command in &profile.commands {
            let aliases = if command.aliases.is_empty() {
                "(no aliases)".to_string()
            } else {
                command.aliases.join(", ")
            };
            lines.push(format!(
                "  {aliases} — {} ({})",
                action_description(&command.action),
                command.id
            ));
        }
    }
    lines.join("\n")
}

pub fn render_proposals(profile: &CompiledProfileManifest, proposals: &[Proposal]) -> String {
    if proposals.is_empty() {
        return "Proposals: (none)".into();
    }
    let mut body = String::from("Proposals:");
    for (index, proposal) in proposals.iter().enumerate() {
        body.push_str(&format!(
            "\n[{}] {} ({}, kind: {})",
            index + 1,
            proposal.title,
            proposal.id,
            proposal.kind
        ));
        match profile.proposal(&proposal.kind) {
            Some(manifest) => match manifest.payload_validator {
                ProposalPayloadValidator::IssueDraft => match proposal.issue_payload() {
                    Ok(Some(issue)) => {
                        if let Some(rationale) =
                            issue.rationale.as_deref().filter(|text| !text.is_empty())
                        {
                            body.push_str(&format!("\n    {rationale}"));
                        }
                    }
                    Ok(None) => {}
                    Err(_) => body.push_str("\n    (invalid issue_draft payload)"),
                },
            },
            None => body.push_str("\n    (proposal kind is not declared by this profile)"),
        }
    }
    body
}

pub fn resolve_proposal_selector(
    proposals: &[Proposal],
    proposal_kind: &ProposalKind,
    selector: &str,
) -> Result<ProposalId, String> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err("proposal id or number is required".into());
    }
    let matching = proposals
        .iter()
        .filter(|proposal| &proposal.kind == proposal_kind)
        .collect::<Vec<_>>();
    if let Ok(index) = selector.parse::<usize>() {
        if index > 0 && index <= matching.len() {
            return Ok(matching[index - 1].id.clone());
        }
        return Err(format!(
            "proposal number {index} is not available for kind `{proposal_kind}`"
        ));
    }
    matching
        .iter()
        .find(|proposal| proposal.id.as_str() == selector)
        .map(|proposal| proposal.id.clone())
        .ok_or_else(|| format!("proposal `{selector}` is not available for kind `{proposal_kind}`"))
}

fn action_description(action: &CommandActionManifest) -> String {
    match action {
        CommandActionManifest::AcceptProposal {
            proposal_kind,
            acceptance_action,
        } => format!("accept `{proposal_kind}` proposal via `{acceptance_action}`"),
    }
}
