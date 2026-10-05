//! Generated landing sweep against a set-based statement, exact pins and
//! full landing-rule memory (domain/authority.md, 10–11; forge.md, 8.3).

use alloc::boxed::Box;
use core::mem::{size_of, size_of_val};

use skein_lib::{List, Map, Queue, Rng, Set, bytes::copy_of};

use crate::{
    Answer, Approval, Authority, Budget, Ci, Delegation, Domain, Effect, EffectAsk, Event, Fact, Finding, Freshness,
    Gate, Grant, Head, Implies, Landing, LandingRule, Last, Limits, Name, POLICY_MAX_OUT, Pattern, Policy, PolicyFact,
    PolicyRefusal, Proposals, Requests, Requirement, Review, Role, Rules, Scopes, Status, Tools, Verdict, check_effect,
    max_out, step, worst_case,
};

const HEAD: Head = [1; 32];
const OLD: Head = [2; 32];
const FOREIGN: Head = [3; 32];
const LIMITS: Limits = Limits {
    projects: 1,
    roles: 2,
    requirements: 1,
    facts: 2,
    grants: 1,
    executors: 0,
    segments: 2,
    segment_bytes: 8,
    implications: 0,
    batch: 1,
    accounts: 0,
    writes: 0,
    landing_rules: 2,
    gates: 2,
    approvals: 2,
    heads: 2,
    verdicts: 4,
    reviews: 4,
};

fn pattern() -> Pattern {
    Pattern { segments: Box::new([]), last: Last::Open(copy_of(b"")) }
}
fn branch() -> Pattern {
    Pattern { segments: Box::new([copy_of(b"repo"), copy_of(b"main")]), last: Last::None }
}
fn authority() -> Authority {
    Authority {
        tools: Tools(1),
        grants: Box::new([Grant { connector: 1, kind: 2, pattern: pattern() }]),
        delegation: Delegation { kinds: Box::new([]), tasks: 10, depth: 3 },
        budget: Budget { spend: 100, deadline: None },
        notes: Scopes(0),
    }
}
fn rule() -> LandingRule {
    LandingRule {
        connector: 1,
        kind: 2,
        pattern: branch(),
        ci: true,
        up_to_date: true,
        gates: Box::new([]),
        approvals: Box::new([]),
    }
}
fn gate(blocking: bool, freshness: Freshness) -> Gate {
    Gate { number: 10, blocking, freshness }
}
fn approval(freshness: Freshness, people: u32) -> Approval {
    Approval { role: 7, people, freshness }
}
fn rules() -> Rules {
    Rules {
        ceiling: authority(),
        period_spend: 1_000,
        minimum_run_spend: 1,
        maximum_run_spend: 100,
        implies: Implies::new(Box::new([]), 0).unwrap(),
        requirements: Box::new([]),
        landing: Box::new([rule()]),
    }
}
fn policy() -> Policy {
    let mut landing = rule();
    landing.ci = false;
    landing.up_to_date = false;
    landing.gates = Box::new([gate(true, Freshness::Clean)]);
    landing.approvals = Box::new([approval(Freshness::Clean, 1)]);
    let first =
        Role { number: 7, authority: authority(), period_spend: 100, requests: Requests::ALL, decides: Proposals::ALL };
    let mut second = first.clone();
    second.number = 8;
    Policy {
        ceiling: authority(),
        period_spend: 500,
        roles: Box::new([first, second]),
        requirements: Box::new([]),
        landing: Box::new([landing]),
    }
}
fn install(domain: &mut Domain, policy: Policy) -> PolicyFact {
    let mut out = Queue::with_capacity(POLICY_MAX_OUT);
    step(domain, Event::Policy { project: 1, policy }, &mut out);
    out.pop().unwrap()
}
fn domain(policy: Policy) -> Domain {
    let mut domain = Domain::new(rules(), LIMITS).unwrap();
    assert_eq!(install(&mut domain, policy), PolicyFact::Added { project: 1 });
    domain
}
fn landing() -> Landing {
    Landing {
        head: HEAD,
        tip: OLD,
        contains_tip: Status::Passed,
        ci: Ci { head: HEAD, status: Status::Passed },
        clean: Box::new([OLD]),
        gates: Box::new([]),
        verdicts: Box::new([Verdict { gate: 10, head: OLD, status: Status::Passed }]),
        reviews: Box::new([Review { person: 1, role: 7, head: OLD, status: Status::Passed }]),
    }
}
fn ask() -> EffectAsk {
    EffectAsk {
        project: 1,
        authority: authority(),
        effect: Effect {
            connector: 1,
            kind: 2,
            name: Name { segments: Box::new([copy_of(b"repo"), copy_of(b"main")]) },
            state: HEAD,
        },
        landing: Some(landing()),
    }
}
fn check(domain: &Domain, ask: &EffectAsk) -> (Answer, Queue<Finding>) {
    let mut out = Queue::with_capacity(max_out(domain.limits()).unwrap());
    let answer = check_effect(domain, ask, &[], &mut out);
    (answer, out)
}
fn status(rng: &mut Rng) -> Status {
    match rng.below(8) {
        0 => Status::Unknown,
        1 => Status::Pending,
        2 => Status::Failed,
        _ => Status::Passed,
    }
}
fn head(rng: &mut Rng) -> Head {
    match rng.below(5) {
        0 => FOREIGN,
        1 | 2 => OLD,
        _ => HEAD,
    }
}
fn fresh(rng: &mut Rng) -> Freshness {
    if rng.chance(500) { Freshness::Clean } else { Freshness::Exact }
}

