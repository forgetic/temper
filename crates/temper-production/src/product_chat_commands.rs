//! Shared product-chat slash command parsing.

use crate::product_chat::ProductManagerDraftIssue;

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
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }
        if trimmed == "/drafts" {
            Some(Self::Drafts)
        } else if let Some(raw) = trimmed.strip_prefix("/file ") {
            Some(Self::File(raw.trim()))
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
