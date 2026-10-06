# Step 01: authority

Provisional, 2026-10-04. The authority child domain,
`temper-engine-domain-authority`: authority as a value with an order,
resources named by paths, budgets carved and returned, the rules and
policies in force, and the checks every action passes (`authority.md`).
It is pure policy, built beside the legacy `rules`, which it replaces at
the cutover. Overview and conventions: README.md.

## 1. What it is, and what it is not

- **A decision over data**, as `rules` is today: free functions over
  values its caller gathers, and a small state holding only the
  deployment's rules and each live project's policy, changed by events.
- **It keeps no numbers.** The numbers against each funder (budget,
  spent, spent by ended tasks it funded, reserved) are the tasks child
  domain's (`tasks.md`, section 3); the root passes them in, and a check's
  answer says what they become (`authority.md`, section 2).
- **It knows no connector.** Names are paths of byte segments and kinds
  are numbers; each connector gives the table of its kinds' order as data
  (`authority.md`, section 4). The forge's table comes from step 04,
  through the root.

Nothing of the legacy engine is touched. From `temper-legacy-engine-domain-rules`
it ports, by copying and adapting: the four answers and the bounded queue
of findings each check writes why into; the protected-landing rule's
sweep against an independent statement, which becomes the landing rule's
(`authority.md`, section 10).

## 2. The crate

```
crates/temper-engine-domain-authority/src/
├── lib.rs          its doc (what it decides, what it never keeps), re-exports
├── value.rs        Authority, Tools, Grant, Pattern, Name, Delegation, Budget, Scopes
├── order.rs        at_most, fits, covers (a pattern, a grant, a holder)
├── numbers.rs      Numbers, Funder; carve, return_unspent, charge, move_funding
├── rules.rs        Rules (the deployment's), Policy (a project's), Role, Requirement, LandingRule, Implies
├── check.rs        check_batch, check_effect, check_run, check_request, check_call, needs, covers
├── domain.rs       Domain (rules and policies in force), step: policies added, changed, dropped
├── limits.rs       Limits, worst_case
└── tests.rs        the laws, the sweeps, every check's cells
```

### 2.1 The value

```rust
/// What a task may do (authority.md, section 3).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Authority {
    /// The tool families an agent's run may call, one bit each.
    pub tools: Tools,
    pub grants: Box<[Grant]>,
    pub delegation: Delegation,
    pub budget: Budget,
    /// The note scopes it may write in.
    pub notes: Scopes,
}

/// An effect or a read of one connector's kind, on every resource a
/// pattern covers (authority.md, section 4).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Grant {
    pub connector: u16,
    pub kind: u16,
    pub pattern: Pattern,
}

/// Segments matched exactly, then a last one exact or open.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Pattern {
    pub segments: Box<[Box<[u8]>]>,
    pub last: Last,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Last {
    /// Only the name the segments spell.
    None,
    /// That name with one more segment, exactly this one, and every name beneath.
    Exact(Box<[u8]>),
    /// That name with one more segment beginning with this, and every name beneath.
    Open(Box<[u8]>),
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Delegation {
    /// The executors its delegates may have: charters, procedures, roles.
    pub kinds: Box<[Executor]>,
    /// How many tasks its subtree may make over its life, and how deep.
    pub tasks: u32,
    pub depth: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Budget {
    /// In the deployment's unit.
    pub spend: u64,
    pub deadline: Option<Wall>,
}
```

`Executor` is authority's own: a charter, a procedure or a role, each by
the number configuration or the policy gives it. Nothing here borrows
another crate's types; the root translates (`engine.md`, section 3).

### 2.2 The order and fitting

```rust
/// Whether `a` is at most `b`: every part of it (authority.md, section 5).
#[must_use]
pub fn at_most(a: &Authority, b: &Authority, implies: &Implies) -> bool;

/// Whether `child` fits under `creator`, given what the creator has left:
/// at most it, with the creator's depth less one and its tasks and spend
/// replaced by what is left. Each part that does not fit is written to
/// `lacks`, so a refusal can name it and a proposal can ask for it.
pub fn fits(child: &Authority, creator: &Authority, left: &Numbers, implies: &Implies, lacks: &mut Queue<Lack>) -> bool;
```

Pattern order is decided segment by segment, never by enumerating names.
Kinds are ordered by the connector's `Implies` table (for the forge,
`push` implies `branch` on what it covers, and `land` implies nothing).

### 2.3 Numbers

