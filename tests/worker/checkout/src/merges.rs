//! Merge scenarios against the real checkout child, fake disk and fake forge.
//! The referee predicts complete trees, parent pairs and remote refs from
//! the scenario; it never reads the domain's private state.
use crate::{forge::Forge, translate};
use skein_lib::{Duration, Env, Queue, Time, Token, Wall};
use temper_fake_checkout::{Checkout, git::Remote};
use temper_worker_domain_checkout::git::{Done, Fault, Op};
use temper_worker_domain_checkout::{
    Domain, Event, Landing, Limits, MAX_OUT, Message, Outcome, Prepared, Repository, Request, Spec, Start, step,
};

#[derive(Clone, Copy, Debug)]
pub enum Case {
    Clean,
    Conflict,
    Unchanged,
    Moved,
    Ambiguous,
    Saved,
}

#[derive(PartialEq, Eq, Debug)]
pub struct Report {
    trace: Vec<String>,
    tree: temper_fake_checkout::git::Tree,
    parents: (u64, u64),
}

struct World {
    domain: Domain,
    env: Env<Limits>,
    out: Queue<Request>,
    forge: Forge,
    disk: Checkout,
    hold: Option<Token>,
    trace: Vec<String>,
    expected: temper_fake_checkout::git::Tree,
    ours: u64,
    base: u64,
}

impl World {
    fn new(seed: u64, case: Case, facts: u32) -> Self {
        let limits = Limits {
            workspaces: 1,
            repositories: 1,
            name_bytes: 64,
            message_bytes: 128,
            conflicts: 4,
            path_bytes: 128,
            remote_timeout: Duration::from_secs(10),
            local_timeout: Duration::from_secs(5),
            facts,
        };
        let mut forge = Forge::new(seed);
        let first = forge.repository(b"forge/temper", b"main", [(b"code".to_vec(), b"original\n".to_vec())].into());
        forge.create_branch(b"forge/temper", b"topic", first).expect("topic created");
        let left = format!("ours {seed}\n");
        let right = format!("base {seed}\n");
        let (ours, base, expected) = match case {
            Case::Unchanged | Case::Moved => {
                let base = forge.advance(b"forge/temper", b"main", b"code", right.as_bytes());
                (first, base, [(b"code".to_vec(), right.into_bytes())].into())
            }
            Case::Conflict => {
                let ours = forge.advance(b"forge/temper", b"topic", b"code", left.as_bytes());
                let base = forge.advance(b"forge/temper", b"main", b"code", right.as_bytes());
                (ours, base, [(b"code".to_vec(), b"resolved\n".to_vec())].into())
            }
            Case::Clean | Case::Ambiguous | Case::Saved => {
                let ours = forge.advance(b"forge/temper", b"topic", b"ours", left.as_bytes());
                let base = forge.advance(b"forge/temper", b"main", b"theirs", right.as_bytes());
                (
                    ours,
                    base,
                    [
                        (b"code".to_vec(), b"original\n".to_vec()),
                        (b"ours".to_vec(), left.into_bytes()),
                        (b"theirs".to_vec(), right.into_bytes()),
                    ]
                    .into(),
                )
            }
        };
        Self {
            domain: Domain::new(&limits),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            out: Queue::with_capacity(MAX_OUT),
            forge,
            disk: Checkout::new(),
            hold: None,
            trace: Vec::new(),
            expected,
            ours,
            base,
        }
    }

    fn finish(&mut self, mut event: Event, mut ambiguous: bool) -> Request {
        for _ in 0..64 {
            step(&mut self.domain, &self.env, event, &mut self.out);
            let mut pending = None;
            let mut terminal = None;
            let mut count = 0;
            while let Some(request) = self.out.pop() {
                count += 1;
                self.trace.push(format!("{request:?}"));
                match request {
                    Request::Held { hold, .. } => {
                        assert!(self.hold.replace(hold).is_none());
                    }
                    Request::Io { owner, op, deadline } => {
                        assert_eq!(Some(owner), self.hold);
                        let timeout =
                            if op.is_remote() { self.env.limits.remote_timeout } else { self.env.limits.local_timeout };
                        assert_eq!(deadline, self.env.now.saturating_add(timeout));
                        assert!(pending.replace((owner, op)).is_none(), "one IO operation per hold");
                    }
                    answer @ (Request::Prepared { .. }
                    | Request::Pushed { .. }
                    | Request::Saved { .. }
                    | Request::Released { .. }) => {
                        assert!(terminal.replace(answer).is_none(), "one terminal per caller operation");
                    }
                    Request::Cancel { .. } => panic!("these scenarios have no outstanding cancellation"),
                }
            }
            assert!(count <= MAX_OUT);
            while self.domain.pop_fact().is_some() {}
            self.domain.reclaim();
            if let Some(terminal) = terminal {
                assert!(pending.is_none());
                return terminal;
            }
            let (owner, op) = pending.expect("caller waits for one IO operation");
            let pushing = matches!(op, Op::Push { .. });
            let mut done = translate::perform(&mut self.forge, &mut self.disk, op);
            if ambiguous && pushing {
                assert_eq!(done, Done::Succeeded, "the timed-out push actually landed");
                done = Done::Failed { fault: Fault::TimedOut };
                ambiguous = false;
            }
            event = Event::Done { owner, done };
        }
        panic!("bounded scenario settles");
    }

