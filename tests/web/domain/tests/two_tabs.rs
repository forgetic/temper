use skein_lib::{Duration, Token, Wall};
use temper_fake_person::{Doing, Find, Next, Person, Step, TreeFace, dom_event};
use temper_web_domain::{
    Address, Answer, Ask, Change, Event, ObjectKey, Query, ReadResult, Request, Snapshot, StreamEnd, StreamEvent,
    Watch, Why,
};
use temper_web_domain_world::{Scenario, Settings, World, tab::Tab};
use temper_web_view::Role;

fn route(tab: &mut Tab, world: &mut World, requests: Vec<Request>, change: Option<(Token, bool)>) {
    let mut queue = std::collections::VecDeque::from(requests);
    while let Some(request) = queue.pop_front() {
        let event = match request {
            Request::Save { saved } => {
                tab.saved = Some(saved);
                continue;
            }
            Request::Address { address, push } => {
                tab.address(address, push);
                continue;
            }
            Request::SignIn { then } => {
                world.engine.signed_in = true;
                tab.address(then, false);
                continue;
            }
            Request::Open { stream, watch } => {
                queue.extend(tab.step(&world.settings, world.now, Event::Opened { stream }));
                let snapshot = match watch {
                    Watch::Person => Some(Snapshot::Person(world.engine.snapshot())),
                    Watch::Task { number } => world.engine.task_snapshot(number).map(Snapshot::Task),
                };
                if let Some(snapshot) = snapshot {
                    Event::Streamed { stream, event: StreamEvent::Snapshot(snapshot) }
                } else {
                    Event::Ended { stream, end: StreamEnd::Gone }
                }
            }
            Request::Close { stream } => Event::Ended { stream, end: StreamEnd::Closed },
            Request::Read { read, query } => {
                let result = match query {
                    Query::Chats { project, .. } => world.engine.read_chats(project, world.settings.domain.window),
                    Query::Escalation { task } => {
                        ReadResult::Escalation(world.engine.task_snapshot(task).and_then(|task| task.escalation))
                    }
                    Query::Result { task } => {
                        ReadResult::Result(world.engine.task_snapshot(task).and_then(|task| task.result))
                    }
                };
                Event::Read { read, result }
            }
            Request::Send { request, key, ask } => {
                let prior = if let Ask::Decide { waiting: temper_web_domain::Waiting::Escalation { task }, .. } = &ask {
                    world
                        .engine
                        .decisions
                        .get(task)
                        .map(|(by, choice, at)| (*task, Why::Decided { by: by.clone(), choice: *choice, at: *at }))
                } else {
                    None
                };
                let answer: Answer = world.engine.commit(key, ask, Wall::from_nanos(world.now.as_nanos()));
                if let (Some((stream, true)), Some((task, why))) = (change, prior.as_ref()) {
                    queue.extend(tab.step(
                        &world.settings,
                        world.now,
                        Event::Streamed {
                            stream,
                            event: StreamEvent::Change(Change::Left {
                                key: ObjectKey::Escalation { task: *task },
                                why: why.clone(),
                            }),
                        },
                    ));
                }
                if let (Some((stream, false)), Some((task, why))) = (change, prior) {
                    queue.extend(tab.step(&world.settings, world.now, Event::Answered { request, answer }));
                    Event::Streamed {
                        stream,
                        event: StreamEvent::Change(Change::Left { key: ObjectKey::Escalation { task }, why }),
                    }
                } else {
                    Event::Answered { request, answer }
                }
            }
        };
        queue.extend(tab.step(&world.settings, world.now, event));
    }
}

fn run(tab: &mut Tab, world: &mut World, script: Box<[Step]>, change: Option<(Token, bool)>) {
    let mut person = Person::new(script);
    for _ in 0..100 {
        let next = person.next(&mut TreeFace::new(tab.view.tree()), world.now);
        match next {
            Next::Done => return,
            Next::Failed { step, why } => panic!("second tab step {step}: {why:?}"),
            Next::Wait { .. } => world.now = world.now.saturating_add(Duration::from_millis(1)),
            Next::Do(Doing::Go { address }) => {
                let requests = tab.go(&world.settings, world.now, address);
                route(tab, world, requests, change);
            }
            Next::Do(action @ (Doing::Press { .. } | Doing::Type { .. } | Doing::Send { .. })) => {
                let requests = tab.event(&world.settings, world.now, dom_event(action).expect("DOM action"));
                route(tab, world, requests, change);
            }
            Next::Do(Doing::Reload) => {
                let requests = tab.reload(&world.settings, 847, world.now);
                route(tab, world, requests, change);
            }
        }
    }
    panic!("second tab did not finish");
}

fn two_tab_story(change_before_answer: bool) {
    let mut world = World::new(Settings::calm(301), Scenario::held(27, b"Review memory"));
    let mut second = Tab::new(&world.settings, 302, Address::Task { number: 27, section: None });
    let requests = second.start(&world.settings, world.now);
    let task_watch = requests
        .iter()
        .find_map(|request| match request {
            Request::Open { stream, watch: Watch::Task { number: 27 } } => Some(*stream),
            Request::Open { .. }
            | Request::Send { .. }
            | Request::Read { .. }
            | Request::Close { .. }
            | Request::Address { .. }
            | Request::Save { .. }
            | Request::SignIn { .. } => None,
        })
        .expect("task watch");
    route(&mut second, &mut world, requests, None);
    world.run(&mut Person::new(Box::from([
        Step::Go { address: Address::Task { number: 27, section: None } },
        Step::See { find: Find::named(Role::Button, b"Release task"), within: Duration::from_secs(1) },
        Step::Press { find: Find::named(Role::Button, b"Release task") },
        Step::Press { find: Find::named(Role::Button, b"Confirm decision") },
    ])));
    world.advance(Duration::from_millis(30));
    assert_eq!(world.engine.decision_count, 1);
    run(
        &mut second,
        &mut world,
        Box::from([
            Step::Press { find: Find::named(Role::Button, b"Release task") },
            Step::Press { find: Find::named(Role::Button, b"Confirm decision") },
            Step::See {
                find: Find {
                    within: Box::new([]),
                    target: temper_fake_person::Target::Text { text: Box::from(b"Ada released at".as_slice()) },
                },
                within: Duration::from_secs(1),
            },
        ]),
        Some((task_watch, change_before_answer)),
    );
    assert_eq!(world.engine.decision_count, 1);
    assert_eq!(world.engine.durable.len(), 2);
}

#[test]
fn two_tabs_answer_before_watch_change() {
    two_tab_story(false);
}

#[test]
fn two_tabs_watch_change_before_answer() {
    two_tab_story(true);
}
