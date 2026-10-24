//! Mockup documents, answered through the same windows as the engine.
use crate::driver::{answer, deliver, limits, step};
use crate::samples::{Then, World};
use World::{FirstRun, Reference, Solo};
use jig_client::contract as c;
use skein_lib::{Duration, List, Queue, Token, Wall};
use temper_client_domain::{Address, Domain, Event, Request};
const FORGE: u16 = temper_client_protocol_wire::FORGE;
const PROJECT: u32 = 1;
const SEED: u64 = 11;
const CHAT_LABEL: u32 = 1;
const CHAT_FLAGS: c::TaskFlags =
    c::TaskFlags { ended_by: c::EndedBy::Result, conversation: true, listed: false, retention: c::Retention::Kept };
pub(super) const PROPOSAL: c::ProposalKey = c::ProposalKey { proposer: c::Proposer::Task(10), number: 1 };
const JSON_TITLE: &str = "JSON output for tally report";
const JSON_WHY: &str = "It changes report’s output and its tests, and lands into main: more than a chat does alone.";
const QUESTION: &str =
    "The old parser accepts a trailing comma as an empty last field. RFC 4180 does not. Keep it, or drop it in 0.5?";
fn person(w: World) -> &'static str {
    match w {
        Reference | World::Bare => "Pat",
        Solo => "Robin",
        FirstRun { .. } => "Sam",
    }
}
fn project(w: World) -> &'static str {
    match w {
        Reference => "factory",
        Solo => "tally",
        World::Bare => "gallery",
        FirstRun { .. } => "Default",
    }
}
fn doc(s: &str) -> c::DocumentText {
    c::DocumentText::complete(text(s))
}
fn matches_demo(w: World) -> bool {
    match w {
        FirstRun { .. } => true,
        Reference | Solo | World::Bare => false,
    }
}
fn model(w: World) -> c::LlmChoice {
    c::LlmChoice {
        account: Some(1),
        main: c::ModelEffort {
            model: text(if matches_demo(w) { "Scripted model" } else { "Opus" }),
            effort: match w {
                Reference => Some(text("xhigh")),
                Solo | World::Bare => Some(text("high")),
                FirstRun { .. } => None,
            },
        },
        subagents: List::with_capacity(0),
    }
}
fn frame(w: World, connecting: bool) -> c::Me {
    let mut me = base_frame(person(w), project(w));
    let member = me.projects.get_mut(0).expect("one project");
    let template = member.templates.get_mut(0).expect("chat template");
    template.llm = match w {
        FirstRun { .. } => None,
        Reference | Solo | World::Bare => Some(model(w)),
    };
    member.models = List::with_capacity(1);
    let choice = model(w);
    let mut efforts = List::with_capacity(1);
    if let Some(effort) = choice.main.effort {
        efforts.push(effort).expect("effort");
    }
    member
        .models
        .push(c::MeModel {
            name: choice.main.model,
            provider: Some(17),
            description: None,
            prices: c::ModelPrices { input: 0, cached: 0, cache_write: None, output: 0, unit: 1 },
            window: 200_000,
            output: 16_384,
            efforts,
        })
        .expect("model");
    me.providers = providers(w);
    if connecting {
        for index in 0..me.providers.len() {
            let provider = me.providers.get_mut(index).expect("provider index");
            if provider.name.bytes() == b"OpenAI" {
                provider.name = doc("openai");
                provider.sign_in = true;
            } else if provider.name.bytes() == b"Anthropic" {
                provider.name = doc("anthropic");
            }
        }
    }
    me.needs = needs(w);
    me.needs_total = if w == Solo { 2 } else { 0 };
    me.updates = c::Count { value: if w == Solo { 2 } else { u32::from(w == Reference) }, limited: false };
    me.deployment_role = Some(c::HeldRole::Owner);
    me
}
fn providers(w: World) -> List<c::ProviderOffer> {
    let mut rows = List::with_capacity(2);
    let names: &[(u32, &str)] = match w {
        Reference => &[(17, "Anthropic"), (18, "OpenAI")],
        Solo => &[(17, "Anthropic")],
        World::Bare => &[(18, "OpenAI")],
        FirstRun { .. } => &[(17, "Playground"), (18, "OpenAI")],
    };
    for (number, name) in names {
        let mut programs = List::with_capacity(1);
        if *name == "Anthropic" {
            programs.push(text("Claude Code")).expect("program");
        }
        rows.push(c::ProviderOffer {
            provider: *number,
            name: doc(name),
            key_bytes: 4096,
            sign_in: *name == "OpenAI" && w == Reference,
            key: true,
            programs,
            programs_cut: false,
            none: false,
            signed_in_plan: *name != "Playground",
            manage_usage: None,
        })
        .expect("provider");
    }
    rows
}
fn needs(w: World) -> c::Needs {
    c::Needs { proposals: u32::from(w == Solo), questions: u32::from(w == Solo), escalations: 0, person_tasks: 0 }
}
pub(super) fn world(w: World, address: Address, then: Then) -> Domain {
    let connecting = matches!(
        address,
        Address::ConnectModel { .. } | Address::Welcome { step: temper_client_domain::Step::Model { .. } }
    );
    let limits = limits();
    let mut root = temper_client_domain::unlinked(&limits, SEED);
    let mut out = Queue::with_capacity(temper_client_domain::max_out(&limits));
    step(&mut root, Event::Start { address, kept: List::with_capacity(0), with: None }, &mut out);
    for event in [
        jig_client::Event::StartUnlinked { kept: List::with_capacity(0) },
        jig_client::Event::SignIn { with: jig_client::With::Transport },
        jig_client::Event::Up { up: jig_client::Up::Served { served: true, versions: None } },
    ] {
        step(&mut root, Event::Client { event }, &mut out);
    }
    let mut forge_watch = None;
    // Every response is to an actual open/read, including the demo family read.
    for _ in 0_u32..256 {
        let Some(request) = out.pop() else { break };
        if let Some(down) = crate::driver::down(request) {
            match down {
                jig_client::Down::Open { watch, subject: jig_client::Watchable::Jig(subject) } => {
                    let document = match subject {
                        c::Subject::Me => Some(c::Delivery::Me(Box::new(frame(w, connecting)))),
                        c::Subject::Accounts => Some(c::Delivery::OwnerAccounts(Box::new(accounts(w, then)))),
                        c::Subject::Tasks { window, .. } => Some(c::Delivery::Tasks(Box::new(task_window(w, window)))),
                        c::Subject::Inbox { window } => {
                            Some(c::Delivery::OwnerInbox(Box::new(inbox_window(w, window))))
                        }
                        c::Subject::Task { task: number } => {
                            Some(c::Delivery::Task(Box::new(shown_task(w, number, then))))
                        }
                        c::Subject::Tree { task, window } => Some(plan_tree(w, task, window, then)),
                        c::Subject::Deployment => Some(c::Delivery::OwnerDeployment(Box::new(deployment(w)))),
                        c::Subject::Settings | c::Subject::Chips { .. } | c::Subject::Run { .. } => None,
                    };
                    if let Some(document) = document {
                        deliver(&mut root, watch, document, &mut out);
                    }
                }
                jig_client::Down::Read { read, what: jig_client::Readable::Jig(what) } => {
                    if let Some(value) = read_value(w, &what, then) {
                        answer(&mut root, read, value, &mut out);
                    }
                }
                jig_client::Down::Read { read, what: jig_client::Readable::Family { .. } } => {
                    demo(&mut root, w, read, &mut out);
                }
                jig_client::Down::Open { watch, subject: jig_client::Watchable::Family { record, .. } }
                    if record.family == FORGE =>
                {
                    use temper_client_protocol_wire::{self as wire, v1 as w};
                    let watching = wire::decode_watching(
                        &record.bytes,
                        &temper_client_domain::forge::wire_limits(&limits.forge).expect("wire limits"),
                    )
                    .expect("gallery forge watch");
                    let subject = match watching.watch() {
                        w::Watch::Forges => {
                            forge_watch = Some(watch);
                            forges_watched(&mut root, w, watch, &mut out);
                            continue;
                        }
                        w::Watch::Change(subject) => subject,
                    };
                    if let Some(panel) = change_panel(w, subject.task()) {
                        for up in [
                            jig_client::Up::Watched { watch, event: c::WatchEvent::Opened },
                            jig_client::Up::FamilyDelivered {
                                watch,
                                record: jig_client::Family {
                                    family: wire::FORGE,
                                    version: wire::CLIENT_VERSION,
                                    bytes: wire::change_panel(&panel),
                                },
                            },
                        ] {
                            step(&mut root, Event::Client { event: jig_client::Event::Up { up } }, &mut out);
                        }
                    }
                }
                jig_client::Down::Save { .. }
                | jig_client::Down::Erase { .. }
                | jig_client::Down::Send { .. }
                | jig_client::Down::Open { .. }
                | jig_client::Down::Continue { .. }
                | jig_client::Down::Close { .. } => {}
            }
        }
    }
    assert!(out.is_empty(), "world fixture settles");
    if w == (FirstRun { step: 8 }) {
        adopt_owned_sample(&mut root, forge_watch.expect("owned forge watch"), &mut out);
    }
    if matches!(w, FirstRun { step: 1..=4 }) {
        assert!(root.demo().expect("answered demo").1.model);
    }
    root
}
fn forges_watched(root: &mut Domain, world: World, watch: Token, out: &mut Queue<Request>) {
    use temper_client_protocol_wire as wire;
    let record = jig_client::Family { family: wire::FORGE, version: wire::CLIENT_VERSION, bytes: forges(world) };
    step(
        root,
        Event::Client { event: jig_client::Event::Up { up: jig_client::Up::FamilyDelivered { watch, record } } },
        out,
    );
}
#[expect(clippy::too_many_lines, reason = "complete illustrative forge rows keep their additive fields together")]
fn forges(world: World) -> Box<[u8]> {
    use temper_client_protocol_wire::{self as wire, v1 as w};
    let limits = temper_client_domain::forge::wire_limits(&limits().forge).expect("wire limits");
    let mut rows = List::with_capacity(3);
    if matches_demo(world) {
        rows.push(
            w::ForgeRow::new(
                &limits,
                w::ForgeRowParts {
                    number: FORGE,
                    kind: w::Kind::Forgejo,
                    mode: w::Mode::Fake,
                    connection: w::Connection::Connected(
                        w::Connected::new(&limits, w::ConnectedParts { id: 1, login: b"temper".as_slice().into() })
                            .expect("demo login"),
                    ),
                    offer: w::Offer::NotOffered,
                    first: None,
                    installations: List::with_capacity(0),
                    ended: None,
                    app: None,
                    web: None,
                    repositories: List::with_capacity(0),
                    owned: None,
                    admin: false,
                    ended_stop: None,
                },
            )
            .expect("demo forge"),
        )
        .expect("one forge");
    }
    if let FirstRun { step } = world {
        rows.push(
            w::ForgeRow::new(
                &limits,
                w::ForgeRowParts {
                    number: 2,
                    kind: w::Kind::Github,
                    mode: w::Mode::Managed,
                    connection: w::Connection::NotConnected,
                    offer: w::Offer::NotOffered,
                    first: None,
                    installations: List::with_capacity(0),
                    ended: None,
                    app: None,
                    web: None,
                    repositories: List::with_capacity(0),
                    owned: None,
                    admin: false,
                    ended_stop: None,
                },
            )
            .expect("GitHub choice"),
        )
        .expect("forge row");
        let connection = match step {
            9 => w::Connection::SetUpFailed(
                w::SetUpFailed::new(
                    &limits,
                    w::SetUpFailedParts {
                        why: "its port, 127.0.0.1:8368, is taken by another program. temper box’s output has the rest."
                            .as_bytes()
                            .into(),
                        at: Some(w::FailedStep::Prepare),
                    },
                )
                .expect("failed preparation"),
            ),
            6 => w::Connection::SettingUp(
                w::SettingUp::new(&limits, w::SettingUpParts { step: Some(w::SetUpStep::Prepare) }).expect("preparing"),
            ),
            7 => w::Connection::Connected(
                w::Connected::new(&limits, w::ConnectedParts { id: 2, login: b"temper".as_slice().into() })
                    .expect("connected"),
            ),
            _ => w::Connection::NotSetUp,
        };
        rows.push(
            w::ForgeRow::new(
                &limits,
                w::ForgeRowParts {
                    number: 3,
                    kind: w::Kind::Forgejo,
                    mode: w::Mode::Owned,
                    connection,
                    offer: w::Offer::Offered,
                    first: if step == 6 || step == 7 || step == 9 {
                        Some(
                            w::FirstRepository::new(
                                &limits,
                                w::FirstRepositoryParts {
                                    org: b"work".as_slice().into(),
                                    repository: b"hello".as_slice().into(),
                                    project: w::SetUpProject::Existing(
                                        w::ExistingProject::new(&limits, w::ExistingProjectParts { project: 1 })
                                            .expect("project"),
                                    ),
                                    state: if step == 7 {
                                        w::FirstState::Made(
                                            w::Made::new(&limits, w::MadeParts { id: 42 }).expect("made"),
                                        )
                                    } else {
                                        w::FirstState::Making
                                    },
                                    label: b"temper-host".as_slice().into(),
                                },
                            )
                            .expect("first repository"),
                        )
                    } else {
                        None
                    },
                    installations: List::with_capacity(0),
                    ended: None,
                    app: None,
                    web: None,
                    repositories: List::with_capacity(0),
                    owned: None,
                    admin: false,
                    ended_stop: None,
                },
            )
            .expect("owned Forgejo"),
        )
        .expect("forge row");
    }
    wire::forges(&w::Forges::new(&limits, w::ForgesParts { forges: rows }).expect("gallery forges"))
}
fn change_panel(w: World, task: u64) -> Option<temper_client_protocol_wire::v1::ChangePanel> {
    use temper_client_protocol_wire::v1 as w;
    let l = temper_client_domain::forge::wire_limits(&limits().forge).expect("wire limits");
    let (branch, head_prefix, pull_number, pull_address, step) = match (w, task) {
        (Reference, 28) => {
            let mut ahead = List::with_capacity(1);
            ahead.push(26).expect("task ahead");
            (
                b"temper/28/build-path-one-cache-per-lane".as_slice(),
                [0xc4, 0x0e, 0x9b, 0x20],
                216,
                b"https://github.com/acme/kit/pull/216".as_slice(),
                w::Step::Queued(w::Queued::new(&l, w::QueuedParts { place: 2, ahead }).expect("queued")),
            )
        }
        (Reference, 24) => {
            let mut merge = [0; 32];
            merge[..4].copy_from_slice(&[0x4e, 0x1a, 0x9c, 0x70]);
            (
                b"temper/24/fake-server-replay-fixtures".as_slice(),
                [0x8b, 0x2d, 0x41, 0x00],
                212,
                b"https://github.com/acme/kit/pull/212".as_slice(),
                w::Step::Landed(w::Landed::new(&l, w::LandedParts { merge }).expect("landed")),
            )
        }
        _ => return None,
    };
    let mut head = [0; 32];
    head[..4].copy_from_slice(&head_prefix);
    Some(
        w::ChangePanel::new(
            &l,
            w::ChangePanelParts {
                task,
                forge: 1,
                owner: Box::from(b"acme".as_slice()),
                name: Box::from(b"kit".as_slice()),
                branch: Box::from(branch),
                landing: Box::from(b"main".as_slice()),
                head: Some(head),
                pull: Some(
                    w::Pull::new(&l, w::PullParts { number: pull_number, address: Box::from(pull_address) })
                        .expect("pull"),
                ),
                step,
                hold: None,
                runs: None,
                ci: w::Ci::Passed,
                gates: List::with_capacity(0),
                repairs: w::Used::new(&l, w::UsedParts { used: 0, limit: 3 }).expect("repairs"),
                resolutions: w::Used::new(&l, w::UsedParts { used: 0, limit: 3 }).expect("resolutions"),
                updates: w::Used::new(&l, w::UsedParts { used: 0, limit: 3 }).expect("updates"),
            },
        )
        .expect("change panel"),
    )
}
fn demo(root: &mut Domain, w: World, read: Token, out: &mut Queue<Request>) {
    use temper_client_protocol_wire::{self as wire, v1};
    let record = if matches_demo(w) {
        let mut cues = List::with_capacity(3);
        for cue in ["Add CSV export to widget", "Add a dark theme to widget", "Fix widget’s empty search state"] {
            cues.push(Box::from(cue.as_bytes())).expect("cue");
        }
        let value = v1::DemoAnswer::new(
            &v1::CEILINGS,
            v1::DemoAnswerParts {
                state: if matches!(w, FirstRun { step: 0 }) {
                    v1::DemoState::NotStarted
                } else {
                    v1::DemoState::Running
                },
                project: PROJECT,
                forge: 1,
                provider: 17,
                owner: Box::from(b"playground".as_slice()),
                name: Box::from(b"widget".as_slice()),
                cues,
            },
        )
        .expect("checked demo");
        Some(wire::demo_answer(&value))
    } else {
        None
    };
    step(
        root,
        Event::Client {
            event: jig_client::Event::Up {
                up: jig_client::Up::ReadDone {
                    read,
                    result: jig_client::Readout::Family { family: wire::FORGE, record },
                },
            },
        },
        out,
    );
}
fn owner(chip: &mut c::Chip) -> (&mut c::OwnerChipPhase, &mut c::OwnerChipDetails) {
    match chip.details.as_mut() {
        c::ChipPhase::Owner { phase, details } => (phase, details),
        c::ChipPhase::Active(_)
        | c::ChipPhase::Waiting { .. }
        | c::ChipPhase::Idle { .. }
        | c::ChipPhase::Starting
        | c::ChipPhase::Running { .. }
        | c::ChipPhase::WaitingForWords { .. }
        | c::ChipPhase::BackingOff { .. }
        | c::ChipPhase::Procedure { .. }
        | c::ChipPhase::Person { .. }
        | c::ChipPhase::Closing(_)
        | c::ChipPhase::Held { .. }
        | c::ChipPhase::Ended { .. } => unreachable!("gallery's owner chip"),
    }
}
fn finish_document(mut t: c::Task) -> c::Task {
    let compact = t.chip.phase;
    let executor = t.chip.executor;
    let (phase, details) = owner(&mut t.chip);
    *phase = match compact {
        c::Phase::Ended | c::Phase::Done => c::OwnerChipPhase::Ended,
        c::Phase::Running => c::OwnerChipPhase::Active(c::RunStage::Running { attempt: 1 }),
        c::Phase::Waiting
        | c::Phase::Idle
        | c::Phase::Due
        | c::Phase::Preparing
        | c::Phase::Claimed
        | c::Phase::BackingOff
        | c::Phase::Active
        | c::Phase::Closing
        | c::Phase::Held
        | c::Phase::Failed
        | c::Phase::Cancelled => c::OwnerChipPhase::Active(c::RunStage::Idle),
    };
    details.executor = match executor {
        c::Executor::Agent { charter } => c::OwnerExecutor::Agent { charter },
        c::Executor::Procedure { connector, kind } => c::OwnerExecutor::Procedure { connector, code: kind },
        c::Executor::Party { .. } | c::Executor::Role { .. } => {
            unreachable!("worlds have agents and change procedures")
        }
    };
    details.wake = t.wake;
    details.budget = t.budget.clone();
    details.activity = t.activity;
    details.live_delegates = t.delegates.len();
    if compact == c::Phase::Ended || compact == c::Phase::Done {
        details.ended_at = Some(t.last_activity);
    }
    match &t.result {
        Some(c::TaskResult::Ending(ending)) => {
            details.ending = Some(*ending.clone());
            details.ended_at = Some(t.last_activity);
        }
        Some(c::TaskResult::Document(_)) | None => {}
    }
    t
}
fn task_line(t: c::Task) -> c::TaskLine {
    c::TaskLine {
        ended_at: if t.chip.phase == c::Phase::Ended || t.chip.phase == c::Phase::Done {
            Some(t.last_activity)
        } else {
            None
        },
        chip: t.chip,
        project: t.project,
        requester: t.requester,
        tracked: t.priority,
        label: t.label,
        flags: t.flags,
        changed: t.changed,
        last_activity: t.last_activity,
        budget: t.budget,
        waits_on: t.waits_on,
        phases: c::OwnerPhaseCounts::ZERO,
        held_below: 0,
        conversation: None,
    }
}
fn chat_rows(w: World, ended: bool) -> List<c::TaskLine> {
    let numbers: &[u64] = match (w, ended) {
        (Reference, false) | (Solo, true) => &[1],
        (Reference, true) => &[3, 2],
        (Solo, false) => &[6, 10],
        (FirstRun { .. } | World::Bare, _) => &[],
    };
    let mut rows = List::with_capacity(u32::try_from(numbers.len()).expect("small chats"));
    for number in numbers {
        rows.push(task_line(document(w, *number))).expect("row");
    }
    rows
}
fn inbox_document(w: World) -> c::Inbox {
    let keys: &[(c::ObjectKey, c::InboxGroup)] = match w {
        Reference => &[(c::ObjectKey::Task(1), c::InboxGroup::Replies)],
        Solo => &[
            (c::ObjectKey::Proposal(PROPOSAL), c::InboxGroup::Proposals),
            (c::ObjectKey::Question { task: 3, message: 1 }, c::InboxGroup::Questions),
            (c::ObjectKey::Task(10), c::InboxGroup::Replies),
            (c::ObjectKey::Task(6), c::InboxGroup::Replies),
        ],
        FirstRun { .. } | World::Bare => &[],
    };
    let mut entries = List::with_capacity(u32::try_from(keys.len()).expect("small inbox"));
    for (object, group) in keys {
        entries
            .push(c::InboxEntry {
                reply: match object {
                    c::ObjectKey::Task(task) => Some(Box::new(reply_line(w, *task))),
                    c::ObjectKey::Proposal(_)
                    | c::ObjectKey::Escalation(_)
                    | c::ObjectKey::Question { .. }
                    | c::ObjectKey::PersonTask(_)
                    | c::ObjectKey::Result(_)
                    | c::ObjectKey::Note { .. } => None,
                },
                lineage: List::with_capacity(0),
                goal: None,
                recipient: None,
                stall: None,
                object: *object,
                project: PROJECT,
                group: *group,
                position: 1,
                at: match object {
                    c::ObjectKey::Proposal(_) => offset(w, -12),
                    c::ObjectKey::Question { .. } => offset(w, -18),
                    c::ObjectKey::Task(task) => document(w, *task).last_activity,
                    c::ObjectKey::Escalation(_)
                    | c::ObjectKey::PersonTask(_)
                    | c::ObjectKey::Result(_)
                    | c::ObjectKey::Note { .. } => unreachable!("world inbox has proposals, questions and replies"),
                },
            })
            .expect("entry");
    }
    c::Inbox {
        entries,
        needs: needs(w),
        updates: c::Count { value: if w == Solo { 2 } else { u32::from(w == Reference) }, limited: false },
        results_seen: 0,
        replies_seen: List::with_capacity(0),
        more: false,
    }
}
fn proposed() -> List<c::ProposedTask> {
    let mut base = base_task(0, JSON_TITLE);
    base.authority.authority.spend = 6;
    // Both distinct written fields fit the proposed task's specification.
    base.spec.specification.words = doc(
        "JSON output for tally report\n\nA goal of yours: `tally report --json`, one object per row, with tests, landed into main.",
    );
    let mut resources = List::with_capacity(1);
    resources
        .push(c::OwnerPart {
            connector: FORGE,
            kind: c::OwnerPartKind::Resource,
            key: c::OwnerPartKey::Path(path(&[b"forge", b"github.com", b"robin", b"tally", b"branch", b"main"])),
        })
        .expect("landing branch");
    let mut tasks = List::with_capacity(1);
    tasks
        .push(c::ProposedTask {
            number: 1,
            spec: base.spec,
            executor: c::OwnerExecutor::Agent { charter: 2 },
            contract: base.result_contract,
            authority: base.authority,
            deadline: c::NewDeadline::Named,
            budget: 6,
            funder: c::Funder::Pool { project: PROJECT, person: 73, period: 1 },
            dependencies: List::with_capacity(0),
            resources,
            wake: base.wake,
            priority: Some(2),
            cut: false,
        })
        .expect("proposed goal");
    tasks
}
fn proposal() -> c::ReadValue {
    c::ReadValue::Proposal(Box::new(c::Proposal {
        key: PROPOSAL,
        project: PROJECT,
        proposer: PROPOSAL.proposer,
        why: text(JSON_WHY),
        action: c::ProposalAction::Batch { tasks: proposed(), own: true },
        needs: c::Authority {
            models: None,
            budget: 6,
            grants: List::with_capacity(0),
            task_kinds: List::with_capacity(0),
            depth: 1,
            tasks: 1,
            deadline: None,
            tools: List::with_capacity(0),
            note_scopes: List::with_capacity(0),
        },
        funding: c::Budget { total: 50, own: 10, ended: 0, reserved: 0, left: 40, funder: c::Actor::Party(73) },
        waiting: c::Recipient::Party(73),
        trail: List::with_capacity(0),
        since: offset(Solo, -12),
        acts: offers(&[c::ActKind::Accept, c::ActKind::Reject]),
        ending: None,
    }))
}
fn read_value(w: World, what: &c::Read, then: Then) -> Option<c::ReadValue> {
    match what {
        c::Read::Tasks { query: c::TaskQuery { parent: Some(parent), phase, .. }, .. }
            if *phase == Some(c::PhaseClass::Ended) =>
        {
            let children = facts(w, *parent).children;
            let mut rows = List::with_capacity(u32::try_from(children.len()).expect("small children"));
            for child in children {
                let task = document(w, *child);
                if matches!(task.chip.phase, c::Phase::Done | c::Phase::Ended) {
                    rows.push(task_line(task)).expect("ended child");
                }
            }
            Some(c::ReadValue::Tasks(Box::new(c::TaskPage {
                live: List::with_capacity(0),
                live_after: None,
                ended: c::EndedGroup::Rows { rows, after: None, cut: false },
            })))
        }
        c::Read::Tasks { .. } => Some(c::ReadValue::Tasks(Box::new(c::TaskPage {
            live: List::with_capacity(0),
            live_after: None,
            ended: c::EndedGroup::Rows { rows: ended_rows(w), after: None, cut: false },
        }))),
        c::Read::Task { task: number } => Some(c::ReadValue::Task(Box::new(shown_task(w, *number, then)))),
        c::Read::Activity { task: number, before } => {
            Some(c::ReadValue::Activity(Box::new(activity_window(w, *number, before.as_ref()))))
        }
        c::Read::Proposal { .. } => Some(proposal()),
        c::Read::Question { .. } => Some(c::ReadValue::Question(Box::new(c::Question {
            task: document(Solo, 3).chip,
            from: Some(c::OwnerRequester::Task(3)),
            message: 1,
            words: text(QUESTION),
            since: offset(Solo, -18),
            acts: offers(&[c::ActKind::Answer]),
            ending: None,
        }))),
        c::Read::Settings
        | c::Read::Setting(_)
        | c::Read::Revision(_)
        | c::Read::Revisions(_)
        | c::Read::Me
        | c::Read::Turn { .. }
        | c::Read::Escalation { .. }
        | c::Read::PersonTask { .. }
        | c::Read::Result { .. }
        | c::Read::Ended { .. }
        | c::Read::History { .. }
        | c::Read::Inbox { .. }
        | c::Read::Spend { .. }
        | c::Read::Project { .. }
        | c::Read::Notes { .. }
        | c::Read::Deployment => None,
    }
}
fn agent_line(words: &str, number: u32, read: Option<u64>) -> c::Activity {
    let mut blocks = List::with_capacity(1);
    let bytes = words.as_bytes();
    let limit = usize::try_from(limits().client.domain.text).expect("text limit");
    let length = words.floor_char_boundary(bytes.len().min(limit));
    let text = c::Text::new(Box::from(&bytes[..length]), limits().client.domain.text).expect("received turn text");
    blocks.push(c::Block::Text(text)).expect("agent text");
    turn_line(&blocks, number, read, length < bytes.len())
}
fn turn_line(source: &List<c::Block>, number: u32, read: Option<u64>, mut cut: bool) -> c::Activity {
    let mut blocks = List::with_capacity(limits().client.domain.rows.min(source.len()));
    for block in source {
        if blocks.len() == blocks.capacity() {
            cut = true;
            break;
        }
        blocks.push(block.clone()).expect("receiving block window");
    }
    let mut messages = List::with_capacity(1);
    messages.push(c::Message { speaker: c::Speaker::Assistant, blocks }).expect("message");
    c::Activity::Turn {
        name: c::TurnName { attempt: 1, number },
        turn: c::TurnRead::Decoded(c::Turn {
            messages,
            used: c::Usage { input: 0, output: 0, cached: 0, cache_written: 0 },
            spent: 0,
            read_through: read,
            cut,
        }),
    }
}
fn tools_line(calls: &[(&str, u32)], made: &[u64], proposes: bool, number: u32) -> c::Activity {
    let mut blocks = List::with_capacity(32);
    let mut call_number: u64 = 0;
    for (name, count) in calls {
        for _ in 0..*count {
            call_number = call_number.checked_add(1).expect("small calls");
            let mut objects = List::with_capacity(4);
            if *name == "delegate" {
                for task in made {
                    objects.push(c::ObjectKey::Task(*task)).expect("delegate object");
                }
            }
            if *name == "propose" && proposes {
                objects.push(c::ObjectKey::Proposal(PROPOSAL)).expect("proposal object");
            }
            blocks
                .push(c::Block::Call(c::Call {
                    number: call_number,
                    tool: text(name),
                    input: text(""),
                    outcome: None,
                    failed: false,
                    objects,
                    parts: List::with_capacity(0),
                }))
                .expect("tool call");
        }
    }
    turn_line(&blocks, number, None, false)
}
// BEGIN WORLD DATA
struct TaskFacts {
    title: &'static str,
    brief: &'static str,
    charter: u32,
    parent: Option<u64>,
    phase: c::Phase,
    wait: u8,
    spent_tenths: u64,
    budget: u64,
    priority: Option<u32>,
    change: bool,
    result: Option<&'static str>,
    merge: bool,
    model: &'static str,
    effort: Option<&'static str>,
    account: u32,
    children: &'static [u64],
    proposal: bool,
    activity_at: u64,
}
fn chip(w: World, number: u64) -> c::Chip {
    let f = facts(w, number);
    let mut t = base_task(number, f.title);
    t.chip.phase = f.phase;
    t.chip.executor = if f.change {
        c::Executor::Procedure { connector: FORGE, kind: 1 }
    } else {
        c::Executor::Agent { charter: f.charter }
    };
    t.budget = budget(&f);
    finish_document(t).chip
}
fn budget(f: &TaskFacts) -> c::OwnerBudget {
    let spent = round_mockup_spend(f.spent_tenths);
    c::OwnerBudget {
        numbers: c::OwnerNumbers { budget: f.budget, spent, spent_below: 0, reserved: 0 },
        left: f.budget.saturating_sub(spent),
        funder: match f.parent {
            Some(parent) => c::Funder::Task(parent),
            None => c::Funder::Pool { project: PROJECT, person: 73, period: 1 },
        },
        deadline: None,
    }
}
#[expect(clippy::manual_map, reason = "fixture follows the step crates’ explicit Option matches")]
fn document(w: World, number: u64) -> c::Task {
    let f = facts(w, number);
    let mut t = base_task(number, f.title);
    t.chip = chip(w, number);
    t.priority = f.priority;
    if f.priority.is_some() {
        t.goal = Some(number);
        t.flags.conversation = false;
        t.flags.listed = true;
        // A goal offers amend, stop and cancel (client README 3.5, the Task row).
        t.acts = offers(&[c::ActKind::Amend, c::ActKind::Stop, c::ActKind::Cancel]);
    }
    t.kept_account = if matches_demo(w) || f.change { None } else { Some(f.account) };
    t.last_activity = Wall::from_nanos(f.activity_at);
    t.changed = t.last_activity;
    t.requester = match f.parent {
        Some(parent) => c::OwnerRequester::Task(parent),
        None => c::OwnerRequester::Person(73),
    };
    t.spec.specification.words = doc(f.brief);
    // Mockup amounts have decimals; the view prints integer deployment units.
    // Round to the nearest unit, and keep the missing below-ledger at zero.
    t.budget = budget(&f);
    t.llm = if f.change || matches_demo(w) {
        None
    } else {
        Some(c::LlmChoice {
            account: Some(f.account),
            main: c::ModelEffort {
                model: text(f.model),
                effort: match f.effort {
                    Some(e) => Some(text(e)),
                    None => None,
                },
            },
            subagents: List::with_capacity(0),
        })
    };
    t.waits_on = match f.wait {
        1 => c::WaitsOn::Delegates,
        2 => c::WaitsOn::News,
        3 => c::WaitsOn::Party {
            whom: c::Whom::Person(73),
            entries: 1,
            task: number,
            kind: if number == 10 { c::WaitedFor::Proposal } else { c::WaitedFor::Question },
        },
        _ => c::WaitsOn::Nothing,
    };
    if f.change {
        t.result_contract = c::TaskContractDocument::Change { connector: FORGE, kind: 1, words: 4096 };
    }
    if let Some(result) = f.result {
        let value = if f.merge {
            c::ResultValue::Change {
                kind: 1,
                part: c::OwnerPart {
                    connector: FORGE,
                    kind: c::OwnerPartKind::Result,
                    key: c::OwnerPartKey::Number(number),
                },
                words: doc(result),
            }
        } else {
            c::ResultValue::Report(doc(result))
        };
        t.result = Some(c::TaskResult::Ending(Box::new(c::OwnerChipEnding::Done(value))));
    }
    if f.phase == c::Phase::Done || f.phase == c::Phase::Ended {
        t.acts.clear();
    }
    t.delegates = List::with_capacity(16);
    for child in f.children {
        let child_facts = facts(w, *child);
        if child_facts.phase != c::Phase::Done && child_facts.phase != c::Phase::Ended {
            t.delegates.push(c::LiveDelegate { chip: chip(w, *child), takeover_budget: 0 }).expect("live delegate");
        }
    }
    if f.proposal {
        t.proposals = List::with_capacity(1);
        t.proposals.push(PROPOSAL).expect("proposal");
    }
    t.activity = activity(w, number).rows.len().into();
    finish_document(t)
}
// Plausible gallery figures, not the engine's rule: a person's "as allowed"
// task gets min(pool left, its role's ceiling) less the same batch's named
// amounts; a delegate gets its parent's remainder after its spend, the settled
// spend below, reservations and named siblings.
#[expect(clippy::too_many_lines, reason = "literal task facts copied from the three mockup worlds")]
fn facts(w: World, number: u64) -> TaskFacts {
    match (w, number) {
        (Reference, 1) => TaskFacts {
            title: "Director",
            brief: "Morning. I'm landing Export's draft within the hour.",
            charter: 1,
            parent: None,
            phase: c::Phase::Idle,
            wait: 1,
            spent_tenths: 62,
            budget: 100,
            priority: None,
            change: false,
            result: None,
            merge: false,
            model: "Opus",
            effort: Some("xhigh"),
            account: 1,
            children: &[12, 7, 8],
            proposal: false,
            activity_at: 1_792_653_000_000_000_000,
        },
        (Reference, 7) => TaskFacts {
            title: "Owners' status digest",
            brief: "Every two hours, read each owner's ledger and write the director a digest.",
            charter: 7,
            parent: Some(1),
            phase: c::Phase::Idle,
            wait: 2,
            spent_tenths: 9,
            budget: 94,
            priority: None,
            change: false,
            result: None,
            merge: false,
            model: "GPT",
            effort: Some("medium"),
            account: 2,
            children: &[],
            proposal: false,
            activity_at: 1_792_524_000_000_000_000,
        },
        (Reference, 8) => TaskFacts {
            title: "Completion checker",
            brief: "When an owner reports done, check its draft's items against what landed.",
            charter: 7,
            parent: Some(1),
            phase: c::Phase::Idle,
            wait: 2,
            spent_tenths: 4,
            budget: 94,
            priority: None,
            change: false,
            result: None,
            merge: false,
            model: "GPT",
            effort: Some("medium"),
            account: 2,
            children: &[],
            proposal: false,
            activity_at: 1_792_524_000_000_000_000,
        },
        (Reference, 12) => TaskFacts {
            title: "Harness",
            brief: "Own the draft Harness: kit's fake server, the flaky tests and the build path. Take taste from one-shot Opus advisors; bring significant decisions to the director.",
            charter: 2,
            parent: Some(1),
            phase: c::Phase::Idle,
            wait: 1,
            spent_tenths: 186,
            budget: 60,
            priority: Some(1),
            change: false,
            result: None,
            merge: false,
            model: "GPT",
            effort: Some("high"),
            account: 2,
            children: &[13, 14, 24, 26, 28, 31, 33, 34, 35],
            proposal: false,
            activity_at: 1_792_598_700_000_000_000,
        },
        (Reference, 13) => TaskFacts {
            title: "Advise: the fake server's API",
            brief: "Read the draft's fake server section and the three candidate shapes; recommend one, with its reasons.",
            charter: 3,
            parent: Some(12),
            phase: c::Phase::Done,
            wait: 0,
            spent_tenths: 13,
            budget: 41,
            priority: None,
            change: false,
            result: Some("Shape 2: a scripted server with typed replies; streams as a reply kind, not a mode."),
            merge: false,
            model: "Opus",
            effort: Some("xhigh"),
            account: 1,
            children: &[],
            proposal: false,
            activity_at: 1_792_614_000_000_000_000,
        },
        (Reference, 14) => TaskFacts {
            title: "Survey: kit's flaky tests",
            brief: "Run kit's suite 40 times and list every test that fails without a change, with its signature.",
            charter: 4,
            parent: Some(12),
            phase: c::Phase::Done,
            wait: 0,
            spent_tenths: 8,
            budget: 41,
            priority: None,
            change: false,
            result: Some("One flake: reconnect_after_drop, 3 in 40, timed out waiting for reconnect."),
            merge: false,
            model: "GPT",
            effort: Some("medium"),
            account: 2,
            children: &[],
            proposal: false,
            activity_at: 1_792_648_320_000_000_000,
        },
        (Reference, 24) => TaskFacts {
            title: "Fake server: replay fixtures",
            brief: "",
            charter: 1,
            parent: Some(12),
            phase: c::Phase::Done,
            wait: 0,
            spent_tenths: 0,
            budget: 41,
            priority: None,
            change: true,
            result: Some("4e1a9c7"),
            merge: true,
            model: "Opus",
            effort: Some("high"),
            account: 1,
            children: &[25],
            proposal: false,
            activity_at: 1_792_651_680_000_000_000,
        },
        (Reference, 25) => TaskFacts {
            title: "Produce T24's branch",
            brief: "Add replay fixtures to the fake server: record a session and replay it byte for byte, with tests.",
            charter: 5,
            parent: Some(24),
            phase: c::Phase::Done,
            wait: 0,
            spent_tenths: 34,
            budget: 41,
            priority: None,
            change: false,
            result: Some("Pushed temper/24/fake-server-replay-fixtures at 8b2d410."),
            merge: false,
            model: "GPT",
            effort: Some("high"),
            account: 2,
            children: &[],
            proposal: false,
            activity_at: 1_792_647_000_000_000_000,
        },
        (Reference, 26) => TaskFacts {
            title: "Fake server: typed errors",
            brief: "",
            charter: 1,
            parent: Some(12),
            phase: c::Phase::Running,
            wait: 0,
            spent_tenths: 0,
            budget: 41,
            priority: None,
            change: true,
            result: None,
            merge: false,
            model: "Opus",
            effort: Some("high"),
            account: 1,
            children: &[27],
            proposal: false,
            activity_at: 1_792_638_900_000_000_000,
        },
        (Reference, 27) => TaskFacts {
            title: "Produce T26's branch",
            brief: "Give the fake server typed errors in place of strings, with tests for each.",
            charter: 5,
            parent: Some(26),
            phase: c::Phase::Done,
            wait: 0,
            spent_tenths: 29,
            budget: 41,
            priority: None,
            change: false,
            result: Some("Pushed temper/26/fake-server-typed-errors at 51ce0a7."),
            merge: false,
            model: "GPT",
            effort: Some("high"),
            account: 2,
            children: &[],
            proposal: false,
            activity_at: 1_792_648_500_000_000_000,
        },
        (Reference, 28) => TaskFacts {
            title: "Build path: one cache per lane",
            brief: "",
            charter: 1,
            parent: Some(12),
            phase: c::Phase::Running,
            wait: 0,
            spent_tenths: 0,
            budget: 41,
            priority: None,
            change: true,
            result: None,
            merge: false,
            model: "Opus",
            effort: Some("high"),
            account: 1,
            children: &[29],
            proposal: false,
            activity_at: 1_792_639_200_000_000_000,
        },
        (Reference, 29) => TaskFacts {
            title: "Produce T28's branch",
            brief: "One build cache per lane in scripts/build, so lanes stop evicting each other.",
            charter: 5,
            parent: Some(28),
            phase: c::Phase::Done,
            wait: 0,
            spent_tenths: 22,
            budget: 41,
            priority: None,
            change: false,
            result: Some("Pushed temper/28/build-path-one-cache-per-lane at c40e9b2."),
            merge: false,
            model: "GPT",
            effort: Some("medium"),
            account: 2,
            children: &[],
            proposal: false,
            activity_at: 1_792_650_300_000_000_000,
        },
        (Reference, 31) => TaskFacts {
            title: "Fix the reconnect race",
            brief: "",
            charter: 1,
            parent: Some(12),
            phase: c::Phase::Running,
            wait: 0,
            spent_tenths: 0,
            budget: 41,
            priority: None,
            change: true,
            result: None,
            merge: false,
            model: "Opus",
            effort: Some("high"),
            account: 1,
            children: &[32],
            proposal: false,
            activity_at: 1_792_648_320_000_000_000,
        },
        (Reference, 32) => TaskFacts {
            title: "Produce T31's branch",
            brief: "Fix reconnect_after_drop's race (finding F): the reconnect timer must not fire before the drop is seen. Prove it with 200 runs.",
            charter: 5,
            parent: Some(31),
            phase: c::Phase::Done,
            wait: 0,
            spent_tenths: 11,
            budget: 41,
            priority: None,
            change: false,
            result: Some("Pushed temper/31/fix-the-reconnect-race at e7a31d5."),
            merge: false,
            model: "GPT",
            effort: Some("medium"),
            account: 2,
            children: &[],
            proposal: false,
            activity_at: 1_792_654_800_000_000_000,
        },
        (Reference, 33) => TaskFacts {
            title: "Benchmark: the fake server under load",
            brief: "Measure replies per second and tail latency at 1, 8 and 64 clients. Needs s1 quiet.",
            charter: 5,
            parent: Some(12),
            phase: c::Phase::Running,
            wait: 0,
            spent_tenths: 16,
            budget: 41,
            priority: None,
            change: false,
            result: None,
            merge: false,
            model: "GPT",
            effort: Some("medium"),
            account: 2,
            children: &[],
            proposal: false,
            activity_at: 1_792_652_640_000_000_000,
        },
        (Reference, 34) => TaskFacts {
            title: "Fake server: streaming replies",
            brief: "Add streamed replies to the fake server as a reply kind (T13), with its tests. Hand back the branch.",
            charter: 5,
            parent: Some(12),
            phase: c::Phase::Running,
            wait: 0,
            spent_tenths: 24,
            budget: 41,
            priority: None,
            change: false,
            result: None,
            merge: false,
            model: "GPT",
            effort: Some("high"),
            account: 2,
            children: &[],
            proposal: false,
            activity_at: 1_792_655_940_000_000_000,
        },
        (Reference, 35) => TaskFacts {
            title: "Investigate: the slow build path",
            brief: "Find why kit's build path takes 9 minutes from clean; name the cause with evidence.",
            charter: 6,
            parent: Some(12),
            phase: c::Phase::Running,
            wait: 0,
            spent_tenths: 9,
            budget: 41,
            priority: None,
            change: false,
            result: None,
            merge: false,
            model: "GPT",
            effort: Some("medium"),
            account: 2,
            children: &[],
            proposal: false,
            activity_at: 1_792_654_440_000_000_000,
        },
        (Reference, 3) => TaskFacts {
            title: "Why did kit’s slow tier fail on Monday?",
            brief: "",
            charter: 1,
            parent: None,
            phase: c::Phase::Ended,
            wait: 0,
            spent_tenths: 10,
            budget: 100,
            priority: None,
            change: false,
            result: None,
            merge: false,
            model: "Opus",
            effort: Some("high"),
            account: 1,
            children: &[],
            proposal: false,
            activity_at: 1_792_422_000_000_000_000,
        },
        (Reference, 2) => TaskFacts {
            title: "The process documents, read aloud",
            brief: "",
            charter: 1,
            parent: None,
            phase: c::Phase::Ended,
            wait: 0,
            spent_tenths: 10,
            budget: 100,
            priority: None,
            change: false,
            result: None,
            merge: false,
            model: "Opus",
            effort: Some("high"),
            account: 1,
            children: &[],
            proposal: false,
            activity_at: 1_792_332_000_000_000_000,
        },
        (Solo, 3) => TaskFacts {
            title: "CSV import",
            brief: "Import CSV with quoted fields (RFC 4180), keeping the old parser’s behaviour where tests pin it.",
            charter: 2,
            parent: None,
            phase: c::Phase::Idle,
            wait: 3,
            spent_tenths: 48,
            budget: 20,
            priority: Some(1),
            change: false,
            result: None,
            merge: false,
            model: "Opus",
            effort: Some("high"),
            account: 1,
            children: &[4, 8],
            proposal: false,
            activity_at: 1_793_003_400_000_000_000,
        },
        (Solo, 4) => TaskFacts {
            title: "CSV import: the quoted-field parser",
            brief: "",
            charter: 1,
            parent: Some(3),
            phase: c::Phase::Running,
            wait: 0,
            spent_tenths: 0,
            budget: 15,
            priority: None,
            change: true,
            result: None,
            merge: false,
            model: "Opus",
            effort: Some("high"),
            account: 1,
            children: &[5],
            proposal: false,
            activity_at: 1_793_004_600_000_000_000,
        },
        (Solo, 5) => TaskFacts {
            title: "Produce T4's branch",
            brief: "Replace the CSV splitter with a parser for quoted fields, escaped quotes and embedded newlines (docs/csv.md), with tests for each.",
            charter: 2,
            parent: Some(4),
            phase: c::Phase::Done,
            wait: 0,
            spent_tenths: 21,
            budget: 15,
            priority: None,
            change: false,
            result: Some("Pushed temper/3/csv-import-the-quoted-field-parser at 3c9e1f0."),
            merge: false,
            model: "Sonnet",
            effort: Some("medium"),
            account: 1,
            children: &[],
            proposal: false,
            activity_at: 1_793_091_000_000_000_000,
        },
        (Solo, 6) => TaskFacts {
            title: "Why is tally sum slow on big files?",
            brief: "Why is tally sum slow on big files?\n2 GB takes 40 s.",
            charter: 1,
            parent: None,
            phase: c::Phase::Idle,
            wait: 2,
            spent_tenths: 8,
            budget: 40,
            priority: None,
            change: false,
            result: None,
            merge: false,
            model: "Opus",
            effort: Some("high"),
            account: 1,
            children: &[7],
            proposal: false,
            activity_at: 1_793_088_120_000_000_000,
        },
        (Solo, 10) => TaskFacts {
            title: "Add a --json flag to tally report",
            brief: "Add a --json flag to tally report\nOne object per row: my dashboard script reads it.",
            charter: 1,
            parent: None,
            phase: c::Phase::Idle,
            wait: 3,
            spent_tenths: 3,
            budget: 40,
            priority: None,
            change: false,
            result: None,
            merge: false,
            model: "Opus",
            effort: Some("high"),
            account: 1,
            children: &[],
            proposal: true,
            activity_at: 1_793_092_680_000_000_000,
        },
        (Solo, 7) => TaskFacts {
            title: "Profile tally sum on a 2 GB file",
            brief: "Profile `tally sum` on a generated 2 GB CSV and name where the time goes.",
            charter: 2,
            parent: Some(6),
            phase: c::Phase::Done,
            wait: 0,
            spent_tenths: 5,
            budget: 39,
            priority: None,
            change: false,
            result: Some(
                "92% of the time is the CSV splitter allocating a string per line. A reused buffer cuts the run from 40 s to 1.8 s.",
            ),
            merge: false,
            model: "Sonnet",
            effort: Some("medium"),
            account: 1,
            children: &[],
            proposal: false,
            activity_at: 1_793_088_000_000_000_000,
        },
        (Solo, 8) => TaskFacts {
            title: "CSV import: errors with line and column",
            brief: "",
            charter: 1,
            parent: Some(3),
            phase: c::Phase::Running,
            wait: 0,
            spent_tenths: 0,
            budget: 15,
            priority: None,
            change: true,
            result: None,
            merge: false,
            model: "Opus",
            effort: Some("high"),
            account: 1,
            children: &[9],
            proposal: false,
            activity_at: 1_793_092_020_000_000_000,
        },
        (Solo, 9) => TaskFacts {
            title: "Produce T8's branch",
            brief: "Give CSV import’s errors the line and column where they occur, counting embedded newlines, with tests.",
            charter: 2,
            parent: Some(8),
            phase: c::Phase::Running,
            wait: 0,
            spent_tenths: 6,
            budget: 15,
            priority: None,
            change: false,
            result: None,
            merge: false,
            model: "Sonnet",
            effort: Some("medium"),
            account: 1,
            children: &[],
            proposal: false,
            activity_at: 1_793_093_340_000_000_000,
        },
        (Solo, 1) => TaskFacts {
            title: "Make tally sum stream its input",
            brief: "",
            charter: 1,
            parent: None,
            phase: c::Phase::Ended,
            wait: 0,
            spent_tenths: 10,
            budget: 40,
            priority: None,
            change: false,
            result: None,
            merge: false,
            model: "Opus",
            effort: Some("high"),
            account: 1,
            children: &[],
            proposal: false,
            activity_at: 1_792_998_000_000_000_000,
        },
        (FirstRun { step: 2 }, 1) => TaskFacts {
            title: "Add CSV export to widget",
            brief: "Add CSV export to widget",
            charter: 1,
            parent: None,
            phase: c::Phase::Idle,
            wait: 1,
            spent_tenths: 0,
            budget: 20,
            priority: None,
            change: false,
            result: None,
            merge: false,
            model: "Scripted model",
            effort: None,
            account: 1,
            children: &[2],
            proposal: false,
            activity_at: 1_792_702_860_000_000_000,
        },
        (FirstRun { step: 2 }, 2) => TaskFacts {
            title: "Export widget's table as CSV",
            brief: "",
            charter: 1,
            parent: Some(1),
            phase: c::Phase::Running,
            wait: 0,
            spent_tenths: 0,
            budget: 20,
            priority: None,
            change: true,
            result: None,
            merge: false,
            model: "Scripted model",
            effort: None,
            account: 1,
            children: &[3],
            proposal: false,
            activity_at: 1_792_702_860_000_000_000,
        },
        (FirstRun { step: 2 }, 3) => TaskFacts {
            title: "Export widget's table as CSV",
            brief: "Export widget's table as CSV",
            charter: 2,
            parent: Some(2),
            phase: c::Phase::Running,
            wait: 0,
            spent_tenths: 0,
            budget: 20,
            priority: None,
            change: false,
            result: None,
            merge: false,
            model: "Scripted model",
            effort: None,
            account: 1,
            children: &[],
            proposal: false,
            activity_at: 1_792_702_860_000_000_000,
        },
        (FirstRun { step: 3 | 4 }, 1) => TaskFacts {
            title: "Add CSV export to widget",
            brief: "Add CSV export to widget",
            charter: 1,
            parent: None,
            phase: c::Phase::Idle,
            wait: 2,
            spent_tenths: 0,
            budget: 20,
            priority: None,
            change: false,
            result: None,
            merge: false,
            model: "Scripted model",
            effort: None,
            account: 1,
            children: &[2],
            proposal: false,
            activity_at: 1_792_703_040_000_000_000,
        },
        (FirstRun { step: 3 | 4 }, 2) => TaskFacts {
            title: "Export widget's table as CSV",
            brief: "",
            charter: 1,
            parent: Some(1),
            phase: c::Phase::Done,
            wait: 0,
            spent_tenths: 0,
            budget: 20,
            priority: None,
            change: true,
            result: Some("e3b8d4192c0f5a7e"),
            merge: true,
            model: "Scripted model",
            effort: None,
            account: 1,
            children: &[3],
            proposal: false,
            activity_at: 1_792_703_040_000_000_000,
        },
        (FirstRun { step: 3 | 4 }, 3) => TaskFacts {
            title: "Export widget's table as CSV",
            brief: "Export widget's table as CSV",
            charter: 2,
            parent: Some(2),
            phase: c::Phase::Done,
            wait: 0,
            spent_tenths: 0,
            budget: 20,
            priority: None,
            change: false,
            result: Some(
                "**Add CSV export to widget**\n\n`widget --csv` prints the table as CSV: a header line, `name,count`, then a line for each row.",
            ),
            merge: false,
            model: "Scripted model",
            effort: None,
            account: 1,
            children: &[],
            proposal: false,
            activity_at: 1_792_702_980_000_000_000,
        },
        (FirstRun { step: 4 }, 4) => TaskFacts {
            title: "Add a dark theme to widget",
            brief: "Add a dark theme to widget",
            charter: 1,
            parent: None,
            phase: c::Phase::Idle,
            wait: 2,
            spent_tenths: 0,
            budget: 20,
            priority: None,
            change: false,
            result: None,
            merge: false,
            model: "Scripted model",
            effort: None,
            account: 1,
            children: &[],
            proposal: false,
            activity_at: 1_792_703_100_000_000_000,
        },
        _ => panic!("task absent from this world step"),
    }
}
fn activity(w: World, task: u64) -> c::ActivityPage {
    let mut events = List::with_capacity(16);
    match (w, task) {
        (Reference, 1) => {
            events
                .push(agent_line("Noted. I'll keep s1's three spare slots free until then.", 2, Some(1)))
                .expect("world activity");
            events
                .push(tools_line(
                    &[("recall", 2), ("read_pull", 3), ("read_items", 1), ("workers", 1), ("accounts", 2)],
                    &[],
                    false,
                    3,
                ))
                .expect("world activity");
            events.push(agent_line("**Check · two-hour timer**\n\n| | |\n|---|---|\n| Harness | T12 on s1 · 3 delegates running (T33 benchmark, T34 streaming, T35 build path) · 3 changes in kit’s queue |\n| Landed | kit: T24 replay fixtures at 06:48, slow tier passed · app: nothing since yesterday |\n| Findings | F, kit’s reconnect flake · B’s · fix f1 (T31) ready, 3rd in kit’s queue |\n| Workers | ws idle · s1 4 of 7 slots, disk 142 GB free (floor 80) |\n| Plans | Claude week 58%, above its pace of 48% · ChatGPT week 22%, under its 28% |\n| Spend | since 05:10: Harness 3.1 · Director 0.4 |\n| Needs you | nothing |\n\nNext check at 09:10.", 4, Some(1))).expect("world activity");
        }
        (Solo, 6) => {
            events
                .push(agent_line("I'll profile it on a generated 2 GB file first.", 2, Some(1)))
                .expect("world activity");
            events.push(tools_line(&[("delegate", 1)], &[7], false, 3)).expect("world activity");
            events.push(agent_line("T7 found it: **92%** of the time is the CSV splitter allocating a string per line.\n\nA reused buffer cuts the run from 40 s to 1.8 s. Shall I make that a change?", 4, Some(1))).expect("world activity");
        }
        (Solo, 9) => {
            events.push(tools_line(&[("read", 4), ("shell", 1)], &[], false, 1)).expect("world activity");
            events.push(agent_line("Errors carry their line now, and an embedded newline moves it on. The column next: counted from the start of its line, quotes included.", 2, None)).expect("world activity");
            events.push(tools_line(&[("edit", 3), ("shell", 2)], &[], false, 3)).expect("world activity");
            // Its last mockup turn is live; static gallery states show committed turns only (browser.md, 7.1).
        }
        (Solo, 10) => {
            events.push(agent_line("That changes `report`'s output and its tests, and lands into main, so I've proposed it as a goal of yours, with a budget of its own.", 2, Some(1))).expect("world activity");
            events.push(tools_line(&[("read", 2), ("propose", 1)], &[], true, 3)).expect("world activity");
        }
        (FirstRun { step: 2 }, 1) => {
            events.push(agent_line("I’ll delegate this change in playground/widget. It will push a branch, open a pull request, check CI and land into main on Playground forge.", 2, Some(1))).expect("world activity");
            events.push(tools_line(&[("delegate", 1)], &[2], false, 3)).expect("world activity");
            events.push(tools_line(&[("wait", 1)], &[], false, 4)).expect("world activity");
        }
        (FirstRun { step: 3 | 4 }, 1) => {
            events.push(agent_line("I’ll delegate this change in playground/widget. It will push a branch, open a pull request, check CI and land into main on Playground forge.", 2, Some(1))).expect("world activity");
            events.push(tools_line(&[("delegate", 1)], &[2], false, 3)).expect("world activity");
            events.push(tools_line(&[("wait", 1)], &[], false, 4)).expect("world activity");
            events.push(agent_line("CSV export is available from widget’s table. The change landed on Playground forge. This was a scripted change.", 5, Some(1))).expect("world activity");
            events.push(tools_line(&[("wait", 1)], &[], false, 6)).expect("world activity");
        }
        (FirstRun { step: 4 }, 4) => {
            events.push(agent_line("I would give widget a dark theme that follows the system's preference, with its colours as variables in one place so both themes stay in step. I am the scripted model and play this from a script, so nothing is changed: connect a model from Accounts to have the work done.", 2, Some(1))).expect("world activity");
        }
        _ => {}
    }
    let minutes: &[i64] = match (w, task) {
        (Reference, 1) => &[-64, -50, -50],
        (Solo, 6) => &[-118, -117, -88],
        (Solo, 10) => &[-14, -12],
        (Solo, 9) => &[-21, -15, -14],
        (FirstRun { step: 2 }, 1) => &[1, 1, 1],
        (FirstRun { step: 3 | 4 }, 1) => &[1, 1, 1, 4, 4],
        (FirstRun { step: 4 }, 4) => &[5],
        _ => &[],
    };
    let mut rows = List::with_capacity(events.len());
    for i in (0..events.len()).rev() {
        rows.push(c::ActivityLine {
            position: u64::from(i).checked_add(1).expect("position"),
            at: offset(w, *minutes.get(usize::try_from(i).expect("event index")).expect("transcript time")),
            event: events.get(i).expect("event").clone(),
        })
        .expect("activity row");
    }
    c::ActivityPage { rows, after: None, before: None, cut: false }
}
#[expect(clippy::too_many_lines, reason = "literal account and source facts from the mockup worlds")]
fn accounts(w: World, then: Then) -> c::OwnerAccounts {
    let mut rows = List::with_capacity(2);
    match w {
        Reference => {
            let mut sources = List::with_capacity(2);
            sources
                .push(c::OwnerAccountSource {
                    name: Some(Box::from("claude-pat".as_bytes())),
                    place: 1,
                    number: 1,
                    kind: c::OwnerAccountKind::Program,
                    state: c::OwnerAccountState::Granted,
                    usable: true,
                    expiring: false,
                    attention: false,
                    spent_until: None,
                })
                .expect("source");
            sources
                .push(c::OwnerAccountSource {
                    name: Some(Box::from("claude-pat-2".as_bytes())),
                    place: 2,
                    number: 2,
                    kind: c::OwnerAccountKind::Program,
                    state: c::OwnerAccountState::Granted,
                    usable: true,
                    expiring: false,
                    attention: false,
                    spent_until: None,
                })
                .expect("source");
            rows.push(c::OwnerAccountLine {
                number: 1,
                place: 1,
                uses: None,
                plan: true,
                provider: Some(17),
                provider_name: Some(doc("Anthropic")),
                manage_usage: None,
                display: doc("Claude"),
                holder: c::OwnerAccountHolder::Party(73),
                sources,
                cut: false,
            })
            .expect("account");
            let mut sources = List::with_capacity(2);
            sources
                .push(c::OwnerAccountSource {
                    name: Some(Box::from("pat@example.com".as_bytes())),
                    place: 1,
                    number: 3,
                    kind: if then == Then::KeyReady { c::OwnerAccountKind::Key } else { c::OwnerAccountKind::SignIn },
                    state: c::OwnerAccountState::Granted,
                    usable: true,
                    expiring: false,
                    attention: false,
                    spent_until: None,
                })
                .expect("source");
            rows.push(c::OwnerAccountLine {
                number: 2,
                place: 2,
                uses: None,
                plan: then != Then::KeyReady,
                provider: Some(18),
                provider_name: Some(doc("OpenAI")),
                manage_usage: None,
                display: doc(if matches!(then, Then::SignInReady | Then::Declined | Then::Renewed) {
                    "pat@example.com"
                } else {
                    "ChatGPT"
                }),
                holder: c::OwnerAccountHolder::Party(73),
                sources,
                cut: false,
            })
            .expect("account");
        }
        Solo => {
            let mut sources = List::with_capacity(2);
            sources
                .push(c::OwnerAccountSource {
                    name: Some(Box::from("claude-robin".as_bytes())),
                    place: 1,
                    number: 1,
                    kind: c::OwnerAccountKind::Program,
                    state: c::OwnerAccountState::Granted,
                    usable: true,
                    expiring: false,
                    attention: false,
                    spent_until: None,
                })
                .expect("source");
            sources
                .push(c::OwnerAccountSource {
                    name: Some(Box::from("robin-key".as_bytes())),
                    place: 2,
                    number: 2,
                    kind: c::OwnerAccountKind::Key,
                    state: c::OwnerAccountState::Granted,
                    usable: true,
                    expiring: false,
                    attention: false,
                    spent_until: None,
                })
                .expect("source");
            rows.push(c::OwnerAccountLine {
                number: 1,
                place: 1,
                uses: None,
                plan: true,
                provider: Some(17),
                provider_name: Some(doc("Anthropic")),
                manage_usage: None,
                display: doc("Claude"),
                holder: c::OwnerAccountHolder::Party(73),
                sources,
                cut: false,
            })
            .expect("account");
        }
        FirstRun { .. } | World::Bare => {
            let mut sources = List::with_capacity(2);
            sources
                .push(c::OwnerAccountSource {
                    name: Some(Box::from("playground".as_bytes())),
                    place: 1,
                    number: 1,
                    kind: c::OwnerAccountKind::Key,
                    state: c::OwnerAccountState::Granted,
                    usable: true,
                    expiring: false,
                    attention: false,
                    spent_until: None,
                })
                .expect("source");
            rows.push(c::OwnerAccountLine {
                number: 1,
                place: 1,
                uses: None,
                plan: false,
                provider: Some(if matches!(w, FirstRun { step: 5..=7 | 9 }) { 18 } else { 17 }),
                provider_name: Some(doc(if matches!(w, FirstRun { step: 5..=7 | 9 }) {
                    "OpenAI"
                } else {
                    "Playground"
                })),
                manage_usage: None,
                display: doc(if matches!(w, FirstRun { step: 5..=7 | 9 }) { "ChatGPT" } else { "Scripted model" }),
                holder: c::OwnerAccountHolder::Party(73),
                sources,
                cut: false,
            })
            .expect("account");
        }
    }
    if w == World::Bare {
        rows.clear();
    }
    let mut hosts = List::with_capacity(2);
    if w == Reference || w == Solo {
        let names: &[(&str, &str)] = if w == Reference {
            &[("ws", "claude-pat"), ("s1", "claude-pat-2")]
        } else {
            &[("laptop", "claude-robin")]
        };
        for (i, (name, login)) in names.iter().enumerate() {
            let mut programs = List::with_capacity(1);
            programs
                .push(c::OwnerProgramLogin {
                    name: Box::from(login.as_bytes()),
                    state: Some(c::OwnerProgramState::Usable),
                    sign_out: None,
                    cleanup: false,
                    cut: false,
                })
                .expect("login");
            hosts
                .push(c::OwnerProgramHost {
                    name: Box::from(name.as_bytes()),
                    channel: u64::try_from(i).expect("host").checked_add(1).expect("host"),
                    connected: true,
                    awaiting_logins: false,
                    programs,
                    cut: false,
                })
                .expect("host");
        }
    }
    let mut readiness = List::with_capacity(2);
    for provider in &providers(w) {
        readiness
            .push(c::PayerReadiness {
                provider: provider.provider,
                payer: c::OwnerAccountUse::Party(73),
                readiness: if w == World::Bare || matches_demo(w) && provider.provider == 18 {
                    c::Readiness::Unavailable
                } else {
                    c::Readiness::Usable
                },
                settled: Some(true),
            })
            .expect("provider readiness");
    }
    c::OwnerAccounts {
        program_hosts: hosts,
        accounts: rows,
        signing: List::with_capacity(0),
        last_end: match then {
            Then::SignInReady | Then::Declined => Some(c::OwnerAccountLastEnd {
                source: 43,
                end: c::OwnerAccountEnd::Made { account: 2, declined: then == Then::Declined },
            }),
            Then::Renewed => Some(c::OwnerAccountLastEnd {
                source: 43,
                end: c::OwnerAccountEnd::Renewed { account: 2, declined: false },
            }),
            Then::SignInRefused => Some(c::OwnerAccountLastEnd {
                source: 43,
                end: c::OwnerAccountEnd::Refused { refusal: c::OwnerAccountRefusal::Held { provider: 18 } },
            }),
            Then::Shown
            | Then::Filled
            | Then::Refused
            | Then::SignedOut
            | Then::GoogleSignIn
            | Then::OutOfDate
            | Then::Closing
            | Then::Rejecting
            | Then::Held
            | Then::Releasing
            | Then::SigningIn
            | Then::AccountRefused
            | Then::KeyReady => None,
        },
        usable: w != World::Bare,
        settled: true,
        cut: false,
        readiness,
        readiness_cut: false,
    }
}

