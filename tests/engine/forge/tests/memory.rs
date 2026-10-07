use skein_lib::{Env, Queue, Time, Wall};
use temper_engine_domain_forge as top;
use temper_engine_domain_forge_client as client;
use temper_engine_forge_world::{LIMITS, REPO};
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

fn drain(out: &mut Queue<top::Request>) {
    for _ in 0..out.len() {
        out.pop();
    }
}

#[test]
fn bounded_connector_tables_and_client_fit_the_declared_heap() {
    for limits in [LIMITS, top::Limits { facts: 0, client: client::Limits { facts: 0, ..LIMITS.client }, ..LIMITS }] {
        let meter = Meter::new();
        let mut domain = top::Domain::new(
            &limits,
            9,
            client::Config {
                namespace: Box::from(&b"world"[..]),
                writers: Box::new([client::Writer { forge: REPO.forge, author: 1 }]),
            },
        )
        .expect("admitted limits");
        let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
        let mut out = Queue::with_capacity(top::max_out(&limits));
        let bound = top::worst_case(&limits).expect("admitted limits");
        top::step(&mut domain, &env, top::Event::Restored { clock: client::RecoveryClock::Monotonic }, &mut out);
        drain(&mut out);
        for task in 1..=u64::from(limits.tasks) {
            let name = top::Name { forge: REPO.forge, repository: REPO.repository, what: top::What::Issue(task) };
            meter.start();
            top::step(&mut domain, &env, top::Event::Names { task, resources: Box::new([name.clone()]) }, &mut out);
            let measured = meter.end();
            drain(&mut out);
            meter.check(measured, bound, limits);
            domain.reclaim();

            meter.start();
            top::step(&mut domain, &env, top::Event::Hold { task, resource: name, from: None }, &mut out);
            let measured = meter.end();
            drain(&mut out);
            meter.check(measured, bound, limits);
            domain.reclaim();
        }
    }
}
