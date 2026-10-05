use skein_lib::Rng;
use temper_agent_domain_session::{self as session, llm};
use temper_agent_session_world::recorded::{self, World, opening};

#[test]
fn randomized_priced_turns_and_terminal_races() {
    let mut closing_won = 0;
    let mut closing_lost = 0;
    let mut no_close = 0;
    let mut next_admitted = 0;
    let mut next_stopped = 0;
    for seed in 0..64 {
        let mut rng = Rng::new(seed);
        let mut world = World::new(seed, if seed % 2 == 0 { 0 } else { 256 });
        world.open(opening(None, 20));
        let usage = llm::Usage {
            input_tokens: rng.below(30),
            output_tokens: rng.below(10),
            cache_read_tokens: rng.below(12),
            cache_write_tokens: rng.below(9),
        };
        let subtotal = (usage.input_tokens + usage.cache_write_tokens) * 7
            + usage.cache_read_tokens * 3
            + usage.output_tokens * 11;
        let parent = subtotal.div_ceil(10);
        world.complete(recorded::called(), llm::Stop::ToolUse, usage);
        let owner = world.delegated[0];
        let closing = rng.below(2) == 0;
        if closing {
            world.step(session::Event::Close { session: world.session.expect("the scenario supplied a value") });
        }
        let wins = !closing || rng.below(2) == 0;
        let child = rng.below(17);
        match (closing, wins) {
            (true, true) => closing_lost += 1,
            (true, false) => closing_won += 1,
            (false, true) => no_close += 1,
            (false, false) => unreachable!("a call not closed answers"),
        }
        if wins {
            world.step(session::Event::AnsweredV2 {
                owner,
                text: vec![b'v'; usize::try_from(rng.below(64)).expect("the scenario supplied a value")].into(),
                error: false,
                spent: child,
            });
        } else {
            world.step(session::Event::AnswerCancelledV2 { owner, spent: child });
        }
        assert_eq!(world.turns.len(), 1, "seed {seed}");
        let expected = parent + child;
        assert_eq!(world.turns[0].spent, expected, "seed {seed}");
        if !closing {
            if expected >= 20 {
                next_stopped += 1;
            } else {
                next_admitted += 1;
            }
            assert_eq!(world.prompts.len(), if expected >= 20 { 1 } else { 2 }, "seed {seed}");
        }
        world.close();
    }
    assert!(closing_won > 0 && closing_lost > 0 && no_close > 0, "every terminal race outcome occurred");
    assert!(next_admitted > 0 && next_stopped > 0, "both sides of the unit cap occurred");
}
