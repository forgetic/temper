#[path = "../../../../testing/first_run.rs"]
pub mod first_run;
use jig_client::contract as c;
use jig_client::contract::{OwnerPart, OwnerPartKey, OwnerPartKind, Part};
use skein_lib::{Duration, Env, List, Queue, RequestLimits, Time, Token, Wall, Writer};
use temper_client_domain::{Limits, forge};
use temper_client_protocol_wire::{self as wire, v1 as w};
pub const NOW: Wall = Wall::EPOCH;
pub const FORGE: u16 = temper_client_protocol_wire::number(temper_client_protocol_wire::Owner::Forge);
pub const PROJECT: u32 = 1;
pub const SECOND_PROJECT: u32 = 2;
pub const SEED: u64 = 11;
#[expect(clippy::arithmetic_side_effects, reason = "the request count is the nonzero constant declared here")]
pub fn limits() -> Limits {
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
            accounts: temper_client_domain::limits::AccountLimits { accounts: 16, sources: 8, uses: 8, sign_ins: 4 },
        }
    }
}
pub fn words() -> crate::words::Words {
    static WORDS: std::sync::OnceLock<crate::words::Words> = std::sync::OnceLock::new();
    WORDS.get_or_init(|| english(crate::words::Shell::Terminal)).clone()
}
pub fn browser_words() -> crate::words::Words {
    static WORDS: std::sync::OnceLock<crate::words::Words> = std::sync::OnceLock::new();
    WORDS.get_or_init(|| english(crate::words::Shell::Browser)).clone()
}
fn english(shell: crate::words::Shell) -> crate::words::Words {
    crate::words::Words::english(FORGE, 64, shell).expect("forge English fits")
}

pub fn tree_limits() -> skein_ui::Limits {
    skein_ui::Limits {
        // Two time nodes for each of the six inbox cards, and a goal's page
        // with nine plan rows, live and ended (181 nodes).
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
    }
}
pub fn domain_limits() -> Limits {
    limits()
}
pub fn path(segments: &[&[u8]]) -> List<Box<[u8]>> {
    let mut path = List::with_capacity(u32::try_from(segments.len()).expect("fixture segments fit"));
    for segment in segments {
        path.push(skein_lib::bytes::copy_of(segment)).expect("fixture path fits");
    }
    path
}
pub fn text(tree: &skein_ui::Tree) -> Box<[u8]> {
    nodes_text(tree.nodes())
}
/// The text of a node and its descendants, in tree order.
pub fn node_text(tree: &skein_ui::Tree, index: usize) -> Box<[u8]> {
    let end = index
        .checked_add(usize::try_from(tree.nodes()[index].size).expect("fixture subtree fits"))
        .expect("fixture index fits");
    nodes_text(&tree.nodes()[index..end])
}
pub(super) fn nodes_text(nodes: &[skein_ui::Node]) -> Box<[u8]> {
    let mut bytes = 0_usize;
    for node in nodes {
        for run in &node.text {
            bytes = bytes.checked_add(run.text.len()).expect("fixture text fits");
        }
    }
    let mut writer = Writer::new(bytes);
    for node in nodes {
        for run in &node.text {
            writer.put(&run.text).expect("text counted");
        }
    }
    writer.finish()
}
pub fn owner_part_text(words: &crate::words::Words, segments: &[&[u8]]) -> Box<[u8]> {
    let mut builder = skein_ui::Builder::new(&tree_limits());
    crate::forge::owner_part(
        &mut builder,
        &OwnerPart { connector: FORGE, kind: OwnerPartKind::Resource, key: OwnerPartKey::Path(path(segments)) },
        words,
    );
    text(&builder.finish())
}
pub fn part_tree(words: &crate::words::Words, part: Part) -> skein_ui::Tree {
    let mut builder = skein_ui::Builder::new(&tree_limits());
    crate::forge::drawn(&mut builder, &part, words);
    builder.finish()
}
pub fn part_text(words: &crate::words::Words, part: Part) -> Box<[u8]> {
    text(&part_tree(words, part))
}
pub fn built() -> crate::View {
    let domain = domain_limits();
    let mut root = temper_client_domain::unlinked(&domain, SEED);
    root_event(
        &mut root,
        temper_client_domain::Event::Client { event: jig_client::Event::SignIn { with: jig_client::With::Transport } },
    );
    let forge = words();
    let envelope = crate::Limits::of(&domain, &forge).expect("composed envelope fits");
    assert!(envelope.tree.nodes >= 128);
    let limits = crate::Limits { tree: tree_limits() };
    let mut view = crate::View::new(&limits, &domain, forge, FORGE);
    crate::build(&mut view, &root, NOW, &mut Queue::with_capacity(limits.tree.patches));
    view
}
pub fn view() -> crate::View {
    view_of(words())
}
pub fn view_of(words: crate::words::Words) -> crate::View {
    let domain = domain_limits();
    let limits = crate::Limits { tree: tree_limits() };
    crate::View::new(&limits, &domain, words, FORGE)
}
pub fn build<'a>(view: &'a mut crate::View, root: &temper_client_domain::Domain) -> &'a skein_ui::Tree {
    crate::build(view, root, NOW, &mut Queue::with_capacity(view.limits().tree.patches));
    view.tree()
}
pub fn find(tree: &skein_ui::Tree, role: skein_ui::Role, name: &[u8]) -> Option<skein_ui::NodeId> {
    for node in tree.nodes() {
        if node.role == role && node.name.as_deref() == Some(name) {
            return Some(node.id);
        }
    }
    None
}
pub fn person_find(tree: &skein_ui::Tree, role: skein_ui::Role, name: &[u8]) -> Option<skein_ui::NodeId> {
    use skein_fake_person::{Face, Find, Found, TreeFace};
    match TreeFace::new(tree).find(&Find::exactly_named(role, name)) {
        Found::Node(id) => Some(id),
        Found::Absent | Found::Pending => None,
    }
}
pub fn root_in_project(project: Option<u32>) -> temper_client_domain::Domain {
    root_with_watch(project).0
}
pub fn root_with_watch(project: Option<u32>) -> (temper_client_domain::Domain, Token) {
    let (root, me, _) = root_at(
        temper_client_domain::Address::Inbox {
            filter: temper_client_domain::address::Filter { project, ..temper_client_domain::address::Filter::ALL },
        },
        &domain_limits(),
    );
    (root, me)
}
pub(super) fn root_at(
    address: temper_client_domain::Address,
    limits: &Limits,
) -> (temper_client_domain::Domain, Token, Option<Token>) {
    let (root, me, page, _) = root_at_with_tree(address, limits);
    (root, me, page)
}
pub(super) fn root_at_with_tree(
    address: temper_client_domain::Address,
    limits: &Limits,
) -> (temper_client_domain::Domain, Token, Option<Token>, Option<Token>) {
    root_at_with_tree_named(address, limits, b"Shop")
}
fn root_at_with_tree_named(
    address: temper_client_domain::Address,
    limits: &Limits,
    project_name: &[u8],
) -> (temper_client_domain::Domain, Token, Option<Token>, Option<Token>) {
    let (root, me, page, tree, _) = root_at_full(address, limits, project_name);
    (root, me, page, tree)
}
pub fn account_root() -> (temper_client_domain::Domain, Token) {
    let (root, _, _, _, accounts) = root_at_full(temper_client_domain::Address::Accounts, &domain_limits(), b"Shop");
    (root, accounts.expect("global accounts watch"))
}
pub(super) fn root_at_full(
    address: temper_client_domain::Address,
    limits: &Limits,
    project_name: &[u8],
) -> (temper_client_domain::Domain, Token, Option<Token>, Option<Token>, Option<Token>) {
    root_at_with_demo(address, limits, project_name, None)
}
#[expect(clippy::manual_map, reason = "fixtures use the same closure-free steps as the client")]
pub(super) fn root_at_with_demo(
    address: temper_client_domain::Address,
    limits: &Limits,
    project_name: &[u8],
    demo: Option<(u32, bool)>,
) -> (temper_client_domain::Domain, Token, Option<Token>, Option<Token>, Option<Token>) {
    let state = if let temper_client_domain::Address::Welcome { .. } = address {
        w::DemoState::NotStarted
    } else {
        w::DemoState::Running
    };
    root_at_with_activity(
        address,
        limits,
        project_name,
        match demo {
            Some((project, model)) => Some((project, model, state)),
            None => None,
        },
        None,
        None,
    )
}
fn root_at_with_activity(
    address: temper_client_domain::Address,
    limits: &Limits,
    project_name: &[u8],
    demo: Option<(u32, bool, w::DemoState)>,
    activity: Option<c::ActivityPage>,
    forge_rows: Option<List<w::ForgeRow>>,
) -> (temper_client_domain::Domain, Token, Option<Token>, Option<Token>, Option<Token>) {
    let (root, me, page, tree, accounts, _) =
        root_at_with_forges(address, limits, project_name, demo, activity, forge_rows);
    (root, me, page, tree, accounts)
}

