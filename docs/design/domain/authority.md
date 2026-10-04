# Authority

Provisional, 2026-10-04. What a task may do, and how temper holds it
there: authority as a value, its order and how delegation narrows it,
where it comes from, budgets and what runs spend, the checks every
action passes, what a proposal needs and what accepting it funds, and
the rules no task loosens. It is the fourth primitive of core.md (3.4)
in depth. Proposals travel the tree as tasks.md, section 8 says; people's
roles are people.md, section 4. What is still open is listed in
section 13.

## 1. In one page

- **Authority is a value with an order.** It names tools, effects on
  resources, what may be delegated and what may be spent. One authority
  is at most another when every part of it is: a preorder, decidable
  from the two values alone. Whether a delegate's authority **fits**
  under its creator's is that order, checked against what the creator has
  left.
- **Resources are named by paths, grants by prefixes.** Each connector
  names its resources as paths of segments; an effect is granted on
  every resource under a prefix. Prefixes order themselves, so the
  authority child domain knows no connector, and the deployment's rules
  are written once for all of them.
- **Budgets are carved, not copied.** What a task gives a delegate of
  its budget is reserved from its own, recording who funded it; what the
  delegate leaves unspent goes back to its funder when it ends, and what
  it spent is counted up the chain that funded it. Spend is counted in
  one unit of the deployment's, priced as each run spends it.
- **Every action is checked** before it commits: a batch, an effect, a
  tool call, a run starting, a person's request, an amendment, a note.
  A check answers allow, wait for facts, propose, or refuse.
- **Beyond its authority, a task proposes.** The nearest holder above it
  whose authority covers what the action needs accepts it and funds it,
  or rejects it. Acceptance, escalation and widening a plan's reach are
  one mechanism.
- **The root is the deployment's rules and the project's policy.** No
  task, and no person, loosens them: a person's authority is their
  role's, within the project's policy, within the deployment's rules.
- **Pure policy.** The authority child domain decides over values it is
  given and keeps nothing between calls but the rules and policies in
  force, so it is tested by its step tests, as today's rules are.

## 2. Structure

`temper-engine-domain-authority` is a child domain of the engine's root
(engine.md, section 3). It is policy, as `rules` is today: a decision
over data. It holds the deployment's rules and each live project's
policy, which change only by a commit (people.md, 5.2). The numbers kept
against each task's authority (what it spent, what its ended delegates
spent, what it reserved, who funded it) are the tasks child domain's
(tasks.md, section 3); the root gives them with each question, and
authority's answer says what they become, which the root gives back.
Each child domain has its own type for authority, which the root
translates.

Its questions, each a pure function of what the root gathers:

- is this authority at most that one (section 5)?
- does this batch fit under its creator, and what do the numbers become
  (8.1)?
- may this task, or this procedure, make this effect, with these facts
  (8.2)?
- may this run start, and with what budget (8.3)?
- may this person do this, in this project (8.4)?
- what does this proposal need, and which of these holders covers it
  (section 9)?

## 3. What authority is

```
Authority
  tools       the tool families an agent's run may call
  grants      effects and reads, each a connector's kind on a prefix of resources
  delegation  executor kinds and charters it may delegate to; how many tasks; how deep
  budget      what it may spend, in the deployment's unit; by when
  notes       the scopes it may write notes in
```

- **Tools:** families, as an agent's charter names them (agent.md, 4.1):
  inspecting the workspace, modifying it, the shell, sub-agents, each
  connector's reads, and the engine's tools, each its own family
  (delegate, message, amend and cancel, decide, propose, subscribe,
  effect, note, recall). A procedure has no tools; its authority's tools
  are empty.
- **Grants:** each is a connector, a kind of effect or read that
  connector defines, and a pattern of resource names (section 4): "push
  to the branches under `ai/temper` `temper/7/`", "land into `ai/temper`
  `main`", "create issues in `ai/temper`", "read anything under `ai/`".
  A task may make an effect only if some grant covers it.
- **Delegation:**
  - *kinds:* which executors its delegates may have: agents with which
    charters, which procedures, people in which roles;
  - *tasks:* how many tasks its subtree may make over its life;
  - *depth:* how deep below it its subtree may go.
- **Budget:**
  - *spend:* how much its subtree may spend;
  - *deadline:* by when it must end, in wall time, if at all.
- **Notes:** the scopes it may write in: its goal, a repository, its
  project, the deployment (engine.md, section 10). It may read every
  scope its project reaches.