/// The mockup's tenths rounded to the contract's whole deployment unit.
fn round_mockup_spend(tenths: u64) -> u64 {
    (tenths + 5) / 10
}
fn text(s: &str) -> c::Text {
    c::Text::new(s.as_bytes().into(), limits().client.domain.text).expect("mockup text fits shipped text limit")
}
fn path(segments: &[&[u8]]) -> List<Box<[u8]>> {
    let mut result = List::with_capacity(u32::try_from(segments.len()).expect("short path"));
    for segment in segments {
        result.push(Box::from(*segment)).expect("path capacity");
    }
    result
}
fn offers(kinds: &[c::ActKind]) -> List<c::Offered> {
    let mut result = List::with_capacity(u32::try_from(kinds.len()).expect("few acts"));
    for act in kinds {
        let effect = match act {
            c::ActKind::Cancel => Some(c::Effect::Cancel(c::Impact {
                tasks: c::PhaseCounts {
                    waiting: 0,
                    running: 0,
                    held: 0,
                    closing: 0,
                    ended: 0,
                    done: 0,
                    failed: 0,
                    cancelled: 0,
                },
                live_runs: 0,
                proposals: 0,
            })),
            c::ActKind::Message
            | c::ActKind::Answer
            | c::ActKind::Take
            | c::ActKind::HandBack
            | c::ActKind::Accept
            | c::ActKind::Reject
            | c::ActKind::PassUp
            | c::ActKind::Release
            | c::ActKind::LeaveHeld
            | c::ActKind::Amend
            | c::ActKind::Stop
            | c::ActKind::Move
            | c::ActKind::Reorder
            | c::ActKind::Seen
            | c::ActKind::EditNote
            | c::ActKind::DeleteNote
            | c::ActKind::Adopt
            | c::ActKind::ChangePolicy
            | c::ActKind::SetRoles
            | c::ActKind::MakeService
            | c::ActKind::SetPool
            | c::ActKind::Rename
            | c::ActKind::Withdraw => None,
        };
        result.push(c::Offered { act: *act, admission: c::Admission::Allowed, effect }).expect("act capacity");
    }
    result
}
fn base_frame(person: &str, project: &str) -> c::Me {
    let mut templates = List::with_capacity(1);
    templates
        .push(c::Template {
            number: 1,
            name: doc("chat"),
            executor: c::TemplateExecutor::Charter(1),
            contract: c::TaskContractDocument::Report { words: limits().client.domain.text },
            shallower: 1,
            budget: c::TemplateBudget::AsAllowed { cap: None },
            deadline: c::TemplateDeadline::None,
            tracked: None,
            flags: CHAT_FLAGS,
            label: CHAT_LABEL,
            grace: Some(Duration::from_secs(30)),
            llm: None,
        })
        .expect("one template");
    let mut projects = List::with_capacity(1);
    projects
        .push(c::MeProject {
            project: PROJECT,
            name: doc(project),
            role: c::HeldRole::Owner,
            pool: None,
            templates,
            creating: None,
            models: List::with_capacity(0),
        })
        .expect("one project");
    c::Me {
        party: c::MeParty { number: 73, kind: c::PartyKind::Person, login: doc(person), name: doc(person) },
        projects,
        needs: c::Needs { questions: 0, proposals: 0, escalations: 0, person_tasks: 0 },
        needs_total: 0,
        updates: c::Count { value: 0, limited: false },
        results_seen: 0,
        cut: false,
        retention: Duration::from_secs(1000),
        deployment_role: Some(c::HeldRole::Owner),
        providers: List::with_capacity(0),
        providers_cut: false,
        key_bytes: 4096,
        redirect_bytes: 4096,
    }
}
fn base_task(number: u64, title: &str) -> c::Task {
    let wake = c::OwnerWakePolicy {
        words: c::WakeRule::Immediate,
        notices: c::WakeRule::Never,
        news: c::WakeRule::Never,
        results: c::ResultsWake::Each,
        questions: true,
        answers: true,
        timers: false,
    };
    let budget = c::OwnerBudget {
        numbers: c::OwnerNumbers { budget: 0, spent: 0, spent_below: 0, reserved: 0 },
        left: 0,
        funder: c::Funder::Pool { project: PROJECT, person: 73, period: 1 },
        deadline: None,
    };
    c::Task {
        chip: c::Chip {
            task: number,
            title: doc(title),
            executor: c::Executor::Agent { charter: 1 },
            phase: c::Phase::Idle,
            details: Box::new(c::ChipPhase::Owner {
                phase: c::OwnerChipPhase::Active(c::RunStage::Idle),
                details: Box::new(c::OwnerChipDetails {
                    dependencies: List::with_capacity(0),
                    project: PROJECT,
                    executor: c::OwnerExecutor::Agent { charter: 1 },
                    phase_since: Wall::EPOCH,
                    run_since: None,
                    attempt: None,
                    retry: None,
                    wake,
                    person_wait: None,
                    hold: None,
                    budget: budget.clone(),
                    activity: 0,
                    resources: List::with_capacity(0),
                    resources_cut: false,
                    resources_observed: false,
                    procedure: None,
                    live_delegates: 0,
                    closing_connectors: None,
                    ending: None,
                    ended_at: None,
                    observation: None,
                }),
            }),
        },
        label: CHAT_LABEL,
        flags: CHAT_FLAGS,
        last_activity: Wall::EPOCH,
        waits_on: c::WaitsOn::Nothing,
        goal: None,
        priority: None,
        lineage_chips: List::with_capacity(0),
        inputs: List::with_capacity(0),
        project: PROJECT,
        requester: c::OwnerRequester::Person(73),
        lineage: List::with_capacity(0),
        spec: c::SpecificationDocument::complete(c::Specification {
            words: text(""),
            parameters: List::with_capacity(0),
            inputs: List::with_capacity(0),
        }),
        spec_revision: c::Revision { value: 1 },
        person_revision: None,
        taken_by: None,
        instructions: None,
        wake,
        subscriptions: c::Subscriptions { entries: List::with_capacity(0) },
        result_contract: c::TaskContractDocument::Report { words: limits().client.domain.text },
        dependencies: List::with_capacity(0),
        dependency_facts: List::with_capacity(0),
        references: List::with_capacity(0),
        cut: false,
        resources: List::with_capacity(0),
        budget,
        authority: c::OwnerAuthorityDocument::complete(c::OwnerAuthority {
            models: None,
            tools: 0,
            grants: List::with_capacity(0),
            delegation: c::OwnerDelegation { kinds: List::with_capacity(0), tasks: 0, depth: 0 },
            spend: 0,
            deadline: None,
            notes: 0,
            note_resources: List::with_capacity(0),
        }),
        hold: None,
        proposals: List::with_capacity(0),
        activity: 0,
        changed: Wall::EPOCH,
        acts: offers(&[c::ActKind::Message, c::ActKind::Stop, c::ActKind::Cancel]),
        result: None,
        delegates: List::with_capacity(0),
        context: None,
        llm: None,
        kept_account: None,
        run_wait: None,
    }
}
fn task_window(w: World, window: u32) -> c::Tasks {
    let all = chat_rows(w, false);
    let mut rows = List::with_capacity(window);
    for row in &all {
        if rows.len() < window {
            rows.push(row.clone()).expect("window");
        }
    }
    c::Tasks { more: all.len() > rows.len(), rows }
}
fn ended_rows(w: World) -> List<c::TaskLine> {
    let all = chat_rows(w, true);
    let mut rows = List::with_capacity(limits().client.domain.rows);
    for row in &all {
        if rows.len() < rows.capacity() {
            rows.push(row.clone()).expect("ended window");
        }
    }
    rows
}
fn reply_line(w: World, task: u64) -> c::TaskLine {
    let mut line = task_line(document(w, task));
    line.conversation = Some(c::ConversationRead { seen: 0, unread: c::Count { value: 1, limited: false } });
    line
}
fn inbox_window(w: World, window: u32) -> c::OwnerInbox {
    // The engine allocates needs and updates independently, each at the requested window.
    let source = inbox_document(w);
    let mut needs = List::with_capacity(window);
    let mut updates = List::with_capacity(window);
    let mut more = None;
    for row in &source.entries {
        let (task, object) = match row.object {
            c::ObjectKey::Proposal(key) => {
                (10, c::OwnerInboxObject::Proposal { proposer: key.proposer, number: key.number })
            }
            c::ObjectKey::Question { task, message } => (task, c::OwnerInboxObject::Question { message }),
            c::ObjectKey::Task(task) => (task, c::OwnerInboxObject::Reply { message: 1 }),
            c::ObjectKey::Escalation(_)
            | c::ObjectKey::PersonTask(_)
            | c::ObjectKey::Result(_)
            | c::ObjectKey::Note { .. } => unreachable!("world inbox has proposals, questions and replies"),
        };
        let cursor = c::OwnerInboxCursor::Inbox { task, object, at: Some(row.at) };
        let reply = if row.group == c::InboxGroup::Replies { Some(Box::new(reply_line(w, task))) } else { None };
        let entry = c::OwnerInboxEntry {
            reply,
            task: c::OwnerTaskLink::Live(chip(w, task)),
            project: Some(PROJECT),
            object,
            activity: Some(row.position),
            at: Some(row.at),
            unread: Some(true),
            cursor,
        };
        let group = match row.group {
            c::InboxGroup::Proposals
            | c::InboxGroup::Questions
            | c::InboxGroup::Escalations
            | c::InboxGroup::PersonTasks => &mut needs,
            c::InboxGroup::Replies | c::InboxGroup::Results => &mut updates,
        };
        if group.len() < window {
            group.push(entry).expect("inbox window");
        } else {
            more = Some(cursor);
        }
    }
    c::OwnerInbox {
        party: 73,
        needs,
        updates,
        next: more,
        cached_needs: source.needs.proposals + source.needs.questions,
        cached_updates: source.updates,
        results_seen: source.results_seen,
        updates_time_complete: true,
        selection: None,
    }
}
// A page of `rows` lines, newest first, from the cursor's row offset; the
// cursor carries the next page's offset, so a chat filling its window ends.
fn activity_window(w: World, task: u64, before: Option<&c::Cursor>) -> c::ActivityPage {
    let mut document = activity(w, task);
    let all = document.rows;
    let from = match before {
        Some(cursor) => usize::from(*cursor.value.bytes().first().expect("gallery cursor")),
        None => 0,
    };
    document.rows = List::with_capacity(limits().client.domain.rows);
    for row in all.iter().skip(from) {
        if document.rows.len() < document.rows.capacity() {
            document.rows.push(row.clone()).expect("activity window");
        }
    }
    let next = from.checked_add(document.rows.as_slice().len()).expect("gallery offset");
    if all.as_slice().len() > next {
        let offset = u8::try_from(next).expect("gallery activity fits a byte");
        document.before =
            Some(c::Cursor::new(Box::from([offset].as_slice()), limits().client.domain.text).expect("cursor"));
    }
    document
}
fn shown_task(w: World, task: u64, then: Then) -> c::Task {
    let mut t = document(w, task);
    if then == Then::Held || then == Then::Releasing {
        t.goal = Some(task);
        t.flags.conversation = false;
        t.flags.listed = true;
        let hold = c::OwnerChipHold {
            circle: None,
            account: None,
            source: None,
            reason: c::HoldCategory::Failures { class: c::HoldFailure::Run },
            revision: 7,
            before: c::OwnerPriorPhase::Active(c::RunStage::Idle),
            escalation: None,
            tries: c::OwnerFailureTries { transient: 0, permanent: 0, run: 2, agent: 0, lost: 0, invalid: 0 },
            hold_wait_since: None,
        };
        t.hold = Some(c::TaskHold::Owner(Box::new(hold.clone())));
        t.chip.phase = c::Phase::Held;
        let (phase, details) = owner(&mut t.chip);
        *phase = c::OwnerChipPhase::Held { reason: hold.reason, revision: hold.revision };
        details.hold = Some(hold);
        t.acts = List::with_capacity(1);
        t.acts
            .push(c::Offered {
                act: c::ActKind::Release,
                admission: c::Admission::Allowed,
                effect: Some(c::Effect::Release(c::ReleaseEffect::RunAgain)),
            })
            .expect("release");
    }
    t
}
fn plan_tree(w: World, task: u64, window: u32, then: Then) -> c::Delivery {
    let root = shown_task(w, task, then).chip;
    let mut descendants = Vec::new();
    let mut pending = vec![(task, 0_u32)];
    while let Some((parent, depth)) = pending.pop() {
        for number in facts(w, parent).children {
            let f = facts(w, *number);
            // The owner tree contains live tasks; T5's retained result is
            // not a plan row (client/README.md, 5.8).
            if f.phase != c::Phase::Done && f.phase != c::Phase::Ended {
                descendants.push((*number, parent, depth + 1));
                pending.push((*number, depth + 1));
            }
        }
    }
    // The engine's live task view supplies number order, not mockup order.
    descendants.sort_unstable_by_key(|(number, _, _)| *number);
    let mut rows = List::with_capacity(window);
    let mut phases = c::OwnerPhaseCounts::ZERO;
    for (number, parent, depth) in descendants {
        if rows.len() < window {
            rows.push(c::OwnerTreeRow {
                chip: chip(w, number),
                parent,
                depth,
                proposals: List::with_capacity(0),
                spent: round_mockup_spend(facts(w, number).spent_tenths),
                dependencies: List::with_capacity(0),
                cut: false,
            })
            .expect("tree window");
        } else {
            // The solo goal's omitted live descendants are running.
            assert_eq!(facts(w, number).phase, c::Phase::Running);
            phases.running += 1;
        }
    }
    let mut folded = List::with_capacity(1);
    let next = if phases.running > 0 {
        folded.push(c::OwnerTreeFold { parent: task, phases, held_below: 0 }).expect("root fold");
        Some(c::Cursor::new(Box::from(b"next".as_slice()), limits().client.domain.text).expect("opaque cursor"))
    } else {
        None
    };
    c::Delivery::OwnerTree(Box::new(c::OwnerTree { root, rows, folded, held_below: 0, next }))
}