#[expect(clippy::too_many_lines, reason = "one fixture delivers the page's documents and the session's reads together")]
pub(super) fn root_at_with_forges(
    address: temper_client_domain::Address,
    limits: &Limits,
    project_name: &[u8],
    demo: Option<(u32, bool, w::DemoState)>,
    activity: Option<c::ActivityPage>,
    forge_rows: Option<List<w::ForgeRow>>,
) -> (temper_client_domain::Domain, Token, Option<Token>, Option<Token>, Option<Token>, Option<Token>) {
    use jig_client::contract::{Delivery, Subject, WatchEvent};
    use temper_client_domain::{Event, Request};
    let forge_rows_answered = forge_rows.is_some();
    let env = Env { now: Time::ZERO, wall: NOW, limits: *limits };
    let mut root = temper_client_domain::unlinked(limits, SEED);
    let mut out = Queue::with_capacity(temper_client_domain::max_out(limits));
    temper_client_domain::step(
        &mut root,
        &env,
        Event::Client { event: jig_client::Event::StartUnlinked { kept: List::with_capacity(0) } },
        &mut out,
    );
    let welcome = match address {
        temper_client_domain::Address::Welcome { .. } => true,
        temper_client_domain::Address::Forges
        | temper_client_domain::Address::ConnectForge { .. }
        | temper_client_domain::Address::Chats { .. }
        | temper_client_domain::Address::Accounts
        | temper_client_domain::Address::Inbox { .. }
        | temper_client_domain::Address::Adopt { .. }
        | temper_client_domain::Address::Chat { .. }
        | temper_client_domain::Address::Task { .. }
        | temper_client_domain::Address::ConnectModel { .. } => false,
    };
    // Sign in before opening the page's one-shot activity read.
    temper_client_domain::step(
        &mut root,
        &env,
        Event::Start { address, kept: List::with_capacity(0), with: Some(jig_client::With::Transport) },
        &mut out,
    );
    temper_client_domain::step(
        &mut root,
        &env,
        Event::Client { event: jig_client::Event::Up { up: jig_client::Up::Served { served: true, versions: None } } },
        &mut out,
    );
    let mut watch = None;
    let mut inbox_watch = None;
    let mut tree_watch = None;
    let mut account_watch = None;
    let mut activity_read = None;
    for _ in 0..out.len() {
        let request = out.pop();
        if let Some(read) = activity_token(request.as_ref()) {
            activity_read = Some(read);
        }
        if let Some(Request::Client {
            request:
                jig_client::Request::Down {
                    down: jig_client::Down::Open { watch: token, subject: jig_client::Watchable::Jig(subject) },
                },
        }) = request
        {
            if subject == Subject::Me {
                watch = Some(token);
            } else if subject == Subject::Accounts {
                account_watch = Some(token);
            } else if let Subject::Tree { .. } = subject {
                tree_watch = Some(token);
            } else if let Subject::Inbox { .. } | Subject::Task { .. } = subject {
                inbox_watch = Some(token);
            }
        }
    }
    let mut frame = fixture_me(project_name);
    if forge_rows_answered {
        frame.deployment_role = Some(c::HeldRole::Owner);
    }
    temper_client_domain::step(
        &mut root,
        &env,
        Event::Client {
            event: jig_client::Event::Up {
                up: jig_client::Up::Watched {
                    watch: watch.expect("frame opened"),
                    event: WatchEvent::Delivered(Delivery::Me(Box::new(frame))),
                },
            },
        },
        &mut out,
    );
    answer_activity(&mut root, &env, activity_read, activity, &mut out);
    let mut forges = None;
    for _ in 0..out.len() {
        let request = out.pop().expect("counted request");
        if let Request::Client {
            request:
                jig_client::Request::Down {
                    down: jig_client::Down::Open { watch, subject: jig_client::Watchable::Family { .. } },
                },
        } = &request
        {
            forges = Some(*watch);
        }
        out.push(request);
    }
    if let Some(watch) = forges {
        let rows = match forge_rows {
            Some(rows) => rows,
            None => List::with_capacity(0),
        };
        let wire_limits = forge::wire_limits(&limits.forge).unwrap();
        temper_client_domain::step(
            &mut root,
            &env,
            Event::Client {
                event: jig_client::Event::Up {
                    up: jig_client::Up::FamilyDelivered {
                        watch,
                        record: jig_client::Family {
                            family: wire::FORGE,
                            version: wire::CLIENT_VERSION,
                            bytes: wire::forges(
                                &w::Forges::new(&wire_limits, w::ForgesParts { forges: rows }).unwrap(),
                            ),
                        },
                    },
                },
            },
            &mut out,
        );
    }

    if demo.is_some() || welcome || forge_rows_answered {
        let mut read = None;
        while let Some(request) = out.pop() {
            if let Request::Client {
                request:
                    jig_client::Request::Down {
                        down: jig_client::Down::Read { read: token, what: jig_client::Readable::Family { .. } },
                    },
            } = request
            {
                read = Some(token);
            }
        }
        if let Some((project, model, state)) = demo {
            deliver_demo(
                &mut root,
                read.expect("live link reads demo"),
                account_watch.expect("accounts watch"),
                project,
                model,
                state,
            );
        } else {
            deliver_no_demo(&mut root, read.expect("demo read"), account_watch.expect("accounts watch"));
        }
    }
    (root, watch.expect("me opened"), inbox_watch, tree_watch, account_watch, forges)
}
fn deliver_no_demo(root: &mut temper_client_domain::Domain, read: Token, watch: Token) {
    root_event(
        root,
        temper_client_domain::Event::Client {
            event: jig_client::Event::Up {
                up: jig_client::Up::ReadDone {
                    read,
                    result: jig_client::Readout::Family { family: wire::FORGE, record: None },
                },
            },
        },
    );
    root_event(
        root,
        temper_client_domain::Event::Client {
            event: jig_client::Event::Up {
                up: jig_client::Up::Watched {
                    watch,
                    event: c::WatchEvent::Delivered(c::Delivery::OwnerAccounts(Box::new(c::OwnerAccounts {
                        accounts: List::with_capacity(0),
                        readiness: List::with_capacity(0),
                        readiness_cut: false,
                        program_hosts: List::with_capacity(0),
                        signing: List::with_capacity(0),
                        last_end: None,
                        usable: false,
                        settled: true,
                        cut: false,
                    }))),
                },
            },
        },
    );
}
fn activity_token(request: Option<&temper_client_domain::Request>) -> Option<Token> {
    if let Some(temper_client_domain::Request::Client {
        request:
            jig_client::Request::Down {
                down: jig_client::Down::Read { read, what: jig_client::Readable::Jig(c::Read::Activity { .. }) },
            },
    }) = request
    {
        Some(*read)
    } else {
        None
    }
}
fn answer_activity(
    root: &mut temper_client_domain::Domain,
    env: &Env<Limits>,
    mut read: Option<Token>,
    activity: Option<c::ActivityPage>,
    out: &mut Queue<temper_client_domain::Request>,
) {
    let Some(page) = activity else { return };
    // Signing in can replace a page's initial read. Answer its current one.
    for _ in 0..out.len() {
        let request = out.pop();
        if let Some(token) = activity_token(request.as_ref()) {
            read = Some(token);
        }
        if let Some(request) = request {
            out.push(request);
        }
    }
    temper_client_domain::step(
        root,
        env,
        temper_client_domain::Event::Client {
            event: jig_client::Event::Up {
                up: jig_client::Up::ReadDone {
                    read: read.expect("activity read"),
                    result: jig_client::Readout::Jig(c::ReadResult::Value(c::ReadValue::Activity(Box::new(page)))),
                },
            },
        },
        out,
    );
}
pub fn adoption_root() -> temper_client_domain::Domain {
    adoption_page(PROJECT, b"shop")
}
pub fn adoption_page(project: u32, name: &[u8]) -> temper_client_domain::Domain {
    root_at_with_tree_named(temper_client_domain::Address::Adopt { project }, &domain_limits(), name).0
}
pub fn root_event(root: &mut temper_client_domain::Domain, event: temper_client_domain::Event) {
    let limits = domain_limits();
    temper_client_domain::step(
        root,
        &Env { now: Time::ZERO, wall: NOW, limits },
        event,
        &mut Queue::with_capacity(temper_client_domain::max_out(&limits)),
    );
}

