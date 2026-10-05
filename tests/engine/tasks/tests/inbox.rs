use skein_lib::{Duration, Wall};
use temper_engine_domain_tasks::{
    self as tasks, Accepted, Active, End, Event, Hold, Interest, Key, Message, NewsClass, Party, Phase, Refusal, Rule,
    Stored, Subscription, SubscriptionKind, UserMessage,
};
use temper_engine_tasks_world::{LIMITS, Reply, World, task};
fn words(value: u8, count: usize) -> UserMessage {
    UserMessage::Words { words: vec![value; count].into_boxed_slice() }
}
fn refused(reply: &Reply, why: Refusal) {
    assert!(matches!(reply, Reply::Refused(problem) if problem.why == why), "{reply:?}, wanted {why:?}");
}
fn subscribe(w: &mut World, number: u64, task: u64, kind: SubscriptionKind) {
    let reply_to = w.to();
    w.send(Event::Subscribe { reply_to, subscription: Subscription { number, task, kind, pending: false } });
}
#[test]
fn whole_oldest_inbox_and_turns_have_independent_durable_cuts() {
    let mut w = World::new(41, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[])]);
    assert_eq!(w.mail(1, Party::Person(1), words(1, 40)), Reply::Sent(Accepted::New));
    let second = w.number();
    let reply_to = w.to();
    w.stage(Event::Send { reply_to, number: second, task: 1, from: Party::Person(1), message: words(2, 40) });
    assert_eq!(w.messages(1).len(), 1, "uncommitted mail invisible");
    w.restart();
    assert_eq!(w.messages(1).len(), 1);
    w.mail_number(second, 1, Party::Person(1), words(2, 40));
    let reply_to = w.to();
    w.send(Event::Peek { reply_to, task: 1, bytes: 60 });
    assert!(
        w.replies
            .values()
            .any(|reply| matches!(reply, Reply::Inbox(rows, true) if rows.len() == 1 && rows[0].number == 1))
    );
    w.claim(1, 1);
    let reply_to = w.to();
    w.stage(Event::Turn { reply_to, task: 1, attempt: 1, turn: 1, read: Some(second) });
    assert_eq!(w.messages(1).len(), 2, "take waits for turn commit");
    w.restart();
    assert_eq!(w.messages(1).len(), 2);
    let reply_to = w.to();
    w.stage(Event::Turn { reply_to, task: 1, attempt: 1, turn: 1, read: Some(second) });
    w.durable();
    assert!(w.messages(1).is_empty());
    w.restart();
    assert_eq!(w.record(1).turn, 1);
    assert_eq!(w.turn(1, 1, Some(second)), Reply::Turn(Accepted::Already));
    refused(&w.turn(1, 3, None), Refusal::Turn);
    refused(&w.turn(1, 2, Some(999)), Refusal::Read);
    assert_eq!(w.mail_number(second, 1, Party::Person(1), words(2, 40)), Reply::Sent(Accepted::Already));
    refused(&w.mail_number(second, 1, Party::Person(1), words(3, 40)), Refusal::KeyConflict);
    w.terminal(1, End::Parked);
    w.mail(1, Party::Person(1), words(4, 1));
    assert_eq!(w.record(1).phase, Phase::Active(Active::Due));
}
#[test]
fn promised_results_answers_and_cancel_survive_pressure_and_restart() {
    let l = tasks::Limits { inbox_messages: 4, inbox_bytes: 256, receipts: 4, ..LIMITS };
    let mut w = World::new(42, l);
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.claim(1, 1);
    w.make(Party::Task(1), vec![task(2, &[])]);
    w.claim(2, 2);
    w.mail(1, Party::Person(1), words(1, 64));
    w.mail(1, Party::Person(1), words(2, 64));
    w.mail(1, Party::Person(1), words(3, 64));
    refused(&w.mail(1, Party::Person(1), words(4, 1)), Refusal::Inbox);
    let original = w.records.clone();
    refused(&w.make(Party::Task(1), vec![task(3, &[])]), Refusal::Inbox);
    assert_eq!(w.records, original);
    w.finish(2);
    w.observe(temper_engine_tasks_world::referee::Seen::Settled { task: 2 });
    w.stage(Event::Settled { task: 2 });
    assert!(!w.results.contains_key(&2));
    w.durable();
    assert!(w.messages(1).iter().any(|e| matches!(e.message, Message::Result { task: 2, .. })));
    assert!(w.record(1).results_due.is_empty());
    w.restart();
    assert_eq!(w.messages(1).len(), 4);
    w.cancel(1, b"cancel at full inbox");
    w.complete_cancel();
    assert_eq!(w.live(), 0);
    assert_eq!(w.records.keys().filter(|key| matches!(key, Key::ArchivedMessage(_))).count(), 4);
    let mut w = World::new(43, tasks::Limits { inbox_messages: 2, receipts: 3, ..LIMITS });
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.make(Party::Task(1), vec![task(2, &[])]);
    // Delegate asks its requester. The answer consumes its reserved slot and
    // receipt credit even after all ordinary capacity is occupied.
    let question = w.number();
    w.mail_number(question, 1, Party::Task(2), UserMessage::Question { words: Box::new([1]) });
    w.mail(2, Party::Person(1), words(1, 64));
    refused(&w.mail(2, Party::Person(1), words(2, 1)), Refusal::Busy);
    w.restart();
    assert_eq!(
        w.mail(2, Party::Task(1), UserMessage::Answer { question, words: vec![2; 64].into_boxed_slice() }),
        Reply::Sent(Accepted::New)
    );
    assert_eq!(w.messages(2).len(), 2);
}
#[test]
fn introduced_dependency_is_legal_but_combined_existing_wait_cycle_is_atomic() {
    let mut w = World::new(44, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.make(Party::Task(1), vec![task(2, &[]), task(3, &[])]);
    refused(&w.mail(2, Party::Task(3), words(1, 1)), Refusal::Reference);
    let reply_to = w.to();
    w.send(Event::Introduce { reply_to, by: Party::Task(1), left: 2, right: 3 });
    assert_eq!(w.mail(2, Party::Task(3), words(1, 1)), Reply::Sent(Accepted::New));
    assert_eq!(w.make(Party::Task(2), vec![task(4, &[3])]), Reply::Made(vec![4]));
    assert_eq!(w.record(4).phase, Phase::Waiting);
    w.restart();
    // 3 -> new5 -> 2 -> 4 -> 3, including delegation waits.
    let before = w.records.clone();
    refused(&w.make(Party::Task(3), vec![task(5, &[2])]), Refusal::Cycle);
    assert_eq!(w.records, before);
    let reply_to = w.to();
    w.send(Event::Introduce { reply_to, by: Party::Task(2), left: 1, right: 3 });
    let reply_to = w.to();
    w.send(Event::Introduce { reply_to, by: Party::Task(1), left: 2, right: 3 });
    w.claim(3, 3);
    w.finish(3);
    w.settle(3);
    w.claim(4, 4);
    assert!(w.records.contains_key(&Key::Stub(3)), "ended introduced target retained");
    let mut duplicate_inputs = task(5, &[]);
    duplicate_inputs.spec.inputs = Box::new([3, 3]);
    let before = w.records.clone();
    refused(&w.make(Party::Task(2), vec![duplicate_inputs]), Refusal::Spec);
    assert_eq!(w.records, before);
    let reply_to = w.to();
    w.send(Event::ForgetStub { reply_to, task: 3 });
    assert!(matches!(w.replies.values().last(), Some(Reply::Refused(problem)) if problem.why == Refusal::Busy));
    w.cancel(1, b"end");
    w.complete_cancel();
}
#[test]
fn batching_age_person_words_lowered_news_and_merge_offer_are_distinct() {
    let mut w = World::new(45, LIMITS);
    let mut new = task(1, &[]);
    new.policy.news = Rule::Batch { count: 3, age: Duration::from_secs(5) };
    new.policy.words = Rule::Never;
    w.make(Party::Person(1), vec![new]);
    w.claim(1, 1);
    w.terminal(1, End::Parked);
    subscribe(&mut w, 1, 1, SubscriptionKind::Topic { connector: 1, topic: 2 });
    for _ in 0..2 {
        let number = w.number();
        let reply_to = w.to();
        w.send(Event::News { reply_to, number, subscription: 1, class: NewsClass::Wakes, words: Box::new([1]) });
    }
    assert_eq!(w.record(1).phase, Phase::Active(Active::Idle));
    assert_eq!(w.messages(1).len(), 1);
    assert_eq!(w.messages(1)[0].hits, 2);
    w.advance();
    assert_eq!(w.record(1).phase, Phase::Active(Active::Due));
    w.claim(1, 2);
    let old = w.messages(1)[0].number;
    let number = w.number();
    let reply_to = w.to();
    w.send(Event::News { reply_to, number, subscription: 1, class: NewsClass::Wakes, words: Box::new([2]) });
    assert_eq!(w.turn(1, 1, Some(old)), Reply::Turn(Accepted::New));
    assert_eq!(w.messages(1)[0].number, number, "reading old offer cannot take replacement");
    w.restart();
    assert_eq!(w.turn(1, 2, Some(number)), Reply::Turn(Accepted::New));
    w.terminal(1, End::Parked);
    let number = w.number();
    let reply_to = w.to();
    w.send(Event::News { reply_to, number, subscription: 1, class: NewsClass::Kept, words: Box::new([3]) });
    assert_eq!(w.record(1).phase, Phase::Active(Active::Idle));
    let number = w.number();
    let reply_to = w.to();
    w.send(Event::News { reply_to, number, subscription: 1, class: NewsClass::Dropped, words: Box::new([4]) });
    assert_eq!(w.messages(1).len(), 1);
    w.mail(1, Party::Person(1), words(5, 1));
    assert_eq!(w.record(1).phase, Phase::Active(Active::Due));
    let mut corrected = World::new(55, LIMITS);
    corrected.env.wall = Wall::from_nanos(Duration::from_secs(10).as_nanos());
    let mut new = task(1, &[]);
    new.policy.news = Rule::Batch { count: 3, age: Duration::from_secs(5) };
    corrected.make(Party::Person(1), vec![new]);
    corrected.claim(1, 1);
    corrected.terminal(1, End::Parked);
    subscribe(&mut corrected, 1, 1, SubscriptionKind::Topic { connector: 1, topic: 1 });
    let number = corrected.number();
    let reply_to = corrected.to();
    corrected.send(Event::News { reply_to, number, subscription: 1, class: NewsClass::Wakes, words: Box::new([1]) });
    corrected.send(Event::Hold { task: 1, why: Hold::Stopped });
    corrected.env.wall = Wall::EPOCH;
    corrected.advance();
    assert!(corrected.messages(1)[0].eligible, "original monotonic age expires despite wall correction");
    corrected.restart();
    let reply_to = corrected.to();
    corrected.send(Event::Release { reply_to, task: 1 });
    assert_eq!(corrected.record(1).phase, Phase::Active(Active::Due), "held eligibility survives restore and release");
}
#[test]
fn task_notices_periodic_timers_and_held_inboxes_restore() {
    let mut w = World::new(46, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.make(Party::Task(1), vec![task(2, &[])]);
    w.claim(1, 1);
    w.terminal(1, End::Parked);
    subscribe(&mut w, 1, 1, SubscriptionKind::Task { target: 2, interest: Interest::StateAndResult });
    let reply_to = w.to();
    w.send(Event::Hold { task: 2, why: Hold::Stopped });
    let _ = reply_to;
    assert!(
        w.messages(1).iter().any(|message| matches!(
            message.message,
            Message::Notice { notice: tasks::Notice::Held(Hold::Stopped), .. }
        ))
    );
    w.claim(1, 2);
    let read = w.messages(1).last().expect("inbox nonempty").number;
    w.turn(1, 1, Some(read));
    w.terminal(1, End::Parked);
    subscribe(
        &mut w,
        2,
        1,
        SubscriptionKind::Timer {
            at: Wall::from_nanos(Duration::from_secs(1).as_nanos()),
            period: Some(Duration::from_secs(1)),
        },
    );
    w.send(Event::Hold { task: 1, why: Hold::Stopped });
    w.advance();
    w.advance();
    assert!(matches!(w.record(1).phase, Phase::Held { .. }));
    assert_eq!(w.messages(1).iter().filter(|message| matches!(message.message, Message::Timer { .. })).count(), 1);
    w.restart();
    let reply_to = w.to();
    w.send(Event::Release { reply_to, task: 1 });
    assert_eq!(w.record(1).phase, Phase::Active(Active::Due));
    let reply_to = w.to();
    w.send(Event::Release { reply_to, task: 2 });
    w.claim(2, 3);
    w.finish(2);
    w.settle(2);
    assert!(
        w.messages(1)
            .iter()
            .any(|message| matches!(message.message, Message::Notice { notice: tasks::Notice::Ended(_), .. }))
    );
    w.cancel(1, b"end");
    w.complete_cancel();
    assert!(w.topics.is_empty());
    assert!(!w.records.keys().any(|key| matches!(key, Key::Subscription(_))));
}
#[test]
fn deferred_old_message_is_not_consumed_by_a_newer_live_relay() {
    let mut w = World::new(47, LIMITS);
    let mut root = task(1, &[]);
    root.policy.words = Rule::Never;
    w.make(Party::Person(1), vec![root]);
    w.make(Party::Task(1), vec![task(2, &[])]);
    w.claim(1, 1);
    w.mail(1, Party::Task(2), words(1, 1));
    let old = w.messages(1)[0].number;
    w.mail(1, Party::Person(1), words(2, 1));
    let new = w.messages(1).last().expect("inbox nonempty").number;
    assert!(!w.relays.contains_key(&(1, 1, old)));
    assert!(w.relays.contains_key(&(1, 1, new)));
    w.turn(1, 1, Some(new));
    assert_eq!(w.messages(1)[0].number, old);
    w.terminal(1, End::Parked);
    w.mail(1, Party::Person(1), words(3, 1));
    w.claim(1, 2);
    assert_eq!(w.turn(1, 1, Some(old)), Reply::Turn(Accepted::New));
    assert_eq!(w.messages(1).len(), 1);
    assert!(matches!(w.records.get(&Key::Live(1)), Some(Stored::Live(record)) if record.turn == 1));
}
#[test]
fn reference_admission_post_end_subscriptions_and_hint_growth_keep_their_bounds() {
    let mut w = World::new(48, tasks::Limits { references: 0, ..LIMITS });
    w.make(Party::Person(1), vec![task(1, &[]), task(2, &[])]);
    let before = w.records.clone();
    let reply_to = w.to();
    w.send(Event::Introduce { reply_to, by: Party::Person(1), left: 1, right: 2 });
    assert_eq!(w.records, before, "both reference grants refused atomically");
    refused(&w.make(Party::Task(1), vec![task(3, &[])]), Refusal::Busy);
    let mut pressure = World::new(51, tasks::Limits { references: 1, ..LIMITS });
    pressure.make(Party::Person(1), vec![task(1, &[])]);
    pressure.make(Party::Task(1), vec![task(2, &[])]);
    let before = pressure.records.clone();
    refused(&pressure.make(Party::Task(1), vec![task(3, &[])]), Refusal::Busy);
    assert_eq!(pressure.records, before);
    let reply_to = pressure.to();
    pressure.send(Event::ForgetReference { reply_to, task: 1, target: 2 });
    assert!(pressure.record(1).references.contains(&2), "result credit prevents early forget");
    pressure.claim(2, 2);
    pressure.finish(2);
    pressure.settle(2);
    let reply_to = pressure.to();
    pressure.send(Event::ForgetReference { reply_to, task: 1, target: 2 });
    assert!(!pressure.record(1).references.contains(&2));
    assert_eq!(
        pressure.make(Party::Task(1), vec![task(3, &[])]),
        Reply::Made(vec![3]),
        "pressure refusal is retryable after forget"
    );
    let mut w = World::new(49, tasks::Limits { inbox_messages: 3, inbox_bytes: 129, ..LIMITS });
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.make(Party::Task(1), vec![task(2, &[])]);
    w.claim(2, 2);
    w.finish(2);
    w.settle(2);
    subscribe(&mut w, 1, 1, SubscriptionKind::Task { target: 2, interest: Interest::Result });
    assert!(
        w.messages(1).iter().any(|message| matches!(
            message.message,
            Message::Notice { target: 2, notice: tasks::Notice::Ended(_), .. }
        )),
        "root reads committed ended result on subscription"
    );
    let reply_to = w.to();
    w.send(Event::ForgetReference { reply_to, task: 1, target: 2 });
    assert!(w.record(1).references.contains(&2), "live subscription retains visibility");
    w.restart();
    let mut w = World::new(50, tasks::Limits { inbox_bytes: 128, ..LIMITS });
    let mut new = task(1, &[]);
    new.policy.news_ceiling = NewsClass::Kept;
    w.make(Party::Person(1), vec![new]);
    w.claim(1, 1);
    w.terminal(1, End::Parked);
    subscribe(&mut w, 1, 1, SubscriptionKind::Topic { connector: 1, topic: 1 });
    let number = w.number();
    let reply_to = w.to();
    w.send(Event::News { reply_to, number, subscription: 1, class: NewsClass::Wakes, words: Box::new([1]) });
    assert_eq!(w.record(1).phase, Phase::Active(Active::Idle), "policy only lowers news class");
    w.mail(1, Party::Person(1), words(2, 64));
    refused(&w.mail(1, Party::Person(1), words(3, 1)), Refusal::Inbox);
    let number = w.number();
    let reply_to = w.to();
    w.send(Event::News {
        reply_to,
        number,
        subscription: 1,
        class: NewsClass::Wakes,
        words: vec![4; 64].into_boxed_slice(),
    });
    assert_eq!(
        w.messages(1)
            .iter()
            .map(|message| match &message.message {
                Message::Words { words } | Message::News { words, .. } => words.len(),
                Message::Question { .. }
                | Message::Answer { .. }
                | Message::Result { .. }
                | Message::Notice { .. }
                | Message::Timer { .. } => 0,
            })
            .sum::<usize>(),
        128
    );
    w.restart();
}
#[test]
fn refused_message_candidates_and_retired_receipts_are_retryable() {
    let mut w = World::new(52, tasks::Limits { inbox_messages: 1, receipts: 1, ..LIMITS });
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.claim(1, 1);
    assert_eq!(w.mail_number(1, 1, Party::Person(1), words(1, 1)), Reply::Sent(Accepted::New));
    refused(&w.mail_number(2, 1, Party::Person(1), words(2, 1)), Refusal::Busy);
    w.turn(1, 1, Some(1));
    let reply_to = w.to();
    w.send(Event::ForgetReceipt { reply_to, number: 1 });
    assert_eq!(w.mail_number(2, 1, Party::Person(1), words(2, 1)), Reply::Sent(Accepted::New));
    assert_eq!(w.messages(1).len(), 1);
    w.restart();
    assert_eq!(w.mail_number(2, 1, Party::Person(1), words(2, 1)), Reply::Sent(Accepted::Already));
}
#[test]
fn pressure_retry_after_intervening_delivery_uses_fresh_candidate_and_old_turn_cannot_take_it() {
    let mut w = World::new(54, tasks::Limits { inbox_messages: 2, receipts: 2, ..LIMITS });
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.claim(1, 1);
    subscribe(&mut w, 1, 1, SubscriptionKind::Topic { connector: 1, topic: 1 });
    w.mail_number(1, 1, Party::Person(1), words(1, 1));
    let reply_to = w.to();
    w.send(Event::News { reply_to, number: 2, subscription: 1, class: NewsClass::Wakes, words: Box::new([2]) });
    // Same logical call is refused at pressure. It saves no accepted receipt.
    refused(&w.mail_number(3, 1, Party::Person(1), words(3, 1)), Refusal::Inbox);
    assert!(!w.records.contains_key(&Key::Receipt(3)));
    let reply_to = w.to();
    w.send(Event::News { reply_to, number: 4, subscription: 1, class: NewsClass::Wakes, words: Box::new([4]) });
    assert_eq!(w.turn(1, 1, Some(2)), Reply::Turn(Accepted::New));
    assert_eq!(w.messages(1)[0].number, 4, "old read cannot erase replacement");
    refused(&w.mail_number(3, 1, Party::Person(1), words(3, 1)), Refusal::Busy);
    assert_eq!(
        w.mail_number(5, 1, Party::Person(1), words(3, 1)),
        Reply::Sent(Accepted::New),
        "root retries logical call with fresh candidate"
    );
    w.restart();
    assert_eq!(w.turn(1, 1, Some(2)), Reply::Turn(Accepted::Already));
    assert_eq!(w.messages(1).iter().map(|message| message.number).collect::<Vec<_>>(), vec![4, 5]);
    assert_eq!(w.mail_number(5, 1, Party::Person(1), words(3, 1)), Reply::Sent(Accepted::Already));
}
