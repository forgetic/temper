use crate::jig::key::Name;
use skein_lib::List;
use std::path::PathBuf;
/// Source files under the extraction boundary; capacity is a source-tree bound.
const SOURCE_FILES: u32 = 128;
pub(crate) fn name(bytes: &[u8]) -> Name {
    Name::new(bytes).expect("a fixture key name")
}
pub(crate) fn sources() -> List<(PathBuf, Box<str>)> {
    let mut paths = List::with_capacity(SOURCE_FILES);
    let mut sources = List::with_capacity(SOURCE_FILES);
    paths.push(PathBuf::from("src/jig.rs")).expect("module root");
    paths.push(PathBuf::from("src/jig")).expect("module directory");
    for index in 0..SOURCE_FILES {
        let Some(path) = paths.get(index).cloned() else {
            break;
        };
        if path.is_dir() {
            for entry in std::fs::read_dir(path).expect("read source directory") {
                paths.push(entry.expect("source entry").path()).expect("bounded sources");
            }
        } else if path.extension() == Some(std::ffi::OsStr::new("rs")) {
            let text = std::fs::read_to_string(&path).expect("read source").into_boxed_str();
            sources.push((path, text)).expect("bounded sources");
        }
    }
    sources
}
/// Enough words for every published call, including repeated checks and moves.
const WORDS: u32 = 128;
pub(crate) fn parse(words: &[&str]) -> Result<crate::jig::call::Parsed, crate::jig::argv::Usage> {
    let mut bytes = List::with_capacity(WORDS);
    for word in words {
        bytes.push(word.as_bytes()).expect("bounded words");
    }
    crate::table::parse(bytes.as_slice())
}
pub(crate) fn call(words: &[&str]) -> crate::jig::call::Call {
    let crate::jig::call::Parsed::Call { call, json: false } = parse(words).expect("fixture argv") else {
        panic!("a plain call");
    };
    call
}
pub(crate) fn back(call: &crate::jig::call::Call) -> Box<[Box<[u8]>]> {
    let mut words = List::with_capacity(WORDS);
    crate::jig::argv::back(crate::table::TOOLS, call, &mut words).expect("a call with argv");
    words.into_boxed()
}
pub(crate) fn owned(words: &[&str]) -> Box<[Box<[u8]>]> {
    let mut owned = List::with_capacity(WORDS);
    for word in words {
        owned.push(Box::from(word.as_bytes())).expect("bounded words");
    }
    owned.into_boxed()
}
/// One argv per table row and form; the help derivation shares these examples.
pub(crate) const EVERY_TOOL: &[&[&str]] = &[
    &["me"],
    &["inbox"],
    &["tasks", "--project", "shop"],
    &["task", "41"],
    &["chat", "41"],
    &["activity", "41"],
    &["turn", "41", "3.7"],
    &["run", "41"],
    CREATE_CHAT_CALL,
    &["message", "41", "hello", "--key", "say-hello"],
    CLOSE_CALL,
    &["answer", "41", "3", "yes"],
    &["decide", "t41/3", "accept"],
    &["decide", "41", "release", "--hold", "2"],
    &["release", "41", "--hold", "2"],
    STOP_CALL,
    &["cancel", "41", "finished"],
    &["wait", "me"],
    ADOPTION,
    &["sign-in", "-"],
    &["sign-out"],
    &["pending"],
    &["help"],
    &["accounts"],
    &["models"],
    &["connect", "9", "--key", "connect"],
    &["disconnect", "43"],
    &["add-key", "9", "key-file", "--key", "add"],
    &["replace-key", "43", "key-file"],
    &["forget", "43"],
    &["rank", "43", "1"],
    &["retry", "43"],
    &["amend", "41", "--rev", "7", "--model", "chosen-model", "because"],
    &["declare", "9", "helper", "work-login", "--key", "declare"],
    &["verify", "43"],
    &["forges"],
    &["forge-setup", "3"],
    &["demo-start"],
    &["forge-connect", "3", "github-app", "4242", "app.pem", "--key", "app"],
    &["forge-connect", "3", "github", "--key", "app"],
    &["forge-connect", "3", "forgejo", "tokens", "--key", "app"],
    &["forge-connect", "3", "forgejo-admin", "token", "--key", "admin"],
    &["forge-install", "3"],
    &["repositories", "3"],
    &["wait", "forges"],
    &["export", "43", "record"],
    &["import", "record", "--key", "move-in"],
];

