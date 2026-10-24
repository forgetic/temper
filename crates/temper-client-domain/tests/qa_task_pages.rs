//! QA1: adversarial task-page behaviour from design and public events.
#[cfg(test)]
#[expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::wildcard_enum_match_arm,
    reason = "QA is an ordinary Rust driver, not step code; selectors collect only the requests each assertion reads"
)]
mod tests {
    use jig_client::contract as c;
    use skein_lib::{Duration, Env, List, Queue, RequestLimits, Time, Token, Wall};
    use temper_client_domain::{self as domain, Domain, Event, Limits, Request, forge};
    use temper_client_protocol_wire as wire;
    const TASK: u64 = 9;
    const FORGE: u16 = wire::FORGE;
    #[expect(clippy::arithmetic_side_effects, reason = "the request count is the nonzero constant declared here")]
    fn domain_limits() -> Limits {
        {
            let requests = RequestLimits {
                requests: 4,
                bytes: 262_144,
                out: 16,
                first: Duration::from_millis(10),
                most: Duration::from_millis(80),
                margin: Duration::from_secs(1),
            };
            let client = jig_client::Limits {
                domain: jig_client::DomainLimits {
                    accounts: jig_client::AccountCounts { accounts: 16, sources: 8, sign_ins: 4 },
                    owners: &[jig_client::Spoken {
                        family: temper_client_protocol_wire::FORGE,
                        version: temper_client_protocol_wire::CLIENT_VERSION,
                    }],
                    requests,
                    ask_bytes: requests.bytes / u64::from(requests.requests),
                    handles: 9,
                    watches: 9,
                    turn_bytes: 65536,
                    backoff: jig_client::Backoff { first: Duration::from_millis(10), most: Duration::from_millis(80) },
                    authority: 12,
                    heartbeat: Duration::from_millis(100),
                    reads: 8,
                    parts: 8,
                    delegates: 2,
                    document_bytes: 65536,
                    records: 2,
                    kept: 4,
                    projects: 2,
                    text: 32,
                    turn_text: 32,
                    rows: 2,
                    fields: 4,
                    segments: 9,
                },
                exchanges: 8,
            };

            Limits {
                forge: forge::Limits { field_bytes: 256, checks: 8 },
                client,
                accounts: temper_client_domain::limits::AccountLimits {
                    accounts: 16,
                    sources: 8,
                    uses: 8,
                    sign_ins: 4,
                },
            }
        }
    }
    fn fixture_me(project_name: &[u8]) -> c::Me {
        use jig_client::contract::{self, DocumentText, HeldRole, Me, MeParty, MeProject, Needs, PartyKind};
        let mut projects = List::with_capacity(2);
        projects
            .push(MeProject {
                project: 1,
                name: DocumentText::complete(
                    contract::Text::new(skein_lib::bytes::copy_of(project_name), 32).expect("name fits"),
                ),
                role: HeldRole::Policy { role: 1 },
                pool: None,
                templates: List::with_capacity(0),
                creating: None,
                models: List::with_capacity(0),
            })
            .expect("one project");
        let mut second = projects.as_slice()[0].clone();
        second.project = 2;
        second.name = DocumentText::complete(contract::Text::new(Box::from(*b"Second"), 32).expect("name fits"));
        projects.push(second).expect("second project");
        Me {
            providers: List::with_capacity(0),
            providers_cut: false,
            key_bytes: 256,
            redirect_bytes: 256,
            party: MeParty {
                number: 73,
                kind: PartyKind::Person,
                login: DocumentText::complete(contract::Text::new(Box::from(*b"pat"), 32).expect("login fits")),
                name: DocumentText::complete(contract::Text::new(Box::from(*b"Pat"), 32).expect("name fits")),
            },
            projects,
            needs: Needs { questions: 0, proposals: 0, escalations: 0, person_tasks: 0 },
            needs_total: 0,
            updates: contract::Count { value: 0, limited: false },
            results_seen: 0,
            cut: false,
            retention: Duration::from_secs(1000),
            deployment_role: None,
        }
    }

