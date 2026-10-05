//! Independent check cells, fitting laws and bounded policy memory
//! (domain/authority.md, sections 5, 8, 9 and 11).

use alloc::boxed::Box;
use core::mem::{size_of, size_of_val};

use skein_lib::{Map, Queue, Wall, bytes::copy_of};

use crate::{
    Action, Answer, Authority, BatchAsk, Budget, Call, CallAsk, Delegate, Delegation, Domain, Effect, EffectAsk, Event,
    FITS_MAX_OUT, Fact, Finding, Grant, Holder, Implication, Implies, Lack, Last, Limits, Name, Numbers,
    POLICY_MAX_OUT, Pattern, PersonAsk, PersonRequest, Policy, PolicyFact, PolicyRefusal, ProposalKind, Proposals,
    Requests, Requirement, Role, Rules, RunAsk, Scopes, Source, Status, Tools, Write, Writer, at_most, check_batch,
    check_call, check_effect, check_request, check_run, covers, fits, max_out, needs, step, worst_case,
};

const LIMITS: Limits = Limits {
    projects: 2,
    roles: 2,
    requirements: 2,
    facts: 4,
    grants: 3,
    executors: 3,
    segments: 4,
    segment_bytes: 8,
    implications: 3,
    batch: 3,
    accounts: 2,
    writes: 2,
    landing_rules: 0,
    gates: 0,
    approvals: 0,
    heads: 0,
    verdicts: 0,
    reviews: 0,
};

fn numbers(budget: u64) -> Numbers {
    Numbers { budget, spent: 0, spent_below: 0, reserved: 0 }
}

fn pattern() -> Pattern {
    Pattern { segments: Box::new([]), last: Last::Open(copy_of(b"")) }
}

fn authority() -> Authority {
    Authority {
        tools: Tools(u64::MAX),
        grants: Box::new([Grant { connector: 1, kind: 3, pattern: pattern() }]),
        delegation: Delegation {
            kinds: Box::new([crate::Executor::Charter(1), crate::Executor::Procedure(2), crate::Executor::Role(3)]),
            tasks: 10,
            depth: 5,
        },
        budget: Budget { spend: 100, deadline: None },
        notes: Scopes(15),
    }
}

fn requirement(fact: u16) -> Requirement {
    Requirement { connector: 1, kind: 1, pattern: pattern(), facts: Box::new([fact]) }
}

fn rules(ceiling: Authority) -> Rules {
    Rules {
        ceiling,
        period_spend: 1_000,
        minimum_run_spend: 1,
        maximum_run_spend: 80,
        implies: Implies::new(
            Box::new([
                Implication { connector: 1, kind: 3, implies: 2 },
                Implication { connector: 1, kind: 2, implies: 1 },
                Implication { connector: 1, kind: 3, implies: 1 },
            ]),
            3,
        )
        .unwrap(),
        requirements: Box::new([requirement(7)]),
        landing: Box::new([]),
    }
}

fn role(ceiling: Authority) -> Role {
    Role { number: 7, authority: ceiling, period_spend: 100, requests: Requests::ALL, decides: Proposals::ALL }
}

fn policy(ceiling: Authority) -> Policy {
    Policy {
        roles: Box::new([role(ceiling.clone())]),
        ceiling,
        period_spend: 500,
        requirements: Box::new([requirement(8)]),
        landing: Box::new([]),
    }
}

fn apply(domain: &mut Domain, event: Event) -> PolicyFact {
    let mut out = Queue::with_capacity(POLICY_MAX_OUT);
    step(domain, event, &mut out);
    out.pop().unwrap()
}

fn domain_with(deployment: Authority, project: Authority) -> Domain {
    let mut domain = Domain::new(rules(deployment), LIMITS).unwrap();
    assert_eq!(
        apply(&mut domain, Event::Policy { project: 1, policy: policy(project) }),
        PolicyFact::Added { project: 1 }
    );
    domain
}

fn domain() -> Domain {
    domain_with(authority(), authority())
}

fn findings(domain: &Domain) -> Queue<Finding> {
    Queue::with_capacity(max_out(domain.limits()).unwrap())
}

fn effect() -> Effect {
    Effect { connector: 1, kind: 1, name: Name { segments: Box::new([copy_of(b"repo")]) }, state: [1; 32] }
}