pub fn signed_in_with_project() -> crate::View {
    let mut view = view();
    build(&mut view, &root_in_project(Some(PROJECT)));
    view
}
pub fn navigation_link_names(view: &crate::View) -> Box<[Box<[u8]>]> {
    let mut names = List::with_capacity(view.limits().tree.nodes);
    for (index, node) in view.tree().nodes().iter().enumerate() {
        if node.role == skein_ui::Role::Navigation {
            let end =
                index.checked_add(usize::try_from(node.size).expect("navigation fits")).expect("navigation end fits");
            for child in &view.tree().nodes()[index..end] {
                if child.role == skein_ui::Role::Link
                    && let Some(name) = &child.name
                {
                    names.push(skein_lib::bytes::copy_of(name)).expect("tree bounds link names");
                }
            }
        }
    }
    names.into_boxed()
}
pub fn banner_statuses(tree: &skein_ui::Tree) -> Box<[Box<[u8]>]> {
    let mut names = List::with_capacity(tree.len());
    for (index, node) in tree.nodes().iter().enumerate() {
        if node.role == skein_ui::Role::Banner {
            let end = index.checked_add(usize::try_from(node.size).expect("banner fits")).expect("banner end fits");
            for child in &tree.nodes()[index..end] {
                if child.role == skein_ui::Role::Status
                    && let Some(name) = &child.name
                {
                    names.push(skein_lib::bytes::copy_of(name)).expect("tree bounds statuses");
                }
            }
        }
    }
    names.into_boxed()
}

pub const INBOX_TASK: u64 = 9;
pub const INBOX_REPLY_TASK: u64 = 10;
pub const INBOX_QUESTION: u64 = 1;
pub const INBOX_POSITION: u64 = 1;
pub const OTHER_PARTY: u64 = 74;
pub fn inbox_root() -> temper_client_domain::Domain {
    inbox_root_filtered(temper_client_domain::address::Filter::ALL)
}
pub fn inbox_root_filtered(filter: temper_client_domain::address::Filter) -> temper_client_domain::Domain {
    inbox_with_document(filter, None)
}
pub fn inbox_with_document(
    filter: temper_client_domain::address::Filter,
    document: Option<(jig_client::contract::Read, jig_client::contract::ReadValue)>,
) -> temper_client_domain::Domain {
    inbox_with_proposer(filter, document, c::Proposer::Task(INBOX_TASK))
}
pub fn inbox_from(proposer: c::Proposer) -> temper_client_domain::Domain {
    inbox_with_proposer(temper_client_domain::address::Filter::ALL, None, proposer)
}
fn inbox_with_proposer(
    filter: temper_client_domain::address::Filter,
    document: Option<(c::Read, c::ReadValue)>,
    proposer: c::Proposer,
) -> temper_client_domain::Domain {
    inbox_facts(filter, document, proposer, false, None)
}
pub fn inbox_facts(
    filter: temper_client_domain::address::Filter,
    document: Option<(c::Read, c::ReadValue)>,
    proposer: c::Proposer,
    cut: bool,
    unread: Option<c::Count>,
) -> temper_client_domain::Domain {
    use jig_client::contract::{self, InboxGroup as Group, ObjectKey as Object};
    let mut limits = domain_limits();
    // Six card reads beside the frame's own bounded reads.
    limits.client.domain.reads = 8;
    let (mut root, _, watch) = root_at(temper_client_domain::Address::Inbox { filter }, &limits);
    let env = Env { now: Time::ZERO, wall: NOW, limits };
    let mut out = Queue::with_capacity(temper_client_domain::max_out(&limits));
    let mut entries = List::with_capacity(6);
    for (object, group) in [
        (Object::Question { task: INBOX_TASK, message: INBOX_QUESTION }, Group::Questions),
        (Object::Proposal(contract::ProposalKey { proposer, number: INBOX_QUESTION }), Group::Proposals),
        (Object::Escalation(INBOX_TASK), Group::Escalations),
        (Object::PersonTask(INBOX_TASK), Group::PersonTasks),
        (Object::Result(INBOX_TASK), Group::Results),
        (Object::Task(INBOX_REPLY_TASK), Group::Replies),
    ] {
        entries
            .push(contract::InboxEntry {
                reply: if group == Group::Replies {
                    let mut line = super::chats::line();
                    line.chip.task = INBOX_REPLY_TASK;
                    if let Some(unread) = unread {
                        line.conversation = Some(c::ConversationRead { seen: 0, unread });
                    }
                    Some(Box::new(line))
                } else {
                    None
                },
                lineage: List::with_capacity(0),
                goal: None,
                recipient: None,
                stall: None,
                object,
                project: PROJECT,
                group,
                position: INBOX_POSITION,
                at: Wall::EPOCH,
            })
            .expect("six kinds");
    }
    let inbox = contract::Inbox {
        entries,
        needs: contract::Needs { questions: 1, proposals: 1, escalations: 1, person_tasks: 1 },
        updates: contract::Count { value: 2, limited: false },
        results_seen: 0,
        replies_seen: List::with_capacity(0),
        more: cut,
    };
    temper_client_domain::step(
        &mut root,
        &env,
        temper_client_domain::Event::Client {
            event: jig_client::Event::Up {
                up: jig_client::Up::Watched {
                    watch: watch.expect("inbox watch"),
                    event: contract::WatchEvent::Delivered(contract::Delivery::Inbox(Box::new(inbox))),
                },
            },
        },
        &mut out,
    );
    if let Some((read, page)) = document {
        while let Some(request) = out.pop() {
            if let temper_client_domain::Request::Client {
                request:
                    jig_client::Request::Down {
                        down: jig_client::Down::Read { read: token, what: jig_client::Readable::Jig(what) },
                    },
            } = request
                && what == read
            {
                root_event(
                    &mut root,
                    temper_client_domain::Event::Client {
                        event: jig_client::Event::Up {
                            up: jig_client::Up::ReadDone {
                                read: token,
                                result: jig_client::Readout::Jig(contract::ReadResult::Value(page)),
                            },
                        },
                    },
                );
                break;
            }
        }
    }
    root
}