// The statement gathers the valid heads and people into sets. It does not
// share the production predicate, counters, matching or findings.
fn accepted_heads(landing: &Landing, freshness: Freshness) -> Set<Head> {
    let mut set = Set::with_capacity(3);
    set.insert(landing.head).unwrap();
    match freshness {
        Freshness::Exact => {}
        Freshness::Clean => {
            for head in &landing.clean {
                set.insert(*head).unwrap();
            }
        }
    }
    set
}
fn fact_answer(status: Status) -> Answer {
    if status == Status::Failed {
        Answer::Refuse
    } else if status == Status::Passed {
        Answer::Allow
    } else {
        Answer::Wait
    }
}
fn gate_statement(gate: Gate, landing: &Landing) -> Answer {
    if !gate.blocking {
        return Answer::Allow;
    }
    let heads = accepted_heads(landing, gate.freshness);
    let mut statuses = Set::with_capacity(4);
    for verdict in &landing.verdicts {
        if verdict.gate == gate.number && heads.contains(&verdict.head) {
            // Status has no order; numbers here are the statement's truth table.
            let key = match verdict.status {
                Status::Unknown => 0_u8,
                Status::Pending => 1,
                Status::Passed => 2,
                Status::Failed => 3,
            };
            statuses.insert(key).unwrap();
        }
    }
    if statuses.contains(&3) {
        Answer::Refuse
    } else if statuses.contains(&0) || statuses.contains(&1) || !statuses.contains(&2) {
        Answer::Wait
    } else {
        Answer::Allow
    }
}
fn approval_statement(approval: &Approval, landing: &Landing) -> Answer {
    let heads = accepted_heads(landing, approval.freshness);
    let mut people = Set::with_capacity(LIMITS.reviews);
    let mut uncertain = Set::with_capacity(LIMITS.reviews);
    let mut failed = false;
    for review in &landing.reviews {
        if review.role == approval.role && heads.contains(&review.head) {
            if review.status == Status::Passed {
                people.insert(review.person).unwrap();
            } else {
                uncertain.insert(review.person).unwrap();
            }
            failed |= review.status == Status::Failed;
        }
    }
    for person in &uncertain {
        people.remove(person);
    }
    if failed {
        Answer::Refuse
    } else if people.len() < approval.people {
        Answer::Wait
    } else {
        Answer::Allow
    }
}
fn statement(policy: &Policy, ask: &EffectAsk) -> Answer {
    let mut answer = if policy.ceiling.grants.is_empty() {
        Answer::Refuse
    } else if ask.authority.grants.is_empty() {
        Answer::Propose
    } else {
        Answer::Allow
    };
    let Some(landing) = &ask.landing else {
        return answer.max(Answer::Wait);
    };
    if landing.head != ask.effect.state {
        return Answer::Refuse;
    }
    let ci = if landing.ci.head == HEAD { landing.ci.status } else { Status::Unknown };
    answer = answer.max(fact_answer(ci)).max(fact_answer(landing.contains_tip));
    for gate in &policy.landing[0].gates {
        answer = answer.max(gate_statement(*gate, landing));
    }
    for approval in &policy.landing[0].approvals {
        answer = answer.max(approval_statement(approval, landing));
    }
    for gate in &landing.gates {
        answer = answer.max(gate_statement(*gate, landing));
    }
    answer
}

