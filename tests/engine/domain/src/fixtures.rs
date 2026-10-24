//! The forge-backed direct driver's limits and adoption form values.
use crate::direct::Driver;
use jig_domain_tasks as tasks;
use skein_lib::Token;
use temper_engine_domain as engine;
use temper_engine_domain_forge as forge;

#[must_use]
pub fn forge_limits() -> engine::Limits {
    let mut limits = crate::walking::limits();
    limits.people.people = 4;
    limits.people.holdings = 4;
    limits.journal.writes +=
        (jig_domain_people::max_out(&limits.people) - jig_domain_people::max_out(&crate::walking::limits().people)) * 4;
    limits
}

#[must_use]
pub fn adoption(forge: u16) -> forge::Adoption {
    forge::Adoption {
        project: 1,
        home: true,
        forge,
        host: Box::from(&b"forge.example"[..]),
        owner: Box::from(&b"org"[..]),
        name: Box::from(&b"repo"[..]),
        prefix: Box::from(&b"temper/"[..]),
        role: forge::Role::Owned,
        landing: Box::from(&b"main"[..]),
        ci: true,
        checks: Box::new([]),
    }
}

#[must_use]
pub fn report_delegate(words: &[u8], dependencies: Box<[engine::Dependency]>) -> engine::Delegate {
    engine::Delegate {
        tracked: None,
        label: 0,
        flags: tasks::Flags::PLAIN,
        budget: jig_domain_people::Budget::Named,
        executor: tasks::Executor::Agent { charter: 1 },
        spec: tasks::Spec { words: words.into(), parameters: Box::new([]), inputs: Box::new([]) },
        contract: tasks::Contract::Report { words: 128 },
        authority: tasks::Authority {
            tools: tasks::Tools(1023),
            models: tasks::Models::NONE,
            grants: Box::new([]),
            delegation: tasks::Delegation { kinds: Box::new([]), tasks: 0, depth: 0 },
            budget: tasks::Budget { spend: 10, deadline: None },
            notes: tasks::Scopes(0),
            note_resources: Box::new([]),
        },
        llm: None,
        symbolic_grants: Box::new([]),
        dependencies,
        wake: tasks::WakePolicy::DEFAULT,
    }
}

pub fn tool_call(driver: &mut Driver, source: &engine::Assignment, call: u64, tool: engine::Tool) {
    let body = engine::Call { completion: 1, position: u32::try_from(call).expect("small call"), tool };
    driver.call(Token::new(7), source.task, source.attempt, call, body);
    for _ in 0..60 {
        driver.advance(true);
    }
}

#[must_use]
pub fn task_amendment(words: Option<&[u8]>, authority: Option<tasks::Authority>, reason: &[u8]) -> tasks::Amendment {
    tasks::Amendment {
        tracked: None,
        addressed: None,
        instructions: None,
        release: None,
        spec: words.map(|words| tasks::Spec { words: words.into(), parameters: Box::new([]), inputs: Box::new([]) }),
        wake: None,
        dependencies: None,
        authority,
        llm: None,
        reason: reason.into(),
    }
}

/// An authenticated chat with authority left for one direct delegate.
#[must_use]
pub fn revision_delegate() -> (Driver, engine::Assignment, u64) {
    use crate::walking::{config, limits};
    let mut bounds = limits();
    bounds.call_records = 8;
    bounds.journal.writes = 6000;
    bounds.journal.held = 300;
    bounds.journal.deliveries = 100;
    bounds.tasks.depth = 2;
    bounds.brief.sections = 5;
    let configuration = std::array::from_fn(|_| {
        let mut configuration = config(92);
        let mut rules = configuration.core.defaults.rules.clone();
        rules.ceiling.delegation.depth = 2;
        let mut policy = configuration.core.seed_projects[0].policy.policy.clone();
        policy.ceiling.delegation.depth = 2;
        policy.roles[0].authority.delegation.depth = 2;
        let authority = jig_domain_authority::Domain::new(rules, bounds.authority).expect("fixture authority");
        configuration.core.seed_projects[0].policy.policy = policy;
        configuration.core.defaults.rules = authority.rules().clone();
        configuration
    });
    let (mut driver, parent) = Driver::configured_chat(configuration, &bounds);
    let mut member = report_delegate(b"first", Box::new([]));
    member.authority.budget.spend = 100;
    tool_call(&mut driver, &parent, 60, engine::Tool::Delegate { batch: Box::new([member]) });
    let Some(engine::CallAnswer::Delegated(children)) = driver.answered(60) else {
        panic!("one delegate admitted: {:?}", driver.answered(60));
    };
    let child = children[0].task;
    (driver, parent, child)
}