    fn document() -> c::Task {
        let mut parts = jig_client_test_records::task_record().into_parts();
        parts.chip = jig_client_test_records::owner_waiting_chip_record();
        let record = jig_client_protocol_wire::v19::Task::new(&jig_client_protocol_wire::v19::CEILINGS, parts)
            .expect("checked task record");
        let limits = domain_limits();
        let link =
            jig_client_protocol_link::Limits::new(limits.client.domain, limits.client.exchanges).expect("link limits");
        let mut task = jig_client_protocol_link::translate::documents::task(record, &link).expect("translated task");
        task.chip.task = TASK;
        task.project = 1;
        task.chip.executor = c::Executor::Procedure { connector: FORGE, kind: 2 };
        task.priority = None;
        task.goal = None;
        task.chip.phase = c::Phase::Active;
        let c::ChipPhase::Owner { phase, details, .. } = task.chip.details.as_mut() else {
            unreachable!("owner fixture")
        };
        *phase = c::OwnerChipPhase::Active(c::RunStage::Idle);
        details.project = 1;
        details.executor = c::OwnerExecutor::Procedure { connector: FORGE, code: 2 };
        details.procedure = Some(c::OwnerPart {
            connector: FORGE,
            kind: c::OwnerPartKind::Procedure,
            key: c::OwnerPartKey::Number(TASK),
        });
        details.observation = None;
        details.ending = None;
        details.live_delegates = 0;
        task.acts = List::with_capacity(0);
        task.waits_on = c::WaitsOn::Nothing;
        task
    }
    fn root_event(root: &mut Domain, event: Event) -> Vec<jig_client::Down> {
        let limits = domain_limits();
        let mut out = Queue::with_capacity(domain::max_out(&limits));
        domain::step(root, &Env { now: Time::ZERO, wall: Wall::EPOCH, limits }, event, &mut out);
        let mut down = Vec::new();
        while let Some(request) = out.pop() {
            if let Request::Client { request: jig_client::Request::Down { down: value } } = request {
                down.push(value);
            }
        }
        down
    }
    fn up(root: &mut Domain, up: jig_client::Up) -> Vec<jig_client::Down> {
        root_event(root, Event::Client { event: jig_client::Event::Up { up } })
    }
    fn deliver(root: &mut Domain, watch: Token, value: c::Delivery) -> Vec<jig_client::Down> {
        up(root, jig_client::Up::Watched { watch, event: c::WatchEvent::Delivered(value) })
    }
    fn opened(document: c::Task) -> (Domain, Token, Token, Vec<jig_client::Down>) {
        opened_with_tree(document, true)
    }
    fn opened_with_tree(document: c::Task, supply_tree: bool) -> (Domain, Token, Token, Vec<jig_client::Down>) {
        let mut root = domain::unlinked(&domain_limits(), 11);
        let mut requests = root_event(
            &mut root,
            Event::Start {
                address: domain::Address::Task { task: TASK, filter: domain::PlanFilter::All },
                kept: List::with_capacity(0),
                with: None,
            },
        );
        // Successful sign-in ends the pre-sign-in read; its owner asks anew
        // before the first me (client/README.md, 3.4). Keep that output too.
        let before_sign_in = requests.len();
        for event in [
            jig_client::Event::StartUnlinked { kept: List::with_capacity(0) },
            jig_client::Event::SignIn { with: jig_client::With::Transport },
            jig_client::Event::Up { up: jig_client::Up::Served { served: true, versions: None } },
        ] {
            requests.extend(root_event(&mut root, Event::Client { event }));
        }
        let mut task_watch = None;
        let mut tree_watch = None;
        let mut me_watch = None;
        for request in &requests {
            if let jig_client::Down::Open { watch, subject: jig_client::Watchable::Jig(subject) } = request {
                match subject {
                    c::Subject::Me => me_watch = Some(*watch),
                    c::Subject::Task { task: TASK } => task_watch = Some(*watch),
                    c::Subject::Tree { .. } => tree_watch = Some(*watch),
                    _ => (),
                }
            }
        }
        let mut after = requests.split_off(before_sign_in);
        after.extend(deliver(&mut root, me_watch.expect("Me watch"), c::Delivery::Me(Box::new(fixture_me(b"Shop")))));
        let task_watch = task_watch.expect("task watch");
        after.extend(deliver(&mut root, task_watch, c::Delivery::Task(Box::new(document.clone()))));
        let tree_watch = tree_watch.expect("tree watch");
        if supply_tree {
            after.extend(deliver(&mut root, tree_watch, tree(&document)));
        }
        (root, task_watch, tree_watch, after)
    }
    fn tree(document: &c::Task) -> c::Delivery {
        c::Delivery::OwnerTree(Box::new(c::OwnerTree {
            root: document.chip.clone(),
            rows: List::with_capacity(0),
            folded: List::with_capacity(0),
            held_below: 0,
            next: None,
        }))
    }
    fn words(bytes: &[u8]) -> c::DocumentText {
        c::DocumentText::complete(c::Text::new(bytes.into(), 32).expect("fixture words fit"))
    }
    fn phase(task: &mut c::Task, value: c::OwnerChipPhase) {
        task.chip.phase = value.compact();
        let c::ChipPhase::Owner { phase, .. } = task.chip.details.as_mut() else { unreachable!("owner") };
        *phase = value;
    }
    fn details(task: &mut c::Task) -> &mut c::OwnerChipDetails {
        let c::ChipPhase::Owner { details, .. } = task.chip.details.as_mut() else { unreachable!("owner") };
        details
    }
    fn agent() -> c::Task {
        let mut task = document();
        phase(&mut task, c::OwnerChipPhase::Active(c::RunStage::Idle));
        task.chip.executor = c::Executor::Agent { charter: 1 };
        details(&mut task).executor = c::OwnerExecutor::Agent { charter: 1 };
        details(&mut task).procedure = None;
        details(&mut task).attempt = None;
        details(&mut task).run_since = None;
        task.chip.title = words(b"QA agent brief");
        task.spec.specification.words = words(b"QA agent specification");
        task.label = 0;
        task.lineage_chips = List::with_capacity(0);
        task.dependencies = List::with_capacity(0);
        task.dependency_facts = List::with_capacity(0);
        task.hold = None;
        task.result = None;
        task
    }
    fn goal() -> c::Task {
        let mut task = agent();
        task.priority = Some(1);
        task.goal = Some(TASK);
        details(&mut task).live_delegates = 17;
        task
    }
    fn finish(task: &mut c::Task, ending: c::OwnerChipEnding) {
        phase(task, c::OwnerChipPhase::Ended);
        details(task).ending = Some(ending.clone());
        details(task).ended_at = Some(Wall::EPOCH);
        task.result = Some(c::TaskResult::Ending(Box::new(ending)));
    }
    fn reads(requests: &[jig_client::Down]) -> Vec<(Token, c::Read)> {
        requests
            .iter()
            .filter_map(|request| match request {
                jig_client::Down::Read { read, what: jig_client::Readable::Jig(what) } => Some((*read, what.clone())),
                _ => None,
            })
            .collect()
    }
    fn activity_reads(requests: &[jig_client::Down]) -> Vec<Token> {
        reads(requests)
            .into_iter()
            .filter_map(|(read, what)| if let c::Read::Activity { .. } = what { Some(read) } else { None })
            .collect()
    }
    fn answer(root: &mut Domain, read: Token, value: c::ReadValue) -> Vec<jig_client::Down> {
        up(root, jig_client::Up::ReadDone { read, result: jig_client::Readout::Jig(c::ReadResult::Value(value)) })
    }
    fn empty_activity() -> c::ReadValue {
        c::ReadValue::Activity(Box::new(c::ActivityPage {
            rows: List::with_capacity(0),
            before: None,
            after: None,
            cut: false,
        }))
    }
    fn activity(position: u64) -> c::ReadValue {
        let mut rows = List::with_capacity(1);
        rows.push(c::ActivityLine {
            position,
            at: Wall::EPOCH,
            event: c::Activity::News {
                topic: 1,
                summary: c::Text::new(b"Activity at read watermark".as_slice().into(), 32).expect("news fits"),
                part: None,
            },
        })
        .expect("one activity");
        c::ReadValue::Activity(Box::new(c::ActivityPage { rows, before: None, after: None, cut: false }))
    }
    fn act(root: &mut Domain, act: domain::pages::Act) -> Vec<jig_client::Down> {
        root_event(root, Event::Act { action: domain::Action::Pages(act) })
    }