/// Parse the printer's bytes back without re-creating argv in each test.
pub(crate) fn printed(call: &crate::jig::call::Call) -> crate::jig::call::Parsed {
    let words = back(call);
    let mut borrowed = List::with_capacity(WORDS);
    for word in &words {
        borrowed.push(word.as_ref()).expect("bounded words");
    }
    crate::table::parse(borrowed.as_slice()).expect("printed argv")
}
/// The common adoption form; optional flags stay absent.
pub(crate) fn adoption() -> crate::jig::call::Call {
    call(ADOPTION)
}

/// Required adoption fields and its named key, shared by form tests.
pub(crate) const ADOPTION: &[&str] =
    &["adopt", "--project", "shop", "--owner", "o", "--name", "n", "--landing", "main", "--key", "adopt-repo"];

pub(crate) fn from_json(bytes: &[u8]) -> crate::jig::call::Call {
    json(bytes).expect("fixture JSON call")
}
pub(crate) fn json(bytes: &[u8]) -> Result<crate::jig::call::Call, crate::jig::argv::Usage> {
    crate::jig::json::call(
        crate::table::TOOLS,
        bytes,
        crate::jig::call::Source::Path(Box::from(b"call.json".as_slice())),
        &crate::limits::tools(),
    )
}
/// Shared JSON wait, deliberately putting the condition before the subject.
pub(crate) fn json_wait(subject: &str, until: Option<&str>) -> Box<[u8]> {
    let limits = skein_json::writer::Limits { depth: WAIT_JSON_DEPTH, length: crate::limits::TEXT };
    let mut measured = skein_json::writer::Encoder::measure(&limits);
    encode_wait(&mut measured, subject, until);
    let length = measured.measured().expect("fixture JSON size");
    let mut output = skein_json::writer::Encoder::write(length, &limits);
    encode_wait(&mut output, subject, until);
    output.finish()
}
fn encode_wait(out: &mut skein_json::writer::Encoder, subject: &str, until: Option<&str>) {
    out.object_start();
    out.key(b"tool");
    out.string(b"wait");
    out.key(b"arguments");
    out.object_start();
    if let Some(until) = until {
        out.key(b"until");
        out.string(until.as_bytes());
    }
    out.key(b"since");
    out.string(b"0011223344556677");
    out.key(b"subject");
    out.string(subject.as_bytes());
    out.object_end();
    out.object_end();
}
pub(crate) fn wait_call(subject: &str, until: Option<&str>) -> crate::jig::call::Call {
    let mut words = List::with_capacity(WORDS);
    words.push(b"wait".as_slice()).expect("bounded words");
    for word in subject.split_ascii_whitespace() {
        words.push(word.as_bytes()).expect("bounded words");
    }
    words.push(b"--since".as_slice()).expect("bounded words");
    words.push(b"0011223344556677".as_slice()).expect("bounded words");
    if let Some(until) = until {
        words.push(b"--until".as_slice()).expect("bounded words");
        words.push(until.as_bytes()).expect("bounded words");
    }
    let crate::jig::call::Parsed::Call { call, json: false } =
        crate::table::parse(words.as_slice()).expect("wait argv")
    else {
        panic!("a plain wait call");
    };
    call
}

