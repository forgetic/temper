//! What the LLMs do: scripts the fake provider plays, each cued by a word in
//! a conversation's system text. A run's main conversation is cued by its
//! job's word, which starts the guidance of the step it runs for, and so its
//! brief (which comes first in the system text); a sub-agent by the word its
//! asker writes at the start of its brief. A job with no word is played at
//! random.
//!
//! The scripts follow the fixture ([`crate::fixture`]): the answer in
//! `src/lib.rs` is 42 and the checks want 43.

use temper_llm_model::api::{Finish, Line, Script, Turn};

/// What a run is for, which picks the script of its main conversation.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Job {
    /// Reads, edits, runs a command, finishes with a change whose checks
    /// fail, fixes it, and finishes again; and once more if the push fails.
    Coding,
    /// Reads and searches, finishes with a verdict the run rejects, then
    /// with one it takes: changes asked for.
    Review,
    /// Reads, and finishes with a report.
    Reporting,
    /// Asks two read-only sub-agents side by side, then a writable one that
    /// fixes the code and asks one of its own (and tries to finish, which it
    /// may not), then finishes with a change.
    Delegating,
    /// Asks two sub-agents that read on and on, spending the run's turns
    /// between them.
    Spending,
    /// Played at random.
    Wandering,
}

/// Every job, for worlds that draw them.
pub const JOBS: [Job; 6] = [Job::Coding, Job::Review, Job::Reporting, Job::Delegating, Job::Spending, Job::Wandering];

/// The word that cues the script of `job`'s main conversation, if it has one.
#[must_use]
pub const fn cue(job: Job) -> Option<&'static [u8]> {
    match job {
        Job::Coding => Some(b"@coding"),
        Job::Review => Some(b"@review"),
        Job::Reporting => Some(b"@report"),
        Job::Delegating => Some(b"@delegate"),
        Job::Spending => Some(b"@spend"),
        Job::Wandering => None,
    }
}

/// Every script, for the provider.
#[must_use]
pub fn all() -> Box<[Script]> {
    Box::new([
        script(b"@coding", coding()),
        script(b"@review", review()),
        script(b"@report", reporting()),
        script(b"@delegate", delegating()),
        script(b"@spend", spending()),
        script(b"@explore", exploring()),
        script(b"@fix", fixing()),
        script(b"@burn", burning()),
    ])
}

/// The change every finishing script declares.
pub const TITLE: &[u8] = b"Make the answer 43";

fn script(cue: &[u8], turns: Vec<Turn>) -> Script {
    Script { cue: cue.into(), turns: turns.into() }
}

fn call(name: &str, arguments: &str) -> Line {
    Line::Call { name: name.as_bytes().into(), arguments: arguments.as_bytes().into() }
}

fn text(text: &str) -> Line {
    Line::Text { text: text.as_bytes().into() }
}

/// An answer that calls tools.
fn calls(lines: Vec<Line>) -> Turn {
    Turn { lines: lines.into(), finish: Finish::ToolCalls, tokens: 40 }
}

/// An answer that ends the turn saying `said`.
fn says(said: &str) -> Turn {
    Turn { lines: Box::new([text(said)]), finish: Finish::Stop, tokens: 20 }
}

fn read(path: &str) -> Line {
    call("read_file", &format!(r#"{{"path":"{path}"}}"#))
}

fn edit(old: &str, new: &str) -> Line {
    call("edit_file", &format!(r#"{{"path":"src/lib.rs","old":"{old}","new":"{new}"}}"#))
}

fn change(body: &str) -> Line {
    let title = String::from_utf8_lossy(TITLE);
    call("finish", &format!(r#"{{"title":"{title}","body":"{body}"}}"#))
}

fn sub_agent(brief: &str, tools: &str, more: &str) -> Line {
    call("sub_agent", &format!(r#"{{"brief":"{brief}","tools":"{tools}"{more}}}"#))
}

fn coding() -> Vec<Turn> {
    vec![
        calls(vec![text("Let me look."), read("src/lib.rs"), call("list_dir", r#"{"path":"."}"#)]),
        calls(vec![edit("42", "41")]),
        // A tool that is not offered, beside the command: answered with its
        // problem, while the command runs.
        calls(vec![call("run_shell", r#"{"command":"cargo test"}"#), call("delete_repository", "{}")]),
        calls(vec![change("The answer is 41 now.")]),
        // The checks failed: they want 43.
        calls(vec![edit("41", "43")]),
        calls(vec![change("The answer is 43 now.")]),
        // The push failed: again.
        calls(vec![change("The answer is 43 now, pushed again.")]),
    ]
}

fn review() -> Vec<Turn> {
    vec![
        calls(vec![read("README.md"), call("search", r#"{"path":"src","pattern":"answer"}"#)]),
        // A broken call, and a verdict that asks for changes and names none.
        calls(vec![call("finish", r#"{"verdict":"request-changes","body":"Fix it."#)]),
        calls(vec![call("finish", r#"{"verdict":"request-changes","body":"Fix it."}"#)]),
        calls(vec![call(
            "finish",
            r#"{"verdict":"request-changes","body":"Fix it.","1.kind":"blocking","1.path":"src/lib.rs","1.body":"43"}"#,
        )]),
    ]
}

fn reporting() -> Vec<Turn> {
    vec![
        calls(vec![read("README.md"), call("list_dir", r#"{"path":"."}"#)]),
        calls(vec![call("finish", r#"{"verdict":"report","body":"The answer is 42, and the checks want 43."}"#)]),
    ]
}

fn delegating() -> Vec<Turn> {
    vec![
        calls(vec![
            sub_agent("@explore Find where the answer is.", "inspect", ""),
            sub_agent("@explore Find what the checks want.", "inspect", r#","llm":"fake-2""#),
        ]),
        calls(vec![sub_agent("@fix Make the answer 43.", "inspect,modify,shell", r#","agents":"yes""#)]),
        calls(vec![change("A sub-agent made the answer 43.")]),
    ]
}

fn exploring() -> Vec<Turn> {
    // A long answer, which a run may cut to its limit.
    let answer = "The answer is in src/lib.rs, and should be 43. ".repeat(8);
    vec![calls(vec![read("src/lib.rs"), read("AGENTS.md")]), says(&answer)]
}

fn fixing() -> Vec<Turn> {
    vec![
        calls(vec![read("src/lib.rs")]),
        calls(vec![edit("42", "43")]),
        // A sub-agent may not finish: the call is answered as no tool's.
        calls(vec![sub_agent("@explore Check that the answer is 43.", "inspect", ""), change("Done.")]),
        says("The answer is 43 now."),
    ]
}

fn spending() -> Vec<Turn> {
    vec![
        calls(vec![
            sub_agent("@burn Read everything.", "inspect", ""),
            sub_agent("@burn Read everything again.", "inspect", ""),
        ]),
        calls(vec![read("README.md")]),
    ]
}

fn burning() -> Vec<Turn> {
    (0..64).map(|_| calls(vec![read("src/lib.rs")])).collect()
}
