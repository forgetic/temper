//! Checked-in v2 wire fixtures and independent literal wire-position checks.
use crate::{
    Sizes, codec,
    primitives::Encoder,
    wire::{self, v2::*},
};
use alloc::boxed::Box;
use skein_lib::{Duration, Reader};

use crate::golden;

golden::fixtures! {
    hello,
    repository,
    workspace,
    assign,
    answer,
    agent_repository,
    agent_start,
    finished,
    agent_call,
    turn,
    turn_name,
    agent_turn,
    start_0,
    start_1,
    start_2,
    start_3,
    access_0,
    access_1,
    link_answer_0,
    link_answer_1,
    link_answer_2,
    link_answer_3,
    finish_0,
    finish_1,
    finish_2,
    ask_0,
    ask_1,
    frame_hello_v2,
    frame_answer_v2,
    frame_assign_v2,
    frame_agent_call_v2,
    frame_finish_v2,
    frame_agent_start_v2,
    frame_turn,
    frame_acknowledge_turn,
    frame_turn_busy,
    frame_agent_turn,
}

#[test]
fn golden_manifest_matches_files_and_fixture_cases() {
    golden::check("wire", FIXTURES, false);
}

#[test]
#[ignore = "rewrites checked-in fixtures; see golden/README.md"]
fn regenerate_goldens() {
    golden::check("wire", FIXTURES, true);
}

