//! What a run tells its LLMs (agent-model.md, 4.1): the system text, which is
//! the brief (the charter's for main, the asker's for a sub-agent), then what
//! the run found in its checkout (each repository's `AGENTS.md`), then the
//! sections on the run's own mechanics (its tools, its checkout, its
//! sub-agents, and how to finish, or for a sub-agent how to answer); and the
//! nudges. The brief and the guides go in as
//! they came; the rest is rendered here from constant fragments and the
//! charter's data, its labels verbatim.
//!
//! A text is rendered twice: once to measure it, then into a [`Writer`] of
//! exactly that length (programming-model.md, 8).

use alloc::boxed::Box;

use temper_lib::Writer;

use crate::boundary::Stop;
use crate::charter::{Charter, Families, Llm, Repository, Tools};
use crate::outcome::{ChangeSpec, Children, OutcomeSpec, VerdictRule};
use crate::prepare::{self, Found, Guide};

/// The first user message of a main conversation.
pub(crate) const BEGIN: &[u8] = b"Begin the work your brief describes.";

/// The system text of a run's main conversation, given what the run found in
/// its checkout.
pub(crate) fn system(charter: &Charter, found: &Found) -> Box<[u8]> {
    let families = Families::of(&charter.grants);
    let mut measured = Text::measuring();
    render_system(&mut measured, charter, found, &charter.brief, families, true);
    let mut text = measured.writing();
    render_system(&mut text, charter, found, &charter.brief, families, true);
    text.finish()
}

/// The system text of a sub-agent asked for with `brief` and `families`.
pub(crate) fn child(charter: &Charter, found: &Found, brief: &[u8], families: Families) -> Box<[u8]> {
    let mut measured = Text::measuring();
    render_system(&mut measured, charter, found, brief, families, false);
    let mut text = measured.writing();
    render_system(&mut text, charter, found, brief, families, false);
    text.finish()
}

/// What a run says to an LLM that stopped for `stop` without finishing, in its
/// nudge numbered `nudge` of `nudges`.
pub(crate) fn nudge(stop: Stop, nudge: u32, nudges: u32) -> Box<[u8]> {
    let mut measured = Text::measuring();
    render_nudge(&mut measured, stop, nudge, nudges);
    let mut text = measured.writing();
    render_nudge(&mut text, stop, nudge, nudges);
    text.finish()
}

/// The system text of main, or of a sub-agent: on `brief`, with `families`.
fn render_system(text: &mut Text, charter: &Charter, found: &Found, brief: &[u8], families: Families, main: bool) {
    if !brief.is_empty() {
        text.put(brief);
        end_paragraph(text, brief);
    }
    let repositories = &charter.checkout.repositories;
    for guide in &found.guides {
        render_guide(text, repositories, guide);
    }
    render_tools(text, families.tools);
    text.put(b"\n");
    render_checkout(text, repositories, found.checks.as_slice());
    text.put(b"\n");
    if families.agents {
        render_agents(text, &charter.models);
        text.put(b"\n");
    }
    if main {
        render_finishing(text, &charter.outcome, !found.checks.is_empty());
    } else {
        text.put(b"## Answering\n\nWhen you are done, end your turn with your answer: your last message goes, as it ");
        text.put(b"is, to the LLM that asked for you, and you are done.\n");
    }
}

fn render_agents(text: &mut Text, models: &[Llm]) {
    text.put(b"## Sub-agents\n\n");
    text.put(
        b"You can ask for a sub-agent: an LLM of its own, working on a brief you write, with tools no wider than ",
    );
    text.put(b"yours and a share of the budget. Its last message comes back to you as the result. It runs on the ");
    if models.is_empty() {
        text.put(b"run's main LLM.\n");
        return;
    }
    text.put(b"run's main LLM unless you name one of these: ");
    let mut rest = models.len();
    for llm in models {
        text.put(b"`");
        text.put(&llm.model);
        text.put(b"`");
        rest = rest.saturating_sub(1);
        match rest {
            0 => {}
            1 => text.put(b" or "),
            _ => text.put(b", "),
        }
    }
    text.put(b".\n");
}

/// One blank line after a text that came as it is, whether or not it ends
/// its last line.
fn end_paragraph(text: &mut Text, came: &[u8]) {
    if !came.ends_with(b"\n") {
        text.put(b"\n");
    }
    text.put(b"\n");
}

fn render_guide(text: &mut Text, repositories: &[Repository], guide: &Guide) {
    text.put(b"## ");
    text.put(prepare::GUIDE);
    text.put(b" in `");
    text.put(&name(repositories, guide.repository).name);
    text.put(b"`\n\n");
    if !guide.text.is_empty() {
        text.put(&guide.text);
        end_paragraph(text, &guide.text);
    }
    if !guide.whole {
        text.put(b"(The file goes on: read the rest with your tools.)\n\n");
    }
}