use crate::root;
use alloc::collections::BTreeMap;
use jig_client::{self as client, contract as c};
use skein_lib::{Env, Queue, Time, Token, Wall};
/// Distinct from the other party in recovery tests.
pub(crate) const PARTY: u64 = 73;
/// The first goal read throughout this session.
pub(crate) const TASK: u64 = 41;
/// A second task makes call correlation observable out of order.
pub(crate) const OTHER_TASK: u64 = TASK + 1;
/// Shared argv keeps the first task's identity in one fixture.
pub(crate) const TASK_CALL: &[&str] = &["task", "41"];
/// A second concurrent read must have another document identity.
pub(crate) const OTHER_TASK_CALL: &[&str] = &["task", "42"];
/// The live frame read in both output forms.
pub(crate) const ME_CALL: &[&str] = &["me"];
/// A fixed deterministic client seed, independent of every call's prefix.
pub(crate) const SEED: u64 = 11;
/// One project suffices to resolve its supplied name.
pub(crate) const PROJECT: u32 = 1;
/// One project is required to resolve its supplied name.
pub(crate) const PROJECTS: u32 = 1;
/// Empty projections have no entries to exercise; growing this adds unrelated data.
pub(crate) const EMPTY: u32 = 0;
/// The wait arguments nest one object inside the result object.
const WAIT_JSON_DEPTH: u32 = 2;
/// Two offers establish preserved document order.
pub(crate) const OFFERS: u32 = 2;
/// Root and client effects plus one answer per admitted call.
pub(crate) const OUTPUTS: u32 = 256;
/// An older prefix must be skipped when new work starts.
pub(crate) const RESTORED_PREFIX: u32 = 7;
/// Deliberately distinct from every allocated call prefix.
pub(crate) const CORRELATION: u32 = 97;
/// One record restores the high-water mark without filling the request table.
pub(crate) const KEPT: u32 = 1;
/// Leave enough characters beyond the shipped text limit to exercise a cut.
pub(crate) const LONG_WORDS: u32 = crate::limits::TEXT + 1024;
/// The same frame is read in JSON without repeating its command.
pub(crate) const ME_JSON_CALL: &[&str] = &[ME_CALL[0], "--json"];
pub(crate) fn limits_root() -> root::Limits {
    root::Limits { domain: crate::limits::client(), tools: crate::limits::tools() }
}
pub(crate) fn me(party: u64) -> c::Me {
    let mut projects = List::with_capacity(PROJECTS);
    projects
        .push(c::MeProject {
            project: PROJECT,
            name: document_text(b"shop"),
            role: c::HeldRole::Owner,
            pool: None,
            templates: List::with_capacity(EMPTY),
            creating: None,
            models: List::with_capacity(EMPTY),
        })
        .expect("one project");
    c::Me {
        providers: List::with_capacity(0),
        providers_cut: false,
        key_bytes: 256,
        redirect_bytes: 256,
        party: c::MeParty {
            number: party,
            kind: c::PartyKind::Service,
            login: document_text(b"pat"),
            name: document_text(b"Pat"),
        },
        projects,
        needs: c::Needs { questions: 1, proposals: 0, escalations: 0, person_tasks: 0 },
        needs_total: 1,
        updates: c::Count { value: 0, limited: false },
        results_seen: 0,
        cut: false,
        retention: skein_lib::Duration::from_secs(1000),
        deployment_role: None,
    }
}
pub(crate) fn document_text(bytes: &[u8]) -> c::DocumentText {
    c::DocumentText::complete(c::Text::new(Box::from(bytes), LONG_WORDS).expect("fixture text fits"))
}
pub(crate) fn waiting_on_me() -> c::WaitsOn {
    c::WaitsOn::Party { whom: c::Whom::Person(PARTY), entries: 1, task: OTHER_TASK, kind: c::WaitedFor::Question }
}
pub(crate) fn goal(number: u64, waits_on: c::WaitsOn) -> c::Task {
    use jig_client_protocol_wire::v19 as w;
    let record =
        w::Task::new(&w::CEILINGS, jig_client_test_records::task_record().into_parts()).expect("record fixture");
    let limits = jig_client_protocol_link::Limits::new(
        crate::limits::client().client.domain,
        crate::limits::client().client.exchanges,
    )
    .expect("shipped link limits");
    let mut task = jig_client_protocol_link::translate::documents::task(record, &limits).expect("document fixture");
    task.chip.task = number;
    task.goal = Some(number);
    task.flags.conversation = false;
    task.chip.phase = c::Phase::Running;
    task.priority = Some(2);
    task.project = PROJECT;
    task.requester = c::OwnerRequester::Person(PARTY);
    task.spec.specification.words = document_text(b"Add a /health endpoint that reports the store's state");
    task.budget.numbers.spent = 120;
    task.budget.numbers.budget = 500;
    task.budget.deadline = None;
    task.waits_on = waits_on;
    task.context = None;
    task.acts = List::with_capacity(OFFERS);
    for act in [c::ActKind::Stop, c::ActKind::Cancel] {
        task.acts.push(c::Offered { act, admission: c::Admission::Allowed, effect: None }).expect("two ordered offers");
    }
    task
}
pub(crate) fn task_result(task: c::Task) -> c::ReadResult {
    c::ReadResult::Value(c::ReadValue::Task(Box::new(task)))
}
pub(crate) fn historical_task_result(task: c::Task, ending: c::Ending) -> c::ReadResult {
    let result = c::Result {
        task: task.chip.clone(),
        ending,
        value: Some(c::ResultValue::Report(document_text(b"Final task report"))),
        at: None,
        details: None,
        acts: List::with_capacity(EMPTY),
    };
    c::ReadResult::Value(c::ReadValue::Historical(Box::new(c::OwnerHistorical {
        key: c::ObjectKey::Task(task.chip.task),
        value: c::OwnerHistoricalValue::Task { document: Box::new(task), result: Box::new(result) },
        at: None,
        by: None,
    })))
}
pub(crate) fn golden(name: &str) -> Box<[u8]> {
    let path = std::path::Path::new("src/tests/golden").join(name).with_extension("txt");
    std::fs::read(path).expect("checked golden").into_boxed_slice()
}
pub(crate) struct Harness {
    pub root: root::Root,
    pub now: Time,
    pub wall: Wall,
    pub limits: root::Limits,
    pub reads: BTreeMap<u32, (Token, c::Read)>,
    pub extra_reads: BTreeMap<u32, Queue<(Token, c::Read)>>,
    pub serving: Option<u32>,
    pub closed: List<client::Watchable>,
    pub watches: BTreeMap<Token, client::Watchable>,
    pub answers: BTreeMap<u32, crate::jig::Printed>,
    pub sends: BTreeMap<u32, (Token, skein_lib::RequestKey, client::Ask)>,
    pub carried: BTreeMap<u32, skein_lib::Secret>,
    pub records: BTreeMap<u32, skein_lib::Secret>,
    pub saved: BTreeMap<u32, skein_lib::RequestKey>,
    pub kept_lines: List<Box<[u8]>>,
    pub progress: List<Box<[u8]>>,
    pub facts: u32,
    pub next: u32,
    pub pending_reads: Queue<u32>,
}
pub(crate) fn started(kept: List<skein_lib::RequestRecord<client::Held>>) -> Harness {
    started_with(kept, SEED, false)
}
pub(crate) fn started_with(kept: List<skein_lib::RequestRecord<client::Held>>, seed: u64, json: bool) -> Harness {
    started_with_limits(kept, seed, json, limits_root())
}
pub(crate) fn started_with_limits(
    kept: List<skein_lib::RequestRecord<client::Held>>,
    seed: u64,
    json: bool,
    limits: root::Limits,
) -> Harness {
    let mut root = root::Root::unlinked(&limits, seed).expect("shipped client limits");
    root.kept_json(json);
    let mut harness = Harness {
        root,
        now: Time::ZERO,
        wall: Wall::EPOCH,
        limits,
        reads: BTreeMap::new(),
        extra_reads: BTreeMap::new(),
        serving: None,
        closed: List::with_capacity(OUTPUTS),
        watches: BTreeMap::new(),
        answers: BTreeMap::new(),
        sends: BTreeMap::new(),
        saved: BTreeMap::new(),
        carried: BTreeMap::new(),
        records: BTreeMap::new(),
        progress: List::with_capacity(OUTPUTS),
        kept_lines: List::with_capacity(OUTPUTS),
        facts: 0,
        next: 0,
        pending_reads: Queue::with_capacity(OUTPUTS),
    };
    harness.client(client::Event::StartUnlinked { kept });
    harness.client(client::Event::SignIn { with: client::With::Transport });
    harness.client(client::Event::Up { up: client::Up::Served { served: true, versions: None } });
    harness
}
pub(crate) fn signed_in(party: u64) -> Harness {
    let mut harness = started(List::with_capacity(EMPTY));
    harness.deliver(c::Delivery::Me(Box::new(me(party))));
    harness
}
impl Harness {
    fn event(&mut self, event: root::Event) {
        let env = Env {
            now: self.now,
            wall: self.wall,
            limits: root::Limits { domain: self.limits.domain, tools: self.limits.tools },
        };
        let mut out = Queue::with_capacity(OUTPUTS);
        root::step(&mut self.root, &env, event, &mut out);
        self.outputs(out);
    }
    pub(crate) fn outputs(&mut self, mut out: Queue<root::Request>) {
        for _ in 0..out.len() {
            match out.pop().expect("counted root output") {
                root::Request::Client {
                    request:
                        client::Request::Down { down: client::Down::Read { read, what: client::Readable::Jig(what) } },
                } => {
                    let call = match self.serving {
                        Some(call) => call,
                        None => self.pending_reads.pop().expect("submitted read"),
                    };
                    if let alloc::collections::btree_map::Entry::Vacant(e) = self.reads.entry(call) {
                        e.insert((read, what));
                    } else {
                        self.extra_reads.entry(call).or_insert(Queue::with_capacity(OUTPUTS));
                        self.extra_reads.get_mut(&call).expect("read queue").push((read, what));
                    }
                }
                root::Request::Client {
                    request: client::Request::Down { down: client::Down::Open { watch, subject } },
                } => {
                    self.watches.insert(watch, subject);
                }
                root::Request::Client { request: client::Request::Down { down: client::Down::Close { watch } } } => {
                    let subject = self.watches.remove(&watch).expect("open watch closes");
                    self.closed.push(subject).expect("bounded closes");
                    self.client(client::Event::Up { up: client::Up::WatchEnded { watch, end: c::WatchEnd::Closed } });
                }
                root::Request::Printed { number, printed } => {
                    assert!(self.answers.insert(number, printed).is_none(), "answered exactly once");
                }
                root::Request::Client {
                    request: client::Request::Down { down: client::Down::Send { attempt, key, request, value } },
                } => {
                    let call = self.serving.unwrap_or(self.next);
                    self.sends.insert(call, (attempt, key, request));
                    if let Some(value) = value {
                        self.carried.insert(call, value);
                    }
                }
                root::Request::Client { request: client::Request::Down { down: client::Down::Save { record } } } => {
                    let call = self.serving.unwrap_or(self.next);
                    self.saved.insert(call, record.key);
                }
                root::Request::Progress { line } => self.progress.push(line).expect("bounded progress lines"),
                root::Request::Kept { line } => {
                    self.kept_lines.push(line).expect("bounded recovery lines");
                }
                root::Request::Fact { .. } => self.facts = self.facts.checked_add(1).expect("bounded facts"),
                root::Request::Record { number, record } => {
                    self.records.insert(number, record);
                }
                root::Request::Client { .. } | root::Request::Ready => {}
            }
        }
    }
    pub fn client(&mut self, event: client::Event) {
        self.event(root::Event::Client { event });
    }
    pub fn submit(&mut self, words: &[&str]) -> u32 {
        self.submit_value(words, None)
    }
    pub fn submit_value(&mut self, words: &[&str], value: Option<skein_lib::Secret>) -> u32 {
        let crate::jig::call::Parsed::Call { call, json } = parse(words).expect("fixture call") else {
            panic!("parsed call")
        };
        self.submit_prepared(call, json, value)
    }
    pub fn submit_prepared(
        &mut self,
        call: crate::jig::call::Call,
        json: bool,
        value: Option<skein_lib::Secret>,
    ) -> u32 {
        self.next = self.next.checked_add(1).expect("fixture calls fit");
        let count = match call.tool {
            1 | 2 | 4 | 5 | 6 | 9..=15 | 31 => SINGLE_READ,
            3 => {
                if crate::jig::call::number(&call, 2).is_some() {
                    COMPOSITE_READS
                } else {
                    SINGLE_READ
                }
            }
            _ => 0,
        };
        for _ in 0..count {
            self.pending_reads.push(self.next);
        }
        self.event(root::Event::Call { number: self.next, call, json, value });
        self.next
    }
    pub fn deliver(&mut self, delivery: c::Delivery) {
        let mut watches = List::with_capacity(OUTPUTS);
        for (token, subject) in &self.watches {
            if *subject == client::Watchable::Jig(c::Subject::Me) {
                watches.push(*token).expect("bounded me watches");
            }
        }
        for token in &watches {
            self.client(client::Event::Up {
                up: client::Up::Watched { watch: *token, event: c::WatchEvent::Delivered(delivery.clone()) },
            });
        }
    }
    pub fn serve(&mut self, call: u32, result: c::ReadResult) {
        self.serve_readout(call, client::Readout::Jig(result));
    }
    pub fn serve_readout(&mut self, call: u32, result: client::Readout) {
        let (token, _) = self.reads.remove(&call).expect("the call read once");
        if let Some(reads) = self.extra_reads.get_mut(&call)
            && let Some(next) = reads.pop()
        {
            self.reads.insert(call, next);
        }
        self.serving = Some(call);
        self.client(client::Event::Up { up: client::Up::ReadDone { read: token, result } });
        self.serving = None;
    }
    pub fn answer(&mut self, call: u32) -> Option<crate::jig::Printed> {
        self.answers.remove(&call)
    }
}
pub(crate) fn kept(prefix: u32) -> List<skein_lib::RequestRecord<client::Held>> {
    let mut kept = List::with_capacity(KEPT);
    kept.push(skein_lib::RequestRecord {
        key: KEPT_KEY,
        scope: PARTY,
        first_sent: None,
        request: client::Held {
            key: client::Key { asker: root::RUNNER.asker(), number: (u64::from(prefix) << 32u32) | 1 },
            request: client::Ask::Jig(Box::new(c::Ask::Stop { project: PROJECT, task: TASK })),
        },
    })
    .expect("one kept request");
    kept
}