fn fact(kind: u16, status: Status) -> Fact {
    let effect = effect();
    Fact { connector: effect.connector, kind, name: effect.name, state: effect.state, status }
}

fn child(spend: u64) -> Delegate {
    let mut child = authority();
    child.budget.spend = spend;
    child.delegation.tasks = 0;
    child.delegation.depth = 0;
    Delegate { executor: crate::Executor::Charter(1), authority: child }
}

fn saw(why: &Queue<Finding>, finding: Finding) -> bool {
    for actual in why {
        if *actual == finding {
            return true;
        }
    }
    false
}

#[test]
fn effect_cells_use_pinned_facts_and_the_independent_strictest_statement() {
    for task_granted in [false, true] {
        for project_granted in [false, true] {
            for status in [Status::Unknown, Status::Pending, Status::Passed, Status::Failed] {
                for pinned in [false, true] {
                    let mut ceiling = authority();
                    if !project_granted {
                        ceiling.grants = Box::new([]);
                    }
                    let domain = domain_with(authority(), ceiling);
                    let mut task = authority();
                    if !task_granted {
                        task.grants = Box::new([]);
                    }
                    let ask = EffectAsk { project: 1, authority: task, effect: effect(), landing: None };
                    let mut reported = fact(7, status);
                    if !pinned {
                        reported.state = [2; 32];
                    }
                    let mut why = findings(&domain);
                    let actual = check_effect(&domain, &ask, &[reported, fact(8, Status::Passed)], &mut why);
                    let expected = if !project_granted || (pinned && status == Status::Failed) {
                        Answer::Refuse
                    } else if !task_granted {
                        Answer::Propose
                    } else if !pinned || status != Status::Passed {
                        Answer::Wait
                    } else {
                        Answer::Allow
                    };
                    assert_eq!(
                        actual, expected,
                        "task={task_granted}, project={project_granted}, status={status:?}, pinned={pinned}"
                    );
                    assert!(why.len() <= max_out(&LIMITS).unwrap(), "all findings fit their bound");
                }
            }
        }
    }
    let domain = domain();
    let ask = EffectAsk { project: 1, authority: authority(), effect: effect(), landing: None };
    for wrong in 0_u8..3 {
        let mut reported = fact(7, Status::Passed);
        match wrong {
            0 => reported.connector = 2,
            1 => reported.kind = 9,
            _ => reported.name.segments = Box::new([copy_of(b"other")]),
        }
        let mut why = findings(&domain);
        assert_eq!(check_effect(&domain, &ask, &[reported, fact(8, Status::Passed)], &mut why), Answer::Wait);
    }
    let mut why = findings(&domain);
    assert_eq!(
        check_effect(
            &domain,
            &ask,
            &[fact(7, Status::Passed), fact(7, Status::Failed), fact(8, Status::Passed)],
            &mut why
        ),
        Answer::Refuse
    );
    let mut why = findings(&domain);
    assert_eq!(
        check_effect(
            &domain,
            &ask,
            &[fact(7, Status::Passed), fact(7, Status::Pending), fact(8, Status::Passed)],
            &mut why
        ),
        Answer::Wait
    );
    let mut why = findings(&domain);
    assert_eq!(check_effect(&domain, &ask, &[], &mut why), Answer::Wait);
    let mut different = ask.clone();
    different.effect.kind = 2;
    let mut why = findings(&domain);
    assert_eq!(
        check_effect(&domain, &different, &[], &mut why),
        Answer::Allow,
        "requirements match exact kinds, not grant implications"
    );
    let mut different = ask.clone();
    different.effect.connector = 2;
    let mut why = findings(&domain);
    assert_eq!(check_effect(&domain, &different, &[], &mut why), Answer::Refuse);
    assert!(saw(&why, Finding::Grant { source: Source::Deployment }), "deployment ceiling is independently checked");
    let mut missing = ask.clone();
    missing.project = 99;
    let mut why = findings(&domain);
    assert_eq!(check_effect(&domain, &missing, &[], &mut why), Answer::Refuse);
    assert!(saw(&why, Finding::UnknownProject), "missing policy is a refusal");
    let mut huge = ask;
    huge.effect.name.segments = Box::new([copy_of(b"too-long-name")]);
    let mut why = findings(&domain);
    assert_eq!(check_effect(&domain, &huge, &[], &mut why), Answer::Refuse);
    assert!(saw(&why, Finding::Oversized), "names are refused at admission");
}