pub mod task;

pub fn signed_out() -> temper_client_domain::Domain {
    let mut root = root_in_project(Some(PROJECT));
    root_event(&mut root, temper_client_domain::Event::SignedOut);
    root
}
pub fn out_of_date() -> temper_client_domain::Domain {
    let mut root = root_in_project(Some(PROJECT));
    root_event(
        &mut root,
        temper_client_domain::Event::Client {
            event: jig_client::Event::Up { up: jig_client::Up::Served { served: false, versions: None } },
        },
    );
    root
}
pub fn main_text(tree: &skein_ui::Tree) -> Box<[u8]> {
    let mut main = None;
    for (index, node) in tree.nodes().iter().enumerate() {
        if node.role == skein_ui::Role::Main {
            let end = index
                .checked_add(usize::try_from(node.size).expect("fixture subtree fits"))
                .expect("fixture index fits");
            main = tree.nodes().get(index..end);
        }
    }
    let main = main.expect("fixture main");
    let mut bytes = 0_usize;
    for node in main {
        for run in &node.text {
            bytes = bytes.checked_add(run.text.len()).expect("fixture text fits");
        }
    }
    let mut writer = Writer::new(bytes);
    for node in main {
        for run in &node.text {
            writer.put(&run.text).expect("main text counted");
        }
    }
    writer.finish()
}

/// Shared chat setup for the focused view stories.
pub mod chat {
    use super as fixtures;
    use jig_client::{contract as c, thread::Thread};
    use skein_lib::{List, Wall};
    use skein_ui::Builder;
    pub const TASK: u64 = 9;
    pub const ATTEMPT: u64 = 23;
    pub const TURN: u32 = 1;
    pub const RETRY: u32 = 0;
    pub const OFFSET: u32 = 0;
    pub const COMMITTED: u32 = 0;
    pub const LINES: u32 = 4;
    pub const BYTES: u64 = 65536;
    pub const TEXT: u32 = 32;
    pub const MESSAGE: u64 = 3;
    pub const PARTY: u64 = 73;
    pub const DELEGATE: u64 = 12;
    pub const OTHER_DELEGATE: u64 = 13;
    pub const CLOSE_GRACE: skein_lib::Duration = skein_lib::Duration::from_secs(5);
    pub const CLOSE_OPERATIONS: u32 = 3;
    pub const FRACTIONAL_GRACE: skein_lib::Duration = skein_lib::Duration::from_millis(125);
    pub const GRACES: [(Option<skein_lib::Duration>, &[u8]); 3] = [
        (Some(CLOSE_GRACE), b"allow 5 seconds to finish, then cancel"),
        (Some(FRACTIONAL_GRACE), b"allow 125000000 nanoseconds to finish, then cancel"),
        (None, b"Close chat now"),
    ];
    pub const CHAT_LABEL: u32 = 7;
    pub const CHAT_TEMPLATE: u32 = 1;
    pub const CHAT_CHARTER: u32 = 1;
    pub const SINGLE_TEMPLATE: u32 = 1;
    pub const EMPTY_PARAMETERS: u32 = 0;
    const MESSAGING_OFFERS: u32 = 2;
    pub const PROPOSAL: c::ProposalKey = c::ProposalKey { proposer: c::Proposer::Task(DELEGATE), number: 3 };
    pub const PROPOSAL_OUTPUTS: u32 = 256;
    pub const PROPOSAL_NOW: skein_lib::Time = skein_lib::Time::ZERO;
    pub const PROPOSAL_OFFERS: u32 = 2;
    pub const EMPTY_BATCH: u32 = 0;