/// A nonzero delegation depth makes that authority field observable.
const AUTHORITY_DEPTH: u32 = 1;
/// One entry exercises each independently bounded authority collection.
const AUTHORITY_ENTRIES: u32 = 1;
pub(crate) fn bounded_offer(beyond: c::Beyond) -> c::Admission {
    let authority = c::OwnerAuthorityDocument::complete(c::OwnerAuthority {
        models: None,
        tools: 5,
        grants: List::with_capacity(EMPTY),
        delegation: c::OwnerDelegation { kinds: List::with_capacity(EMPTY), tasks: 3, depth: AUTHORITY_DEPTH },
        spend: 17,
        deadline: None,
        notes: 2,
        note_resources: List::with_capacity(EMPTY),
    });
    let mut grants = List::with_capacity(AUTHORITY_ENTRIES);
    grants
        .push(c::OwnerGrant {
            connector: 4,
            kind: 2,
            pattern: c::OwnerPattern {
                segments: List::with_capacity(EMPTY),
                last: c::OwnerPatternEnd::Open(Box::from(b"src".as_slice())),
            },
        })
        .expect("one held grant");
    c::Admission::Bounded {
        within: c::Bounds::Amend {
            budget_left: 19,
            grants,
            implications: List::with_capacity(EMPTY),
            authority: Box::new(authority.clone()),
            ceiling: Box::new(authority),
        },
        beyond,
    }
}