    fn went(root: &mut Domain, address: domain::Address) -> Vec<jig_client::Down> {
        root_event(root, Event::Went { place: domain::Place::Address(address) })
    }

    // client/README.md 5.8: "reads an ended goal's document once more when its watch ends";
    // jig's contract.md 7.2: an ended subject is read for its end.
    #[test]
    fn goal_watch_end_reads_and_keeps_the_final_document() {
        let (mut root, watch, _, _) = opened(goal());
        let requests = up(&mut root, jig_client::Up::WatchEnded { watch, end: c::WatchEnd::SubjectEnded });
        let terminal = reads(&requests).into_iter().find(|(_, what)| *what == c::Read::Task { task: TASK });
        let (read, _) = terminal.expect("watch ending requests final task");
        let mut task = goal();
        finish(&mut task, c::OwnerChipEnding::Done(c::ResultValue::Report(words(b"Final goal report"))));
        drop(answer(&mut root, read, c::ReadValue::Task(Box::new(task.clone()))));
        assert_eq!(
            root.pages().task().expect("task page").document(root.client()),
            Some(&task),
            "final document remains readable"
        );
    }

    // client/README.md 5.9: latest thread page "read again on each change of the document";
    // the chat's bounded thread holds one page read at a time.
    #[test]
    fn agent_document_changes_coalesce_the_latest_activity_read() {
        let (mut root, watch, _, requests) = opened(agent());
        let initial = activity_reads(&requests).into_iter().next().expect("initial latest activity read");
        let mut task = agent();
        task.activity = 42;
        task.changed = Wall::from_nanos(42);
        let changed = deliver(&mut root, watch, c::Delivery::Task(Box::new(task)));
        assert!(activity_reads(&changed).is_empty(), "one outstanding latest read: {changed:?}");
        let followup = answer(&mut root, initial, activity(1));
        let reads = activity_reads(&followup);
        assert_eq!(reads.len(), 1, "new document invalidation is read after the outstanding page");
        let read = reads.into_iter().next().expect("coalesced activity read");
        let settled = answer(&mut root, read, activity(42));
        assert!(activity_reads(&settled).is_empty(), "no uncaused third read");
    }

