use temper_engine_domain_forge_client::{Condition, Effect, Entry, Made, Outcome, Resource, Watch, What, api};
use temper_engine_forge_client_world::{REPO, Settings, World};
fn issue(entry: u64) -> Entry {
    Entry {
        number: entry,
        task: 7,
        repository: REPO,
        effect: Effect {
            write: api::Write::CreateIssue {
                key: Box::from(&b"deployment:goal"[..]),
                title: Box::from(&b"goal"[..]),
                body: Box::from(&b"plan"[..]),
            },
            condition: Condition::None,
        },
        start: None,
        attempt: None,
        failures: 0,
    }
}
fn post(entry: u64, number: u64) -> Entry {
    Entry {
        number: entry,
        task: 7,
        repository: REPO,
        effect: Effect {
            write: api::Write::Post {
                number,
                key: Box::new([u8::try_from(entry).expect("test entry bound")]),
                body: Box::from(&b"milestone"[..]),
            },
            condition: Condition::None,
        },
        start: None,
        attempt: None,
        failures: 0,
    }
}
fn run(settings: Settings) -> temper_engine_forge_client_world::Stats {
    let mut world = World::new(settings);
    world.make(issue(1));
    world.run_for(30);
    let mut number = None;
    for (_, outcome) in world.outcomes() {
        match outcome {
            Outcome::Made { made: Made::Created(n), .. } => number = Some(*n),
            Outcome::Made { .. }
            | Outcome::Failed(_)
            | Outcome::Raced { .. }
            | Outcome::Uncertain
            | Outcome::Held
            | Outcome::Withdrawn => {}
        }
    }
    if let Some(number) = number {
        world.keep(Box::new([Watch {
            resource: Resource { repository: REPO, what: What::Issue(number) },
            participating: true,
        }]));
        for entry in 2..=4 {
            world.make(post(entry, number));
        }
        world.run_for(1);
        world.restart();
        world.run_for(30);
    }
    world.finish();
    world.stats()
}
#[test]
fn keyed_creations_and_comments_survive_faults_late_landings_and_restart() {
    for seed in 0..4 {
        let stats = run(Settings::random(seed));
        assert!(stats.creations > 0);
        assert_eq!(stats.calls, stats.terminals);
    }
}

#[test]
fn a_keyed_branch_retry_keeps_its_deadline_across_restart_and_rejects_a_late_copy() {
    let mut world = World::new(Settings::calm(41));
    let head = world.branch(b"main").expect("fixture main branch");
    world.land_writes_late(skein_lib::Duration::from_secs(20));
    world.make(Entry {
        number: 1,
        task: 7,
        repository: REPO,
        effect: Effect {
            write: api::Write::CreateBranch {
                branch: Box::from(&b"temper/7"[..]),
                commit: temper_engine_forge_client_world::translate::commit(head),
            },
            condition: Condition::None,
        },
        start: None,
        attempt: None,
        failures: 0,
    });
    world.run_for(1);
    let first = world.entry(1).expect("attempt retained").attempt.expect("write was sent");
    assert_eq!(world.stats().writes, 1);
    assert_eq!(world.branch(b"temper/7"), None);
    world.calm();
    world.restart();
    world.run_for(8);
    assert_eq!(world.entry(1).expect("deadline retained").attempt, Some(first));
    assert_eq!(world.stats().writes, 1, "restart cannot renew the first deadline or send early");
    world.run_for(5);
    assert_eq!(
        world.stats().writes,
        2,
        "retry is sent after the original deadline: {:?} {:?}",
        world.entry(1),
        world.outcomes()
    );
    assert_eq!(world.branch(b"temper/7"), Some(head));
    world.run_for(10);
    assert_eq!(world.stats().late_landings, 1);
    assert_eq!(world.branch(b"temper/7"), Some(head));
    assert!(
        world
            .outcomes()
            .iter()
            .any(|(entry, outcome)| { *entry == 1 && matches!(outcome, Outcome::Made { made: Made::Branch(_), .. }) })
    );
    world.finish();
}