/// Two path segments are needed to exercise joining without cutting either.
const PATH_SEGMENTS: u32 = 2;
/// One resource suffices to route an owner part through the application's words.
const RESOURCES: u32 = 1;
pub(crate) fn long_resource(task: &mut c::Task) {
    let path = long_path();
    task.resources = List::with_capacity(RESOURCES);
    task.resources
        .push(c::TaskResource::Owner(Box::new(c::OwnerResourceLine {
            part: c::OwnerPart { connector: 4, kind: c::OwnerPartKind::Resource, key: c::OwnerPartKey::Path(path) },
            kind: 2,
            taken: false,
            holder: None,
            writer: None,
            writer_lost: false,
        })))
        .expect("one owner resource");
}

pub(crate) fn long_path() -> List<Box<[u8]>> {
    let mut words = skein_lib::Writer::new(usize::try_from(LONG_WORDS).expect("fixture size"));
    for _ in 0..LONG_WORDS {
        words.put(b"x").expect("fixture length");
    }
    let mut path = List::with_capacity(PATH_SEGMENTS);
    path.push(Box::from(b"src".as_slice())).expect("first segment");
    path.push(words.finish()).expect("long segment");
    path
}

/// Start one read of the common waiting goal before its answer is supplied.
pub(crate) fn task_read() -> (Harness, u32, c::Task) {
    let (root, call) = read_call(TASK_CALL);
    (root, call, goal(TASK, waiting_on_me()))
}
/// The same task is requested in JSON without repeating its command.
pub(crate) const TASK_JSON_CALL: &[&str] = &[TASK_CALL[0], TASK_CALL[1], "--json"];

