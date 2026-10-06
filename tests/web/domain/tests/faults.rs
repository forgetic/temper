use skein_lib::Duration;
use temper_fake_person::{Find, Person, Step};
use temper_web_domain_world::{Scenario, Settings, World};
use temper_web_view::Role;

fn submit(world: &mut World) {
    world.advance(Duration::from_millis(20));
    let mut person = Person::new(Box::from([
        Step::Type {
            find: Find::named(Role::TextBox, b"Start a new chat"),
            words: Box::from(b"Remember this".as_slice()),
        },
        Step::Press { find: Find::named(Role::Button, b"Start chat") },
    ]));
    world.run(&mut person);
}

#[test]
fn restart_before_durable_resends_same_key() {
    let mut world = World::new(Settings::calm(201), Scenario::default());
    submit(&mut world);
    world.restart(Duration::from_millis(30));
    world.advance(Duration::from_millis(150));
    assert_eq!(world.engine.creations, 1);
    assert_eq!(world.engine.durable.len(), 1);
}

#[test]
fn restart_after_durable_answers_by_key() {
    let mut world = World::new(Settings::calm(202), Scenario::default());
    submit(&mut world);
    world.advance(Duration::from_millis(5));
    assert_eq!(world.engine.creations, 1);
    world.restart(Duration::from_millis(30));
    world.advance(Duration::from_millis(150));
    assert_eq!(world.engine.creations, 1);
}

#[test]
fn reload_in_flight_and_double_press_make_one_chat() {
    let mut settings = Settings::calm(203);
    settings.double_per_mille = 1000;
    let mut world = World::new(settings, Scenario::default());
    submit(&mut world);
    world.reload();
    world.advance(Duration::from_millis(100));
    assert_eq!(world.engine.creations, 1);
    assert_eq!(world.engine.durable.len(), 1);
}

#[test]
fn dropped_and_missed_watch_recover() {
    let mut world = World::new(Settings::calm(204), Scenario::default());
    world.advance(Duration::from_millis(20));
    world.drop_watch();
    world.advance(Duration::from_millis(100));
    world.miss_watch();
    world.advance(Duration::from_millis(100));
    assert_eq!(world.stats.faults.get(&temper_web_domain_world::Fault::Dropped), Some(&1));
    assert_eq!(world.stats.faults.get(&temper_web_domain_world::Fault::Missed), Some(&1));
}

#[test]
fn signed_out_mid_request_resumes_after_sign_in() {
    let mut world = World::new(Settings::calm(205), Scenario::default());
    submit(&mut world);
    world.expire_signin();
    let mut person = Person::new(Box::from([
        Step::See { find: Find::named(Role::Button, b"Sign in"), within: Duration::from_secs(1) },
        Step::Press { find: Find::named(Role::Button, b"Sign in") },
    ]));
    world.run(&mut person);
    world.advance(Duration::from_millis(100));
    assert_eq!(world.engine.creations, 1);
}

#[test]
fn busy_answer_retries_with_the_same_key() {
    let mut world = World::new(Settings::calm(206), Scenario::default());
    world.busy_once();
    submit(&mut world);
    world.advance(Duration::from_millis(150));
    assert_eq!(world.engine.creations, 1);
    assert_eq!(world.engine.durable.len(), 1);
    assert_eq!(world.stats.faults.get(&temper_web_domain_world::Fault::Busy), Some(&1));
}

#[test]
fn refused_chat_shows_a_reason() {
    let mut world = World::new(Settings::calm(207), Scenario::default());
    world.advance(Duration::from_millis(20));
    world.engine.projects.clear();
    submit(&mut world);
    world.advance(Duration::from_millis(30));
    assert_eq!(world.engine.creations, 0);
    let mut person = Person::new(Box::from([Step::See {
        find: temper_fake_person::Find {
            within: Box::new([]),
            target: temper_fake_person::Target::Text { text: Box::from(b"cannot".as_slice()) },
        },
        within: Duration::from_secs(1),
    }]));
    world.run(&mut person);
}