## 4. Resources and their names

- **A resource's name is a path** of segments, given by its connector:
  for the forge, `forge` `<host>` `<owner>` `<repo>`, then `branch`
  followed by the branch's own segments, or `pull` and a number, or
  `issue` and a number (forge.md, section 2). Names are bytes, compared
  segment by segment.
- **A grant names a pattern:** segments matched exactly, then a last
  segment that is either exact or open, an open one covering every
  segment that begins with it, and every name beneath. `ai/temper`
  `branch` `temper` `7` and an open empty segment covers every branch
  under `temper/7/`; an open `r42-` covers `temper/7/r42-1` and
  `temper/7/r42-2`; an exact `c42` covers `temper/7/c42` and its
  descendants, not `temper/7/c420`. Both terminal forms require one more
  segment after the exact base and include descendants; a pattern with
  no terminal covers only the exact base name. Empty segments are literal
  bytes too, and an open empty terminal matches any additional segment.
  One pattern is at most another when every name the
  first covers, the second covers too, which is decided segment by
  segment.
- **temper's own branches are named after their tree** (forge.md,
  section 11): every branch of a tree is under `temper/<root>/`, so the
  authority over a tree's branches is one prefix.
- **A task's own branches are named in its batch,** symbolically: "its
  own change branch, its saved work, its runs' branches". The batch
  resolves them once the task has its number: its change branch and its
  saved work to exact names, its runs' branches to an open `r<task>-`,
  each with its grant, so a creator can narrow a delegate to its own
  branches without knowing its number.
- **Kinds are a connector's,** with an order of their own where one
  implies another: for the forge, a grant to land into a branch does not
  imply pushing to it, and pushing does not imply landing. A kind's
  order is a table the connector gives, and the authority child domain
  holds as data. Equality is implicit and the connector's table includes
  every transitive implication. Authority validates that preorder within
  the configured maximum number of pairs before admitting the table.

## 5. Order and fitting

- **The order.** Authority A is at most B when:
  - A's tools, kinds of delegate and note scopes are subsets of B's;
  - every grant of A is covered by one of B's: the same connector, a kind
    at most B's, a pattern at most B's;
  - A's tasks, depth and spend are at most B's, and A's deadline no later
    than B's (no deadline being the latest).

  It is reflexive and transitive, a preorder: two authorities with the
  same grants written differently may each be at most the other. The
  worlds check its laws (section 11).
- **Fitting.** A delegate's authority A fits under its creator B, given
  the numbers kept against B, when A is at most B with B's depth less
  one and B's tasks and spend replaced by what B has left. Fitting is
  what a batch, an amendment and an acceptance are checked against; the
  order alone is what the laws are about.
- **Narrowing** is giving a delegate an authority that fits. It is
  checked, never applied silently: a batch asking for more than fits is
  refused, naming what it lacked, and may be proposed (section 9); it is
  never trimmed to fit. What a task asks for is what it gets, or it hears
  why not.
- **What is consumed and what is not.** Tools, grants, kinds, depth,
  deadlines and scopes are shared: a delegate given a grant does not take
  it from its creator. Spend and tasks are consumed: what a delegate is
  given is reserved from its creator (section 7).

## 6. Where it comes from

```
deployment's rules       the most any project may have; requirements on effects
  project's policy       the most any task in the project may have; its roles; its landings' rules
    a person's role      what a person may give, accept and do in the project
      a root task        what its creator gave it, fitting under the creator
        its delegates    narrowed at every level
```

- **The deployment's rules** are configuration: the connectors and the
  kinds of effect any project may be granted, the deployment's spend per
  period, the charters and procedures it has, the limits. A rule may
  forbid what no project may grant at all: force-pushing does not exist,
  landing without CI on the exact head may be forbidden everywhere.
- **A project's policy** is in the store, changed by the people whose
  role allows it, within the deployment's rules:
  - its ceiling: the authority no task in the project exceeds, which
    follows from its repositories' roles (owned: push under temper's
    prefix and land into the named landing branches; fork: push into the
    fork; context: read) and from what temper was given on each at
    adoption (connectors.md, section 11);
  - its spend per period, from which every root task's budget is carved;
  - its roles: for each, the authority a person in it has, the spend a
    person in it may allot per period, and the kinds of proposal it may
    decide (people.md, section 4);
  - its landing rules: per landing branch, what a landing requires
    (section 10).