fn deployment(w: World) -> c::OwnerDeployment {
    c::OwnerDeployment {
        store: c::StoreFill { bytes: 0, mark: 0 },
        hosts: List::with_capacity(0),
        attempts: List::with_capacity(0),
        accounts: List::with_capacity(0),
        tasks: c::OwnerCapacity { used: 0, limit: 0 },
        workers: c::OwnerCapacity { used: 0, limit: 0 },
        attempts_capacity: c::OwnerCapacity { used: 0, limit: 0 },
        waiting_attempts: 0,
        proposals: c::OwnerCapacity { used: 0, limit: 0 },
        projects: List::with_capacity(0),
        grace_bound: Duration::from_secs(30),
        hello_refusals: List::with_capacity(0),
        hosts_forgotten: 0,
        hello_refusals_forgotten: 0,
        person_room_refusals: List::with_capacity(0),
        person_room_refusals_forgotten: 0,
        person_answer_retention: Duration::from_secs(1000),
        task_room_refusals: List::with_capacity(0),
        task_room_refusals_forgotten: 0,
        cut: false,
        connectors: List::with_capacity(0),
        connectors_cut: false,
        providers: providers(w),
        providers_cut: false,
        key_bytes: 4096,
        readiness: List::with_capacity(0),
        readiness_cut: false,
    }
}