```rust
/// What is kept against one funder (authority.md, section 7). What is left
/// is the budget less the other three, never below zero.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Numbers {
    pub budget: u64,
    pub spent: u64,
    pub spent_below: u64,
    pub reserved: u64,
}

/// A funder's numbers after reserving `budgets` for the tasks of a batch.
pub fn carve(funder: Numbers, budgets: &[u64]) -> Option<Numbers>;

/// As a funded task ends: its funder's numbers once what it left is
/// returned and what it and its own funded tasks spent is counted below.
pub fn settle(funder: Numbers, ended: Numbers) -> Numbers;

/// A turn's or an answer's spend charged to the task that ran it; past what
/// it had left, the excess is counted all the same, and the answer says so.
pub fn charge(task: Numbers, spent: u64) -> Charged;
```

Checked arithmetic throughout; an overflow is a refusal, never a wrap.

### 2.4 Rules, policies and checks

```rust
/// Every check's answer, the strictest of what it found (authority.md, 8).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Answer {
    Allow,
    /// Facts a connector has yet to report, which nobody clears by deciding.
    Wait,
    /// Beyond the task, within what someone above it may accept.
    Propose,
    /// Beyond the project's policy or the deployment's rules, or against a fact.
    Refuse,
}

pub fn check_batch(domain: &Domain, ask: &BatchAsk, why: &mut Queue<Finding>) -> Checked;
pub fn check_effect(domain: &Domain, ask: &EffectAsk, facts: &[Fact], why: &mut Queue<Finding>) -> Answer;
pub fn check_run(domain: &Domain, ask: &RunAsk, why: &mut Queue<Finding>) -> Answer;
pub fn check_request(domain: &Domain, ask: &PersonAsk, why: &mut Queue<Finding>) -> Checked;
pub fn check_call(domain: &Domain, ask: &CallAsk, why: &mut Queue<Finding>) -> Answer;

/// The least authority a proposal's action takes (authority.md, section 9).
pub fn needs(action: &Action) -> Option<Authority>;
/// Whether `holder` covers it, its depth counted from the holder.
pub fn covers(domain: &Domain, needs: &Authority, holder: &Holder, below: u32) -> bool;

/// An answer, and the funder's numbers it leaves when it allows.
pub struct Checked {
    pub answer: Answer,
    pub numbers: Option<Numbers>,
}
```

`Domain` holds `Rules` and `Map<u32, Policy>`, bounded by the limits on
projects, roles and landing rules; its `step` takes `Event::Policy {
project, policy }` and `Event::Dropped { project }`, the root sending them
at a restart and as a person changes a policy (`people.md`, 5.2). It
emits nothing but facts.

## 3. Tests

Step tests only (`authority.md`, section 11), in `src/tests.rs`, sized
for the default suite:

- **The order against an independent statement.** A small universe of
  names (paths of up to four segments over a three-letter alphabet), each
  pattern's set of covered names enumerated naively; `at_most` on patterns
  agrees with set inclusion over every pair of a generated sample.
- **Its laws,** over generated authorities: reflexive, transitive;
  `fits` never answers yes where `at_most` (with the creator's numbers
  as its budget) answers no.
- **Carving and settling** over generated funding trees: every task's
  spend counted exactly once at the top, never above what funded it but
  by the overruns charged; a moved task's funding returned and reserved
  anew; a period's reset freeing nothing reserved.
- **Each check's cells:** every reason for each answer, and the
  strictest winning when several hold.
- **The landing rule's sweep:** ported from the legacy rules' protected
  landing sweep, over generated facts (CI on the exact head, gates
  carried or exact through clean updates, the head containing the tip,
  approvals by role), against an independent statement.
- **Memory:** `worst_case` against the containers at their limits.

## 4. Increments

1. **01a the value, names and order:** `value.rs`, `order.rs`, the laws
   and the independent statement.
2. **01b numbers:** `numbers.rs`, the funding-tree tests.
3. **01c rules, policies and checks:** `rules.rs`, `check.rs`, `domain.rs`,
   every check's cells.
4. **01d requirements and landing rules:** the landing sweep.

Each is a branch through the gate; none touches another crate. 01a and
01b are what step 02 needs first (`tasks` carries authority as its own
value and keeps the numbers, so it needs their shape, not this crate).

## 5. Done when

- every answer of `authority.md`, section 8, has a test, and every law of
  section 5 a generated check;
- the landing sweep passes against its independent statement;
- the crate depends on `skein-lib` alone, and is a member of the
  workspace with its tests in the default suite within its allotment.
