//! The check's offline readers (protocol/setup.md, 8.4).
use super::Check;
use crate::directories::Directories;
use jig_protocol_store::{codec::component::Format, format, keys};
use skein_fake_forge_domain::promises::Judge;
use skein_fake_forge_domain::promises::Recorded;
use skein_fake_forge_protocol::observations::decode_with_limits;
use skein_io::{
    file,
    file_layer::{self, FileIo},
    kernel::{Complete, Submit},
};
use skein_kv::{Event, Range, Request, Store};
use skein_lib::{Env, Queue, Time, Token, Wall};
use skein_shell::Host as _;
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Take, Write},
    path::{Path, PathBuf},
};
use temper_fake::check::{self, Reason};
use temper_forge_setup::setup::Mode;

const COMMAND: &str = "temper fake forge check";

/// The check's exit status and its two streams (protocol/setup.md, 8.4).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub(crate) status: u8,
    pub(crate) stdout: Box<[u8]>,
    pub(crate) stderr: Box<[u8]>,
}

/// Reads the document, the observations and the store's copy, and judges.
pub(crate) fn outcome(directories: &Directories, check: &Check) -> Outcome {
    let limits = match configuration(directories, check.forge) {
        Ok(limits) => limits,
        Err(why) => {
            return Outcome {
                status: 1,
                stdout: Box::new([]),
                stderr: crate::prefixed(COMMAND, &why).into_bytes().into_boxed_slice(),
            };
        }
    };
    let observations = match Observations::open(&check.observations) {
        Ok(observations) => observations,
        Err(why) => return undecided(Reason::ObservationsMissing, Vec::new(), Some(&why)),
    };
    let mut stderr = check::boundary_line(observations.lines(), observations.lost()).into_vec();
    let rows = match read_store(&check.store, &limits.store) {
        Ok(rows) => rows,
        Err(why) => return undecided(Reason::StoreUnreadable, stderr, Some(&why)),
    };
    let evidence = match check::evidence(check.forge, &rows, &limits.forge) {
        Ok(evidence) => evidence,
        Err(reason) => return undecided(reason, stderr, None),
    };
    stderr.extend_from_slice(&check::evidence_lines(&evidence));
    let mut judge = Judge::new(evidence.profile);
    for decision in &evidence.decisions {
        judge.decide(decision);
    }
    judge.decided();
    let records = match observations.records() {
        Ok(records) => records,
        Err(why) => return undecided(Reason::ObservationsMissing, stderr, Some(&why)),
    };
    for record in records {
        match record {
            Ok(record) => judge.observe(&record),
            Err(why) => return undecided(Reason::ObservationsMissing, stderr, Some(&why)),
        }
    }
    let output = check::verdict(&judge.verdict(), evidence.complete);
    Outcome { status: output.status, stdout: output.stdout, stderr: stderr.into_boxed_slice() }
}
fn configuration(directories: &Directories, forge: u16) -> Result<temper_service::startup::StoreLimits, String> {
    let document = directories.document();
    let bytes = skein_shell::read_file(&document, crate::CONFIG_BYTES)
        .map_err(|why| format!("configuration {}: {}", document.display(), crate::unread(why)))?;
    let jig = directories.jig();
    let crate::setup::Deployment { checked, .. } = crate::setup::check(&bytes, &jig)?;
    let limits = temper_service::startup::store_limits(&bytes, &jig)
        .map_err(|why| String::from_utf8_lossy(&temper_service::startup::lines(&why)).into_owned())?;
    let entry = checked.setup.entry(forge).ok_or_else(|| format!("forge {forge}: the setup has no entry for it"))?;
    match &entry.mode {
        Mode::Fake(_) => Ok(limits),
        Mode::Managed { .. } => Err(format!("forge {forge}: it is managed, not fake")),
        Mode::Owned(_) => Err(format!("forge {forge}: it is owned, not fake")),
    }
}
fn undecided(reason: Reason, mut stderr: Vec<u8>, why: Option<&str>) -> Outcome {
    if let Some(why) = why {
        stderr.extend_from_slice(crate::prefixed(COMMAND, why).as_bytes());
    }
    let output = check::undecided(reason);
    Outcome { status: output.status, stdout: output.stdout, stderr: stderr.into_boxed_slice() }
}