#[test]
fn an_unrecoverable_creation_is_held_when_its_late_copy_is_still_missing() {
    let mut world = World::new(Settings::calm(42));
    world.land_writes_late(skein_lib::Duration::from_secs(20));
    world.make(issue(1));
    world.run_for(1);
    assert_eq!(world.stats().writes, 1);
    world.calm();
    world.restart();
    world.run_for(12);
    assert!(world.outcomes().contains(&(1, Outcome::Held)));
    assert_eq!(world.stats().writes, 1, "an unrecoverable creation is never retried");
    world.run_for(10);
    assert_eq!(world.stats().late_landings, 1);
    assert_eq!(world.stats().creations, 1, "the late copy may still make its effect");
    world.finish();
}

#[test]
fn a_conditional_merge_lands_once_when_the_old_copy_arrives_after_its_retry() {
    let mut world = World::new(Settings::calm(43));
    let pull = world.historical_pull();
    let head = world.branch(b"topic").expect("fixture pull head");
    let landing = world.branch(b"main").expect("fixture landing branch");
    world.land_writes_late(skein_lib::Duration::from_secs(20));
    world.make(Entry {
        number: 1,
        task: 7,
        repository: REPO,
        effect: Effect {
            write: api::Write::Merge { number: pull, head: temper_engine_forge_client_world::translate::commit(head) },
            condition: Condition::Merge { base: Box::from(&b"main"[..]) },
        },
        start: None,
        attempt: None,
        failures: 0,
    });
    world.run_for(1);
    assert_eq!(world.stats().writes, 1);
    world.calm();
    world.restart();
    world.run_for(8);
    assert_eq!(world.stats().writes, 1);
    world.run_for(5);
    assert_eq!(world.stats().writes, 2);
    let merged = world.branch(b"main").expect("merged landing branch");
    assert_ne!(merged, landing);
    world.run_for(10);
    assert_eq!(world.stats().late_landings, 1);
    assert_eq!(world.branch(b"main"), Some(merged), "the stale copy cannot merge a second time");
    assert!(
        world
            .outcomes()
            .iter()
            .any(|(entry, outcome)| { *entry == 1 && matches!(outcome, Outcome::Made { made: Made::Merged(_), .. }) })
    );
    world.finish();
}

#[test]
fn a_set_may_be_replayed_without_changing_its_final_state() {
    let mut world = World::new(Settings::calm(44));
    let head = temper_engine_forge_client_world::translate::commit(world.branch(b"main").expect("fixture main"));
    world.land_writes_late(skein_lib::Duration::from_secs(20));
    world.make(Entry {
        number: 1,
        task: 7,
        repository: REPO,
        effect: Effect {
            write: api::Write::Status { commit: head, context: Box::from(&b"build"[..]), check: api::Check::Passed },
            condition: Condition::None,
        },
        start: None,
        attempt: None,
        failures: 0,
    });
    world.run_for(1);
    world.calm();
    world.restart();
    world.run_for(8);
    assert_eq!(world.stats().writes, 1);
    world.run_for(15);
    assert_eq!(world.stats().writes, 2);
    assert_eq!(world.stats().late_landings, 1);
    world.read(9, api::Read::Statuses { commit: head, page: 1 });
    world.run_for(1);
    match world.read_result(9) {
        Some(Ok(api::Answer::Statuses { statuses, .. })) => {
            assert_eq!(statuses.len(), 1);
            assert_eq!(statuses[0].context.as_ref(), b"build");
            assert_eq!(statuses[0].check, api::Check::Passed);
        }
        other => panic!("set status visible after both copies: {other:?}"),
    }
    world.finish();
}

