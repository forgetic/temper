//! Sparse entrance facts without invented names, words or bounds (jig's
//! client/domain/contract.md, 4).
use jig_client_protocol_wire::v19 as w;
use jig_domain_people as p;
use skein_lib::Writer;
#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive translation preserves the complete embedded refusal vocabulary"
)]
pub(crate) fn refusal(value: p::Refusal, limits: &w::Limits) -> Result<w::Refusal, w::Problem> {
    Ok(match value {
        // Only ChangeSettings emits Stale; forge applications never carry that ask.
        p::Refusal::Stale { .. }
        // Only settings admission emits Full; a forge application carries no settings value.
        | p::Refusal::Full { .. }
        // Only finishing an account sign-in emits SignInRedirect.
        | p::Refusal::SignInRedirect { .. } => {
            return Err(w::Problem { path: w::Path::RefusalTag, reason: skein_codec::Reason::Bound });
        }
        p::Refusal::Account { account, why } => w::Refusal::Account(w::RefusalAccount::new(
            limits,
            w::RefusalAccountParts {
                account,
                why: match why {
                    p::AccountWhy::Unknown => w::AccountWhy::Unknown,
                    p::AccountWhy::Provider => w::AccountWhy::Provider,
                    p::AccountWhy::Uses => w::AccountWhy::Uses,
                    p::AccountWhy::Limited => w::AccountWhy::Limited,
                },
            },
        )?),
        p::Refusal::NotWellFormed { at, why } => w::Refusal::Malformed(w::RefusalMalformed::new(
            limits,
            w::RefusalMalformedParts {
                at,
                why: match why {
                    // Forge application values contain no settings for the core to check.
                    p::Malformed::Settings { .. }
                    // Only a settings change can leave the effective settings unchanged.
                    | p::Malformed::SettingsUnchanged => {
                        return Err(w::Problem { path: w::Path::MalformedTag, reason: skein_codec::Reason::Bound });
                    }
                    p::Malformed::AsAllowedTwice => w::Malformed::TwoAsAllowed,
                    p::Malformed::Conversation => w::Malformed::ConversationNotAgent,
                    p::Malformed::Name(rule) => w::Malformed::Name(w::MalformedName::new(
                        limits,
                        w::MalformedNameParts {
                            rule: match rule {
                                p::NameRule::Empty => w::NameRule::Empty,
                                p::NameRule::Long => w::NameRule::Long,
                                p::NameRule::Control => w::NameRule::Control,
                                p::NameRule::Space => w::NameRule::Space,
                                p::NameRule::Digits => w::NameRule::Digits,
                            },
                        },
                    )?),
                    p::Malformed::Cycle => w::Malformed::Cycle,
                    p::Malformed::EndedBy => w::Malformed::EndedBy,
                    p::Malformed::ProposedAsAllowed => w::Malformed::ProposedAsAllowed,
                },
            },
        )?),
        p::Refusal::Role { held } => {
            let held = match held {
                Some(value) => Some(role(value, limits)?),
                None => None,
            };
            w::Refusal::Role(w::RefusalRole::new(limits, w::RefusalRoleParts { held })?)
        }
        p::Refusal::Authority { lacking, proposable } => w::Refusal::Authority(w::RefusalAuthority::new(
            limits,
            w::RefusalAuthorityParts {
                lacking: lack(lacking, limits)?,
                proposable: match proposable {
                    Some(p::Accepter::Role(_)) => Some(true),
                    Some(p::Accepter::Nobody) => Some(false),
                    None => None,
                },
            },
        )?),
        p::Refusal::Limit { which } => {
            let which =
                w::Limit::Admission(w::LimitAdmission::new(limits, w::LimitAdmissionParts { which: limit(which) })?);
            w::Refusal::Limit(w::RefusalLimit::new(limits, w::RefusalLimitParts { which })?)
        }
        p::Refusal::Moved { now } => moved(now, limits)?,
        p::Refusal::Ended { how } => {
            w::Refusal::Ended(w::RefusalEnded::new(limits, w::RefusalEndedParts { how: ending(how, limits)? })?)
        }
        p::Refusal::Circle { tasks, cut } => w::Refusal::Circle(w::RefusalCircle::new(
            limits,
            w::RefusalCircleParts {
                tasks: numbers(&tasks, limits.refusal_circle_tasks, w::Path::RefusalCircleTasks)?,
                cut,
            },
        )?),
        p::Refusal::Dependency { dependency, how } => w::Refusal::Dependency(w::RefusalDependency::new(
            limits,
            w::RefusalDependencyParts {
                dependency,
                how: match how {
                    p::DependencyStatus::Done => w::OwnerTaskStatus::Done,
                    p::DependencyStatus::Failed => w::OwnerTaskStatus::Failed,
                    p::DependencyStatus::Cancelled => w::OwnerTaskStatus::Cancelled,
                },
            },
        )?),
        p::Refusal::Kept { pending } => w::Refusal::Kept(w::RefusalKept::new(
            limits,
            w::RefusalKeptParts {
                pending: numbers(&pending, limits.refusal_kept_pending, w::Path::RefusalKeptPending)?,
            },
        )?),
        p::Refusal::Taken { by } => w::Refusal::Taken(w::RefusalTaken::new(limits, w::RefusalTakenParts { by })?),
        p::Refusal::NotWaiting => w::Refusal::NotWaiting,
        p::Refusal::NoFurther => w::Refusal::NoFurther,
        p::Refusal::NameTaken => w::Refusal::NameTaken,
        p::Refusal::Unknown => w::Refusal::Unknown,
        p::Refusal::NotOffered => w::Refusal::NotOffered,
        p::Refusal::Provider => w::Refusal::Provider,
        p::Refusal::NoEscalationHolder => w::Refusal::NoEscalationHolder,
        p::Refusal::KeyConflict => w::Refusal::KeyConflict,
        p::Refusal::AccountMove { why } => w::Refusal::AccountMove(w::RefusalAccountMove::new(
            limits,
            w::RefusalAccountMoveParts {
                why: match why {
                    p::AccountMoveWhy::NotSignedIn => w::AccountMoveWhy::NotSignedIn,
                    p::AccountMoveWhy::NotOurs => w::AccountMoveWhy::NotOurs,
                    p::AccountMoveWhy::Held => w::AccountMoveWhy::Held,
                    p::AccountMoveWhy::Cancelled => w::AccountMoveWhy::Cancelled,
                    p::AccountMoveWhy::Moving => w::AccountMoveWhy::Moving,
                },
            },
        )?),
        // As jig's entrance facts: only a made or amended task's refusal names a model, by the
        // configured models, which a fact here has none of.
        p::Refusal::Llm { .. } => {
            return Err(w::Problem { path: w::Path::RefusalLlmModel, reason: skein_codec::Reason::Bound });
        }
    })
}
fn role(value: p::Role, limits: &w::Limits) -> Result<w::HeldRole, w::Problem> {
    Ok(match value {
        p::Role::Owner => w::HeldRole::Owner,
        p::Role::Maintainer => w::HeldRole::Maintainer,
        p::Role::Member => w::HeldRole::Member,
        p::Role::Observer => w::HeldRole::Observer,
        p::Role::Policy { role } => {
            w::HeldRole::Policy(w::HeldRolePolicy::new(limits, w::HeldRolePolicyParts { role })?)
        }
    })
}
fn lack(value: p::Lack, limits: &w::Limits) -> Result<w::Lacking, w::Problem> {
    Ok(match value {
        p::Lack::Approver => w::Lacking::Approver,
        p::Lack::Funding => w::Lacking::Funding,
        p::Lack::Tool => w::Lacking::Tool,
        p::Lack::Scope => w::Lacking::Scope,
        p::Lack::Reference => w::Lacking::Reference,
        p::Lack::ResourceAccess => w::Lacking::ResourceAccess,
        p::Lack::ResourceUnavailable => w::Lacking::ResourceUnavailable,
        p::Lack::Shape => w::Lacking::Shape,
        p::Lack::Account => w::Lacking::Account,
        p::Lack::Writer => w::Lacking::Writer,
        p::Lack::Models => w::Lacking::Models,
        p::Lack::Budget { needs, left } => {
            w::Lacking::Budget(w::LackingBudget::new(limits, w::LackingBudgetParts { needed: needs, left })?)
        }
        p::Lack::Grant { kind } => {
            w::Lacking::GrantKind(w::LackingGrantKind::new(limits, w::LackingGrantKindParts { kind })?)
        }
        p::Lack::TaskKind { .. } => {
            w::Lacking::TaskKind(w::LackingTaskKind::new(limits, w::LackingTaskKindParts { executor: None })?)
        }
        p::Lack::Depth { most } => {
            w::Lacking::Depth(w::LackingDepth::new(limits, w::LackingDepthParts { needed: None, allowed: most })?)
        }
        p::Lack::Deadline => w::Lacking::Deadline(w::LackingDeadline::new(
            limits,
            w::LackingDeadlineParts { asked: None, latest: None },
        )?),
        p::Lack::Tasks { left } => {
            w::Lacking::Tasks(w::LackingTasks::new(limits, w::LackingTasksParts { needed: None, left })?)
        }
        p::Lack::Again { required } => {
            w::Lacking::Again(w::LackingAgain::new(limits, w::LackingAgainParts { required })?)
        }
        p::Lack::Required { connector, kind } => {
            w::Lacking::Required(w::LackingRequired::new(limits, w::LackingRequiredParts { connector, kind })?)
        }
        p::Lack::Failed { connector, kind } => {
            w::Lacking::Failed(w::LackingFailed::new(limits, w::LackingFailedParts { connector, kind })?)
        }
        p::Lack::Unguarded { connector, kind } => {
            w::Lacking::Unguarded(w::LackingUnguarded::new(limits, w::LackingUnguardedParts { connector, kind })?)
        }
    })
}
pub(super) fn limit(value: p::LimitKind) -> w::AdmissionLimit {
    match value {
        p::LimitKind::Words => w::AdmissionLimit::Words,
        p::LimitKind::Identity => w::AdmissionLimit::Identity,
        p::LimitKind::Roles => w::AdmissionLimit::Roles,
        p::LimitKind::Arithmetic => w::AdmissionLimit::Arithmetic,
        p::LimitKind::Duplicate => w::AdmissionLimit::Duplicate,
        p::LimitKind::Empty => w::AdmissionLimit::Empty,
        p::LimitKind::Batch => w::AdmissionLimit::Batch,
        p::LimitKind::ProjectLiveTasks => w::AdmissionLimit::ProjectLiveTasks,
        p::LimitKind::Projects => w::AdmissionLimit::Projects,
        p::LimitKind::Goals => w::AdmissionLimit::Goals,
        p::LimitKind::LiveTasks => w::AdmissionLimit::LiveTasks,
        p::LimitKind::Tree => w::AdmissionLimit::Tree,
        p::LimitKind::Depth => w::AdmissionLimit::Depth,
        p::LimitKind::Delegates => w::AdmissionLimit::Delegates,
        p::LimitKind::Subscription => w::AdmissionLimit::Subscription,
        p::LimitKind::Dependencies => w::AdmissionLimit::Dependencies,
        p::LimitKind::Cycle => w::AdmissionLimit::Cycle,
        p::LimitKind::Operations => w::AdmissionLimit::Operations,
        p::LimitKind::Grace => w::AdmissionLimit::Grace,
        p::LimitKind::Spec => w::AdmissionLimit::Spec,
        p::LimitKind::Contract => w::AdmissionLimit::Contract,
        p::LimitKind::Authority => w::AdmissionLimit::Authority,
        p::LimitKind::Inputs => w::AdmissionLimit::Inputs,
        p::LimitKind::Attempts => w::AdmissionLimit::Attempts,
        p::LimitKind::LiveDelegates => w::AdmissionLimit::LiveDelegates,
        p::LimitKind::Restore => w::AdmissionLimit::Restore,
        p::LimitKind::Read => w::AdmissionLimit::Read,
        p::LimitKind::Turn => w::AdmissionLimit::Turn,
        p::LimitKind::HoldKind => w::AdmissionLimit::HoldKind,
        p::LimitKind::HoldTaken => w::AdmissionLimit::HoldTaken,
        p::LimitKind::Holds => w::AdmissionLimit::Holds,
        p::LimitKind::InboxFull => w::AdmissionLimit::InboxFull,
        p::LimitKind::Notes => w::AdmissionLimit::Notes,
        p::LimitKind::Revision => w::AdmissionLimit::Revision,
        p::LimitKind::Question => w::AdmissionLimit::Question,
        p::LimitKind::Priority => w::AdmissionLimit::Priority,
        p::LimitKind::Policy => w::AdmissionLimit::Policy,
        p::LimitKind::Resource => w::AdmissionLimit::Resource,
        p::LimitKind::Service => w::AdmissionLimit::Service,
        p::LimitKind::Accounts | p::LimitKind::HolderAccounts | p::LimitKind::Sources | p::LimitKind::SignIns => {
            unreachable!("an application asks for no account")
        }
    }
}
fn moved(value: p::Moved, limits: &w::Limits) -> Result<w::Refusal, w::Problem> {
    let (revision, what) = match value {
        p::Moved::Hold { revision, reason } => {
            let value = hold(reason, limits)?;
            let reason =
                Some(w::CurrentHold::Reason(w::CurrentHoldReason::new(limits, w::CurrentHoldReasonParts { value })?));
            (revision, w::Moved::Hold(w::MovedHold::new(limits, w::MovedHoldParts { reason })?))
        }
        p::Moved::Spec { revision } => (revision, w::Moved::Spec),
        p::Moved::Person { revision } => (revision, w::Moved::PersonTask),
        p::Moved::Note { revision } => (u64::from(revision), w::Moved::Note),
    };
    let now = w::Revision::new(limits, w::RevisionParts { value: revision })?;
    Ok(w::Refusal::Moved(w::RefusalMoved::new(limits, w::RefusalMovedParts { now, what })?))
}
fn hold(value: p::HoldReason, limits: &w::Limits) -> Result<w::HoldCategory, w::Problem> {
    Ok(match value {
        p::HoldReason::Exceeded(value) => {
            let bound = match value.bound {
                p::Bound::Reasoning => w::Bound::Reasoning,
                p::Bound::Metadata => w::Bound::Metadata,
                p::Bound::Items => w::Bound::Items,
                p::Bound::Answer => w::Bound::Answer,
                p::Bound::Tools => w::Bound::Tools,
                p::Bound::HistoryItems => w::Bound::HistoryItems,
                p::Bound::History => w::Bound::History,
                p::Bound::Window => w::Bound::Window,
                p::Bound::Output => w::Bound::Output,
            };
            w::HoldCategory::Exceeded(w::Exceeded::new(limits, w::ExceededParts { bound, value: value.value })?)
        }
        p::HoldReason::Stalled => w::HoldCategory::Stalled,
        p::HoldReason::Stopped => w::HoldCategory::Stopped,
        p::HoldReason::Drift => w::HoldCategory::Drift,
        p::HoldReason::Effects => w::HoldCategory::Effects,
        p::HoldReason::EffectFailed => w::HoldCategory::EffectFailed,
        p::HoldReason::Budget => w::HoldCategory::Budget,
        p::HoldReason::Deadline => w::HoldCategory::Deadline,
        p::HoldReason::HoldsWaited => w::HoldCategory::HoldsWaited,
        p::HoldReason::ResourceUnavailable => w::HoldCategory::ResourceUnavailable,
        p::HoldReason::ResourceAccess => w::HoldCategory::ResourceAccess,
        p::HoldReason::Circle => w::HoldCategory::Circle,
        p::HoldReason::Procedure => w::HoldCategory::Procedure,
        p::HoldReason::Dependency { task } => w::HoldCategory::Dependency(w::HoldCategoryDependency::new(
            limits,
            w::HoldCategoryDependencyParts { task },
        )?),
        p::HoldReason::StoppedBy { party } => {
            w::HoldCategory::StoppedBy(w::HoldCategoryStoppedBy::new(limits, w::HoldCategoryStoppedByParts { party })?)
        }
        p::HoldReason::Uncertain { entry } => {
            w::HoldCategory::Uncertain(w::HoldCategoryUncertain::new(limits, w::HoldCategoryUncertainParts { entry })?)
        }
        p::HoldReason::PoolLost { pool } => {
            w::HoldCategory::PoolLost(w::HoldCategoryPoolLost::new(limits, w::HoldCategoryPoolLostParts { pool })?)
        }
        p::HoldReason::Failures { class } => {
            let class = match class {
                p::FailureClass::Transient => w::HoldFailure::Transient,
                p::FailureClass::Permanent => w::HoldFailure::Permanent,
                p::FailureClass::Run => w::HoldFailure::Run,
                p::FailureClass::Agent => w::HoldFailure::Agent,
                p::FailureClass::Lost => w::HoldFailure::Lost,
                p::FailureClass::Invalid => w::HoldFailure::Invalid,
            };
            w::HoldCategory::Failures(w::HoldCategoryFailures::new(limits, w::HoldCategoryFailuresParts { class })?)
        }
    })
}
fn actor(value: p::Party, limits: &w::Limits) -> Result<w::Actor, w::Problem> {
    Ok(match value {
        p::Party::Person(value) => w::Actor::Party(w::ActorParty::new(limits, w::ActorPartyParts { value })?),
        p::Party::Task(value) => w::Actor::Task(w::ActorTask::new(limits, w::ActorTaskParts { value })?),
        p::Party::Deployment { project } => w::Actor::ProjectDeployment(w::ActorProjectDeployment::new(
            limits,
            w::ActorProjectDeploymentParts { project },
        )?),
    })
}
fn ending(value: p::Ending, limits: &w::Limits) -> Result<w::Ending, w::Problem> {
    let value = match value {
        p::Ending::Done => w::EndingFields::Done,
        p::Ending::Failed => {
            w::EndingFields::Failed(w::EndingFailedFields::new(limits, w::EndingFailedFieldsParts { reason: None })?)
        }
        p::Ending::Cancelled { by, at } => w::EndingFields::Cancelled(w::EndingCancelledFields::new(
            limits,
            w::EndingCancelledFieldsParts { by: actor(by, limits)?, reason: None, at: Some(at.as_nanos()) },
        )?),
        p::Ending::Decided { by, at } => w::EndingFields::Decided(w::EndingDecidedFields::new(
            limits,
            w::EndingDecidedFieldsParts { by, party: None, how: None, at: at.as_nanos() },
        )?),
        p::Ending::Deleted => w::EndingFields::Deleted,
    };
    let row = w::Fact::Ending(w::EndingRow::new(limits, w::EndingRowParts { parent: None, value })?);
    let size = row
        .measure()
        .checked_add(4)
        .ok_or(w::Problem { path: w::Path::EndingRows, reason: skein_codec::Reason::Bound })?;
    if size > limits.ending_rows {
        return Err(w::Problem { path: w::Path::EndingRows, reason: skein_codec::Reason::Bound });
    }
    let mut rows = Writer::new(usize::try_from(size).expect("wire size fits"));
    rows.put(&row.measure().to_be_bytes()).expect("measured piece length");
    row.encode(&mut rows).expect("measured ending row");
    w::Ending::new(limits, w::EndingParts { rows: rows.finish() })
}

fn numbers(values: &[u64], limit: u32, path: w::Path) -> Result<skein_lib::List<u64>, w::Problem> {
    let Ok(length) = u32::try_from(values.len()) else {
        return Err(w::Problem { path, reason: skein_codec::Reason::Bound });
    };
    if length > limit {
        return Err(w::Problem { path, reason: skein_codec::Reason::Bound });
    }
    let mut rows = skein_lib::List::with_capacity(length);
    for value in values {
        rows.push(*value).expect("checked refusal list length");
    }
    Ok(rows)
}
