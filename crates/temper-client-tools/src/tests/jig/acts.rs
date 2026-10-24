use crate::tests::fixtures;
use jig_client::contract as c;
#[test]
fn a_message_under_a_name_sends_it_as_the_engine_key() {
    let (root, call) = fixtures::message();
    assert_eq!(root.sent_name(call), Some(fixtures::named("port-answer")));
}
#[test]
fn a_message_prints_done() {
    let (mut root, call) = fixtures::message();
    root.done(call, c::Outcome::Changed);
    let answer = root.answer(call).expect("answered once durable");
    assert_eq!((answer.status, answer.stdout.as_ref()), (0, b"done: task 41, message\n".as_slice()));
}
#[test]
fn an_act_without_key_draws_one() {
    let mut root = fixtures::signed_in(fixtures::PARTY);
    let call = root.submit(fixtures::STOP_CALL);
    root.serve(call, fixtures::task_result(fixtures::offered(&[c::ActKind::Stop])));
    assert_eq!(root.sent_name(call), Some(*root.saved.get(&call).expect("drawn key saved")));
}
#[test]
fn an_act_not_offered_exits_1_with_nothing_sent() {
    let mut root = fixtures::signed_in(fixtures::PARTY);
    let call = root.submit(fixtures::MESSAGE_CALL);
    root.serve(call, fixtures::task_result(fixtures::offered(&[])));
    assert_eq!((root.answer(call).expect("locally refused").status, root.sent_name(call)), (1, None));
}
#[test]
fn an_engine_key_conflict_exits_2() {
    let (mut root, call) = fixtures::message();
    root.answered(call, c::Answer::Refused { refusal: c::Refusal::KeyConflict, at: None });
    assert_eq!(root.answer(call).expect("engine refused").status, 2);
}
#[test]
fn held_exits_1_with_nothing_sent() {
    let (mut root, _) = fixtures::message();
    let call = root.submit(fixtures::OTHER_MESSAGE);
    root.serve(call, fixtures::task_result(fixtures::offered(&[c::ActKind::Message])));
    assert_eq!((root.answer(call).expect("held answered").status, root.sent_name(call)), (1, None));
}
#[test]
fn chat_prints_the_task_it_made() {
    let (mut root, call) = fixtures::send_chat(fixtures::CHAT_BUDGET, fixtures::SEED, fixtures::EMPTY.into());
    let mut tasks = skein_lib::List::with_capacity(fixtures::KEPT);
    tasks.push(fixtures::TASK).expect("one task");
    root.done(call, c::Outcome::Made { what: c::Made::Tasks { tasks } });
    let answer = root.answer(call).expect("chat answered");
    assert_eq!((answer.status, answer.stdout.as_ref()), (0, b"done: task 41, chat\n".as_slice()));
}
#[test]
fn a_proposable_act_prints_to_whom() {
    let (mut root, call) =
        fixtures::send_chat(fixtures::CHAT_BUDGET + fixtures::OVER_BOUND, fixtures::SEED, fixtures::EMPTY.into());
    root.done(
        call,
        c::Outcome::Proposed {
            proposal: c::ProposalKey { proposer: c::Proposer::Party(fixtures::PARTY), number: fixtures::PROPOSAL },
        },
    );
    assert_eq!(root.answer(call).expect("proposed").stdout.as_ref(), b"proposed: p73/17, to party 91\n");
}
#[test]
fn chat_checks_both_sides_of_its_bounds() {
    for (budget, expected) in [
        (fixtures::CHAT_BUDGET, b"create".as_slice()),
        (fixtures::CHAT_BUDGET + fixtures::OVER_BOUND, b"propose".as_slice()),
        (fixtures::CHAT_CEILING, b"propose".as_slice()),
        (fixtures::CHAT_CEILING + fixtures::OVER_BOUND, b"refused".as_slice()),
    ] {
        let (mut root, call) = fixtures::send_chat(budget, fixtures::SEED, fixtures::EMPTY.into());
        let result = match root.sends.get(&call) {
            Some((_, _, jig_client::Ask::Jig(ask))) => {
                if let c::Ask::Create { .. } = ask.as_ref() {
                    b"create".as_slice()
                } else if let c::Ask::Propose { .. } = ask.as_ref() {
                    b"propose".as_slice()
                } else {
                    panic!("chat request")
                }
            }
            Some(_) => panic!("jig request"),
            None => {
                assert_eq!(root.answer(call).expect("refused locally").status, 1);
                b"refused".as_slice()
            }
        };
        assert_eq!(result, expected, "budget {budget}");
    }
}
#[test]
fn decide_sends_its_two_forms() {
    for (argv, expected) in [
        (
            fixtures::DECIDE_TASK_CALL,
            c::DecideOn::Proposal {
                running: None,
                proposal: c::ProposalKey { proposer: c::Proposer::Task(fixtures::TASK), number: fixtures::PROPOSAL },
                decision: c::Decision::Accepted,
            },
        ),
        (
            fixtures::DECIDE_ESCALATION_CALL,
            c::DecideOn::Escalation {
                task: fixtures::TASK,
                revision: c::Revision { value: fixtures::HOLD },
                decision: c::Decision::Released { again: None },
            },
        ),
    ] {
        let (root, call) = fixtures::send_act(argv, fixtures::SEED, fixtures::EMPTY.into());
        let (_, _, ask) = root.sends.get(&call).expect("decision sent");
        assert_eq!(
            ask,
            &jig_client::Ask::Jig(Box::new(c::Ask::Decide { project: fixtures::PROJECT, on: expected })),
            "{argv:?}"
        );
    }
}
#[test]
fn decided_first_prints_the_winner() {
    let (mut root, call) = fixtures::message();
    root.done(
        call,
        c::Outcome::DecidedFirst {
            by: c::Party {
                number: fixtures::OTHER_PARTY,
                kind: c::PartyKind::Person,
                name: fixtures::act_text(b"ada"),
            },
            how: c::Decision::Accepted,
            at: fixtures::DECIDED_AT,
        },
    );
    assert_eq!(
        root.answer(call).expect("winner answered").stdout.as_ref(),
        b"decided first: by ada, accepted, 14:02\n"
    );
}
#[test]
fn each_kept_request_prints_its_operation_without_keys() {
    for (records, json, expected) in [
        (fixtures::restored_message(fixtures::PARTY), false, b"kept: message, done: task 41, message\n".as_slice()),
        (
            fixtures::restored_message(fixtures::PARTY),
            true,
            b"{\"operation\":\"message\",\"outcome\":{\"done\":\"changed\"}}\n".as_slice(),
        ),
        (fixtures::kept(fixtures::RESTORED_PREFIX), false, b"kept: stop, done: task 41, stop\n".as_slice()),
        (
            fixtures::kept(fixtures::RESTORED_PREFIX),
            true,
            b"{\"operation\":\"stop\",\"outcome\":{\"done\":\"changed\"}}\n".as_slice(),
        ),
    ] {
        let mut root = fixtures::started_with(records, fixtures::SEED, json);
        root.deliver(c::Delivery::Me(Box::new(fixtures::me(fixtures::PARTY))));
        root.done(fixtures::EMPTY, c::Outcome::Changed);
        assert_eq!(root.kept_lines.as_slice(), &[Box::<[u8]>::from(expected)], "{expected:?}");
    }
}
#[test]
fn kept_counts_only_this_partys_unanswered_requests() {
    let mut root = fixtures::started(fixtures::restored_message(fixtures::PARTY));
    root.deliver(c::Delivery::Me(Box::new(fixtures::me(fixtures::PARTY))));
    assert_eq!(root.root.kept(), fixtures::KEPT);
    root.done(fixtures::EMPTY, c::Outcome::Changed);
    assert_eq!(root.root.kept(), fixtures::EMPTY);
}
#[test]
fn another_partys_parked_name_is_held() {
    let mut root = fixtures::started(fixtures::restored_message(fixtures::OTHER_PARTY));
    root.deliver(c::Delivery::Me(Box::new(fixtures::me(fixtures::PARTY))));
    let call = root.submit(fixtures::MESSAGE_CALL);
    root.serve(call, fixtures::task_result(fixtures::offered(&[c::ActKind::Message])));
    assert_eq!(
        (root.answer(call).expect("held name answered").status, root.sent_name(call), root.root.kept()),
        (1, None, fixtures::EMPTY)
    );
}
#[test]
fn other_party_kept_lines_use_the_selected_format() {
    for (json, expected) in [
        (false, b"kept: 1 for party 74, unsent while it is not signed in here\n".as_slice()),
        (true, b"{\"party\":74,\"unsent\":1}\n".as_slice()),
    ] {
        let mut root = fixtures::started_with(fixtures::restored_message(fixtures::OTHER_PARTY), fixtures::SEED, json);
        root.deliver(c::Delivery::Me(Box::new(fixtures::me(fixtures::PARTY))));
        root.deliver(c::Delivery::Me(Box::new(fixtures::me(fixtures::PARTY))));
        assert_eq!(root.kept_lines.as_slice(), &[Box::<[u8]>::from(expected)], "json {json}");
    }
}
#[test]
fn a_named_chat_has_the_same_request_bytes_under_two_clocks() {
    let (first, a) = fixtures::send_chat(fixtures::CHAT_BUDGET, fixtures::SEED, fixtures::EMPTY.into());
    let (second, b) = fixtures::send_chat(fixtures::CHAT_BUDGET, fixtures::OTHER_SEED, fixtures::LATER);
    assert_eq!(
        fixtures::encoded(&first.sends.get(&a).expect("first chat").2),
        fixtures::encoded(&second.sends.get(&b).expect("second chat").2)
    );
}
#[test]
fn a_named_act_has_no_process_local_fields() {
    for argv in fixtures::ACT_CALLS {
        let (first, a) = fixtures::send_act(argv, fixtures::SEED, fixtures::EMPTY.into());
        let (second, b) = fixtures::send_act(argv, fixtures::OTHER_SEED, fixtures::LATER);
        let Some(first) = first.sends.get(&a) else { panic!("first act {argv:?}") };
        let Some(second) = second.sends.get(&b) else { panic!("second act {argv:?}") };
        assert_eq!(fixtures::encoded(&first.2), fixtures::encoded(&second.2), "{argv:?}");
    }
}
#[test]
fn changed_me_inputs_can_change_a_named_ask() {
    let (first, a) = fixtures::send_chat(fixtures::CHAT_BUDGET, fixtures::SEED, fixtures::EMPTY.into());
    let (second, b) =
        fixtures::send_chat(fixtures::CHAT_BUDGET - fixtures::OVER_BOUND, fixtures::SEED, fixtures::EMPTY.into());
    assert_ne!(
        fixtures::encoded(&first.sends.get(&a).expect("first chat").2),
        fixtures::encoded(&second.sends.get(&b).expect("changed chat").2)
    );
}
#[test]
fn a_restored_proposals_kept_line_prints_its_identity_without_a_recipient() {
    let mut root = fixtures::started(fixtures::restored_chat());
    root.deliver(c::Delivery::Me(Box::new(fixtures::me(fixtures::PARTY))));
    root.done(
        fixtures::EMPTY,
        c::Outcome::Proposed {
            proposal: c::ProposalKey {
                proposer: c::Proposer::Task(fixtures::KEPT_PROPOSER),
                number: fixtures::PROPOSAL,
            },
        },
    );
    assert_eq!(root.kept_lines.as_slice(), &[Box::<[u8]>::from(b"kept: chat, proposed: t3/17\n".as_slice())]);
}
#[test]
fn close_passes_its_reason_or_empty() {
    for (extra, reason) in
        [(&[][..], b"".as_slice()), (fixtures::CLOSE_REASON, fixtures::CLOSE_REASON_WORDS.as_bytes())]
    {
        let argv = fixtures::extended(fixtures::CLOSE_CALL, extra);
        let (root, call) = fixtures::send_act(argv.as_slice(), fixtures::SEED, fixtures::EMPTY.into());
        let operations = fixtures::close_operations(&root, call);
        assert_eq!(
            operations.as_slice(),
            &[c::Operation::Cancel {
                project: fixtures::PROJECT,
                task: fixtures::TASK,
                reason: fixtures::act_text(reason),
                grace: Some(fixtures::CHAT_SPAN),
            }],
            "{extra:?}"
        );
    }
}
#[test]
fn close_moves_selected_delegates() {
    let mut root = fixtures::chat_root(fixtures::CHAT_BUDGET, fixtures::SEED);
    let argv = fixtures::extended(fixtures::CLOSE_CALL, fixtures::CLOSE_MOVE);
    let call = root.submit(argv.as_slice());
    root.serve(call, fixtures::task_result(fixtures::with_delegates()));
    let operations = fixtures::close_operations(&root, call);
    assert_eq!(
        operations.as_slice(),
        &[
            c::Operation::Move { project: fixtures::PROJECT, task: fixtures::MOVED, reason: fixtures::act_text(b"") },
            c::Operation::Cancel {
                project: fixtures::PROJECT,
                task: fixtures::CANCELLED,
                reason: fixtures::act_text(b""),
                grace: None
            },
            c::Operation::Cancel {
                project: fixtures::PROJECT,
                task: fixtures::TASK,
                reason: fixtures::act_text(b""),
                grace: Some(fixtures::CHAT_SPAN)
            },
        ]
    );
}
#[test]
fn proposal_names_preserve_task_or_party() {
    for (argv, proposer) in [
        (fixtures::DECIDE_TASK_CALL, c::Proposer::Task(fixtures::TASK)),
        (fixtures::DECIDE_PARTY_CALL, c::Proposer::Party(fixtures::PARTY)),
    ] {
        let (root, call) = fixtures::send_act(argv, fixtures::SEED, fixtures::EMPTY.into());
        let (_, _, jig_client::Ask::Jig(ask)) = root.sends.get(&call).expect("decision sent") else {
            panic!("jig decision")
        };
        assert_eq!(
            ask.as_ref(),
            &c::Ask::Decide {
                project: fixtures::PROJECT,
                on: c::DecideOn::Proposal {
                    running: None,
                    proposal: c::ProposalKey { proposer, number: fixtures::PROPOSAL },
                    decision: c::Decision::Accepted,
                }
            },
            "{argv:?}"
        );
    }
}
#[test]
fn chat_sends_the_templates_deadline_span() {
    let (root, call) = fixtures::send_chat(fixtures::CHAT_BUDGET, fixtures::SEED, fixtures::EMPTY.into());
    let c::Ask::Create { tasks, .. } = fixtures::act_ask(&root, call) else { panic!("create") };
    assert_eq!(tasks.get(fixtures::EMPTY).expect("one chat").deadline, c::NewDeadline::Span(fixtures::CHAT_SPAN));
}
#[test]
fn client_refusals_exit_1() {
    for outcome in [
        jig_client::Outcome::NotKept,
        jig_client::Outcome::Unknown,
        jig_client::Outcome::Limit,
        jig_client::Outcome::Held,
    ] {
        let (mut root, call) = fixtures::message();
        root.client_answer(call, outcome.clone());
        assert_eq!(root.answer(call).expect("client refused").status, 1, "{outcome:?}");
    }
}
#[test]
fn unconfirmed_says_tools_bug() {
    let (mut root, call) = fixtures::message();
    root.client_answer(call, jig_client::Outcome::Unconfirmed);
    let answer = root.answer(call).expect("unconfirmed answered");
    assert_eq!(
        (answer.status, answer.stderr.as_ref()),
        (1, b"tools' bug; unconfirmed request; report it as a finding\n".as_slice())
    );
}
#[test]
fn a_client_key_collision_says_tools_bug_without_sending() {
    let (mut root, _) = fixtures::message();
    crate::root::act_fixture::repeat_number(&mut root.root);
    let call = root.submit(fixtures::OTHER_MESSAGE);
    root.serve(call, fixtures::task_result(fixtures::offered(&[c::ActKind::Message])));
    let answer = root.answer(call).expect("collision answered");
    assert_eq!(
        (answer.status, answer.stderr.as_ref(), root.sent_name(call)),
        (1, b"tools' bug; client key collision; report it as a finding\n".as_slice(), None)
    );
}
#[test]
fn dropped_kept_counts_use_the_selected_format() {
    for (json, expected) in [
        (false, b"dropped: 1 kept records these tools cannot read\n".as_slice()),
        (true, b"{\"dropped\":1}\n".as_slice()),
    ] {
        let mut root = fixtures::started_with(skein_lib::List::with_capacity(fixtures::EMPTY), fixtures::SEED, json);
        root.tell(jig_client::Told::Dropped { count: fixtures::KEPT });
        assert_eq!(root.kept_lines.as_slice(), &[Box::<[u8]>::from(expected)], "json {json}");
    }
}
#[test]
fn a_refusal_exits_2_with_its_alternatives() {
    let json = fixtures::json_message();
    for (argv, refusal, expected) in [
        (fixtures::MESSAGE_CALL, c::Refusal::Moved { now: c::Revision { value: fixtures::HOLD }, what: c::Moved::Spec }, b"message: The object changed\nread again: task 41\n".as_slice()),
        (json.as_slice(), c::Refusal::Moved { now: c::Revision { value: fixtures::HOLD }, what: c::Moved::Spec }, b"{\"operation\":\"message\",\"outcome\":{\"refused\":{\"refusal\":{\"moved\":{\"now\":{\"value\":2},\"what\":\"spec\"}},\"at\":null}},\"task\":41,\"to\":null,\"read_again\":\"task 41\"}".as_slice()),
        (fixtures::MESSAGE_CALL, c::Refusal::Authority { lacking: c::Lacking::Approver, proposable: Some(true) }, b"message: Authority missing\npropose instead: message 41 'Use port 8080' --key port-answer\n".as_slice()),
        (json.as_slice(), c::Refusal::Authority { lacking: c::Lacking::Approver, proposable: Some(true) }, b"{\"operation\":\"message\",\"outcome\":{\"refused\":{\"refusal\":{\"authority\":{\"lacking\":\"approver\",\"proposable\":true}},\"at\":null}},\"task\":41,\"to\":null,\"propose_instead\":\"message 41 'Use port 8080' --key port-answer\"}".as_slice()),
    ] {
        let (mut root, call) = fixtures::send_act(argv, fixtures::SEED, fixtures::EMPTY.into());
        root.answered(call, c::Answer::Refused { refusal, at: None });
        let answer = root.answer(call).expect("refusal answered");
        assert_eq!((answer.status, answer.stderr.as_ref()), (2, expected), "{argv:?}");
    }
}

