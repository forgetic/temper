#![expect(clippy::disallowed_methods, reason = "ordinary Rust tests append trailing bytes to golden fixtures")]
#![expect(clippy::disallowed_macros, reason = "ordinary Rust tests load fixed binary golden fixtures")]
//! Independent golden fixtures for every v2 schema and every enum arm.
use crate::{Sizes, payload::v2::*};
use alloc::boxed::Box;
use skein_lib::Duration;

#[test]
fn budget() {
    let sizes = Sizes::STARTING;
    let value = Budget { spend: 7, turns: 7, time: Duration::from_nanos(7) };
    let golden = include_bytes!("golden/budget.bin");
    assert_eq!(encode_budget(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_budget(golden, &sizes).unwrap(), value);
    assert!(decode_budget(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_budget(&trailing, &sizes).is_none());
}

#[test]
fn prices() {
    let sizes = Sizes::STARTING;
    let value = Prices { input: 7, cached: 7, output: 7, unit: 7 };
    let golden = include_bytes!("golden/prices.bin");
    assert_eq!(encode_prices(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_prices(golden, &sizes).unwrap(), value);
    assert!(decode_prices(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_prices(&trailing, &sizes).is_none());
}

#[test]
fn model() {
    let sizes = Sizes::STARTING;
    let value = Model {
        endpoint: 7,
        kind: ModelKind::Main,
        model: Box::from(*b"abc"),
        max_tokens: 7,
        prices: Prices { input: 7, cached: 7, output: 7, unit: 7 },
    };
    let golden = include_bytes!("golden/model.bin");
    assert_eq!(encode_model(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_model(golden, &sizes).unwrap(), value);
    assert!(decode_model(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_model(&trailing, &sizes).is_none());
}

#[test]
fn section() {
    let sizes = Sizes::STARTING;
    let value = Section { kind: SectionKind::Task, words: Box::from(*b"abc") };
    let golden = include_bytes!("golden/section.bin");
    assert_eq!(encode_section(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_section(golden, &sizes).unwrap(), value);
    assert!(decode_section(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_section(&trailing, &sizes).is_none());
}

#[test]
fn field_rule() {
    let sizes = Sizes::STARTING;
    let value = FieldRule { name: Box::from(*b"abc"), required: true };
    let golden = include_bytes!("golden/field_rule.bin");
    assert_eq!(encode_field_rule(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_field_rule(golden, &sizes).unwrap(), value);
    assert!(decode_field_rule(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_field_rule(&trailing, &sizes).is_none());
}

#[test]
fn verdict_rule() {
    let sizes = Sizes::STARTING;
    let value = VerdictRule {
        number: 7,
        name: Box::from(*b"abc"),
        fields: Box::from([FieldRule { name: Box::from(*b"abc"), required: true }]),
        follow_up_kinds: Box::from([7]),
        most_follow_ups: 7,
    };
    let golden = include_bytes!("golden/verdict_rule.bin");
    assert_eq!(encode_verdict_rule(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_verdict_rule(golden, &sizes).unwrap(), value);
    assert!(decode_verdict_rule(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_verdict_rule(&trailing, &sizes).is_none());
}

#[test]
fn charter() {
    let sizes = Sizes::STARTING;
    let value = Charter {
        instructions: Box::from(*b"abc"),
        brief: Box::from([Section { kind: SectionKind::Task, words: Box::from(*b"abc") }]),
        tools: 7,
        contract: Contract::Report { most: 7 },
        budget: Budget { spend: 7, turns: 7, time: Duration::from_nanos(7) },
        models: Box::from([Model {
            endpoint: 7,
            kind: ModelKind::Main,
            model: Box::from(*b"abc"),
            max_tokens: 7,
            prices: Prices { input: 7, cached: 7, output: 7, unit: 7 },
        }]),
        waiting: Duration::from_nanos(7),
    };
    let golden = include_bytes!("golden/charter.bin");
    assert_eq!(encode_charter(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_charter(golden, &sizes).unwrap(), value);
    assert!(decode_charter(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_charter(&trailing, &sizes).is_none());
}

#[test]
fn name() {
    let sizes = Sizes::STARTING;
    let value = Name { segments: Box::from([Box::from(*b"abc")]) };
    let golden = include_bytes!("golden/name.bin");
    assert_eq!(encode_name(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_name(golden, &sizes).unwrap(), value);
    assert!(decode_name(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_name(&trailing, &sizes).is_none());
}

#[test]
fn pattern() {
    let sizes = Sizes::STARTING;
    let value = Pattern { segments: Box::from([Box::from(*b"abc")]), last: Last::None };
    let golden = include_bytes!("golden/pattern.bin");
    assert_eq!(encode_pattern(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_pattern(golden, &sizes).unwrap(), value);
    assert!(decode_pattern(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_pattern(&trailing, &sizes).is_none());
}

#[test]
fn grant() {
    let sizes = Sizes::STARTING;
    let value = Grant {
        connector: 7,
        kind: 7,
        pattern: Pattern { segments: Box::from([Box::from(*b"abc")]), last: Last::None },
    };
    let golden = include_bytes!("golden/grant.bin");
    assert_eq!(encode_grant(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_grant(golden, &sizes).unwrap(), value);
    assert!(decode_grant(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_grant(&trailing, &sizes).is_none());
}

#[test]
fn delegation() {
    let sizes = Sizes::STARTING;
    let value = Delegation { kinds: Box::from([ExecutorKind::Charter { kind: 7 }]), tasks: 7, depth: 7 };
    let golden = include_bytes!("golden/delegation.bin");
    assert_eq!(encode_delegation(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_delegation(golden, &sizes).unwrap(), value);
    assert!(decode_delegation(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_delegation(&trailing, &sizes).is_none());
}

#[test]
fn authority() {
    let sizes = Sizes::STARTING;
    let value = Authority {
        tools: 7,
        grants: Box::from([Grant {
            connector: 7,
            kind: 7,
            pattern: Pattern { segments: Box::from([Box::from(*b"abc")]), last: Last::None },
        }]),
        delegation: Delegation { kinds: Box::from([ExecutorKind::Charter { kind: 7 }]), tasks: 7, depth: 7 },
        spend: 7,
        deadline: Some(7),
        notes: 7,
    };
    let golden = include_bytes!("golden/authority.bin");
    assert_eq!(encode_authority(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_authority(golden, &sizes).unwrap(), value);
    assert!(decode_authority(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_authority(&trailing, &sizes).is_none());
}

#[test]
fn resources() {
    let sizes = Sizes::STARTING;
    let value = Resources {
        read: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
        write: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
    };
    let golden = include_bytes!("golden/resources.bin");
    assert_eq!(encode_resources(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_resources(golden, &sizes).unwrap(), value);
    assert!(decode_resources(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_resources(&trailing, &sizes).is_none());
}

#[test]
fn wake() {
    let sizes = Sizes::STARTING;
    let value = Wake {
        own: WakeClass::All,
        related: WakeClass::All,
        subscribed: WakeClass::All,
        messages: WakeClass::All,
        count: 7,
        age: Some(Duration::from_nanos(7)),
    };
    let golden = include_bytes!("golden/wake.bin");
    assert_eq!(encode_wake(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_wake(golden, &sizes).unwrap(), value);
    assert!(decode_wake(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_wake(&trailing, &sizes).is_none());
}

#[test]
fn spec() {
    let sizes = Sizes::STARTING;
    let value = Spec {
        words: Box::from(*b"abc"),
        resources: Resources {
            read: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
            write: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
        },
        inputs: Box::from([7]),
    };
    let golden = include_bytes!("golden/spec.bin");
    assert_eq!(encode_spec(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_spec(golden, &sizes).unwrap(), value);
    assert!(decode_spec(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_spec(&trailing, &sizes).is_none());
}

#[test]
fn new_task() {
    let sizes = Sizes::STARTING;
    let value = NewTask {
        spec: Spec {
            words: Box::from(*b"abc"),
            resources: Resources {
                read: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
                write: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
            },
            inputs: Box::from([7]),
        },
        contract: TaskContract::Run { contract: Contract::Report { most: 7 } },
        executor: Executor::Agent { charter: 7 },
        authority: Authority {
            tools: 7,
            grants: Box::from([Grant {
                connector: 7,
                kind: 7,
                pattern: Pattern { segments: Box::from([Box::from(*b"abc")]), last: Last::None },
            }]),
            delegation: Delegation { kinds: Box::from([ExecutorKind::Charter { kind: 7 }]), tasks: 7, depth: 7 },
            spend: 7,
            deadline: Some(7),
            notes: 7,
        },
        dependencies: Box::from([Dependency::Task { number: 7 }]),
        references: Box::from([7]),
        wake: Wake {
            own: WakeClass::All,
            related: WakeClass::All,
            subscribed: WakeClass::All,
            messages: WakeClass::All,
            count: 7,
            age: Some(Duration::from_nanos(7)),
        },
        subscriptions: Box::from([Topic::Task { task: 7 }]),
        tracked: true,
        priority: 7,
    };
    let golden = include_bytes!("golden/new_task.bin");
    assert_eq!(encode_new_task(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_new_task(golden, &sizes).unwrap(), value);
    assert!(decode_new_task(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_new_task(&trailing, &sizes).is_none());
}

#[test]
fn amendment() {
    let sizes = Sizes::STARTING;
    let value = Amendment {
        spec: Some(Spec {
            words: Box::from(*b"abc"),
            resources: Resources {
                read: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
                write: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
            },
            inputs: Box::from([7]),
        }),
        wake: Some(Wake {
            own: WakeClass::All,
            related: WakeClass::All,
            subscribed: WakeClass::All,
            messages: WakeClass::All,
            count: 7,
            age: Some(Duration::from_nanos(7)),
        }),
        remove_dependencies: Box::from([7]),
        authority: Some(Authority {
            tools: 7,
            grants: Box::from([Grant {
                connector: 7,
                kind: 7,
                pattern: Pattern { segments: Box::from([Box::from(*b"abc")]), last: Last::None },
            }]),
            delegation: Delegation { kinds: Box::from([ExecutorKind::Charter { kind: 7 }]), tasks: 7, depth: 7 },
            spend: 7,
            deadline: Some(7),
            notes: 7,
        }),
        instructions: Some(Box::from(*b"abc")),
        procedure: Some(ProcedureParameters::Land {
            repository: Name { segments: Box::from([Box::from(*b"abc")]) },
            branch: Box::from(*b"abc"),
            base: Box::from(*b"abc"),
            head: [7; 32],
        }),
    };
    let golden = include_bytes!("golden/amendment.bin");
    assert_eq!(encode_amendment(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_amendment(golden, &sizes).unwrap(), value);
    assert!(decode_amendment(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_amendment(&trailing, &sizes).is_none());
}

#[test]
fn note() {
    let sizes = Sizes::STARTING;
    let value = Note { name: Box::from(*b"abc"), revision: 7, words: Box::from(*b"abc") };
    let golden = include_bytes!("golden/note.bin");
    assert_eq!(encode_note(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_note(golden, &sizes).unwrap(), value);
    assert!(decode_note(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_note(&trailing, &sizes).is_none());
}

#[test]
fn field() {
    let sizes = Sizes::STARTING;
    let value = Field { name: Box::from(*b"abc"), value: Box::from(*b"abc") };
    let golden = include_bytes!("golden/field.bin");
    assert_eq!(encode_field(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_field(golden, &sizes).unwrap(), value);
    assert!(decode_field(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_field(&trailing, &sizes).is_none());
}

#[test]
fn inbound() {
    let sizes = Sizes::STARTING;
    let value = Inbound {
        number: 7,
        message: Message::Result {
            from: 7,
            ending: Ending::Done,
            result: Outcome::Change { title: Box::from(*b"abc"), body: Box::from(*b"abc") },
        },
    };
    let golden = include_bytes!("golden/inbound.bin");
    assert_eq!(encode_inbound(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_inbound(golden, &sizes).unwrap(), value);
    assert!(decode_inbound(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_inbound(&trailing, &sizes).is_none());
}

#[test]
fn file() {
    let sizes = Sizes::STARTING;
    let value = File { path: Box::from(*b"abc"), content: Box::from(*b"abc") };
    let golden = include_bytes!("golden/file.bin");
    assert_eq!(encode_file(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_file(golden, &sizes).unwrap(), value);
    assert!(decode_file(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_file(&trailing, &sizes).is_none());
}

#[test]
fn ci_status() {
    let sizes = Sizes::STARTING;
    let value = CiStatus { name: Box::from(*b"abc"), passed: true, pending: true };
    let golden = include_bytes!("golden/ci_status.bin");
    assert_eq!(encode_ci_status(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_ci_status(golden, &sizes).unwrap(), value);
    assert!(decode_ci_status(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_ci_status(&trailing, &sizes).is_none());
}

#[test]
fn comment() {
    let sizes = Sizes::STARTING;
    let value = Comment { number: 7, words: Box::from(*b"abc") };
    let golden = include_bytes!("golden/comment.bin");
    assert_eq!(encode_comment(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_comment(golden, &sizes).unwrap(), value);
    assert!(decode_comment(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_comment(&trailing, &sizes).is_none());
}

#[test]
fn turn() {
    let sizes = Sizes::STARTING;
    let value = Turn { version: 7, body: Box::from(*b"abc"), spent: 7, read: Some(7) };
    let golden = include_bytes!("golden/turn.bin");
    assert_eq!(encode_turn(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_turn(golden, &sizes).unwrap(), value);
    assert!(decode_turn(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_turn(&trailing, &sizes).is_none());
}

#[test]
fn committed_call() {
    let sizes = Sizes::STARTING;
    let value = CommittedCall {
        call: 7,
        ask: Call::Delegate {
            batch: Box::from([NewTask {
                spec: Spec {
                    words: Box::from(*b"abc"),
                    resources: Resources {
                        read: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
                        write: Box::from([Resource::Named {
                            name: Name { segments: Box::from([Box::from(*b"abc")]) },
                        }]),
                    },
                    inputs: Box::from([7]),
                },
                contract: TaskContract::Run { contract: Contract::Report { most: 7 } },
                executor: Executor::Agent { charter: 7 },
                authority: Authority {
                    tools: 7,
                    grants: Box::from([Grant {
                        connector: 7,
                        kind: 7,
                        pattern: Pattern { segments: Box::from([Box::from(*b"abc")]), last: Last::None },
                    }]),
                    delegation: Delegation {
                        kinds: Box::from([ExecutorKind::Charter { kind: 7 }]),
                        tasks: 7,
                        depth: 7,
                    },
                    spend: 7,
                    deadline: Some(7),
                    notes: 7,
                },
                dependencies: Box::from([Dependency::Task { number: 7 }]),
                references: Box::from([7]),
                wake: Wake {
                    own: WakeClass::All,
                    related: WakeClass::All,
                    subscribed: WakeClass::All,
                    messages: WakeClass::All,
                    count: 7,
                    age: Some(Duration::from_nanos(7)),
                },
                subscriptions: Box::from([Topic::Task { task: 7 }]),
                tracked: true,
                priority: 7,
            }]),
        },
        answer: Served::Delegated { tasks: Box::from([7]) },
    };
    let golden = include_bytes!("golden/committed_call.bin");
    assert_eq!(encode_committed_call(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_committed_call(golden, &sizes).unwrap(), value);
    assert!(decode_committed_call(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_committed_call(&trailing, &sizes).is_none());
}

#[test]
fn transcript() {
    let sizes = Sizes::STARTING;
    let value = Transcript {
        turns: Box::from([Turn { version: 7, body: Box::from(*b"abc"), spent: 7, read: Some(7) }]),
        calls: Box::from([CommittedCall {
            call: 7,
            ask: Call::Delegate {
                batch: Box::from([NewTask {
                    spec: Spec {
                        words: Box::from(*b"abc"),
                        resources: Resources {
                            read: Box::from([Resource::Named {
                                name: Name { segments: Box::from([Box::from(*b"abc")]) },
                            }]),
                            write: Box::from([Resource::Named {
                                name: Name { segments: Box::from([Box::from(*b"abc")]) },
                            }]),
                        },
                        inputs: Box::from([7]),
                    },
                    contract: TaskContract::Run { contract: Contract::Report { most: 7 } },
                    executor: Executor::Agent { charter: 7 },
                    authority: Authority {
                        tools: 7,
                        grants: Box::from([Grant {
                            connector: 7,
                            kind: 7,
                            pattern: Pattern { segments: Box::from([Box::from(*b"abc")]), last: Last::None },
                        }]),
                        delegation: Delegation {
                            kinds: Box::from([ExecutorKind::Charter { kind: 7 }]),
                            tasks: 7,
                            depth: 7,
                        },
                        spend: 7,
                        deadline: Some(7),
                        notes: 7,
                    },
                    dependencies: Box::from([Dependency::Task { number: 7 }]),
                    references: Box::from([7]),
                    wake: Wake {
                        own: WakeClass::All,
                        related: WakeClass::All,
                        subscribed: WakeClass::All,
                        messages: WakeClass::All,
                        count: 7,
                        age: Some(Duration::from_nanos(7)),
                    },
                    subscriptions: Box::from([Topic::Task { task: 7 }]),
                    tracked: true,
                    priority: 7,
                }]),
            },
            answer: Served::Delegated { tasks: Box::from([7]) },
        }]),
    };
    let golden = include_bytes!("golden/transcript.bin");
    assert_eq!(encode_transcript(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_transcript(golden, &sizes).unwrap(), value);
    assert!(decode_transcript(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_transcript(&trailing, &sizes).is_none());
}

#[test]
fn model_kind_0() {
    let sizes = Sizes::STARTING;
    let value = ModelKind::Main;
    let golden = include_bytes!("golden/model_kind_0.bin");
    assert_eq!(encode_model_kind(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_model_kind(golden, &sizes).unwrap(), value);
    assert!(decode_model_kind(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_model_kind(&trailing, &sizes).is_none());
}

#[test]
fn model_kind_1() {
    let sizes = Sizes::STARTING;
    let value = ModelKind::Subagent;
    let golden = include_bytes!("golden/model_kind_1.bin");
    assert_eq!(encode_model_kind(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_model_kind(golden, &sizes).unwrap(), value);
    assert!(decode_model_kind(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_model_kind(&trailing, &sizes).is_none());
}

#[test]
fn section_kind_0() {
    let sizes = Sizes::STARTING;
    let value = SectionKind::Task;
    let golden = include_bytes!("golden/section_kind_0.bin");
    assert_eq!(encode_section_kind(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_section_kind(golden, &sizes).unwrap(), value);
    assert!(decode_section_kind(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_section_kind(&trailing, &sizes).is_none());
}

#[test]
fn section_kind_1() {
    let sizes = Sizes::STARTING;
    let value = SectionKind::Inputs;
    let golden = include_bytes!("golden/section_kind_1.bin");
    assert_eq!(encode_section_kind(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_section_kind(golden, &sizes).unwrap(), value);
    assert!(decode_section_kind(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_section_kind(&trailing, &sizes).is_none());
}

#[test]
fn section_kind_2() {
    let sizes = Sizes::STARTING;
    let value = SectionKind::Delegates;
    let golden = include_bytes!("golden/section_kind_2.bin");
    assert_eq!(encode_section_kind(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_section_kind(golden, &sizes).unwrap(), value);
    assert!(decode_section_kind(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_section_kind(&trailing, &sizes).is_none());
}

#[test]
fn section_kind_3() {
    let sizes = Sizes::STARTING;
    let value = SectionKind::Inbox;
    let golden = include_bytes!("golden/section_kind_3.bin");
    assert_eq!(encode_section_kind(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_section_kind(golden, &sizes).unwrap(), value);
    assert!(decode_section_kind(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_section_kind(&trailing, &sizes).is_none());
}

#[test]
fn section_kind_4() {
    let sizes = Sizes::STARTING;
    let value = SectionKind::Notes;
    let golden = include_bytes!("golden/section_kind_4.bin");
    assert_eq!(encode_section_kind(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_section_kind(golden, &sizes).unwrap(), value);
    assert!(decode_section_kind(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_section_kind(&trailing, &sizes).is_none());
}

#[test]
fn section_kind_5() {
    let sizes = Sizes::STARTING;
    let value = SectionKind::Resources;
    let golden = include_bytes!("golden/section_kind_5.bin");
    assert_eq!(encode_section_kind(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_section_kind(golden, &sizes).unwrap(), value);
    assert!(decode_section_kind(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_section_kind(&trailing, &sizes).is_none());
}

#[test]
fn section_kind_6() {
    let sizes = Sizes::STARTING;
    let value = SectionKind::Calls;
    let golden = include_bytes!("golden/section_kind_6.bin");
    assert_eq!(encode_section_kind(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_section_kind(golden, &sizes).unwrap(), value);
    assert!(decode_section_kind(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_section_kind(&trailing, &sizes).is_none());
}

#[test]
fn contract_0() {
    let sizes = Sizes::STARTING;
    let value = Contract::Report { most: 7 };
    let golden = include_bytes!("golden/contract_0.bin");
    assert_eq!(encode_contract(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_contract(golden, &sizes).unwrap(), value);
    assert!(decode_contract(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_contract(&trailing, &sizes).is_none());
}

#[test]
fn contract_1() {
    let sizes = Sizes::STARTING;
    let value = Contract::Verdict {
        verdicts: Box::from([VerdictRule {
            number: 7,
            name: Box::from(*b"abc"),
            fields: Box::from([FieldRule { name: Box::from(*b"abc"), required: true }]),
            follow_up_kinds: Box::from([7]),
            most_follow_ups: 7,
        }]),
    };
    let golden = include_bytes!("golden/contract_1.bin");
    assert_eq!(encode_contract(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_contract(golden, &sizes).unwrap(), value);
    assert!(decode_contract(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_contract(&trailing, &sizes).is_none());
}

#[test]
fn contract_2() {
    let sizes = Sizes::STARTING;
    let value = Contract::Change { checks: true };
    let golden = include_bytes!("golden/contract_2.bin");
    assert_eq!(encode_contract(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_contract(golden, &sizes).unwrap(), value);
    assert!(decode_contract(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_contract(&trailing, &sizes).is_none());
}

#[test]
fn last_0() {
    let sizes = Sizes::STARTING;
    let value = Last::None;
    let golden = include_bytes!("golden/last_0.bin");
    assert_eq!(encode_last(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_last(golden, &sizes).unwrap(), value);
    assert!(decode_last(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_last(&trailing, &sizes).is_none());
}

#[test]
fn last_1() {
    let sizes = Sizes::STARTING;
    let value = Last::Exact { segment: Box::from(*b"abc") };
    let golden = include_bytes!("golden/last_1.bin");
    assert_eq!(encode_last(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_last(golden, &sizes).unwrap(), value);
    assert!(decode_last(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_last(&trailing, &sizes).is_none());
}

#[test]
fn last_2() {
    let sizes = Sizes::STARTING;
    let value = Last::Open { prefix: Box::from(*b"abc") };
    let golden = include_bytes!("golden/last_2.bin");
    assert_eq!(encode_last(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_last(golden, &sizes).unwrap(), value);
    assert!(decode_last(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_last(&trailing, &sizes).is_none());
}

#[test]
fn executor_kind_0() {
    let sizes = Sizes::STARTING;
    let value = ExecutorKind::Charter { kind: 7 };
    let golden = include_bytes!("golden/executor_kind_0.bin");
    assert_eq!(encode_executor_kind(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_executor_kind(golden, &sizes).unwrap(), value);
    assert!(decode_executor_kind(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_executor_kind(&trailing, &sizes).is_none());
}

#[test]
fn executor_kind_1() {
    let sizes = Sizes::STARTING;
    let value = ExecutorKind::Procedure { kind: 7 };
    let golden = include_bytes!("golden/executor_kind_1.bin");
    assert_eq!(encode_executor_kind(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_executor_kind(golden, &sizes).unwrap(), value);
    assert!(decode_executor_kind(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_executor_kind(&trailing, &sizes).is_none());
}

#[test]
fn executor_kind_2() {
    let sizes = Sizes::STARTING;
    let value = ExecutorKind::Role { kind: 7 };
    let golden = include_bytes!("golden/executor_kind_2.bin");
    assert_eq!(encode_executor_kind(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_executor_kind(golden, &sizes).unwrap(), value);
    assert!(decode_executor_kind(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_executor_kind(&trailing, &sizes).is_none());
}

#[test]
fn branch_role_0() {
    let sizes = Sizes::STARTING;
    let value = BranchRole::Change;
    let golden = include_bytes!("golden/branch_role_0.bin");
    assert_eq!(encode_branch_role(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_branch_role(golden, &sizes).unwrap(), value);
    assert!(decode_branch_role(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_branch_role(&trailing, &sizes).is_none());
}

#[test]
fn branch_role_1() {
    let sizes = Sizes::STARTING;
    let value = BranchRole::Saved;
    let golden = include_bytes!("golden/branch_role_1.bin");
    assert_eq!(encode_branch_role(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_branch_role(golden, &sizes).unwrap(), value);
    assert!(decode_branch_role(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_branch_role(&trailing, &sizes).is_none());
}

#[test]
fn branch_role_2() {
    let sizes = Sizes::STARTING;
    let value = BranchRole::Runs;
    let golden = include_bytes!("golden/branch_role_2.bin");
    assert_eq!(encode_branch_role(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_branch_role(golden, &sizes).unwrap(), value);
    assert!(decode_branch_role(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_branch_role(&trailing, &sizes).is_none());
}

#[test]
fn resource_0() {
    let sizes = Sizes::STARTING;
    let value = Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } };
    let golden = include_bytes!("golden/resource_0.bin");
    assert_eq!(encode_resource(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_resource(golden, &sizes).unwrap(), value);
    assert!(decode_resource(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_resource(&trailing, &sizes).is_none());
}

#[test]
fn resource_1() {
    let sizes = Sizes::STARTING;
    let value =
        Resource::Own { repository: Name { segments: Box::from([Box::from(*b"abc")]) }, role: BranchRole::Change };
    let golden = include_bytes!("golden/resource_1.bin");
    assert_eq!(encode_resource(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_resource(golden, &sizes).unwrap(), value);
    assert!(decode_resource(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_resource(&trailing, &sizes).is_none());
}

#[test]
fn dependency_0() {
    let sizes = Sizes::STARTING;
    let value = Dependency::Task { number: 7 };
    let golden = include_bytes!("golden/dependency_0.bin");
    assert_eq!(encode_dependency(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_dependency(golden, &sizes).unwrap(), value);
    assert!(decode_dependency(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_dependency(&trailing, &sizes).is_none());
}

#[test]
fn dependency_1() {
    let sizes = Sizes::STARTING;
    let value = Dependency::Batch { index: 7 };
    let golden = include_bytes!("golden/dependency_1.bin");
    assert_eq!(encode_dependency(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_dependency(golden, &sizes).unwrap(), value);
    assert!(decode_dependency(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_dependency(&trailing, &sizes).is_none());
}

#[test]
fn wake_class_0() {
    let sizes = Sizes::STARTING;
    let value = WakeClass::All;
    let golden = include_bytes!("golden/wake_class_0.bin");
    assert_eq!(encode_wake_class(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_wake_class(golden, &sizes).unwrap(), value);
    assert!(decode_wake_class(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_wake_class(&trailing, &sizes).is_none());
}

#[test]
fn wake_class_1() {
    let sizes = Sizes::STARTING;
    let value = WakeClass::Critical;
    let golden = include_bytes!("golden/wake_class_1.bin");
    assert_eq!(encode_wake_class(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_wake_class(golden, &sizes).unwrap(), value);
    assert!(decode_wake_class(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_wake_class(&trailing, &sizes).is_none());
}

#[test]
fn wake_class_2() {
    let sizes = Sizes::STARTING;
    let value = WakeClass::Never;
    let golden = include_bytes!("golden/wake_class_2.bin");
    assert_eq!(encode_wake_class(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_wake_class(golden, &sizes).unwrap(), value);
    assert!(decode_wake_class(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_wake_class(&trailing, &sizes).is_none());
}

#[test]
fn topic_0() {
    let sizes = Sizes::STARTING;
    let value = Topic::Task { task: 7 };
    let golden = include_bytes!("golden/topic_0.bin");
    assert_eq!(encode_topic(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_topic(golden, &sizes).unwrap(), value);
    assert!(decode_topic(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_topic(&trailing, &sizes).is_none());
}

#[test]
fn topic_1() {
    let sizes = Sizes::STARTING;
    let value = Topic::Resource { connector: 7, resource: Name { segments: Box::from([Box::from(*b"abc")]) } };
    let golden = include_bytes!("golden/topic_1.bin");
    assert_eq!(encode_topic(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_topic(golden, &sizes).unwrap(), value);
    assert!(decode_topic(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_topic(&trailing, &sizes).is_none());
}

#[test]
fn topic_2() {
    let sizes = Sizes::STARTING;
    let value = Topic::Timer { at: 7, repeat: Some(Duration::from_nanos(7)) };
    let golden = include_bytes!("golden/topic_2.bin");
    assert_eq!(encode_topic(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_topic(golden, &sizes).unwrap(), value);
    assert!(decode_topic(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_topic(&trailing, &sizes).is_none());
}

#[test]
fn procedure_parameters_0() {
    let sizes = Sizes::STARTING;
    let value = ProcedureParameters::Land {
        repository: Name { segments: Box::from([Box::from(*b"abc")]) },
        branch: Box::from(*b"abc"),
        base: Box::from(*b"abc"),
        head: [7; 32],
    };
    let golden = include_bytes!("golden/procedure_parameters_0.bin");
    assert_eq!(encode_procedure_parameters(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_procedure_parameters(golden, &sizes).unwrap(), value);
    assert!(decode_procedure_parameters(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_procedure_parameters(&trailing, &sizes).is_none());
}

#[test]
fn procedure_parameters_1() {
    let sizes = Sizes::STARTING;
    let value = ProcedureParameters::Watch { topic: Topic::Task { task: 7 } };
    let golden = include_bytes!("golden/procedure_parameters_1.bin");
    assert_eq!(encode_procedure_parameters(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_procedure_parameters(golden, &sizes).unwrap(), value);
    assert!(decode_procedure_parameters(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_procedure_parameters(&trailing, &sizes).is_none());
}

#[test]
fn executor_0() {
    let sizes = Sizes::STARTING;
    let value = Executor::Agent { charter: 7 };
    let golden = include_bytes!("golden/executor_0.bin");
    assert_eq!(encode_executor(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_executor(golden, &sizes).unwrap(), value);
    assert!(decode_executor(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_executor(&trailing, &sizes).is_none());
}

#[test]
fn executor_1() {
    let sizes = Sizes::STARTING;
    let value = Executor::Procedure {
        connector: 7,
        kind: 7,
        parameters: ProcedureParameters::Land {
            repository: Name { segments: Box::from([Box::from(*b"abc")]) },
            branch: Box::from(*b"abc"),
            base: Box::from(*b"abc"),
            head: [7; 32],
        },
    };
    let golden = include_bytes!("golden/executor_1.bin");
    assert_eq!(encode_executor(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_executor(golden, &sizes).unwrap(), value);
    assert!(decode_executor(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_executor(&trailing, &sizes).is_none());
}

#[test]
fn executor_2() {
    let sizes = Sizes::STARTING;
    let value = Executor::Person { role: 7, question: Box::from(*b"abc"), choices: Box::from([Box::from(*b"abc")]) };
    let golden = include_bytes!("golden/executor_2.bin");
    assert_eq!(encode_executor(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_executor(golden, &sizes).unwrap(), value);
    assert!(decode_executor(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_executor(&trailing, &sizes).is_none());
}

#[test]
fn task_contract_0() {
    let sizes = Sizes::STARTING;
    let value = TaskContract::Run { contract: Contract::Report { most: 7 } };
    let golden = include_bytes!("golden/task_contract_0.bin");
    assert_eq!(encode_task_contract(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_task_contract(golden, &sizes).unwrap(), value);
    assert!(decode_task_contract(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_task_contract(&trailing, &sizes).is_none());
}

#[test]
fn task_contract_1() {
    let sizes = Sizes::STARTING;
    let value = TaskContract::Choice { choices: Box::from([Box::from(*b"abc")]), words: true };
    let golden = include_bytes!("golden/task_contract_1.bin");
    assert_eq!(encode_task_contract(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_task_contract(golden, &sizes).unwrap(), value);
    assert!(decode_task_contract(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_task_contract(&trailing, &sizes).is_none());
}

#[test]
fn decision_0() {
    let sizes = Sizes::STARTING;
    let value = Decision::Accept;
    let golden = include_bytes!("golden/decision_0.bin");
    assert_eq!(encode_decision(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_decision(golden, &sizes).unwrap(), value);
    assert!(decode_decision(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_decision(&trailing, &sizes).is_none());
}

#[test]
fn decision_1() {
    let sizes = Sizes::STARTING;
    let value = Decision::Reject { reason: Box::from(*b"abc") };
    let golden = include_bytes!("golden/decision_1.bin");
    assert_eq!(encode_decision(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_decision(golden, &sizes).unwrap(), value);
    assert!(decode_decision(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_decision(&trailing, &sizes).is_none());
}

#[test]
fn decision_2() {
    let sizes = Sizes::STARTING;
    let value = Decision::Pass;
    let golden = include_bytes!("golden/decision_2.bin");
    assert_eq!(encode_decision(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_decision(golden, &sizes).unwrap(), value);
    assert!(decode_decision(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_decision(&trailing, &sizes).is_none());
}

#[test]
fn message_kind_0() {
    let sizes = Sizes::STARTING;
    let value = MessageKind::Words;
    let golden = include_bytes!("golden/message_kind_0.bin");
    assert_eq!(encode_message_kind(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_message_kind(golden, &sizes).unwrap(), value);
    assert!(decode_message_kind(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_message_kind(&trailing, &sizes).is_none());
}

#[test]
fn message_kind_1() {
    let sizes = Sizes::STARTING;
    let value = MessageKind::Question;
    let golden = include_bytes!("golden/message_kind_1.bin");
    assert_eq!(encode_message_kind(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_message_kind(golden, &sizes).unwrap(), value);
    assert!(decode_message_kind(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_message_kind(&trailing, &sizes).is_none());
}

#[test]
fn message_kind_2() {
    let sizes = Sizes::STARTING;
    let value = MessageKind::Answer;
    let golden = include_bytes!("golden/message_kind_2.bin");
    assert_eq!(encode_message_kind(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_message_kind(golden, &sizes).unwrap(), value);
    assert!(decode_message_kind(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_message_kind(&trailing, &sizes).is_none());
}

#[test]
fn note_change_0() {
    let sizes = Sizes::STARTING;
    let value = NoteChange::Write { words: Box::from(*b"abc") };
    let golden = include_bytes!("golden/note_change_0.bin");
    assert_eq!(encode_note_change(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_note_change(golden, &sizes).unwrap(), value);
    assert!(decode_note_change(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_note_change(&trailing, &sizes).is_none());
}

#[test]
fn note_change_1() {
    let sizes = Sizes::STARTING;
    let value = NoteChange::Remove;
    let golden = include_bytes!("golden/note_change_1.bin");
    assert_eq!(encode_note_change(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_note_change(golden, &sizes).unwrap(), value);
    assert!(decode_note_change(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_note_change(&trailing, &sizes).is_none());
}

#[test]
fn recall_0() {
    let sizes = Sizes::STARTING;
    let value = Recall::Named { name: Box::from(*b"abc") };
    let golden = include_bytes!("golden/recall_0.bin");
    assert_eq!(encode_recall(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_recall(golden, &sizes).unwrap(), value);
    assert!(decode_recall(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_recall(&trailing, &sizes).is_none());
}

#[test]
fn recall_1() {
    let sizes = Sizes::STARTING;
    let value = Recall::Search { words: Box::from(*b"abc"), most: 7 };
    let golden = include_bytes!("golden/recall_1.bin");
    assert_eq!(encode_recall(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_recall(golden, &sizes).unwrap(), value);
    assert!(decode_recall(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_recall(&trailing, &sizes).is_none());
}

#[test]
fn forge_read_0() {
    let sizes = Sizes::STARTING;
    let value = ForgeRead::Pull {
        repository: Name { segments: Box::from([Box::from(*b"abc")]) },
        number: 7,
        head: Some([7; 32]),
    };
    let golden = include_bytes!("golden/forge_read_0.bin");
    assert_eq!(encode_forge_read(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_read(golden, &sizes).unwrap(), value);
    assert!(decode_forge_read(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_read(&trailing, &sizes).is_none());
}

#[test]
fn forge_read_1() {
    let sizes = Sizes::STARTING;
    let value = ForgeRead::Files { repository: Name { segments: Box::from([Box::from(*b"abc")]) }, head: [7; 32] };
    let golden = include_bytes!("golden/forge_read_1.bin");
    assert_eq!(encode_forge_read(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_read(golden, &sizes).unwrap(), value);
    assert!(decode_forge_read(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_read(&trailing, &sizes).is_none());
}

#[test]
fn forge_read_2() {
    let sizes = Sizes::STARTING;
    let value = ForgeRead::Diff {
        repository: Name { segments: Box::from([Box::from(*b"abc")]) },
        base: [7; 32],
        head: [7; 32],
    };
    let golden = include_bytes!("golden/forge_read_2.bin");
    assert_eq!(encode_forge_read(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_read(golden, &sizes).unwrap(), value);
    assert!(decode_forge_read(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_read(&trailing, &sizes).is_none());
}

#[test]
fn forge_read_3() {
    let sizes = Sizes::STARTING;
    let value = ForgeRead::Ci { repository: Name { segments: Box::from([Box::from(*b"abc")]) }, head: [7; 32] };
    let golden = include_bytes!("golden/forge_read_3.bin");
    assert_eq!(encode_forge_read(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_read(golden, &sizes).unwrap(), value);
    assert!(decode_forge_read(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_read(&trailing, &sizes).is_none());
}

#[test]
fn forge_read_4() {
    let sizes = Sizes::STARTING;
    let value = ForgeRead::Issue { repository: Name { segments: Box::from([Box::from(*b"abc")]) }, number: 7 };
    let golden = include_bytes!("golden/forge_read_4.bin");
    assert_eq!(encode_forge_read(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_read(golden, &sizes).unwrap(), value);
    assert!(decode_forge_read(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_read(&trailing, &sizes).is_none());
}

#[test]
fn forge_read_5() {
    let sizes = Sizes::STARTING;
    let value = ForgeRead::Comments { resource: Name { segments: Box::from([Box::from(*b"abc")]) }, after: Some(7) };
    let golden = include_bytes!("golden/forge_read_5.bin");
    assert_eq!(encode_forge_read(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_read(golden, &sizes).unwrap(), value);
    assert!(decode_forge_read(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_read(&trailing, &sizes).is_none());
}

#[test]
fn read_0() {
    let sizes = Sizes::STARTING;
    let value = Read::Forge {
        read: ForgeRead::Pull {
            repository: Name { segments: Box::from([Box::from(*b"abc")]) },
            number: 7,
            head: Some([7; 32]),
        },
    };
    let golden = include_bytes!("golden/read_0.bin");
    assert_eq!(encode_read(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_read(golden, &sizes).unwrap(), value);
    assert!(decode_read(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_read(&trailing, &sizes).is_none());
}

#[test]
fn forge_effect_0() {
    let sizes = Sizes::STARTING;
    let value = ForgeEffect::OpenPull {
        repository: Name { segments: Box::from([Box::from(*b"abc")]) },
        branch: Box::from(*b"abc"),
        base: Box::from(*b"abc"),
        head: [7; 32],
        title: Box::from(*b"abc"),
        body: Box::from(*b"abc"),
    };
    let golden = include_bytes!("golden/forge_effect_0.bin");
    assert_eq!(encode_forge_effect(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_effect(golden, &sizes).unwrap(), value);
    assert!(decode_forge_effect(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_effect(&trailing, &sizes).is_none());
}

#[test]
fn forge_effect_1() {
    let sizes = Sizes::STARTING;
    let value = ForgeEffect::Merge {
        repository: Name { segments: Box::from([Box::from(*b"abc")]) },
        number: 7,
        expected: [7; 32],
    };
    let golden = include_bytes!("golden/forge_effect_1.bin");
    assert_eq!(encode_forge_effect(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_effect(golden, &sizes).unwrap(), value);
    assert!(decode_forge_effect(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_effect(&trailing, &sizes).is_none());
}

#[test]
fn forge_effect_2() {
    let sizes = Sizes::STARTING;
    let value = ForgeEffect::Comment {
        resource: Name { segments: Box::from([Box::from(*b"abc")]) },
        words: Box::from(*b"abc"),
    };
    let golden = include_bytes!("golden/forge_effect_2.bin");
    assert_eq!(encode_forge_effect(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_effect(golden, &sizes).unwrap(), value);
    assert!(decode_forge_effect(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_effect(&trailing, &sizes).is_none());
}

#[test]
fn forge_effect_3() {
    let sizes = Sizes::STARTING;
    let value = ForgeEffect::CreateIssue {
        repository: Name { segments: Box::from([Box::from(*b"abc")]) },
        title: Box::from(*b"abc"),
        body: Box::from(*b"abc"),
    };
    let golden = include_bytes!("golden/forge_effect_3.bin");
    assert_eq!(encode_forge_effect(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_effect(golden, &sizes).unwrap(), value);
    assert!(decode_forge_effect(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_effect(&trailing, &sizes).is_none());
}

#[test]
fn forge_effect_4() {
    let sizes = Sizes::STARTING;
    let value = ForgeEffect::EditIssue {
        repository: Name { segments: Box::from([Box::from(*b"abc")]) },
        number: 7,
        title: Some(Box::from(*b"abc")),
        body: Some(Box::from(*b"abc")),
    };
    let golden = include_bytes!("golden/forge_effect_4.bin");
    assert_eq!(encode_forge_effect(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_effect(golden, &sizes).unwrap(), value);
    assert!(decode_forge_effect(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_effect(&trailing, &sizes).is_none());
}

#[test]
fn forge_effect_5() {
    let sizes = Sizes::STARTING;
    let value = ForgeEffect::CloseIssue { repository: Name { segments: Box::from([Box::from(*b"abc")]) }, number: 7 };
    let golden = include_bytes!("golden/forge_effect_5.bin");
    assert_eq!(encode_forge_effect(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_effect(golden, &sizes).unwrap(), value);
    assert!(decode_forge_effect(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_effect(&trailing, &sizes).is_none());
}

#[test]
fn forge_effect_6() {
    let sizes = Sizes::STARTING;
    let value = ForgeEffect::ClosePull { repository: Name { segments: Box::from([Box::from(*b"abc")]) }, number: 7 };
    let golden = include_bytes!("golden/forge_effect_6.bin");
    assert_eq!(encode_forge_effect(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_effect(golden, &sizes).unwrap(), value);
    assert!(decode_forge_effect(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_effect(&trailing, &sizes).is_none());
}

#[test]
fn forge_effect_7() {
    let sizes = Sizes::STARTING;
    let value = ForgeEffect::PushBranch {
        repository: Name { segments: Box::from([Box::from(*b"abc")]) },
        branch: Box::from(*b"abc"),
        head: [7; 32],
        expected: Some([7; 32]),
    };
    let golden = include_bytes!("golden/forge_effect_7.bin");
    assert_eq!(encode_forge_effect(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_effect(golden, &sizes).unwrap(), value);
    assert!(decode_forge_effect(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_effect(&trailing, &sizes).is_none());
}

#[test]
fn effect_0() {
    let sizes = Sizes::STARTING;
    let value = Effect::Forge {
        effect: ForgeEffect::OpenPull {
            repository: Name { segments: Box::from([Box::from(*b"abc")]) },
            branch: Box::from(*b"abc"),
            base: Box::from(*b"abc"),
            head: [7; 32],
            title: Box::from(*b"abc"),
            body: Box::from(*b"abc"),
        },
    };
    let golden = include_bytes!("golden/effect_0.bin");
    assert_eq!(encode_effect(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_effect(golden, &sizes).unwrap(), value);
    assert!(decode_effect(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_effect(&trailing, &sizes).is_none());
}

#[test]
fn action_0() {
    let sizes = Sizes::STARTING;
    let value = Action::Delegate {
        batch: Box::from([NewTask {
            spec: Spec {
                words: Box::from(*b"abc"),
                resources: Resources {
                    read: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
                    write: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
                },
                inputs: Box::from([7]),
            },
            contract: TaskContract::Run { contract: Contract::Report { most: 7 } },
            executor: Executor::Agent { charter: 7 },
            authority: Authority {
                tools: 7,
                grants: Box::from([Grant {
                    connector: 7,
                    kind: 7,
                    pattern: Pattern { segments: Box::from([Box::from(*b"abc")]), last: Last::None },
                }]),
                delegation: Delegation { kinds: Box::from([ExecutorKind::Charter { kind: 7 }]), tasks: 7, depth: 7 },
                spend: 7,
                deadline: Some(7),
                notes: 7,
            },
            dependencies: Box::from([Dependency::Task { number: 7 }]),
            references: Box::from([7]),
            wake: Wake {
                own: WakeClass::All,
                related: WakeClass::All,
                subscribed: WakeClass::All,
                messages: WakeClass::All,
                count: 7,
                age: Some(Duration::from_nanos(7)),
            },
            subscriptions: Box::from([Topic::Task { task: 7 }]),
            tracked: true,
            priority: 7,
        }]),
    };
    let golden = include_bytes!("golden/action_0.bin");
    assert_eq!(encode_action(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_action(golden, &sizes).unwrap(), value);
    assert!(decode_action(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_action(&trailing, &sizes).is_none());
}

#[test]
fn action_1() {
    let sizes = Sizes::STARTING;
    let value = Action::Message { to: 7, words: Box::from(*b"abc"), kind: MessageKind::Words };
    let golden = include_bytes!("golden/action_1.bin");
    assert_eq!(encode_action(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_action(golden, &sizes).unwrap(), value);
    assert!(decode_action(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_action(&trailing, &sizes).is_none());
}

#[test]
fn action_2() {
    let sizes = Sizes::STARTING;
    let value = Action::Amend {
        task: 7,
        amendment: Amendment {
            spec: Some(Spec {
                words: Box::from(*b"abc"),
                resources: Resources {
                    read: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
                    write: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
                },
                inputs: Box::from([7]),
            }),
            wake: Some(Wake {
                own: WakeClass::All,
                related: WakeClass::All,
                subscribed: WakeClass::All,
                messages: WakeClass::All,
                count: 7,
                age: Some(Duration::from_nanos(7)),
            }),
            remove_dependencies: Box::from([7]),
            authority: Some(Authority {
                tools: 7,
                grants: Box::from([Grant {
                    connector: 7,
                    kind: 7,
                    pattern: Pattern { segments: Box::from([Box::from(*b"abc")]), last: Last::None },
                }]),
                delegation: Delegation { kinds: Box::from([ExecutorKind::Charter { kind: 7 }]), tasks: 7, depth: 7 },
                spend: 7,
                deadline: Some(7),
                notes: 7,
            }),
            instructions: Some(Box::from(*b"abc")),
            procedure: Some(ProcedureParameters::Land {
                repository: Name { segments: Box::from([Box::from(*b"abc")]) },
                branch: Box::from(*b"abc"),
                base: Box::from(*b"abc"),
                head: [7; 32],
            }),
        },
    };
    let golden = include_bytes!("golden/action_2.bin");
    assert_eq!(encode_action(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_action(golden, &sizes).unwrap(), value);
    assert!(decode_action(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_action(&trailing, &sizes).is_none());
}

#[test]
fn action_3() {
    let sizes = Sizes::STARTING;
    let value = Action::Cancel { task: 7, reason: Box::from(*b"abc") };
    let golden = include_bytes!("golden/action_3.bin");
    assert_eq!(encode_action(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_action(golden, &sizes).unwrap(), value);
    assert!(decode_action(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_action(&trailing, &sizes).is_none());
}

#[test]
fn action_4() {
    let sizes = Sizes::STARTING;
    let value = Action::Release { task: 7 };
    let golden = include_bytes!("golden/action_4.bin");
    assert_eq!(encode_action(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_action(golden, &sizes).unwrap(), value);
    assert!(decode_action(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_action(&trailing, &sizes).is_none());
}

#[test]
fn action_5() {
    let sizes = Sizes::STARTING;
    let value = Action::Decide { waiting: 7, decision: Decision::Accept };
    let golden = include_bytes!("golden/action_5.bin");
    assert_eq!(encode_action(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_action(golden, &sizes).unwrap(), value);
    assert!(decode_action(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_action(&trailing, &sizes).is_none());
}

#[test]
fn action_6() {
    let sizes = Sizes::STARTING;
    let value = Action::Subscribe { topic: Topic::Task { task: 7 } };
    let golden = include_bytes!("golden/action_6.bin");
    assert_eq!(encode_action(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_action(golden, &sizes).unwrap(), value);
    assert!(decode_action(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_action(&trailing, &sizes).is_none());
}

#[test]
fn action_7() {
    let sizes = Sizes::STARTING;
    let value = Action::Unsubscribe { topic: Topic::Task { task: 7 } };
    let golden = include_bytes!("golden/action_7.bin");
    assert_eq!(encode_action(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_action(golden, &sizes).unwrap(), value);
    assert!(decode_action(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_action(&trailing, &sizes).is_none());
}

#[test]
fn action_8() {
    let sizes = Sizes::STARTING;
    let value = Action::Effect {
        connector: 7,
        effect: Effect::Forge {
            effect: ForgeEffect::OpenPull {
                repository: Name { segments: Box::from([Box::from(*b"abc")]) },
                branch: Box::from(*b"abc"),
                base: Box::from(*b"abc"),
                head: [7; 32],
                title: Box::from(*b"abc"),
                body: Box::from(*b"abc"),
            },
        },
    };
    let golden = include_bytes!("golden/action_8.bin");
    assert_eq!(encode_action(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_action(golden, &sizes).unwrap(), value);
    assert!(decode_action(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_action(&trailing, &sizes).is_none());
}

#[test]
fn action_9() {
    let sizes = Sizes::STARTING;
    let value = Action::Note {
        scope: 7,
        name: Box::from(*b"abc"),
        revision: Some(7),
        change: NoteChange::Write { words: Box::from(*b"abc") },
    };
    let golden = include_bytes!("golden/action_9.bin");
    assert_eq!(encode_action(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_action(golden, &sizes).unwrap(), value);
    assert!(decode_action(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_action(&trailing, &sizes).is_none());
}

#[test]
fn action_10() {
    let sizes = Sizes::STARTING;
    let value = Action::Widen {
        task: 7,
        authority: Authority {
            tools: 7,
            grants: Box::from([Grant {
                connector: 7,
                kind: 7,
                pattern: Pattern { segments: Box::from([Box::from(*b"abc")]), last: Last::None },
            }]),
            delegation: Delegation { kinds: Box::from([ExecutorKind::Charter { kind: 7 }]), tasks: 7, depth: 7 },
            spend: 7,
            deadline: Some(7),
            notes: 7,
        },
    };
    let golden = include_bytes!("golden/action_10.bin");
    assert_eq!(encode_action(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_action(golden, &sizes).unwrap(), value);
    assert!(decode_action(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_action(&trailing, &sizes).is_none());
}

#[test]
fn call_0() {
    let sizes = Sizes::STARTING;
    let value = Call::Delegate {
        batch: Box::from([NewTask {
            spec: Spec {
                words: Box::from(*b"abc"),
                resources: Resources {
                    read: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
                    write: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
                },
                inputs: Box::from([7]),
            },
            contract: TaskContract::Run { contract: Contract::Report { most: 7 } },
            executor: Executor::Agent { charter: 7 },
            authority: Authority {
                tools: 7,
                grants: Box::from([Grant {
                    connector: 7,
                    kind: 7,
                    pattern: Pattern { segments: Box::from([Box::from(*b"abc")]), last: Last::None },
                }]),
                delegation: Delegation { kinds: Box::from([ExecutorKind::Charter { kind: 7 }]), tasks: 7, depth: 7 },
                spend: 7,
                deadline: Some(7),
                notes: 7,
            },
            dependencies: Box::from([Dependency::Task { number: 7 }]),
            references: Box::from([7]),
            wake: Wake {
                own: WakeClass::All,
                related: WakeClass::All,
                subscribed: WakeClass::All,
                messages: WakeClass::All,
                count: 7,
                age: Some(Duration::from_nanos(7)),
            },
            subscriptions: Box::from([Topic::Task { task: 7 }]),
            tracked: true,
            priority: 7,
        }]),
    };
    let golden = include_bytes!("golden/call_0.bin");
    assert_eq!(encode_call(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_call(golden, &sizes).unwrap(), value);
    assert!(decode_call(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_call(&trailing, &sizes).is_none());
}

#[test]
fn call_1() {
    let sizes = Sizes::STARTING;
    let value = Call::Message { to: 7, words: Box::from(*b"abc"), kind: MessageKind::Words };
    let golden = include_bytes!("golden/call_1.bin");
    assert_eq!(encode_call(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_call(golden, &sizes).unwrap(), value);
    assert!(decode_call(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_call(&trailing, &sizes).is_none());
}

#[test]
fn call_2() {
    let sizes = Sizes::STARTING;
    let value = Call::Amend {
        task: 7,
        amendment: Amendment {
            spec: Some(Spec {
                words: Box::from(*b"abc"),
                resources: Resources {
                    read: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
                    write: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
                },
                inputs: Box::from([7]),
            }),
            wake: Some(Wake {
                own: WakeClass::All,
                related: WakeClass::All,
                subscribed: WakeClass::All,
                messages: WakeClass::All,
                count: 7,
                age: Some(Duration::from_nanos(7)),
            }),
            remove_dependencies: Box::from([7]),
            authority: Some(Authority {
                tools: 7,
                grants: Box::from([Grant {
                    connector: 7,
                    kind: 7,
                    pattern: Pattern { segments: Box::from([Box::from(*b"abc")]), last: Last::None },
                }]),
                delegation: Delegation { kinds: Box::from([ExecutorKind::Charter { kind: 7 }]), tasks: 7, depth: 7 },
                spend: 7,
                deadline: Some(7),
                notes: 7,
            }),
            instructions: Some(Box::from(*b"abc")),
            procedure: Some(ProcedureParameters::Land {
                repository: Name { segments: Box::from([Box::from(*b"abc")]) },
                branch: Box::from(*b"abc"),
                base: Box::from(*b"abc"),
                head: [7; 32],
            }),
        },
    };
    let golden = include_bytes!("golden/call_2.bin");
    assert_eq!(encode_call(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_call(golden, &sizes).unwrap(), value);
    assert!(decode_call(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_call(&trailing, &sizes).is_none());
}

#[test]
fn call_3() {
    let sizes = Sizes::STARTING;
    let value = Call::Cancel { task: 7, reason: Box::from(*b"abc") };
    let golden = include_bytes!("golden/call_3.bin");
    assert_eq!(encode_call(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_call(golden, &sizes).unwrap(), value);
    assert!(decode_call(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_call(&trailing, &sizes).is_none());
}

#[test]
fn call_4() {
    let sizes = Sizes::STARTING;
    let value = Call::Release { task: 7 };
    let golden = include_bytes!("golden/call_4.bin");
    assert_eq!(encode_call(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_call(golden, &sizes).unwrap(), value);
    assert!(decode_call(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_call(&trailing, &sizes).is_none());
}

#[test]
fn call_5() {
    let sizes = Sizes::STARTING;
    let value = Call::Decide { waiting: 7, decision: Decision::Accept };
    let golden = include_bytes!("golden/call_5.bin");
    assert_eq!(encode_call(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_call(golden, &sizes).unwrap(), value);
    assert!(decode_call(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_call(&trailing, &sizes).is_none());
}

#[test]
fn call_6() {
    let sizes = Sizes::STARTING;
    let value = Call::Propose {
        action: Action::Delegate {
            batch: Box::from([NewTask {
                spec: Spec {
                    words: Box::from(*b"abc"),
                    resources: Resources {
                        read: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
                        write: Box::from([Resource::Named {
                            name: Name { segments: Box::from([Box::from(*b"abc")]) },
                        }]),
                    },
                    inputs: Box::from([7]),
                },
                contract: TaskContract::Run { contract: Contract::Report { most: 7 } },
                executor: Executor::Agent { charter: 7 },
                authority: Authority {
                    tools: 7,
                    grants: Box::from([Grant {
                        connector: 7,
                        kind: 7,
                        pattern: Pattern { segments: Box::from([Box::from(*b"abc")]), last: Last::None },
                    }]),
                    delegation: Delegation {
                        kinds: Box::from([ExecutorKind::Charter { kind: 7 }]),
                        tasks: 7,
                        depth: 7,
                    },
                    spend: 7,
                    deadline: Some(7),
                    notes: 7,
                },
                dependencies: Box::from([Dependency::Task { number: 7 }]),
                references: Box::from([7]),
                wake: Wake {
                    own: WakeClass::All,
                    related: WakeClass::All,
                    subscribed: WakeClass::All,
                    messages: WakeClass::All,
                    count: 7,
                    age: Some(Duration::from_nanos(7)),
                },
                subscriptions: Box::from([Topic::Task { task: 7 }]),
                tracked: true,
                priority: 7,
            }]),
        },
        reason: Box::from(*b"abc"),
        accepter_requests: true,
    };
    let golden = include_bytes!("golden/call_6.bin");
    assert_eq!(encode_call(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_call(golden, &sizes).unwrap(), value);
    assert!(decode_call(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_call(&trailing, &sizes).is_none());
}

#[test]
fn call_7() {
    let sizes = Sizes::STARTING;
    let value = Call::Subscribe { topic: Topic::Task { task: 7 } };
    let golden = include_bytes!("golden/call_7.bin");
    assert_eq!(encode_call(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_call(golden, &sizes).unwrap(), value);
    assert!(decode_call(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_call(&trailing, &sizes).is_none());
}

#[test]
fn call_8() {
    let sizes = Sizes::STARTING;
    let value = Call::Unsubscribe { topic: Topic::Task { task: 7 } };
    let golden = include_bytes!("golden/call_8.bin");
    assert_eq!(encode_call(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_call(golden, &sizes).unwrap(), value);
    assert!(decode_call(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_call(&trailing, &sizes).is_none());
}

#[test]
fn call_9() {
    let sizes = Sizes::STARTING;
    let value = Call::Effect {
        connector: 7,
        effect: Effect::Forge {
            effect: ForgeEffect::OpenPull {
                repository: Name { segments: Box::from([Box::from(*b"abc")]) },
                branch: Box::from(*b"abc"),
                base: Box::from(*b"abc"),
                head: [7; 32],
                title: Box::from(*b"abc"),
                body: Box::from(*b"abc"),
            },
        },
    };
    let golden = include_bytes!("golden/call_9.bin");
    assert_eq!(encode_call(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_call(golden, &sizes).unwrap(), value);
    assert!(decode_call(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_call(&trailing, &sizes).is_none());
}

#[test]
fn call_10() {
    let sizes = Sizes::STARTING;
    let value = Call::Note {
        scope: 7,
        name: Box::from(*b"abc"),
        revision: Some(7),
        change: NoteChange::Write { words: Box::from(*b"abc") },
    };
    let golden = include_bytes!("golden/call_10.bin");
    assert_eq!(encode_call(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_call(golden, &sizes).unwrap(), value);
    assert!(decode_call(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_call(&trailing, &sizes).is_none());
}

#[test]
fn call_11() {
    let sizes = Sizes::STARTING;
    let value = Call::Recall { scope: 7, query: Recall::Named { name: Box::from(*b"abc") } };
    let golden = include_bytes!("golden/call_11.bin");
    assert_eq!(encode_call(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_call(golden, &sizes).unwrap(), value);
    assert!(decode_call(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_call(&trailing, &sizes).is_none());
}

#[test]
fn call_12() {
    let sizes = Sizes::STARTING;
    let value = Call::Read {
        connector: 7,
        read: Read::Forge {
            read: ForgeRead::Pull {
                repository: Name { segments: Box::from([Box::from(*b"abc")]) },
                number: 7,
                head: Some([7; 32]),
            },
        },
    };
    let golden = include_bytes!("golden/call_12.bin");
    assert_eq!(encode_call(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_call(golden, &sizes).unwrap(), value);
    assert!(decode_call(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_call(&trailing, &sizes).is_none());
}

#[test]
fn outcome_0() {
    let sizes = Sizes::STARTING;
    let value = Outcome::Change { title: Box::from(*b"abc"), body: Box::from(*b"abc") };
    let golden = include_bytes!("golden/outcome_0.bin");
    assert_eq!(encode_outcome(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_outcome(golden, &sizes).unwrap(), value);
    assert!(decode_outcome(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_outcome(&trailing, &sizes).is_none());
}

#[test]
fn outcome_1() {
    let sizes = Sizes::STARTING;
    let value = Outcome::Verdict {
        verdict: 7,
        fields: Box::from([Field { name: Box::from(*b"abc"), value: Box::from(*b"abc") }]),
        follow_ups: Box::from([NewTask {
            spec: Spec {
                words: Box::from(*b"abc"),
                resources: Resources {
                    read: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
                    write: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
                },
                inputs: Box::from([7]),
            },
            contract: TaskContract::Run { contract: Contract::Report { most: 7 } },
            executor: Executor::Agent { charter: 7 },
            authority: Authority {
                tools: 7,
                grants: Box::from([Grant {
                    connector: 7,
                    kind: 7,
                    pattern: Pattern { segments: Box::from([Box::from(*b"abc")]), last: Last::None },
                }]),
                delegation: Delegation { kinds: Box::from([ExecutorKind::Charter { kind: 7 }]), tasks: 7, depth: 7 },
                spend: 7,
                deadline: Some(7),
                notes: 7,
            },
            dependencies: Box::from([Dependency::Task { number: 7 }]),
            references: Box::from([7]),
            wake: Wake {
                own: WakeClass::All,
                related: WakeClass::All,
                subscribed: WakeClass::All,
                messages: WakeClass::All,
                count: 7,
                age: Some(Duration::from_nanos(7)),
            },
            subscriptions: Box::from([Topic::Task { task: 7 }]),
            tracked: true,
            priority: 7,
        }]),
    };
    let golden = include_bytes!("golden/outcome_1.bin");
    assert_eq!(encode_outcome(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_outcome(golden, &sizes).unwrap(), value);
    assert!(decode_outcome(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_outcome(&trailing, &sizes).is_none());
}

#[test]
fn outcome_2() {
    let sizes = Sizes::STARTING;
    let value = Outcome::Report { text: Box::from(*b"abc") };
    let golden = include_bytes!("golden/outcome_2.bin");
    assert_eq!(encode_outcome(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_outcome(golden, &sizes).unwrap(), value);
    assert!(decode_outcome(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_outcome(&trailing, &sizes).is_none());
}

#[test]
fn outcome_3() {
    let sizes = Sizes::STARTING;
    let value = Outcome::Failure { reason: Box::from(*b"abc") };
    let golden = include_bytes!("golden/outcome_3.bin");
    assert_eq!(encode_outcome(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_outcome(golden, &sizes).unwrap(), value);
    assert!(decode_outcome(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_outcome(&trailing, &sizes).is_none());
}

#[test]
fn ending_0() {
    let sizes = Sizes::STARTING;
    let value = Ending::Done;
    let golden = include_bytes!("golden/ending_0.bin");
    assert_eq!(encode_ending(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_ending(golden, &sizes).unwrap(), value);
    assert!(decode_ending(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_ending(&trailing, &sizes).is_none());
}

#[test]
fn ending_1() {
    let sizes = Sizes::STARTING;
    let value = Ending::Failed;
    let golden = include_bytes!("golden/ending_1.bin");
    assert_eq!(encode_ending(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_ending(golden, &sizes).unwrap(), value);
    assert!(decode_ending(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_ending(&trailing, &sizes).is_none());
}

#[test]
fn ending_2() {
    let sizes = Sizes::STARTING;
    let value = Ending::Cancelled;
    let golden = include_bytes!("golden/ending_2.bin");
    assert_eq!(encode_ending(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_ending(golden, &sizes).unwrap(), value);
    assert!(decode_ending(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_ending(&trailing, &sizes).is_none());
}

#[test]
fn party_0() {
    let sizes = Sizes::STARTING;
    let value = Party::Task { task: 7 };
    let golden = include_bytes!("golden/party_0.bin");
    assert_eq!(encode_party(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_party(golden, &sizes).unwrap(), value);
    assert!(decode_party(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_party(&trailing, &sizes).is_none());
}

#[test]
fn party_1() {
    let sizes = Sizes::STARTING;
    let value = Party::Person { person: 7 };
    let golden = include_bytes!("golden/party_1.bin");
    assert_eq!(encode_party(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_party(golden, &sizes).unwrap(), value);
    assert!(decode_party(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_party(&trailing, &sizes).is_none());
}

#[test]
fn party_2() {
    let sizes = Sizes::STARTING;
    let value = Party::Deployment;
    let golden = include_bytes!("golden/party_2.bin");
    assert_eq!(encode_party(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_party(golden, &sizes).unwrap(), value);
    assert!(decode_party(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_party(&trailing, &sizes).is_none());
}

#[test]
fn class_0() {
    let sizes = Sizes::STARTING;
    let value = Class::Critical;
    let golden = include_bytes!("golden/class_0.bin");
    assert_eq!(encode_class(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_class(golden, &sizes).unwrap(), value);
    assert!(decode_class(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_class(&trailing, &sizes).is_none());
}

#[test]
fn class_1() {
    let sizes = Sizes::STARTING;
    let value = Class::Ordinary;
    let golden = include_bytes!("golden/class_1.bin");
    assert_eq!(encode_class(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_class(golden, &sizes).unwrap(), value);
    assert!(decode_class(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_class(&trailing, &sizes).is_none());
}

#[test]
fn forge_news_0() {
    let sizes = Sizes::STARTING;
    let value =
        ForgeNews::Pull { repository: Name { segments: Box::from([Box::from(*b"abc")]) }, number: 7, head: [7; 32] };
    let golden = include_bytes!("golden/forge_news_0.bin");
    assert_eq!(encode_forge_news(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_news(golden, &sizes).unwrap(), value);
    assert!(decode_forge_news(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_news(&trailing, &sizes).is_none());
}

#[test]
fn forge_news_1() {
    let sizes = Sizes::STARTING;
    let value =
        ForgeNews::Ci { repository: Name { segments: Box::from([Box::from(*b"abc")]) }, head: [7; 32], passed: true };
    let golden = include_bytes!("golden/forge_news_1.bin");
    assert_eq!(encode_forge_news(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_news(golden, &sizes).unwrap(), value);
    assert!(decode_forge_news(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_news(&trailing, &sizes).is_none());
}

#[test]
fn forge_news_2() {
    let sizes = Sizes::STARTING;
    let value = ForgeNews::Issue { repository: Name { segments: Box::from([Box::from(*b"abc")]) }, number: 7 };
    let golden = include_bytes!("golden/forge_news_2.bin");
    assert_eq!(encode_forge_news(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_news(golden, &sizes).unwrap(), value);
    assert!(decode_forge_news(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_news(&trailing, &sizes).is_none());
}

#[test]
fn forge_news_3() {
    let sizes = Sizes::STARTING;
    let value = ForgeNews::Comment { resource: Name { segments: Box::from([Box::from(*b"abc")]) }, number: 7 };
    let golden = include_bytes!("golden/forge_news_3.bin");
    assert_eq!(encode_forge_news(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_news(golden, &sizes).unwrap(), value);
    assert!(decode_forge_news(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_news(&trailing, &sizes).is_none());
}

#[test]
fn news_0() {
    let sizes = Sizes::STARTING;
    let value = News::Forge {
        news: ForgeNews::Pull {
            repository: Name { segments: Box::from([Box::from(*b"abc")]) },
            number: 7,
            head: [7; 32],
        },
    };
    let golden = include_bytes!("golden/news_0.bin");
    assert_eq!(encode_news(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_news(golden, &sizes).unwrap(), value);
    assert!(decode_news(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_news(&trailing, &sizes).is_none());
}

#[test]
fn notice_0() {
    let sizes = Sizes::STARTING;
    let value = Notice::Held { reason: Box::from(*b"abc") };
    let golden = include_bytes!("golden/notice_0.bin");
    assert_eq!(encode_notice(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_notice(golden, &sizes).unwrap(), value);
    assert!(decode_notice(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_notice(&trailing, &sizes).is_none());
}

#[test]
fn notice_1() {
    let sizes = Sizes::STARTING;
    let value = Notice::Released;
    let golden = include_bytes!("golden/notice_1.bin");
    assert_eq!(encode_notice(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_notice(golden, &sizes).unwrap(), value);
    assert!(decode_notice(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_notice(&trailing, &sizes).is_none());
}

#[test]
fn notice_2() {
    let sizes = Sizes::STARTING;
    let value = Notice::Cancelled { reason: Box::from(*b"abc") };
    let golden = include_bytes!("golden/notice_2.bin");
    assert_eq!(encode_notice(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_notice(golden, &sizes).unwrap(), value);
    assert!(decode_notice(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_notice(&trailing, &sizes).is_none());
}

#[test]
fn notice_3() {
    let sizes = Sizes::STARTING;
    let value = Notice::EffectFailed { effect: 7, reason: Box::from(*b"abc") };
    let golden = include_bytes!("golden/notice_3.bin");
    assert_eq!(encode_notice(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_notice(golden, &sizes).unwrap(), value);
    assert!(decode_notice(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_notice(&trailing, &sizes).is_none());
}

#[test]
fn notice_4() {
    let sizes = Sizes::STARTING;
    let value = Notice::MessageLost { message: 7 };
    let golden = include_bytes!("golden/notice_4.bin");
    assert_eq!(encode_notice(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_notice(golden, &sizes).unwrap(), value);
    assert!(decode_notice(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_notice(&trailing, &sizes).is_none());
}

#[test]
fn notice_5() {
    let sizes = Sizes::STARTING;
    let value = Notice::Budget { remaining: 7 };
    let golden = include_bytes!("golden/notice_5.bin");
    assert_eq!(encode_notice(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_notice(golden, &sizes).unwrap(), value);
    assert!(decode_notice(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_notice(&trailing, &sizes).is_none());
}

#[test]
fn message_0() {
    let sizes = Sizes::STARTING;
    let value = Message::Result {
        from: 7,
        ending: Ending::Done,
        result: Outcome::Change { title: Box::from(*b"abc"), body: Box::from(*b"abc") },
    };
    let golden = include_bytes!("golden/message_0.bin");
    assert_eq!(encode_message(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_message(golden, &sizes).unwrap(), value);
    assert!(decode_message(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_message(&trailing, &sizes).is_none());
}

#[test]
fn message_1() {
    let sizes = Sizes::STARTING;
    let value = Message::Question { from: 7, words: Box::from(*b"abc") };
    let golden = include_bytes!("golden/message_1.bin");
    assert_eq!(encode_message(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_message(golden, &sizes).unwrap(), value);
    assert!(decode_message(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_message(&trailing, &sizes).is_none());
}

#[test]
fn message_2() {
    let sizes = Sizes::STARTING;
    let value = Message::Answer { from: 7, words: Box::from(*b"abc") };
    let golden = include_bytes!("golden/message_2.bin");
    assert_eq!(encode_message(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_message(golden, &sizes).unwrap(), value);
    assert!(decode_message(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_message(&trailing, &sizes).is_none());
}

#[test]
fn message_3() {
    let sizes = Sizes::STARTING;
    let value = Message::Decision { proposal: 7, decision: Decision::Accept };
    let golden = include_bytes!("golden/message_3.bin");
    assert_eq!(encode_message(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_message(golden, &sizes).unwrap(), value);
    assert!(decode_message(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_message(&trailing, &sizes).is_none());
}

#[test]
fn message_4() {
    let sizes = Sizes::STARTING;
    let value = Message::Amendment {
        amendment: Amendment {
            spec: Some(Spec {
                words: Box::from(*b"abc"),
                resources: Resources {
                    read: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
                    write: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
                },
                inputs: Box::from([7]),
            }),
            wake: Some(Wake {
                own: WakeClass::All,
                related: WakeClass::All,
                subscribed: WakeClass::All,
                messages: WakeClass::All,
                count: 7,
                age: Some(Duration::from_nanos(7)),
            }),
            remove_dependencies: Box::from([7]),
            authority: Some(Authority {
                tools: 7,
                grants: Box::from([Grant {
                    connector: 7,
                    kind: 7,
                    pattern: Pattern { segments: Box::from([Box::from(*b"abc")]), last: Last::None },
                }]),
                delegation: Delegation { kinds: Box::from([ExecutorKind::Charter { kind: 7 }]), tasks: 7, depth: 7 },
                spend: 7,
                deadline: Some(7),
                notes: 7,
            }),
            instructions: Some(Box::from(*b"abc")),
            procedure: Some(ProcedureParameters::Land {
                repository: Name { segments: Box::from([Box::from(*b"abc")]) },
                branch: Box::from(*b"abc"),
                base: Box::from(*b"abc"),
                head: [7; 32],
            }),
        },
    };
    let golden = include_bytes!("golden/message_4.bin");
    assert_eq!(encode_message(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_message(golden, &sizes).unwrap(), value);
    assert!(decode_message(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_message(&trailing, &sizes).is_none());
}

#[test]
fn message_5() {
    let sizes = Sizes::STARTING;
    let value = Message::Words { from: Party::Task { task: 7 }, words: Box::from(*b"abc") };
    let golden = include_bytes!("golden/message_5.bin");
    assert_eq!(encode_message(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_message(golden, &sizes).unwrap(), value);
    assert!(decode_message(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_message(&trailing, &sizes).is_none());
}

#[test]
fn message_6() {
    let sizes = Sizes::STARTING;
    let value = Message::News {
        topic: Topic::Task { task: 7 },
        class: Class::Critical,
        news: News::Forge {
            news: ForgeNews::Pull {
                repository: Name { segments: Box::from([Box::from(*b"abc")]) },
                number: 7,
                head: [7; 32],
            },
        },
    };
    let golden = include_bytes!("golden/message_6.bin");
    assert_eq!(encode_message(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_message(golden, &sizes).unwrap(), value);
    assert!(decode_message(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_message(&trailing, &sizes).is_none());
}

#[test]
fn message_7() {
    let sizes = Sizes::STARTING;
    let value = Message::Notice { notice: Notice::Held { reason: Box::from(*b"abc") } };
    let golden = include_bytes!("golden/message_7.bin");
    assert_eq!(encode_message(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_message(golden, &sizes).unwrap(), value);
    assert!(decode_message(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_message(&trailing, &sizes).is_none());
}

#[test]
fn message_8() {
    let sizes = Sizes::STARTING;
    let value = Message::Timer { at: 7 };
    let golden = include_bytes!("golden/message_8.bin");
    assert_eq!(encode_message(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_message(golden, &sizes).unwrap(), value);
    assert!(decode_message(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_message(&trailing, &sizes).is_none());
}

#[test]
fn message_9() {
    let sizes = Sizes::STARTING;
    let value = Message::Waiting {
        proposal: 7,
        action: Action::Delegate {
            batch: Box::from([NewTask {
                spec: Spec {
                    words: Box::from(*b"abc"),
                    resources: Resources {
                        read: Box::from([Resource::Named { name: Name { segments: Box::from([Box::from(*b"abc")]) } }]),
                        write: Box::from([Resource::Named {
                            name: Name { segments: Box::from([Box::from(*b"abc")]) },
                        }]),
                    },
                    inputs: Box::from([7]),
                },
                contract: TaskContract::Run { contract: Contract::Report { most: 7 } },
                executor: Executor::Agent { charter: 7 },
                authority: Authority {
                    tools: 7,
                    grants: Box::from([Grant {
                        connector: 7,
                        kind: 7,
                        pattern: Pattern { segments: Box::from([Box::from(*b"abc")]), last: Last::None },
                    }]),
                    delegation: Delegation {
                        kinds: Box::from([ExecutorKind::Charter { kind: 7 }]),
                        tasks: 7,
                        depth: 7,
                    },
                    spend: 7,
                    deadline: Some(7),
                    notes: 7,
                },
                dependencies: Box::from([Dependency::Task { number: 7 }]),
                references: Box::from([7]),
                wake: Wake {
                    own: WakeClass::All,
                    related: WakeClass::All,
                    subscribed: WakeClass::All,
                    messages: WakeClass::All,
                    count: 7,
                    age: Some(Duration::from_nanos(7)),
                },
                subscriptions: Box::from([Topic::Task { task: 7 }]),
                tracked: true,
                priority: 7,
            }]),
        },
        reason: Box::from(*b"abc"),
    };
    let golden = include_bytes!("golden/message_9.bin");
    assert_eq!(encode_message(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_message(golden, &sizes).unwrap(), value);
    assert!(decode_message(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_message(&trailing, &sizes).is_none());
}

#[test]
fn forge_answer_0() {
    let sizes = Sizes::STARTING;
    let value =
        ForgeAnswer::Pull { number: 7, head: [7; 32], title: Box::from(*b"abc"), body: Box::from(*b"abc"), open: true };
    let golden = include_bytes!("golden/forge_answer_0.bin");
    assert_eq!(encode_forge_answer(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_answer(golden, &sizes).unwrap(), value);
    assert!(decode_forge_answer(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_answer(&trailing, &sizes).is_none());
}

#[test]
fn forge_answer_1() {
    let sizes = Sizes::STARTING;
    let value = ForgeAnswer::Files {
        files: Box::from([File { path: Box::from(*b"abc"), content: Box::from(*b"abc") }]),
        more: true,
    };
    let golden = include_bytes!("golden/forge_answer_1.bin");
    assert_eq!(encode_forge_answer(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_answer(golden, &sizes).unwrap(), value);
    assert!(decode_forge_answer(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_answer(&trailing, &sizes).is_none());
}

#[test]
fn forge_answer_2() {
    let sizes = Sizes::STARTING;
    let value = ForgeAnswer::Diff { patch: Box::from(*b"abc"), cut: 7 };
    let golden = include_bytes!("golden/forge_answer_2.bin");
    assert_eq!(encode_forge_answer(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_answer(golden, &sizes).unwrap(), value);
    assert!(decode_forge_answer(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_answer(&trailing, &sizes).is_none());
}

#[test]
fn forge_answer_3() {
    let sizes = Sizes::STARTING;
    let value = ForgeAnswer::Ci {
        head: [7; 32],
        statuses: Box::from([CiStatus { name: Box::from(*b"abc"), passed: true, pending: true }]),
        more: true,
    };
    let golden = include_bytes!("golden/forge_answer_3.bin");
    assert_eq!(encode_forge_answer(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_answer(golden, &sizes).unwrap(), value);
    assert!(decode_forge_answer(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_answer(&trailing, &sizes).is_none());
}

#[test]
fn forge_answer_4() {
    let sizes = Sizes::STARTING;
    let value = ForgeAnswer::Issue { number: 7, title: Box::from(*b"abc"), body: Box::from(*b"abc"), open: true };
    let golden = include_bytes!("golden/forge_answer_4.bin");
    assert_eq!(encode_forge_answer(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_answer(golden, &sizes).unwrap(), value);
    assert!(decode_forge_answer(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_answer(&trailing, &sizes).is_none());
}

#[test]
fn forge_answer_5() {
    let sizes = Sizes::STARTING;
    let value =
        ForgeAnswer::Comments { comments: Box::from([Comment { number: 7, words: Box::from(*b"abc") }]), more: true };
    let golden = include_bytes!("golden/forge_answer_5.bin");
    assert_eq!(encode_forge_answer(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_forge_answer(golden, &sizes).unwrap(), value);
    assert!(decode_forge_answer(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_forge_answer(&trailing, &sizes).is_none());
}

#[test]
fn read_answer_0() {
    let sizes = Sizes::STARTING;
    let value = ReadAnswer::Forge {
        answer: ForgeAnswer::Pull {
            number: 7,
            head: [7; 32],
            title: Box::from(*b"abc"),
            body: Box::from(*b"abc"),
            open: true,
        },
    };
    let golden = include_bytes!("golden/read_answer_0.bin");
    assert_eq!(encode_read_answer(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_read_answer(golden, &sizes).unwrap(), value);
    assert!(decode_read_answer(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_read_answer(&trailing, &sizes).is_none());
}

#[test]
fn effect_result_0() {
    let sizes = Sizes::STARTING;
    let value =
        EffectResult::Made { resource: Name { segments: Box::from([Box::from(*b"abc")]) }, revision: Some([7; 32]) };
    let golden = include_bytes!("golden/effect_result_0.bin");
    assert_eq!(encode_effect_result(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_effect_result(golden, &sizes).unwrap(), value);
    assert!(decode_effect_result(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_effect_result(&trailing, &sizes).is_none());
}

#[test]
fn effect_result_1() {
    let sizes = Sizes::STARTING;
    let value = EffectResult::Pending { effect: 7 };
    let golden = include_bytes!("golden/effect_result_1.bin");
    assert_eq!(encode_effect_result(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_effect_result(golden, &sizes).unwrap(), value);
    assert!(decode_effect_result(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_effect_result(&trailing, &sizes).is_none());
}

#[test]
fn effect_result_2() {
    let sizes = Sizes::STARTING;
    let value = EffectResult::Failed { reason: Box::from(*b"abc") };
    let golden = include_bytes!("golden/effect_result_2.bin");
    assert_eq!(encode_effect_result(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_effect_result(golden, &sizes).unwrap(), value);
    assert!(decode_effect_result(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_effect_result(&trailing, &sizes).is_none());
}

#[test]
fn lack_0() {
    let sizes = Sizes::STARTING;
    let value = Lack::Tool { family: 7 };
    let golden = include_bytes!("golden/lack_0.bin");
    assert_eq!(encode_lack(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_lack(golden, &sizes).unwrap(), value);
    assert!(decode_lack(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_lack(&trailing, &sizes).is_none());
}

#[test]
fn lack_1() {
    let sizes = Sizes::STARTING;
    let value = Lack::Grant { connector: 7, kind: 7, resource: Name { segments: Box::from([Box::from(*b"abc")]) } };
    let golden = include_bytes!("golden/lack_1.bin");
    assert_eq!(encode_lack(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_lack(golden, &sizes).unwrap(), value);
    assert!(decode_lack(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_lack(&trailing, &sizes).is_none());
}

#[test]
fn lack_2() {
    let sizes = Sizes::STARTING;
    let value = Lack::Delegation { kind: ExecutorKind::Charter { kind: 7 } };
    let golden = include_bytes!("golden/lack_2.bin");
    assert_eq!(encode_lack(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_lack(golden, &sizes).unwrap(), value);
    assert!(decode_lack(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_lack(&trailing, &sizes).is_none());
}

#[test]
fn lack_3() {
    let sizes = Sizes::STARTING;
    let value = Lack::Spend { missing: 7 };
    let golden = include_bytes!("golden/lack_3.bin");
    assert_eq!(encode_lack(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_lack(golden, &sizes).unwrap(), value);
    assert!(decode_lack(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_lack(&trailing, &sizes).is_none());
}

#[test]
fn lack_4() {
    let sizes = Sizes::STARTING;
    let value = Lack::Depth;
    let golden = include_bytes!("golden/lack_4.bin");
    assert_eq!(encode_lack(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_lack(golden, &sizes).unwrap(), value);
    assert!(decode_lack(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_lack(&trailing, &sizes).is_none());
}

#[test]
fn lack_5() {
    let sizes = Sizes::STARTING;
    let value = Lack::Tasks;
    let golden = include_bytes!("golden/lack_5.bin");
    assert_eq!(encode_lack(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_lack(golden, &sizes).unwrap(), value);
    assert!(decode_lack(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_lack(&trailing, &sizes).is_none());
}

#[test]
fn lack_6() {
    let sizes = Sizes::STARTING;
    let value = Lack::Notes { scope: 7 };
    let golden = include_bytes!("golden/lack_6.bin");
    assert_eq!(encode_lack(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_lack(golden, &sizes).unwrap(), value);
    assert!(decode_lack(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_lack(&trailing, &sizes).is_none());
}

#[test]
fn lack_7() {
    let sizes = Sizes::STARTING;
    let value = Lack::Deadline;
    let golden = include_bytes!("golden/lack_7.bin");
    assert_eq!(encode_lack(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_lack(golden, &sizes).unwrap(), value);
    assert!(decode_lack(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_lack(&trailing, &sizes).is_none());
}

#[test]
fn unserved_0() {
    let sizes = Sizes::STARTING;
    let value = Unserved::Lost;
    let golden = include_bytes!("golden/unserved_0.bin");
    assert_eq!(encode_unserved(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_unserved(golden, &sizes).unwrap(), value);
    assert!(decode_unserved(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_unserved(&trailing, &sizes).is_none());
}

#[test]
fn unserved_1() {
    let sizes = Sizes::STARTING;
    let value = Unserved::Withdrawn;
    let golden = include_bytes!("golden/unserved_1.bin");
    assert_eq!(encode_unserved(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_unserved(golden, &sizes).unwrap(), value);
    assert!(decode_unserved(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_unserved(&trailing, &sizes).is_none());
}

#[test]
fn unserved_2() {
    let sizes = Sizes::STARTING;
    let value = Unserved::Busy;
    let golden = include_bytes!("golden/unserved_2.bin");
    assert_eq!(encode_unserved(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_unserved(golden, &sizes).unwrap(), value);
    assert!(decode_unserved(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_unserved(&trailing, &sizes).is_none());
}

#[test]
fn unserved_3() {
    let sizes = Sizes::STARTING;
    let value = Unserved::Unavailable;
    let golden = include_bytes!("golden/unserved_3.bin");
    assert_eq!(encode_unserved(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_unserved(golden, &sizes).unwrap(), value);
    assert!(decode_unserved(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_unserved(&trailing, &sizes).is_none());
}

#[test]
fn unserved_4() {
    let sizes = Sizes::STARTING;
    let value = Unserved::TooLarge;
    let golden = include_bytes!("golden/unserved_4.bin");
    assert_eq!(encode_unserved(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_unserved(golden, &sizes).unwrap(), value);
    assert!(decode_unserved(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_unserved(&trailing, &sizes).is_none());
}

#[test]
fn served_0() {
    let sizes = Sizes::STARTING;
    let value = Served::Delegated { tasks: Box::from([7]) };
    let golden = include_bytes!("golden/served_0.bin");
    assert_eq!(encode_served(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_served(golden, &sizes).unwrap(), value);
    assert!(decode_served(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_served(&trailing, &sizes).is_none());
}

#[test]
fn served_1() {
    let sizes = Sizes::STARTING;
    let value = Served::Sent { message: 7 };
    let golden = include_bytes!("golden/served_1.bin");
    assert_eq!(encode_served(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_served(golden, &sizes).unwrap(), value);
    assert!(decode_served(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_served(&trailing, &sizes).is_none());
}

#[test]
fn served_2() {
    let sizes = Sizes::STARTING;
    let value = Served::Done;
    let golden = include_bytes!("golden/served_2.bin");
    assert_eq!(encode_served(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_served(golden, &sizes).unwrap(), value);
    assert!(decode_served(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_served(&trailing, &sizes).is_none());
}

#[test]
fn served_3() {
    let sizes = Sizes::STARTING;
    let value = Served::Proposed { proposal: 7, holder: Party::Task { task: 7 } };
    let golden = include_bytes!("golden/served_3.bin");
    assert_eq!(encode_served(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_served(golden, &sizes).unwrap(), value);
    assert!(decode_served(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_served(&trailing, &sizes).is_none());
}

#[test]
fn served_4() {
    let sizes = Sizes::STARTING;
    let value = Served::Decision { accepted: true, passed_to: Some(Party::Task { task: 7 }) };
    let golden = include_bytes!("golden/served_4.bin");
    assert_eq!(encode_served(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_served(golden, &sizes).unwrap(), value);
    assert!(decode_served(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_served(&trailing, &sizes).is_none());
}

#[test]
fn served_5() {
    let sizes = Sizes::STARTING;
    let value = Served::Effect {
        result: EffectResult::Made {
            resource: Name { segments: Box::from([Box::from(*b"abc")]) },
            revision: Some([7; 32]),
        },
    };
    let golden = include_bytes!("golden/served_5.bin");
    assert_eq!(encode_served(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_served(golden, &sizes).unwrap(), value);
    assert!(decode_served(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_served(&trailing, &sizes).is_none());
}

#[test]
fn served_6() {
    let sizes = Sizes::STARTING;
    let value = Served::Notes {
        notes: Box::from([Note { name: Box::from(*b"abc"), revision: 7, words: Box::from(*b"abc") }]),
        more: true,
    };
    let golden = include_bytes!("golden/served_6.bin");
    assert_eq!(encode_served(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_served(golden, &sizes).unwrap(), value);
    assert!(decode_served(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_served(&trailing, &sizes).is_none());
}

#[test]
fn served_7() {
    let sizes = Sizes::STARTING;
    let value = Served::Read {
        answer: ReadAnswer::Forge {
            answer: ForgeAnswer::Pull {
                number: 7,
                head: [7; 32],
                title: Box::from(*b"abc"),
                body: Box::from(*b"abc"),
                open: true,
            },
        },
    };
    let golden = include_bytes!("golden/served_7.bin");
    assert_eq!(encode_served(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_served(golden, &sizes).unwrap(), value);
    assert!(decode_served(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_served(&trailing, &sizes).is_none());
}

#[test]
fn served_8() {
    let sizes = Sizes::STARTING;
    let value = Served::Refused { task: Some(7), reason: Box::from(*b"abc") };
    let golden = include_bytes!("golden/served_8.bin");
    assert_eq!(encode_served(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_served(golden, &sizes).unwrap(), value);
    assert!(decode_served(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_served(&trailing, &sizes).is_none());
}

#[test]
fn served_9() {
    let sizes = Sizes::STARTING;
    let value = Served::Beyond { lacked: Box::from([Lack::Tool { family: 7 }]) };
    let golden = include_bytes!("golden/served_9.bin");
    assert_eq!(encode_served(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_served(golden, &sizes).unwrap(), value);
    assert!(decode_served(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_served(&trailing, &sizes).is_none());
}

#[test]
fn served_10() {
    let sizes = Sizes::STARTING;
    let value = Served::Unserved { reason: Unserved::Lost };
    let golden = include_bytes!("golden/served_10.bin");
    assert_eq!(encode_served(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_served(golden, &sizes).unwrap(), value);
    assert!(decode_served(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes).is_none());
    let mut trailing = golden.to_vec();
    trailing.push(0);
    assert!(decode_served(&trailing, &sizes).is_none());
}

#[test]
fn counts_and_lengths_are_rejected_before_decoded_storage_is_reserved() {
    let sizes = Sizes { entries: 1, call: 8, ..Sizes::STARTING };
    // Delegate tag followed by a peer-chosen count, with no task records.
    assert!(decode_call(&[0, 255, 255, 255, 255], &sizes).is_none());
    assert!(decode_call(&[0, 0, 0, 0, 1], &sizes).is_none());
    // A report's byte length cannot overrun its enclosing payload.
    assert!(decode_outcome(&[2, 255, 255, 255, 255], &sizes).is_none());
    assert!(decode_call(&[255], &sizes).is_none());
    assert!(decode_call(&[0, 0, 0, 0, 0, 0, 0, 0, 0], &sizes).is_none());
}
#[test]
fn optional_amendments_and_all_static_heap_formulas_have_explicit_bounds() {
    let value = Amendment {
        spec: None,
        wake: None,
        remove_dependencies: Box::from([]),
        authority: None,
        instructions: None,
        procedure: None,
    };
    let sizes = Sizes::STARTING;
    let golden = &[0, 0, 0, 0, 0, 0, 0, 0, 0];
    assert_eq!(encode_amendment(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_amendment(golden, &sizes).unwrap(), value);
    assert!(heap_charter(&sizes).is_some());
    assert!(heap_call(&sizes).is_some());
    assert!(heap_inbound(&sizes).is_some());
    assert!(heap_outcome(&sizes).is_some());
    assert!(heap_served(&sizes).is_some());
    assert!(heap_transcript(&sizes).is_some());
    let overflowing = Sizes { entries: u32::MAX, detail: u32::MAX, ..sizes };
    assert!(heap_transcript(&overflowing).is_none());
}

#[test]
fn a_zero_price_unit_is_refused_by_the_typed_schema() {
    let sizes = Sizes::STARTING;
    assert!(encode_prices(&Prices { input: 1, cached: 0, output: 1, unit: 0 }, &sizes).is_none());
    assert!(decode_prices(&[0; 28], &sizes).is_none());
}

#[test]
fn budget_fields_have_independent_wire_positions() {
    let sizes = Sizes::STARTING;
    let value =
        Budget { spend: 0x0102_0304_0506_0708, turns: 0x1112_1314, time: Duration::from_nanos(0x2122_2324_2526_2728) };
    let golden = &[1, 2, 3, 4, 5, 6, 7, 8, 17, 18, 19, 20, 33, 34, 35, 36, 37, 38, 39, 40];
    assert_eq!(encode_budget(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_budget(golden, &sizes), Some(value));
}

#[test]
fn price_categories_and_token_unit_have_independent_wire_positions() {
    let sizes = Sizes::STARTING;
    let value = Prices {
        input: 0x0102_0304_0506_0708,
        cached: 0x1112_1314_1516_1718,
        output: 0x2122_2324_2526_2728,
        unit: 0x3132_3334,
    };
    let golden =
        &[1, 2, 3, 4, 5, 6, 7, 8, 17, 18, 19, 20, 21, 22, 23, 24, 33, 34, 35, 36, 37, 38, 39, 40, 49, 50, 51, 52];
    assert_eq!(encode_prices(&value, &sizes).unwrap().as_ref(), golden);
    assert_eq!(decode_prices(golden, &sizes), Some(value));
}