    fn push(&mut self, saved: bool, ambiguous: bool) -> Box<[Landing]> {
        let hold = self.hold.expect("prepared");
        let message =
            Message { title: b"Merge checked tree".as_slice().into(), body: b"Keep both parents.".as_slice().into() };
        let event = if saved {
            Event::Save { hold, branch: b"saved".as_slice().into(), message }
        } else {
            Event::Push { hold, message }
        };
        match self.finish(event, ambiguous) {
            Request::Pushed { outcome: Outcome::Pushed { landings }, .. } if !saved => landings,
            Request::Saved { outcome: Outcome::Pushed { landings }, .. } if saved => landings,
            other @ (Request::Held { .. }
            | Request::Prepared { .. }
            | Request::Pushed { .. }
            | Request::Saved { .. }
            | Request::Released { .. }
            | Request::Io { .. }
            | Request::Cancel { .. }) => panic!("push/save terminal: {other:?}"),
        }
    }
}

/// Run one complete scenario, checking every promised terminal and observable
/// tree/ref against its independent prediction. Seed changes file content.
#[must_use]
pub fn run(seed: u64, case: Case, facts: u32) -> Report {
    let mut world = World::new(seed, case, facts);
    let spec = Spec {
        key: b"merge".as_slice().into(),
        repositories: Box::new([Repository {
            name: b"temper".as_slice().into(),
            remote: b"forge/temper".as_slice().into(),
            start: Start::Merge { branch: b"topic".as_slice().into(), base: translate::commit(world.base) },
            identity: 0,
            push: Some(b"topic".as_slice().into()),
            expected: Some(translate::commit(world.ours)),
        }]),
    };
    world.forge.moves();
    let Request::Prepared { prepared: Prepared::Ready { workspace, conflicts }, .. } =
        world.finish(Event::Prepare { client: Token::new(9), spec }, false)
    else {
        panic!("prepared")
    };
    assert!(world.forge.moves().is_empty(), "preparation moves no remote refs");
    let at = translate::path(&temper_worker_domain_checkout::git::Place {
        workspace,
        repository: b"temper".as_slice().into(),
    });
    if matches!(case, Case::Conflict) {
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].repository, 0);
        assert_eq!(&*conflicts[0].files, &[Box::<[u8]>::from(&b"code"[..])]);
        assert_eq!(&*world.push(false, false), &[Landing::Conflicted { files: Box::new([b"code".as_slice().into()]) }]);
        assert!(world.forge.moves().is_empty(), "remaining markers make no remote effect");
        world.disk.write(&[&at[..], b"/code"].concat(), b"resolved\n");
    } else {
        assert!(conflicts.is_empty());
    }
    if matches!(case, Case::Moved) {
        assert_eq!(
            world.forge.push(b"forge/temper", b"topic", world.base, None).expect("peer fast-forward"),
            temper_fake_checkout::git::Pushed::Pushed
        );
    }
    if matches!(case, Case::Saved) {
        let saved = world.push(true, false);
        let Landing::Landed { commit } = saved[0] else { panic!("saved") };
        assert_eq!(world.forge.branch(b"forge/temper", b"saved"), Some(translate::fake(commit)));
        assert_eq!(world.forge.branch(b"forge/temper", b"topic"), Some(world.ours));
    }
    let landed = world.push(false, matches!(case, Case::Ambiguous));
    let commit = if matches!(case, Case::Moved) {
        assert_eq!(&*landed, &[Landing::Moved]);
        assert_eq!(world.forge.branch(b"forge/temper", b"topic"), Some(world.base));
        None
    } else {
        let Landing::Landed { commit } = landed[0] else { panic!("landed: {landed:?}") };
        Some(translate::fake(commit))
    };
    if let Some(commit) = commit {
        assert_eq!(world.forge.parent(commit), Some(world.ours));
        assert_eq!(world.forge.merge_parent(commit), Some(world.base));
        assert_eq!(world.forge.tree(commit), world.expected);
        assert_eq!(world.forge.branch(b"forge/temper", b"topic"), Some(commit));
        assert_eq!(&*world.push(false, false), &[Landing::Unchanged], "repeat creates no new commit or push");
    }
    let hold = world.hold.expect("held");
    assert!(matches!(world.finish(Event::Release { hold }, false), Request::Released { .. }));
    assert_eq!(world.domain.holds(), 0);
    Report { trace: world.trace, tree: world.expected, parents: (world.ours, world.base) }
}
