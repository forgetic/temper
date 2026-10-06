use skein_lib::Duration;
use temper_fake_person::{Find, Person, Step, Target};
use temper_web_domain::Address;
use temper_web_domain_world::{Scenario, Settings, World};
use temper_web_view::Role;

fn chat_steps(words: &[u8]) -> Person {
    Person::new(Box::from([
        Step::See {
            find: Find {
                within: Box::from([Target::Role {
                    role: Role::Status,
                    name: Box::from(b"Connection status".as_slice()),
                }]),
                target: Target::Text { text: Box::from(b"Live".as_slice()) },
            },
            within: Duration::from_secs(1),
        },
        Step::See { find: Find::named(Role::TextBox, b"Start a new chat"), within: Duration::from_secs(1) },
        Step::Type { find: Find::named(Role::TextBox, b"Start a new chat"), words: Box::from(words) },
        Step::Press { find: Find::named(Role::Button, b"Start chat") },
        Step::See {
            find: Find {
                within: Box::from([Target::Role { role: Role::Log, name: Box::from(b"Conversation".as_slice()) }]),
                target: Target::Text { text: Box::from(words) },
            },
            within: Duration::from_secs(1),
        },
    ]))
}

#[test]
fn signs_in_then_starts_one_chat() {
    let scenario = Scenario { signed_in: false, ..Scenario::default() };
    let mut world = World::new(Settings::calm(101), scenario);
    world.advance(Duration::from_millis(20));
    let mut sign_in = Person::new(Box::from([
        Step::See { find: Find::named(Role::Button, b"Sign in"), within: Duration::from_secs(1) },
        Step::Press { find: Find::named(Role::Button, b"Sign in") },
        Step::See { find: Find::named(Role::TextBox, b"Start a new chat"), within: Duration::from_secs(1) },
    ]));
    world.run(&mut sign_in);
    world.run(&mut chat_steps(b"Fix login"));
    assert_eq!(world.engine.creations, 1);
    assert!(matches!(world.tab.address, Address::Task { number: 1, .. }));
}

#[test]
fn composer_survives_reload_without_submission() {
    let mut world = World::new(Settings::calm(102), Scenario::default());
    world.advance(Duration::from_millis(20));
    let mut person = Person::new(Box::from([
        Step::Type { find: Find::named(Role::TextBox, b"Start a new chat"), words: Box::from(b"A draft".as_slice()) },
        Step::Reload,
        Step::See { find: Find::named(Role::TextBox, b"Start a new chat"), within: Duration::from_secs(1) },
    ]));
    world.run(&mut person);
    let field = world
        .tab
        .view
        .tree()
        .nodes()
        .iter()
        .find(|node| node.name.as_deref() == Some(b"Start a new chat".as_slice()))
        .expect("composer");
    assert_eq!(field.value.as_ref().expect("value").text.as_ref(), b"A draft");
    assert_eq!(world.engine.creations, 0);
}

#[test]
fn seed_replays_the_same_boundary_trace() {
    let mut first = World::new(Settings::calm(103), Scenario::default());
    let mut second = World::new(Settings::calm(103), Scenario::default());
    for world in [&mut first, &mut second] {
        world.advance(Duration::from_millis(20));
        world.run(&mut chat_steps(b"Replay"));
        world.settle();
    }
    assert_eq!(first.trace(), second.trace());
}