    // client/README.md 5.9: an agent's task is "no priority" and has a thread,
    // while ended delegate queries belong to goals and procedures (5.8, 5.10).
    #[test]
    fn untracked_agent_reads_activity_without_an_ended_children_query() {
        let (root, _, _, requests) = opened(agent());
        assert!(!activity_reads(&requests).is_empty(), "agent's latest thread is read");
        assert!(
            reads(&requests)
                .iter()
                .find_map(|(_, what)| if let c::Read::Tasks { query, .. } = what { Some(query) } else { None })
                .is_none(),
            "agent does not read ended children"
        );
        assert_eq!(root.pages().task().expect("agent").shape, domain::pages::task::Shape::Agent);
        assert!(root.pages().task().expect("agent").ended.is_none());
    }

    // A filter moves within the goal's page; reopening the unfiltered address shows All.
    #[test]
    fn goal_filter_moves_without_wire_work_and_the_plain_address_reopens_all() {
        let (mut root, watch, tree_watch, _) = opened(goal());
        let local = act(&mut root, domain::pages::Act::PlanFilter { filter: domain::address::PlanFilter::Live });
        assert!(local.is_empty(), "filter move emitted no wire work");
        let moved = went(&mut root, domain::Address::Task { task: TASK, filter: domain::PlanFilter::Live });
        assert!(moved.is_empty(), "same task keeps its reads");
        assert_eq!(root.pages().task().expect("goal").filter, domain::PlanFilter::Live);
        drop(went(&mut root, domain::Address::Chats { filter: domain::ChatsFilter::All, start: false }));
        let requests = went(&mut root, domain::Address::Task { task: TASK, filter: domain::PlanFilter::All });
        let page = root.pages().task().expect("reopened task");
        assert_eq!(page.filter, domain::address::PlanFilter::All);
        let new_watch = requests
            .iter()
            .find_map(|request| match request {
                jig_client::Down::Open {
                    watch,
                    subject: jig_client::Watchable::Jig(c::Subject::Task { task: TASK }),
                } => Some(*watch),
                _ => None,
            })
            .expect("new document watch");
        assert_ne!(new_watch, watch, "new open uses a fresh task watch");
        let late = deliver(&mut root, tree_watch, tree(&goal()));
        assert!(reads(&late).is_empty(), "old tree cannot invalidate new page");
    }