/// Writes the outcome's streams and returns its status.
pub(crate) fn run(directories: &Directories, check: &Check) -> u8 {
    let outcome = outcome(directories, check);
    let stdout = std::io::stdout().write_all(&outcome.stdout);
    let mut stderr = std::io::stderr();
    let written = stderr.write_all(&outcome.stderr);
    let mut status = outcome.status;
    for (stream, result) in [("stdout", stdout), ("stderr", written)] {
        if let Err(why) = result {
            status = 1;
            // A broken stderr may also refuse this last diagnostic.
            let _ = stderr.write_all(crate::prefixed(COMMAND, &format!("{stream}: {why}")).as_bytes());
        }
    }
    status
}

type Row = (Box<[u8]>, Box<[u8]>);
pub(crate) type Rows = Box<[Row]>;
const OWNER: Token = Token::new(1);
// One file operation at a time; room for a KV step's largest batch of answers.
const ROOM: u32 = skein_kv::MAX_OUT.above * 2;
const FILES: u32 = 8;
const ENTRIES: u32 = 32;
const OPERATIONS: u32 = 32;

pub(crate) fn read_store(path: &Path, limits: &jig_protocol_store::Limits) -> Result<Rows, String> {
    run_store(path, StoreHost::new(limits.store))
}
fn run_store(path: &Path, mut host: StoreHost) -> Result<Rows, String> {
    let root = skein_shell::open_root(path).map_err(|why| format!("opening {}: errno {why}", path.display()))?;
    let mut kernel = match skein_shell::Kernel::open(skein_shell::Config { operations: host.operations() }) {
        Ok(kernel) => kernel,
        Err(why) => {
            skein_shell::close_keeper_fd(root);
            return Err(format!("opening kernel: {why}"));
        }
    };
    host.request = Some(Request::Open { owner: OWNER, root });
    skein_shell::drive(&mut kernel, &skein_shell::Clock::new(), &mut host);
    // The KV borrows its root; Close settles its own files first.
    skein_shell::close_keeper_fd(root);
    match host.error {
        Some(why) => Err(why),
        None => Ok(host.rows.into_boxed_slice()),
    }
}