    pub fn chat_header() -> skein_ui::Tree {
        let mut b = Builder::new(&fixtures::tree_limits());
        let mut bindings = crate::frame::Bindings::new(fixtures::tree_limits().bindings);
        crate::pages::chat::main(
            &mut b,
            &chat_root(),
            &fixtures::words(),
            &mut bindings,
            LINES,
            TEXT,
            &mut crate::words::Times::new(fixtures::NOW),
        );
        b.finish()
    }
    pub fn provisional() -> Thread {
        provisional_with_text(b"Hello")
    }
    pub fn provisional_with_text(words: &[u8]) -> Thread {
        let mut thread = Thread::new(TASK, &jig_client::thread::Limits { lines: LINES, bytes: BYTES, text: TEXT });
        let text = c::RunText {
            attempt: ATTEMPT,
            began: Some(Wall::EPOCH),
            turn: c::ProvisionalTurnName { attempt: ATTEMPT, number: TURN, retry: RETRY },
            from: OFFSET,
            through: u32::try_from(words.len()).expect("run text size fits"),
            text: c::Text::new(Box::from(words), u32::try_from(BYTES).expect("byte bound fits")).expect("text"),
            cut: false,
            call: None,
        };
        thread.run(
            &c::RunState { task: TASK, attempt: Some(ATTEMPT), committed_turn: COMMITTED, writing: None, call: None },
            Some(&text),
        );
        thread
    }
    pub fn projected_tree(thread: &Thread) -> skein_ui::Tree {
        drawn_tree(thread, crate::pages::chat::Drawn::Chat { request: None, delegations: None })
    }
    pub fn requested_tree(thread: &Thread, request: &c::SpecificationDocument) -> skein_ui::Tree {
        drawn_tree(thread, crate::pages::chat::Drawn::Chat { request: Some(request), delegations: None })
    }
    pub fn tree(thread: &Thread) -> skein_ui::Tree {
        drawn_tree(thread, crate::pages::chat::Drawn::Lines)
    }
    fn drawn_tree(thread: &Thread, drawn: crate::pages::chat::Drawn<'_>) -> skein_ui::Tree {
        let mut b = Builder::new(&fixtures::tree_limits());
        crate::pages::chat::conversation(
            &mut b,
            thread,
            &fixtures::words(),
            (LINES, TEXT),
            crate::pages::chat::Speakers { person: None, requester: false },
            drawn,
            &mut crate::frame::Bindings::new(fixtures::tree_limits().bindings),
        );
        b.finish()
    }
    pub fn closing(grace: Option<skein_lib::Duration>) -> skein_ui::Tree {
        let mut operations = List::with_capacity(CLOSE_OPERATIONS);
        let reason = c::Text::new(Box::from(*b"Chat closed"), TEXT).expect("reason");
        operations
            .push(c::Operation::Cancel {
                project: fixtures::PROJECT,
                task: DELEGATE,
                reason: reason.clone(),
                grace: None,
            })
            .expect("delegate cancel");
        operations
            .push(c::Operation::Move { project: fixtures::PROJECT, task: OTHER_DELEGATE, reason: reason.clone() })
            .expect("delegate move");
        operations
            .push(c::Operation::Cancel { project: fixtures::PROJECT, task: TASK, reason, grace })
            .expect("chat close");
        let mut b = Builder::new(&fixtures::tree_limits());
        crate::confirm::close_choices(
            &mut b,
            TASK,
            &operations,
            true,
            &fixtures::words(),
            &mut crate::frame::Bindings::new(fixtures::tree_limits().bindings),
        );
        b.finish()
    }
    pub fn chat_root() -> temper_client_domain::Domain {
        chat_offering(false)
    }
    /// `chat_root`'s, with the message act offered beside the close.
    pub fn messaging_root() -> temper_client_domain::Domain {
        chat_offering(true)
    }
    fn chat_offering(message: bool) -> temper_client_domain::Domain {
        use temper_client_domain::{Address, Event};
        let (mut root, me_watch, task_watch) =
            fixtures::root_at(Address::Chat { task: TASK }, &fixtures::domain_limits());
        let mut me = root.pages().me(root.client()).expect("me").clone();
        let member = me.projects.get_mut(EMPTY_PARAMETERS).expect("project");
        member.templates = List::with_capacity(SINGLE_TEMPLATE);
        member
            .templates
            .push(c::Template {
                number: CHAT_TEMPLATE,
                name: c::DocumentText::complete(c::Text::new(Box::from(*b"Chat"), TEXT).expect("name")),
                executor: c::TemplateExecutor::Charter(CHAT_CHARTER),
                contract: c::TaskContractDocument::Report { words: TEXT },
                shallower: SINGLE_TEMPLATE,
                budget: c::TemplateBudget::AsAllowed { cap: None },
                deadline: c::TemplateDeadline::None,
                tracked: None,
                flags: c::TaskFlags {
                    ended_by: jig_client::contract::EndedBy::Result,
                    conversation: true,
                    listed: false,
                    retention: c::Retention::Kept,
                },
                label: CHAT_LABEL,
                grace: Some(CLOSE_GRACE),
                llm: None,
            })
            .expect("template");
        let mut task = fixtures::task::document();
        task.chip.task = TASK;
        task.project = fixtures::PROJECT;
        task.acts = List::with_capacity(if message { MESSAGING_OFFERS } else { SINGLE_TEMPLATE });
        if message {
            task.acts
                .push(c::Offered { act: c::ActKind::Message, admission: c::Admission::Allowed, effect: None })
                .expect("message");
        }
        task.acts
            .push(c::Offered {
                act: c::ActKind::Cancel,
                admission: c::Admission::Allowed,
                effect: Some(c::Effect::Cancel(c::Impact {
                    tasks: c::PhaseCounts {
                        waiting: 0,
                        running: 1,
                        held: 0,
                        closing: 0,
                        ended: 0,
                        done: 0,
                        failed: 0,
                        cancelled: 0,
                    },
                    live_runs: 1,
                    proposals: 0,
                })),
            })
            .expect("close");
        for (watch, document) in [
            (me_watch, c::Delivery::Me(Box::new(me))),
            (task_watch.expect("chat watch"), c::Delivery::Task(Box::new(task))),
        ] {
            fixtures::root_event(
                &mut root,
                Event::Client {
                    event: jig_client::Event::Up {
                        up: jig_client::Up::Watched { watch, event: c::WatchEvent::Delivered(document) },
                    },
                },
            );
        }
        root
    }
    pub fn proposal_root() -> temper_client_domain::Domain {
        proposal_root_with_activity(None)
    }
    pub fn proposal_root_with_activity(activity: Option<c::ActivityPage>) -> temper_client_domain::Domain {
        proposal_root_with_document(activity, None)
    }
    pub fn proposal_root_with_document(
        activity: Option<c::ActivityPage>,
        document: Option<c::ReadValue>,
    ) -> temper_client_domain::Domain {
        use temper_client_domain::Event;
        let mut limits = fixtures::domain_limits();
        limits.client.domain.rows = 4;
        limits.client.domain.turn_text = 512;
        let (mut root, _, watch, _, _) = fixtures::root_at_with_activity(
            temper_client_domain::Address::Chat { task: TASK },
            &limits,
            b"Shop",
            None,
            activity,
            None,
        );
        let mut task = fixtures::task::document();
        task.chip.task = TASK;
        task.proposals = List::with_capacity(SINGLE_TEMPLATE);
        task.proposals.push(PROPOSAL).expect("one proposal");
        let mut out = skein_lib::Queue::with_capacity(PROPOSAL_OUTPUTS);
        temper_client_domain::step(
            &mut root,
            &skein_lib::Env { now: PROPOSAL_NOW, wall: fixtures::NOW, limits },
            Event::Client {
                event: jig_client::Event::Up {
                    up: jig_client::Up::Watched {
                        watch: watch.expect("task watch"),
                        event: c::WatchEvent::Delivered(c::Delivery::Task(Box::new(task))),
                    },
                },
            },
            &mut out,
        );
        let mut acts = List::with_capacity(PROPOSAL_OFFERS);
        for act in [c::ActKind::Accept, c::ActKind::Reject] {
            acts.push(c::Offered { act, admission: c::Admission::Allowed, effect: None }).expect("offer");
        }
        let page = match document {
            Some(page) => page,
            None => c::ReadValue::OwnerProposal(Box::new(c::OwnerProposal {
                key: PROPOSAL,
                project: fixtures::PROJECT,
                reason: c::DocumentText::complete(c::Text::new(Box::from(*b"Make a goal"), TEXT).expect("reason")),
                action: c::OwnerProposalAction::Batch(List::with_capacity(EMPTY_BATCH)),
                as_holder: true,
                waiting: c::OwnerProposalWaiting { holder: c::OwnerHistoryHolder::Person(PARTY), since: Wall::EPOCH },
                funding: List::with_capacity(EMPTY_BATCH),
                acts,
            })),
        };
        let mut answered = false;
        while let Some(request) = out.pop() {
            if let temper_client_domain::Request::Client {
                request:
                    jig_client::Request::Down {
                        down:
                            jig_client::Down::Read {
                                read,
                                what: jig_client::Readable::Jig(c::Read::Proposal { proposal: PROPOSAL }),
                            },
                    },
            } = request
            {
                fixtures::root_event(
                    &mut root,
                    Event::Client {
                        event: jig_client::Event::Up {
                            up: jig_client::Up::ReadDone {
                                read,
                                result: jig_client::Readout::Jig(c::ReadResult::Value(page)),
                            },
                        },
                    },
                );
                answered = true;
                break;
            }
        }
        assert!(answered, "proposal read emitted");
        root
    }
    pub fn rejecting_chat() -> temper_client_domain::Domain {
        use temper_client_domain::{Action, Event, pages::Act};
        let mut root = proposal_root();
        let object = c::ObjectKey::Proposal(PROPOSAL);
        for act in [
            Act::CardWords { object, words: Box::from(*b"Too costly") },
            Act::Decide { object, act: c::ActKind::Reject, revision: None },
        ] {
            fixtures::root_event(&mut root, Event::Act { action: Action::Pages(act) });
        }
        root
    }
}

