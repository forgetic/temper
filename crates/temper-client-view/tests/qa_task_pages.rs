//! QA1: adversarial task-page behaviour from design and public events.
#[cfg(test)]
#[expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::disallowed_macros,
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
            match request {
                Request::Client { request: jig_client::Request::Down { down: value } } => down.push(value),
                Request::Show { address, .. } => {
                    down.extend(root_event(root, Event::Went { place: domain::Place::Address(address) }));
                }
                _ => {}
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
        let mut after = deliver(&mut root, me_watch.expect("Me watch"), c::Delivery::Me(Box::new(fixture_me(b"Shop"))));
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
    fn ended_reads(requests: &[jig_client::Down]) -> Vec<Token> {
        requests
            .iter()
            .filter_map(|request| match request {
                jig_client::Down::Read { read, what: jig_client::Readable::Jig(c::Read::Tasks { .. }) } => Some(*read),
                _ => None,
            })
            .collect()
    }
    fn ended_answer(root: &mut Domain, read: Token, page: c::TaskPage) -> Vec<jig_client::Down> {
        up(
            root,
            jig_client::Up::ReadDone {
                read,
                result: jig_client::Readout::Jig(c::ReadResult::Value(c::ReadValue::Tasks(Box::new(page)))),
            },
        )
    }
    fn empty_page() -> c::TaskPage {
        c::TaskPage {
            live: List::with_capacity(0),
            live_after: None,
            ended: c::EndedGroup::Rows { rows: List::with_capacity(0), after: None, cut: false },
        }
    }
    fn answer_all_initial_reads(root: &mut Domain, requests: &[jig_client::Down], page: &c::TaskPage) {
        let reads = ended_reads(requests);
        assert_eq!(reads.len(), 1, "one initial read");
        let requests = ended_answer(root, *reads.first().expect("initial read"), page.clone());
        let reads = ended_reads(&requests);
        assert!(reads.len() <= 1, "one coalesced refresh");
        if let Some(read) = reads.first() {
            let requests = ended_answer(root, *read, page.clone());
            assert!(ended_reads(&requests).is_empty(), "no older page is read");
        }
    }
    fn ended_line(number: u64, parent: u64) -> c::TaskLine {
        let mut task = document();
        end(&mut task);
        task.chip.task = number;
        task.chip.title = c::DocumentText::complete(
            c::Text::new(format!("ended child {number}").into_bytes().into(), 32).expect("title"),
        );
        task.chip.executor = c::Executor::Agent { charter: 1 };
        let c::ChipPhase::Owner { details, .. } = task.chip.details.as_mut() else { unreachable!("owner") };
        details.executor = c::OwnerExecutor::Agent { charter: 1 };
        details.procedure = None;
        c::TaskLine {
            chip: task.chip,
            project: 1,
            requester: c::OwnerRequester::Task(parent),
            tracked: None,
            label: task.label,
            flags: task.flags,
            changed: Wall::EPOCH,
            last_activity: Wall::EPOCH,
            ended_at: Some(Wall::EPOCH),
            budget: task.budget,
            waits_on: c::WaitsOn::Nothing,
            phases: c::OwnerPhaseCounts::ZERO,
            held_below: 0,
            conversation: None,
        }
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
    fn end(task: &mut c::Task) {
        finish(task, c::OwnerChipEnding::Failed(words(b"ended")));
    }
    fn finish(task: &mut c::Task, ending: c::OwnerChipEnding) {
        phase(task, c::OwnerChipPhase::Ended);
        details(task).ending = Some(ending.clone());
        details(task).ended_at = Some(Wall::EPOCH);
        task.result = Some(c::TaskResult::Ending(Box::new(ending)));
    }
    fn view(root: &Domain) -> temper_client_view::View {
        let words = temper_client_view::words::Words::english(FORGE, 64, temper_client_view::words::Shell::Browser)
            .expect("English");
        // The allowed fixture envelope, never raised when a case exhausts it.
        let tree = skein_ui::Limits {
            nodes: 192,
            depth: 32,
            text_per_node: 512,
            text_bytes: 98304,
            drawing_bytes: 0,
            scene_marks: 0,
            scene_bytes: 0,
            styles_per_node: 2,
            patches: 960,
            bindings: 128,
            slots: 32,
            markdown_depth: 3,
        };
        let limits = temper_client_view::Limits { tree };
        let mut view = temper_client_view::View::new(&limits, &domain_limits(), words, FORGE);
        temper_client_view::build(&mut view, root, Wall::EPOCH, &mut Queue::with_capacity(tree.patches));
        view
    }
    fn text(nodes: &[skein_ui::Node]) -> String {
        let mut bytes = Vec::new();
        for node in nodes {
            for run in &node.text {
                bytes.extend_from_slice(&run.text);
            }
        }
        String::from_utf8(bytes).expect("visible UTF-8")
    }
    fn find(view: &temper_client_view::View, role: skein_ui::Role, name: &[u8]) -> skein_ui::NodeId {
        use skein_fake_person::{Face, Find, Found, TreeFace};
        let Found::Node(node) = TreeFace::new(view.tree()).find(&Find::exactly_named(role, name)) else {
            panic!("missing {:?}: {}", String::from_utf8_lossy(name), text(view.tree().nodes()));
        };
        node
    }
    fn press(root: &mut Domain, label: &[u8]) -> Vec<jig_client::Down> {
        let mut view = view(root);
        let node = find(&view, skein_ui::Role::Button, label);
        let action = temper_client_view::act(&mut view, node, skein_ui::Act::Press).expect("button's act");
        root_event(root, Event::Act { action })
    }
    fn page(rows: &[u64], cut: bool) -> c::TaskPage {
        let mut page = empty_page();
        let mut ended = List::with_capacity(domain_limits().client.domain.rows);
        for number in rows {
            ended.push(ended_line(*number, TASK)).expect("ended rows fit fixture");
        }
        page.ended = c::EndedGroup::Rows { rows: ended, after: None, cut };
        page
    }
    fn plan_links(root: &Domain) -> Vec<String> {
        let view = view(root);
        let nodes = view.tree().nodes();
        let index = nodes.iter().position(|node| node.role == skein_ui::Role::Tree).expect("plan tree");
        let end = index.checked_add(usize::try_from(nodes[index].size).expect("subtree size")).expect("subtree end");
        nodes[index..end]
            .iter()
            .filter(|node| node.role == skein_ui::Role::Link)
            .filter_map(|node| node.name.as_ref().map(|name| String::from_utf8_lossy(name).into_owned()))
            .collect()
    }

    // client/README.md 5.8: "filter is drawn from the page's start, before the read answers";
    // "two buttons, the chosen one pressed"; they "filter ... reading nothing".
    #[test]
    fn goal_filter_is_operable_before_the_first_ended_answer() {
        let (mut root, _, _, _) = opened(goal());
        let requests = press(&mut root, b"Live only");
        assert!(requests.is_empty(), "filter is local: {requests:?}");
        let view = view(&root);
        let node = find(&view, skein_ui::Role::Button, b"Live only");
        assert!(
            view.tree()
                .nodes()
                .iter()
                .find(|n| n.id == node)
                .expect("button")
                .states
                .as_slice()
                .contains(&skein_ui::State::Pressed)
        );
        assert!(text(view.tree().nodes()).contains("No live tasks."));
    }

    // client/README.md 5.8: "read goes on under either" filter; counts use
    // "N the goal's chip's live_delegates" and "more than M ended" when cut.
    #[test]
    fn live_filter_keeps_counts_and_receives_refreshed_ended_rows() {
        let (mut root, _, tree_watch, requests) = opened(goal());
        answer_all_initial_reads(&mut root, &requests, &page(&[21], true));
        drop(press(&mut root, b"Live only"));
        let refresh = deliver(&mut root, tree_watch, tree(&goal()));
        let read = ended_reads(&refresh).into_iter().next().expect("refresh read");
        drop(ended_answer(&mut root, read, page(&[22, 23], false)));
        let drawing = text(view(&root).tree().nodes());
        assert!(drawing.contains("Its delegates: 17 live, 2 ended."), "supplied counts: {drawing}");
        assert!(plan_links(&root).is_empty(), "ended rows hidden under Live only");
        assert!(!drawing.contains("More of its ended tasks are not shown."), "more line hidden");
        drop(press(&mut root, b"All"));
        assert_eq!(plan_links(&root), ["Task 22", "Task 23"], "All uses the answer received while filtered");
    }

    // client/README.md 5.8: "last page answered stays drawn until the next one answers";
    // the cut page says "more than M ended" and the filter "does not change" counts.
    #[test]
    fn goal_refresh_keeps_old_rows_and_cut_count_until_answered() {
        let (mut root, _, tree_watch, requests) = opened(goal());
        answer_all_initial_reads(&mut root, &requests, &page(&[21], true));
        drop(deliver(&mut root, tree_watch, tree(&goal())));
        let drawing = text(view(&root).tree().nodes());
        assert_eq!(plan_links(&root), ["Task 21"]);
        assert!(drawing.contains("Its delegates: 17 live, more than 1 ended."), "cut count: {drawing}");
        assert!(drawing.contains("More of its ended tasks are not shown."));
    }

    // client/README.md 5.8: a delegate in both sources "is drawn once, as the tree's row".
    #[test]
    fn overlapping_ended_and_live_rows_use_the_tree_row_once() {
        let task = goal();
        let (mut root, _, tree_watch, requests) = opened(task.clone());
        let mut child = agent();
        child.chip.task = 21;
        child.chip.title = words(b"Tree row takes precedence");
        let mut rows = List::with_capacity(1);
        rows.push(c::OwnerTreeRow {
            chip: child.chip,
            parent: TASK,
            depth: 1,
            proposals: List::with_capacity(0),
            spent: 999,
            dependencies: List::with_capacity(0),
            cut: false,
        })
        .expect("row");
        drop(deliver(
            &mut root,
            tree_watch,
            c::Delivery::OwnerTree(Box::new(c::OwnerTree {
                root: task.chip,
                rows,
                folded: List::with_capacity(0),
                held_below: 0,
                next: None,
            })),
        ));
        answer_all_initial_reads(&mut root, &requests, &page(&[21], false));
        assert_eq!(plan_links(&root), ["Task 21"]);
        let drawing = text(view(&root).tree().nodes());
        assert!(drawing.contains("Tree row takes precedence"), "tree supplies content: {drawing}");
        assert!(!drawing.contains("ended child 21"), "page duplicate omitted: {drawing}");
    }

    // client/README.md 5.9: with no lineage, "parent is its requester's Task N link
    // when a task asked it"; otherwise no parent.
    #[test]
    fn agent_lineage_falls_back_only_to_a_task_requester() {
        for (requester, parent) in [
            (c::OwnerRequester::Task(12), Some("Task 12")),
            (c::OwnerRequester::Person(73), None),
            (c::OwnerRequester::Deployment { project: 1 }, None),
        ] {
            let mut task = agent();
            task.requester = requester;
            let (root, _, _, _) = opened(task);
            let mut view = view(&root);
            let nodes = view.tree().nodes();
            let index = nodes
                .iter()
                .position(|n| n.styles.as_slice().contains(&temper_client_view::look::AGENT_LINEAGE))
                .expect("lineage bar");
            let end = index.checked_add(usize::try_from(nodes[index].size).expect("size")).expect("end");
            let links: Vec<_> = nodes[index..end].iter().filter(|n| n.role == skein_ui::Role::Link).collect();
            assert_eq!(links.len(), usize::from(parent.is_some()), "{requester:?}");
            if let Some(parent) = parent {
                let node = find(&view, skein_ui::Role::Link, parent.as_bytes());
                assert_eq!(temper_client_view::href(&mut view, node).as_deref(), Some(b"/tasks/12".as_slice()));
            }
        }
    }

    // client/README.md 5.9: claimed, running and closing run are "live now";
    // a live task with no run out says "waiting", and run_since is only shown there.
    #[test]
    fn agent_run_headings_follow_phase_even_without_observed_work() {
        let since = Wall::from_nanos(1_792_640_880_000_000_000);
        for (value, expected) in [
            (c::OwnerChipPhase::Active(c::RunStage::Claimed { attempt: 2 }), "Run 2 · live now"),
            (c::OwnerChipPhase::Active(c::RunStage::Running { attempt: 2 }), "Run 2 · live now"),
            (c::OwnerChipPhase::Closing(c::ClosingStage::Run { attempt: 2 }), "Run 2 · live now"),
            (c::OwnerChipPhase::Closing(c::ClosingStage::Delegates), "Run 2 · waiting"),
            (c::OwnerChipPhase::Active(c::RunStage::BackingOff { until: since }), "Run 2 · waiting"),
        ] {
            let mut task = agent();
            phase(&mut task, value.clone());
            details(&mut task).attempt = Some(2);
            details(&mut task).run_since = Some(since);
            let (root, _, _, _) = opened(task);
            let drawing = text(view(&root).tree().nodes());
            assert!(drawing.contains(expected), "{value:?}: {drawing}");
            assert!(drawing.contains("Started 10-22 03:48Z"), "run placement: {drawing}");
            assert!(!drawing.contains("Result"), "live task has no result: {drawing}");
        }
    }

    // client/README.md 5.9: "no heading once it has ended without [a run]";
    // result uses "Completed", "Failed", "Cancelled" and the result's words.
    #[test]
    fn never_run_agent_endings_show_result_without_a_run_heading() {
        for (ending, status, said) in [
            (
                c::OwnerChipEnding::Done(c::ResultValue::Report(words(b"Complete report"))),
                "Completed",
                "Complete report",
            ),
            (c::OwnerChipEnding::Failed(words(b"Failure reason")), "Failed", "Failure reason"),
            (
                c::OwnerChipEnding::Cancelled {
                    reason: words(b"Cancellation reason"),
                    result: None,
                    by: c::OwnerRequester::Person(73),
                    at: Wall::EPOCH,
                },
                "Cancelled",
                "Cancellation reason",
            ),
        ] {
            let mut task = agent();
            finish(&mut task, ending);
            let (root, _, _, _) = opened(task);
            let view = view(&root);
            find(&view, skein_ui::Role::Region, b"Result");
            let drawing = text(view.tree().nodes());
            assert!(drawing.contains(status) && drawing.contains(said), "{status}: {drawing}");
            assert!(!drawing.contains("No run now") && !drawing.contains("Run 0"), "no invented run: {drawing}");
        }
    }

    fn region_text(root: &Domain, name: &[u8]) -> String {
        let view = view(root);
        let id = find(&view, skein_ui::Role::Region, name);
        let nodes = view.tree().nodes();
        let index = nodes.iter().position(|n| n.id == id).expect("region");
        let end = index.checked_add(usize::try_from(nodes[index].size).expect("size")).expect("end");
        text(&nodes[index..end])
    }

    // client/README.md 5.8: budget reads "S spent · L left of B" as supplied,
    // with "Delegates spent" and "Reserved for delegates" kept separate;
    // "it computes no sum, share or difference".
    #[test]
    fn goal_budget_uses_supplied_amounts_without_recomputing_left() {
        let mut task = goal();
        task.budget.numbers = c::OwnerNumbers { budget: 100, spent: 7, spent_below: 31, reserved: 19 };
        task.budget.left = 43;
        task.budget.funder = c::Funder::Pool { project: 1, person: 73, period: 1 };
        task.budget.deadline = None;
        let (root, _, _, _) = opened(task);
        let drawing = region_text(&root, b"Budget and authority");
        assert!(drawing.contains("7 spent · 43 left of 100"), "budget's own left: {drawing}");
        assert!(drawing.contains("Delegates spent31"), "ended spend separate: {drawing}");
        assert!(drawing.contains("Reserved for delegates19"), "reservation separate: {drawing}");
        assert!(drawing.contains("Your pool") && drawing.contains("DeadlineNone"), "funder and no deadline: {drawing}");
    }

    // client/README.md 5.8: dependency waits use "N their supplied count";
    // party waits say "you" only when the person is that party or holds that role.
    #[test]
    fn goal_header_uses_exact_wait_count_and_scoped_role_identity() {
        let mut pending = List::with_capacity(2);
        pending.push(21).expect("dependency");
        pending.push(22).expect("dependency");
        for (project, wait, expected) in [
            (1, c::WaitsOn::Dependencies { pending, count: 41 }, "waiting on 41 tasks"),
            (
                1,
                c::WaitsOn::Party { whom: c::Whom::Person(73), entries: 1, task: TASK, kind: c::WaitedFor::Question },
                "waiting on an answer from you",
            ),
            (
                2,
                c::WaitsOn::Party {
                    whom: c::Whom::Role { project: 2, role: 1 },
                    entries: 1,
                    task: TASK,
                    kind: c::WaitedFor::Question,
                },
                "waiting on an answer from you",
            ),
            (
                3,
                c::WaitsOn::Party {
                    whom: c::Whom::Role { project: 3, role: 1 },
                    entries: 1,
                    task: TASK,
                    kind: c::WaitedFor::Question,
                },
                "waiting on an answer from role 1",
            ),
        ] {
            let mut task = goal();
            task.project = project;
            details(&mut task).project = project;
            task.waits_on = wait.clone();
            let (root, _, _, _) = opened(task);
            let view = view(&root);
            find(&view, skein_ui::Role::Status, expected.as_bytes());
        }
    }

    // client/README.md 5.9: brief dependencies are "in order" with added marks
    // from "dependency_facts"; document cut "does not say which ... was cut",
    // so the line has no invented last item (5.8 marks match by number).
    #[test]
    fn agent_dependency_marks_match_numbers_and_do_not_invent_cut_items() {
        let mut task = agent();
        task.cut = true;
        task.dependencies = List::with_capacity(2);
        task.dependencies
            .push(c::OwnerTaskLink::Stored { number: 21, status: Some(c::OwnerTaskStatus::Done) })
            .expect("dependency");
        task.dependencies.push(c::OwnerTaskLink::Stored { number: 22, status: None }).expect("dependency");
        task.dependency_facts = List::with_capacity(2);
        task.dependency_facts
            .push(c::Dependency {
                number: 22,
                pending: true,
                added: Some(c::DependencyAdded { at: Wall::EPOCH, by: c::OwnerRequester::Person(73) }),
                keeps: true,
                status: None,
            })
            .expect("facts");
        task.dependency_facts
            .push(c::Dependency {
                number: 21,
                pending: false,
                added: Some(c::DependencyAdded { at: Wall::EPOCH, by: c::OwnerRequester::Task(12) }),
                keeps: false,
                status: Some(c::OwnerTaskStatus::Done),
            })
            .expect("facts");
        let (root, _, _, _) = opened(task);
        let drawing = region_text(&root, b"Task brief");
        let first = drawing.find("Task 21").expect("first dependency");
        let second = drawing.find("Task 22").expect("second dependency");
        assert!(first < second, "dependency order: {drawing}");
        assert!(drawing.contains("Task 21CompletedAdded by task 12"), "facts matched by number: {drawing}");
        assert!(drawing.contains("Task 22Added by you · keeps task 9 from ending"), "viewer and keeps: {drawing}");
        assert!(
            !drawing.contains("More dependencies not shown."),
            "aggregate cut identifies no lost dependency: {drawing}"
        );
    }

    // client/README.md 5.11: "An act the document does not offer is not drawn,
    // not even disabled"; offered Message alone controls the agent's composer.
    #[test]
    fn agent_composer_disappears_when_the_offer_is_removed() {
        let mut task = agent();
        task.acts = List::with_capacity(1);
        task.acts
            .push(c::Offered { act: c::ActKind::Message, admission: c::Admission::Allowed, effect: None })
            .expect("message offered");
        let (mut root, watch, _, _) = opened(task.clone());
        // Keep typed steering words while a new whole document removes Message.
        drop(root_event(
            &mut root,
            Event::Act {
                action: domain::Action::Pages(domain::pages::Act::MessageWords {
                    words: b"A private draft".as_slice().into(),
                }),
            },
        ));
        task.acts = List::with_capacity(0);
        drop(deliver(&mut root, watch, c::Delivery::Task(Box::new(task))));
        let view = view(&root);
        assert!(
            !view
                .tree()
                .nodes()
                .iter()
                .any(|node| (node.role == skein_ui::Role::TextArea || node.role == skein_ui::Role::Button)
                    && (node.name.as_deref() == Some(b"Message") || node.name.as_deref() == Some(b"Send"))),
            "removed Message hides controls: {}",
            text(view.tree().nodes())
        );
    }

    // client/README.md 5.11: "While ... confirmation is open ... draw no act";
    // the dialog's Cancel is "the only one on the page".
    #[test]
    fn goal_stop_confirmation_hides_other_toolbar_acts() {
        let mut task = goal();
        task.acts = List::with_capacity(3);
        for act in [c::ActKind::Amend, c::ActKind::Stop, c::ActKind::Cancel] {
            task.acts.push(c::Offered { act, admission: c::Admission::Allowed, effect: None }).expect("toolbar offers");
        }
        let (mut root, _, _, _) = opened(task);
        drop(press(&mut root, b"Stop"));
        let view = view(&root);
        find(&view, skein_ui::Role::Dialog, b"Stop");
        let buttons: Vec<_> = view
            .tree()
            .nodes()
            .iter()
            .filter(|n| n.role == skein_ui::Role::Button)
            .filter_map(|n| n.name.as_deref())
            .collect();
        assert_eq!(buttons.iter().filter(|n| **n == b"Cancel").count(), 1, "one dialog Cancel");
        assert!(!buttons.iter().any(|n| *n == b"Amend" || *n == b"Stop"), "toolbar hidden while confirming");
    }

    // client/README.md 5.8: Live only keeps "the live rows, their folds and held
    // cards"; "No live tasks" requires "no live row, fold or held count".
    #[test]
    fn live_filter_does_not_call_a_folded_only_plan_empty() {
        let task = goal();
        let (mut root, _, tree_watch, requests) = opened(task.clone());
        let mut folded = List::with_capacity(1);
        folded
            .push(c::OwnerTreeFold {
                parent: TASK,
                phases: c::OwnerPhaseCounts { running: 17, ..c::OwnerPhaseCounts::ZERO },
                held_below: 0,
            })
            .expect("one fold");
        drop(deliver(
            &mut root,
            tree_watch,
            c::Delivery::OwnerTree(Box::new(c::OwnerTree {
                root: task.chip,
                rows: List::with_capacity(0),
                folded,
                held_below: 0,
                next: None,
            })),
        ));
        answer_all_initial_reads(&mut root, &requests, &page(&[21], false));
        drop(press(&mut root, b"Live only"));
        let drawing = region_text(&root, b"Plan");
        assert!(!drawing.contains("No live tasks."), "folds attest hidden live work: {drawing}");
        assert!(drawing.contains("17"), "supplied fold count remains visible: {drawing}");
        assert!(plan_links(&root).is_empty(), "ended row remains filtered");
    }

    // client/README.md 5.9: Agent is "an agent's executor, no priority, not the
    // chat label"; 5.11 allows untracking a goal. Its resulting page has a brief
    // and conversation, with no inherited ended-delegate query (5.8).
    #[test]
    fn untracking_a_goal_switches_to_the_agent_page() {
        let (mut root, watch, _, requests) = opened(goal());
        answer_all_initial_reads(&mut root, &requests, &page(&[21], false));
        drop(deliver(&mut root, watch, c::Delivery::Task(Box::new(agent()))));
        let view = view(&root);
        find(&view, skein_ui::Role::Region, b"Task brief");
        let drawing = text(view.tree().nodes());
        assert!(!drawing.contains("ended child 21"), "goal's ended rows leave with its tracked shape: {drawing}");
        assert!(!drawing.contains("Its delegates:"), "agent has no goal plan counts: {drawing}");
    }
}