fn render_tools(text: &mut Text, tools: Tools) {
    let Tools { inspect, modify, shell } = tools;
    text.put(b"## Tools\n\n");
    if inspect {
        text.put(b"You can read, list and search the files in the checkout.\n");
    }
    if modify {
        text.put(b"You can write and edit files in its writable repositories.\n");
    }
    if shell {
        text.put(b"You can run shell commands.\n");
    }
    if !inspect && !modify && !shell {
        text.put(b"You have no tools that act on the checkout.\n");
    }
}

fn render_checkout(text: &mut Text, repositories: &[Repository], checks: &[u32]) {
    text.put(b"## Checkout\n\n");
    if repositories.is_empty() {
        text.put(b"There is no checkout.\n");
    }
    let mut index: u32 = 0;
    for Repository { name, root: _, writable } in repositories {
        text.put(b"- `");
        text.put(name);
        text.put(if *writable { b"`, which you may change" } else { b"`, which you may only read" });
        if checks.contains(&index) {
            text.put(b", with checks (`");
            text.put(prepare::CHECKS);
            text.put(b"`)");
        }
        text.put(b"\n");
        index = index.saturating_add(1);
    }
}

fn render_finishing(text: &mut Text, spec: &OutcomeSpec, checks: bool) {
    text.put(b"## Finishing\n\n");
    text.put(
        b"When the work is done, call `finish` with its outcome. If the outcome does not fit what this run allows, ",
    );
    text.put(b"`finish` says what is wrong, and you can fix it and call `finish` again. Stopping without calling ");
    text.put(b"`finish` does not finish the run.\n");
    if let Some(ChangeSpec { checks: wanted }) = spec.change {
        text.put(b"\nYou can finish with a change: what you changed in the checkout, with a title and a body for ");
        text.put(b"its pull request.");
        if wanted && checks {
            text.put(b" First the checks of the repositories that have them run; if any fail, `finish` gives you ");
            text.put(b"their output, and you can carry on.");
        }
        text.put(b" Then the change is pushed; if its branch has moved since the run started, `finish` says so.\n");
    }
    if !spec.verdicts.is_empty() {
        text.put(b"\nYou can finish with one of these verdicts:\n\n");
        for rule in &spec.verdicts {
            render_verdict(text, rule);
        }
    }
}

fn name(repositories: &[Repository], index: u32) -> &Repository {
    let index = usize::try_from(index).expect("a u32 fits in a usize");
    repositories.get(index).expect("a guide is of a repository of the checkout")
}

/// A verdict and its contract, as one item of a list.
fn render_verdict(text: &mut Text, rule: &VerdictRule) {
    text.put(b"- `");
    text.put(&rule.name);
    text.put(b"`, with ");
    let Children { min, max } = rule.children;
    if max == 0 {
        text.put(b"no children\n");
        return;
    }
    if min == max {
        text.put(b"exactly ");
        text.put_decimal(min);
    } else if min == 0 {
        text.put(b"up to ");
        text.put_decimal(max);
    } else {
        text.put_decimal(min);
        text.put(b" to ");
        text.put_decimal(max);
    }
    text.put(if max == 1 { b" child, " } else { b" children, " });
    text.put(b"each of kind ");
    render_labels(text, &rule.kinds, b" or ");
    if !rule.fields.is_empty() {
        text.put(if rule.fields.len() == 1 { b", each with the field " } else { b", each with the fields " });
        render_labels(text, &rule.fields, b" and ");
    }
    text.put(b"\n");
}

/// Labels in backticks, as a list in prose: `a`, `b` and `c`.
fn render_labels(text: &mut Text, labels: &[Box<[u8]>], last: &[u8]) {
    let mut rest = labels.len();
    for label in labels {
        text.put(b"`");
        text.put(label);
        text.put(b"`");
        rest = rest.saturating_sub(1);
        match rest {
            0 => {}
            1 => text.put(last),
            _ => text.put(b", "),
        }
    }
}

fn render_nudge(text: &mut Text, stop: Stop, nudge: u32, nudges: u32) {
    text.put(match stop {
        Stop::EndTurn => b"You stopped without calling `finish`, so the work is not done.",
        Stop::MaxTokens => b"Your answer was cut off: it ran out of tokens.",
        Stop::Refusal => b"You declined to go on.",
        Stop::NoCalls => b"You said you would call a tool, and named none.",
    });
    text.put(b" Carry on with the work, and call `finish` when it is done. This is reminder ");
    text.put_decimal(nudge);
    text.put(b" of ");
    text.put_decimal(nudges);
    text.put(b": if you stop again after the last one, the run ends unfinished.");
}

