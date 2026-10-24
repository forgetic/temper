//! QA1: design-driven change projection and ended-child reads.
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
        after.extend(deliver(&mut root, tree_watch, tree(&document)));
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

    // client/README.md 5.10, "Its steps": "each side step at the one it serves",
    // "the one its step is at is current, those before it done", all six once landed.
    #[test]
    fn every_step_and_repair_reason_has_its_design_slot() {
        let expected = [
            (w::Step::Producing, Some(0), 0),
            (w::Step::Opening, Some(1), 1),
            (w::Step::Recreating, Some(1), 1),
            (w::Step::Reopening, Some(1), 1),
            (w::Step::Checking, Some(2), 2),
            (w::Step::Gating, Some(3), 3),
            (queued(3, &[7, 6]), Some(4), 4),
            (w::Step::First, Some(4), 4),
            (w::Step::Updating, Some(4), 4),
            (w::Step::Resolving, Some(4), 4),
            (repairing(w::Reason::Ci), Some(2), 2),
            (repairing(gate_reason()), Some(3), 3),
            (repairing(w::Reason::Semantic), Some(4), 4),
            (w::Step::Landing, Some(5), 5),
            (landed(), None, 6),
        ];
        for (step, current, done) in expected {
            let slots = forge::change::slot(&step);
            assert_eq!(slots.current, current, "current slot for {step:?}");
            assert_eq!(slots.done, done, "completed slots for {step:?}");
        }
    }

    // client/README.md 5.10: "the place an English ordinal", including the teen exceptions;
    // README 4.6: queue place is u32, starting at 1.
    #[test]
    fn queue_ordinals_include_teens_centuries_and_the_wire_maximum() {
        use forge::change::Ordinal;
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
            let actual = match forge::change::ordinal(number) {
                Ordinal::St => "st",
                Ordinal::Nd => "nd",
                Ordinal::Rd => "rd",
                Ordinal::Th => "th",
            };
            assert_eq!(actual, suffix, "ordinal of {number}");
        }
    }

    // client/README.md 5.10: "the tasks ahead it names, nearest first" and
    // "and N more for those it does not name"; README 4.6 bounds names at 64.
    #[test]
    fn queue_keeps_nearest_names_and_counts_only_the_unnamed_remainder() {
        for (place, ahead, unnamed) in [
            (1, vec![], 0),
            (2, vec![81], 0),
            (6, vec![81, 80], 3),
            (6, vec![], 5),
            (u32::MAX, vec![], u32::MAX.saturating_sub(1)),
        ] {
            let step = queued(place, &ahead);
            let queue = forge::change::queue(&step).expect("queued has queue facts");
            assert_eq!(queue.place, place, "place comes from panel");
            assert_eq!(queue.ahead, ahead, "queue retains nearest-first order");
            assert_eq!(queue.unnamed, unnamed, "unnamed tasks at place {place}");
        }
        let ahead: Vec<u64> = (101..165).rev().collect();
        let step = queued(u32::MAX, &ahead);
        let queue = forge::change::queue(&step).expect("maximum queue");
        assert_eq!(queue.ahead, ahead, "all 64 names retained in order");
        assert_eq!(queue.unnamed, u32::MAX.saturating_sub(65), "largest unnamed remainder");
    }

    // client/README.md 5.10: "Queued · 1st when first"; other side steps
    // remain at their serving slot without inventing a queue place.
    #[test]
    fn first_has_place_one_and_nonqueue_steps_do_not_invent_places() {
        let queue = forge::change::queue(&w::Step::First).expect("first has a queue place");
        assert_eq!(queue.place, 1, "first is first");
        assert!(queue.ahead.is_empty(), "first has nothing ahead");
        assert_eq!(queue.unnamed, 0, "first has no unnamed predecessor");
        for step in steps() {
            match step {
                w::Step::Queued(_) | w::Step::First => (),
                w::Step::Producing
                | w::Step::Opening
                | w::Step::Recreating
                | w::Step::Reopening
                | w::Step::Checking
                | w::Step::Gating
                | w::Step::Updating
                | w::Step::Resolving
                | w::Step::Repairing(_)
                | w::Step::Landing
                | w::Step::Landed(_) => {
                    assert!(forge::change::queue(&step).is_none(), "no queue position supplied at {step:?}");
                }
            }
        }
    }

    // client/README.md 5.10: "Each fact is drawn as the panel supplies it";
    // client/forge.md 9.2: subject names the page's task, in its project's scope.
    #[test]
    fn another_tasks_panel_is_not_the_pages_panel() {
        let state = forge::panel::Panel {
            subject: forge::panel::Subject { task: 88, project: 1 },
            state: forge::panel::State::Shown(Box::new(
                w::ChangePanel::new(&w::CEILINGS, panel_parts(w::Step::Checking)).expect("panel"),
            )),
            handle: None,
        };
        assert!(forge::change::for_task(Some(&state), TASK).is_none(), "foreign subject never attaches to this page");
        assert!(forge::change::for_task(None, TASK).is_none(), "absent side remains absent");
    }

    fn ended_reads(requests: &[jig_client::Down]) -> Vec<(Token, c::Read)> {
        requests
            .iter()
            .filter_map(|request| match request {
                jig_client::Down::Read { read, what: jig_client::Readable::Jig(what @ c::Read::Tasks { .. }) } => {
                    Some((*read, what.clone()))
                }
                _ => None,
            })
            .collect()
    }
    fn ended_page(cut: bool, after: bool) -> c::TaskPage {
        c::TaskPage {
            live: List::with_capacity(0),
            live_after: None,
            ended: c::EndedGroup::Rows {
                rows: List::with_capacity(0),
                cut,
                after: after.then(|| c::Cursor::new(b"next".as_slice().into(), 32).expect("cursor")),
            },
        }
    }
    fn answer_ended(root: &mut Domain, read: Token, page: c::TaskPage) -> Vec<jig_client::Down> {
        up(
            root,
            jig_client::Up::ReadDone {
                read,
                result: jig_client::Readout::Jig(c::ReadResult::Value(c::ReadValue::Tasks(Box::new(page)))),
            },
        )
    }
    fn settled_ended() -> (Domain, Token, Token) {
        let (mut root, document, tree, requests) = opened(document());
        let reads = ended_reads(&requests);
        assert_eq!(reads.len(), 1, "one initial read");
        let (read, _) = reads.first().expect("initial ended read");
        let requests = answer_ended(&mut root, *read, ended_page(false, false));
        let reads = ended_reads(&requests);
        // opened delivered its tree after its document, so that invalidation may
        // already have asked the coalesced follow-up.
        assert!(reads.len() <= 1, "at most one coalesced read after initial answer");
        if let Some((read, _)) = reads.first() {
            let requests = answer_ended(&mut root, *read, ended_page(false, false));
            assert!(ended_reads(&requests).is_empty(), "no reread without another change");
        }
        (root, document, tree)
    }

    // client/README.md 5.10: "its children in the order they were made"
    // (Read::Tasks by parent and the ended phase, QueryOrder::Ended, by number);
    // "Every procedure's task reads them, its panel shown or not."
    #[test]
    fn procedure_reads_only_its_ended_children_without_needing_a_panel() {
        let (root, _, _, requests) = opened(document());
        let reads = ended_reads(&requests);
        assert_eq!(reads.len(), 1, "procedure opens exactly one ended-child read");
        let (_, read) = reads.first().expect("ended read");
        let c::Read::Tasks { query, live, ended } = read else { unreachable!("selected Tasks read") };
        assert_eq!(query.parent, Some(TASK), "only direct children of this procedure");
        assert_eq!(query.phase, Some(c::PhaseClass::Ended), "only ended children");
        assert_eq!(query.order, c::QueryOrder::Ended, "by number");
        assert_eq!(
            (query.requester, query.root, query.label, query.executor, query.tracked),
            (None, None, None, None, None),
            "no unrelated query restriction"
        );
        assert!(live.is_none() && ended.is_none(), "only the first page");
        assert!(
            root.pages().task().expect("page").ended.as_ref().expect("ended state").reading,
            "read kept as in flight"
        );
    }

    // client/README.md 5.10: "one read out at a time; each ... read again when
    // its tree or its document changes", including changes while a read is outstanding.
    #[test]
    fn tree_and_document_changes_coalesce_behind_one_read_and_keep_the_new_page() {
        let (mut root, document_watch, tree_watch) = settled_ended();
        let mut changed = document();
        changed.changed = Wall::from_nanos(1);
        let requests = deliver(&mut root, tree_watch, tree(&changed));
        let reads = ended_reads(&requests);
        assert_eq!(reads.len(), 1, "a tree change starts a read");
        let (old_read, _) = reads.first().expect("tree read");
        let requests = deliver(&mut root, document_watch, c::Delivery::Task(Box::new(changed.clone())));
        assert!(ended_reads(&requests).is_empty(), "document change cannot overlap the outstanding read");
        let requests = deliver(&mut root, tree_watch, tree(&changed));
        assert!(ended_reads(&requests).is_empty(), "tree change also waits");
        let ended = root.pages().task().expect("page").ended.as_ref().expect("ended");
        assert!(ended.reading && ended.stale, "newer facts marked stale while reading");
        let old_page = ended_page(true, true);
        let requests = answer_ended(&mut root, *old_read, old_page.clone());
        let reads = ended_reads(&requests);
        assert_eq!(reads.len(), 1, "new facts get one read after the old answer");
        let ended = root.pages().task().expect("page").ended.as_ref().expect("ended");
        assert_eq!(ended.page.as_deref(), Some(&old_page), "answered rows stay available during refresh");
        assert!(ended.reading && !ended.stale, "new read covers the coalesced changes");
        let (new_read, read) = reads.first().expect("refresh");
        let c::Read::Tasks { live, ended, .. } = read else { unreachable!("Tasks read") };
        assert!(live.is_none() && ended.is_none(), "refresh starts from first page, never follows the old cursor");
        let latest = ended_page(false, false);
        let requests = answer_ended(&mut root, *new_read, latest.clone());
        assert!(ended_reads(&requests).is_empty(), "no third read without newer facts");
        let ended = root.pages().task().expect("page").ended.as_ref().expect("ended");
        assert_eq!(ended.page.as_deref(), Some(&latest), "new page replaces old rows and cut marker");
        assert!(!ended.reading && !ended.stale, "refresh finishes cleanly");
    }

    // client/README.md 5.10: "read again when ... its document changes" even
    // when there is no tree change and no watchable panel yet.
    #[test]
    fn a_document_change_alone_refreshes_the_ended_children() {
        let (mut root, watch, _) = settled_ended();
        let mut changed = document();
        changed.activity = changed.activity.saturating_add(1);
        changed.changed = Wall::from_nanos(2);
        let requests = deliver(&mut root, watch, c::Delivery::Task(Box::new(changed)));
        assert_eq!(ended_reads(&requests).len(), 1, "document alone refreshes ended tasks");
    }

    // client/README.md 5.10: "The page reads no older page"; the panel's
    // records do not affect the ended query's first page.
    #[test]
    fn a_cut_ended_page_is_kept_without_following_its_cursor() {
        let (mut root, _, tree_watch) = settled_ended();
        let requests = deliver(&mut root, tree_watch, tree(&document()));
        let reads = ended_reads(&requests);
        let (read, _) = reads.first().expect("refresh");
        let page = ended_page(true, true);
        let requests = answer_ended(&mut root, *read, page.clone());
        assert!(ended_reads(&requests).is_empty(), "a page cursor never requests another page");
        assert_eq!(
            root.pages().task().expect("page").ended.as_ref().expect("ended").page.as_deref(),
            Some(&page),
            "whole cut page retained"
        );
    }

    // client/forge.md 9.2: "in the scope of the task's project" and the
    // subject names its task; README 3.5: leaving releases the page's panel state.
    // jig defers the native Close until Open is acknowledged (covered separately).
    #[test]
    fn panel_watch_names_the_task_and_project_and_navigation_releases_its_state() {
        let (mut root, _, _, requests) = opened(document());
        let mut panel_watch = None;
        for request in &requests {
            if let jig_client::Down::Open { watch, subject: jig_client::Watchable::Family { scope, record, .. } } =
                request
            {
                let watching = wire::decode_watching(&record.bytes, &w::CEILINGS).expect("watch record");
                // The deployment's forges watch opens beside the panel's.
                let w::Watch::Change(subject) = watching.watch() else { continue };
                assert_eq!(*scope, c::Scope::Project(1), "watch is admitted in this task's project");
                assert_eq!(subject.task(), TASK, "watch names the page's task");
                assert_eq!(record.family, FORGE, "forge owns the watch");
                panel_watch = Some(*watch);
            }
        }
        assert!(panel_watch.is_some(), "panel watch opened");
        drop(root_event(
            &mut root,
            Event::Went {
                place: domain::Place::Address(domain::Address::Inbox { filter: domain::address::Filter::ALL }),
            },
        ));
        assert!(root.forge().panel().is_none(), "leaving releases panel state");
    }

    // client/README.md 3.5 / 5.10: a page holds its own ended-child page;
    // an outstanding answer arriving after navigation cannot attach to a new task.
    #[test]
    fn an_old_ended_read_answer_cannot_attach_to_the_next_task() {
        let (mut root, _, _, requests) = opened(document());
        let reads = ended_reads(&requests);
        let (read, _) = reads.first().expect("old ended read");
        drop(root_event(
            &mut root,
            Event::Went {
                place: domain::Place::Address(domain::Address::Task { task: 88, filter: domain::PlanFilter::All }),
            },
        ));
        drop(answer_ended(&mut root, *read, ended_page(true, true)));
        let page = root.pages().task().expect("new page");
        assert_eq!(page.number, 88, "new page stays open");
        assert!(page.ended.is_none(), "old ended page is not the new task's page");
    }

    // client/README.md 3.5 / client/forge.md 9.2: a shown panel is watched
    // only while its task page is open, including leaving with a child read out.
    #[test]
    fn leaving_a_delivered_panel_releases_its_native_watch() {
        let (mut root, _, _, requests) = opened(document());
        let watch = requests.iter().find_map(change_open).expect("panel watch");
        let panel = w::ChangePanel::new(&w::CEILINGS, panel_parts(w::Step::Gating)).expect("panel");
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
        let requests = root_event(
            &mut root,
            Event::Went {
                place: domain::Place::Address(domain::Address::Inbox { filter: domain::address::Filter::ALL }),
            },
        );
        assert!(
            requests.contains(&jig_client::Down::Close { watch }),
            "leaving a delivered panel closes its native watch"
        );
        assert!(root.forge().panel().is_none(), "delivered panel released");
    }

    // client/README.md 3.5: "leaving it closes its handles" even if the
    // native Open is acknowledged late; the abandoned panel must never reattach.
    #[test]
    fn a_late_open_acknowledgement_closes_an_abandoned_panel_watch() {
        let (mut root, _, _, requests) = opened(document());
        let watch = requests.iter().find_map(change_open).expect("panel watch");
        drop(root_event(
            &mut root,
            Event::Went {
                place: domain::Place::Address(domain::Address::Inbox { filter: domain::address::Filter::ALL }),
            },
        ));
        assert!(root.forge().panel().is_none(), "abandoned loading panel released immediately");
        let requests = up(&mut root, jig_client::Up::Watched { watch, event: c::WatchEvent::Opened });
        assert!(
            requests.contains(&jig_client::Down::Close { watch }),
            "native acknowledgement finishes closing the abandoned watch"
        );
        let panel = w::ChangePanel::new(&w::CEILINGS, panel_parts(w::Step::Gating)).expect("old panel");
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
        assert!(root.forge().panel().is_none(), "late delivery never reattaches to a different page");
    }
}