#[test]
fn batch_cells_count_direct_creation_and_reserve_only_after_every_check_allows() {
    let domain = domain();
    let base = BatchAsk {
        project: 1,
        creator: authority(),
        numbers: numbers(100),
        tasks_left: 10,
        tasks: Box::new([child(10), child(20)]),
    };
    let mut why = findings(&domain);
    let allowed = check_batch(&domain, &base, &mut why);
    assert_eq!(allowed.answer, Answer::Allow);
    assert_eq!(allowed.numbers.unwrap().reserved, 30, "budgets are summed once and carved together");
    assert!(why.is_empty(), "allow has no deficits");
    for cause in 0_u8..6 {
        let mut ask = base.clone();
        match cause {
            0 => ask.tasks_left = 1,
            1 => ask.numbers.reserved = 80,
            2 => ask.creator.delegation.depth = 0,
            3 => ask.creator.delegation.kinds = Box::new([]),
            4 => ask.creator.tools = Tools(0),
            _ => ask.creator.budget.deadline = Some(Wall::EPOCH),
        }
        let mut why = findings(&domain);
        let result = check_batch(&domain, &ask, &mut why);
        assert_eq!(result.answer, Answer::Propose, "cause {cause} remains within the hard ceiling");
        assert_eq!(result.numbers, None, "proposals never reserve funding");
    }
    let mut ask = base.clone();
    ask.tasks[0].authority.budget.spend = 101;
    let mut why = findings(&domain);
    assert_eq!(
        check_batch(&domain, &ask, &mut why).answer,
        Answer::Refuse,
        "a hard ceiling wins over creator deficits"
    );
    let mut ask = base.clone();
    ask.tasks[0].executor = crate::Executor::Charter(99);
    let mut why = findings(&domain);
    assert_eq!(check_batch(&domain, &ask, &mut why).answer, Answer::Refuse);
    assert!(
        saw(&why, Finding::Executor { source: Source::Deployment }),
        "own executor must be delegated by every ceiling"
    );
    let mut ask = base.clone();
    ask.tasks[0].authority.delegation.tasks = u32::MAX;
    let mut why = findings(&domain);
    assert_eq!(check_batch(&domain, &ask, &mut why).answer, Answer::Refuse);
    assert!(saw(&why, Finding::Arithmetic), "subtree task-count overflow is a refusal");
    let mut ask = base.clone();
    ask.tasks = Box::new([child(u64::MAX), child(u64::MAX)]);
    let mut why = findings(&domain);
    assert_eq!(check_batch(&domain, &ask, &mut why).answer, Answer::Refuse);
    assert!(saw(&why, Finding::Arithmetic), "batch budget sums cannot wrap");
    let mut ask = base.clone();
    ask.numbers.spent = u64::MAX;
    ask.numbers.spent_below = 1;
    let mut why = findings(&domain);
    assert_eq!(
        check_batch(&domain, &ask, &mut why).answer,
        Answer::Refuse,
        "unrepresentable accounting cannot allow even an empty batch"
    );
    let mut ask = base.clone();
    ask.tasks = Box::new([child(0), child(0), child(0), child(0)]);
    let mut why = findings(&domain);
    assert_eq!(check_batch(&domain, &ask, &mut why).answer, Answer::Refuse);
    assert!(saw(&why, Finding::Oversized), "batch admission precedes element scans");
    let mut ask = base;
    ask.project = 99;
    let mut why = findings(&domain);
    assert_eq!(check_batch(&domain, &ask, &mut why).answer, Answer::Refuse);
}