#[test]
fn generated_landing_sweep_agrees_with_independent_sets_and_gates_only_tighten() {
    let mut answers = [0_u32; 4];
    for seed in 0..384 {
        let mut rng = Rng::new(seed);
        let mut policy = policy();
        policy.landing[0].gates[0].freshness = fresh(&mut rng);
        policy.landing[0].approvals[0].freshness = fresh(&mut rng);
        policy.landing[0].approvals[0].people = u32::try_from(rng.between(1, 2)).unwrap();
        let mut ask = ask();
        let mut landing = landing();
        landing.ci = Ci { head: head(&mut rng), status: status(&mut rng) };
        landing.contains_tip = status(&mut rng);
        if rng.chance(300) {
            landing.clean = Box::new([]);
        }
        landing.verdicts = Box::new([
            Verdict { gate: 10, head: head(&mut rng), status: status(&mut rng) },
            Verdict { gate: 10, head: head(&mut rng), status: status(&mut rng) },
        ]);
        let mut reviews = List::with_capacity(4);
        for _ in 0..rng.between(1, 4) {
            reviews
                .push(Review {
                    person: rng.between(1, 3),
                    role: if rng.chance(800) { 7 } else { 8 },
                    head: head(&mut rng),
                    status: status(&mut rng),
                })
                .unwrap();
        }
        landing.reviews = reviews.into_boxed();
        landing.gates = Box::new([gate(rng.chance(500), fresh(&mut rng))]);
        ask.landing = if rng.chance(50) { None } else { Some(landing) };
        if rng.chance(100) {
            ask.authority.grants = Box::new([]);
        }
        if rng.chance(50) {
            policy.ceiling.grants = Box::new([]);
            for role in &mut policy.roles {
                role.authority.grants = Box::new([]);
            }
        }
        let expected = statement(&policy, &ask);
        let mut bare = policy.clone();
        bare.landing[0].gates = Box::new([]);
        bare.landing[0].approvals = Box::new([]);
        let bare_domain = domain(bare);
        let bare_answer = check(&bare_domain, &ask).0;
        let domain = domain(policy);
        let (actual, why) = check(&domain, &ask);
        assert_eq!(actual, expected, "seed {seed}: {ask:?}");
        assert!(actual >= bare_answer, "additional project requirements never loosen seed {seed}");
        assert!(why.len() <= max_out(&LIMITS).unwrap());
        let index = match actual {
            Answer::Allow => 0,
            Answer::Wait => 1,
            Answer::Propose => 2,
            Answer::Refuse => 3,
        };
        answers[index] = answers[index].checked_add(1).unwrap();
    }
    for count in answers {
        assert!(count > 0, "the sweep visits every answer");
    }
}

