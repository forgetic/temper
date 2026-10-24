//! Drain before closing children, close the store last (protocol/README.md, 6).
use crate::{End, Stop, engine::Engine};
use jig_protocol_config::{Path, Problems, What};
use temper_engine_domain as root;

pub(crate) struct Drain {
    pub(crate) closing: bool,
    stopped: Option<Stop>,
}
/// A second call aborts, ending at once with the reason a failure's drain
/// carries (protocol/README.md, section 4).
pub(crate) fn shutdown(engine: &mut Engine) {
    if let Some(drain) = &mut engine.drain {
        engine.end = Some(match drain.stopped.take() {
            Some(why) => End::Stopped(why),
            None => End::Aborted,
        });
        return;
    }
    engine.drain = Some(Drain { closing: false, stopped: None });
    engine.workers.down(engine.env.now, jig_protocol_engine_workers::Event::Stop, &mut engine.events);
    engine.clients.down(jig_protocol_engine_clients::Event::Stop);
    engine.events.push(root::Event::Stop);
}
pub(crate) fn drained(engine: &mut Engine) {
    let Some(drain) = &mut engine.drain else {
        return;
    };
    if drain.closing {
        return;
    }
    drain.closing = true;
    engine.setup.close();
    engine.forge.down(skein_forge_connection::Request::Close);
    engine.clients.down(jig_protocol_engine_clients::Event::Close);
    engine.workers.close(engine.env.now, &mut engine.events);
    if engine.accounts.is_some() {
        engine.step(root::Event::Account(jig_domain_accounts::Event::Close));
    }
}
pub(crate) fn advance(engine: &mut Engine) {
    if engine.end.is_some() {
        return;
    }
    let Some(drain) = &mut engine.drain else {
        return;
    };
    if !drain.closing {
        return;
    }
    engine.setup.close();
    let closed = match &engine.accounts {
        Some(accounts) => accounts.is_empty(),
        None => true,
    };
    if closed
        && engine.setup.closed
        && engine.setup.io.is_empty()
        && engine.forge.closed
        && engine.forge.io.is_empty()
        && engine.workers.closed()
        && engine.clients.closed
        && engine.clients.io.is_empty()
        && engine.store.idle()
        && engine.events.is_empty()
        && engine.requests.is_empty()
    {
        engine.store.close();
        if engine.store.closed() {
            engine.end = Some(match drain.stopped.take() {
                Some(why) => End::Stopped(why),
                None => End::Drained,
            });
        }
    }
}
pub(crate) fn failed(engine: &mut Engine, number: u64) {
    engine.step(root::Event::Store(root::StoreInput::Uncommitted { number }));
    engine.end = Some(End::Stopped(Stop::CommitFailed { number }));
}
pub(crate) fn open(engine: &mut Engine, why: &jig_protocol_store::OpenRefusal) {
    engine.end = Some(End::Stopped(Stop::StoreOpen { why: why.clone() }));
}
pub(crate) fn unavailable(engine: &mut Engine, member: &[u8]) {
    let mut problems = Problems::empty();
    problems.push(Path::root().member(member), What::Refers);
    engine.end = Some(End::Stopped(Stop::Start(problems)));
}

/// Losing the webhook listener drains the engine (protocol/forge.md, 2.3).
pub(crate) fn hooks(engine: &mut Engine) {
    if engine.drain.is_none() {
        shutdown(engine);
    }
    if let Some(drain) = &mut engine.drain
        && drain.stopped.is_none()
    {
        drain.stopped = Some(Stop::Hooks);
    }
}

/// Preserve jig's named settings failure in the ordinary startup diagnostics.
pub(crate) fn root(engine: &mut Engine) {
    let Some(jig_domain_core::RestartRequest::Settings { failure, changed }) = engine.domain.settings_failure() else {
        unavailable(engine, b"deployment");
        return;
    };
    engine.end = Some(End::Stopped(Stop::Start(settings_problem(failure, changed))));
}
fn settings_problem(failure: jig_domain_core::settings::Failure, changed: bool) -> Problems {
    use jig_domain_core::settings::{Fault, Setting};
    let origin: &[u8] = if changed { b"owner's stored change" } else { b"binary's default" };
    let path = Path::root().member(b"settings").member(origin);
    let path = match failure.setting {
        Setting::Charter(number) => path.member(b"charter").child(jig_protocol_config::Segment::Index(number)),
        Setting::Template(number) => path.member(b"template").child(jig_protocol_config::Segment::Index(number)),
        Setting::Policy(number) => path.member(b"policy").child(jig_protocol_config::Segment::Index(number)),
        Setting::Rules => path.member(b"rules"),
    };
    let fault: &[u8] = match failure.fault {
        Fault::Shape => b"shape",
        Fault::Count => b"count",
        Fault::Reference => b"reference",
        Fault::Rules => b"rules",
        Fault::Budget => b"budget",
        Fault::Grace => b"grace",
        Fault::Owner => b"owner",
    };
    let mut problems = Problems::empty();
    problems.push(path.member(fault), What::Form);
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_start_refusals_name_the_setting_fault_and_source() {
        use jig_domain_core::settings::{Failure, Fault, Setting};
        for (changed, origin) in [(true, b"owner's stored change".as_slice()), (false, b"binary's default".as_slice())]
        {
            let problems = settings_problem(Failure { setting: Setting::Template(1), fault: Fault::Grace }, changed);
            let problem = problems.problems.get(0).unwrap();
            let path = &problem.path.segments;
            assert_eq!(path.get(0), Some(&jig_protocol_config::Segment::Member(b"settings".as_slice().into())));
            assert_eq!(path.get(1), Some(&jig_protocol_config::Segment::Member(origin.into())));
            assert_eq!(path.get(2), Some(&jig_protocol_config::Segment::Member(b"template".as_slice().into())));
            assert_eq!(path.get(3), Some(&jig_protocol_config::Segment::Index(1)));
            assert_eq!(path.get(4), Some(&jig_protocol_config::Segment::Member(b"grace".as_slice().into())));
        }
    }
}