#[test]
fn run_cells_hold_for_readiness_propose_grants_and_refuse_hard_caps() {
    let domain = domain();
    let base = RunAsk {
        project: 1,
        authority: authority(),
        numbers: numbers(100),
        budget: 20,
        wall: Wall::from_nanos(10),
        accounts: Box::new([true]),
        writes: Box::new([Write { effect: effect(), held: Writer::Task }]),
    };
    let mut why = findings(&domain);
    assert_eq!(check_run(&domain, &base, &mut why), Answer::Allow);
    for cause in 0_u8..6 {
        let mut ask = base.clone();
        let expected_finding = match cause {
            0 => {
                ask.numbers.reserved = 100;
                Finding::RunBudget
            }
            1 => {
                ask.budget = 1;
                Finding::RunBudget
            }
            2 => {
                ask.authority.budget.deadline = Some(Wall::from_nanos(9));
                Finding::Deadline
            }
            3 => {
                ask.accounts[0] = false;
                Finding::Account
            }
            4 => {
                ask.writes[0].held = Writer::Pending;
                Finding::Writer
            }
            _ => {
                ask.writes[0].held = Writer::Other;
                Finding::Writer
            }
        };
        let mut why = findings(&domain);
        assert_eq!(check_run(&domain, &ask, &mut why), Answer::Wait, "cause {cause}");
        assert!(saw(&why, expected_finding), "readiness refusal states its reason");
    }
    let mut ask = base.clone();
    ask.writes[0].held = Writer::Ancestor;
    ask.authority.budget.deadline = Some(ask.wall);
    let mut why = findings(&domain);
    assert_eq!(
        check_run(&domain, &ask, &mut why),
        Answer::Allow,
        "ancestor holds and equality at a deadline are valid"
    );
    let mut ask = base.clone();
    ask.authority.grants = Box::new([]);
    ask.accounts[0] = false;
    let mut why = findings(&domain);
    assert_eq!(check_run(&domain, &ask, &mut why), Answer::Propose, "a grant deficit wins over readiness wait");
    let mut ask = base.clone();
    ask.budget = 81;
    ask.accounts[0] = false;
    let mut why = findings(&domain);
    assert_eq!(check_run(&domain, &ask, &mut why), Answer::Refuse);
    assert!(saw(&why, Finding::RunCap), "run cap wins over readiness wait");
    let mut ask = base.clone();
    ask.numbers.spent = u64::MAX;
    ask.numbers.spent_below = 1;
    let mut why = findings(&domain);
    assert_eq!(check_run(&domain, &ask, &mut why), Answer::Refuse);
    assert!(saw(&why, Finding::Arithmetic), "invalid accounting never becomes a readiness-only wait");
    let mut ask = base.clone();
    ask.accounts = Box::new([true, true, true]);
    let mut why = findings(&domain);
    assert_eq!(check_run(&domain, &ask, &mut why), Answer::Refuse);
    let mut ask = base;
    ask.project = 99;
    let mut why = findings(&domain);
    assert_eq!(check_run(&domain, &ask, &mut why), Answer::Refuse);
}

#[test]
fn call_cells_check_family_reads_references_scopes_and_ceiling_precedence() {
    let domain = domain();
    let base = CallAsk { project: 1, authority: authority(), family: Tools(1), call: Call::Tool };
    for call in [Call::Tool, Call::Read(effect()), Call::Message { referenced: true }, Call::Note(Scopes::GOAL)] {
        let mut ask = base.clone();
        ask.call = call;
        let mut why = findings(&domain);
        assert_eq!(check_call(&domain, &ask, &mut why), Answer::Allow);
    }
    let mut ask = base.clone();
    ask.authority.tools = Tools(0);
    let mut why = findings(&domain);
    assert_eq!(check_call(&domain, &ask, &mut why), Answer::Propose);
    assert!(saw(&why, Finding::Tool), "family membership is checked");
    let mut ask = base.clone();
    ask.call = Call::Read(effect());
    ask.authority.grants = Box::new([]);
    let mut why = findings(&domain);
    assert_eq!(check_call(&domain, &ask, &mut why), Answer::Propose);
    let mut ask = base.clone();
    ask.call = Call::Note(Scopes::PROJECT);
    ask.authority.notes = Scopes(0);
    let mut why = findings(&domain);
    assert_eq!(check_call(&domain, &ask, &mut why), Answer::Propose);
    let mut ask = base.clone();
    ask.authority.tools = Tools(0);
    ask.call = Call::Message { referenced: false };
    let mut why = findings(&domain);
    assert_eq!(check_call(&domain, &ask, &mut why), Answer::Refuse, "missing standing wins over a family proposal");
    assert!(saw(&why, Finding::Reference), "messages need a reference");
    let mut limited = authority();
    limited.tools = Tools(0);
    limited.notes = Scopes(0);
    limited.grants = Box::new([]);
    let restricted = domain_with(limited.clone(), limited);
    for call in [Call::Tool, Call::Read(effect()), Call::Note(Scopes::DEPLOYMENT)] {
        let mut ask = base.clone();
        ask.call = call;
        let mut why = findings(&restricted);
        assert_eq!(check_call(&restricted, &ask, &mut why), Answer::Refuse);
    }
    let mut ask = base.clone();
    ask.family = Tools(3);
    let mut why = findings(&domain);
    assert_eq!(check_call(&domain, &ask, &mut why), Answer::Refuse, "one call names exactly one configured family");
    let mut ask = base.clone();
    ask.call = Call::Note(Scopes(3));
    let mut why = findings(&domain);
    assert_eq!(check_call(&domain, &ask, &mut why), Answer::Refuse, "a note names one scope");
    let mut ask = base;
    ask.project = 99;
    let mut why = findings(&domain);
    assert_eq!(check_call(&domain, &ask, &mut why), Answer::Refuse);
}