#[test]
fn landing_cells_preserve_pins_freshness_distinct_people_and_rule_conjunction() {
    let domain = domain(policy());
    assert_eq!(check(&domain, &ask()).0, Answer::Allow, "reviews and gate verdicts carry over a clean update");
    for cause in 0_u8..9 {
        let mut ask = ask();
        let landing = ask.landing.as_mut().unwrap();
        let expected = match cause {
            0 => {
                landing.ci.head = OLD;
                Answer::Wait
            }
            1 => {
                landing.clean = Box::new([]);
                Answer::Wait
            }
            2 => {
                landing.head = OLD;
                Answer::Refuse
            }
            3 => {
                landing.contains_tip = Status::Failed;
                Answer::Refuse
            }
            4 => {
                landing.verdicts[0].status = Status::Failed;
                Answer::Refuse
            }
            5 => {
                landing.reviews[0].status = Status::Failed;
                Answer::Refuse
            }
            6 => {
                landing.reviews[0].role = 8;
                Answer::Wait
            }
            7 => {
                ask.landing = None;
                Answer::Wait
            }
            _ => {
                landing.ci.status = Status::Failed;
                Answer::Refuse
            }
        };
        assert_eq!(check(&domain, &ask).0, expected, "cause {cause}");
    }
    let mut ask = ask();
    ask.landing = None;
    ask.effect.name.segments[1] = copy_of(b"other");
    assert_eq!(check(&domain, &ask).0, Answer::Allow, "ordinary unmatched effects need no landing payload");
    let mut exact = policy();
    exact.landing[0].gates[0].freshness = Freshness::Exact;
    assert_eq!(check(&self::domain(exact), &self::ask()).0, Answer::Wait);
    let mut exact = policy();
    exact.landing[0].approvals[0].freshness = Freshness::Exact;
    assert_eq!(check(&self::domain(exact), &self::ask()).0, Answer::Wait);
    check_approval_duplicates();
    check_gate_conflicts();
    check_generic_conjunction();
}

fn check_gate_conflicts() {
    let mut ask = ask();
    ask.landing.as_mut().unwrap().verdicts = Box::new([
        Verdict { gate: 10, head: HEAD, status: Status::Passed },
        Verdict { gate: 10, head: OLD, status: Status::Failed },
    ]);
    assert_eq!(check(&domain(policy()), &ask).0, Answer::Refuse, "a valid carried failure cannot be cleared");
    let mut exact = policy();
    exact.landing[0].gates[0].freshness = Freshness::Exact;
    assert_eq!(check(&domain(exact), &ask).0, Answer::Allow, "an exact requirement ignores stale failures");
    ask.landing.as_mut().unwrap().verdicts[1].status = Status::Pending;
    assert_eq!(check(&domain(policy()), &ask).0, Answer::Wait, "a valid pending gate cannot be cleared");
    let mut advisory = policy();
    advisory.landing[0].gates[0].blocking = false;
    ask.landing.as_mut().unwrap().verdicts[1].status = Status::Failed;
    assert_eq!(check(&domain(advisory), &ask).0, Answer::Allow, "a project's advisory gate holds nothing");
}

fn check_approval_duplicates() {
    let mut policy = policy();
    policy.landing[0].approvals[0].people = 2;
    let domain = domain(policy);
    let mut ask = ask();
    let landing = ask.landing.as_mut().unwrap();
    let review = landing.reviews[0];
    landing.reviews = Box::new([review, review]);
    assert_eq!(check(&domain, &ask).0, Answer::Wait, "one person counts once");
    ask.landing.as_mut().unwrap().reviews[1].person = 2;
    assert_eq!(check(&domain, &ask).0, Answer::Allow);
    ask.landing.as_mut().unwrap().reviews[1].status = Status::Pending;
    assert_eq!(check(&domain, &ask).0, Answer::Wait);
    ask.landing.as_mut().unwrap().reviews = Box::new([review, Review { status: Status::Pending, ..review }]);
    assert_eq!(check(&domain, &ask).0, Answer::Wait, "conflicting reports cannot create an approval");
    let landing = ask.landing.as_mut().unwrap();
    landing.reviews = Box::new([Review { head: FOREIGN, ..review }, review, Review { person: 2, ..review }]);
    assert_eq!(check(&domain, &ask).0, Answer::Allow, "an earlier stale duplicate cannot hide a valid approval");
}