enum Step {
    Opening,
    Format,
    Loading {
        from: Box<[u8]>,
    },
    #[cfg(test)]
    Seeding,
    Closing,
    Done,
}
struct StoreHost {
    store: Store,
    io: FileIo,
    limits: skein_kv::Limits,
    above: Queue<Event>,
    below: Queue<file::Request>,
    file_events: Queue<file::Event>,
    completions: Queue<Complete>,
    submissions: Queue<Submit>,
    step: Step,
    request: Option<Request>,
    rows: Vec<Row>,
    error: Option<String>,
    #[cfg(test)]
    seed: Option<Rows>,
}
impl StoreHost {
    fn new(limits: skein_kv::Limits) -> Self {
        let mut io = FileIo::new(FILES, limits.chunk, ENTRIES, limits.deadline);
        io.seed_randomness(0);
        Self {
            store: Store::new(&limits),
            io,
            limits,
            above: Queue::with_capacity(ROOM),
            below: Queue::with_capacity(ROOM),
            file_events: Queue::with_capacity(ROOM),
            completions: Queue::with_capacity(OPERATIONS),
            submissions: Queue::with_capacity(OPERATIONS),
            step: Step::Opening,
            request: None,
            rows: Vec::new(),
            error: None,
            #[cfg(test)]
            seed: None,
        }
    }
    fn close(&mut self) {
        self.step = Step::Closing;
        self.request = Some(Request::Close { owner: OWNER });
    }
    fn load(&mut self, from: Box<[u8]>) {
        let mut range = Range::prefix(&[keys::APPLICATION]);
        range.start.clone_from(&from);
        self.step = Step::Loading { from };
        self.request = Some(Request::Load { owner: OWNER, range, max: self.limits.page });
    }
    fn opened(&mut self) {
        #[cfg(test)]
        if let Some(rows) = self.seed.take() {
            self.step = Step::Seeding;
            self.request = Some(Request::Commit {
                owner: OWNER,
                ops: rows.into_vec().into_iter().map(|(key, value)| skein_kv::Op::Put { key, value }).collect(),
            });
            return;
        }
        if let Some(key) = keys::encode_own(&keys::Own::Format, self.limits.key) {
            self.step = Step::Format;
            self.request = Some(Request::Get { owner: OWNER, key });
        } else {
            self.error = Some("format key: does not fit store key limit".into());
            self.close();
        }
    }
    fn answer(&mut self, event: Event) {
        match (&self.step, event) {
            (Step::Opening, Event::Opened { .. }) => self.opened(),
            (Step::Format, Event::Got { value, .. }) => {
                let checked = value.ok_or_else(|| "format: Got None".to_owned()).and_then(|value| {
                    let found = Format::decode(&value).map_err(|why| format!("format decode: {why:?}"))?;
                    format::check(&found, &format::current(temper_protocol_store::STORE_VERSION, found.deployment))
                        .map_err(|why| format!("format check: {why:?}"))
                });
                match checked {
                    Ok(()) => self.load(Box::from([keys::APPLICATION])),
                    Err(why) => {
                        self.error = Some(why);
                        self.close();
                    }
                }
            }
            (Step::Loading { from }, Event::Loaded { rows, next, .. }) => {
                // A next key is the first unreturned key, reused inclusively.
                // Refuse a stalled cursor rather than loop on a broken answer.
                if next.as_ref().is_some_and(|next| next <= from) {
                    self.error = Some(format!("loading: Loaded with non-advancing next {next:?}"));
                    self.close();
                    return;
                }
                for row in rows {
                    let Some(key) = row.key.strip_prefix(&[keys::APPLICATION]) else {
                        self.error = Some(format!("loading: Loaded key outside temper's interval: {:?}", row.key));
                        self.close();
                        return;
                    };
                    self.rows.push((key.into(), row.value));
                }
                match next {
                    Some(from) => self.load(from),
                    None => self.close(),
                }
            }
            #[cfg(test)]
            (Step::Seeding, Event::Committed { .. }) => self.close(),
            (Step::Closing, Event::Closed { .. }) => self.step = Step::Done,
            (step, answer) => {
                let name = match step {
                    Step::Opening => "opening",
                    Step::Format => "format",
                    Step::Loading { .. } => "loading",
                    #[cfg(test)]
                    Step::Seeding => "seeding",
                    Step::Closing => "closing",
                    Step::Done => "done",
                };
                let why = format!("{name}: {answer:?}");
                self.error = Some(match self.error.take() {
                    Some(earlier) => format!("{earlier}; {why}"),
                    None => why,
                });
                if matches!(self.step, Step::Opening | Step::Closing | Step::Done) {
                    self.step = Step::Done;
                } else {
                    self.close();
                }
            }
        }
    }
}
impl skein_shell::Host for StoreHost {
    fn iterate(&mut self, now: Time, wall: Wall) {
        let env = Env { now, wall, limits: self.limits };
        while let Some(complete) = self.completions.pop() {
            file_layer::up(&mut self.io, complete, &mut self.file_events, &mut self.submissions);
        }
        while let Some(event) = self.file_events.pop() {
            skein_kv::up(&mut self.store, &env, event, &mut self.above, &mut self.below);
        }
        while let Some(event) = self.above.pop() {
            self.answer(event);
        }
        if let Some(request) = self.request.take() {
            skein_kv::down(&mut self.store, &env, request, &mut self.above, &mut self.below);
        }
        if self.io.is_due(now) {
            file_layer::expire(&mut self.io, now, &mut self.submissions);
        }
        if self.io.takes() && self.below.is_empty() && skein_kv::is_ready(&self.store, &self.limits) {
            skein_kv::fire(&mut self.store, &env, &mut self.above, &mut self.below);
        }
        if self.io.takes()
            && let Some(request) = self.below.pop()
        {
            file_layer::down(&mut self.io, now, request, &mut self.file_events, &mut self.submissions);
        }
    }
    fn completions(&mut self) -> &mut Queue<Complete> {
        &mut self.completions
    }
    fn submissions(&mut self) -> &mut Queue<Submit> {
        &mut self.submissions
    }
    fn work_pending(&self, _now: Time) -> bool {
        self.request.is_some()
            || !self.above.is_empty()
            || !self.file_events.is_empty()
            || self.io.takes() && (!self.below.is_empty() || skein_kv::is_ready(&self.store, &self.limits))
    }
    fn next_deadline(&self) -> Option<Time> {
        self.io.next_deadline()
    }
    fn next_policy_deadline(&self) -> Option<Time> {
        None
    }
    fn is_empty(&self) -> bool {
        matches!(self.step, Step::Done)
            && self.io.takes()
            && self.io.open_files() == 0
            && self.request.is_none()
            && self.above.is_empty()
            && self.below.is_empty()
            && self.file_events.is_empty()
            && self.completions.is_empty()
            && self.submissions.is_empty()
    }
    fn worst_case(&self) -> u64 {
        // The KV charges 128 bytes per entry plus payload; twice that budget
        // covers the collected boxes and Vec's spare row cells too.
        u64::try_from(size_of::<Self>())
            .expect("host size fits u64")
            .saturating_add(skein_kv::worst_case(&self.limits).expect("usable store limits"))
            .saturating_add(self.limits.budget.saturating_mul(2))
            .saturating_add(
                FileIo::worst_case(FILES, self.limits.chunk, ENTRIES, self.limits.chunk).expect("file limits"),
            )
            .saturating_add(Queue::<Event>::worst_case(ROOM).expect("queue limits"))
            .saturating_add(Queue::<file::Request>::worst_case(ROOM).expect("queue limits"))
            .saturating_add(Queue::<file::Event>::worst_case(ROOM).expect("queue limits"))
            .saturating_add(Queue::<Complete>::worst_case(OPERATIONS).expect("queue limits"))
            .saturating_add(Queue::<Submit>::worst_case(OPERATIONS).expect("queue limits"))
    }
    fn operations(&self) -> u32 {
        OPERATIONS
    }
}