fn hello(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = Hello {
        slots: 7,
        workstreams: Box::from([Box::from(*b"abc")]),
        hosting: Box::from([wire::Hosting { run: 7, attempt: 7, phase: wire::HostingPhase::Preparing }]),
        graces: Duration::from_nanos(7),
        push_deadline: Duration::from_nanos(7),
    };
    let mut out = Encoder::measure();
    put_hello(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_hello(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "hello.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_hello(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_hello(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn repository(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = Repository {
        tag: 7,
        name: Box::from(*b"abc"),
        remote: Box::from(*b"abc"),
        start: Start::Branch { branch: Box::from(*b"abc") },
        access: Access::ReadOnly,
        identity: 7,
    };
    let mut out = Encoder::measure();
    put_repository(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_repository(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "repository.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_repository(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_repository(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn workspace(run: &mut golden::Run) {
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
    let mut out = Encoder::measure();
    put_workspace(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_workspace(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "workspace.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_workspace(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_workspace(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn assign(run: &mut golden::Run) {
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
    let mut out = Encoder::measure();
    put_assign(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_assign(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "assign.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_assign(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_assign(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn answer(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = Answer {
        run: 7,
        attempt: 7,
        turns: 7,
        spent: 7,
        answer: LinkAnswer::Refused { refusal: wire::AssignmentRefusal::Busy },
    };
    let mut out = Encoder::measure();
    put_answer(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_answer(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "answer.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_answer(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_answer(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn agent_repository(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value =
        AgentRepository { name: Box::from(*b"abc"), writable: true, conflicts: Box::from([Box::from(*b"abc")]) };
    let mut out = Encoder::measure();
    put_agent_repository(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_agent_repository(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "agent_repository.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_agent_repository(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(
        get_agent_repository(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none()
    );
}

fn agent_start(run: &mut golden::Run) {
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
    let mut out = Encoder::measure();
    put_agent_start(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_agent_start(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "agent_start.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_agent_start(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_agent_start(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn finished(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = Finished { turns: 7, spent: 7, finish: Finish::Ended { outcome: Box::from(*b"abc") } };
    let mut out = Encoder::measure();
    put_finished(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_finished(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "finished.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_finished(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_finished(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn agent_call(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = AgentCall { call: 7, ask: Ask::Push { title: Box::from(*b"abc"), body: Box::from(*b"abc") } };
    let mut out = Encoder::measure();
    put_agent_call(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_agent_call(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "agent_call.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_agent_call(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_agent_call(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn turn(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = Turn { run: 7, attempt: 7, turn: 7, spent: 7, read: Some(7), body: Box::from(*b"abc") };
    let mut out = Encoder::measure();
    put_turn(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_turn(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "turn.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_turn(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_turn(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn turn_name(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = TurnName { run: 7, attempt: 7, turn: 7 };
    let mut out = Encoder::measure();
    put_turn_name(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_turn_name(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "turn_name.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_turn_name(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_turn_name(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn agent_turn(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = AgentTurn { turn: 7, spent: 7, read: Some(7), body: Box::from(*b"abc") };
    let mut out = Encoder::measure();
    put_agent_turn(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_agent_turn(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "agent_turn.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_agent_turn(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_agent_turn(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn start_0(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = Start::Branch { branch: Box::from(*b"abc") };
    let mut out = Encoder::measure();
    put_start(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_start(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "start_0.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_start(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_start(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn start_1(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = Start::Commit { commit: [7; 32] };
    let mut out = Encoder::measure();
    put_start(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_start(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "start_1.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_start(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_start(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn start_2(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = Start::Saved { branch: Box::from(*b"abc") };
    let mut out = Encoder::measure();
    put_start(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_start(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "start_2.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_start(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_start(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn start_3(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = Start::Merge { branch: Box::from(*b"abc"), base: Box::from(*b"abc") };
    let mut out = Encoder::measure();
    put_start(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_start(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "start_3.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_start(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_start(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn access_0(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = Access::ReadOnly;
    let mut out = Encoder::measure();
    put_access(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_access(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "access_0.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_access(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_access(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn access_1(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = Access::Writable { push: Box::from(*b"abc"), expected: Some([7; 32]) };
    let mut out = Encoder::measure();
    put_access(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_access(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "access_1.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_access(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_access(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn link_answer_0(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = LinkAnswer::Refused { refusal: wire::AssignmentRefusal::Busy };
    let mut out = Encoder::measure();
    put_link_answer(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_link_answer(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "link_answer_0.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_link_answer(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_link_answer(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn link_answer_1(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = LinkAnswer::Ended {
        outcome: Box::from(*b"abc"),
        work: wire::Work { landed: Box::from([wire::Landed { tag: 7, commit: [7; 32] }]), saved: None },
    };
    let mut out = Encoder::measure();
    put_link_answer(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_link_answer(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "link_answer_1.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_link_answer(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_link_answer(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn link_answer_2(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = LinkAnswer::Parked {
        work: wire::Work { landed: Box::from([wire::Landed { tag: 7, commit: [7; 32] }]), saved: None },
    };
    let mut out = Encoder::measure();
    put_link_answer(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_link_answer(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "link_answer_2.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_link_answer(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_link_answer(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn link_answer_3(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = LinkAnswer::Failed {
        failure: wire::Failure::Run { failure: wire::RunFailure::Model },
        detail: Box::from(*b"abc"),
        work: wire::Work { landed: Box::from([wire::Landed { tag: 7, commit: [7; 32] }]), saved: None },
    };
    let mut out = Encoder::measure();
    put_link_answer(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_link_answer(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "link_answer_3.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_link_answer(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_link_answer(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn finish_0(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = Finish::Ended { outcome: Box::from(*b"abc") };
    let mut out = Encoder::measure();
    put_finish(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_finish(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "finish_0.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_finish(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_finish(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn finish_1(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = Finish::Parked;
    let mut out = Encoder::measure();
    put_finish(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_finish(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "finish_1.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_finish(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_finish(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn finish_2(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = Finish::Failed { failure: wire::RunFailure::Model };
    let mut out = Encoder::measure();
    put_finish(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_finish(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "finish_2.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_finish(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_finish(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn ask_0(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = Ask::Push { title: Box::from(*b"abc"), body: Box::from(*b"abc") };
    let mut out = Encoder::measure();
    put_ask(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_ask(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "ask_0.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_ask(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_ask(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn ask_1(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = Ask::Relay { body: Box::from(*b"abc") };
    let mut out = Encoder::measure();
    put_ask(&mut out, &value, &sizes).unwrap();
    let mut out = Encoder::writing(out.length());
    put_ask(&mut out, &value, &sizes).unwrap();
    let golden = golden::bytes("wire", "ask_1.bin", run, out.finish().as_ref());
    let golden = golden.as_ref();
    let mut input = Reader::new(golden);
    assert_eq!(get_ask(&mut input, &sizes).unwrap(), value);
    assert!(input.is_empty());
    assert!(get_ask(&mut Reader::new(golden.get(..golden.len().saturating_sub(1)).unwrap()), &sizes).is_none());
}

fn frame_hello_v2(run: &mut golden::Run) {
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
    let golden =
        golden::bytes("wire", "frame_hello_v2.bin", run, codec::encode_version(&value, &sizes, 2).unwrap().as_ref());
    let golden = golden.as_ref();
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

fn frame_answer_v2(run: &mut golden::Run) {
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
    let golden =
        golden::bytes("wire", "frame_answer_v2.bin", run, codec::encode_version(&value, &sizes, 2).unwrap().as_ref());
    let golden = golden.as_ref();
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

fn frame_assign_v2(run: &mut golden::Run) {
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
    let golden =
        golden::bytes("wire", "frame_assign_v2.bin", run, codec::encode_version(&value, &sizes, 2).unwrap().as_ref());
    let golden = golden.as_ref();
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

fn frame_agent_call_v2(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = wire::Message::AgentCallV2 {
        call: AgentCall { call: 7, ask: Ask::Push { title: Box::from(*b"abc"), body: Box::from(*b"abc") } },
    };
    let golden = golden::bytes(
        "wire",
        "frame_agent_call_v2.bin",
        run,
        codec::encode_version(&value, &sizes, 2).unwrap().as_ref(),
    );
    let golden = golden.as_ref();
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

fn frame_finish_v2(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = wire::Message::FinishV2 {
        finish: Finished { turns: 7, spent: 7, finish: Finish::Ended { outcome: Box::from(*b"abc") } },
    };
    let golden =
        golden::bytes("wire", "frame_finish_v2.bin", run, codec::encode_version(&value, &sizes, 2).unwrap().as_ref());
    let golden = golden.as_ref();
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

fn frame_agent_start_v2(run: &mut golden::Run) {
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
    let golden = golden::bytes(
        "wire",
        "frame_agent_start_v2.bin",
        run,
        codec::encode_version(&value, &sizes, 2).unwrap().as_ref(),
    );
    let golden = golden.as_ref();
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

fn frame_turn(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Turn {
        turn: Turn { run: 7, attempt: 7, turn: 7, spent: 7, read: Some(7), body: Box::from(*b"abc") },
    };
    let golden =
        golden::bytes("wire", "frame_turn.bin", run, codec::encode_version(&value, &sizes, 2).unwrap().as_ref());
    let golden = golden.as_ref();
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

fn frame_acknowledge_turn(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = wire::Message::AcknowledgeTurn { turn: TurnName { run: 7, attempt: 7, turn: 7 } };
    let golden = golden::bytes(
        "wire",
        "frame_acknowledge_turn.bin",
        run,
        codec::encode_version(&value, &sizes, 2).unwrap().as_ref(),
    );
    let golden = golden.as_ref();
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

fn frame_turn_busy(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value = wire::Message::TurnBusy { turn: TurnName { run: 7, attempt: 7, turn: 7 } };
    let golden =
        golden::bytes("wire", "frame_turn_busy.bin", run, codec::encode_version(&value, &sizes, 2).unwrap().as_ref());
    let golden = golden.as_ref();
    assert_eq!(codec::decode_version(golden, &sizes, 2).unwrap(), value);
    assert!(codec::encode(&value, &sizes).is_none());
    assert!(codec::decode_version(golden.get(..golden.len().saturating_sub(1)).unwrap(), &sizes, 2).is_none());
}

fn frame_agent_turn(run: &mut golden::Run) {
    let sizes = Sizes::STARTING;
    let value =
        wire::Message::AgentTurn { turn: AgentTurn { turn: 7, spent: 7, read: Some(7), body: Box::from(*b"abc") } };
    let golden =
        golden::bytes("wire", "frame_agent_turn.bin", run, codec::encode_version(&value, &sizes, 2).unwrap().as_ref());
    let golden = golden.as_ref();
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