pub(crate) fn read_call(argv: &[&str]) -> (Harness, u32) {
    let mut root = signed_in(PARTY);
    let call = root.submit(argv);
    (root, call)
}
/// Distinct from every application's allocated owner.
pub(crate) const UNKNOWN_ASKER: client::Asker = client::Asker(999);
/// A stable stored request's engine key, unrelated to its client call prefix.
const KEPT_KEY: skein_lib::RequestKey = [9; 16];
/// One slot for each guide call, including the model-choice seam.
pub(super) const GUIDE_CALLS: u32 = 24;
/// One slot per guide line, including the final line; fewer loses that line.
const GUIDE_LINES: u32 = 123;
/// One slot per row and form; fewer omits decide's second form.
pub(super) const TOOL_FORMS: u32 = 47;
/// Exactly one example exercises each row and form; zero misses it entirely.
pub(super) const EXAMPLES_PER_FORM: u32 = 1;
/// One missing byte exercises the first refused destination; zero would fit.
pub(super) const HELP_SHORTFALL: usize = 1;
/// Refusing an output must publish no bytes; any positive count is partial help.
pub(super) const NO_HELP_BYTES: usize = 0;
/// The guide's examples use whitespace-separated words without shell quoting.
pub(super) fn parse_line(line: &[u8]) -> Result<crate::jig::call::Parsed, crate::jig::argv::Usage> {
    let mut words = List::with_capacity(WORDS);
    let mut start = 0;
    for at in 0..=line.len() {
        let boundary = match line.get(at) {
            Some(byte) => byte.is_ascii_whitespace(),
            None => true,
        };
        if boundary {
            if start < at {
                words.push(line.get(start..at).expect("word range")).expect("bounded guide words");
            }
            start = at.checked_add(1).expect("guide range fits usize");
        }
    }
    crate::table::parse(words.as_slice())
}
/// Measure help inside its window, then fill an exact-length writer.
pub(super) fn help(tool: Option<u8>) -> Box<[u8]> {
    let mut measured = skein_lib::Writer::new(usize::try_from(crate::limits::TEXT).expect("text fits usize"));
    crate::help::text(tool, &mut measured).expect("help fits its window");
    let mut exact = skein_lib::Writer::new(measured.written());
    crate::help::text(tool, &mut exact).expect("help fits its measured length");
    exact.finish()
}
/// A row's usage lines, sharing help's text bound.
pub(super) fn usage(tool: &crate::jig::table::Tool) -> Box<[u8]> {
    let mut measured = skein_lib::Writer::new(usize::try_from(crate::limits::TEXT).expect("text fits usize"));
    crate::jig::help::usage(tool, &mut measured).expect("usage fits its window");
    let mut exact = skein_lib::Writer::new(measured.written());
    crate::jig::help::usage(tool, &mut exact).expect("usage fits its measured length");
    exact.finish()
}