#[test]
fn dependency_refusals_print_jigs_words_and_integer_facts() {
    for (refusal, words, json) in [
        (
            c::Refusal::Circle { tasks: skein_lib::List::with_capacity(0), cut: true },
            b"These tasks would wait for one another".as_slice(),
            br#""circle":{"tasks":[],"cut":true}"#.as_slice(),
        ),
        (
            c::Refusal::Dependency { dependency: fixtures::TASK, how: c::OwnerTaskStatus::Failed },
            b"The added dependency has ended",
            br#""dependency":{"dependency":41,"how":"failed"}"#,
        ),
        (
            c::Refusal::Kept { pending: skein_lib::List::with_capacity(0) },
            b"This answer waits for pending dependencies",
            br#""kept":{"pending":[]}"#,
        ),
    ] {
        for (args, expected) in
            [(fixtures::MESSAGE_CALL, words), (&["message", "41", "hello", "--key", "words", "--json"][..], json)]
        {
            let mut root = fixtures::signed_in(fixtures::PARTY);
            let call = root.submit(args);
            root.serve(call, fixtures::task_result(fixtures::offered(&[c::ActKind::Message])));
            root.answered(call, c::Answer::Refused { refusal: refusal.clone(), at: None });
            let printed = root.answer(call).expect("refused");
            assert!(skein_lib::bytes::find(&printed.stderr, expected).is_some(), "{printed:?}");
        }
    }
}

