use skein_lib::Rng;
use temper_agent_domain_session::{self as session, llm};
use temper_agent_session_world::recorded::{self, World, opening};

#[test]
fn randomized_priced_turns_and_terminal_races() {
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
            assert_eq!(world.prompts.len(), if expected >= 20 { 1 } else { 2 }, "seed {seed}");
        }
        world.close();
    }
}
