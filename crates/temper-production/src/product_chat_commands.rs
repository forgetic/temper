//! Shared product-chat slash command parsing.

use temper_interaction::CommandActionManifest;

use crate::product_chat::{product_profile_manifest, ProductManagerDraftIssue};

pub const COMMAND_HELP: &str = "Commands: /drafts, /file <n>, /issue, /help, /quit";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProductChatCommand<'a> {
    Drafts,
    File(&'a str),
    Help,
    Issue,
    Quit,
    Unknown(&'a str),
}

impl<'a> ProductChatCommand<'a> {
    pub fn parse(input: &'a str) -> Option<Self> {
        Self::parse_with_accept_aliases(input, &accept_proposal_aliases())
    }

    fn parse_with_accept_aliases(input: &'a str, accept_aliases: &[String]) -> Option<Self> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }
        if trimmed == "/drafts" {
            Some(Self::Drafts)
        } else if let Some(raw) = parse_accept_alias(trimmed, accept_aliases) {
            Some(Self::File(raw))
        } else if trimmed == "/help" {
            Some(Self::Help)
        } else if trimmed == "/issue" {
            Some(Self::Issue)
        } else if trimmed == "/quit" {
            Some(Self::Quit)
        } else {
            Some(Self::Unknown(trimmed))
        }
    }
}

fn parse_accept_alias<'a>(trimmed: &'a str, aliases: &[String]) -> Option<&'a str> {
    aliases.iter().find_map(|alias| {
        let alias = alias.trim();
        trimmed
            .strip_prefix(alias)
            .and_then(|rest| rest.strip_prefix(' '))
            .map(str::trim)
    })
}

fn accept_proposal_aliases() -> Vec<String> {
    product_profile_manifest()
        .map(|manifest| {
            manifest
                .commands
                .iter()
                .filter(|command| {
                    matches!(
                        &command.action,
                        CommandActionManifest::AcceptProposal { .. }
                    )
                })
                .flat_map(|command| command.aliases.clone())
                .collect()
        })
        .unwrap_or_else(|_| vec!["/file".into()])
}

pub fn render_drafts(drafts: &[ProductManagerDraftIssue]) -> String {
    if drafts.is_empty() {
        return "Drafts: (none)".into();
    }
    let mut body = String::from("Drafts:");
    for (index, draft) in drafts.iter().enumerate() {
        body.push_str(&format!("\n[{}] {}", index + 1, draft.title));
        if let Some(rationale) = draft.rationale.as_deref().filter(|text| !text.is_empty()) {
            body.push_str(&format!("\n    {rationale}"));
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_help_without_treating_it_as_prose() {
        assert_eq!(
            ProductChatCommand::parse("/help"),
            Some(ProductChatCommand::Help)
        );
        assert_eq!(ProductChatCommand::parse("hello"), None);
    }
}
