//! Typed boundary tests through the public conversions.

use jig_core_accounts as accounts;
use skein_lib::{Duration, ReplyTo, Token};
use smith_domain as smith;
use smith_domain_run as run;
use temper_engine_domain::engine;
use temper_engine_domain_brief as brief;
use temper_engine_domain_tasks as tasks;

use crate::{ChangeResource, Problem, answer, call, result, start, tools};

fn decoded(tool: &[u8], json: &[u8]) -> Result<engine::Call, Problem> {
    let input = run::HostInput::attested(json.into()).expect("bounded object exterior");
    call(run::CallName { activation: 7, completion: 2, position: 1 }, tool, &input)
}

fn assignment() -> engine::Assignment {
    engine::Assignment {
        task: 23,
        attempt: 7,
        charter: 1,
        run: Box::new(engine::RunCharter {
            policy: engine::RunPolicy {
                instructions: b"Do the task".as_slice().into(),
                waiting: Duration::from_secs(1),
                resume: true,
                turns: 8,
                time: Duration::from_secs(60),
                model: engine::Model {
                    dialect: 1,
                    account: 1,
                    endpoint: 1,
                    name: b"scripted".as_slice().into(),
                    max_tokens: 1024,
                    input_price: 1,
                    cached_price: 1,
                    output_price: 1,
                    price_unit: 1000,
                },
                alternatives: Box::new([]),
                inspect: true,
                modify: false,
                shell: false,
                agents: false,
                call_timeout: Duration::from_secs(10),
            },
            contract: tasks::Contract::Report { words: 128 },
            authority: tasks::Authority {
                tools: tasks::Tools(1),
                grants: Box::new([]),
                delegation: tasks::Delegation { kinds: Box::new([]), tasks: 0, depth: 0 },
                budget: tasks::Budget { spend: 40, deadline: None },
                notes: tasks::Scopes(0),
            },
            budget: 40,
        }),
        sections: Box::new([engine::BriefSection {
            kind: engine::BriefKind::Core(brief::Core::Task),
            body: engine::BriefBody::Text(b"Say hello".as_slice().into()),
        }]),
        inbox: Box::new([]),
        saved: Box::new([]),
        workspace: engine::ForgeWorkspace { key: Box::new([]), repositories: Box::new([]) },
        transcript: Box::new([]),
        answered: Box::new([]),
        grant: accounts::Grant { account: 1, generation: 2, valid: Duration::from_secs(30) },
    }
}

#[test]
fn root_charter_becomes_a_smith_start_with_ordered_brief_and_exact_budget() {
    let smith::Event::Start { activation, charter, grants, transcript, .. } =
        start(&assignment(), None, None, ReplyTo::new(Token::new(50)), Token::new(23))
    else {
        panic!("start conversion produces Smith Start")
    };
    assert_eq!(activation, 7);
    assert_eq!(charter.instructions.as_ref(), b"Do the task");
    assert_eq!(charter.brief.sections[0].title.as_ref(), b"Task");
    assert_eq!(charter.brief.sections[0].text.as_ref(), b"Say hello");
    assert_eq!(charter.budget.spend, 40);
    assert_eq!(charter.grants.host_tools.len(), tools(Duration::from_secs(10)).len());
    assert_eq!(grants[0].name.generation, 2);
    assert!(transcript.is_none());
}

