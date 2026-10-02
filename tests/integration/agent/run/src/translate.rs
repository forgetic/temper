//! The mapping between the worker's vocabulary and the run's: what the two
//! protocol layers and the channel between them do, without the bytes.

use temper_agent_model_run as run;
use temper_agent_model_run::charter;
use temper_agent_model_run::outcome::{Change, ChangeSpec, Children, OutcomeSpec, VerdictRule};
use temper_fake_worker_model::api as worker;
use temper_lib::Token;

/// The run's charter for the worker's, its repositories at `roots`, as io
/// names them.
#[must_use]
pub fn charter(charter: worker::Charter, roots: &[Token]) -> run::Charter {
    let worker::Charter {
        brief,
        repositories,
        tools,
        forge,
        agents,
        outlets,
        outcome,
        budget,
        endpoint,
        model,
        max_tokens,
        models,
    } = charter;
    run::Charter {
        brief,
        checkout: charter::Checkout {
            repositories: repositories.into_iter().zip(roots).map(|(found, root)| repository(found, *root)).collect(),
        },
        grants: charter::Grants {
            tools: charter::Tools { inspect: tools.read, modify: tools.write, shell: tools.shell },
            forge,
            agents,
            outlets: outlets.into_iter().map(|name| charter::Outlet { name }).collect(),
        },
        outcome: OutcomeSpec {
            change: outcome.change.then_some(ChangeSpec { checks: outcome.checks }),
            verdicts: outcome.verdicts.into_iter().map(verdict).collect(),
        },
        budget: self::budget(budget),
        llm: charter::Llm { endpoint: charter::Endpoint(endpoint), model, max_tokens },
        models: models
            .into_iter()
            .map(|model| charter::Llm { endpoint: charter::Endpoint(endpoint), model, max_tokens })
            .collect(),
    }
}

/// The run's budget for the worker's.
#[must_use]
pub fn budget(budget: worker::Budget) -> run::Budget {
    run::Budget {
        turns: budget.turns,
        input: budget.input_tokens,
        output: budget.output_tokens,
        cache_read: budget.cache_read_tokens,
        cache_write: budget.cache_write_tokens,
        time: budget.wall_time,
    }
}

/// The worker's change for the run's.
#[must_use]
pub fn change(change: Change) -> worker::Change {
    let Change { title, body } = change;
    worker::Change { title, body }
}

/// The run's push for the worker's.
#[must_use]
pub fn push(pushed: worker::Pushed) -> run::Push {
    match pushed {
        worker::Pushed::Done => run::Push::Done,
        worker::Pushed::Moved => run::Push::Moved,
        worker::Pushed::Failed => run::Push::Failed,
    }
}

/// The worker's answer for the run's.
#[must_use]
pub fn answer(answer: &run::Answer) -> worker::Answer {
    match answer {
        run::Answer::Refused(run::Refusal::Busy) => worker::Answer::Busy,
        run::Answer::Refused(run::Refusal::Invalid(_)) => worker::Answer::Invalid,
        run::Answer::Accepted { outcome: _, spent } => worker::Answer::Done { usage: usage(*spent) },
        run::Answer::Failed { failure, spent } => {
            let reason = match failure {
                run::Failure::Model(_) => worker::Reason::Model,
                run::Failure::Budget(_) => worker::Reason::Budget,
                run::Failure::Policy(run::Policy::Unfinished { .. }) => worker::Reason::Unfinished,
                run::Failure::Cancelled => worker::Reason::Cancelled,
            };
            worker::Answer::Failed { reason, usage: usage(*spent) }
        }
    }
}

/// The run's repository for the worker's: where the worker put it is io's
/// business, which names it by `root`.
fn repository(repository: worker::Repository, root: Token) -> charter::Repository {
    let worker::Repository { name, path: _, writable } = repository;
    charter::Repository { name, root, writable }
}

fn verdict(verdict: worker::Verdict) -> VerdictRule {
    let worker::Verdict { name, min_children, max_children, kinds, fields } = verdict;
    VerdictRule { name, children: Children { min: min_children, max: max_children }, kinds, fields }
}

fn usage(spent: run::Spend) -> worker::Usage {
    worker::Usage {
        turns: spent.turns,
        input_tokens: spent.input,
        output_tokens: spent.output,
        cache_read_tokens: spent.cache_read,
        cache_write_tokens: spent.cache_write,
    }
}