pub(crate) struct Observations {
    path: PathBuf,
    end: u64,
    lines: u64,
    lost: u64,
}
impl Observations {
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        let mut reader = Lines::open(path, u64::MAX)?;
        let mut boundary = None;
        let mut lines = 0_u64;
        let mut refused = None;
        while let Some(line) = reader.next_line()? {
            lines += 1;
            let record = line.and_then(|()| decode(&reader.bytes));
            match record {
                Ok(Some(Recorded::Lost(lost))) => {
                    if let Some(why) = refused {
                        return Err(why);
                    }
                    boundary = Some(Self { path: path.into(), end: reader.offset, lines, lost });
                }
                Ok(Some(Recorded::Observed { .. }) | None) => {}
                Err(why) if why == GIT_FULL || why == INSTALLATIONS_FULL => return Err(why),
                Err(why) => {
                    if refused.is_none() {
                        refused = Some(format!("line {lines}: {why}"));
                    }
                }
            }
        }
        boundary.ok_or_else(|| "observations: no checkpoint".into())
    }
    pub(crate) fn lines(&self) -> u64 {
        self.lines
    }
    pub(crate) fn lost(&self) -> u64 {
        self.lost
    }
    pub(crate) fn records(&self) -> Result<Records, String> {
        Ok(Records { reader: Lines::open(&self.path, self.end)?, end: self.end, done: false })
    }
}
const GIT_FULL: &str = "GitHub git-call record is full";
const INSTALLATIONS_FULL: &str = "GitHub installation record is full";
fn decode(bytes: &[u8]) -> Result<Option<Recorded>, String> {
    match decode_with_limits(bytes, &temper_fake::forge::LIMITS.limits) {
        Ok(record) => Ok(Some(record)),
        Err(refused) => match temper_fake::forge::observations::decode(bytes) {
            Ok(Some(
                temper_fake::forge::observations::Observation::GitCall(_)
                | temper_fake::forge::observations::Observation::InstallationCreated { .. }
                | temper_fake::forge::observations::Observation::InstallationDeleted { .. },
            )) => Ok(None),
            Ok(Some(temper_fake::forge::observations::Observation::InstallationsFull { .. })) => {
                Err(INSTALLATIONS_FULL.into())
            }
            Ok(Some(temper_fake::forge::observations::Observation::GitCallsFull { .. })) => Err(GIT_FULL.into()),
            Ok(None) | Err(_) => Err(format!("decode: {refused:?}")),
        },
    }
}