#[test]
fn message_schema_decodes_a_named_question_and_rejects_a_malformed_body() {
    assert_eq!(
        decoded(b"message", br#"{"target":23,"form":"question","words":"Ready?"}"#),
        Ok(engine::Call {
            completion: 2,
            position: 1,
            tool: engine::Tool::Message {
                target: 23,
                form: engine::MessageForm::Question,
                words: b"Ready?".as_slice().into(),
            },
        })
    );
    assert_eq!(decoded(b"message", br#"{"target":0,"form":"words","words":"x"}"#), Err(Problem::Range));
    assert_eq!(decoded(b"message", br#"{"target":23,,"form":"words","words":"x"}"#), Err(Problem::Malformed));
}

#[test]
fn every_declared_tool_decodes_its_minimum_shape_and_rejects_a_wrong_shape() {
    let cases: &[(&[u8], &[u8], &[u8])] = &[
        (b"read_items", br#"{"repository":{"forge":1,"repository":2},"read":{"since":0,"page":1}}"#, br#"{"repository":{"forge":1,"repository":2},"read":{}}"#),
        (b"read_item", br#"{"repository":{"forge":1,"repository":2},"read":{"number":1,"after":0}}"#, br#"{"repository":{"forge":1,"repository":2},"read":{}}"#),
        (b"read_pull", br#"{"repository":{"forge":1,"repository":2},"read":{"number":1}}"#, br#"{"repository":{"forge":1,"repository":2},"read":{}}"#),
        (b"read_pull_for", br#"{"repository":{"forge":1,"repository":2},"read":{"head":"h","base":"b"}}"#, br#"{"repository":{"forge":1,"repository":2},"read":{}}"#),
        (b"read_reviews", br#"{"repository":{"forge":1,"repository":2},"read":{"number":1,"page":1}}"#, br#"{"repository":{"forge":1,"repository":2},"read":{}}"#),
        (b"read_statuses", br#"{"repository":{"forge":1,"repository":2},"read":{"commit":"0000000000000000000000000000000000000000000000000000000000000000","page":1}}"#, br#"{"repository":{"forge":1,"repository":2},"read":{}}"#),
        (b"read_remarks", br#"{"repository":{"forge":1,"repository":2},"read":{"number":1,"review":1,"page":1}}"#, br#"{"repository":{"forge":1,"repository":2},"read":{}}"#),
        (b"read_branch", br#"{"repository":{"forge":1,"repository":2},"read":{"branch":"main"}}"#, br#"{"repository":{"forge":1,"repository":2},"read":{}}"#),
        (b"read_branches", br#"{"repository":{"forge":1,"repository":2},"read":{}}"#, br#"{"repository":{"forge":1,"repository":2},"read":{"kind":"wrong"}}"#),
        (b"read_pull_files", br#"{"repository":{"forge":1,"repository":2},"read":{"number":1,"head":"0000000000000000000000000000000000000000000000000000000000000000","page":1}}"#, br#"{"repository":{"forge":1,"repository":2},"read":{}}"#),
        (b"read_compare", br#"{"repository":{"forge":1,"repository":2},"read":{"before":"0000000000000000000000000000000000000000000000000000000000000000","after":"0000000000000000000000000000000000000000000000000000000000000000"}}"#, br#"{"repository":{"forge":1,"repository":2},"read":{}}"#),
        (b"read_checks", br#"{"repository":{"forge":1,"repository":2},"read":{"commit":"0000000000000000000000000000000000000000000000000000000000000000"}}"#, br#"{"repository":{"forge":1,"repository":2},"read":{}}"#),
        (b"read_job", br#"{"repository":{"forge":1,"repository":2},"read":{"head":"0000000000000000000000000000000000000000000000000000000000000000","run":1,"job":1,"attempt":1,"max_bytes":32}}"#, br#"{"repository":{"forge":1,"repository":2},"read":{}}"#),
        (b"read_protection", br#"{"repository":{"forge":1,"repository":2},"read":{"branch":"main"}}"#, br#"{"repository":{"forge":1,"repository":2},"read":{}}"#),
        (b"read_settings", br#"{"repository":{"forge":1,"repository":2},"read":{}}"#, br#"{"repository":{"forge":1,"repository":2},"read":{"kind":"wrong"}}"#),
        (b"read_collaborators", br#"{"repository":{"forge":1,"repository":2},"read":{"page":1}}"#, br#"{"repository":{"forge":1,"repository":2},"read":{}}"#),
        (b"read_permission", br#"{"repository":{"forge":1,"repository":2},"read":{"user":1}}"#, br#"{"repository":{"forge":1,"repository":2},"read":{}}"#),
        (b"effect_forge", br#"{"repository":{"forge":1,"repository":2},"resource":{"kind":"repository"},"write":{"kind":"close","number":3}}"#, br#"{"repository":{"forge":1,"repository":2},"resource":{},"write":{"kind":"close","number":3}}"#),
        (b"subscribe_forge", br#"{"topic":{"repository":{"forge":1,"repository":2},"kind":"pull","number":3}}"#, br#"{"topic":{}}"#),
        (b"delegate", br#"{"batch":[]}"#, br#"{"batch":{}}"#),
        (b"amend", br#"{"target":3,"amendment":{"reason":"x"}}"#, br#"{"target":3,"amendment":{}}"#),
        (b"message", br#"{"target":3,"form":"words","words":"x"}"#, br#"{"target":3,"form":"unknown","words":"x"}"#),
        (b"cancel", br#"{"target":3,"reason":"x"}"#, br#"{"target":3}"#),
        (b"release", br#"{"target":3}"#, br#"{"target":0}"#),
        (b"introduce", br#"{"left":3,"right":4}"#, br#"{"left":3}"#),
        (b"unsubscribe", br#"{"subscription":3}"#, br#"{"subscription":"x"}"#),
        (b"withdraw", br#"{"proposal":3}"#, br#"{"proposal":0}"#),
        (b"decide_escalation", br#"{"task":3,"revision":1,"decision":"release"}"#, br#"{"task":3,"revision":1,"decision":"unknown"}"#),
        (b"decide", br#"{"proposer":3,"proposal":1,"decision":"accept"}"#, br#"{"proposer":3,"proposal":1,"decision":"unknown"}"#),
        (b"subscribe", br#"{"kind":"task","target":3}"#, br#"{"kind":"task"}"#),
        (b"propose", br#"{"action":"release","task":3,"reason":"x"}"#, br#"{"action":"release","reason":"x"}"#),
    ];
    let declared = tools(Duration::from_secs(10));
    assert_eq!(declared.len(), cases.len());
    for (name, valid, invalid) in cases {
        let mut found = false;
        for tool in &declared {
            if tool.name.as_ref() == *name {
                found = true;
            }
        }
        assert!(found, "tool declared: {name:?}");
        assert!(decoded(name, valid).is_ok(), "valid input for {name:?}");
        assert!(decoded(name, invalid).is_err(), "invalid input for {name:?}");
    }
}

fn spend(units: u64) -> run::Spend {
    run::Spend { turns: 1, input: 2, output: 3, cache_read: 0, cache_write: 0, units }
}

fn change_answer() -> run::Answer {
    run::Answer::Accepted {
        outcome: run::outcome::Declared::Change(run::outcome::Change {
            fields: Box::new([
                run::outcome::Field { name: b"title".as_slice().into(), value: b"Fix".as_slice().into() },
                run::outcome::Field { name: b"body".as_slice().into(), value: b"Details".as_slice().into() },
            ]),
        }),
        spent: spend(9),
        turns: 1,
    }
}

#[test]
fn smith_terminals_preserve_charge_and_the_root_result_contract() {
    assert_eq!(
        result(run::Answer::Parked { spent: spend(5), turns: 1 }, None),
        Ok(crate::Terminal { cumulative: 5, turns: 1, end: tasks::End::Parked })
    );
    assert_eq!(
        result(
            run::Answer::Accepted {
                outcome: run::outcome::Declared::Report(run::outcome::Report {
                    text: b"done".as_slice().into(),
                    fields: Box::new([]),
                }),
                spent: spend(7),
                turns: 1,
            },
            None,
        ),
        Ok(crate::Terminal {
            cumulative: 7,
            turns: 1,
            end: tasks::End::Finished {
                result: tasks::TaskResult::Report { words: b"done".as_slice().into() },
                cancel_delegates: false,
            },
        })
    );
    assert_eq!(result(change_answer(), None), Err(Problem::Missing));
    assert_eq!(
        result(change_answer(), Some(ChangeResource { connector: 1, kind: 2, resource: 42 })),
        Ok(crate::Terminal {
            cumulative: 9,
            turns: 1,
            end: tasks::End::Finished {
                result: tasks::TaskResult::Change {
                    connector: 1,
                    kind: 2,
                    resource: 42,
                    words: b"Details".as_slice().into(),
                },
                cancel_delegates: false,
            },
        })
    );
}

#[test]
fn root_call_answers_become_bounded_smith_text_or_error() {
    let done = answer(&temper_engine_domain::CallAnswer::Controlled);
    assert_eq!(done.text(), b"Done");
    assert!(!done.error());
    let denied = answer(&temper_engine_domain::CallAnswer::Unavailable);
    assert!(denied.error());
    assert!(!denied.text().is_empty());
}