/// A text being rendered: measured first, then written into a box of exactly
/// the length measured.
struct Text {
    /// The bytes put so far.
    len: usize,
    /// Where they go, once the text has been measured.
    writer: Option<Writer>,
}

impl Text {
    fn measuring() -> Text {
        Text { len: 0, writer: None }
    }

    /// A text to write what this one measured.
    fn writing(self) -> Text {
        Text { len: 0, writer: Some(Writer::new(self.len)) }
    }

    fn put(&mut self, bytes: &[u8]) {
        self.len = self.len.saturating_add(bytes.len());
        if let Some(writer) = &mut self.writer {
            writer.put(bytes).expect("a text is written as it was measured");
        }
    }

    /// `n` in decimal digits.
    fn put_decimal(&mut self, n: u32) {
        let mut digits = [b'0'; 10];
        let mut rest = n;
        // The first digit to put: the leftmost that is not a leading zero.
        let mut first: usize = 9;
        for (index, digit) in digits.iter_mut().enumerate().rev() {
            let value = u8::try_from(rest.checked_rem(10).unwrap_or(0)).expect("a digit fits in a byte");
            *digit = b'0'.saturating_add(value);
            if value != 0 {
                first = index;
            }
            rest = rest.checked_div(10).unwrap_or(0);
        }
        self.put(digits.get(first..).unwrap_or_default());
    }

    fn finish(self) -> Box<[u8]> {
        self.writer.expect("a text is finished once written").finish()
    }
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;

    use temper_lib::Token;

    use super::{Text, child, nudge, system};
    use crate::boundary::Stop;
    use crate::charter::{Charter, Checkout, Families, Grants, Repository, Tools};
    use crate::outcome::{ChangeSpec, Children, OutcomeSpec, VerdictRule};
    use crate::prepare::{Found, Guide};
    use crate::tests::{bytes, charter, rule};

    /// A rendered text, to compare and print.
    #[expect(clippy::disallowed_methods, reason = "a test reads the text it checks")]
    fn text(bytes: &[u8]) -> &str {
        core::str::from_utf8(bytes).expect("the test charters are text")
    }

    /// What a run of the test charter found: a guide for `temper`, cut, and
    /// checks in `docs`.
    fn found() -> Found {
        let mut found = Found::with_capacity(2);
        let guide = Guide { repository: 0, text: bytes(b"Run `make test` before you finish."), whole: false };
        found.guides.push(guide).expect("room");
        found.checks.push(1).expect("room");
        found
    }

    #[test]
    fn the_system_text_is_the_brief_then_the_checkouts_guides_then_the_runs_mechanics() {
        let mut charter = charter();
        charter.checkout.repositories = Box::new([
            charter.checkout.repositories[0].clone(),
            Repository { name: bytes(b"docs"), root: Token::new(901), writable: true },
        ]);
        charter.outcome.change = Some(ChangeSpec { checks: true });
        let expected: &[u8] = b"Review the change.

## AGENTS.md in `temper`

Run `make test` before you finish.

(The file goes on: read the rest with your tools.)

## Tools

You can read, list and search the files in the checkout.
You can run shell commands.

## Checkout

- `temper`, which you may only read
- `docs`, which you may change, with checks (`.temper/pre-pr`)

## Finishing

When the work is done, call `finish` with its outcome. If the outcome does not fit what this run allows, \
`finish` says what is wrong, and you can fix it and call `finish` again. Stopping without calling `finish` \
does not finish the run.

You can finish with a change: what you changed in the checkout, with a title and a body for its pull request. \
First the checks of the repositories that have them run; if any fail, `finish` gives you their output, and you \
can carry on. Then the change is pushed; if its branch has moved since the run started, `finish` says so.

You can finish with one of these verdicts:

- `approve`, with no children
- `request`, with 1 to 8 children, each of kind `blocking` or `nit`, each with the fields `path` and `body`
";
        assert_eq!(text(&system(&charter, &found())), text(expected));
    }

