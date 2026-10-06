use skein_lib::{Env, Queue, Time, Wall};
use temper_web_domain::{Address, ChatLine, Domain, Event, Offset, ReadResult, Request, max_out, step};
use temper_web_domain_world::Settings;
use temper_web_view::{Patch, View, render};
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

#[test]
fn full_chat_window_stays_within_declared_heap_plus_fixed_render_words() {
    let settings = Settings::calm(301);
    let meter = Meter::new();
    meter.start();
    let mut domain = Domain::new(&settings.domain, 301);
    let mut view = View::new(&settings.view);
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: settings.domain };
    let mut requests = Queue::<Request>::with_capacity(max_out(&settings.domain));
    let mut patches = Queue::<Patch>::with_capacity(settings.view.patches);
    step(&mut domain, &env, Event::Start { address: Address::Chats, saved: None, offset: Offset(0) }, &mut requests);
    let mut read = None;
    while let Some(request) = requests.pop() {
        if let Request::Read { read: token, .. } = request {
            read = Some(token);
        }
    }
    let rows = (0..settings.domain.window)
        .map(|task| ChatLine {
            task: u64::from(task) + 1,
            project: 7,
            title: vec![b'x'; usize::try_from(settings.domain.text).expect("text fits")].into_boxed_slice(),
            live: true,
            last_activity: Wall::EPOCH,
        })
        .collect::<Vec<_>>();
    step(
        &mut domain,
        &env,
        Event::Read {
            read: read.expect("read token"),
            result: ReadResult::Chats { rows: rows.into_boxed_slice(), older: None },
        },
        &mut requests,
    );
    render(&mut view, &domain, &settings.view, &mut patches);
    let measured = meter.end();
    let bound = temper_web_domain::worst_case(&settings.domain).expect("domain bound")
        + temper_web_view::worst_case(&settings.view).expect("view bound")
        + 16_384;
    meter.check(measured, bound, "full chats view");
    assert!(!patches.is_empty());
}
