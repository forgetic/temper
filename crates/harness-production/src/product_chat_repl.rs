//! Terminal REPL for product-manager chat.

use std::io::{self, Write};
use std::sync::Arc;

use harness_agents::{AuthChoice, ProductManagerAgent, ProductManagerDraftIssue, ProviderConfig};
use harness_forge::{Forge, ItemNumber};
use harness_forge_forgejo::{ForgejoConfig, ForgejoForge};

use crate::product_chat::{
    ProductChatError, ProductChatOpenOptions, ProductChatSession, ProductManagerResponder,
};
use crate::product_chat_args::{AuthKind, ProductChatArgs};

pub fn run_repl(args: &ProductChatArgs) -> Result<(), ProductChatError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| ProductChatError::Runtime(error.to_string()))?;
    runtime.block_on(run_repl_async(args))
}

async fn run_repl_async(args: &ProductChatArgs) -> Result<(), ProductChatError> {
    let provider = ProviderConfig::from_auth(
        auth_choice(args.auth),
        args.codex_model.clone(),
        args.auth_file.clone(),
    )?;
    let agent = Arc::new(ProductManagerAgent::new(provider));
    let human_forge = Arc::new(build_forge(&args.base_url, &args.human_token));
    let product_forge = Arc::new(build_forge(&args.base_url, &args.product_manager_token));
    let mut session = ProductChatSession::open(
        human_forge,
        product_forge,
        agent,
        ProductChatOpenOptions {
            base_url: args.base_url.clone(),
            repo_path: args.repo.clone(),
            transcript_issue: args.transcript_issue.map(ItemNumber::new),
        },
    )
    .await?;
    println!(
        "Opened product conversation:\n  {}",
        session.transcript_url()
    );
    print_help();
    repl_loop(&mut session).await
}

fn build_forge(base_url: &str, token: &str) -> ForgejoForge {
    ForgejoForge::new(ForgejoConfig::new(base_url.to_string(), token.to_string()))
}

fn auth_choice(auth: AuthKind) -> AuthChoice {
    match auth {
        AuthKind::DeepSeek => AuthChoice::DeepSeek,
        AuthKind::ChatGptOAuth => AuthChoice::ChatGptOAuth,
        AuthKind::AnthropicOAuth => AuthChoice::AnthropicOAuth,
    }
}

async fn repl_loop<H, P, R>(
    session: &mut ProductChatSession<H, P, R>,
) -> Result<(), ProductChatError>
where
    H: Forge + ?Sized,
    P: Forge + ?Sized,
    R: ProductManagerResponder + ?Sized,
{
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
        if input == "/quit" {
            return Ok(());
        } else if input == "/help" {
            print_help();
        } else if input == "/issue" {
            println!("{}", session.transcript_url());
        } else if input == "/drafts" {
            print_drafts(session.latest_drafts());
        } else if let Some(raw) = input.strip_prefix("/file ") {
            handle_file_command(session, raw.trim()).await?;
        } else if input.starts_with('/') {
            println!("unknown command; try /help");
        } else {
            let response = session.send_human_turn(input).await?;
            println!("\nproduct-manager> {}\n", response.reply.trim());
            print_drafts(&response.drafts);
        }
    }
}

async fn handle_file_command<H, P, R>(
    session: &ProductChatSession<H, P, R>,
    raw: &str,
) -> Result<(), ProductChatError>
where
    H: Forge + ?Sized,
    P: Forge + ?Sized,
    R: ProductManagerResponder + ?Sized,
{
    let number = raw
        .parse::<usize>()
        .map_err(|_| ProductChatError::InvalidDraftNumber {
            requested: 0,
            available: session.latest_drafts().len(),
        })?;
    let outcome = session.file_draft(number).await?;
    let status = if outcome.created {
        "Filed"
    } else {
        "Already filed"
    };
    println!(
        "product-manager> {status} intake issue:\n  {}",
        session.issue_url_for(outcome.issue.number)
    );
    Ok(())
}

fn print_help() {
    println!("Commands: /drafts, /file <n>, /issue, /help, /quit");
}

fn print_drafts(drafts: &[ProductManagerDraftIssue]) {
    if drafts.is_empty() {
        println!("Drafts: (none)");
        return;
    }
    println!("Drafts:");
    for (index, draft) in drafts.iter().enumerate() {
        println!("[{}] {}", index + 1, draft.title);
        if let Some(rationale) = draft.rationale.as_deref().filter(|text| !text.is_empty()) {
            println!("    {rationale}");
        }
    }
}
