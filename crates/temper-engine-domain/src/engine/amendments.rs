//! Authenticated person amendments (domain/people.md, 5.1; jig's domain/tasks.md, 7).
//! The root translates the person's typed payload and routes the core's
//! admission decision to tasks. Tasks retains the change and history.
use super::{Domain, Env, Limits, PersonTaskRoute, ReplyTo, Token, Work, people, person_control_refused, tasks};
use skein_lib::List;

fn wake_rule(value: people::WakeRule) -> tasks::WakeRule {
    match value {
        people::WakeRule::Never => tasks::WakeRule::Never,
        people::WakeRule::Immediate => tasks::WakeRule::Immediate,
        people::WakeRule::Batch { count, age } => tasks::WakeRule::Batch { count, age },
    }
}

fn wake(value: people::WakePolicy) -> tasks::WakePolicy {
    tasks::WakePolicy {
        words: wake_rule(value.words),
        notices: wake_rule(value.notices),
        news: wake_rule(value.news),
        results: match value.results {
            people::ResultsWake::Never => tasks::ResultsWake::Never,
            people::ResultsWake::Each => tasks::ResultsWake::Each,
            people::ResultsWake::LastOrFailure => tasks::ResultsWake::LastOrFailure,
        },
        questions: value.questions,
        answers: value.answers,
        timers: value.timers,
    }
}

fn spec(value: people::Spec, limits: &tasks::Limits) -> Option<tasks::Spec> {
    let mut parameters = List::with_capacity(limits.parameters);
    for parameter in value.parameters {
        let translated = match parameter {
            people::Parameter::Number { name, value } => tasks::Parameter::Number { name, value },
            people::Parameter::Bytes { name, value } => tasks::Parameter::Bytes { name, value },
            people::Parameter::Resource { name, connector, resource } => {
                tasks::Parameter::Resource { name, connector, resource }
            }
        };
        parameters.push(translated).ok()?;
    }
    Some(tasks::Spec { words: value.words, parameters: parameters.into_boxed(), inputs: value.inputs })
}

fn granted(value: people::Authority, limits: &tasks::Limits) -> Option<tasks::Authority> {
    let mut grants = List::with_capacity(limits.authority_grants);
    for grant in value.grants {
        let mut segments = List::with_capacity(limits.authority_segments);
        for segment in grant.pattern.segments {
            segments.push(segment).ok()?;
        }
        let last = match grant.pattern.last {
            people::Last::Exact(word) => tasks::Last::Exact(word),
            people::Last::Open(word) => tasks::Last::Open(word),
        };
        grants
            .push(tasks::Grant {
                connector: grant.connector,
                kind: grant.kind,
                pattern: tasks::Pattern { segments: segments.into_boxed(), last },
            })
            .ok()?;
    }
    let mut note_resources = List::with_capacity(limits.authority_grants);
    for scope in value.note_resources {
        let mut segments = List::with_capacity(limits.authority_segments);
        for segment in scope.pattern.segments {
            segments.push(segment).ok()?;
        }
        let last = match scope.pattern.last {
            people::Last::Exact(word) => tasks::Last::Exact(word),
            people::Last::Open(word) => tasks::Last::Open(word),
        };
        note_resources
            .push(tasks::ResourceScope {
                connector: scope.connector,
                pattern: tasks::Pattern { segments: segments.into_boxed(), last },
            })
            .ok()?;
    }
    let mut kinds = List::with_capacity(limits.executor_kinds);
    for kind in value.delegation.kinds {
        kinds
            .push(match kind {
                people::Executor::Charter(number) => tasks::AuthorityExecutor::Charter(number),
                people::Executor::Procedure(number) => tasks::AuthorityExecutor::Procedure(number),
                people::Executor::Role(number) => tasks::AuthorityExecutor::Role(number),
            })
            .ok()?;
    }
    Some(tasks::Authority {
        tools: tasks::Tools(value.tools),
        grants: grants.into_boxed(),
        delegation: tasks::Delegation {
            kinds: kinds.into_boxed(),
            tasks: value.delegation.tasks,
            depth: value.delegation.depth,
        },
        budget: tasks::Budget { spend: value.spend, deadline: value.deadline },
        notes: tasks::Scopes(value.notes),
        note_resources: note_resources.into_boxed(),
    })
}

#[expect(clippy::manual_map, reason = "the step subset spells out both option cases without a closure")]
fn translate(value: people::Amendment, limits: &tasks::Limits) -> Option<tasks::Amendment> {
    Some(tasks::Amendment {
        spec: match value.spec {
            Some(specification) => Some(spec(specification, limits)?),
            None => None,
        },
        wake: match value.wake {
            Some(policy) => Some(wake(policy)),
            None => None,
        },
        dependencies: value.dependencies,
        authority: match value.authority {
            Some(authority) => Some(granted(authority, limits)?),
            None => None,
        },
        reason: value.reason,
    })
}

/// Check this person's standing and current authority, then submit one full task amendment.
#[expect(clippy::too_many_arguments, reason = "one keyed person amendment names its caller, project and task")]
pub(super) fn begin(
    domain: &mut Domain,
    env: &Env<Limits>,
    request: Token,
    person: u64,
    role: Option<people::Role>,
    project: u32,
    task: u64,
    amendment: people::Amendment,
) {
    let Some(amendment) = translate(amendment, &env.limits.tasks) else {
        return person_control_refused(domain, request, people::Refusal::Limit);
    };
    let (stop_run, propose) = match domain.core.amend_admit(
        person,
        role,
        project,
        task,
        &amendment,
        env.limits.tasks.depth,
        &env.limits.tasks,
    ) {
        Ok(admitted) => admitted,
        Err(why) => return person_control_refused(domain, request, why),
    };
    if propose {
        let Some(proposal) = crate::fresh(&mut domain.core.counters, super::Family::Message) else {
            return person_control_refused(domain, request, people::Refusal::Limit);
        };
        assert!(
            domain.core.person_tasks.insert(request, PersonTaskRoute::AmendProposed { task, proposal }) == Ok(None),
            "one amendment proposal"
        );
        domain.work.push(Work::Tasks(tasks::Event::Propose {
            reply_to: ReplyTo::new(request),
            proposal: tasks::Proposal {
                number: proposal,
                proposer: task,
                project,
                reason: amendment.reason.clone(),
                as_holder: false,
                action: tasks::ProposalAction::Amend { task, amendment },
                state: tasks::ProposalState::Pending {
                    holder: tasks::ProposalHolder::Policy { project, kind: tasks::ProposalKind::Amend },
                    since: env.wall,
                },
            },
        }));
        return;
    }
    let Some(message) = crate::fresh(&mut domain.core.counters, super::Family::Message) else {
        return person_control_refused(domain, request, people::Refusal::Limit);
    };
    assert!(
        domain.core.person_tasks.insert(request, PersonTaskRoute::Amended(task)) == Ok(None),
        "one person amendment"
    );
    domain.work.push(Work::Tasks(tasks::Event::Amend {
        reply_to: ReplyTo::new(request),
        by: tasks::Party::Person(person),
        task,
        message,
        stop_run,
        amendment,
    }));
}
