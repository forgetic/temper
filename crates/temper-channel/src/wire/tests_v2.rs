#![expect(clippy::disallowed_macros, reason = "ordinary Rust tests load fixed binary golden fixtures")]
//! Explicit v2 wire fixtures, independent of the Rust encoder.
use crate::{
    Sizes, codec,
    primitives::Encoder,
    wire::{self, v2::*},
};
use alloc::boxed::Box;
use skein_lib::{Duration, Reader};

#[test]
fn hello() {
    let sizes = Sizes::STARTING;
    let value = Hello {
        slots: 7,
        workstreams: Box::from([Box::from(*b"abc")]),
        hosting: Box::from([wire::Hosting { run: 7, attempt: 7, phase: wire::HostingPhase::Preparing }]),
        graces: Duration::from_nanos(7),
        push_deadline: Duration::from_nanos(7),
    };
    let golden = include_bytes!("golden/hello.bin");
    let mut out = Encoder::measure();
    put_hello(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_hello(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_hello(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_hello(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn repository() {
    let sizes = Sizes::STARTING;
    let value = Repository {
        tag: 7,
        name: Box::from(*b"abc"),
        remote: Box::from(*b"abc"),
        start: Start::Branch { branch: Box::from(*b"abc") },
        access: Access::ReadOnly,
        identity: 7,
    };
    let golden = include_bytes!("golden/repository.bin");
    let mut out = Encoder::measure();
    put_repository(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_repository(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_repository(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_repository(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn workspace() {
    let sizes = Sizes::STARTING;
    let value = Workspace {
        key: Box::from(*b"abc"),
        repositories: Box::from([Repository {
            tag: 7,
            name: Box::from(*b"abc"),
            remote: Box::from(*b"abc"),
            start: Start::Branch { branch: Box::from(*b"abc") },
            access: Access::ReadOnly,
            identity: 7,
        }]),
    };
    let golden = include_bytes!("golden/workspace.bin");
    let mut out = Encoder::measure();
    put_workspace(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_workspace(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_workspace(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_workspace(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn assign() {
    let sizes = Sizes::STARTING;
    let value = Assign {
        run: 7,
        attempt: 7,
        workspace: Workspace {
            key: Box::from(*b"abc"),
            repositories: Box::from([Repository {
                tag: 7,
                name: Box::from(*b"abc"),
                remote: Box::from(*b"abc"),
                start: Start::Branch { branch: Box::from(*b"abc") },
                access: Access::ReadOnly,
                identity: 7,
            }]),
        },
        save: Some(Box::from(*b"abc")),
        charter: Box::from(*b"abc"),
        transcript: Some(Box::from(*b"abc")),
        grants: Box::from([wire::Grant {
            account: 7,
            generation: 7,
            valid: Duration::from_nanos(7),
            token: Box::from(*b"abc"),
            account_id: Box::from(*b"abc"),
        }]),
    };
    let golden = include_bytes!("golden/assign.bin");
    let mut out = Encoder::measure();
    put_assign(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_assign(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_assign(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_assign(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn answer() {
    let sizes = Sizes::STARTING;
    let value = Answer {
        run: 7,
        attempt: 7,
        turns: 7,
        spent: 7,
        answer: LinkAnswer::Refused { refusal: wire::AssignmentRefusal::Busy },
    };
    let golden = include_bytes!("golden/answer.bin");
    let mut out = Encoder::measure();
    put_answer(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_answer(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_answer(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_answer(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn agent_repository() {
    let sizes = Sizes::STARTING;
    let value =
        AgentRepository { name: Box::from(*b"abc"), writable: true, conflicts: Box::from([Box::from(*b"abc")]) };
    let golden = include_bytes!("golden/agent_repository.bin");
    let mut out = Encoder::measure();
    put_agent_repository(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_agent_repository(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_agent_repository(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(
        get_agent_repository(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none()
    );
}

#[test]
fn agent_start() {
    let sizes = Sizes::STARTING;
    let value = AgentStart {
        charter: Box::from(*b"abc"),
        transcript: Some(Box::from(*b"abc")),
        repositories: Box::from([AgentRepository {
            name: Box::from(*b"abc"),
            writable: true,
            conflicts: Box::from([Box::from(*b"abc")]),
        }]),
        endpoints: Box::from([wire::EndpointDescriptor {
            endpoint: 7,
            provider: wire::Provider::OpenAi,
            host: Box::from(*b"abc"),
            address: wire::Address::V4 { bytes: [7; 4] },
            port: 7,
            path: Box::from(*b"abc"),
            account: 7,
            effort: Box::from(*b"abc"),
            thinking: Some(7),
        }]),
        grants: Box::from([wire::Grant {
            account: 7,
            generation: 7,
            valid: Duration::from_nanos(7),
            token: Box::from(*b"abc"),
            account_id: Box::from(*b"abc"),
        }]),
    };
    let golden = include_bytes!("golden/agent_start.bin");
    let mut out = Encoder::measure();
    put_agent_start(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_agent_start(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_agent_start(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_agent_start(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn finished() {
    let sizes = Sizes::STARTING;
    let value = Finished { turns: 7, spent: 7, finish: Finish::Ended { outcome: Box::from(*b"abc") } };
    let golden = include_bytes!("golden/finished.bin");
    let mut out = Encoder::measure();
    put_finished(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_finished(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_finished(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_finished(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn agent_call() {
    let sizes = Sizes::STARTING;
    let value = AgentCall { call: 7, ask: Ask::Push { title: Box::from(*b"abc"), body: Box::from(*b"abc") } };
    let golden = include_bytes!("golden/agent_call.bin");
    let mut out = Encoder::measure();
    put_agent_call(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_agent_call(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_agent_call(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_agent_call(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn turn() {
    let sizes = Sizes::STARTING;
    let value = Turn { run: 7, attempt: 7, turn: 7, spent: 7, read: Some(7), body: Box::from(*b"abc") };
    let golden = include_bytes!("golden/turn.bin");
    let mut out = Encoder::measure();
    put_turn(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_turn(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_turn(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_turn(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn turn_name() {
    let sizes = Sizes::STARTING;
    let value = TurnName { run: 7, attempt: 7, turn: 7 };
    let golden = include_bytes!("golden/turn_name.bin");
    let mut out = Encoder::measure();
    put_turn_name(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_turn_name(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_turn_name(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_turn_name(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn agent_turn() {
    let sizes = Sizes::STARTING;
    let value = AgentTurn { turn: 7, spent: 7, read: Some(7), body: Box::from(*b"abc") };
    let golden = include_bytes!("golden/agent_turn.bin");
    let mut out = Encoder::measure();
    put_agent_turn(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_agent_turn(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_agent_turn(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_agent_turn(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn start_0() {
    let sizes = Sizes::STARTING;
    let value = Start::Branch { branch: Box::from(*b"abc") };
    let golden = include_bytes!("golden/start_0.bin");
    let mut out = Encoder::measure();
    put_start(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_start(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_start(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_start(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn start_1() {
    let sizes = Sizes::STARTING;
    let value = Start::Commit { commit: [7; 32] };
    let golden = include_bytes!("golden/start_1.bin");
    let mut out = Encoder::measure();
    put_start(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_start(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_start(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_start(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn start_2() {
    let sizes = Sizes::STARTING;
    let value = Start::Saved { branch: Box::from(*b"abc") };
    let golden = include_bytes!("golden/start_2.bin");
    let mut out = Encoder::measure();
    put_start(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_start(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_start(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_start(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn start_3() {
    let sizes = Sizes::STARTING;
    let value = Start::Merge { branch: Box::from(*b"abc"), base: Box::from(*b"abc") };
    let golden = include_bytes!("golden/start_3.bin");
    let mut out = Encoder::measure();
    put_start(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_start(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_start(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_start(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn access_0() {
    let sizes = Sizes::STARTING;
    let value = Access::ReadOnly;
    let golden = include_bytes!("golden/access_0.bin");
    let mut out = Encoder::measure();
    put_access(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_access(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_access(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_access(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn access_1() {
    let sizes = Sizes::STARTING;
    let value = Access::Writable { push: Box::from(*b"abc"), expected: Some([7; 32]) };
    let golden = include_bytes!("golden/access_1.bin");
    let mut out = Encoder::measure();
    put_access(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_access(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_access(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_access(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn link_answer_0() {
    let sizes = Sizes::STARTING;
    let value = LinkAnswer::Refused { refusal: wire::AssignmentRefusal::Busy };
    let golden = include_bytes!("golden/link_answer_0.bin");
    let mut out = Encoder::measure();
    put_link_answer(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_link_answer(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_link_answer(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_link_answer(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn link_answer_1() {
    let sizes = Sizes::STARTING;
    let value = LinkAnswer::Ended {
        outcome: Box::from(*b"abc"),
        work: wire::Work { landed: Box::from([wire::Landed { tag: 7, commit: [7; 32] }]), saved: None },
    };
    let golden = include_bytes!("golden/link_answer_1.bin");
    let mut out = Encoder::measure();
    put_link_answer(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_link_answer(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_link_answer(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_link_answer(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn link_answer_2() {
    let sizes = Sizes::STARTING;
    let value = LinkAnswer::Parked {
        work: wire::Work { landed: Box::from([wire::Landed { tag: 7, commit: [7; 32] }]), saved: None },
    };
    let golden = include_bytes!("golden/link_answer_2.bin");
    let mut out = Encoder::measure();
    put_link_answer(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_link_answer(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_link_answer(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_link_answer(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn link_answer_3() {
    let sizes = Sizes::STARTING;
    let value = LinkAnswer::Failed {
        failure: wire::Failure::Run { failure: wire::RunFailure::Model },
        detail: Box::from(*b"abc"),
        work: wire::Work { landed: Box::from([wire::Landed { tag: 7, commit: [7; 32] }]), saved: None },
    };
    let golden = include_bytes!("golden/link_answer_3.bin");
    let mut out = Encoder::measure();
    put_link_answer(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_link_answer(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_link_answer(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_link_answer(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn finish_0() {
    let sizes = Sizes::STARTING;
    let value = Finish::Ended { outcome: Box::from(*b"abc") };
    let golden = include_bytes!("golden/finish_0.bin");
    let mut out = Encoder::measure();
    put_finish(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_finish(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_finish(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_finish(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn finish_1() {
    let sizes = Sizes::STARTING;
    let value = Finish::Parked;
    let golden = include_bytes!("golden/finish_1.bin");
    let mut out = Encoder::measure();
    put_finish(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_finish(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_finish(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_finish(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn finish_2() {
    let sizes = Sizes::STARTING;
    let value = Finish::Failed { failure: wire::RunFailure::Model };
    let golden = include_bytes!("golden/finish_2.bin");
    let mut out = Encoder::measure();
    put_finish(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_finish(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_finish(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_finish(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn ask_0() {
    let sizes = Sizes::STARTING;
    let value = Ask::Push { title: Box::from(*b"abc"), body: Box::from(*b"abc") };
    let golden = include_bytes!("golden/ask_0.bin");
    let mut out = Encoder::measure();
    put_ask(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_ask(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_ask(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_ask(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn ask_1() {
    let sizes = Sizes::STARTING;
    let value = Ask::Relay { body: Box::from(*b"abc") };
    let golden = include_bytes!("golden/ask_1.bin");
    let mut out = Encoder::measure();
    put_ask(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_ask(&mut out, &value, &sizes).unwrap();
    assert_eq!(out.finish().as_ref(), golden);
    let mut input = Reader::new(golden);
    assert_eq!(get_ask(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_ask(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

#[test]
fn frame_hello_v2() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::HelloV2 {
        hello: Hello {
            slots: 7,
            workstreams: Box::from([Box::from(*b"abc")]),
            hosting: Box::from([wire::Hosting { run: 7, attempt: 7, phase: wire::HostingPhase::Preparing }]),
            graces: Duration::from_nanos(7),
            push_deadline: Duration::from_nanos(7),
        },
    };
    let golden = include_bytes!("golden/frame_hello_v2.bin");
    assert_eq!(codec::encode_version(&value, &sizes, 2).unwrap().as_ref(), golden);
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

#[test]
fn frame_answer_v2() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::AnswerV2 {
        answer: Answer {
            run: 7,
            attempt: 7,
            turns: 7,
            spent: 7,
            answer: LinkAnswer::Refused { refusal: wire::AssignmentRefusal::Busy },
        },
    };
    let golden = include_bytes!("golden/frame_answer_v2.bin");
    assert_eq!(codec::encode_version(&value, &sizes, 2).unwrap().as_ref(), golden);
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

#[test]
fn frame_assign_v2() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::AssignV2 {
        assign: Assign {
            run: 7,
            attempt: 7,
            workspace: Workspace {
                key: Box::from(*b"abc"),
                repositories: Box::from([Repository {
                    tag: 7,
                    name: Box::from(*b"abc"),
                    remote: Box::from(*b"abc"),
                    start: Start::Branch { branch: Box::from(*b"abc") },
                    access: Access::ReadOnly,
                    identity: 7,
                }]),
            },
            save: Some(Box::from(*b"abc")),
            charter: Box::from(*b"abc"),
            transcript: Some(Box::from(*b"abc")),
            grants: Box::from([wire::Grant {
                account: 7,
                generation: 7,
                valid: Duration::from_nanos(7),
                token: Box::from(*b"abc"),
                account_id: Box::from(*b"abc"),
            }]),
        },
    };
    let golden = include_bytes!("golden/frame_assign_v2.bin");
    assert_eq!(codec::encode_version(&value, &sizes, 2).unwrap().as_ref(), golden);
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

#[test]
fn frame_agent_call_v2() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::AgentCallV2 {
        call: AgentCall { call: 7, ask: Ask::Push { title: Box::from(*b"abc"), body: Box::from(*b"abc") } },
    };
    let golden = include_bytes!("golden/frame_agent_call_v2.bin");
    assert_eq!(codec::encode_version(&value, &sizes, 2).unwrap().as_ref(), golden);
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

#[test]
fn frame_finish_v2() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::FinishV2 {
        finish: Finished { turns: 7, spent: 7, finish: Finish::Ended { outcome: Box::from(*b"abc") } },
    };
    let golden = include_bytes!("golden/frame_finish_v2.bin");
    assert_eq!(codec::encode_version(&value, &sizes, 2).unwrap().as_ref(), golden);
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

#[test]
fn frame_agent_start_v2() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::AgentStartV2 {
        start: AgentStart {
            charter: Box::from(*b"abc"),
            transcript: Some(Box::from(*b"abc")),
            repositories: Box::from([AgentRepository {
                name: Box::from(*b"abc"),
                writable: true,
                conflicts: Box::from([Box::from(*b"abc")]),
            }]),
            endpoints: Box::from([wire::EndpointDescriptor {
                endpoint: 7,
                provider: wire::Provider::OpenAi,
                host: Box::from(*b"abc"),
                address: wire::Address::V4 { bytes: [7; 4] },
                port: 7,
                path: Box::from(*b"abc"),
                account: 7,
                effort: Box::from(*b"abc"),
                thinking: Some(7),
            }]),
            grants: Box::from([wire::Grant {
                account: 7,
                generation: 7,
                valid: Duration::from_nanos(7),
                token: Box::from(*b"abc"),
                account_id: Box::from(*b"abc"),
            }]),
        },
    };
    let golden = include_bytes!("golden/frame_agent_start_v2.bin");
    assert_eq!(codec::encode_version(&value, &sizes, 2).unwrap().as_ref(), golden);
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

#[test]
fn frame_turn() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Turn {
        turn: Turn { run: 7, attempt: 7, turn: 7, spent: 7, read: Some(7), body: Box::from(*b"abc") },
    };
    let golden = include_bytes!("golden/frame_turn.bin");
    assert_eq!(codec::encode_version(&value, &sizes, 2).unwrap().as_ref(), golden);
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

#[test]
fn frame_acknowledge_turn() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::AcknowledgeTurn { turn: TurnName { run: 7, attempt: 7, turn: 7 } };
    let golden = include_bytes!("golden/frame_acknowledge_turn.bin");
    assert_eq!(codec::encode_version(&value, &sizes, 2).unwrap().as_ref(), golden);
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

#[test]
fn frame_turn_busy() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::TurnBusy { turn: TurnName { run: 7, attempt: 7, turn: 7 } };
    let golden = include_bytes!("golden/frame_turn_busy.bin");
    assert_eq!(codec::encode_version(&value, &sizes, 2).unwrap().as_ref(), golden);
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

#[test]
fn frame_agent_turn() {
    let sizes = Sizes::STARTING;
    let value =
        wire::Message::AgentTurn { turn: AgentTurn { turn: 7, spent: 7, read: Some(7), body: Box::from(*b"abc") } };
    let golden = include_bytes!("golden/frame_agent_turn.bin");
    assert_eq!(codec::encode_version(&value, &sizes, 2).unwrap().as_ref(), golden);
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

#[test]
fn fixed_unsupported_status_has_the_same_small_frame_in_both_versions() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Unsupported { kind: 264 };
    let golden = &[0, 17, 0, 0, 0, 0, 0, 2, 1, 8];
    for version in [1, 2] {
        assert_eq!(codec::encode_version(&value, &sizes, version).unwrap().as_ref(), golden);
        assert_eq!(codec::decode_version(golden, &sizes, version).unwrap(), value);
    }
}

#[test]
fn a_relay_at_the_call_limit_fits_when_call_is_larger_than_detail() {
    let sizes = Sizes { call: 64, detail: 2, ..Sizes::STARTING };
    let value =
        wire::Message::AgentCallV2 { call: AgentCall { call: 7, ask: Ask::Relay { body: Box::from([41; 64]) } } };
    // Call name (8), ask tag (1), byte length (4), and all 64 relay bytes.
    assert_eq!(crate::sizes::largest_version(0x0201, &sizes, 2), Some(77));
    assert_eq!(crate::sizes::frame_version(0x0201, &sizes, 2), Some(85));
    let frame = codec::encode_version(&value, &sizes, 2).unwrap();
    assert_eq!(frame.len(), 85);
    assert_eq!(codec::header_version(frame.get(..8).unwrap(), &sizes, 2).unwrap().length, 77);
    assert_eq!(codec::decode_version(&frame, &sizes, 2), Some(value));
}

#[test]
fn link_answer_identity_and_accounting_have_independent_wire_positions() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::AnswerV2 {
        answer: Answer {
            run: 0x0102_0304_0506_0708,
            attempt: 0x1112_1314_1516_1718,
            turns: 0x2122_2324,
            spent: 0x3132_3334_3536_3738,
            answer: LinkAnswer::Refused { refusal: wire::AssignmentRefusal::Busy },
        },
    };
    let golden = &[
        1, 2, 0, 0, 0, 0, 0, 30, 1, 2, 3, 4, 5, 6, 7, 8, 17, 18, 19, 20, 21, 22, 23, 24, 33, 34, 35, 36, 49, 50, 51,
        52, 53, 54, 55, 56, 0, 0,
    ];
    assert_eq!(codec::encode_version(&value, &sizes, 2).unwrap().as_ref(), golden);
    assert_eq!(codec::decode_version(golden, &sizes, 2), Some(value));
}

#[test]
fn turn_names_number_spend_and_read_watermark_have_independent_wire_positions() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Turn {
        turn: Turn {
            run: 0x0102_0304_0506_0708,
            attempt: 0x1112_1314_1516_1718,
            turn: 0x2122_2324,
            spent: 0x3132_3334_3536_3738,
            read: Some(0x4142_4344_4546_4748),
            body: Box::from(*b"T"),
        },
    };
    let golden = &[
        1, 8, 0, 0, 0, 0, 0, 42, 1, 2, 3, 4, 5, 6, 7, 8, 17, 18, 19, 20, 21, 22, 23, 24, 33, 34, 35, 36, 49, 50, 51,
        52, 53, 54, 55, 56, 1, 65, 66, 67, 68, 69, 70, 71, 72, 0, 0, 0, 1, 84,
    ];
    assert_eq!(codec::encode_version(&value, &sizes, 2).unwrap().as_ref(), golden);
    assert_eq!(codec::decode_version(golden, &sizes, 2), Some(value));
}

#[test]
fn a_push_keeps_its_short_title_apart_from_its_longer_body() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::AgentCallV2 {
        call: AgentCall {
            call: 0x0102_0304_0506_0708,
            ask: Ask::Push { title: Box::from(*b"hi"), body: Box::from(*b"world") },
        },
    };
    let golden = &[
        2, 1, 0, 0, 0, 0, 0, 24, 1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 2, 104, 105, 0, 0, 0, 5, 119, 111, 114, 108, 100,
    ];
    assert_eq!(codec::encode_version(&value, &sizes, 2).unwrap().as_ref(), golden);
    assert_eq!(codec::decode_version(golden, &sizes, 2), Some(value));
}