pub fn chats_root() -> temper_client_domain::Domain {
    root_at(
        temper_client_domain::Address::Chats { filter: temper_client_domain::ChatsFilter::All, start: false },
        &domain_limits(),
    )
    .0
}
/// Act `Input` on the named text area, as a person typing, and return the area's value after the root took it.
pub fn type_into(root: &mut temper_client_domain::Domain, name: &[u8], words: &[u8]) -> Box<[u8]> {
    let mut view = page_view(root);
    let area = find(view.tree(), skein_ui::Role::TextArea, name).expect("text area");
    let action = crate::act(&mut view, area, skein_ui::Act::Input { value: Box::from(words) }).expect("input action");
    root_event(root, temper_client_domain::Event::Act { action });
    let view = page_view(root);
    for node in view.tree().nodes() {
        if node.role == skein_ui::Role::TextArea && node.name.as_deref() == Some(name) {
            return node.value.as_ref().expect("area value").text.clone();
        }
    }
    panic!("text area");
}
/// The styles of each node of `role`, in tree order.
pub fn styles(tree: &skein_ui::Tree, role: skein_ui::Role) -> Box<[Box<[skein_ui::Style]>]> {
    let mut found = List::with_capacity(u32::try_from(tree.nodes().len()).expect("nodes fit"));
    for node in tree.nodes() {
        if node.role == role {
            found.push(Box::from(node.styles.as_slice())).expect("a node per style list");
        }
    }
    found.into_boxed()
}
/// Build a page once for the status-only page tests.
pub fn page_view(root: &temper_client_domain::Domain) -> crate::View {
    let mut view = view();
    build(&mut view, root);
    view
}
/// `page_view`'s, with the browser's words.
pub fn browser_page_view(root: &temper_client_domain::Domain) -> crate::View {
    let mut view = view_of(browser_words());
    build(&mut view, root);
    view
}