    // client/README.md 5.9: a live run has a run watch, no run out ends it as a chat's;
    // 3.5: "Leaving releases the page" including its run and conversation.
    #[test]
    fn leaving_agent_releases_its_run_and_drops_late_thread_answers() {
        let mut task = agent();
        phase(&mut task, c::OwnerChipPhase::Active(c::RunStage::Running { attempt: 2 }));
        details(&mut task).attempt = Some(2);
        let (mut root, _, _, requests) = opened(task);
        let activity = activity_reads(&requests).into_iter().next().expect("thread read");
        let run = requests
            .iter()
            .find_map(|request| match request {
                jig_client::Down::Open { watch, subject: jig_client::Watchable::Jig(c::Subject::Run { .. }) } => {
                    Some(*watch)
                }
                _ => None,
            })
            .expect("live run watch");
        drop(up(&mut root, jig_client::Up::Watched { watch: run, event: c::WatchEvent::Opened }));
        let left = went(&mut root, domain::Address::Chats { filter: domain::ChatsFilter::All, start: false });
        assert!(left.contains(&jig_client::Down::Close { watch: run }), "navigation closes run: {left:?}");
        drop(answer(&mut root, activity, empty_activity()));
        assert!(root.pages().task().is_none(), "late activity cannot resurrect page");
    }

    // client/README.md 5.9: an agent's task has "no priority" and its thread:
    // 5.11's untracking changes the document to an agent's page, which reads it.
    #[test]
    fn untracking_opens_the_agents_thread() {
        let (mut root, watch, _, requests) = opened(goal());
        // Complete the goal's initial page before changing shape. A pending
        // latest read coalesces changes, as the agent test above checks.
        let initial = activity_reads(&requests).into_iter().next().expect("goal's initial activity read");
        drop(answer(&mut root, initial, empty_activity()));
        let requests = deliver(&mut root, watch, c::Delivery::Task(Box::new(agent())));
        let task = root.pages().task().expect("task page");
        assert_eq!(task.shape, domain::pages::task::Shape::Agent);
        assert_eq!(activity_reads(&requests).len(), 1, "agent opens its own thread");
    }

    // client/README.md 5.8's ended-delegate page belongs to a goal's shape; the
    // README does not say whether 5.11's untracking releases it.
    #[test]
    #[ignore = "backlog: an untracked goal's page keeps its goal-only ended page (Task.ended stays Some under Shape::Agent); nothing draws it, and README 5.8/5.11 do not say it is released"]
    fn untracking_releases_the_goals_ended_page() {
        let (mut root, watch, _, _) = opened(goal());
        drop(deliver(&mut root, watch, c::Delivery::Task(Box::new(agent()))));
        let task = root.pages().task().expect("task page");
        assert_eq!(task.shape, domain::pages::task::Shape::Agent);
        assert!(task.ended.is_none(), "untracked agent no longer owns a goal's ended page");
    }
}
