use skein_lib::{ReplyTo, Token, Wall};
use temper_engine_domain_tasks::{Accepted, End, Event, Party, Refusal, Word};
use temper_engine_tasks_world::{LIMITS, Reply, World, task};

fn say(world: &mut World, number: u64) {
    let reply_to = world.to();
    world.send(Event::Message {
        reply_to,
        project: 1,
        task: 1,
        word: Word {
            number,
            from: Party::Person(9),
            words: Box::from([u8::try_from(number).expect("small message")]),
            at: Wall::EPOCH,
        },
    });
}

#[test]
fn a_read_fence_takes_only_offered_words() {
    let mut world = World::new(81, LIMITS);
    world.make(Party::Person(9), vec![task(1, &[])]);
    world.claim(1, 1);
    say(&mut world, 1);
    say(&mut world, 2);
    let before = world.record(1).clone();
    let call = 1001;
    world.send(Event::Turn {
        reply_to: ReplyTo::new(Token::new(call)),
        task: 1,
        attempt: 1,
        turn: 1,
        read: Some(2),
        offered: Some(1),
        cumulative: 1,
    });
    assert!(matches!(world.replies[&call], Reply::Refused(problem) if problem.why == Refusal::Read));
    assert_eq!(world.record(1), &before);
    let call = 1002;
    world.send(Event::Turn {
        reply_to: ReplyTo::new(Token::new(call)),
        task: 1,
        attempt: 1,
        turn: 1,
        read: Some(1),
        offered: Some(1),
        cumulative: 1,
    });
    assert_eq!(world.replies[&call], Reply::Turn(Accepted::New));
    assert_eq!(world.record(1).inbox.len(), 1);
    assert_eq!(world.record(1).inbox[0].number, 2);
}

#[test]
fn a_parked_chat_wakes_on_its_persons_words() {
    let mut world = World::new(82, LIMITS);
    world.make(Party::Person(9), vec![task(1, &[])]);
    world.claim(1, 1);
    assert_eq!(world.terminal(1, End::Parked), Reply::Acknowledged(Accepted::New));
    say(&mut world, 1);
    assert!(world.activations.contains(&1));
    assert_eq!(world.contexts[&1].inbox[0].number, 1);
}