#[test]
fn another_writer_taking_a_keyed_branch_name_is_reported_without_overwriting_it() {
    let mut world = World::new(Settings::calm(45));
    world.historical_pull();
    let intended = world.branch(b"main").expect("fixture main");
    let other = world.branch(b"topic").expect("fixture topic");
    assert_ne!(intended, other);
    world.land_writes_late(skein_lib::Duration::from_secs(20));
    world.make(Entry {
        number: 1,
        task: 7,
        repository: REPO,
        effect: Effect {
            write: api::Write::CreateBranch {
                branch: Box::from(&b"temper/7"[..]),
                commit: temper_engine_forge_client_world::translate::commit(intended),
            },
            condition: Condition::None,
        },
        start: None,
        attempt: None,
        failures: 0,
    });
    world.run_for(1);
    world.calm();
    world.outside_branch(b"temper/7", other);
    world.restart();
    world.run_for(2);
    assert_eq!(world.stats().writes, 1);
    assert_eq!(world.branch(b"temper/7"), Some(other));
    assert!(world.outcomes().contains(&(1, Outcome::Failed(api::Error::Exists))));
    world.run_for(20);
    assert_eq!(world.stats().late_landings, 1);
    assert_eq!(world.branch(b"temper/7"), Some(other));
    world.finish();
}
#[test]
fn a_seed_replays_and_facts_change_no_decision() {
    let first = run(Settings::random(7));
    let second = run(Settings::random(7));
    assert_eq!(first, second);
    let no_facts = run(Settings { facts: 0, ..Settings::random(7) });
    assert_eq!(first, no_facts);
}
#[test]
fn idle_cost_is_independent_of_ten_times_the_history() {
    let mut empty = World::new(Settings::calm(1));
    let mut old = World::new(Settings { history: 20, ..Settings::calm(1) });
    let watch = Watch { resource: Resource { repository: REPO, what: What::Repository }, participating: false };
    empty.keep(Box::new([watch.clone()]));
    old.keep(Box::new([watch]));
    empty.run_for(20);
    old.run_for(20);
    assert_eq!(empty.stats().calls, old.stats().calls);
    assert_eq!(empty.stats().reads, old.stats().reads);
    empty.finish();
    old.finish();
}
#[test]
fn markers_have_the_protocol_spelling_and_arbitrary_bytes_round_trip() {
    let body = temper_engine_forge_client_world::translate::keyed(&[0, 255], b"hello");
    assert_eq!(&*body, b"<!-- temper:key 00ff -->\nhello");
    assert_eq!(temper_engine_forge_client_world::translate::key_of(&body), Some(Box::from(&[0, 255][..])));
}
#[test]
fn every_live_fresh_read_finishes_once_under_faults_and_rate_resets() {
    let mut world = World::new(Settings::random(31));
    for number in 1..=3 {
        world.read(number, api::Read::Branch { branch: Box::from(&b"main"[..]) });
    }
    world.run_for(20);
    world.finish();
    assert_eq!(world.stats().read_terminals, 3);
    assert_eq!(world.stats().calls, world.stats().terminals);
}
#[test]
fn the_groundwork_reads_keep_the_fake_api_facts_in_the_client_projection() {
    let mut world = World::new(Settings::calm(5));
    world.read(
        1,
        api::Read::Job {
            attempt: api::JobAttempt {
                head: temper_engine_forge_client_world::translate::commit(1),
                run: 1,
                job: 1,
                attempt: 1,
            },
            max_bytes: 64,
        },
    );
    world.read(2, api::Read::Protection { branch: Box::from(&b"main"[..]) });
    world.read(3, api::Read::Settings);
    world.read(4, api::Read::Permission { user: 2 });
    world.run_for(1);
    assert_eq!(world.read_result(1), Some(&Err(api::Error::MissingJob)));
    assert_eq!(world.read_result(2), Some(&Ok(api::Answer::Protection(None))));
    assert_eq!(world.read_result(4), Some(&Ok(api::Answer::Permission(api::Permission::Write))));
    match world.read_result(3).expect("settings answered") {
        Ok(api::Answer::Settings(settings)) => {
            assert_eq!(&*settings.default_branch, b"main");
            assert!(settings.merge);
        }
        Ok(_) | Err(_) => panic!("settings projection"),
    }
    world.finish();
}