#[test]
fn person_cells_check_each_request_role_rights_funding_and_proposal_rights() {
    let mut domain = domain();
    let base = PersonAsk { project: 1, role: 7, pool: numbers(100), tasks_left: 10, request: PersonRequest::Watch };
    let giving = child(20).authority;
    for request in [
        PersonRequest::Create(Box::new([child(20)])),
        PersonRequest::Allot(giving.clone()),
        PersonRequest::Amend(giving.clone()),
        PersonRequest::Move(giving.clone()),
        PersonRequest::Cancel,
        PersonRequest::Release,
        PersonRequest::Watch,
        PersonRequest::Policy,
        PersonRequest::Accept(Action::Batch(Box::new([child(20)]))),
        PersonRequest::Accept(Action::Effect(effect())),
        PersonRequest::Accept(Action::Widen(giving.clone())),
        PersonRequest::Accept(Action::Amend(giving.clone())),
        PersonRequest::Accept(Action::Escalate { release: None }),
        PersonRequest::Accept(Action::Escalate { release: Some(giving.clone()) }),
    ] {
        let ask = PersonAsk { request, ..base.clone() };
        let mut why = findings(&domain);
        assert_eq!(check_request(&domain, &ask, &mut why).answer, Answer::Allow, "request={:?}", ask.request);
    }
    let mut ask = base.clone();
    ask.request = PersonRequest::Allot(giving.clone());
    ask.pool.reserved = 90;
    let mut why = findings(&domain);
    assert_eq!(
        check_request(&domain, &ask, &mut why).answer,
        Answer::Refuse,
        "funding must be available in this role's pool"
    );
    let mut ask = base.clone();
    ask.request = PersonRequest::Allot(giving.clone());
    ask.pool.budget = 101;
    let mut why = findings(&domain);
    assert_eq!(check_request(&domain, &ask, &mut why).answer, Answer::Refuse);
    assert!(saw(&why, Finding::PeriodSpend), "a funding pool cannot bypass its role's period cap");
    let mut ask = base.clone();
    ask.role = 99;
    let mut why = findings(&domain);
    assert_eq!(check_request(&domain, &ask, &mut why).answer, Answer::Refuse);
    let mut ask = base.clone();
    ask.project = 99;
    let mut why = findings(&domain);
    assert_eq!(check_request(&domain, &ask, &mut why).answer, Answer::Refuse);
    let mut replacement = policy(authority());
    replacement.roles[0].requests = Requests(0);
    assert_eq!(
        apply(&mut domain, Event::Policy { project: 1, policy: replacement }),
        PolicyFact::Changed { project: 1 }
    );
    let mut why = findings(&domain);
    assert_eq!(check_request(&domain, &base, &mut why).answer, Answer::Refuse);
    assert!(saw(&why, Finding::Unpermitted), "roles govern non-funding requests too");
    let mut replacement = policy(authority());
    replacement.roles[0].decides = Proposals(0);
    apply(&mut domain, Event::Policy { project: 1, policy: replacement });
    let ask = PersonAsk { request: PersonRequest::Accept(Action::Effect(effect())), ..base.clone() };
    let mut why = findings(&domain);
    assert_eq!(check_request(&domain, &ask, &mut why).answer, Answer::Refuse);
    assert!(saw(&why, Finding::Undecidable), "accept needs rights for that proposal kind");
    let mut replacement = policy(authority());
    replacement.roles[0].authority.grants = Box::new([]);
    apply(&mut domain, Event::Policy { project: 1, policy: replacement });
    let mut why = findings(&domain);
    assert_eq!(check_request(&domain, &ask, &mut why).answer, Answer::Refuse);
    assert!(saw(&why, Finding::Grant { source: Source::Role }), "an accepter needs its own grant");
    let mut huge = base;
    huge.request = PersonRequest::Create(Box::new([child(0), child(0), child(0), child(0)]));
    let mut why = findings(&domain);
    assert_eq!(check_request(&domain, &huge, &mut why).answer, Answer::Refuse);
    assert!(saw(&why, Finding::Oversized), "person requests are admitted before scans");
}

