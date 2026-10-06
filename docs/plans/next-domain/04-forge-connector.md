# Step 04: the forge connector

Provisional, 2026-10-04. The forge as a connector (`connectors.md`,
`forge.md`): four crates, a world, and the fake forge grown to what they
need. Resources named by paths, write holds and writer slots, effects
through the outbox and found again after an uncertain failure, a working
set kept up from what changed, topics and landings as news, the change
procedure and landing queues, goals' issues as projections, adoption at
runtime. It is built beside the legacy `forge` child domain and the
change mechanics of the legacy `plan`, which it replaces at the cutover.
Overview and conventions: README.md.

## 1. The subtree

```
temper-engine-domain-forge              the top: resources, holds and slots, topics, adoption, outbox, routing
├── temper-engine-domain-forge-client   the working set, keeping up, fresh reads, writes made once, the request budget
├── temper-engine-domain-forge-change   policy: the change procedure and the landing queues
└── temper-engine-domain-forge-issues   policy: what each goal's issue projects, and when
```

`client` is a capability with state and entry points; `change` and
`issues` are policy, free functions over values the top gathers, as
`plan` is today; the top owns the procedures' and projections' states and
saves them (README.md, 5.2). Nothing below the top knows tasks beyond
their numbers; nothing above it knows the forge's API.

## 2. What it takes from the legacy code

| From | What | Into |
|---|---|---|
| `temper-legacy-engine-domain-forge`, `calls.rs` | the request budget: fresh reads first, writes, keeping up, the slow pass, a share kept for the last two; costs settled when answered; a rate refusal stopping every call until its reset | `client`, `calls.rs`, copied |
| the same, `writes.rs` | write lanes (one at a time per pull request, issue and repository); keyed creations found by their keys; sets written as sets; merges at a head; jittered retries of transient failures | `client`, `writes.rs`, adapted to outbox entries with their start positions |
| the same, `scans.rs` | listing what changed since the forge's time, least recently updated first, paged; per pull request reads on a doubling backoff; hints sooner; the slow pass | `client`, `keep.rs`, over live resources instead of labelled items |
| the same, `reads.rs`, `api.rs` | fresh reads and the forge-shaped vocabulary of calls | `client`, `reads.rs`, `api.rs`, without the wiki's, labels' and records' calls |
| `temper-legacy-engine-domain-plan`, `due.rs` | a change's mechanics: repairs and rebases bounded apart, stalls from a push, a merge at exactly its head, a merge refused for a conflict taken as a moved base, the branch read before producing again | `change`, rewritten to `forge.md`, 8.2, with these as its tests' cases |

What does not come: records and nonces, mangled records, the tracking and
hand-in listings and the probes for unlabelled records, the wiki's pages,
reading verdicts on pull requests temper owns, labels as anything but
display, and the new engine's silence for a lifetime after it starts
(now per uncertain entry, `connectors.md`, 4.3).

## 3. The crates

### 3.1 The top: `temper-engine-domain-forge`

Its vocabulary is the table of `connectors.md`, section 2, in the forge's
terms, and the forge's own protocol (calls, answers, webhooks' hints),
which the root passes through:

```rust
/// root -> forge
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// The resources a live task names, or no longer names (connectors.md, 3.4).
    Names { task: u64, resources: Box<[Name]> },
    Unnamed { task: u64 },
    /// Take a write hold for `task`, or hand one down from `from` (3.3).
    Hold { task: u64, resource: Name, from: Option<u64> },
    /// A run of `task` is claimed: take the writer slots of what it writes.
    Claim { task: u64, attempt: u64, writes: Box<[Name]> },
    /// Its answer committed: free its slots, with the heads it reports pushed.
    Answered { task: u64, attempt: u64, pushed: Box<[Pushed]> },
    /// Its attempt settled as lost: read what it held afresh, then free.
    Lost { task: u64, attempt: u64 },
    /// An outbox entry whose commit is durable: make it.
    Make { entry: u64 },
    /// Step the procedure of `task`: activated, a message, closing.
    Step { task: u64, why: Stepping },
    /// A goal's state, plan and milestones changed: project its issue.
    Project { goal: u64, goal_view: GoalView },
    /// `task` closes as `ending`: release what it held, after its other effects.
    Release { task: u64, ending: Ending },
    Subscribe { task: u64, topic: Topic },
    Unsubscribe { task: u64, topic: Topic },
    /// A run's read (connectors.md, section 8), and a brief's section.
    Read { reply_to: ReplyTo, task: u64, read: Read },
    Gather { owner: Token, task: u64, section: SectionKind, budget: u32 },
    /// The facts a requirement names, for the exact state an effect names.
    Facts { reply_to: ReplyTo, effect: Effect },
    /// A person adopts a repository into a project with a role (section 4).
    Adopt { reply_to: ReplyTo, project: u32, repository: Repository, role: Role },
    /// The forge's protocol: a call's answer, a webhook's hint.
    Answer { call: Token, cost: u32, result: Result<api::Answer, api::Error> },
    Hint { hint: api::Hint },
    Restore { record: Stored },
    Restored,
}

/// forge -> root
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// A resource's role, and what temper may do on it.
    Role { task: u64, resource: Name, role: Role, kinds: Kinds },
    /// A hold or a slot refused: held by another task, taken by another run.
    Taken { task: u64, resource: Name, by: u64 },
    /// What a procedure's step decided (connectors.md, 6.3): effects (already
    /// saved as outbox entries), batches, amendments, cancels, proposals,
    /// messages up, a hold, a result. The root checks each with authority.
    Decided { task: u64, decision: Decision },
    /// An outbox entry was made, failed, or is uncertain; or it was found.
    Outcome { entry: u64, task: u64, outcome: Outcome },
    /// News on a topic, classified for each subscriber (connectors.md, 5.3).
    News { topic: Topic, news: News, to: Box<[Classed]> },
    /// What temper owns and relies on was changed by someone else (section 10).
    Drift { task: u64, resource: Name, drift: Drift },
    Read { reply_to: ReplyTo, answer: ReadAnswer },
    Gathered { owner: Token, section: Section },
    Facts { reply_to: ReplyTo, facts: Box<[Fact]> },
    Adopted { reply_to: ReplyTo, outcome: Adoption },
    /// The forge's protocol: a call to make, answered by one `Answer`.
    Call { call: Token, forge: u16, repository: u32, op: api::Op },
    Save { record: Stored },
    Erase { key: Key },
}
```

Its state: the projects' repositories with their roles and what temper
may do there; write holds and writer slots by resource; topics and their
subscribers by task number; each change's procedure state; each goal's
projection state; the outbox entries not yet made. Its children:
`client`'s `Domain`, and nothing of `change` or `issues` but the values it
passes them.

Its names (`forge.md`, section 2):

```rust
/// A forge resource: `forge` <host> <owner> <repo>, then what it is.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Name {
    pub forge: u16,
    pub repository: u32,
    pub what: What,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum What {
    Repository,
    Branch(Box<[Box<[u8]>]>),
    Pull(u64),
    Issue(u64),
}
```

The root renders a `Name` as authority's path of segments, with the
forge's host and the repository's owner and name it keeps per adopted
repository, and gives authority the forge's kinds and their `Implies`
table (`forge.md`, section 2: `push` implies `branch`, `land` nothing).

### 3.2 `temper-engine-domain-forge-client`

Today's forge child domain without records, the wiki or labels as an
index (`forge.md`, section 19):

- **the working set** of `forge.md`, 5.1, admitted per live resource, not
  per item found by a label;
- **keeping up** (5.2), **fresh reads** (5.3), **the request budget** (5.4);
- **writes** as outbox entries: each with its key (carrying the
  deployment's id), its lane, its start position (the forge's time and the
  last item seen when it first went out), its outcome, and, when
  uncertain, its search by key, by branches or by the state it leads to,
  then a retry once its lifetime has passed (`connectors.md`, 4.3);
- **echoes** of temper's own effects dropped by key, and of its own pushes
  by writer slot (`connectors.md`, section 10).

Its vocabulary is the legacy forge's (`Event::Answered`, `Request::Call`,
the `api` module), less the wiki's, labels' and records' calls, plus the
new calls of `forge.md`, section 17.

### 3.3 `temper-engine-domain-forge-change`

```rust
/// A change's place on its way to landing (forge.md, 8.2).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum State {
    Producing { delegate: u64 },
    Opening,
    Checking { head: Commit },
    Gating { head: Commit, asked: Box<[Asked]> },
    Queued { head: Commit, ready_since: Wall },
    First { head: Commit, ready_since: Wall },
    Updating { from: Commit, base: Commit, ready_since: Wall },
    Resolving { delegate: u64, base: Commit },
    Repairing { delegate: u64, why: Repair },
    Landing { head: Commit },
    Landed { merge: Commit },
    Held { was: Was, why: Hold },
}

/// What a step reads, afresh (forge.md, 5.3): the pull request's head,
/// base and mergeability, CI on the head, the base's tip, the gates'
/// verdicts and the heads temper's clean updates led through, whether the
/// branch's writer slot is taken, the queue's first.
pub struct Facts { /* ... */ }

/// One step: its new state, and one decision (connectors.md, 6.3).
pub fn step(change: &Change, facts: &Facts, heard: &Heard, now: Wall, limits: &Limits) -> Stepped;

/// The ready changes into one branch, in order: priority, then when each
/// first became ready, with aging past the project's window (forge.md, 9).
pub fn first(ready: &[Ready], window: Duration, now: Wall) -> Option<u64>;
```