#[test]
fn bounded_job_bytes_translate_to_a_truncated_attempt_log() {
    let attempt =
        api::JobAttempt { head: temper_engine_forge_client_world::translate::commit(3), run: 3, job: 7, attempt: 2 };
    let asked = api::Op::Read(api::Read::Job { attempt, max_bytes: 4 });
    let result = temper_engine_forge_client_world::translate::answer(
        &asked,
        temper_fake_forge_domain::api::Answer::File(Box::from(&b"error"[..])),
        &temper_engine_forge_client_world::LIMITS,
    );
    assert_eq!(result, api::Answer::Job { attempt, log: Box::from(&b"rror"[..]), truncated: true });
}
#[test]
fn adoption_reads_history_and_preserves_foreign_or_copied_marker_news() {
    let mut world = World::new(Settings::calm(17));
    let number = world.historical_issue();
    let own: Box<[u8]> = Box::from(&b"\x01\x00\x05worldold"[..]);
    let own_id = world.historical_comment(number, 1, Some(own.clone()));
    let foreign = world.historical_comment(number, 1, Some(Box::from(&b"\x01\x00\x05otherold"[..])));
    let copied = world.historical_comment(number, 2, Some(own));
    let human = world.historical_comment(number, 2, None);
    world.keep(Box::new([Watch {
        resource: Resource { repository: REPO, what: What::Issue(number) },
        participating: true,
    }]));
    world.run_for(1);
    let mut ids = skein_lib::List::with_capacity(4);
    for (_, answer) in world.news() {
        match answer {
            api::Answer::Item { comments, .. } => {
                for comment in comments {
                    assert_ne!(comment.id, own_id);
                    ids.push(comment.id).expect("fixture news count");
                }
            }
            api::Answer::Items { .. }
            | api::Answer::Pull(_)
            | api::Answer::Reviews { .. }
            | api::Answer::Statuses { .. }
            | api::Answer::Remarks { .. }
            | api::Answer::Commit(_)
            | api::Answer::Branches(_)
            | api::Answer::PullFiles { .. }
            | api::Answer::Compare { .. }
            | api::Answer::Checks(_)
            | api::Answer::Job { .. }
            | api::Answer::Protection(_)
            | api::Answer::Settings(_)
            | api::Answer::Collaborators { .. }
            | api::Answer::Permission(_)
            | api::Answer::Created(_)
            | api::Answer::Commented(_)
            | api::Answer::Reviewed(_)
            | api::Answer::Merged(_)
            | api::Answer::Branch(_)
            | api::Answer::Done => panic!("fixture has only issue news"),
        }
    }
    assert_eq!(ids.as_slice(), &[foreign, copied, human]);
    world.finish();
    assert_eq!(world.stats().calls, world.stats().terminals);
}
#[test]
fn comment_page_recovery_and_old_comment_edits_reach_the_subscriber_once() {
    let mut world = World::new(Settings::calm(23));
    let number = world.historical_issue();
    let mut expected = skein_lib::List::with_capacity(6);
    for _ in 0..5 {
        expected.push(world.historical_comment(number, 2, None)).expect("fixture comment count");
    }
    let first = *expected.get(0).expect("first comment");
    world.keep(Box::new([Watch {
        resource: Resource { repository: REPO, what: What::Issue(number) },
        participating: true,
    }]));
    world.run_until_inbox(2);
    world.edit_comment(number, first, Box::from(&b"revised human comment"[..]));
    world.restart();
    world.run_for(1);
    world.restart();
    world.run_for(1);
    expected.push(first).expect("one edit version");
    let observed = comment_ids(&world);
    assert_eq!(observed.as_slice(), expected.as_slice());
    world.restart();
    world.run_for(1);
    assert_eq!(comment_ids(&world).as_slice(), expected.as_slice());
    world.finish();
}
fn comment_ids(world: &World) -> skein_lib::List<u64> {
    let mut ids = skein_lib::List::with_capacity(32);
    for (_, answer) in world.news() {
        match answer {
            api::Answer::Item { comments, .. } => {
                for comment in comments {
                    ids.push(comment.id).expect("fixture comment versions");
                }
            }
            api::Answer::Items { .. }
            | api::Answer::Pull(_)
            | api::Answer::Reviews { .. }
            | api::Answer::Statuses { .. }
            | api::Answer::Remarks { .. }
            | api::Answer::Commit(_)
            | api::Answer::Branches(_)
            | api::Answer::PullFiles { .. }
            | api::Answer::Compare { .. }
            | api::Answer::Checks(_)
            | api::Answer::Job { .. }
            | api::Answer::Protection(_)
            | api::Answer::Settings(_)
            | api::Answer::Collaborators { .. }
            | api::Answer::Permission(_)
            | api::Answer::Created(_)
            | api::Answer::Commented(_)
            | api::Answer::Reviewed(_)
            | api::Answer::Merged(_)
            | api::Answer::Branch(_)
            | api::Answer::Done => {}
        }
    }
    ids
}
#[test]
fn review_page_recovery_keeps_old_pending_submissions_without_duplicate_news() {
    let mut world = World::new(Settings::calm(29));
    let number = world.historical_pull();
    let pending = world.historical_review(number, true);
    let mut expected = skein_lib::List::with_capacity(4);
    for _ in 0..3 {
        expected.push(world.historical_review(number, false)).expect("review count");
    }
    world.keep(Box::new([Watch {
        resource: Resource { repository: REPO, what: What::Pull(number) },
        participating: true,
    }]));
    world.run_until_inbox(2);
    world.submit_review(number, pending);
    world.restart();
    world.run_for(1);
    world.restart();
    world.run_for(1);
    expected.push(pending).expect("late submission");
    let mut observed = skein_lib::List::with_capacity(4);
    for (_, answer) in world.news() {
        match answer {
            api::Answer::Reviews { reviews, .. } => {
                for review in reviews {
                    observed.push(review.id).expect("fixture review versions");
                }
            }
            api::Answer::Items { .. }
            | api::Answer::Item { .. }
            | api::Answer::Pull(_)
            | api::Answer::Statuses { .. }
            | api::Answer::Remarks { .. }
            | api::Answer::Commit(_)
            | api::Answer::Branches(_)
            | api::Answer::PullFiles { .. }
            | api::Answer::Compare { .. }
            | api::Answer::Checks(_)
            | api::Answer::Job { .. }
            | api::Answer::Protection(_)
            | api::Answer::Settings(_)
            | api::Answer::Collaborators { .. }
            | api::Answer::Permission(_)
            | api::Answer::Created(_)
            | api::Answer::Commented(_)
            | api::Answer::Reviewed(_)
            | api::Answer::Merged(_)
            | api::Answer::Branch(_)
            | api::Answer::Done => {}
        }
    }
    assert_eq!(observed.as_slice(), expected.as_slice());
    world.restart();
    world.run_for(1);
    world.finish();
}
#[test]
fn inactive_history_does_not_change_a_participating_objects_idle_calls() {
    let mut short = World::new(Settings { history: 2, ..Settings::calm(41) });
    let mut long = World::new(Settings { history: 20, ..Settings::calm(41) });
    for world in [&mut short, &mut long] {
        let number = world.historical_issue();
        world.historical_comment(number, 2, None);
        world.keep(Box::new([Watch {
            resource: Resource { repository: REPO, what: What::Issue(number) },
            participating: true,
        }]));
        world.run_for(60);
    }
    assert_eq!(short.stats().calls, long.stats().calls);
    assert_eq!(short.stats().reads, long.stats().reads);
    short.finish();
    long.finish();
}
#[test]
fn active_comment_history_has_a_bounded_slow_repair_cost_and_catches_a_lost_edit_hint() {
    let short = repair_cost(2);
    let long = repair_cost(20);
    assert_eq!(short.0, long.0, "routine tail polling does not reread unchanged history");
    assert_eq!(long.1.saturating_sub(short.1), 9, "slow repair reads the active history's additional pages");
    assert!(long.1 <= 32, "slow repair and ordinary listing calls are bounded in this horizon");
}
fn repair_cost(history: u32) -> (u32, u32) {
    let mut world = World::new(Settings::calm(43));
    let number = world.historical_issue();
    let mut old = None;
    for _ in 0..history {
        old = Some(world.historical_comment(number, 2, None));
    }
    let old = old.expect("nonempty active history");
    world.keep(Box::new([Watch {
        resource: Resource { repository: REPO, what: What::Issue(number) },
        participating: true,
    }]));
    world.run_for(1);
    let before = world.stats().calls;
    world.run_for(10);
    let routine = world.stats().calls.saturating_sub(before);
    world.missed_edit_comment(old, Box::from(&b"edit whose webhook was lost"[..]));
    let before = world.stats().calls;
    world.run_for(40);
    let repair = world.stats().calls.saturating_sub(before);
    let ids = comment_ids(&world);
    assert_eq!(ids.len(), history.checked_add(1).expect("fixture history cap"));
    assert_eq!(ids.get(history), Some(&old));
    world.finish();
    (routine, repair)
}
#[test]
fn slow_repair_finds_a_lost_review_hint_without_item_time_revision_evidence() {
    let mut world = World::new(Settings::calm(47));
    let number = world.historical_pull();
    let pending = world.recent_review(number, true);
    world.recent_review(number, false);
    world.keep(Box::new([Watch {
        resource: Resource { repository: REPO, what: What::Pull(number) },
        participating: true,
    }]));
    world.run_until_inbox(1);
    // The pending review is submitted within the same forge time quantum:
    // the item's updated time remains equal and its webhook is lost.
    world.missed_review_submission(number, pending);
    world.run_for(10);
    assert_eq!(review_count(&world), 1);
    world.run_for(40);
    assert_eq!(review_count(&world), 2);
    world.restart();
    world.run_for(1);
    assert_eq!(review_count(&world), 2);
    world.finish();
}
fn review_count(world: &World) -> u32 {
    let mut count = 0_u32;
    for (_, answer) in world.news() {
        match answer {
            api::Answer::Reviews { reviews, .. } => {
                count = count.saturating_add(u32::try_from(reviews.len()).expect("bounded review rows"));
            }
            api::Answer::Items { .. }
            | api::Answer::Item { .. }
            | api::Answer::Pull(_)
            | api::Answer::Statuses { .. }
            | api::Answer::Remarks { .. }
            | api::Answer::Commit(_)
            | api::Answer::Branches(_)
            | api::Answer::PullFiles { .. }
            | api::Answer::Compare { .. }
            | api::Answer::Checks(_)
            | api::Answer::Job { .. }
            | api::Answer::Protection(_)
            | api::Answer::Settings(_)
            | api::Answer::Collaborators { .. }
            | api::Answer::Permission(_)
            | api::Answer::Created(_)
            | api::Answer::Commented(_)
            | api::Answer::Reviewed(_)
            | api::Answer::Merged(_)
            | api::Answer::Branch(_)
            | api::Answer::Done => {}
        }
    }
    count
}
#[test]
fn a_human_edit_of_an_old_own_marker_is_news_even_when_the_original_author_stays() {
    let mut world = World::new(Settings::calm(53));
    let number = world.historical_issue();
    let key = Box::from(&b"\x01\x00\x05worldold"[..]);
    let own = world.historical_comment(number, 1, Some(key));
    world.keep(Box::new([Watch {
        resource: Resource { repository: REPO, what: What::Issue(number) },
        participating: true,
    }]));
    world.run_for(1);
    assert!(comment_ids(&world).is_empty());
    let body = temper_engine_forge_client_world::translate::keyed(b"\x01\x00\x05worldold", b"a human revised this");
    world.edit_comment(number, own, body);
    world.run_for(2);
    assert_eq!(comment_ids(&world).as_slice(), &[own]);
    world.restart();
    world.run_for(1);
    assert_eq!(comment_ids(&world).as_slice(), &[own]);
    world.finish();
}
#[test]
fn a_pre_adoption_human_edit_is_news_despite_our_retained_author_and_marker() {
    let mut world = World::new(Settings::calm(59));
    let number = world.historical_issue();
    let own = world.historical_comment(number, 1, Some(Box::from(&b"\x01\x00\x05worldold"[..])));
    let body =
        temper_engine_forge_client_world::translate::keyed(b"\x01\x00\x05worldold", b"human edit before adoption");
    world.missed_edit_comment(own, body);
    world.keep(Box::new([Watch {
        resource: Resource { repository: REPO, what: What::Issue(number) },
        participating: true,
    }]));
    world.run_for(1);
    assert_eq!(comment_ids(&world).as_slice(), &[own]);
    world.restart();
    world.run_for(1);
    assert_eq!(comment_ids(&world).as_slice(), &[own]);
    world.finish();
}