fn check_generic_conjunction() {
    let mut rules = rules();
    rules.requirements = Box::new([Requirement { connector: 1, kind: 2, pattern: pattern(), facts: Box::new([7]) }]);
    let mut domain = Domain::new(rules, LIMITS).unwrap();
    assert_eq!(install(&mut domain, policy()), PolicyFact::Added { project: 1 });
    let mut ask = ask();
    let mut why = Queue::with_capacity(max_out(&LIMITS).unwrap());
    assert_eq!(check_effect(&domain, &ask, &[], &mut why), Answer::Wait, "generic requirements still apply");
    let fact = Fact { connector: 1, kind: 7, name: ask.effect.name.clone(), state: HEAD, status: Status::Passed };
    let mut why = Queue::with_capacity(max_out(&LIMITS).unwrap());
    assert_eq!(check_effect(&domain, &ask, core::slice::from_ref(&fact), &mut why), Answer::Allow);
    ask.landing.as_mut().unwrap().gates = Box::new([Gate { number: 99, blocking: false, freshness: Freshness::Exact }]);
    ask.landing.as_mut().unwrap().verdicts = Box::new([
        Verdict { gate: 10, head: OLD, status: Status::Passed },
        Verdict { gate: 99, head: HEAD, status: Status::Failed },
    ]);
    let mut why = Queue::with_capacity(max_out(&LIMITS).unwrap());
    assert_eq!(check_effect(&domain, &ask, &[fact], &mut why), Answer::Allow, "advisory failure does not hold");
    ask.landing.as_mut().unwrap().gates[0].blocking = true;
    assert_eq!(check(&domain, &ask).0, Answer::Refuse, "a change's blocking gate adds to project rules");
}

#[test]
fn landing_admission_and_owned_memory_are_bounded() {
    let mut configured = rules();
    configured.landing[0].approvals = Box::new([approval(Freshness::Clean, 0)]);
    assert!(Domain::new(configured, LIMITS).is_none());
    let mut domain = domain(policy());
    let mut invalid = policy();
    invalid.landing[0].approvals[0].role = 99;
    assert_eq!(install(&mut domain, invalid), PolicyFact::Refused { project: 1, reason: PolicyRefusal::InvalidRole });
    assert_eq!(check(&domain, &ask()).0, Answer::Allow, "bad replacement retains prior rules");
    for cause in 0_u8..4 {
        let mut ask = ask();
        let landing = ask.landing.as_mut().unwrap();
        match cause {
            0 => landing.clean = Box::new([OLD; 3]),
            1 => landing.gates = Box::new([gate(true, Freshness::Clean); 3]),
            2 => landing.verdicts = Box::new([landing.verdicts[0]; 5]),
            _ => landing.reviews = Box::new([landing.reviews[0]; 5]),
        }
        let (answer, mut why) = check(&domain, &ask);
        assert_eq!(answer, Answer::Refuse);
        assert_eq!(why.pop(), Some(Finding::Oversized));
        assert!(why.is_empty());
    }
    check_landing_memory();
    check_full_finding_output();
    check_bad_landing_configuration(&mut domain);
}

fn check_bad_landing_configuration(domain: &mut Domain) {
    for cause in 0_u8..4 {
        let mut invalid = policy();
        match cause {
            0 => invalid.landing = Box::new([rule(), rule(), rule()]),
            1 => invalid.landing[0].gates = Box::new([gate(true, Freshness::Clean); 3]),
            2 => invalid.landing[0].approvals = Box::new([approval(Freshness::Clean, 1); 3]),
            _ => invalid.landing[0].pattern.last = Last::Exact(copy_of(b"123456789")),
        }
        assert_eq!(install(domain, invalid), PolicyFact::Refused { project: 1, reason: PolicyRefusal::Oversized });
    }
    let mut configured = rules();
    configured.landing[0].approvals = Box::new([Approval { role: 99, ..approval(Freshness::Exact, 1) }]);
    let mut domain = Domain::new(configured, LIMITS).unwrap();
    assert_eq!(install(&mut domain, policy()), PolicyFact::Added { project: 1 });
    let (answer, mut why) = check(&domain, &ask());
    assert_eq!(answer, Answer::Refuse, "deployment role requirements are checked per project");
    assert_eq!(why.pop(), Some(Finding::UnknownRole));
    let mut missing = ask();
    missing.landing = None;
    let (answer, mut why) = check(&domain, &missing);
    assert_eq!(answer, Answer::Refuse, "missing facts cannot hide a known nonexistent required role");
    assert_eq!(why.pop(), Some(Finding::UnknownRole));
    assert_eq!(why.pop(), Some(Finding::LandingMissing));
    assert!(why.is_empty());
    missing.effect.name.segments[1] = copy_of(b"other");
    assert_eq!(check(&domain, &missing).0, Answer::Allow, "only applicable rules require their roles");
}