/// Lines of the guide, shared by its parsing and coverage checks.
pub(super) fn guide_lines() -> List<&'static [u8]> {
    let mut lines = List::with_capacity(GUIDE_LINES);
    let mut start = 0;
    for at in 0..=crate::guide::GUIDE.len() {
        match crate::guide::GUIDE.get(at) {
            Some(b'\n') | None => {
                lines.push(crate::guide::GUIDE.get(start..at).expect("guide line range")).expect("bounded guide lines");
                start = at.checked_add(1).expect("guide range fits usize");
            }
            Some(_) => {}
        }
    }
    lines
}

/// Recovery examples are checked in the same hand-recorded full-help golden.
pub(super) fn guide_golden_has(expected: &[&[u8]]) {
    let golden = golden("help");
    for text in expected {
        assert!(skein_lib::bytes::find(&golden, text).is_some(), "{text:?}");
    }
}

mod reads;
pub(crate) use reads::*;

mod acts;
pub(crate) use acts::*;
mod waits;
pub(crate) use waits::*;

mod adopt;
pub(crate) use adopt::*;
mod digest;
pub(crate) use digest::*;

mod accounts;
pub(crate) use accounts::*;

mod choices;
pub(crate) use choices::*;
mod dependencies;
pub(crate) use dependencies::*;