/// jig's two default templates, the walking chat ended by its result.
#[must_use]
pub fn templates() -> Box<[jig_domain_core::Template]> {
    templates_ending(jig_domain_tasks::EndedBy::Result)
}

/// jig's two default templates, the chat ended as `chat` says: by its
/// requester, as temper's is configured (domain/agent.md, 4.6). The goal is
/// ended by its result either way.
#[must_use]
pub fn templates_ending(chat: jig_domain_tasks::EndedBy) -> Box<[jig_domain_core::Template]> {
    use jig_domain_people::{TemplateBudget, TemplateDeadline, TemplateExecutor};
    use jig_domain_tasks::{Contract, Flags, Retention};
    Box::new([
        jig_domain_core::Template {
            number: 1,
            name: Box::from(&b"chat"[..]),
            executor: TemplateExecutor::Charter(1),
            contract: Contract::Report { words: 32 },
            shallower: 1,
            budget: TemplateBudget::AsAllowed { cap: None },
            deadline: TemplateDeadline::None,
            tracked: None,
            flags: Flags { ended_by: chat, conversation: true, listed: false, retention: Retention::Kept },
            label: 1,
            grace: Some(skein_lib::Duration::from_millis(5000)),
            llm: None,
        },
        jig_domain_core::Template {
            number: 2,
            name: Box::from(&b"goal"[..]),
            executor: TemplateExecutor::Chosen,
            contract: Contract::Report { words: 32 },
            shallower: 1,
            budget: TemplateBudget::Given,
            deadline: TemplateDeadline::Given,
            tracked: Some(1),
            flags: Flags {
                ended_by: jig_domain_tasks::EndedBy::Result,
                conversation: false,
                listed: true,
                retention: Retention::Kept,
            },
            label: 2,
            grace: None,
            llm: None,
        },
    ])
}

#[must_use]
pub fn account_rows(providers: &jig_domain_accounts::Providers) -> Box<[engine::Record]> {
    use jig_domain_accounts::{Holder, Kind, Stored};
    let mut rows = Vec::new();
    for (index, entry) in providers.providers.iter().filter(|p| !p.offers.none()).enumerate() {
        let index = u32::try_from(index).expect("bounded fixture value");
        let (account, source) = if index == 0 { (0, 1) } else { (2 * index + 1, 2 * index) };
        let provider = entry.number;
        {
            for row in [
                Stored::Account {
                    number: account,
                    provider,
                    holder: Holder::Deployment,
                    place: index,
                    uses: None,
                    subject: Box::new([]),
                    display: Box::new([]),
                },
                Stored::Source { account, number: source, place: 0, kind: Kind::Key, name: Box::new([]), held: None },
            ] {
                rows.push(engine::Record::Core(jig_domain_core::Record::Accounts(row)));
            }
        }
    }
    if !rows.is_empty() {
        rows.push(engine::Record::Core(jig_domain_core::Record::Accounts(Stored::Next {
            number: u32::try_from(rows.len()).expect("bounded fixture value"),
        })));
    }
    rows.into_boxed_slice()
}

/// Keep temper's actual first-start transaction, then account rows at receipt two.
/// Callers construct a second identical configuration for their cold root; no domain
/// configuration is cloned by hand and no account/session request precedes the story.
#[must_use]
pub fn restored_store(config: engine::Config, limits: &engine::Limits) -> crate::store::Store {
    restored_over(crate::store::Store::new(), config, limits)
}

/// Same bootstrap over the caller's chosen real store codec bounds.
#[must_use]
pub fn restored_over(
    store: crate::store::Store,
    config: engine::Config,
    limits: &engine::Limits,
) -> crate::store::Store {
    let accounts = account_rows(&config.core.settings.llm_providers);
    let mut store = first_start_over(store, config, limits);
    store.seed(accounts, Box::new([]));
    store
}

/// Real first-start rows alone, also used by conformance's independent receipt observer.
#[must_use]
pub fn first_start_over(
    store: crate::store::Store,
    config: engine::Config,
    limits: &engine::Limits,
) -> crate::store::Store {
    let mut driver = Driver::cold(store, config, limits);
    driver.period_sent = true;
    driver.settle();
    assert_eq!(driver.store.applied, 1, "temper's real first start is receipt one");
    driver.store
}