/// Mockup world clocks, without reading the host's clock (client/browser.md, 7.2).
pub(super) fn now(w: World) -> Wall {
    let minutes = match w {
        FirstRun { step } => match step {
            0 | 1 => 0,
            2 => 2,
            3 => 4,
            4 => 6,
            5 => 15,
            6 | 9 => 16,
            7 | 8 => 19,
            _ => unreachable!("mockup first-run step"),
        },
        Reference | Solo | World::Bare => 0,
    };
    offset(w, minutes)
}

fn offset(w: World, minutes: i64) -> Wall {
    let hour0 = match w {
        Reference => 1_792_656_000_000_000_000_u64,
        Solo => 1_793_093_400_000_000_000,
        FirstRun { .. } => 1_792_702_800_000_000_000,
        World::Bare => 0,
    };
    let delta = minutes.unsigned_abs().checked_mul(60_000_000_000).expect("mockup offset fits");
    Wall::from_nanos(
        if minutes < 0 { hour0.checked_sub(delta) } else { hour0.checked_add(delta) }.expect("mockup time fits"),
    )
}

fn sample_attempt(root: &mut Domain, out: &mut Queue<Request>) -> Token {
    step(root, Event::Client { event: jig_client::Event::Stored }, out);
    while let Some(request) = out.pop() {
        if let Some(jig_client::Down::Send { attempt, .. }) = crate::driver::down(request) {
            return attempt;
        }
    }
    unreachable!("sample sent its request")
}
fn sample_answer(root: &mut Domain, attempt: Token, answer: jig_client::Answered, out: &mut Queue<Request>) {
    step(
        root,
        Event::Client {
            event: jig_client::Event::Up {
                up: jig_client::Up::Answer {
                    attempt,
                    envelope: skein_lib::RequestEnvelope::Final,
                    answer: Some(answer),
                    why: None,
                },
            },
        },
        out,
    );
}
fn sample_forge_answer(
    root: &mut Domain,
    attempt: Token,
    outcome: temper_client_protocol_wire::v1::Outcome,
    out: &mut Queue<Request>,
) {
    use temper_client_protocol_wire::{self as wire, v1 as w};
    sample_answer(
        root,
        attempt,
        jig_client::Answered::Family(jig_client::Family {
            family: wire::FORGE,
            version: wire::CLIENT_VERSION,
            bytes: wire::answer(&w::Answer::new(&w::CEILINGS, w::AnswerParts { outcome }).expect("sample answer")),
        }),
        out,
    );
}
fn adopt_owned_sample(root: &mut Domain, watch: Token, out: &mut Queue<Request>) {
    use temper_client_protocol_wire::v1 as w;
    step(
        root,
        Event::Act {
            action: temper_client_domain::Action::Forge(temper_client_domain::forge::Act::SetUp { forge: 3 }),
        },
        out,
    );
    let attempt = sample_attempt(root, out);
    sample_forge_answer(root, attempt, w::Outcome::SettingUp, out);
    // The first row is durable Made; the project's creation and Adopt follow it.
    forges_watched(root, FirstRun { step: 7 }, watch, out);
    let attempt = sample_attempt(root, out);
    sample_answer(
        root,
        attempt,
        jig_client::Answered::Jig(c::Answer::Done(c::Outcome::Owner(c::OwnerOutcome::ProjectMade {
            project: PROJECT,
        }))),
        out,
    );
    let attempt = sample_attempt(root, out);
    sample_forge_answer(
        root,
        attempt,
        w::Outcome::Adopted(
            w::Adopted::new(
                &w::CEILINGS,
                w::AdoptedParts {
                    forge: 3,
                    repository: 42,
                    owner: b"work".as_slice().into(),
                    name: b"hello".as_slice().into(),
                    role: w::Role::Owned,
                    home: true,
                    kinds: w::Kinds::new(&w::CEILINGS, w::KindsParts { skein_bools: [true; 9] }).expect("sample kinds"),
                    protection: w::Protection::Unknown,
                },
            )
            .expect("sample adopted"),
        ),
        out,
    );
}