- **A person's authority** in a project is their role's, and their
  allotment is a pool per period with its own amount left. A person who
  creates a task (a chat, a goal), or accepts a proposal, funds it from
  that pool, which is itself carved from the project's spend for the
  period.
- **The deployment** is the requester of what configuration or a
  connector starts on a project's behalf (a recurring task, the repair
  of a landing branch's CI), with the authority the project's policy
  gives such tasks, funded from the project's spend.

## 7. Budgets and spend

- **Four numbers per current allotment:** its budget (what it was given),
  what it has spent itself, what the closed allotments it funded spent,
  and what it has reserved for the live allotments it funds. Normally an
  allotment closes as its task ends; moving may close and replace an
  allotment while its task stays live. What it has left is its budget
  less the other three, and never below zero. A task is a funder; so is
  each person's pool and each project's spend, per period, whose numbers
  the tasks child domain keeps too, in the store (engine.md, 5.4).
- **Every reservation records its funder:** the task above, a person's
  pool, or a project's period. A batch reserves each delegate's budget
  from its creator in the commit that makes it; an accepted proposal
  reserves from its accepter. A funding link names the actual task,
  person's pool and period, or project's period, independently of the
  requester tree. Each task has one current allotment and one recorded
  funder. The tasks child durably numbers allotment generations and closes
  each exactly once; an aggregate snapshot cannot recognize a duplicate
  closure, including a zero-budget one.
- **When a task ends,** in the commit that ends it, its reservation is
  released: its unspent budget goes back to its funder, and what it and
  the allotments it funded spent is added to its funder's "spent by closed
  allotments". Each funder passes its own on when it ends, so spend climbs
  the chain one end at a time, to the pool or period at the top, and is
  never counted twice nor lost, nor more than what funded it but by the
  overrun below. Every allotment it funds must have closed before its own
  closes. Closed allotments' historical spend remains in immutable history
  and on the old funding chain, even when current task counters reset.
- **A task moved** to a new requester (tasks.md, section 6) is funded
  anew. In one commit the root virtually closes its live funding subtree
  from the leaves upward, retaining each live task's unspent amount. It
  closes the old root allotment against its actual old funder, which keeps
  all that allotment's spend and receives what remains. The new funder
  reserves the unspent amount of the whole funding subtree. Each live
  task's replacement allotment starts with zero spend and reserves its
  directly funded tasks' retained amounts. Every promised amount must fit
  at every level or the whole move is refused: an overrun is never fixed
  by trimming a live task's budget. A top-up requires a separate authorized
  amendment or proposal. A move to the same actual funder preserves the
  original counters and reservations and skips this normalization.
- **Funding across the requester tree.** An accepted proposal may have a
  task's ancestor fund a delegate directly. Before moving a requester
  subtree, the root checks every task's actual incoming funder link;
  allocations still funded by an old task that must end need separate
  transfers in the same commit, or the move is refused. They need not be
  folded into the moved root's budget. Stable external pool or period
  allocations retain their original funders. Funding cycles are refused,
  and a funding node cannot close while it still funds live allocations.
  New reservations use actual available funds; returning budget to a task
  does not return it to that task's own funder until its allotment closes
  or is explicitly narrowed.
- **One funding source.** A budget widening from the same actual funder
  updates the allotment and its reservation together. From another funder
  it must re-fund the whole remaining allotment, with the increase, under
  these rules and the authority checks, or be refused. It cannot mix a
  second source into counters that record only one funder.
- **Overruns** are charged all the same: what a run spends past what its
  task had left is counted, and its task, left with nothing, is held for
  its budget (tasks.md, 5.5); its funder's numbers carry the excess.
- **Periods.** A project's spend and a person's allotment are per period.
  A reservation counts in the period that funded it, and what it returns
  goes back to that period, so a reset frees nothing reserved. A
  period's identity and counters stay live until its reservations close;
  an explicit transfer to a different period is new funding, even for the
  same person or project. A recurring task's budget is carved again each
  period (tasks.md, section 9).
- **Spend is priced as it is spent.** A charter carries the prices of
  its models, in the deployment's unit. A run prices each completion it
  makes, its sub-agents' included, and reports what it spent with each
  turn it commits (engine.md, 7.2), and the whole with its answer; the
  engine charges each turn as it commits, and the difference between the
  answer's whole and the turns committed when it commits the answer. This
  difference uses cumulative committed spend of that run, kept in history,
  including turns charged to a closed allotment; resetting current task
  counters during a move never charges those turns again. What a worker
  loses with turns never committed, and no answer, is bounded by the turns
  a worker may keep unacknowledged.
