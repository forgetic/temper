use skein_lib::{Duration, Rng, Token, stream};
use temper_agent_domain::{self as agent, run};
use temper_agent_protocol::channel::{State, Up};
use temper_agent_protocol_world::pipe::{self, World};
use temper_channel::wire;

fn run(seed: u64) -> Vec<String> {
    let mut rng = Rng::new(seed);
    let mut world = World::new();
    world.peer(pipe::start());
    world.settle();
    for generation in 2..5 {
        world.now = world.now.saturating_add(Duration::from_nanos(rng.below(2_000_000_000)));
        world.peer(wire::Message::AgentGrant { grant: pipe::grant(0, generation) });
        if rng.chance(300) {
            world.peer(wire::Message::AgentCancel);
        }
        world.settle();
    }
    world.roots();
    if rng.chance(500) {
        world.event(stream::Up::Failed(stream::Fault::Reset));
    }
    world.down(agent::Request::Admitted { worker: Token::new(5), run: Token::new(9) });
    for call in 20..24 {
        world.down(agent::Request::Push {
            worker: Token::new(5),
            owner: Token::new(call),
            change: run::outcome::Change { title: b"push".as_slice().into(), body: Box::new([]) },
        });
        world.settle();
        if world.channel.state() == State::Running {
            world.peer(wire::Message::AgentAnswer {
                call: call + 1000,
                reply: wire::Reply::Pushed { push: wire::Push::Done },
            });
            if rng.chance(500) {
                world.down(agent::Request::CancelHost { owner: Token::new(call) });
                world.settle();
                world.peer(wire::Message::AgentAnswer { call, reply: wire::Reply::Withdrawn });
            } else {
                world.peer(wire::Message::AgentAnswer { call, reply: wire::Reply::Pushed { push: wire::Push::Done } });
            }
            world.settle();
        }
        assert_eq!(world.events.iter().filter(|e| matches!(e, Up::Domain(agent::Event::Pushed { owner, .. } | agent::Event::HostCancelled { owner }) if owner.raw() == call)).count(), 1, "seed {seed} call {call}");
    }
    world.down(agent::Request::Cancel { owner: Token::new(100) });
    world.down(agent::Request::CancelIo { owner: Token::new(101) });
    world.down(agent::Request::Abort { owner: Token::new(102) });
    assert_eq!(world.events.iter().filter(|e| matches!(e, Up::Below(_))).count(), 3, "seed {seed}");
    world.closed();
    assert_eq!(world.channel.state(), State::Closed);
    assert!(
        world
            .channel
            .credentials()
            .get(agent::GrantName { account: 0, generation: 4 }, world.now, Duration::ZERO)
            .is_none()
    );
    world.events.into_iter().map(|event| format!("{event:?}")).collect()
}
#[test]
fn randomized_channel_crossings_replay_and_terminate_each_host_call_once() {
    for seed in 0..64 {
        assert_eq!(run(seed), run(seed), "seed {seed}");
    }
}