fn fixture_me(project_name: &[u8]) -> c::Me {
    use jig_client::contract::{self, DocumentText, HeldRole, Me, MeParty, MeProject, Needs, PartyKind};
    let mut projects = List::with_capacity(2);
    projects
        .push(MeProject {
            project: PROJECT,
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
    second.project = SECOND_PROJECT;
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
pub const DEMO_CUE: &[u8] = b"Change the title";
pub const DEMO_OTHER_CUE: &[u8] = b"Add a test";
pub fn demo_root(address: temper_client_domain::Address, project: u32, model: bool) -> temper_client_domain::Domain {
    demo_choice_root(address, project, model, None)
}
pub fn demo_choice_root(
    address: temper_client_domain::Address,
    project: u32,
    model: bool,
    llm: Option<c::LlmChoice>,
) -> temper_client_domain::Domain {
    demo_frame_root(address, project, model, llm, None)
}
pub fn demo_frame_root(
    address: temper_client_domain::Address,
    project: u32,
    model: bool,
    llm: Option<c::LlmChoice>,
    frame: Option<c::Me>,
) -> temper_client_domain::Domain {
    demo_frame_with_watch(address, project, model, llm, frame).0
}
pub fn demo_frame_with_watch(
    address: temper_client_domain::Address,
    project: u32,
    model: bool,
    llm: Option<c::LlmChoice>,
    frame: Option<c::Me>,
) -> (temper_client_domain::Domain, Token) {
    let (mut root, me_watch, task_watch, _, accounts_watch) =
        root_at_with_demo(address.clone(), &domain_limits(), b"Project", Some((project, model)));
    let template = chat::messaging_root();
    let mut me = match frame {
        Some(me) => me,
        None => template.pages().me(template.client()).expect("chat template").clone(),
    };
    if me.providers.is_empty() {
        me.providers = List::with_capacity(1);
        me.providers.push(demo_provider()).unwrap();
    }
    let mut models = List::with_capacity(2);
    for name in [b"demo".as_slice(), b"selected"] {
        models
            .push(c::MeModel {
                name: c::Text::new(Box::from(name), 32).unwrap(),
                provider: Some(17),
                prices: c::ModelPrices { input: 1, cached: 1, cache_write: None, output: 1, unit: 1000 },
                description: None,
                window: 1024,
                output: 128,
                efforts: List::with_capacity(0),
            })
            .unwrap();
    }
    me.projects.get_mut(0).unwrap().models = models;
    let mut charters = List::with_capacity(1);
    charters.push(1).expect("chat charter");
    me.projects.get_mut(0).expect("demo project").creating = Some(c::Creating {
        within: c::CreateBounds {
            authority: Box::new(c::OwnerAuthorityDocument::complete(c::OwnerAuthority {
                models: None,
                tools: 0,
                grants: List::with_capacity(0),
                delegation: c::OwnerDelegation { kinds: List::with_capacity(0), tasks: 1, depth: 2 },
                spend: 100,
                deadline: None,
                notes: 0,
                note_resources: List::with_capacity(0),
            })),
            pool_left: 100,
            charters,
            procedures: List::with_capacity(0),
            latest: None,
            budget: 100,
            maximum_budget: 100,
            maximum_deadline: None,
        },
        beyond: c::Beyond::Refused(c::Refusal::NotOffered),
    });
    root_event(
        &mut root,
        temper_client_domain::Event::Client {
            event: jig_client::Event::Up {
                up: jig_client::Up::Watched {
                    watch: me_watch,
                    event: jig_client::contract::WatchEvent::Delivered(jig_client::contract::Delivery::Me(Box::new(
                        me,
                    ))),
                },
            },
        },
    );
    if let temper_client_domain::Address::Chat { task: number } = address {
        let mut task = task::document();
        task.chip.task = number;
        task.chip.title = c::DocumentText::complete(c::Text::new(b"Ship the update".as_slice().into(), 32).unwrap());
        task.project = PROJECT;
        task.llm = llm;
        task.kept_account = Some(2);
        task.budget.funder = c::Funder::Pool { project: PROJECT, person: 73, period: 1 };
        task.requester = c::OwnerRequester::Person(root.pages().me(root.client()).unwrap().party.number);
        task.acts = List::with_capacity(1);
        task.acts
            .push(c::Offered { act: c::ActKind::Message, admission: c::Admission::Allowed, effect: None })
            .unwrap();
        root_event(
            &mut root,
            temper_client_domain::Event::Client {
                event: jig_client::Event::Up {
                    up: jig_client::Up::Watched {
                        watch: task_watch.unwrap(),
                        event: c::WatchEvent::Delivered(c::Delivery::Task(Box::new(task))),
                    },
                },
            },
        );
    }
    (root, accounts_watch.unwrap())
}

/// T1 keeps its demo account after another provider's key becomes usable.
pub fn demo_kept_root() -> temper_client_domain::Domain {
    let address = temper_client_domain::Address::Chat { task: 1 };
    let (mut root, me_watch, task_watch, _, account_watch) =
        root_at_with_demo(address.clone(), &domain_limits(), b"Shop", Some((PROJECT, false)));
    let template = demo_root(address, PROJECT, false);
    let mut me = template.pages().me(template.client()).unwrap().clone();
    let mut models = List::with_capacity(3);
    let mut other = me.projects.get(0).unwrap().models.get(0).unwrap().clone();
    other.provider = Some(18);
    other.name = c::Text::new(Box::from(b"other".as_slice()), 32).unwrap();
    models.push(other).unwrap();
    for model in &me.projects.get(0).unwrap().models {
        models.push(model.clone()).unwrap();
    }
    me.projects.get_mut(0).unwrap().models = models;
    let mut doc = temper_client_domain::pages::accounts::document(root.client()).unwrap().clone();
    let mut rows = List::with_capacity(2);
    let mut other = doc.accounts.get(1).unwrap().clone();
    other.sources = doc.accounts.get(0).unwrap().sources.clone();
    other.sources.get_mut(0).unwrap().kind = c::OwnerAccountKind::Key;
    rows.push(other).unwrap();
    rows.push(doc.accounts.get(0).unwrap().clone()).unwrap();
    doc.accounts = rows;
    let mut readiness = List::with_capacity(2);
    readiness
        .push(c::PayerReadiness {
            provider: 18,
            payer: c::OwnerAccountUse::Party(73),
            readiness: c::Readiness::Usable,
            settled: Some(true),
        })
        .unwrap();
    readiness.push(*doc.readiness.get(0).unwrap()).unwrap();
    doc.readiness = readiness;
    let mut task = template.pages().chat().unwrap().task(template.client()).unwrap().clone();
    task.llm = Some(c::LlmChoice {
        account: None,
        main: c::ModelEffort { model: c::Text::new(Box::from(b"demo".as_slice()), 32).unwrap(), effort: None },
        subagents: List::with_capacity(0),
    });
    for (watch, value) in [
        (me_watch, c::Delivery::Me(Box::new(me))),
        (account_watch.unwrap(), c::Delivery::OwnerAccounts(Box::new(doc))),
        (task_watch.unwrap(), c::Delivery::Task(Box::new(task))),
    ] {
        root_event(
            &mut root,
            temper_client_domain::Event::Client {
                event: jig_client::Event::Up {
                    up: jig_client::Up::Watched { watch, event: c::WatchEvent::Delivered(value) },
                },
            },
        );
    }
    root
}

pub fn hidden_demo_root(ready: Option<c::Readiness>, payer: u64) -> temper_client_domain::Domain {
    let address = temper_client_domain::Address::Chat { task: 1 };
    let (mut root, me_watch, task_watch, _, account_watch) =
        root_at_with_demo(address.clone(), &domain_limits(), b"Shop", Some((PROJECT, true)));
    let template = demo_root(address, PROJECT, true);
    let me = template.pages().me(template.client()).unwrap().clone();
    let mut task = template.pages().chat().unwrap().task(template.client()).unwrap().clone();
    task.llm = Some(c::LlmChoice {
        account: None,
        main: c::ModelEffort { model: c::Text::new(Box::from(b"demo".as_slice()), 32).unwrap(), effort: None },
        subagents: List::with_capacity(0),
    });
    task.budget.funder = c::Funder::Pool { project: PROJECT, person: payer, period: 1 };
    let mut doc = demo_accounts(true);
    doc.accounts.clear();
    match ready {
        Some(ready) => doc.readiness.get_mut(0).unwrap().readiness = ready,
        None => doc.readiness.clear(),
    }
    for (watch, value) in [
        (me_watch, c::Delivery::Me(Box::new(me))),
        (task_watch.unwrap(), c::Delivery::Task(Box::new(task))),
        (account_watch.unwrap(), c::Delivery::OwnerAccounts(Box::new(doc))),
    ] {
        root_event(
            &mut root,
            temper_client_domain::Event::Client {
                event: jig_client::Event::Up {
                    up: jig_client::Up::Watched { watch, event: c::WatchEvent::Delivered(value) },
                },
            },
        );
    }
    root
}

fn demo_provider() -> c::ProviderOffer {
    c::ProviderOffer {
        provider: 17,
        name: c::DocumentText::complete(c::Text::new(Box::from(b"demo".as_slice()), 32).unwrap()),
        key_bytes: 4096,
        sign_in: false,
        key: true,
        programs: List::with_capacity(0),
        programs_cut: false,
        none: false,
        signed_in_plan: false,
        manage_usage: None,
    }
}

fn demo_accounts(model: bool) -> c::OwnerAccounts {
    let mut rows = List::with_capacity(2);
    let mut sources = List::with_capacity(1);
    sources
        .push(c::OwnerAccountSource {
            name: None,
            place: 1,
            number: 2,
            kind: c::OwnerAccountKind::None,
            state: c::OwnerAccountState::Granted,
            usable: true,
            expiring: false,
            attention: false,
            spent_until: None,
        })
        .unwrap();
    rows.push(c::OwnerAccountLine {
        number: 2,
        place: 1,
        uses: None,
        plan: false,
        provider: Some(17),
        provider_name: None,
        manage_usage: None,
        display: c::DocumentText::complete(c::Text::new(Box::from(b"Demo".as_slice()), 32).unwrap()),
        holder: c::OwnerAccountHolder::Deployment,
        sources,
        cut: false,
    })
    .unwrap();
    if !model {
        rows.push(c::OwnerAccountLine {
            number: 1,
            place: 1,
            uses: None,
            plan: false,
            provider: Some(18),
            provider_name: None,
            manage_usage: None,
            display: c::DocumentText::complete(c::Text::new(b"Other".as_slice().into(), 32).unwrap()),
            holder: c::OwnerAccountHolder::Party(73),
            sources: List::with_capacity(0),
            cut: true,
        })
        .unwrap();
    }
    let mut readiness = List::with_capacity(1);
    readiness
        .push(c::PayerReadiness {
            provider: 17,
            payer: c::OwnerAccountUse::Party(73),
            readiness: c::Readiness::Usable,
            settled: Some(true),
        })
        .unwrap();
    c::OwnerAccounts {
        program_hosts: List::with_capacity(0),
        readiness,
        readiness_cut: false,
        accounts: rows,
        signing: List::with_capacity(0),
        last_end: None,
        usable: true,
        settled: true,
        cut: false,
    }
}

fn deliver_demo(
    root: &mut temper_client_domain::Domain,
    read: Token,
    watch: Token,
    project: u32,
    model: bool,
    state: w::DemoState,
) {
    let mut cues = List::with_capacity(2);
    cues.push(Box::from(DEMO_CUE)).expect("first cue");
    cues.push(Box::from(DEMO_OTHER_CUE)).expect("second cue");
    let answer = w::DemoAnswer::new(
        &w::CEILINGS,
        w::DemoAnswerParts {
            state,
            project,
            forge: wire::FORGE,
            provider: 17,
            owner: b"acme".as_slice().into(),
            name: b"shop".as_slice().into(),
            cues,
        },
    )
    .expect("demo answer");
    root_event(
        root,
        temper_client_domain::Event::Client {
            event: jig_client::Event::Up {
                up: jig_client::Up::ReadDone {
                    read,
                    result: jig_client::Readout::Family {
                        family: wire::FORGE,
                        record: Some(wire::demo_answer(&answer)),
                    },
                },
            },
        },
    );
    let accounts = demo_accounts(model);
    root_event(
        root,
        temper_client_domain::Event::Client {
            event: jig_client::Event::Up {
                up: jig_client::Up::Watched {
                    watch,
                    event: c::WatchEvent::Delivered(c::Delivery::OwnerAccounts(Box::new(accounts))),
                },
            },
        },
    );
}

pub mod panel;

/// First-run facts answered, with or without the deployment's demo.
pub fn welcome_root(step: temper_client_domain::Step, demo: bool) -> temper_client_domain::Domain {
    let address = temper_client_domain::Address::Welcome { step };
    root_at_with_demo(address, &domain_limits(), b"Shop", if demo { Some((PROJECT, true)) } else { None }).0
}

/// A flow's rows delivered through the session watch.
pub fn forge_root(address: temper_client_domain::Address, rows: List<w::ForgeRow>) -> temper_client_domain::Domain {
    forge_root_accounts(address, rows).0
}

pub fn forge_root_accounts(
    address: temper_client_domain::Address,
    rows: List<w::ForgeRow>,
) -> (temper_client_domain::Domain, Token) {
    let (root, _, _, _, accounts) = root_at_with_activity(address, &domain_limits(), b"Shop", None, None, Some(rows));
    (root, accounts.unwrap())
}
pub fn welcome_account(root: &mut temper_client_domain::Domain, watch: Token) {
    root_event(
        root,
        temper_client_domain::Event::Client {
            event: jig_client::Event::Up {
                up: jig_client::Up::Watched {
                    watch,
                    event: c::WatchEvent::Delivered(c::Delivery::OwnerAccounts(Box::new(demo_accounts(true)))),
                },
            },
        },
    );
}

/// A demo and explicit forge rows share the session watch.
pub fn demo_forges_root(
    address: temper_client_domain::Address,
    rows: List<w::ForgeRow>,
) -> temper_client_domain::Domain {
    root_at_with_activity(
        address,
        &domain_limits(),
        b"Shop",
        Some((PROJECT, true, w::DemoState::Running)),
        None,
        Some(rows),
    )
    .0
}

pub fn demo_state_root(address: temper_client_domain::Address, state: w::DemoState) -> temper_client_domain::Domain {
    root_at_with_activity(address, &domain_limits(), b"Shop", Some((PROJECT, true, state)), None, None).0
}
pub fn demo_start_attempt(root: &mut temper_client_domain::Domain) -> Token {
    let limits = domain_limits();
    let mut out = Queue::with_capacity(temper_client_domain::max_out(&limits));
    temper_client_domain::step(
        root,
        &Env { now: Time::ZERO, wall: NOW, limits },
        temper_client_domain::Event::Act {
            action: temper_client_domain::Action::Pages(temper_client_domain::pages::Act::Playground),
        },
        &mut out,
    );
    while let Some(request) = out.pop() {
        if let temper_client_domain::Request::Client {
            request:
                jig_client::Request::Down {
                    down: jig_client::Down::Send { attempt, request: jig_client::Ask::Family { record, .. }, .. },
                },
        } = request
            && *wire::decode_requesting(&record.bytes, &w::CEILINGS).unwrap().request() == w::Request::StartDemo
        {
            return attempt;
        }
    }
    panic!("demo start sent");
}
pub fn demo_start_answer(root: &mut temper_client_domain::Domain, attempt: Token, outcome: w::Outcome) {
    root_event(
        root,
        temper_client_domain::Event::Client {
            event: jig_client::Event::Up {
                up: jig_client::Up::Answer {
                    attempt,
                    envelope: skein_lib::RequestEnvelope::Final,
                    why: None,
                    answer: Some(jig_client::Answered::Family(jig_client::Family {
                        family: wire::FORGE,
                        version: wire::CLIENT_VERSION,
                        bytes: wire::answer(&w::Answer::new(&w::CEILINGS, w::AnswerParts { outcome }).unwrap()),
                    })),
                },
            },
        },
    );
}

/// A forge flow that can receive subsequent watch deliveries.
pub fn forge_root_watch(
    address: temper_client_domain::Address,
    rows: List<w::ForgeRow>,
) -> (temper_client_domain::Domain, Token) {
    let (root, _, _, _, _, watch) = root_at_with_forges(address, &domain_limits(), b"Shop", None, None, Some(rows));
    (root, watch.expect("forges watch"))
}

/// Page-advertised sign-in ways, in either shell.
pub fn sign_in_view(
    root: &temper_client_domain::Domain,
    shell: crate::words::Shell,
    ways: crate::sign_in::Ways,
) -> crate::View {
    let domain = domain_limits();
    let limits = crate::Limits { tree: tree_limits() };
    let words = crate::words::Words::english(FORGE, domain.client.domain.text, shell).unwrap();
    let mut view = crate::View::with_sign_in(&limits, &domain, words, FORGE, ways);
    build(&mut view, root);
    view
}

pub fn pair(
    first: temper_client_domain::Domain,
    second: temper_client_domain::Domain,
) -> Box<[temper_client_domain::Domain]> {
    let mut roots = List::with_capacity(2);
    roots.push(first).expect("first root");
    roots.push(second).expect("second root");
    roots.into_boxed()
}