- **A run's budget** is what its task has left, capped per run by the
  deployment, in the deployment's unit. The agent enforces it (agent.md,
  4.2), stopping the next completion of each session once the run has
  spent it, so a run spends past its budget by at most one completion of
  each session it has open.
- **Time is a deadline.** A task past its deadline is held, and its
  requester told; its delegates' deadlines are no later than its own.
- **Only LLMs are counted** for now. A forge's requests are its
  connector's load (connectors.md, section 12), not spend; CI's time is
  the forge's.

## 8. Checks

Every check answers one of four, each saying why:

- **allow;**
- **wait** for facts a connector has yet to report (CI on the exact
  head, a gate's verdict), which nobody clears by deciding;
- **propose:** beyond the task's authority, within what someone above
  it may accept;
- **refuse:** beyond the project's policy or the deployment's rules, or
  against a fact (CI failed), which no acceptance changes.

A check answers the strictest of what it found.

### 8.1 Batches

Before a batch commits (tasks.md, section 4): each task's authority fits
under its creator (section 5), the creator's delegation allows each
task's executor kind and charter, and the batch's tasks, depth, budgets
and deadlines are within what its creator has left. Beyond any of these,
the answer is propose; beyond the project's ceiling, refuse. Allowed, the
answer gives the creator's numbers after the reservations.

### 8.2 Effects

When a decision asks for an effect, whether an agent's tool, a
procedure's step or an accepted proposal:

- a grant of the task covers it, or the answer is propose;
- the project's ceiling and the deployment's rules allow it, or the
  answer is refuse;
- the effect's requirements hold (section 10), on the facts the
  connector gives for the exact state the effect names: wait while some
  are unknown or pending, refuse once one has failed.

An effect is checked when it is decided, against facts pinned to what it
names (a head, not a branch), and the effect carries its condition to
the system: a merge is made at exactly the head it was decided on, so
facts read for that head stay true while it is made.

### 8.3 Runs

Before a run is claimed and again as it starts: the task has budget
left above the deployment's minimum for a run, its deadline is not past,
the accounts its charter's models use are usable (engine.md,
section 12), and every branch its workspace writes, saved work
included, is covered by a push grant of the task and held by the task or
by the task above it that holds it (connectors.md, 3.3). A run that
fails this check is not started: its task waits for its budget (held, if
nothing will free one), its account, or its writer slot.

### 8.4 People's requests

Before a person's request is acted on (people.md, section 5): their
role in the project allows the request and, for what gives authority (a
task created, a budget allotted, a proposal accepted), what it gives
fits under their role's authority and what their pool has left.

### 8.5 Tool calls

Before an agent's call is served: its family is among the task's tools,
and a read it makes is covered by a grant. A message to a task needs a
reference to it (tasks.md, 7.5); a note needs its scope.

## 9. Proposals

What may be proposed, what it needs, and what an acceptance does; where
proposals wait and how they move is tasks.md, section 8.

- **What may be proposed:** a batch beyond its creator's authority; an
  effect beyond its grants; a widening of the proposer's own authority
  (more budget, more depth, another grant, a later deadline); an
  amendment beyond the amender's authority. Never what the project's
  policy or the deployment's rules refuse.
- **What it needs** is the least authority the action takes: for a
  batch, every tool, grant, kind and scope its tasks have, the sum of
  their tasks and budgets, their depth, their latest deadline; for an
  effect, the one grant that covers it; for a widening, what it asks
  for. The holder that accepts funds all of it (below), whatever the
  proposer has.
- **An escalation needs** nothing more than standing above the held task,
  unless releasing it needs what the task lacks (budget, time, a grant):
  then it needs that, as a widening would.