    #[test]
    fn a_sub_agent_is_told_its_brief_its_tools_its_checkout_and_how_to_answer() {
        let mut charter = charter();
        charter.models = Box::new([crate::charter::Llm { model: bytes(b"model-b"), ..charter.llm.clone() }]);
        let families =
            Families { tools: Tools { inspect: true, modify: false, shell: false }, forge: false, agents: true };
        let expected: &[u8] = b"Find where tabs are parsed.

## Tools

You can read, list and search the files in the checkout.

## Checkout

- `temper`, which you may only read

## Sub-agents

You can ask for a sub-agent: an LLM of its own, working on a brief you write, with tools no wider than yours \
and a share of the budget. Its last message comes back to you as the result. It runs on the run's main LLM \
unless you name one of these: `model-b`.

## Answering

When you are done, end your turn with your answer: your last message goes, as it is, to the LLM that asked for \
you, and you are done.
";
        let found = Found::with_capacity(1);
        assert_eq!(text(&child(&charter, &found, b"Find where tabs are parsed.", families)), text(expected));
    }

    #[test]
    fn what_a_charter_leaves_out_is_said_plainly() {
        let charter = Charter {
            brief: bytes(b"Write the release notes.\n"),
            checkout: Checkout { repositories: Box::new([]) },
            grants: Grants { tools: Tools { inspect: false, modify: false, shell: false }, ..charter().grants },
            outcome: OutcomeSpec { change: Some(ChangeSpec { checks: true }), verdicts: Box::new([]) },
            ..charter()
        };
        let expected: &[u8] = b"Write the release notes.

## Tools

You have no tools that act on the checkout.

## Checkout

There is no checkout.

## Finishing

When the work is done, call `finish` with its outcome. If the outcome does not fit what this run allows, \
`finish` says what is wrong, and you can fix it and call `finish` again. Stopping without calling `finish` \
does not finish the run.

You can finish with a change: what you changed in the checkout, with a title and a body for its pull request. \
Then the change is pushed; if its branch has moved since the run started, `finish` says so.
";
        assert_eq!(text(&system(&charter, &Found::with_capacity(0))), text(expected));
    }

    fn contract(min: u32, max: u32, kinds: Box<[Box<[u8]>]>, fields: Box<[Box<[u8]>]>) -> VerdictRule {
        VerdictRule { children: Children { min, max }, kinds, fields, ..rule(b"split", 0, 0) }
    }

    #[test]
    fn a_verdicts_contract_is_said_as_it_reads() {
        let cases = [
            (
                contract(1, 1, Box::new([bytes(b"bug")]), Box::new([bytes(b"title")])),
                "- `split`, with exactly 1 child, each of kind `bug`, each with the field `title`\n",
            ),
            (
                contract(0, 3, Box::new([bytes(b"bug"), bytes(b"feature"), bytes(b"chore")]), Box::new([])),
                "- `split`, with up to 3 children, each of kind `bug`, `feature` or `chore`\n",
            ),
            (
                contract(
                    2,
                    2,
                    Box::new([bytes(b"bug")]),
                    Box::new([bytes(b"title"), bytes(b"body"), bytes(b"labels")]),
                ),
                "- `split`, with exactly 2 children, each of kind `bug`, each with the fields `title`, `body` and `labels`\n",
            ),
            (
                contract(10, 4_000_000_000, Box::new([bytes(b"bug")]), Box::new([])),
                "- `split`, with 10 to 4000000000 children, each of kind `bug`\n",
            ),
        ];
        for (verdict, expected) in cases {
            let spec = OutcomeSpec { change: None, verdicts: Box::new([verdict]) };
            let text = system(&Charter { outcome: spec, ..charter() }, &Found::with_capacity(1));
            let rendered = self::text(&text);
            assert!(rendered.ends_with(expected), "{rendered}");
        }
    }

    #[test]
    fn a_nudge_says_why_the_llm_stopped_and_how_many_nudges_are_left() {
        let rest = " Carry on with the work, and call `finish` when it is done. This is reminder 1 of 2: if you stop \
                    again after the last one, the run ends unfinished.";
        let cases = [
            (Stop::EndTurn, "You stopped without calling `finish`, so the work is not done."),
            (Stop::MaxTokens, "Your answer was cut off: it ran out of tokens."),
            (Stop::Refusal, "You declined to go on."),
            (Stop::NoCalls, "You said you would call a tool, and named none."),
        ];
        for (stop, first) in cases {
            let rendered = nudge(stop, 1, 2);
            let rendered = text(&rendered);
            assert!(rendered.starts_with(first) && rendered.ends_with(rest), "{rendered}");
            assert_eq!(rendered.len(), first.len() + rest.len());
        }
    }

    #[test]
    fn numbers_are_written_in_decimal() {
        for (n, expected) in [(0, "0"), (7, "7"), (10, "10"), (305, "305"), (u32::MAX, "4294967295")] {
            let mut measured = Text::measuring();
            measured.put_decimal(n);
            let mut written = measured.writing();
            written.put_decimal(n);
            assert_eq!(text(&written.finish()), expected);
        }
    }
}
