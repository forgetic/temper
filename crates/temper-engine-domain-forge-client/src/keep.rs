//! Bounded keeping-up of live forge resources (domain/forge.md, section 5).
//!
//! The working set holds live rows, scan positions, inbox delivery stamps,
//! and observed heads. It never discovers resources from labels or reads
//! task policy. Admission adds a complete watch set; hints advance due
//! reads; answers update durable positions and publish changed facts;
//! departure erases rows after their pending reads settle.
use crate::api::{self, Answer, Change, Commit, Error, Op, Read, Repository};
use crate::calls::{self, Owner};
use crate::domain::Alarm;
use crate::{
    Cached, Delivery, Domain, Echo, Entry, Fact, Key, Limits, LiveRecord, Position, Priority, RepositoryRecord,
    Request, Resource, Stored, Watch, What,
};
use alloc::boxed::Box;
use skein_lib::{Duration, Env, List, Map, Queue, Time, Token};
#[derive(Debug)]
pub(crate) struct Keep {
    live: Map<Resource, Live>,
    repos: Map<Repository, Repo>,
    sequence: u64,
    ready: bool,
    failed: bool,
    fresh_active: bool,
}
#[derive(Debug)]
struct Live {
    record: LiveRecord,
    alarm: u64,
    active: bool,
    fresh: Fresh,
    writer: bool,
    hinted: bool,
    state: State,
    interval: Duration,
    echo_capacity: u32,
    inbox: Inbox,
}
#[derive(Debug)]
struct Inbox {
    repair_at: Time,
    needs_repair: bool,
    reviews_needed: bool,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum State {
    Idle,
    Due(Phase, Priority),
    Busy(Phase, Priority),
    Waiting(Phase, Priority),
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Phase {
    Object,
    Pull,
    Reviews(u32),
    Branch,
}
#[derive(Debug)]
struct Repo {
    record: RepositoryRecord,
    active: bool,
    fresh: Fresh,
    state: Scan,
    hinted: bool,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fresh {
    Pending,
    Complete,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Scan {
    Idle,
    Due(Progress),
    Busy(Progress),
    Waiting(Progress),
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Progress {
    since: Time,
    page: u32,
    newest: Time,
    begun: Option<Time>,
    tied: Option<Time>,
    moved: bool,
    bootstrap: bool,
}
impl Progress {
    fn first(mark: Option<Time>) -> Progress {
        let since = mark.unwrap_or(Time::from_nanos(u64::MAX));
        Progress { since, page: 1, newest: since, begun: None, tied: None, moved: false, bootstrap: mark.is_none() }
    }
}
impl Keep {
    pub(crate) fn new(l: &Limits) -> Keep {
        Keep {
            live: Map::with_capacity(l.resources),
            repos: Map::with_capacity(l.repositories),
            sequence: 0,
            ready: false,
            failed: false,
            fresh_active: false,
        }
    }
    pub(crate) fn is_ready(&self) -> bool {
        if !self.ready || self.failed {
            return false;
        }
        for (_, row) in &self.live {
            match row.state {
                State::Due(_, _) if row.active => return true,
                State::Due(_, _) | State::Idle | State::Busy(_, _) | State::Waiting(_, _) => {}
            }
        }
        for (_, row) in &self.repos {
            match row.state {
                Scan::Due(_) if row.active => return true,
                Scan::Due(_) | Scan::Idle | Scan::Busy(_) | Scan::Waiting(_) => {}
            }
        }
        false
    }
    pub(crate) const fn failed(&self) -> bool {
        self.failed
    }
    pub(crate) fn cached(&self, resource: &Resource) -> Option<&Cached> {
        match self.live.get(resource) {
            Some(row) if row.active => Some(&row.record.cached),
            Some(_) | None => None,
        }
    }
}
pub(crate) fn worst_case(l: &Limits) -> Option<u64> {
    l.resources.checked_mul(2)?.checked_add(l.repositories.checked_mul(2)?)?.checked_add(1)?;
    let per = u64::try_from(size_of::<Cached>())
        .ok()?
        .checked_add(u64::from(l.op_bytes).checked_mul(6)?)?
        .checked_add(u64::from(l.answer_bytes).checked_mul(2)?)?
        .checked_add(u64::from(l.rows).checked_mul(u64::try_from(size_of::<Echo>()).ok()?)?)?
        .checked_add(u64::from(l.inbox).checked_mul(u64::try_from(size_of::<Delivery>()).ok()?.checked_mul(2)?)?)?;
    Map::<Resource, Live>::worst_case(l.resources)?
        .checked_add(Map::<Repository, Repo>::worst_case(l.repositories)?)?
        .checked_add(List::<Resource>::worst_case(l.resources)?.checked_mul(2)?)?
        .checked_add(List::<Repository>::worst_case(l.repositories)?.checked_mul(2)?)?
        .checked_add(u64::from(l.resources).checked_mul(per)?)?
        // A response is checked before cloning. Replacement can briefly own
        // old cache, incoming data and its retained copy. Delivered row lists
        // reserve rows, independently of the byte cap on an actual answer.
        .checked_add(u64::from(l.answer_bytes).checked_mul(2)?)?
        .checked_add(u64::from(l.op_bytes).checked_mul(2)?)?
        .checked_add(List::<api::Comment>::worst_case(l.rows)?)?
        .checked_add(List::<api::Review>::worst_case(l.rows)?)?
        .checked_add(List::<Echo>::worst_case(l.rows)?)?
        .checked_add(List::<Delivery>::worst_case(l.inbox)?)
}
fn valid_resource(r: &Resource, l: &Limits) -> bool {
    match &r.what {
        What::Branch(name) => !name.is_empty() && name.len() <= usize::try_from(l.op_bytes).expect("u32 fits usize"),
        What::Repository | What::Pull(_) | What::Issue(_) => true,
    }
}
fn first(resource: &Resource) -> Phase {
    match resource.what {
        What::Branch(_) => Phase::Branch,
        What::Pull(_) | What::Issue(_) | What::Repository => Phase::Object,
    }
}
fn live(record: LiveRecord, l: &Limits, alarm: u64) -> Live {
    let phase = first(&record.watch.resource);
    let needs_repair = !record.comment_scanning;
    Live {
        record,
        alarm,
        active: true,
        fresh: Fresh::Complete,
        writer: false,
        hinted: false,
        state: State::Due(phase, Priority::Keep),
        interval: l.poll,
        echo_capacity: l.rows,
        inbox: Inbox { repair_at: Time::ZERO, needs_repair, reviews_needed: false },
    }
}
fn repository(record: RepositoryRecord) -> Repo {
    let progress = Progress::first(record.mark);
    Repo { record, active: true, fresh: Fresh::Complete, state: Scan::Due(progress), hinted: false }
}
fn has(watches: &[Watch], resource: &Resource) -> bool {
    for watch in watches {
        if watch.resource == *resource {
            return true;
        }
    }
    false
}
fn has_repo(watches: &[Watch], repo: Repository) -> bool {
    for watch in watches {
        if watch.resource.repository == repo {
            return true;
        }
    }
    false
}
fn busy(state: State) -> bool {
    match state {
        State::Busy(_, _) => true,
        State::Idle | State::Due(_, _) | State::Waiting(_, _) => false,
    }
}
fn scan_busy(state: Scan) -> bool {
    match state {
        Scan::Busy(_) => true,
        Scan::Idle | Scan::Due(_) | Scan::Waiting(_) => false,
    }
}
fn admission(k: &Keep, l: &Limits, watches: &[Watch]) -> Result<(), Error> {
    if watches.len() > usize::try_from(l.resources).expect("u32 fits usize") {
        return Err(Error::TooLarge);
    }
    if !k.ready || k.failed {
        return Err(Error::Busy);
    }
    for (index, watch) in watches.iter().enumerate() {
        if !valid_resource(&watch.resource, l) {
            return Err(Error::TooLarge);
        }
        if watch.participating {
            match watch.resource.what {
                What::Pull(_) | What::Issue(_) => {}
                What::Repository | What::Branch(_) => return Err(Error::Refused),
            }
        }
        for prior in watches.get(..index).expect("prior slice") {
            if prior.resource == watch.resource {
                return Err(Error::Refused);
            }
        }
    }
    let mut resources = u32::try_from(watches.len()).expect("bounded length");
    for (key, row) in &k.live {
        if !has(watches, key) && busy(row.state) {
            resources = resources.saturating_add(1);
        }
    }
    let mut repositories = 0_u32;
    for (index, watch) in watches.iter().enumerate() {
        if !has_repo(watches.get(..index).expect("prior slice"), watch.resource.repository) {
            repositories = repositories.saturating_add(1);
        }
    }
    for (key, row) in &k.repos {
        if !has_repo(watches, *key) && scan_busy(row.state) {
            repositories = repositories.saturating_add(1);
        }
    }
    if resources > l.resources || repositories > l.repositories { Err(Error::Busy) } else { Ok(()) }
}
pub(crate) fn replace(
    d: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    watches: Box<[Watch]>,
    out: &mut Queue<Request>,
) {
    if let Err(error) = admission(&d.keep, &env.limits, &watches) {
        d.fact(Fact::Refused);
        out.push(Request::Kept { owner, result: Err(error) });
        return;
    }
    remove_live(d, &watches, out);
    remove_repos(d, &watches, out);
    for watch in watches {
        let key = watch.resource.clone();
        match d.keep.live.get_mut(&key) {
            Some(row) => {
                let changed = row.record.watch.participating != watch.participating || !row.active;
                row.active = true;
                row.record.watch = watch;
                if changed {
                    out.push(Request::Save { record: Stored::Live(row.record.clone()) });
                    row.hinted = true;
                }
            }
            None => {
                let record = LiveRecord {
                    watch,
                    position: Position { at: Time::ZERO, comment: 0, review: 0, review_page: 1 },
                    listed: Time::ZERO,
                    comment_after: 0,
                    comment_page: 1,
                    comment_scanning: true,
                    comment_repair: true,
                    review_page: 1,
                    comments: Box::new([]),
                    reviews: Box::new([]),
                    cached: Box::new(Cached { item: None, pull: None, tip: None }),
                    pushed: None,
                    echoes: Box::new([]),
                };
                out.push(Request::Save { record: Stored::Live(record.clone()) });
                d.keep
                    .live
                    .insert(key.clone(), live(record, &env.limits, next_id(&mut d.keep.sequence)))
                    .expect("batch admitted");
            }
        }
        let repo = key.repository;
        match d.keep.repos.get_mut(&repo) {
            Some(row) => {
                if !row.active {
                    out.push(Request::Save { record: Stored::Repository(row.record) });
                }
                row.active = true;
            }
            None => {
                let record = RepositoryRecord { repository: repo, mark: None, clock: Time::ZERO };
                out.push(Request::Save { record: Stored::Repository(record) });
                d.keep.repos.insert(repo, repository(record)).expect("batch admitted");
            }
        }
    }
    out.push(Request::Kept { owner, result: Ok(()) });
}
fn remove_live(d: &mut Domain, watches: &[Watch], out: &mut Queue<Request>) {
    let mut removed = List::with_capacity(d.keep.live.len());
    for key in keys(&d.keep) {
        let row = d.keep.live.get_mut(&key).expect("bounded key remains");
        let key = &key;
        if !has(watches, key) {
            if row.active {
                out.push(Request::Erase { key: Key::Live(key.clone()) });
            }
            row.active = false;
            d.alarms.cancel(Alarm::Resource(row.alarm));
            if busy(row.state) && calls::cancel_resource(&mut d.calls, key) {
                row.state = State::Idle;
            }
            if !busy(row.state) {
                removed.push(key.clone()).expect("bounded removals");
            }
        }
    }
    for key in removed.into_boxed() {
        d.keep.live.remove(&key).expect("removed row exists");
    }
}
fn remove_repos(d: &mut Domain, watches: &[Watch], out: &mut Queue<Request>) {
    let mut removed = List::with_capacity(d.keep.repos.len());
    for key in repos(&d.keep) {
        let row = d.keep.repos.get_mut(&key).expect("bounded key remains");
        let key = &key;
        if !has_repo(watches, *key) {
            if row.active {
                out.push(Request::Erase { key: Key::Repository(*key) });
            }
            row.active = false;
            d.alarms.cancel(Alarm::Repository(*key));
            if scan_busy(row.state) && calls::cancel_repository(&mut d.calls, *key) {
                row.state = Scan::Idle;
            }
            if !scan_busy(row.state) {
                removed.push(*key).expect("bounded removals");
            }
        }
    }
    for key in removed.into_boxed() {
        d.keep.repos.remove(&key).expect("removed row exists");
    }
}
pub(crate) fn restore_live(d: &mut Domain, env: &Env<Limits>, record: LiveRecord) {
    assert!(!d.keep.ready, "restore before restored");
    if !valid_participation(&record.watch)
        || !valid_resource(&record.watch.resource, &env.limits)
        || record.review_page == 0
        || record.review_page > env.limits.inbox
        || record.comment_page == 0
        || record.comment_page > env.limits.inbox
        || !valid_deliveries(&record.comments, env.limits.inbox)
        || !valid_deliveries(&record.reviews, env.limits.inbox)
        || !crate::bounds::cached(&record.cached, &env.limits)
        || record.echoes.len() > usize::try_from(env.limits.rows).expect("u32 fits usize")
        || d.keep.live.len() >= env.limits.resources
    {
        d.keep.failed = true;
        d.fact(Fact::Refused);
        return;
    }
    let key = record.watch.resource.clone();
    assert!(
        d.keep
            .live
            .insert(key, live(record, &env.limits, next_id(&mut d.keep.sequence)))
            .expect("restore admitted")
            .is_none(),
        "one stored row per resource"
    );
}
pub(crate) fn restore_repository(d: &mut Domain, env: &Env<Limits>, record: RepositoryRecord) {
    assert!(!d.keep.ready, "restore before restored");
    if d.keep.repos.len() >= env.limits.repositories {
        d.keep.failed = true;
        d.fact(Fact::Refused);
        return;
    }
    assert!(
        d.keep.repos.insert(record.repository, repository(record)).expect("restore admitted").is_none(),
        "one stored row per repository"
    );
}
pub(crate) fn restored(d: &mut Domain) {
    assert!(!d.keep.ready, "restored once");
    for (key, _) in &d.keep.live {
        if d.keep.repos.get(&key.repository).is_none() {
            d.keep.failed = true;
        }
    }
    for (repository, _) in &d.keep.repos {
        let mut named = false;
        for (key, _) in &d.keep.live {
            if key.repository == *repository {
                named = true;
                break;
            }
        }
        if !named {
            d.keep.failed = true;
        }
    }
    d.keep.ready = true;
}
pub(crate) fn start_fresh(d: &mut Domain) {
    assert!(d.keep.ready && !d.keep.fresh_active, "fresh pass follows restored records once");
    d.keep.fresh_active = true;
    for key in keys(&d.keep) {
        let row = d.keep.live.get_mut(&key).expect("restored live resource remains");
        if row.active {
            row.fresh = match &row.record.watch.resource.what {
                What::Repository => Fresh::Complete,
                What::Branch(_) | What::Pull(_) | What::Issue(_) => Fresh::Pending,
            };
            row.state = State::Due(first(&row.record.watch.resource), Priority::Fresh);
        }
    }
    for key in repos(&d.keep) {
        let row = d.keep.repos.get_mut(&key).expect("restored repository remains");
        if row.active {
            row.fresh = Fresh::Pending;
            row.state = Scan::Due(Progress::first(row.record.mark));
        }
    }
}
pub(crate) fn fresh_done(d: &mut Domain) -> bool {
    if !d.keep.fresh_active {
        return false;
    }
    for (_, row) in &d.keep.live {
        if row.active && row.fresh == Fresh::Pending {
            return false;
        }
    }
    for (_, row) in &d.keep.repos {
        if row.active && row.fresh == Fresh::Pending {
            return false;
        }
    }
    d.keep.fresh_active = false;
    true
}
pub(crate) fn pump(d: &mut Domain, _env: &Env<Limits>, out: &mut Queue<Request>) -> bool {
    if !d.keep.is_ready() || d.calls.is_full() {
        return false;
    }
    for key in repos(&d.keep) {
        let row = d.keep.repos.get_mut(&key).expect("bounded key remains");
        let key = &key;
        match row.state {
            Scan::Due(progress) if row.active => {
                row.state = Scan::Busy(progress);
                calls::queue(
                    &mut d.calls,
                    Owner::Repository(*key),
                    *key,
                    Op::Read(Read::Items { since: progress.since, page: progress.page, kind: None }),
                    if row.fresh == Fresh::Pending { Priority::Fresh } else { Priority::Keep },
                );
                return true;
            }
            Scan::Due(_) | Scan::Idle | Scan::Busy(_) | Scan::Waiting(_) => {}
        }
    }
    for key in keys(&d.keep) {
        let row = d.keep.live.get_mut(&key).expect("bounded key remains");
        let key = &key;
        match row.state {
            State::Due(phase, priority) if row.active => {
                // Repository watches use only the repository listing.
                match key.what {
                    What::Repository => {
                        row.state = State::Idle;
                        continue;
                    }
                    What::Pull(_) | What::Issue(_) | What::Branch(_) => {}
                }
                prepare_inbox(row, phase, priority, out);
                let read = operation(row, key, phase);
                row.state = State::Busy(phase, priority);
                calls::queue(&mut d.calls, Owner::Resource(key.clone()), key.repository, Op::Read(read), priority);
                return true;
            }
            State::Due(_, _) | State::Idle | State::Busy(_, _) | State::Waiting(_, _) => {}
        }
    }
    false
}
fn prepare_inbox(row: &mut Live, phase: Phase, priority: Priority, out: &mut Queue<Request>) {
    match phase {
        Phase::Object if row.record.watch.participating => {
            if !row.record.comment_scanning {
                if priority == Priority::Slow || row.inbox.needs_repair {
                    row.record.comment_after = 0;
                    row.record.comment_repair = true;
                }
                row.record.comment_scanning = true;
                row.record.comment_page = 1;
                row.inbox.needs_repair = false;
                out.push(Request::Save { record: Stored::Live(row.record.clone()) });
            }
        }
        Phase::Object | Phase::Pull | Phase::Reviews(_) | Phase::Branch => {}
    }
}
fn operation(row: &Live, key: &Resource, phase: Phase) -> Read {
    match phase {
        Phase::Object => match key.what {
            What::Pull(number) | What::Issue(number) => Read::Item {
                number,
                after: if row.record.watch.participating { row.record.comment_after } else { u64::MAX },
            },
            What::Repository | What::Branch(_) => unreachable!("item stage"),
        },
        Phase::Pull => match key.what {
            What::Pull(number) => Read::Pull { number },
            What::Repository | What::Issue(_) | What::Branch(_) => unreachable!("pull stage"),
        },
        Phase::Reviews(page) => match key.what {
            What::Pull(number) => Read::Reviews { number, page },
            What::Repository | What::Issue(_) | What::Branch(_) => unreachable!("reviews stage"),
        },
        Phase::Branch => match &key.what {
            What::Branch(branch) => Read::Branch { branch: branch.clone() },
            What::Repository | What::Issue(_) | What::Pull(_) => unreachable!("branch stage"),
        },
    }
}
pub(crate) fn due_resource(d: &mut Domain, env: &Env<Limits>, alarm: u64) {
    let mut found = None;
    for (key, row) in &d.keep.live {
        if row.alarm == alarm {
            found = Some(key.clone());
            break;
        }
    }
    let key = found.expect("alarm owns resource");
    let key = &key;
    let row = d.keep.live.get_mut(key).expect("alarm owns resource");
    row.state = match row.state {
        State::Waiting(phase, priority) => State::Due(phase, priority),
        State::Idle => {
            State::Due(first(key), if env.now >= row.inbox.repair_at { Priority::Slow } else { Priority::Keep })
        }
        State::Due(_, _) | State::Busy(_, _) => unreachable!("idle resource alarm"),
    };
}
pub(crate) fn due_repository(d: &mut Domain, key: Repository) {
    let row = d.keep.repos.get_mut(&key).expect("alarm owns repository");
    row.state = match row.state {
        Scan::Waiting(progress) => Scan::Due(progress),
        Scan::Idle => Scan::Due(Progress::first(row.record.mark)),
        Scan::Due(_) | Scan::Busy(_) => unreachable!("idle repository alarm"),
    };
}
pub(crate) fn writer(d: &mut Domain, env: &Env<Limits>, key: &Resource, taken: bool) {
    if !valid_resource(key, &env.limits) {
        d.fact(Fact::Refused);
        return;
    }
    if let Some(row) = d.keep.live.get_mut(key) {
        if row.writer && !taken && row.active {
            match row.state {
                State::Idle | State::Waiting(_, _) => {
                    d.alarms.cancel(Alarm::Resource(row.alarm));
                    row.state = State::Due(first(key), Priority::Fresh);
                }
                State::Due(_, _) | State::Busy(_, _) => row.hinted = true,
            }
        }
        row.writer = taken;
    }
}
pub(crate) fn pushed(d: &mut Domain, env: &Env<Limits>, key: &Resource, commit: Commit, out: &mut Queue<Request>) {
    if !valid_resource(key, &env.limits) {
        d.fact(Fact::Refused);
        return;
    }
    if let Some(row) = d.keep.live.get_mut(key)
        && row.active
    {
        match key.what {
            What::Branch(_) => {}
            What::Repository | What::Pull(_) | What::Issue(_) => return,
        }
        row.record.pushed = Some(commit);
        row.record.cached.tip = Some(commit);
        out.push(Request::Save { record: Stored::Live(row.record.clone()) });
    }
}
pub(crate) fn hint(d: &mut Domain, env: &Env<Limits>, hint: api::Hint) {
    let valid = match &hint.change {
        Change::Branch(name) => name.len() <= usize::try_from(env.limits.op_bytes).expect("u32 fits usize"),
        Change::Item(_) | Change::Commit(_) => true,
    };
    if !valid {
        d.fact(Fact::Refused);
        return;
    }
    match &hint.key {
        Some(key) if key.len() > usize::try_from(env.limits.op_bytes).expect("u32 fits usize") => {
            d.fact(Fact::Refused);
            return;
        }
        // Hint keys carry no authenticated author; a copied marker must
        // still cause a read so the returned row can establish provenance.
        Some(_) | None => {}
    }
    match &hint.change {
        Change::Branch(name) => {
            for (key, row) in &d.keep.live {
                if key.repository == hint.repository && row.active && row.writer {
                    match &key.what {
                        What::Branch(branch) if branch == name => return,
                        What::Branch(_) | What::Repository | What::Pull(_) | What::Issue(_) => {}
                    }
                }
            }
        }
        Change::Item(_) | Change::Commit(_) => {}
    }
    for key in keys(&d.keep) {
        let row = d.keep.live.get_mut(&key).expect("bounded key remains");
        let key = &key;
        if key.repository != hint.repository || !row.active {
            continue;
        }
        let affected = affected(row, key, &hint.change);
        if affected && !row.writer {
            match hint.change {
                Change::Item(_) => row.inbox.needs_repair = true,
                Change::Branch(_) | Change::Commit(_) => {}
            }
            nudge_resource(&mut d.alarms, env, key, row);
        }
    }
    if let Some(row) = d.keep.repos.get_mut(&hint.repository)
        && row.active
    {
        match row.state {
            Scan::Idle => {
                row.state = Scan::Waiting(Progress::first(row.record.mark));
                d.alarms
                    .arm(Alarm::Repository(hint.repository), env.now.saturating_add(env.limits.hinted))
                    .expect("one repository alarm");
            }
            Scan::Due(_) | Scan::Busy(_) | Scan::Waiting(_) => row.hinted = true,
        }
    }
}
fn affected(row: &Live, key: &Resource, change: &Change) -> bool {
    match change {
        Change::Item(number) => match key.what {
            What::Pull(n) | What::Issue(n) => n == *number,
            What::Repository => true,
            What::Branch(_) => false,
        },
        Change::Branch(name) => match &key.what {
            What::Branch(branch) => branch == name,
            What::Pull(_) => match &row.record.cached.pull {
                Some(pull) => pull.head == *name || pull.base == *name,
                None => true,
            },
            What::Repository => true,
            What::Issue(_) => false,
        },
        Change::Commit(commit) => match key.what {
            What::Pull(_) => match &row.record.cached.pull {
                Some(pull) => pull.commit == *commit || pull.base_commit == Some(*commit),
                None => true,
            },
            What::Branch(_) => row.record.cached.tip == Some(*commit),
            What::Repository => true,
            What::Issue(_) => false,
        },
    }
}
fn nudge_resource(alarms: &mut skein_lib::Deadlines<Alarm>, env: &Env<Limits>, key: &Resource, row: &mut Live) {
    row.interval = env.limits.poll;
    match row.state {
        State::Idle => {
            row.state = State::Waiting(first(key), Priority::Keep);
            alarms
                .arm(Alarm::Resource(row.alarm), env.now.saturating_add(env.limits.hinted))
                .expect("one resource alarm");
        }
        State::Due(_, _) | State::Busy(_, _) | State::Waiting(_, _) => row.hinted = true,
    }
}
pub(crate) fn answered_repository(
    d: &mut Domain,
    env: &Env<Limits>,
    key: Repository,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) {
    let row = d.keep.repos.get(&key).expect("call owns repository");
    if !row.active {
        d.keep.repos.remove(&key).expect("inactive repository exists");
        return;
    }
    let mut progress = match row.state {
        Scan::Busy(progress) => progress,
        Scan::Idle | Scan::Due(_) | Scan::Waiting(_) => unreachable!("repository terminal while busy"),
    };
    let Ok(answer) = result else {
        d.keep.repos.get_mut(&key).expect("repository remains").state = Scan::Waiting(progress);
        d.alarms.arm(Alarm::Repository(key), env.now.saturating_add(env.limits.backoff)).expect("one repository alarm");
        return;
    };
    match answer {
        Answer::Items { items, more, now } => {
            let row = d.keep.repos.get_mut(&key).expect("repository remains");
            row.record.clock = row.record.clock.max(now);
            let begun = progress.begun.unwrap_or(now);
            progress.begun = Some(begun);
            for item in &items {
                progress.newest = progress.newest.max(item.updated);
                progress.moved |= item.updated >= begun;
                listed(d, env, key, item, now, out);
            }
            let row = d.keep.repos.get_mut(&key).expect("repository remains");
            if more && !progress.bootstrap {
                match items.last() {
                    Some(last) if last.updated > progress.since => {
                        progress.since = last.updated;
                        progress.page = 1;
                    }
                    Some(_) | None => {
                        progress.tied = Some(progress.tied.unwrap_or(progress.since));
                        progress.page = match progress.page.checked_add(1) {
                            Some(page) => page,
                            None => {
                                row.state = Scan::Waiting(Progress::first(row.record.mark));
                                d.alarms
                                    .arm(Alarm::Repository(key), env.now.saturating_add(env.limits.backoff))
                                    .expect("one repository alarm");
                                return;
                            }
                        };
                    }
                }
                row.state = Scan::Due(progress);
            } else {
                row.record.mark = Some(if progress.bootstrap {
                    now
                } else if progress.moved {
                    progress.tied.unwrap_or(progress.newest)
                } else {
                    progress.newest
                });
                row.state = Scan::Idle;
                row.fresh = Fresh::Complete;
                let interval = if row.hinted { env.limits.hinted } else { env.limits.poll };
                row.hinted = false;
                d.alarms.arm(Alarm::Repository(key), env.now.saturating_add(interval)).expect("one repository alarm");
            }
            out.push(Request::Save { record: Stored::Repository(row.record) });
        }
        Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::PullFiles { .. }
        | Answer::Compare { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Branch(_)
        | Answer::Done => unreachable!("listing answer shape"),
    }
}
fn listed(
    d: &mut Domain,
    env: &Env<Limits>,
    repo: Repository,
    item: &api::Summary,
    now: Time,
    out: &mut Queue<Request>,
) {
    for key in keys(&d.keep) {
        let row = d.keep.live.get_mut(&key).expect("bounded key remains");
        let key = &key;
        if key.repository != repo || !row.active {
            continue;
        }
        let number = match key.what {
            What::Pull(number) | What::Issue(number) => number,
            What::Repository | What::Branch(_) => continue,
        };
        // Inclusive pages can repeat a time; reread an equal timestamp too.
        // The hint/cursor and response equality suppress duplicate news.
        if item.number == number
            && (item.updated > row.record.listed
                || row.record.cached.item.as_ref() != Some(item)
                || item.updated >= now)
        {
            row.record.listed = item.updated;
            out.push(Request::Save { record: Stored::Live(row.record.clone()) });
            nudge_resource(&mut d.alarms, env, key, row);
        }
    }
}
pub(crate) fn answered_resource(
    d: &mut Domain,
    env: &Env<Limits>,
    key: &Resource,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) {
    let row = d.keep.live.get(key).expect("call owns resource");
    if !row.active {
        d.keep.live.remove(key).expect("inactive resource exists");
        return;
    }
    let phase = match row.state {
        State::Busy(phase, _) => phase,
        State::Idle | State::Due(_, _) | State::Waiting(_, _) => unreachable!("resource terminal while busy"),
    };
    let answer = match result {
        Ok(answer) => answer,
        Err(error) => {
            failed_resource(d, env, key, phase, error, out);
            return;
        }
    };
    match answer {
        Answer::Item { item, comments, more } => item_answered(d, env, key, item, comments, more, out),
        Answer::Pull(pull) => {
            let row = d.keep.live.get_mut(key).expect("resource remains");
            let changed = row.record.cached.pull.as_ref() != Some(&pull);
            row.record.cached.pull = Some(pull.clone());
            out.push(Request::Save { record: Stored::Live(row.record.clone()) });
            if changed {
                row.interval = env.limits.poll;
                out.push(Request::Changed { resource: key.clone(), result: Ok(Answer::Pull(pull)) });
            }
            if row.record.watch.participating && row.inbox.reviews_needed {
                row.state = State::Due(Phase::Reviews(row.record.review_page), priority(row));
            } else {
                rest_resource(d, env, key);
            }
        }
        Answer::Reviews { reviews, more } => reviews_answered(d, env, key, phase, reviews, more, out),
        Answer::Commit(commit) => branch_answered(d, env, key, commit, out),
        Answer::Items { .. }
        | Answer::Branches(_)
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::PullFiles { .. }
        | Answer::Compare { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Branch(_)
        | Answer::Done => unreachable!("resource operation answer shape"),
    }
}
fn item_answered(
    d: &mut Domain,
    env: &Env<Limits>,
    key: &Resource,
    item: api::Summary,
    comments: Box<[api::Comment]>,
    more: bool,
    out: &mut Queue<Request>,
) {
    let row = d.keep.live.get(key).expect("resource remains");
    let after = row.record.comment_after;
    let participating = row.record.watch.participating;
    if more && participating && row.record.comment_page >= env.limits.inbox {
        failed_resource(d, env, key, Phase::Object, Error::TooLarge, out);
        return;
    }
    let mut stamps = copied_stamps(&row.record.comments, env.limits.inbox);
    if participating {
        for comment in &comments {
            if !remember(&mut stamps, Delivery { id: comment.id, revision: comment.revision }) {
                failed_resource(d, env, key, Phase::Object, Error::TooLarge, out);
                return;
            }
        }
    }
    let mut last = after;
    let mut delivered = List::with_capacity(env.limits.rows);
    if participating {
        for comment in comments {
            last = last.max(comment.id);
            if unseen(&row.record.comments, Delivery { id: comment.id, revision: comment.revision })
                && (known_id(&row.record.comments, comment.id)
                    || !original(comment.provenance)
                    || (!is_echo(&row.record.echoes, Echo::Comment(comment.id))
                        && !key_echo(d, key.repository, comment.author, comment.key.as_deref())))
            {
                delivered.push(comment).expect("answer rows checked before retention");
            }
        }
    }
    if more && participating && last <= after {
        failed_resource(d, env, key, Phase::Object, Error::InvalidAnswer, out);
        return;
    }
    let row = d.keep.live.get_mut(key).expect("resource remains");
    let changed = row.record.cached.item.as_ref() != Some(&item);
    row.record.position.comment = row.record.position.comment.max(last);
    row.record.comment_after = if more && participating { last } else { row.record.position.comment };
    row.record.comment_page = if more && participating {
        row.record.comment_page.checked_add(1).expect("scan page admitted below cap")
    } else {
        1
    };
    row.record.comment_scanning = more && participating;
    row.inbox.reviews_needed = changed || row.record.comment_repair || row.record.review_page > 1;
    if !more {
        if row.record.comment_repair || row.record.cached.item.is_none() || priority(row) == Priority::Slow {
            row.inbox.repair_at = env.now.saturating_add(env.limits.slow);
        }
        row.record.comment_repair = false;
    }
    row.record.comments = stamps.into_boxed();
    row.record.cached.item = Some(item.clone());
    prune_comments(row, last);
    out.push(Request::Save { record: Stored::Live(row.record.clone()) });
    if changed || !delivered.is_empty() {
        out.push(Request::Changed {
            resource: key.clone(),
            result: Ok(Answer::Item { item, comments: delivered.into_boxed(), more }),
        });
    }
    if more && participating {
        row.state = State::Due(Phase::Object, priority(row));
        return;
    }
    match key.what {
        What::Pull(_) => row.state = State::Due(Phase::Pull, priority(row)),
        What::Issue(_) => rest_resource(d, env, key),
        What::Repository | What::Branch(_) => unreachable!("item resource"),
    }
}
fn reviews_answered(
    d: &mut Domain,
    env: &Env<Limits>,
    key: &Resource,
    phase: Phase,
    reviews: Box<[api::Review]>,
    more: bool,
    out: &mut Queue<Request>,
) {
    let page = match phase {
        Phase::Reviews(page) => page,
        Phase::Object | Phase::Pull | Phase::Branch => unreachable!("reviews stage"),
    };
    if more && page >= env.limits.inbox {
        failed_resource(d, env, key, phase, Error::TooLarge, out);
        return;
    }
    let next = if more {
        match page.checked_add(1) {
            Some(next) => next,
            None => {
                failed_resource(d, env, key, phase, Error::TooLarge, out);
                return;
            }
        }
    } else {
        1
    };
    let row = d.keep.live.get(key).expect("resource remains");
    let mut stamps = copied_stamps(&row.record.reviews, env.limits.inbox);
    for review in &reviews {
        if !remember(&mut stamps, Delivery { id: review.id, revision: review.revision }) {
            failed_resource(d, env, key, phase, Error::TooLarge, out);
            return;
        }
    }
    let mut delivered = List::with_capacity(env.limits.rows);
    for review in reviews {
        let row = d.keep.live.get(key).expect("resource remains");
        let fresh = unseen(&row.record.reviews, Delivery { id: review.id, revision: review.revision });
        let echo = !known_id(&row.record.reviews, review.id)
            && original(review.provenance)
            && (is_echo(&row.record.echoes, Echo::Review(review.id))
                || key_echo(d, key.repository, review.author, review.key.as_deref()));
        let row = d.keep.live.get_mut(key).expect("resource remains");
        remove_echo(row, Echo::Review(review.id));
        row.record.position.review = row.record.position.review.max(review.id);
        if fresh && !echo {
            delivered.push(review).expect("bounded answer rows");
        }
    }
    let row = d.keep.live.get_mut(key).expect("resource remains");
    row.record.reviews = stamps.into_boxed();
    row.record.review_page = next;
    if more {
        row.state = State::Due(Phase::Reviews(next), priority(row));
    }
    out.push(Request::Save { record: Stored::Live(row.record.clone()) });
    if !delivered.is_empty() {
        out.push(Request::Changed {
            resource: key.clone(),
            result: Ok(Answer::Reviews { reviews: delivered.into_boxed(), more }),
        });
    }
    if !more {
        row.inbox.reviews_needed = false;
        rest_resource(d, env, key);
    }
}
fn valid_deliveries(stamps: &[Delivery], cap: u32) -> bool {
    if stamps.len() > usize::try_from(cap).expect("u32 fits usize") {
        return false;
    }
    for (index, stamp) in stamps.iter().enumerate() {
        for previous in stamps.get(..index).expect("enumerated stamp index") {
            if previous.id == stamp.id {
                return false;
            }
        }
    }
    true
}
fn copied_stamps(stamps: &[Delivery], cap: u32) -> List<Delivery> {
    let mut copied = List::with_capacity(cap);
    for stamp in stamps {
        copied.push(*stamp).expect("restored stamp table bounded");
    }
    copied
}
fn original(provenance: api::Provenance) -> bool {
    match provenance {
        api::Provenance::Original => true,
        api::Provenance::Revised | api::Provenance::Unknown => false,
    }
}
fn known_id(stamps: &[Delivery], wanted: u64) -> bool {
    for stamp in stamps {
        if stamp.id == wanted {
            return true;
        }
    }
    false
}
fn unseen(stamps: &[Delivery], wanted: Delivery) -> bool {
    for stamp in stamps {
        if *stamp == wanted {
            return false;
        }
    }
    true
}
fn remember(stamps: &mut List<Delivery>, wanted: Delivery) -> bool {
    for index in 0..stamps.len() {
        let stamp = stamps.get_mut(index).expect("bounded stamp index");
        if stamp.id == wanted.id {
            stamp.revision = wanted.revision;
            return true;
        }
    }
    stamps.push(wanted).is_ok()
}
fn branch_answered(d: &mut Domain, env: &Env<Limits>, key: &Resource, commit: Commit, out: &mut Queue<Request>) {
    let row = d.keep.live.get_mut(key).expect("resource remains");
    let changed = row.record.cached.tip != Some(commit);
    if !row.writer
        && let Some(expected) = row.record.pushed
        && expected != commit
        && changed
    {
        out.push(Request::Drift { resource: key.clone(), expected, observed: commit });
    }
    row.record.cached.tip = Some(commit);
    out.push(Request::Save { record: Stored::Live(row.record.clone()) });
    if changed && !row.writer {
        out.push(Request::Changed { resource: key.clone(), result: Ok(Answer::Commit(commit)) });
    }
    rest_resource(d, env, key);
}
fn failed_resource(
    d: &mut Domain,
    env: &Env<Limits>,
    key: &Resource,
    phase: Phase,
    error: Error,
    out: &mut Queue<Request>,
) {
    out.push(Request::Changed { resource: key.clone(), result: Err(error) });
    let row = d.keep.live.get_mut(key).expect("resource remains");
    if error == Error::Missing {
        row.fresh = Fresh::Complete;
    }
    row.state = State::Waiting(phase, Priority::Slow);
    d.alarms.arm(Alarm::Resource(row.alarm), env.now.saturating_add(env.limits.backoff)).expect("one resource alarm");
}
fn rest_resource(d: &mut Domain, env: &Env<Limits>, key: &Resource) {
    let row = d.keep.live.get_mut(key).expect("resource remains");
    row.fresh = Fresh::Complete;
    let interval = if row.hinted {
        env.limits.hinted
    } else {
        match key.what {
            What::Pull(_) => {
                let interval = row.interval;
                row.interval = interval.saturating_mul(2).min(env.limits.poll_max);
                interval.min(env.limits.slow)
            }
            What::Branch(_) | What::Issue(_) | What::Repository => env.limits.slow,
        }
    };
    row.hinted = false;
    row.state = State::Idle;
    let next = env.now.saturating_add(interval);
    let next = if row.inbox.repair_at > env.now { next.min(row.inbox.repair_at) } else { next };
    d.alarms.arm(Alarm::Resource(row.alarm), next).expect("one resource alarm");
}
fn is_echo(echoes: &[Echo], wanted: Echo) -> bool {
    for echo in echoes {
        if *echo == wanted {
            return true;
        }
    }
    false
}
fn key_echo(d: &Domain, repo: Repository, author: u64, key: Option<&[u8]>) -> bool {
    if !crate::identity::writer(d, repo, author) {
        return false;
    }
    match key {
        Some(key) => crate::outbox::echo(d, repo, key) || crate::identity::historical(d, key),
        None => false,
    }
}
fn remove_echo(row: &mut Live, removed: Echo) {
    if !is_echo(&row.record.echoes, removed) {
        return;
    }
    let mut remaining = List::with_capacity(u32::try_from(row.record.echoes.len()).expect("bounded echoes"));
    for echo in &row.record.echoes {
        if *echo != removed {
            remaining.push(*echo).expect("bounded echoes");
        }
    }
    row.record.echoes = remaining.into_boxed();
}
fn prune_comments(row: &mut Live, last: u64) {
    let mut remaining = List::with_capacity(u32::try_from(row.record.echoes.len()).expect("bounded echoes"));
    for echo in &row.record.echoes {
        match echo {
            Echo::Comment(id) if *id <= last => {}
            Echo::Comment(_) | Echo::Review(_) => {
                remaining.push(*echo).expect("bounded echoes");
            }
        }
    }
    row.record.echoes = remaining.into_boxed();
}
fn entry_resource(entry: &Entry) -> Option<Resource> {
    let what = match entry.effect.write {
        api::Write::Post { number, .. } => What::Issue(number),
        api::Write::Review { number, .. } => What::Pull(number),
        api::Write::CreateIssue { .. }
        | api::Write::OpenPull { .. }
        | api::Write::Edit { .. }
        | api::Write::SetReviewers { .. }
        | api::Write::Close { .. }
        | api::Write::Reopen { .. }
        | api::Write::Merge { .. }
        | api::Write::Update { .. }
        | api::Write::Status { .. }
        | api::Write::CreateBranch { .. }
        | api::Write::DeleteBranch { .. } => return None,
    };
    Some(Resource { repository: entry.repository, what })
}
fn live_for_entry(k: &Keep, entry: &Entry) -> Option<Resource> {
    let mut resource = entry_resource(entry)?;
    if k.live.get(&resource).is_some() {
        return Some(resource);
    }
    match resource.what {
        What::Issue(number) => resource.what = What::Pull(number),
        What::Pull(_) | What::Branch(_) | What::Repository => return None,
    }
    if k.live.get(&resource).is_some() { Some(resource) } else { None }
}
pub(crate) fn position(k: &Keep, entry: &Entry) -> Position {
    match live_for_entry(k, entry) {
        Some(key) => k.live.get(&key).expect("resource exists").record.position,
        None => Position { at: Time::ZERO, comment: 0, review: 0, review_page: 1 },
    }
}
pub(crate) fn echo_room(k: &Keep, entry: &Entry) -> bool {
    match live_for_entry(k, entry) {
        Some(key) => {
            let row = k.live.get(&key).expect("resource exists");
            !row.active
                || !row.record.watch.participating
                || row.record.echoes.len() < usize::try_from(row.echo_capacity).expect("u32 fits usize")
        }
        None => true,
    }
}
pub(crate) fn made(d: &mut Domain, entry: &Entry, made: crate::Made, out: &mut Queue<Request>) {
    let Some(key) = live_for_entry(&d.keep, entry) else {
        return;
    };
    let row = d.keep.live.get_mut(&key).expect("resource exists");
    if !row.active || !row.record.watch.participating {
        return;
    }
    let echo = match made {
        crate::Made::Commented(id) => Echo::Comment(id),
        crate::Made::Reviewed(id) => Echo::Review(id),
        crate::Made::Created(_)
        | crate::Made::Merged(_)
        | crate::Made::Updated(_)
        | crate::Made::Branch(_)
        | crate::Made::Set => return,
    };
    if !is_echo(&row.record.echoes, echo) {
        let capacity = u32::try_from(row.record.echoes.len())
            .expect("bounded echoes")
            .checked_add(1)
            .expect("admitted echo capacity");
        let mut echoes = List::with_capacity(capacity);
        for value in &row.record.echoes {
            echoes.push(*value).expect("bounded echoes");
        }
        echoes.push(echo).expect("admitted echo capacity");
        row.record.echoes = echoes.into_boxed();
        out.push(Request::Save { record: Stored::Live(row.record.clone()) });
    }
    row.hinted = true;
}

fn keys(k: &Keep) -> Box<[Resource]> {
    let mut keys = List::with_capacity(k.live.len());
    for (key, _) in &k.live {
        keys.push(key.clone()).expect("bounded live keys");
    }
    keys.into_boxed()
}
fn repos(k: &Keep) -> Box<[Repository]> {
    let mut keys = List::with_capacity(k.repos.len());
    for (key, _) in &k.repos {
        keys.push(*key).expect("bounded repository keys");
    }
    keys.into_boxed()
}
fn next_id(sequence: &mut u64) -> u64 {
    *sequence = sequence.checked_add(1).expect("alarm identities do not exhaust in a process");
    *sequence
}

fn priority(row: &Live) -> Priority {
    match row.state {
        State::Due(_, priority) | State::Busy(_, priority) | State::Waiting(_, priority) => priority,
        State::Idle => unreachable!("a pipeline advances while active"),
    }
}
pub(crate) fn active_resource(k: &Keep, key: &Resource) -> bool {
    match k.live.get(key) {
        Some(row) => row.active,
        None => false,
    }
}
pub(crate) fn active_repository(k: &Keep, key: Repository) -> bool {
    match k.repos.get(&key) {
        Some(row) => row.active,
        None => false,
    }
}

fn valid_participation(watch: &Watch) -> bool {
    match watch.resource.what {
        What::Pull(_) | What::Issue(_) => true,
        What::Repository | What::Branch(_) => !watch.participating,
    }
}
