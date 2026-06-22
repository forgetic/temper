//! Deterministic jig LLM for `examples/reference-delivery/run.sh`.
//!
//! The operator demo exercises the real Forgejo/runner/daemon/worker path, but
//! it should not depend on real provider credentials or non-deterministic model
//! behavior. This tiny process serves an OpenAI-compatible jig endpoint with
//! canned role behavior for the reference-delivery workflow.

use std::collections::BTreeSet;
use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use jig_core::{Reply, RequestView, Script, StopReason, Turn, Usage};
use jig_server::FakeLlm;
use serde_json::json;

const GREETING_FILE: &str = "REFERENCE_DELIVERY_GREETING.md";

fn main() -> Result<(), Box<dyn Error>> {
    let url_file = parse_url_file(env::args().skip(1))?;
    let fake = FakeLlm::start(Script::rule(reference_delivery_reply))?;
    let base_url = fake.base_url();
    if let Some(path) = url_file {
        fs::write(path, format!("{base_url}\n"))?;
    }
    println!("{base_url}");
    eprintln!("reference-delivery jig LLM listening at {base_url}");

    loop {
        thread::sleep(Duration::from_secs(3600));
        let _ = fake.requests().len();
    }
}

fn parse_url_file<I>(mut args: I) -> Result<Option<PathBuf>, Box<dyn Error>>
where
    I: Iterator<Item = String>,
{
    let mut url_file = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--url-file" => {
                let value = args.next().ok_or("--url-file expects a path")?;
                url_file = Some(PathBuf::from(value));
            }
            "--help" | "-h" => {
                println!("usage: temper-reference-delivery-jig [--url-file PATH]");
                std::process::exit(0);
            }
            other => return Err(format!("unrecognized argument '{other}'").into()),
        }
    }
    Ok(url_file)
}

fn reference_delivery_reply(view: &RequestView) -> Reply {
    let text = conversation_text(view);
    if has_role(&text, "architect") {
        architect_reply(&text)
    } else if has_role(&text, "engineer") {
        engineer_reply(view, &text)
    } else if has_role(&text, "reviewer") {
        reviewer_reply()
    } else {
        Reply::text(json!({ "summary": "No reference-delivery role was detected." }).to_string())
    }
}

fn conversation_text(view: &RequestView) -> String {
    view.messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn has_role(text: &str, role: &str) -> bool {
    text.contains(&format!("Role: {role}"))
        || text.contains(&format!("ROLE: {role}"))
        || text.contains(&format!("\"role\":\"{role}\""))
        || text.contains(&format!("\"role\": \"{role}\""))
}

fn architect_reply(text: &str) -> Reply {
    let target_repos = target_repos_from_intake(text);
    if target_repos.len() > 1 {
        let children = target_repos
            .iter()
            .map(|repo| {
                json!({
                    "slug": slug_for_repo(repo),
                    "title": format!("Add reference-delivery greeting to {repo}"),
                    "body": child_spec(repo),
                    "labels": ["code", "ready"],
                    "depends_on": [],
                    "target_repo": repo,
                })
            })
            .collect::<Vec<_>>();
        Reply::text(
            json!({
                "verdict": "needs_breakdown",
                "summary": "Split the coordinated intake into one ready code issue per target repository.",
                "children": children,
            })
            .to_string(),
        )
    } else {
        let repo = target_repos
            .first()
            .cloned()
            .or_else(|| first_repo_path(text))
            .unwrap_or_else(|| "acme/service".to_string());
        Reply::text(
            json!({
                "verdict": "ready_code",
                "summary": "Rewrote the intake as a deterministic code specification.",
                "body": child_spec(&repo),
            })
            .to_string(),
        )
    }
}

fn engineer_reply(view: &RequestView, text: &str) -> Reply {
    let repo = first_repo_path(text).unwrap_or_else(|| "acme/service".to_string());
    let dir = first_repo_dir(text).unwrap_or_else(|| repo_name(&repo).to_string());
    if view.prior_tool_results == 0 {
        let path = format!("{dir}/{GREETING_FILE}");
        let content = format!(
            "# Reference delivery greeting\n\nHello from {repo} via Temper reference delivery.\n"
        );
        Reply {
            turns: vec![Turn::ToolCall {
                id: "call_write_reference_delivery_greeting".to_string(),
                name: "write".to_string(),
                args: json!({ "path": path, "content": content }),
            }],
            usage: Usage {
                prompt_tokens: 16,
                completion_tokens: 8,
            },
            stop: StopReason::ToolCalls,
        }
    } else {
        Reply::text(
            json!({
                "summary": format!("Created {GREETING_FILE} with the deterministic reference-delivery greeting for {repo}."),
            })
            .to_string(),
        )
    }
}

fn reviewer_reply() -> Reply {
    Reply::text(
        json!({
            "verdict": "approve",
            "summary": "Approved the deterministic reference-delivery product diff.",
            "review_body": "Approved: the PR contains the expected reference-delivery greeting file and CI is green.",
        })
        .to_string(),
    )
}

fn target_repos_from_intake(text: &str) -> Vec<String> {
    let mut repos = BTreeSet::new();
    let mut rest = text;
    while let Some(index) = rest.find("`target_repo`: `") {
        rest = &rest[index + "`target_repo`: `".len()..];
        if let Some(end) = rest.find('`') {
            let repo = &rest[..end];
            if is_repo_path(repo) {
                repos.insert(repo.to_string());
            }
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }
    repos.into_iter().collect()
}

fn first_repo_path(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("- ") || !trimmed.contains("(dir:") {
            continue;
        }
        let repo = trimmed[2..].split_whitespace().next().unwrap_or_default();
        if is_repo_path(repo) {
            return Some(repo.to_string());
        }
    }
    None
}

fn first_repo_dir(text: &str) -> Option<String> {
    for line in text.lines() {
        let Some(start) = line.find("(dir: ") else {
            continue;
        };
        let rest = &line[start + "(dir: ".len()..];
        let dir = rest.split('/').next().unwrap_or_default().trim();
        if !dir.is_empty() && is_safe_relative_component(dir) {
            return Some(dir.to_string());
        }
    }
    None
}

fn child_spec(repo: &str) -> String {
    format!(
        "Create `{GREETING_FILE}` in `{repo}` containing the exact line \
         `Hello from {repo} via Temper reference delivery.`. This is the \
         deterministic product diff used by the reference-delivery demo."
    )
}

fn slug_for_repo(repo: &str) -> String {
    repo_name(repo)
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn repo_name(repo: &str) -> &str {
    repo.rsplit_once('/').map(|(_, name)| name).unwrap_or(repo)
}

fn is_repo_path(value: &str) -> bool {
    let Some((owner, name)) = value.split_once('/') else {
        return false;
    };
    !owner.is_empty() && !name.is_empty() && !name.contains('/')
}

fn is_safe_relative_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
}
