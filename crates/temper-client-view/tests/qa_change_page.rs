//! QA1: design-driven change-page observations through public events only.
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
    use temper_client_protocol_wire::{self as wire, v1 as w};
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
    // A signed-in client also watches the deployment's forges (client/forge.md); the
    // change panel's watch is the family open whose record is a change watch.
    fn change_open(request: &jig_client::Down) -> Option<Token> {
        let jig_client::Down::Open { watch, subject: jig_client::Watchable::Family { record, .. } } = request else {
            return None;
        };
        let watching = wire::decode_watching(&record.bytes, &w::CEILINGS)?;
        let w::Watch::Change(_) = watching.watch() else { return None };
        Some(*watch)
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
    fn used(used: u32, limit: u32) -> w::Used {
        w::Used::new(&w::CEILINGS, w::UsedParts { used, limit }).expect("wire numbers")
    }
    fn panel_parts(step: w::Step) -> w::ChangePanelParts {
        w::ChangePanelParts {
            task: TASK,
            forge: FORGE,
            owner: b"acme".as_slice().into(),
            name: b"kit".as_slice().into(),
            branch: b"temper/qa".as_slice().into(),
            landing: b"main".as_slice().into(),
            head: Some([0xab; 32]),
            pull: Some(
                w::Pull::new(&w::CEILINGS, w::PullParts { number: 216, address: Box::new([]) })
                    .expect("empty address is valid"),
            ),
            step,
            hold: None,
            runs: None,
            ci: w::Ci::Unknown,
            gates: List::with_capacity(0),
            repairs: used(0, 3),
            resolutions: used(0, 3),
            updates: used(0, 3),
        }
    }
    fn queued(place: u32, ahead: &[u64]) -> w::Step {
        let mut tasks = List::with_capacity(wire::TASKS_AHEAD);
        for task in ahead {
            tasks.push(*task).expect("bounded tasks ahead");
        }
        w::Step::Queued(w::Queued::new(&w::CEILINGS, w::QueuedParts { place, ahead: tasks }).expect("queued"))
    }
    fn repairing(reason: w::Reason) -> w::Step {
        w::Step::Repairing(w::Repairing::new(&w::CEILINGS, w::RepairingParts { reason }).expect("repair"))
    }
    fn gate_reason() -> w::Reason {
        w::Reason::Gate(w::GateReason::new(&w::CEILINGS, w::GateReasonParts { gate: u64::MAX }).expect("gate reason"))
    }
    fn landed() -> w::Step {
        w::Step::Landed(w::Landed::new(&w::CEILINGS, w::LandedParts { merge: [0xab; 32] }).expect("merge"))
    }
    fn steps() -> Vec<w::Step> {
        vec![
            w::Step::Producing,
            w::Step::Opening,
            w::Step::Recreating,
            w::Step::Reopening,
            w::Step::Checking,
            w::Step::Gating,
            queued(3, &[7, 6]),
            w::Step::First,
            w::Step::Updating,
            w::Step::Resolving,
            repairing(w::Reason::Ci),
            repairing(gate_reason()),
            repairing(w::Reason::Semantic),
            w::Step::Landing,
            landed(),
        ]
    }

    fn show(parts: w::ChangePanelParts) -> (Domain, Token) {
        let (mut root, _, _, requests) = opened(document());
        let watch = requests.iter().find_map(change_open).expect("forge panel watch");
        let panel = w::ChangePanel::new(&w::CEILINGS, parts).expect("valid panel");
        drop(up(
            &mut root,
            jig_client::Up::FamilyDelivered {
                watch,
                record: jig_client::Family {
                    family: FORGE,
                    version: wire::CLIENT_VERSION,
                    bytes: wire::change_panel(&panel),
                },
            },
        ));
        (root, watch)
    }
    fn view(root: &Domain) -> temper_client_view::View {
        let words = temper_client_view::words::Words::english(FORGE, 64, temper_client_view::words::Shell::Browser)
            .expect("English");
        let limits = temper_client_view::Limits::of(&domain_limits(), &words).expect("view envelope");
        let mut view = temper_client_view::View::new(&limits, &domain_limits(), words, FORGE);
        temper_client_view::build(&mut view, root, Wall::EPOCH, &mut Queue::with_capacity(limits.tree.patches));
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
    fn assert_no_forge_link(step: w::Step) {
        let (root, _) = show(panel_parts(step.clone()));
        let mut view = view(&root);
        {
            use skein_fake_person::{Face, Find, Found, TreeFace};
            for role in [skein_ui::Role::Link, skein_ui::Role::Button] {
                assert_eq!(
                    TreeFace::new(view.tree()).find(&Find::exactly_named(role, "View on forge ↗".as_bytes())),
                    Found::Absent,
                    "empty address at {step:?}: no accessible forge control"
                );
            }
        }
        assert!(!text(view.tree().nodes()).contains("View on forge"), "empty address at {step:?}: no forge control");
        let links: Vec<_> =
            view.tree().nodes().iter().filter(|n| n.role == skein_ui::Role::Link).map(|n| n.id).collect();
        for node in links {
            assert!(
                temper_client_view::href(&mut view, node).is_none_or(|href| !href.is_empty()),
                "empty address at {step:?}: no empty link destination"
            );
        }
    }
    macro_rules! empty_address {
        ($name:ident, $step:expr) => {
            // client/README.md 5.10: "only when the panel supplies an address";
            // client/forge.md 9.2: an over-bound address gives no "View on forge".
            #[test]
            fn $name() {
                assert_no_forge_link($step);
            }
        };
    }
    empty_address!(empty_address_producing, w::Step::Producing);
    empty_address!(empty_address_opening, w::Step::Opening);
    empty_address!(empty_address_recreating, w::Step::Recreating);
    empty_address!(empty_address_reopening, w::Step::Reopening);
    empty_address!(empty_address_checking, w::Step::Checking);
    empty_address!(empty_address_gating, w::Step::Gating);
    empty_address!(empty_address_queued, queued(3, &[7, 6]));
    empty_address!(empty_address_first, w::Step::First);
    empty_address!(empty_address_updating, w::Step::Updating);
    empty_address!(empty_address_resolving, w::Step::Resolving);
    empty_address!(empty_address_repair_ci, repairing(w::Reason::Ci));
    empty_address!(empty_address_repair_gate, repairing(gate_reason()));
    empty_address!(empty_address_repair_semantic, repairing(w::Reason::Semantic));
    empty_address!(empty_address_landing, w::Step::Landing);
    empty_address!(empty_address_landed, landed());

    // client/README.md 5.10: "external link ... to its pull request's address".
    #[test]
    fn supplied_address_keeps_the_exact_destination() {
        use skein_fake_person::{Face, Find, Found, TreeFace};
        let mut parts = panel_parts(w::Step::Checking);
        let address = b"https://forge.example/acme/kit/pulls/216?from=qa#head";
        parts.pull = Some(
            w::Pull::new(&w::CEILINGS, w::PullParts { number: 216, address: address.as_slice().into() }).expect("pull"),
        );
        let (root, _) = show(parts);
        let mut view = view(&root);
        let Found::Node(node) =
            TreeFace::new(view.tree()).find(&Find::exactly_named(skein_ui::Role::Link, "View on forge ↗".as_bytes()))
        else {
            unreachable!("supplied address needs the forge link: {}", text(view.tree().nodes()))
        };
        assert_eq!(
            temper_client_view::href(&mut view, node).as_deref(),
            Some(address.as_slice()),
            "supplied address is exact"
        );
    }

    // client/README.md 5.10: "No pull request yet · OWNER/NAME · BRANCH → LANDING"
    // and "only when the panel supplies an address".
    #[test]
    fn absent_pull_has_no_forge_link_and_names_the_branches() {
        for step in steps() {
            let mut parts = panel_parts(step.clone());
            parts.pull = None;
            let (root, _) = show(parts);
            let view = view(&root);
            let page = text(view.tree().nodes());
            assert!(
                page.contains("No pull request yet · acme/kit · temper/qa → main"),
                "no-pull line at {step:?}: {page}"
            );
            assert!(!page.contains("View on forge"), "no pull at {step:?} supplies no forge link: {page}");
        }
    }

    fn task_link(view: &mut temper_client_view::View, task: u64) {
        use skein_fake_person::{Face, Find, Found, TreeFace};
        let name = format!("Task {task}");
        let Found::Node(node) =
            TreeFace::new(view.tree()).find(&Find::exactly_named(skein_ui::Role::Link, name.as_bytes()))
        else {
            unreachable!("missing {name} link: {}", text(view.tree().nodes()))
        };
        let destination = domain::address::print(&domain::Address::Task { task, filter: domain::PlanFilter::All });
        assert_eq!(
            temper_client_view::href(view, node).as_deref(),
            Some(destination.as_ref()),
            "{name} opens its task page"
        );
    }
    fn assert_order(page: &str, words: &[&str]) {
        let mut after = 0;
        for word in words {
            let rest = page.get(after..).expect("previous word ended inside page");
            let position =
                rest.find(word).unwrap_or_else(|| unreachable!("{word:?} is absent or out of order: {page}"));
            after = after.checked_add(position).and_then(|n| n.checked_add(word.len())).expect("page offset fits");
        }
    }
    fn step_lines() -> Vec<(w::Step, &'static str, &'static str)> {
        vec![
            (w::Step::Producing, "Producing its branch.", "producing"),
            (w::Step::Opening, "Opening its pull request.", "opening"),
            (w::Step::Recreating, "Making its branch again at the head temper last knew.", "recreating"),
            (w::Step::Reopening, "Reopening its pull request.", "reopening"),
            (w::Step::Checking, "Waiting for CI at its head.", "checking"),
            (w::Step::Gating, "Waiting for its gates at its head.", "gating"),
            (queued(3, &[7, 6]), "3rd in the queue for main, after Task 7 and Task 6.", "queued"),
            (w::Step::First, "First in the queue for main.", "first in the queue"),
            (w::Step::Updating, "Updating from main.", "updating"),
            (w::Step::Resolving, "Resolving its conflicts with main.", "resolving"),
            (repairing(w::Reason::Ci), "Repairing: CI failed at its head.", "repairing"),
            (repairing(gate_reason()), "Repairing: gate 18446744073709551615 asked for changes.", "repairing"),
            (repairing(w::Reason::Semantic), "Repairing: a semantic conflict with main.", "repairing"),
            (w::Step::Landing, "Merging at its head.", "landing"),
            (landed(), "Landed · merge commit abababa", "landed"),
        ]
    }

    // client/README.md 5.10: six steps in order, then "Its step's line ... then
    // the Task N link of the task its step runs", before "Its tasks" and gates/bounds.
    #[test]
    fn every_step_line_precedes_its_running_task_link_and_tasks_region() {
        for (step, line, _) in step_lines() {
            let mut parts = panel_parts(step.clone());
            parts.runs = Some(u64::MAX);
            let (root, _) = show(parts);
            let mut view = view(&root);
            let page = text(view.tree().nodes());
            assert_order(
                &page,
                &[
                    "Producing",
                    "Opening",
                    "Checking",
                    "Gating",
                    "Queued",
                    "Landed",
                    line,
                    "Task 18446744073709551615",
                    "Its tasks",
                    "Gates at its head",
                    "Repairs 0 of 3",
                ],
            );
            task_link(&mut view, u64::MAX);
        }
    }

    // client/README.md 5.10: "Queued · Nth", including the full English ordinal;
    // the line is "Nth in the queue for LANDING" with the same ordinal.
    #[test]
    fn queue_ordinals_are_rendered_in_the_steps_and_the_line() {
        for (number, suffix) in [
            (1, "st"),
            (2, "nd"),
            (3, "rd"),
            (4, "th"),
            (11, "th"),
            (12, "th"),
            (13, "th"),
            (21, "st"),
            (22, "nd"),
            (23, "rd"),
            (101, "st"),
            (111, "th"),
            (112, "th"),
            (113, "th"),
            (u32::MAX, "th"),
        ] {
            let (root, _) = show(panel_parts(queued(number, &[])));
            let view = view(&root);
            let page = text(view.tree().nodes());
            assert!(page.contains(&format!("Queued · {number}{suffix}")), "bar ordinal {number}: {page}");
            assert!(page.contains(&format!("{number}{suffix} in the queue for main")), "line ordinal {number}: {page}");
        }
        let (root, _) = show(panel_parts(w::Step::First));
        let page = text(view(&root).tree().nodes());
        assert!(page.contains("Queued · 1st"), "first bar: {page}");
    }

    // client/README.md 5.10: "after Task A, Task B and N more", nearest first,
    // the named tasks as links to their pages, not just words.
    #[test]
    fn queue_line_names_nearest_tasks_and_the_unnamed_tail() {
        let (root, _) = show(panel_parts(queued(6, &[81, 80])));
        let mut view = view(&root);
        let page = text(view.tree().nodes());
        assert!(page.contains("6th in the queue for main, after Task 81, Task 80 and 3 more."), "queue tail: {page}");
        task_link(&mut view, 81);
        task_link(&mut view, 80);
    }

    // client/README.md 5.10: "after N tasks when it names none", one named task,
    // "and no after when nothing is ahead".
    #[test]
    fn queue_line_handles_no_names_one_name_and_no_predecessor() {
        for (step, line) in [
            (queued(6, &[]), "6th in the queue for main, after 5 tasks."),
            (queued(2, &[81]), "2nd in the queue for main, after Task 81."),
            (queued(1, &[]), "1st in the queue for main."),
            (w::Step::First, "First in the queue for main."),
        ] {
            let (root, _) = show(panel_parts(step));
            let mut view = view(&root);
            let page = text(view.tree().nodes());
            assert!(page.contains(line), "queue special case: {page}");
            if line.contains("Task 81") {
                task_link(&mut view, 81);
            }
            if !line.contains("after") {
                assert!(!page.contains("after"), "nothing ahead means no after: {page}");
            }
        }
    }

    // client/README.md 4.6 / 5.10: at most 64 named tasks ahead, all supplied
    // names shown as links; the remainder must not overflow at u32::MAX.
    #[test]
    fn queue_at_both_wire_bounds_keeps_every_named_task_link() {
        let ahead: Vec<u64> = (101..165).rev().collect();
        let (root, _) = show(panel_parts(queued(u32::MAX, &ahead)));
        let mut view = view(&root);
        let page = text(view.tree().nodes());
        assert!(
            page.contains("4294967295th in the queue for main, after Task 164, Task 163"),
            "queue starts nearest first: {page}"
        );
        assert!(page.contains("Task 101 and 4294967230 more."), "largest remaining count: {page}");
        for task in ahead {
            task_link(&mut view, task);
        }
    }

    fn holds() -> [(w::Hold, &'static str); 10] {
        [
            (w::Hold::Stalled, "its wait stalled"),
            (w::Hold::Repairs, "its repairs are spent"),
            (w::Hold::Resolutions, "its resolutions are spent"),
            (w::Hold::Updates, "its updates are spent"),
            (w::Hold::Moved, "its branch was moved on the forge"),
            (w::Hold::Missing, "its branch is missing on the forge"),
            (w::Hold::Closed, "its pull request was closed on the forge"),
            (w::Hold::Retargeted, "its pull request was retargeted on the forge"),
            (w::Hold::Failed, "a delegate or a write failed for good"),
            (w::Hold::Rejected, "a person rejected a gate"),
        ]
    }

    // client/README.md 5.10: "Held while STEP: REASON", STEP the held wire
    // step ("first in the queue" for first), after the step's line, before Its tasks.
    #[test]
    fn all_hold_reasons_name_the_actual_step_including_side_steps() {
        let mut failures = Vec::new();
        for (step, _, held_step) in step_lines() {
            for (hold, reason) in holds() {
                let mut parts = panel_parts(step.clone());
                parts.hold = Some(hold.clone());
                let (root, _) = show(parts);
                let page = text(view(&root).tree().nodes());
                let expected = format!("Held while {held_step}: {reason}.");
                if page.contains(&expected) {
                    assert_order(&page, &[&expected, "Its tasks", "Gates at its head"]);
                } else {
                    failures.push(format!("{step:?} / {hold:?}: expected {expected:?}; page {page}"));
                }
            }
        }
        assert!(failures.is_empty(), "hold cross-product failures: {failures:#?}");
    }

    fn subtree(nodes: &[skein_ui::Node], index: usize) -> &[skein_ui::Node] {
        let size = usize::try_from(nodes.get(index).expect("node index").size).expect("subtree size");
        let end = index.checked_add(size).expect("subtree end");
        nodes.get(index..end).expect("whole subtree")
    }
    fn gate_rows(view: &temper_client_view::View) -> Vec<String> {
        let mut rows = Vec::new();
        for (index, node) in view.tree().nodes().iter().enumerate() {
            if node.styles.as_slice().contains(&temper_client_view::look::GATE) {
                rows.push(text(subtree(view.tree().nodes(), index)));
            }
        }
        rows
    }

    // client/README.md 5.10: CI at its head is "Not yet", "Running", "Passed",
    // "Failed"; with none, a last row "No other gate.".
    #[test]
    fn ci_states_and_no_other_gate_are_whole_rows() {
        for (ci, verdict) in [
            (w::Ci::Unknown, "Not yet"),
            (w::Ci::Pending, "Running"),
            (w::Ci::Passed, "Passed"),
            (w::Ci::Failed, "Failed"),
        ] {
            let mut parts = panel_parts(w::Step::Checking);
            parts.ci = ci;
            let (root, _) = show(parts);
            let rows = gate_rows(&view(&root));
            assert_eq!(rows.len(), 2, "CI plus the no-other-gate row");
            assert_eq!(rows.first().expect("CI row"), &format!("CI at its head{verdict}"), "CI at exact head");
            assert_eq!(rows.last().expect("last row"), "No other gate.", "no invented landing rules");
        }
    }

    // client/README.md 5.10: each gate, "in the supplied order", names its
    // kind and blocking/advisory, with "Not yet", "Waiting", "Approved", "Changes asked".
    #[test]
    fn gates_preserve_kind_blocking_verdict_and_order_at_the_wire_bound() {
        let mut parts = panel_parts(w::Step::Gating);
        parts.gates = List::with_capacity(wire::GATES);
        let mut expected = Vec::new();
        for number in 0..wire::GATES {
            let (kind, name) = match number % 3 {
                0 => (w::GateKind::Review, "review"),
                1 => (w::GateKind::Approval, "approval"),
                _ => (w::GateKind::Check, "check"),
            };
            let (verdict, word) = match number % 4 {
                0 => (w::Verdict::None, "Not yet"),
                1 => (w::Verdict::Pending, "Waiting"),
                2 => (w::Verdict::Approved, "Approved"),
                _ => (w::Verdict::Changes, "Changes asked"),
            };
            // Blocks alternates every four, exercising each verdict both ways.
            let blocks = (number / 4) % 2 == 0;
            let blocking = if blocks { "blocking" } else { "advisory" };
            let id = u64::MAX.saturating_sub(u64::from(number));
            parts
                .gates
                .push(w::Gate::new(&w::CEILINGS, w::GateParts { number: id, kind, blocks, verdict }).expect("gate"))
                .expect("64 gates");
            expected.push(format!("Gate {id} · {name} · {blocking}{word}"));
        }
        let (root, _) = show(parts);
        let view = view(&root);
        let rows = gate_rows(&view);
        assert_eq!(rows.len(), 65, "CI plus all 64 gates, regardless of ordinary list row bound");
        assert_eq!(rows.get(1..).expect("gate rows after CI"), expected, "gate facts and supplied order stay whole");
        assert!(!text(view.tree().nodes()).contains("No other gate."), "nonempty gates have no empty marker");
    }

    // client/README.md 5.10: "Repairs N of M · resolutions N of M · updates N
    // of M", each supplied number unchanged, including exhausted and over-limit values.
    #[test]
    fn bounds_show_zero_below_at_past_and_the_largest_wire_numbers() {
        for (used_count, limit) in [(0, 0), (0, 3), (2, 3), (3, 3), (4, 3), (u32::MAX, u32::MAX), (u32::MAX, 0)] {
            let mut parts = panel_parts(w::Step::Gating);
            parts.repairs = used(used_count, limit);
            parts.resolutions = used(limit, used_count);
            parts.updates = used(used_count, 1);
            let (root, _) = show(parts);
            let page = text(view(&root).tree().nodes());
            let expected = format!(
                "Repairs {used_count} of {limit} · resolutions {limit} of {used_count} · updates {used_count} of 1"
            );
            assert!(page.contains(&expected), "bounds are facts, not clamped or subtracted: {page}");
            assert_order(&page, &["Gates at its head", &expected]);
        }
    }

    fn assert_unavailable(root: &Domain, expected: &str) {
        let page = text(view(root).tree().nodes());
        assert!(page.contains(expected), "unavailable reason: expected {expected:?}, got {page}");
        assert!(page.contains("Its tasks"), "unavailable panel does not remove its tasks: {page}");
        for omitted in ["Change · Task 9", "View on forge", "CI at its head", "Repairs 0 of 3"] {
            assert!(!page.contains(omitted), "unavailable panel must omit {omitted:?}: {page}");
        }
    }

    // client/README.md 3.5 / 5.10 and jig client/domain/root.md 6:
    // "Loading the change from the forge." until its first delivery.
    #[test]
    fn loading_panel_keeps_the_task_page_without_drawing_panel_facts() {
        let (root, _, _, _) = opened(document());
        assert_unavailable(&root, "Loading the change from the forge.");
    }

    // client/README.md 5.10: "The forge knows no change for this task."
    // when the forge answers unknown, including a previously shown panel.
    #[test]
    fn unknown_after_a_shown_panel_removes_its_old_facts() {
        let (mut root, watch) = show(panel_parts(w::Step::Checking));
        drop(up(&mut root, jig_client::Up::WatchEnded { watch, end: c::WatchEnd::Unknown }));
        assert_unavailable(&root, "The forge knows no change for this task.");
    }

    // jig-client-domain watches.rs maps a non-own WatchEnd::Closed to Why::SubjectEnded;
    // forge::panel::watched maps that to State::Unknown (client/README.md 5.10:
    // "one whose subject left the engine"). Old steps cannot masquerade as current facts.
    #[test]
    fn a_closed_panel_watch_removes_its_old_facts() {
        let (mut root, watch) = show(panel_parts(w::Step::Checking));
        drop(up(&mut root, jig_client::Up::WatchEnded { watch, end: c::WatchEnd::Closed }));
        assert_unavailable(&root, "The forge knows no change for this task.");
    }

    // client/README.md 5.10 / jig client/domain/root.md 6: "a part its owner
    // cannot show says so; neither breaks the page", including access loss while open.
    #[test]
    fn access_failure_of_the_panel_watch_keeps_tasks_and_says_cannot_show() {
        let (mut root, watch) = show(panel_parts(w::Step::Checking));
        drop(up(&mut root, jig_client::Up::WatchEnded { watch, end: c::WatchEnd::NotAllowed }));
        assert_unavailable(&root, "The forge cannot show this change.");
    }

    // client/forge.md 9.2: "Its subject names the change task by its number";
    // README 5.10: a page draws its own procedure's panel, not another task's facts.
    #[test]
    fn a_delivery_for_another_task_never_draws_as_this_tasks_change() {
        let mut parts = panel_parts(w::Step::Checking);
        parts.task = 88;
        parts.owner = b"foreign".as_slice().into();
        let (root, _) = show(parts);
        let page = text(view(&root).tree().nodes());
        assert!(!page.contains("foreign/kit"), "another task's repository never attaches to Task 9: {page}");
        assert!(!page.contains("CI at its head"), "another task's gates never attach: {page}");
        assert!(page.contains("Its tasks"), "Task 9's own tasks remain");
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
    fn end(task: &mut c::Task) {
        task.chip.phase = c::Phase::Ended;
        let c::ChipPhase::Owner { phase, details, .. } = task.chip.details.as_mut() else { unreachable!("owner") };
        *phase = c::OwnerChipPhase::Ended;
        details.ended_at = Some(Wall::EPOCH);
        details.ending = Some(c::OwnerChipEnding::Failed(c::DocumentText::complete(
            c::Text::new(b"ended".as_slice().into(), 32).expect("reason"),
        )));
    }

    // client/README.md 5.10: until the read answers, "No live tasks."; after
    // an empty answer, "None yet." while the task lives and "None." once ended.
    #[test]
    fn an_empty_tasks_region_distinguishes_unread_live_and_ended() {
        for ended in [false, true] {
            let mut task = document();
            if ended {
                end(&mut task);
            }
            let (mut root, _, _, requests) = opened(task);
            let page = text(view(&root).tree().nodes());
            assert!(page.contains("No live tasks."), "unread ended index: {page}");
            answer_all_initial_reads(&mut root, &requests, &empty_page());
            let page = text(view(&root).tree().nodes());
            let expected = if ended { "None." } else { "None yet." };
            assert!(page.contains(expected), "empty tasks after read: {page}");
            assert!(!page.contains("No live tasks."), "read answer replaces loading marker: {page}");
            if ended {
                assert!(!page.contains("None yet."), "ended procedure has no yet: {page}");
            }
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

    // client/README.md 5.10: "Its ended ones are 5.8's ended rows, read once from
    // the engine"; "When the read's page holds less than all of them", the last line
    // is "More ... are not shown."
    #[test]
    fn a_full_ended_page_is_not_more_but_a_cursor_or_cut_is() {
        let count = domain_limits().client.domain.rows;
        for (cut, after) in [(false, false), (false, true), (true, false)] {
            let mut page = empty_page();
            let mut rows = List::with_capacity(count);
            for number in 101..101_u64.saturating_add(u64::from(count)) {
                rows.push(ended_line(number, TASK)).expect("page rows");
            }
            page.ended = c::EndedGroup::Rows {
                rows,
                cut,
                after: after.then(|| c::Cursor::new(b"more".as_slice().into(), 32).expect("cursor")),
            };
            let (mut root, _, _, requests) = opened(document());
            answer_all_initial_reads(&mut root, &requests, &page);
            let mut view = view(&root);
            let text = text(view.tree().nodes());
            assert_eq!(
                text.contains("More of its ended tasks are not shown."),
                cut || after,
                "full page does not imply more by itself: {text}"
            );
            for number in 101..101_u64.saturating_add(u64::from(count)) {
                task_row_link(&mut view, number);
            }
        }
    }
    fn task_row_link(view: &mut temper_client_view::View, task: u64) {
        use skein_fake_person::{Face, Find, Found, TreeFace};
        let name = format!("Task {task}");
        let Found::Node(node) = TreeFace::new(view.tree()).find(&Find::named(skein_ui::Role::Link, name.as_bytes()))
        else {
            unreachable!("missing row link {name}: {}", text(view.tree().nodes()))
        };
        let destination = domain::address::print(&domain::Address::Task { task, filter: domain::PlanFilter::All });
        assert_eq!(
            temper_client_view::href(view, node).as_deref(),
            Some(destination.as_ref()),
            "ended row opens its own task"
        );
    }

    // client/README.md 5.10: the change's words are the forge table's;
    // browser.md 2.4 / README 6: both supported shells show the same panel facts.
    #[test]
    fn terminal_and_browser_words_show_the_panel_at_the_domain_text_bound() {
        let (root, _) = show(panel_parts(w::Step::Gating));
        for shell in [temper_client_view::words::Shell::Terminal, temper_client_view::words::Shell::Browser] {
            let words = temper_client_view::words::Words::english(FORGE, domain_limits().client.domain.text, shell)
                .expect("English at domain bound");
            let limits = temper_client_view::Limits::of(&domain_limits(), &words).expect("view limits");
            let mut view = temper_client_view::View::new(&limits, &domain_limits(), words, FORGE);
            temper_client_view::build(&mut view, &root, Wall::EPOCH, &mut Queue::with_capacity(limits.tree.patches));
            let page = text(view.tree().nodes());
            assert!(
                page.contains("Waiting for its gates at its head."),
                "panel words at supported shell/bound: {page}"
            );
            assert!(page.contains("Change · Task 9"), "panel tag at supported shell/bound: {page}");
        }
    }

    // client/README.md 5.10: Goal and Task G follow the project in the
    // header, only when the goal is another task; a tracked task serves itself.
    #[test]
    fn header_goal_slot_links_only_to_another_goal() {
        for goal in [None, Some(TASK), Some(12)] {
            let mut task = document();
            task.goal = goal;
            let (mut root, _, _, requests) = opened(task);
            let watch = requests.iter().find_map(change_open).expect("panel watch");
            let panel = w::ChangePanel::new(&w::CEILINGS, panel_parts(w::Step::Opening)).expect("panel");
            drop(up(
                &mut root,
                jig_client::Up::FamilyDelivered {
                    watch,
                    record: jig_client::Family {
                        family: FORGE,
                        version: wire::CLIENT_VERSION,
                        bytes: wire::change_panel(&panel),
                    },
                },
            ));
            let mut view = view(&root);
            let page = text(view.tree().nodes());
            if goal == Some(12) {
                assert_order(&page, &["Change · Task 9", "Shop", "Goal", "Task 12", "PR #216"]);
                task_link(&mut view, 12);
            } else {
                assert!(!page.contains("Goal"), "no other goal means no slot: {page}");
            }
        }
    }

    // client/README.md 4.6 / 5.10: address accepts 2048 bytes and the page
    // links to the supplied address, not a client reconstruction of the forge URL.
    #[test]
    fn address_exactly_at_the_wire_bound_is_not_cut() {
        use skein_fake_person::{Face, Find, Found, TreeFace};
        let mut address = b"https://forge.example/".to_vec();
        address.resize(usize::try_from(wire::ADDRESS_BYTES).expect("bound fits"), b'x');
        let mut parts = panel_parts(w::Step::Gating);
        parts.pull = Some(
            w::Pull::new(&w::CEILINGS, w::PullParts { number: u64::MAX, address: address.clone().into() })
                .expect("maximum address"),
        );
        let (root, _) = show(parts);
        let mut view = view(&root);
        let Found::Node(node) =
            TreeFace::new(view.tree()).find(&Find::exactly_named(skein_ui::Role::Link, "View on forge ↗".as_bytes()))
        else {
            unreachable!("maximum address has a forge link")
        };
        assert_eq!(
            temper_client_view::href(&mut view, node).as_deref(),
            Some(address.as_slice()),
            "whole maximum address"
        );
        assert!(text(view.tree().nodes()).contains("PR #18446744073709551615"), "largest pull number is whole");
    }

    // client/README.md 5.10: each side step has its own line; keep these
    // independently reachable even if the combined ordering probe fails earlier.
    #[test]
    fn side_steps_and_landing_draw_their_own_lines() {
        for (step, line, _) in step_lines() {
            match step {
                w::Step::Queued(_) => (),
                w::Step::Producing
                | w::Step::Opening
                | w::Step::Recreating
                | w::Step::Reopening
                | w::Step::Checking
                | w::Step::Gating
                | w::Step::First
                | w::Step::Updating
                | w::Step::Resolving
                | w::Step::Repairing(_)
                | w::Step::Landing
                | w::Step::Landed(_) => {
                    let (root, _) = show(panel_parts(step));
                    let page = text(view(&root).tree().nodes());
                    assert!(page.contains(line), "step's own line {line:?}: {page}");
                }
            }
        }
    }

    // client/README.md 5.10: "Its ended ones are 5.8's ended rows, read once from
    // the engine"; an ended procedure can have no live tree and still has these rows.
    #[test]
    fn an_ended_procedure_without_a_live_tree_still_lists_ended_children() {
        let mut task = document();
        end(&mut task);
        let (mut root, _, _, requests) = opened_with_tree(task, false);
        let mut page = empty_page();
        let mut rows = List::with_capacity(1);
        rows.push(ended_line(101, TASK)).expect("ended child");
        page.ended = c::EndedGroup::Rows { rows, after: None, cut: false };
        answer_all_initial_reads(&mut root, &requests, &page);
        let mut view = view(&root);
        let page = text(view.tree().nodes());
        assert!(page.contains("Its tasks"), "ended-child region does not require a live tree: {page}");
        assert!(page.contains("ended child 101"), "ended child shown without live tree: {page}");
        task_row_link(&mut view, 101);
    }

    // client/README.md 5.10: the shown change's card holds "Its tasks",
    // live and ended, including a landed change with no surviving live tree.
    #[test]
    fn a_landed_panel_without_a_live_tree_keeps_its_ended_child_rows() {
        let mut task = document();
        end(&mut task);
        let (mut root, _, _, requests) = opened_with_tree(task, false);
        let watch = requests.iter().find_map(change_open).expect("panel watch");
        let panel = w::ChangePanel::new(&w::CEILINGS, panel_parts(landed())).expect("panel");
        drop(up(
            &mut root,
            jig_client::Up::FamilyDelivered {
                watch,
                record: jig_client::Family {
                    family: FORGE,
                    version: wire::CLIENT_VERSION,
                    bytes: wire::change_panel(&panel),
                },
            },
        ));
        let mut page = empty_page();
        let mut rows = List::with_capacity(1);
        rows.push(ended_line(101, TASK)).expect("ended child");
        page.ended = c::EndedGroup::Rows { rows, after: None, cut: false };
        answer_all_initial_reads(&mut root, &requests, &page);
        let mut view = view(&root);
        let page = text(view.tree().nodes());
        assert_order(&page, &["Landed · merge commit abababa", "Its tasks", "ended child 101", "Gates at its head"]);
        task_row_link(&mut view, 101);
    }

    // client/README.md 5.10: STEP_DONE on each prior step, STEP_CURRENT on
    // the serving step, and all six done once landed; the list keeps six slots.
    #[test]
    fn drawn_steps_mark_the_side_steps_serving_slot_and_only_prior_slots_done() {
        for (step, current, done) in [
            (w::Step::Producing, Some("Producing"), 0),
            (w::Step::Opening, Some("Opening"), 1),
            (w::Step::Recreating, Some("Opening"), 1),
            (w::Step::Reopening, Some("Opening"), 1),
            (w::Step::Checking, Some("Checking"), 2),
            (w::Step::Gating, Some("Gating"), 3),
            (queued(3, &[7, 6]), Some("Queued · 3rd"), 4),
            (w::Step::First, Some("Queued · 1st"), 4),
            (w::Step::Updating, Some("Queued"), 4),
            (w::Step::Resolving, Some("Queued"), 4),
            (repairing(w::Reason::Ci), Some("Checking"), 2),
            (repairing(gate_reason()), Some("Gating"), 3),
            (repairing(w::Reason::Semantic), Some("Queued"), 4),
            (w::Step::Landing, Some("Landed"), 5),
            (landed(), None, 6),
        ] {
            let (root, _) = show(panel_parts(step.clone()));
            let view = view(&root);
            let mut names = Vec::new();
            let mut current_names = Vec::new();
            let mut done_names = Vec::new();
            for (index, node) in view.tree().nodes().iter().enumerate() {
                if node.styles.as_slice().contains(&temper_client_view::look::STEP) {
                    let name = text(subtree(view.tree().nodes(), index));
                    names.push(name.clone());
                    if node.styles.as_slice().contains(&temper_client_view::look::STEP_CURRENT) {
                        current_names.push(name.clone());
                    }
                    if node.styles.as_slice().contains(&temper_client_view::look::STEP_DONE) {
                        done_names.push(name);
                    }
                }
            }
            assert_eq!(names.len(), 6, "six drawn slots at {step:?}");
            assert_eq!(current_names.len(), usize::from(current.is_some()), "one current unless landed: {step:?}");
            if let Some(current) = current {
                assert_eq!(current_names.first().expect("current"), current, "side step serving slot for {step:?}");
            }
            assert_eq!(done_names, names.get(..done).expect("prior slots"), "only preceding slots done for {step:?}");
        }
    }

    // client/README.md 5.10: "Until its panel is shown ... the page is the
    // bar, the header and Its tasks"; an unread live tree cannot hide ended rows.
    #[test]
    fn a_live_procedure_with_a_tree_still_loading_shows_its_ended_children() {
        let (mut root, _, _, requests) = opened_with_tree(document(), false);
        let mut page = empty_page();
        let mut rows = List::with_capacity(1);
        rows.push(ended_line(101, TASK)).expect("ended child");
        page.ended = c::EndedGroup::Rows { rows, after: None, cut: false };
        answer_all_initial_reads(&mut root, &requests, &page);
        let page = text(view(&root).tree().nodes());
        assert_order(&page, &["Loading the change from the forge.", "Its tasks", "ended child 101"]);
    }
}