pub(crate) struct Records {
    reader: Lines,
    end: u64,
    done: bool,
}
impl Iterator for Records {
    type Item = Result<Recorded, String>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.reader.offset == self.end {
            return None;
        }
        let record = loop {
            match self.reader.next_line() {
                Ok(Some(line)) => match line.and_then(|()| decode(&self.reader.bytes)) {
                    Ok(Some(record)) => break Ok(record),
                    Ok(None) => {
                        if self.reader.offset == self.end {
                            return None;
                        }
                    }
                    Err(why) => break Err(why),
                },
                Ok(None) => break Err("observations: file shorter than checkpoint boundary".into()),
                Err(why) => break Err(why),
            }
        };
        if record.is_err() {
            self.done = true;
        }
        Some(record)
    }
}
// The fixed read buffer and one bounded line are shared by both passes. An
// overlong line is skipped through LF without retaining its remaining bytes.
struct Lines {
    reader: BufReader<Take<File>>,
    bytes: Vec<u8>,
    offset: u64,
}
impl Lines {
    fn open(path: &Path, end: u64) -> Result<Self, String> {
        let file = File::open(path).map_err(|why| format!("observations {}: {why}", path.display()))?;
        Ok(Self { reader: BufReader::with_capacity(8192, file.take(end)), bytes: Vec::new(), offset: 0 })
    }
    fn next_line(&mut self) -> Result<Option<Result<(), String>>, String> {
        self.bytes.clear();
        let mut overlong = false;
        let bound = usize::try_from(temper_fake::limits::OBSERVATION_BYTES).expect("u32 fits usize");
        loop {
            let chunk = self.reader.fill_buf().map_err(|why| format!("observations read: {why}"))?;
            if chunk.is_empty() {
                return Ok(None);
            }
            // Cursor bounds skip_until to this chunk; std's byte search stays
            // efficient in unoptimized builds, including an overlong tail.
            let length = std::io::Cursor::new(chunk).skip_until(b'\n').expect("an in-memory chunk is readable");
            let newline = chunk.get(length - 1) == Some(&b'\n');
            if !overlong {
                if length > bound - self.bytes.len() {
                    self.bytes.clear();
                    overlong = true;
                } else {
                    // Grow at most to the line bound, including spare capacity.
                    let needed = self.bytes.len() + length;
                    if needed > self.bytes.capacity() {
                        self.bytes.reserve_exact(
                            needed.max(self.bytes.capacity().saturating_mul(2)).min(bound) - self.bytes.len(),
                        );
                    }
                    self.bytes.extend_from_slice(&chunk[..length]);
                }
            }
            self.reader.consume(length);
            self.offset += u64::try_from(length).expect("chunk length fits u64");
            if newline {
                return Ok(Some(if overlong { Err(format!("line past {bound} bytes")) } else { Ok(()) }));
            }
        }
    }
}

#[cfg(test)]
mod tests;