#[test]
fn fitting_laws_needs_and_holder_depth_use_separate_current_inputs() {
    let domain = domain();
    let creator = authority();
    let mut lacks = Queue::with_capacity(FITS_MAX_OUT);
    for spent in 0..5_u64 {
        for tasks_left in 0..4_u32 {
            for task_count in 0..3_u32 {
                let mut child = child(3).authority;
                child.delegation.tasks = task_count;
                child.delegation.depth = 1;
                let current = Numbers { spent, ..numbers(5) };
                let actual = fits(&child, &creator, &current, tasks_left, &domain.rules().implies, &mut lacks);
                assert_eq!(
                    actual,
                    spent <= 2 && task_count <= tasks_left,
                    "fitting follows independent scalar capacity"
                );
                if actual {
                    let mut adjusted = creator.clone();
                    adjusted.budget.spend = 5_u64.saturating_sub(spent);
                    adjusted.delegation.tasks = tasks_left;
                    adjusted.delegation.depth = 4;
                    assert!(
                        at_most(&child, &adjusted, &domain.rules().implies),
                        "fitting never exceeds adjusted authority"
                    );
                }
                for _ in 0..FITS_MAX_OUT {
                    if lacks.pop().is_none() {
                        break;
                    }
                }
            }
        }
    }
    let mut bottom = child(0).authority;
    bottom.tools = Tools(0);
    bottom.grants = Box::new([]);
    bottom.delegation.kinds = Box::new([]);
    bottom.budget.deadline = Some(Wall::EPOCH);
    bottom.notes = Scopes(0);
    assert!(!fits(&creator, &bottom, &numbers(0), 0, &domain.rules().implies, &mut lacks), "all deficits are reported");
    assert_eq!(lacks.len(), FITS_MAX_OUT);
    let mut depth_lacked = false;
    for lack in &lacks {
        if *lack == Lack::Depth {
            depth_lacked = true;
        }
    }
    assert!(depth_lacked, "zero creator depth does not permit even a depth-zero child");

    let mut first = child(10);
    first.authority.delegation.tasks = 1;
    first.authority.delegation.depth = 1;
    first.authority.tools = Tools(1);
    first.authority.notes = Scopes::GOAL;
    let mut second = child(20);
    second.authority.delegation.tasks = 2;
    second.authority.delegation.depth = 2;
    second.executor = crate::Executor::Procedure(2);
    second.authority.tools = Tools(2);
    second.authority.notes = Scopes::REPOSITORY;
    second.authority.budget.deadline = Some(Wall::from_nanos(8));
    let needed = needs(&Action::Batch(Box::new([first, second]))).unwrap();
    assert_eq!(needed.delegation.tasks, 5, "one task plus each child's future capacity");
    assert_eq!(needed.delegation.depth, 3, "creation adds exactly one level");
    assert_eq!(needed.budget.spend, 30);
    assert_eq!(needed.budget.deadline, None, "no deadline is later than every finite deadline");
    assert_eq!(needed.tools, Tools(3));
    assert_eq!(needed.notes, Scopes(3));
    assert_eq!(needed.grants.len(), 2);
    assert!(
        needed.delegation.kinds.contains(&crate::Executor::Charter(1))
            && needed.delegation.kinds.contains(&crate::Executor::Procedure(2)),
        "actual executors are among needs"
    );
    let holder = Holder::Task { project: 1, authority: creator, numbers: numbers(100), tasks_left: 10 };
    assert!(covers(&domain, &needed, &holder, 2), "depth includes action depth plus verified distance");
    assert!(!covers(&domain, &needed, &holder, 3), "creation depth is not decremented a second time");
    assert!(!covers(&domain, &needed, &holder, u32::MAX), "distance overflow refuses coverage");
    let holder =
        Holder::Person { project: 1, role: 7, proposal: ProposalKind::Batch, pool: numbers(100), tasks_left: 4 };
    assert!(!covers(&domain, &needed, &holder, 0), "task capacity cannot come from the four funding numbers");
    let holder =
        Holder::Person { project: 1, role: 7, proposal: ProposalKind::Batch, pool: numbers(100), tasks_left: 10 };
    assert!(covers(&domain, &needed, &holder, 0), "role coverage checks actual funding and action needs");
    let holder =
        Holder::Person { project: 1, role: 99, proposal: ProposalKind::Batch, pool: numbers(100), tasks_left: 10 };
    assert!(!covers(&domain, &needed, &holder, 0), "an absent role cannot cover a proposal");
    let effect_need = needs(&Action::Effect(effect())).unwrap();
    assert_eq!(effect_need.grants[0].pattern.last, Last::None, "a proposed effect needs only its exact name");
    assert_eq!(effect_need.budget.spend, 0);
    assert_eq!(needs(&Action::Escalate { release: None }).unwrap().grants.len(), 0);
    let mut overflowing = child(u64::MAX);
    overflowing.authority.delegation.tasks = u32::MAX;
    assert_eq!(needs(&Action::Batch(Box::new([overflowing]))), None, "needs refuses arithmetic overflow");
}