fn check_full_finding_output() {
    let mut full = rule();
    full.gates = Box::new([gate(true, Freshness::Clean); 2]);
    full.approvals = Box::new([approval(Freshness::Clean, 1), Approval { role: 8, ..approval(Freshness::Clean, 1) }]);
    let mut configured = rules();
    configured.landing = Box::new([full.clone(), full.clone()]);
    let required = Requirement { connector: 1, kind: 2, pattern: pattern(), facts: Box::new([7, 8]) };
    configured.requirements = Box::new([required.clone()]);
    let mut resident = policy();
    resident.landing = Box::new([full.clone(), full]);
    resident.requirements = Box::new([required]);
    let mut domain = Domain::new(configured, LIMITS).unwrap();
    assert_eq!(install(&mut domain, resident), PolicyFact::Added { project: 1 });
    let mut ask = ask();
    ask.authority.grants = Box::new([]);
    let landing = ask.landing.as_mut().unwrap();
    landing.ci.status = Status::Failed;
    landing.contains_tip = Status::Failed;
    landing.verdicts[0].status = Status::Failed;
    landing.gates = Box::new([gate(true, Freshness::Clean); 2]);
    let review = Review { person: 1, role: 7, head: HEAD, status: Status::Failed };
    landing.reviews = Box::new([review, Review { person: 2, role: 8, ..review }]);
    let (answer, why) = check(&domain, &ask);
    assert_eq!(answer, Answer::Refuse, "hard failures win over generic waits and task proposals");
    assert_eq!(why.len(), 39, "all deployment, project and change findings survive together");
    assert!(why.len() <= max_out(&LIMITS).unwrap());
}

fn check_landing_memory() {
    let mut full = rule();
    full.pattern = Pattern {
        segments: Box::new([copy_of(b"12345678"), copy_of(b"12345678")]),
        last: Last::Open(copy_of(b"12345678")),
    };
    full.gates = Box::new([gate(true, Freshness::Clean); 2]);
    full.approvals = Box::new([approval(Freshness::Clean, 4); 2]);
    let tables = Box::new([full.clone(), full]);
    let mut bytes = u64::try_from(size_of_val(tables.as_ref())).unwrap();
    for rule in tables.as_ref() {
        bytes = bytes.checked_add(u64::try_from(size_of_val(rule.pattern.segments.as_ref())).unwrap()).unwrap();
        for segment in &rule.pattern.segments {
            bytes = bytes.checked_add(u64::try_from(segment.len()).unwrap()).unwrap();
        }
        match &rule.pattern.last {
            Last::None => {}
            Last::Exact(bytes_) | Last::Open(bytes_) => {
                bytes = bytes.checked_add(u64::try_from(bytes_.len()).unwrap()).unwrap();
            }
        }
        bytes = bytes.checked_add(u64::try_from(size_of_val(rule.gates.as_ref())).unwrap()).unwrap();
        bytes = bytes.checked_add(u64::try_from(size_of_val(rule.approvals.as_ref())).unwrap()).unwrap();
    }
    let mut configured = rules();
    configured.landing = tables.clone();
    let mut resident = policy();
    resident.landing = tables;
    let mut domain = Domain::new(configured, LIMITS).unwrap();
    assert_eq!(install(&mut domain, resident), PolicyFact::Added { project: 1 });
    let bare = Limits { landing_rules: 0, gates: 0, approvals: 0, ..LIMITS };
    let extra = worst_case(&LIMITS).unwrap().checked_sub(worst_case(&bare).unwrap()).unwrap();
    assert_eq!(extra, bytes.checked_mul(2).unwrap(), "deployment and full policy landing boxes are counted exactly");
    assert!(worst_case(&LIMITS).unwrap() >= Map::<u32, Policy>::worst_case(1).unwrap());
    assert_eq!(size_of::<Head>(), 32);
}
