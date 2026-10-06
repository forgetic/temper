//! Small complete escalation/cut matrix with injected latency, exact frozen-state
//! replay and observation saturation (testing-strategy.md, section 8).

use std::collections::BTreeSet;
use temper_engine_domain_world::escalation::{Cut, Settings, run_replayed};
use temper_engine_domain_world::escalation_referee::Story;

#[test]
fn fifteen_small_escalation_histories_cover_every_story_and_durable_cut() {
    let stories = [Story::Release, Story::PassRelease, Story::Reject, Story::RaceRelease, Story::RaceReject];
    let cuts = [Cut::None, Cut::Held, Cut::Decision];
    let mut covered = BTreeSet::new();
    for seed in 0..15_u64 {
        let story = usize::try_from(seed % 5).expect("five stories");
        let cut = usize::try_from(seed / 5).expect("three cuts");
        let settings = Settings {
            cut: cuts[cut],
            commit_delay: u32::try_from(seed % 3).expect("tiny store delay"),
            page_delay: u32::try_from((seed / 3) % 3).expect("tiny page delay"),
            facts: seed.is_multiple_of(2),
            ..Settings::calm(seed + 9200, stories[story])
        };
        let world = run_replayed(settings);
        assert!(world.referee.done(), "seed {seed}");
        assert_eq!(world.restarts, u32::from(settings.cut != Cut::None), "seed {seed}");
        covered.insert((story, cut));
    }
    assert_eq!(covered.len(), 15, "all concrete release/pass/reject/race paths cross every durable cut");
}