fn add(a: u64, b: u64) -> u64 {
    a.checked_add(b).unwrap()
}

fn sized(size: usize) -> u64 {
    u64::try_from(size).unwrap()
}

fn pattern_heap(pattern: &Pattern) -> u64 {
    let mut bytes = sized(size_of_val(pattern.segments.as_ref()));
    for segment in &pattern.segments {
        bytes = add(bytes, sized(segment.len()));
    }
    match &pattern.last {
        Last::None => {}
        Last::Exact(last) | Last::Open(last) => bytes = add(bytes, sized(last.len())),
    }
    bytes
}

fn authority_heap(authority: &Authority) -> u64 {
    let mut bytes =
        add(sized(size_of_val(authority.grants.as_ref())), sized(size_of_val(authority.delegation.kinds.as_ref())));
    for grant in &authority.grants {
        bytes = add(bytes, pattern_heap(&grant.pattern));
    }
    bytes
}

fn requirements_heap(requirements: &[Requirement]) -> u64 {
    let mut bytes = sized(size_of_val(requirements));
    for requirement in requirements {
        bytes = add(add(bytes, pattern_heap(&requirement.pattern)), sized(size_of_val(requirement.facts.as_ref())));
    }
    bytes
}

fn full_pattern() -> Pattern {
    Pattern {
        segments: Box::new([copy_of(b"12345678"), copy_of(b"12345678"), copy_of(b"12345678"), copy_of(b"12345678")]),
        last: Last::Open(copy_of(b"12345678")),
    }
}

fn full_authority() -> Authority {
    let mut authority = authority();
    let grant = Grant { connector: 1, kind: 3, pattern: full_pattern() };
    authority.grants = Box::new([grant.clone(), grant.clone(), grant]);
    authority
}

fn full_requirements() -> Box<[Requirement]> {
    let requirement = Requirement { connector: 1, kind: 1, pattern: full_pattern(), facts: Box::new([7, 7, 7, 7]) };
    Box::new([requirement.clone(), requirement])
}

