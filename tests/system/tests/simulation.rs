use skein_lib::{ReplyTo, Token};
use skein_fake_llm_domain::api::{Finish, Line, Script, Turn};
use smith_agent_world::Job;
use smith_domain_run as run;
use temper_engine_domain::{Delivery, engine};
use temper_engine_domain::{Key, Record};
use temper_engine_domain_people as people;
use temper_engine_domain_tasks as tasks;
use temper_system_world::world;

#[test]
fn a_chat_answers_through_a_smith_run_and_a_committed_root_result() {
    let mut world = world::chat(b"@report", Job::Reporting);
    world.run();
    let result = world.root.store.rows.get(&Key::Tasks(tasks::Key::Ended(world.assignment.task)));
    assert!(matches!(world.agent.answer(), run::Answer::Accepted { outcome: run::outcome::Declared::Report(_), .. }), "{:?}", world.agent.answer());
    assert!(matches!(result, Some(Record::Tasks(tasks::Stored::Ended(row)))
        if matches!(row.phase, tasks::Phase::Ended(tasks::Ending::Done(tasks::TaskResult::Report { .. })))));
}

#[test]
fn a_chat_parks_and_resumes_from_its_concrete_smith_transcript() {
    let mut world = world::chat(b"@chat", Job::Waiting);
    world.run();
    assert!(matches!(world.agent.answer(), run::Answer::Parked { turns: 3, .. }), "{:?}", world.agent.answer());
    let prior = world.agent.turns().len();
    world.say_and_resume(b"continue", 42, Job::Waiting);
    assert_eq!(world.assignment.transcript.len(), prior);
    world.run();
    assert!(world.agent.turns().first().expect("resumed turn").sequence > 1);
}

#[test]
fn a_person_stops_a_smith_run_and_releases_the_chat() {
    let mut world = world::chat(b"@waiting", Job::Waiting);
    let task = world.assignment.task;
    let attempt = world.assignment.attempt;
    world.root.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(980)),
        sign_in: world.root.session(),
        key: [80; 16],
        ask: people::Ask::Stop { project: 1, task },
    });
    world.root.settle();
    assert!(world.root.delivered.iter().any(|item| matches!(item,
        Delivery::Cancel { task: cancelled, attempt: current, .. }
            if *cancelled == task && *current == attempt)));
    world.agent = world::cancelled_agent_for(&world.assignment, Job::Waiting);
    world.run();
    assert!(matches!(world.agent.answer(), run::Answer::Failed { failure: run::Failure::Cancelled, .. }));
    world.root.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(981)),
        sign_in: world.root.session(),
        key: [81; 16],
        ask: people::Ask::Release { project: 1, task },
    });
    world.root.settle();
    assert!(world.root.delivered.iter().any(|item| matches!(item,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::Released { task: released }), .. }
            if *released == task)));
}

#[test]
fn a_smith_host_call_uses_the_roots_committed_answer() {
    let mut world = world::chat(b"@bridge", Job::Reporting);
    world.agent = world::scripted_agent_for(
        &world.assignment,
        Script {
            cue: b"@bridge".as_slice().into(),
            turns: Box::new([
                Turn {
                    lines: Box::new([Line::Call {
                        name: b"message".as_slice().into(),
                        arguments: br#"{"target":1,"form":"words","words":"check the plan"}"#.as_slice().into(),
                    }]),
                    finish: Finish::ToolCalls,
                    tokens: 20,
                },
                Turn {
                    lines: Box::new([Line::Call {
                        name: b"finish".as_slice().into(),
                        arguments: br#"{"report":"The call was answered."}"#.as_slice().into(),
                    }]),
                    finish: Finish::ToolCalls,
                    tokens: 20,
                },
            ]),
        },
    );
    world.run();
    assert_eq!(world.agent.host_submissions().len(), 1);
    assert!(world.root.delivered.iter().any(|delivery| matches!(delivery, Delivery::CallAnswer { .. })));
    assert_eq!(world.agent.host_terminals().len(), 1);
}