#[test]
fn sign_in_redirect_refusals_keep_each_reason_in_json() {
    for (why, expected) in [
        (c::SignInRedirectWhy::NotWaiting, br#""sign_in_redirect":{"why":"not_waiting"}"#.as_slice()),
        (c::SignInRedirectWhy::Address, br#""sign_in_redirect":{"why":"address"}"#),
        (c::SignInRedirectWhy::State, br#""sign_in_redirect":{"why":"state"}"#),
        (c::SignInRedirectWhy::TooMany, br#""sign_in_redirect":{"why":"too_many"}"#),
    ] {
        let mut root = fixtures::signed_in(fixtures::PARTY);
        let call = root.submit(&["message", "41", "hello", "--key", "words", "--json"]);
        root.serve(call, fixtures::task_result(fixtures::offered(&[c::ActKind::Message])));
        root.answered(call, c::Answer::Refused { refusal: c::Refusal::SignInRedirect { why }, at: None });
        let printed = root.answer(call).expect("refused");
        assert_eq!(printed.status, 2);
        assert!(skein_lib::bytes::find(&printed.stderr, expected).is_some(), "{printed:?}");
    }
}

#[test]
fn an_account_finishing_outcome_keeps_its_source_in_json() {
    let mut root = fixtures::signed_in(fixtures::PARTY);
    let call = root.submit(&["message", "41", "hello", "--key", "words", "--json"]);
    root.serve(call, fixtures::task_result(fixtures::offered(&[c::ActKind::Message])));
    root.done(call, c::Outcome::Owner(c::OwnerOutcome::AccountFinishing { source: 23 }));
    let printed = root.answer(call).expect("answered");
    assert_eq!(printed.status, 0);
    assert!(skein_lib::bytes::find(&printed.stdout, br#""account_finishing":{"source":23}"#).is_some(), "{printed:?}");
}