Level-triggered (`connectors.md`, 6.4): `step` decides from the facts and
its state, never from the event that woke it, and never asks for an
effect its state shows outstanding. Gates (8.3), carried or exact, are
data in its parameters, amended while live; a gate added once the merge is
in the outbox is refused as too late.

### 3.4 `temper-engine-domain-forge-issues`

```rust
/// What a goal's issue should say now, and the effects that write it, if
/// anything changed since `last` and the interval has passed (forge.md, 12).
pub fn project(goal: &GoalView, last: &Projected, now: Wall, limits: &Limits) -> Projection;
```

One issue per goal in its project's home repository; the body (the goal
and its plan as a task list) rewritten at most once per interval and
only when its digest changed; milestones as comments, each once, keyed by
the goal and the milestone; the issue closed as the goal closes.

## 4. The fake forge grows

`testing/temper-fake-forge-domain` is extended, never trimmed in this
step (README.md, 3.2); what it loses (the wiki, labels as an index) goes
in step 08. Each addition is a new `Read`, `Write` or `Git` operation,
or an optional field, with its own tests in the crate and in
`tests/fake-forge`:

| Addition | Shape |
|---|---|
| a pull request's files and diff at its head; a comparison's files and commits | `Read::PullFiles`, `Read::Compare` |
| updating a pull request from its base, by merge, refused on a conflict | `Write::Update`, answering `Error::Conflict` |
| merges with two parents, in the git it keeps | the update's commit; `Git::Push` of a merge |
| a failed job's status, link and bounded log for its exact attempt | `Read::Checks`; extend the typed job read for Forgejo v16.0.5 logs, with explicit missing, forbidden and failed outcomes |
| branch protection, repository settings (merge styles, default branch), collaborators with their permissions | `Read::Protection`, `Read::Settings`, `Read::Collaborators` |
| creating a branch at a commit through the API | `Write::CreateBranch` |
| a push refused unless its branch is at the head it expects | `Git::Push { expected: Option<u64> }`, `None` as today |
| statuses posted by temper as gates' verdicts | already `Write::Status` |

Its facts follow step 00's answers about Forgejo, not guesses: where
Forgejo answers a conflicting update in a particular way, the fake does
the same.

The target is Forgejo v16.0.5 (domain/forge.md, section 20.1): failed CI
has a status description, job link and supported API log read naming the
exact job attempt at the failing head. The domain fake returns bounded
owned log bytes or an explicit missing, forbidden or failed outcome. Reply
and repair-brief limits account for those bytes.

## 5. The world

`tests/engine/forge`, package `temper-engine-forge-world`, runs the
top with its children, the fake forge below it through the client's
translation as the world plays the protocol layer, and the root scripted
above it: live tasks naming resources, holds and slots, runs that push
(the world pushes through the fake's git, as a worker would) and answer,
delegates for producing, repairs, resolutions and reviews whose results
the world scripts, goals with plans to project, subscribers to topics,
the store, restarts.

Its stories are `forge.md`, section 18, each a focused test; among them,
those that rest on step 00's facts (a conflicting update resolved from a
merge in progress; a clean update whose CI then fails, repaired). Its
referee is `forge.md`, section 18's: nothing lands without CI and every
blocking gate valid at the exact head merged; merges land exactly the
heads decided; keyed effects made once across restarts; nothing written
to what temper does not own, nor forced; a held change makes no effect;
a landing wakes a goal exactly when it overlaps; no change waits past the
aging window behind later ones; temper's own pushes never taken for
drift; the calls made while idle bounded, the forge preloaded with ten
times the history.

`change` and `issues` have step tests besides: every cell of the change's
states over generated facts; stepping twice on the same facts deciding
the same; the queue's order against an independent statement, aging
included; a sweep of landings against the landing rule as step 01 states
it.

## 6. Increments

1. **04a the fake forge grows** (section 4), with its tests; nothing
   uses the additions yet but their own tests.
2. **04b `client`:** copied from the legacy forge, trimmed, then
   adapted to live resources and outbox entries; its own step tests,
   ported from the legacy forge's where they still hold.
3. **04c `change`:** the procedure and the queues, with their step tests.
4. **04d `issues`:** the projection, with its step tests.
5. **04e the top and its world:** resources, holds and slots, topics and
   overlap, adoption, routing; the world's calm stories.
6. **04f restarts, drift and load:** finding uncertain entries after a
   restart, drift held and released, the idle-cost test against a forge
   with ten times the history; the fuzzy sweep.

## 7. Done when

- every story and every referee rule of `forge.md`, section 18 has its
  test;
- the four crates depend on `skein-lib` and each other only as the tree
  says; the legacy forge is unchanged;
- the fake forge's additions are all used by the world.