#[test]
fn policy_lifecycle_refuses_bad_replacements_and_memory_bounds_full_owned_containers() {
    let mut domain = domain();
    assert_eq!(
        apply(&mut domain, Event::Policy { project: 2, policy: policy(authority()) }),
        PolicyFact::Added { project: 2 }
    );
    assert_eq!(
        apply(&mut domain, Event::Policy { project: 3, policy: policy(authority()) }),
        PolicyFact::Refused { project: 3, reason: PolicyRefusal::Full }
    );
    assert_eq!(
        apply(&mut domain, Event::Policy { project: 1, policy: policy(authority()) }),
        PolicyFact::Changed { project: 1 }
    );
    for cause in 0_u8..4 {
        let mut invalid = policy(authority());
        let expected = match cause {
            0 => {
                invalid.roles = Box::new([role(authority()), role(authority()), role(authority())]);
                PolicyRefusal::Oversized
            }
            1 => {
                invalid.ceiling.budget.spend = 101;
                PolicyRefusal::AboveRules
            }
            2 => {
                invalid.roles = Box::new([role(authority()), role(authority())]);
                PolicyRefusal::InvalidRole
            }
            _ => {
                invalid.roles[0].requests = Requests(512);
                PolicyRefusal::InvalidRole
            }
        };
        assert_eq!(
            apply(&mut domain, Event::Policy { project: 1, policy: invalid }),
            PolicyFact::Refused { project: 1, reason: expected }
        );
        assert_eq!(
            domain.policy(1).unwrap().ceiling.budget.spend,
            100,
            "a bad replacement retains its previous policy"
        );
    }
    assert_eq!(apply(&mut domain, Event::Dropped { project: 2 }), PolicyFact::Dropped { project: 2, existed: true });
    assert_eq!(apply(&mut domain, Event::Dropped { project: 2 }), PolicyFact::Dropped { project: 2, existed: false });
    let mut invalid = rules(authority());
    invalid.minimum_run_spend = invalid.maximum_run_spend;
    assert!(Domain::new(invalid, LIMITS).is_none(), "invalid run bounds are refused at configuration");
    let mut invalid_limits = LIMITS;
    invalid_limits.implications = 2;
    assert!(Domain::new(rules(authority()), invalid_limits).is_none(), "implication storage is bounded too");
    let mut invalid = policy(authority());
    invalid.period_spend = 1_001;
    assert_eq!(
        apply(&mut domain, Event::Policy { project: 1, policy: invalid }),
        PolicyFact::Refused { project: 1, reason: PolicyRefusal::AboveRules }
    );
    let mut invalid = policy(authority());
    invalid.roles[0].period_spend = 501;
    assert_eq!(
        apply(&mut domain, Event::Policy { project: 1, policy: invalid }),
        PolicyFact::Refused { project: 1, reason: PolicyRefusal::InvalidRole }
    );
    let mut invalid = policy(authority());
    invalid.roles[0].authority.budget.spend = 101;
    assert_eq!(
        apply(&mut domain, Event::Policy { project: 1, policy: invalid }),
        PolicyFact::Refused { project: 1, reason: PolicyRefusal::InvalidRole }
    );
    let mut invalid = policy(authority());
    invalid.roles[0].decides = Proposals(32);
    assert_eq!(
        apply(&mut domain, Event::Policy { project: 1, policy: invalid }),
        PolicyFact::Refused { project: 1, reason: PolicyRefusal::InvalidRole }
    );

    check_full_policy_memory();
}

fn check_full_policy_memory() {
    let mut configured = rules(full_authority());
    configured.requirements = full_requirements();
    let mut resident = policy(full_authority());
    resident.requirements = full_requirements();
    let first_role = role(full_authority());
    let mut second_role = first_role.clone();
    second_role.number = 8;
    resident.roles = Box::new([first_role, second_role]);
    let mut held = add(authority_heap(&configured.ceiling), requirements_heap(&configured.requirements));
    held = add(held, u64::from(configured.implies.len()).checked_mul(sized(size_of::<Implication>())).unwrap());
    held = add(held, Map::<u32, Policy>::worst_case(LIMITS.projects).unwrap());
    let mut one = add(authority_heap(&resident.ceiling), requirements_heap(&resident.requirements));
    one = add(one, sized(size_of_val(resident.roles.as_ref())));
    for role in &resident.roles {
        one = add(one, authority_heap(&role.authority));
    }
    held = add(held, one.checked_mul(u64::from(LIMITS.projects)).unwrap());
    let mut full = Domain::new(configured, LIMITS).unwrap();
    for project in 0..LIMITS.projects {
        assert_eq!(
            apply(&mut full, Event::Policy { project, policy: resident.clone() }),
            PolicyFact::Added { project }
        );
    }
    assert_eq!(
        worst_case(&LIMITS),
        Some(held),
        "full owned boxes plus the map's node bound equal the advertised heap bound"
    );
    let enormous = Limits {
        projects: u32::MAX,
        roles: u32::MAX,
        requirements: u32::MAX,
        facts: u32::MAX,
        grants: u32::MAX,
        executors: u32::MAX,
        segments: u32::MAX,
        segment_bytes: u32::MAX,
        implications: u32::MAX,
        batch: u32::MAX,
        accounts: u32::MAX,
        writes: u32::MAX,
        landing_rules: u32::MAX,
        gates: u32::MAX,
        approvals: u32::MAX,
        heads: u32::MAX,
        verdicts: u32::MAX,
        reviews: u32::MAX,
    };
    assert_eq!(worst_case(&enormous), None, "overflowing memory is never wrapped");
    assert_eq!(max_out(&enormous), None, "overflowing queue bounds are never wrapped");
}