- **Who covers it.** A holder covers a proposal when what it needs fits
  under the holder (section 5), its depth counted from the holder (the
  action's depth plus how far below the holder the proposer is): a task
  above the proposer, or a person whose role decides that kind of
  proposal and whose pool has the budget left.
- **An acceptance funds the action** from the accepter: the budgets of a
  batch's tasks are reserved from the accepter, which is recorded as
  their funder; a widening moves budget or a grant from the accepter
  into the proposer; an effect is made with the accepter's grant. The
  action commits with the acceptance, as one decision. Shared parts (a
  grant) are given; consumed parts (budget, tasks) are moved.
- **An acceptance decides one action,** not a standing permission. A
  task that keeps needing more proposes widening its authority instead,
  once.
- **A rejection carries its reason** to the proposer, which may revise
  and propose again.

## 10. Rules no task loosens

Beyond what tasks may be given, the policy and the rules attach
**requirements** to kinds of effect, which hold whatever any task's
authority says:

- **A requirement** names facts its connector reports for the exact
  state an effect names, each with what it must be: CI passed on this
  head; a gate's verdict valid at this head (forge.md, 8.3); this head
  contains the landing branch's tip; a person of a role approved this
  head, or a head temper's clean updates led from to this one, unless
  the rule asks for this head exactly.
- **Landing rules,** per landing branch, are the usual ones: for `main`
  by default, CI passed on the exact head (or the project's checks, for
  a repository without CI), and the head up to date with the branch.
  Gates a project wants on every landing (a review with a lens, a
  person's approval) are added here; gates a plan wants on one change
  are its own (forge.md, 8.3), and add to these, never replace them.
- **Who decides what:** for each kind of proposal (a goal's budget past
  a threshold, a landing into a branch the tree was not granted), the
  role that may accept it.
- **Requirements are checked twice:** by the procedure that waits for
  them, which is the mechanics, and by this child domain at the effect,
  which is the rule. A procedure that asks for an effect whose
  requirements do not hold is refused, which is a bug in the procedure,
  counted, and never a landing.

## 11. The world

Authority, a decision over data, is tested by its step tests, as the
rules are today: the order checked against an independent statement of
it over generated values, and its laws (reflexive, transitive, fitting
never above the order); carving and returning budgets over generated
trees with funders, the spend of a subtree never counted twice and never
above what funded it but by the overruns; every check's answer over
generated facts, a sweep of the landing rule checked against an
independent statement of it. The engine's world exercises it in place
(engine.md, section 15), whose referee holds the first promise of
core.md, section 10.

## 12. From today

- **`rules`** becomes this child domain. Its four answers stay (allow,
  wait for facts, a person's acceptance, refuse), with acceptance now a
  proposal any holder may decide, not only a person. Each rule moves:
  - the protected landing rule becomes a landing rule (section 10), with
    a person's approval one requirement a project may add, no longer the
    default;
  - writes only to the deployment's repositories, and protected branches
    no run pushes to and no write deletes, become the project's ceiling
    and the grants under it (section 6);
  - the plan rule (acceptance past a size, an estimate, a landing on a
    protected branch) becomes the creator's authority;
  - the rule over notes becomes note scopes;
  - spend per run, per goal and per deployment become the per-run cap,
    a goal's budget, and the project's and the deployment's spend per
    period (section 7);
  - what a person may do (open, steer, accept, cancel, release, watch),
    read from their permission on the forge per request, becomes their
    role (people.md, section 4);
  - reviews counted from people at a permission, never from temper's own
    user, become gates, which are temper's own tasks (forge.md, 8.3);
    reviews by others count only on participating objects.
- **Checks on every write.** Today some writes pass unchecked (closing,
  reopening, progress, a run's comment); every effect is checked here
  (8.2), and so is every push a run may make (8.3).
- **The plan's envelope** (how far an accepted plan may grow on its
  own) is the coordinator's authority: growth within it needs nobody;
  beyond it, a proposal.
- **Acceptance of a step,** kept in a record today, is an accepted
  proposal, kept in the store.
- **Grants in an agent's charter** (tool families, outlets) are the
  tools part of the task's authority, derived by the engine.
- **Spend** is new end to end: the worker's channel carries none today,
  so records' spend stays zero and the rules weigh only the budgets
  runs ask for; and today's agent budgets a token count split across
  kinds, where a run now prices each completion in the deployment's
  unit.
- **Credential grants** (`credentials.md`) are unrelated: tokens pushed
  to the attempts that use them, not authority.

## 13. Open questions

- **The deployment's unit,** and whether spend per period has a window
  that rolls or resets.
- **Costs beyond LLMs:** CI minutes, a test environment's time, a
  forge's requests, once one of them is scarce.
- **Read grants:** whether reads within a project need grants at all,
  or are the project's for any of its tasks.
- **Requirements over several systems,** such as a landing that waits
  for a test environment's verdict on its head: the connectors' facts
  would meet in one check; not needed first.
- **Delegated decisions:** whether a holder may give a delegate the
  right to decide some proposals from below it, as a coordinator might
  for its sub-coordinators.
