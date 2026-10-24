//! The document's shape and its checked startup (protocol/configuration.md, 2).
pub(crate) mod application;
pub(crate) mod checks;
mod problems;
use crate::{AccountsRuntime, Service, Worker, WorkerRuntime, settings};
use alloc::boxed::Box;
pub use application::sign_in::{Endpoints, SignIn, url as sign_in_url};
pub use application::{Forge, Identity, Transport, USER_BYTES, User};
pub use checks::redirect_ports;
use jig_protocol_config::{self as config, Path, Segment, read};
pub use problems::{Problem, Problems, What, lines};
use skein_lib::List;

/// Private handovers the shell reads once before starting a fake forge's adoptions.
#[must_use]
pub fn setup_files(checked: &temper_forge_setup::check::Checked) -> Box<[config::File]> {
    let mut files = List::with_capacity(u32::try_from(checked.setup.entries.len()).expect("checked setup entries"));
    for (index, entry) in checked.setup.entries.iter().enumerate() {
        // temper-15 removes the old-form Fake handover read.
        if !checked.records_only.contains(&entry.forge)
            && let temper_forge_setup::setup::Mode::Fake(_) = &entry.mode
        {
            files
                .push(config::File {
                    at: setup_path(index).member(b"admin_token"),
                    path: handover_path(entry),
                    max: 40,
                    private: true,
                })
                .expect("one handover per fake");
        }
    }
    files.into_boxed()
}
/// The same bounded read request is served before admission and by the running host.
pub fn handover_reads(
    document: &[u8],
    directories: &config::Directories,
    checked: &temper_forge_setup::check::Checked,
) -> Result<Box<[crate::HandoverRead]>, Problems> {
    let parsed = load(document, directories, false)?.1.expect("checked engine");
    let mut reads = List::with_capacity(u32::try_from(checked.records_only.len()).expect("checked forges"));
    for entry in &checked.setup.entries {
        // temper-15 removes old forms; their existing file reads stay above.
        if checked.records_only.contains(&entry.forge) {
            reads
                .push(crate::HandoverRead {
                    forge: entry.forge,
                    run: skein_lib::Token::new(0),
                    path: handover_path(entry),
                    max: parsed.application.limits.records.value_bytes,
                })
                .expect("one read per local forge");
        }
    }
    Ok(reads.into_boxed())
}
fn setup_path(index: usize) -> Path {
    Path::root()
        .member(b"application")
        .member(b"setup")
        .child(Segment::Index(u32::try_from(index).expect("checked setup index")))
}
fn handover_path(entry: &temper_forge_setup::setup::Entry) -> Box<[u8]> {
    temper_forge_setup::place::Place { directory: entry.directory.clone(), user: Box::new([]) }
        .path(temper_forge_setup::place::File::AdminToken)
}
fn setup_values(
    checked: Option<&temper_forge_setup::check::Checked>,
    files: &settings::Files,
    problems: &mut Problems,
    max: u32,
) -> Box<[settings::Setup]> {
    let Some(checked) = checked else { return Box::new([]) };
    let mut values = List::with_capacity(u32::try_from(checked.setup.entries.len()).expect("checked setup entries"));
    for (index, entry) in checked.setup.entries.iter().enumerate() {
        let mut handover = None;
        // temper-15 removes the old-form Fake handover read.
        if !checked.records_only.contains(&entry.forge)
            && let temper_forge_setup::setup::Mode::Fake(_) = &entry.mode
        {
            let at = setup_path(index).member(b"admin_token");
            if let Some(bytes) = application::contents(files, &handover_path(entry), 40, at.clone(), problems) {
                let mut valid = bytes.len() == 40;
                for byte in &bytes {
                    valid &= byte.is_ascii_digit() || (b'a'..=b'f').contains(byte);
                }
                if valid {
                    handover = Some(skein_lib::Secret::new(bytes));
                } else {
                    problems.push_file(at, What::Handover, Some(handover_path(entry)));
                }
            }
        }
        if checked.records_only.contains(&entry.forge) {
            let path = handover_path(entry);
            for (name, value) in &files.entries {
                if *name == path {
                    if value.expose().len() > usize::try_from(max).expect("record bound") {
                        problems.push_file(
                            setup_path(index).member(b"admin_token"),
                            What::Handover,
                            Some(path.clone()),
                        );
                    } else {
                        handover = Some(value.clone());
                    }
                }
            }
        }
        values.push(settings::Setup { entry: entry.clone(), handover }).expect("checked setup entries");
    }
    values.into_boxed()
}

/// The services selected by the document's members (protocol/README.md, 15.1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Standalone,
    EngineAlone,
    WorkerAlone,
}
fn member(document: &skein_json::Document, at: u32, name: &[u8]) -> Option<u32> {
    read::Object::new(document, at).ok()?.member(document, name)
}
fn nearest(document: &skein_json::Document) -> Shape {
    let Some(engine) = member(document, 0, b"engine") else { return Shape::WorkerAlone };
    let Some(hosts) = member(document, engine, b"hosts") else { return Shape::Standalone };
    let Some(at) = member(document, hosts, b"listener") else { return Shape::Standalone };
    let Ok(listeners) = read::list(document, at, document.len()) else { return Shape::Standalone };
    let Some(first) = listeners.get(0) else { return Shape::Standalone };
    if member(document, *first, b"socket").is_some() { Shape::EngineAlone } else { Shape::Standalone }
}
/// Read and check the shape before opening any named file.
pub fn shape(input: &[u8], directories: &config::Directories) -> Result<Shape, Problems> {
    Ok(load(input, directories, true)?.0)
}
fn finish_shape(document: &skein_json::Document, object: read::Object, path: &Path, problems: &mut Problems) {
    let mut found = config::Problems::empty();
    object.finish(document, path, &mut found);
    for found in &found.problems {
        problems.push(
            found.path.clone(),
            if found.what == config::What::Unknown { What::WorkerAlone } else { What::Jig(found.what) },
        );
    }
    problems.past = problems.past.saturating_add(found.past);
}
/// A forge API token's bound, until the forge component supplies it.
pub const API_TOKEN_BYTES: u32 = 4096;
/// A provider's fixed sign-in redirect, with its diagnostic path; no credentials.
#[derive(Debug)]
pub struct RedirectPort {
    pub port: u16,
    pub path: Path,
    pub redirect_path: Box<[u8]>,
}
/// The standalone listeners whose fixed ports a redirect must avoid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Listener {
    Clients,
    Webhooks,
    Owned(u32),
    Fake(u32),
    Demo,
}
fn redirects(engine: &config::Engine) -> Box<[RedirectPort]> {
    let mut redirects = List::with_capacity(u32::try_from(engine.llm.providers.len()).expect("bounded providers"));
    for (index, provider) in engine.llm.providers.iter().enumerate() {
        if let Some(registration) = &provider.sign_in
            && let Some(port) = registration.redirect_port
        {
            redirects
                .push(RedirectPort {
                    port,
                    path: Path::root()
                        .member(b"engine")
                        .member(b"llm")
                        .member(b"providers")
                        .child(Segment::Index(u32::try_from(index).expect("bounded providers")))
                        .member(b"sources")
                        .member(b"sign_in")
                        .member(b"redirect_port"),
                    redirect_path: registration.redirect_path.clone(),
                })
                .expect("one redirect per provider");
        }
    }
    redirects.into_boxed()
}
fn check_redirects(engine: &config::Engine, application: &application::Members, problems: &mut Problems) {
    let redirects = redirects(engine);
    redirect_ports(&redirects, checks::port(&application.clients.listener), Listener::Clients, problems);
    let port = match &application.webhooks {
        Some(hooks) => checks::port(&hooks.listener),
        None => None,
    };
    redirect_ports(&redirects, port, Listener::Webhooks, problems);
}
/// The files and directories the binary reads and opens before startup.
#[derive(Debug)]
pub struct Named {
    pub files: Box<[config::File]>,
    pub asked_files: Box<[config::File]>,
    pub redirects: Box<[RedirectPort]>,
    pub deployment: Box<[u8]>,
    pub llm: config::Llm,
    pub projects: Box<[config::Project]>,
    pub worker: Option<NamedWorker>,
    pub forges: Box<[Forge]>,
    pub identities: Box<[Identity]>,
    pub clients: config::Address,
    pub sign_in: Option<SignIn>,
    /// `application.clients.bundle`, for the binary's read.
    pub bundle: Option<Box<[u8]>>,
    /// The bundle's limits, overrides applied.
    pub bundle_limits: crate::limits::Clients,
    pub webhooks: Option<application::Webhooks>,
    pub overrides: Box<[(Box<[u8]>, u64)]>,
}
#[derive(Debug)]
pub struct NamedWorker {
    pub root: Box<[u8]>,
    pub agent: Box<[u8]>,
}
/// The limits serve opens the deployment's store at, for a reader of a copy of it.
#[derive(Debug)]
pub struct StoreLimits {
    pub store: jig_protocol_store::Limits,
    pub forge: temper_engine_domain_forge::Limits,
}
/// Derive the store's opening limits without reading named files or checking runtime facts.
pub fn store_limits(document: &[u8], directories: &config::Directories) -> Result<StoreLimits, Problems> {
    let mut parsed =
        load(document, directories, false)?.1.ok_or(problem(Path::root().member(b"engine"), What::Standalone))?;
    let root = engine_limits(&mut parsed.engine.limits, &parsed.application.limits)?;
    let store = crate::store::limits(&root, &parsed.engine.limits.core, &root.forge, &parsed.engine.limits.store)
        .ok_or(problem(Path::root().member(b"engine").member(b"limits"), What::Jig(config::What::Bound)))?;
    Ok(StoreLimits { store, forge: root.forge })
}
fn engine_limits(
    jig: &mut config::Limits,
    own: &crate::limits::Limits,
) -> Result<temper_engine_domain::Limits, Problems> {
    jig.clients.wire.keyed_value_some = crate::limits::client_value_bytes(jig, own);
    crate::limits::engine(jig, own)
        .ok_or(problem(Path::root().member(b"engine").member(b"limits"), What::Jig(config::What::Bound)))
}
#[derive(Debug)]
struct Parsed {
    engine: config::Engine,
    workers: Box<[config::Worker]>,
    application: application::Members,
}
fn problem(path: Path, what: What) -> Problems {
    let mut problems = Problems::empty();
    problems.push(path, what);
    problems
}
fn indexed(index: usize) -> Path {
    Path::root().member(b"workers").child(Segment::Index(u32::try_from(index).expect("bounded workers")))
}
fn parse(input: &[u8], directories: &config::Directories) -> Result<Parsed, Problems> {
    load(input, directories, true)?.1.ok_or(problem(Path::root().member(b"engine"), What::Standalone))
}
// Readers need the typed document alone. Startup also checks cross-part and
// raw-path constraints here, preserving the order of its accumulated problems.
#[expect(clippy::too_many_lines, reason = "one load pass accumulates all cross-part diagnostics")]
fn load(
    input: &[u8],
    directories: &config::Directories,
    check_start: bool,
) -> Result<(Shape, Option<Parsed>), Problems> {
    let length = u32::try_from(input.len()).or(Err(problem(Path::root(), What::Jig(config::What::Bound))))?;
    let document =
        match config::document(input, &config::DocumentLimits { bytes: 1_048_576, tokens: length, depth: 32 }) {
            Ok(document) => document,
            Err(found) => return Err(problem(found.path, What::Jig(found.what))),
        };
    let shape = nearest(&document);
    let mut problems = Problems::empty();
    let mut found = config::Problems::empty();
    let mut whole = match read::Object::new(&document, 0) {
        Ok(object) => object,
        Err(what) => return Err(problem(Path::root(), What::Jig(what))),
    };
    let root = Path::root();
    // Keep the empty object's established missing-member diagnostics.
    let engine_at = if document.len() == 2 {
        whole.required(&document, b"engine", &root, &mut found)
    } else {
        whole.member(&document, b"engine")
    };
    let worker_at = whole.required(&document, b"workers", &root, &mut found);
    let application_at = whole.required(&document, b"application", &root, &mut found);
    if shape == Shape::WorkerAlone {
        finish_shape(&document, whole, &root, &mut problems);
    } else {
        whole.finish(&document, &root, &mut found);
    }
    problems.append(found, &root);
    let engine = match engine_at {
        Some(at) => match config::engine(&document, at, directories) {
            Ok(mut engine) => {
                engine.limits.clients.watches_per_stream = temper_engine_domain::WATCHES_PER_STREAM;
                Some(engine)
            }
            Err(found) => {
                problems.append(found, &root.member(b"engine"));
                None
            }
        },
        None => None,
    };
    let ats = match worker_at {
        Some(at) => match read::list(&document, at, document.len()) {
            Ok(ats) => ats,
            Err(what) => {
                problems.push(root.member(b"workers"), What::Jig(what));
                List::with_capacity(0)
            }
        },
        None => List::with_capacity(0),
    };
    let mut workers = List::with_capacity(ats.len());
    for (index, at) in ats.iter().enumerate() {
        if shape == Shape::EngineAlone {
            continue;
        }
        if check_start {
            checks::raw_worker(&document, *at, &indexed(index), &mut problems);
        }
        match config::worker(&document, *at, directories) {
            Ok(worker) => workers.push(worker).expect("bounded workers"),
            Err(found) => problems.append(found, &indexed(index)),
        }
    }
    if shape == Shape::WorkerAlone {
        if worker_at.is_some() && ats.len() != 1 {
            problems.push(root.member(b"workers"), What::WorkerAlone);
        }
        for (index, worker) in workers.iter().enumerate() {
            if !checks::loopback(&worker.engine) {
                problems.push(indexed(index).member(b"engine"), What::WorkerAlone);
            }
        }
        if let Some(at) = application_at {
            let _part = application::worker_alone(&document, at, directories, &mut problems);
        }
        return if problems.is_empty() { Ok((shape, None)) } else { Err(problems) };
    }
    let application = match &engine {
        Some(engine) => match application_at {
            Some(at) => application::parse(&document, at, engine, directories, &mut problems),
            None => None,
        },
        None => None,
    };
    if !problems.is_empty() {
        return Err(problems);
    }
    let application = application.expect("required application checked");
    let workers = workers.into_boxed();
    if !check_start {
        return Ok((shape, Some(Parsed { engine: engine.expect("required engine checked"), workers, application })));
    }
    if shape == Shape::EngineAlone && !ats.is_empty() {
        problems.push(root.member(b"workers"), What::EngineAlone);
    }
    checks::standalone(
        engine.as_ref().expect("required engine checked"),
        &application.clients,
        &workers,
        &mut problems,
    );
    if shape == Shape::EngineAlone && !application.workers.is_empty() {
        problems.push(root.member(b"application").member(b"workers"), What::EngineAlone);
    } else if workers.len() != application.workers.len() {
        problems.push(root.member(b"application").member(b"workers"), What::Jig(config::What::Bound));
    }
    checks::hub(engine.as_ref().expect("required engine checked"), &mut problems);
    checks::provider(engine.as_ref().expect("required engine checked"), &mut problems);
    checks::credentials(
        engine.as_ref().expect("required engine checked"),
        &workers,
        &application.identities,
        &mut problems,
    );
    checks::webhooks(&application, &mut problems);
    check_redirects(engine.as_ref().expect("required engine checked"), &application, &mut problems);
    checks::records(
        engine.as_ref().expect("required engine checked"),
        &application.limits,
        &application.forges,
        &mut problems,
    );
    checks::limits(&application.limits, &application.limits.git_component, &mut problems);
    let mut base = 0;
    for forge in &application.forges {
        base = base.max(u64::try_from(forge.git.len()).expect("bounded git base"));
    }
    checks::names(checks::remote(base, application.limits.forge.name_bytes), &application.limits, &mut problems);
    if let Some(worker) = workers.first() {
        checks::sockets(
            engine.as_ref().expect("required engine checked"),
            worker,
            &application.limits.git_component,
            &application,
            &mut problems,
        );
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok((shape, Some(Parsed { engine: engine.expect("required engine checked"), workers, application })))
}
fn all_files(parsed: &Parsed) -> Box<[config::File]> {
    let engine = config::files(&parsed.engine);
    let worker = match parsed.workers.first() {
        Some(worker) => config::worker_files(worker),
        None => Box::new([]),
    };
    let application = application::files(
        &parsed.application,
        API_TOKEN_BYTES,
        jig_protocol_worker_wire::CEILINGS.application_value_secret,
    );
    let count = engine
        .len()
        .checked_add(worker.len())
        .expect("bounded files")
        .checked_add(application.len())
        .expect("bounded files");
    let mut files = List::with_capacity(u32::try_from(count).expect("bounded file count"));
    for (named, prefix) in [(engine, Path::root().member(b"engine")), (worker, indexed(0)), (application, Path::root())]
    {
        for mut file in named {
            let mut path = prefix.clone();
            for segment in &file.at.segments {
                path = path.child(segment.clone());
            }
            file.at = path;
            files.push(file).expect("capacity includes every named file");
        }
    }
    files.into_boxed()
}
/// Parse every member before the binary reads any named file.
pub fn named(document: &[u8], directories: &config::Directories) -> Result<Named, Problems> {
    let parsed = parse(document, directories)?;
    let files = all_files(&parsed);
    Ok(Named {
        files,
        asked_files: application::asked_files(
            &parsed.application,
            API_TOKEN_BYTES,
            jig_protocol_worker_wire::CEILINGS.application_value_secret,
        ),
        redirects: redirects(&parsed.engine),
        clients: parsed.application.clients.listener,
        sign_in: parsed.application.sign_in,
        bundle: parsed.application.clients.bundle,
        bundle_limits: parsed.application.limits.clients,
        webhooks: parsed.application.webhooks,
        forges: parsed.application.forges,
        identities: parsed.application.identities,
        overrides: parsed.application.overrides,
        deployment: parsed.engine.deployment.directory,
        llm: parsed.engine.llm,
        projects: parsed.engine.projects,
        worker: match parsed.workers.first() {
            Some(worker) => Some(NamedWorker {
                root: parsed.application.workers.first().expect("one application worker checked").root.clone(),
                agent: worker.agent.configuration.clone(),
            }),
            None => None,
        },
    })
}
fn check_files(parsed: &Parsed, files: &settings::Files) -> Result<(), Problems> {
    let mut problems = Problems::empty();
    for file in all_files(parsed) {
        match files.contents(&file.path) {
            Some(contents) if contents.len() <= usize::try_from(file.max).expect("file bound") => {}
            Some(_) => problems.push(file.at, What::Jig(config::What::Bound)),
            None => problems.push(file.at, What::Jig(config::What::Unreadable)),
        }
    }
    for file in application::asked_files(
        &parsed.application,
        API_TOKEN_BYTES,
        jig_protocol_worker_wire::CEILINGS.application_value_secret,
    ) {
        if let Some(contents) = files.contents(&file.path)
            && contents.len() > usize::try_from(file.max).expect("file bound")
        {
            problems.push(file.at, What::Jig(config::What::Bound));
        }
    }
    if problems.is_empty() { Ok(()) } else { Err(problems) }
}
/// Checked envelopes from the same calculations startup uses, without starting services.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Requirements {
    pub engine_bytes: u64,
    pub worker_bytes: u64,
}
/// A module's contribution to a service's checked memory envelope.
pub type MemoryTerm = (&'static str, u64);
/// Named contributions at the service's composition boundary; totals are never figures.
#[derive(Debug)]
pub struct MemoryTerms {
    pub engine: Box<[MemoryTerm]>,
    pub worker: Box<[MemoryTerm]>,
}
pub(crate) fn memory_sum(terms: &[MemoryTerm]) -> Option<u64> {
    let mut bytes = 0_u64;
    for (_, term) in terms {
        bytes = bytes.checked_add(*term)?;
    }
    Some(bytes)
}
fn memory_bound() -> Problems {
    problem(Path::root().member(b"memory_bytes"), What::Jig(config::What::Bound))
}
/// List the same terms startup sums, without enforcing the memory declarations.
#[expect(clippy::too_many_arguments, reason = "the same checked inputs as memory_requirements")]
pub fn memory_terms(
    document: &[u8],
    directories: &config::Directories,
    files: &settings::Files,
    agent: &settings::Agent,
    seeds: &settings::Seeds,
    accounts: Option<&AccountsRuntime>,
    forges: List<crate::ForgeRuntime>,
    setup: Option<&temper_forge_setup::check::Checked>,
) -> Result<MemoryTerms, Problems> {
    if shape(document, directories)? == Shape::WorkerAlone {
        let part = worker_part(document, directories)?;
        check_worker_part(&part, files, agent)?;
        let worker = crate::worker_settings::memory_terms(&part.worker, agent, &part.limits).ok_or(memory_bound())?;
        let process =
            crate::system::worker_process_memory_terms(&part.worker, agent, &part.limits).ok_or(memory_bound())?;
        let capacity = u32::try_from(worker.len().checked_add(process.len()).expect("memory term count"))
            .expect("bounded memory term count");
        let mut terms = List::with_capacity(capacity);
        for term in worker {
            terms.push(term).expect("worker memory terms");
        }
        for term in process {
            terms.push(term).expect("process memory terms");
        }
        return Ok(MemoryTerms { engine: Box::new([]), worker: terms.into_boxed() });
    }
    let mut checked = checked_start(document, directories, files, Some(agent), seeds, accounts, forges, setup)?;
    let root = engine_limits(&mut checked.engine.jig.limits, &checked.engine.limits)?;
    let store =
        crate::store::limits(&root, &checked.engine.jig.limits.core, &root.forge, &checked.engine.jig.limits.store)
            .ok_or(memory_bound())?;
    let worker = checked.worker.as_ref().expect("checked embedded worker");
    let operations = crate::worker_settings::operations(&checked.engine.limits.git_component).ok_or(memory_bound())?;
    let engine = match crate::system::startup_memory_terms(
        &checked.engine,
        &root,
        &store,
        accounts,
        operations,
        &checked.io,
        (checked.forges.1, checked.forges.2, &checked.forges.0),
    ) {
        Ok(terms) => Box::new(terms),
        Err(found) => return Err(Problems::jig(found, &Path::root())),
    };
    let worker =
        Box::new(crate::worker_settings::memory_terms(worker, agent, &checked.engine.limits).ok_or(memory_bound())?);
    Ok(MemoryTerms { engine, worker })
}
struct CheckedStart {
    not_set_up: List<u16>,
    engine: settings::Engine,
    worker: Option<settings::Worker>,
    requirements: Requirements,
    io: skein_io::Limits,
    forges: (skein_forge_connection::Settings, u64, u64),
}
/// Derive the two envelopes without enforcing the document's memory declarations.
/// File, agent, root, channel and runtime checks are identical to startup's.
#[expect(clippy::too_many_arguments, reason = "the same inputs as startup, including its checked setup")]
pub fn memory_requirements(
    document: &[u8],
    directories: &config::Directories,
    files: &settings::Files,
    agent: &settings::Agent,
    seeds: &settings::Seeds,
    accounts: Option<&AccountsRuntime>,
    forges: List<crate::ForgeRuntime>,
    setup: Option<&temper_forge_setup::check::Checked>,
) -> Result<Requirements, Problems> {
    if shape(document, directories)? == Shape::WorkerAlone {
        let part = worker_part(document, directories)?;
        check_worker_part(&part, files, agent)?;
        return Ok(Requirements {
            engine_bytes: 0,
            worker_bytes: crate::system::worker_alone_worst_case(&part.worker, agent, &part.limits)
                .ok_or(problem(indexed(0).member(b"memory_bytes"), What::Jig(config::What::Bound)))?,
        });
    }
    Ok(checked_start(document, directories, files, Some(agent), seeds, accounts, forges, setup)?.requirements)
}
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one checked startup pass preserves diagnostic order beside resolved setup and runtime facts"
)]
fn checked_start(
    document: &[u8],
    directories: &config::Directories,
    files: &settings::Files,
    agent: Option<&settings::Agent>,
    seeds: &settings::Seeds,
    accounts: Option<&AccountsRuntime>,
    forges: List<crate::ForgeRuntime>,
    setup: Option<&temper_forge_setup::check::Checked>,
) -> Result<CheckedStart, Problems> {
    let parsed = parse(document, directories)?;
    let agent = match parsed.workers.first() {
        Some(_) => Some(agent.ok_or(problem(indexed(0).member(b"agent"), What::Agent))?),
        None => None,
    };
    // Only a checked local or client-managed entry enables a file-free Forgejo.
    for (index, forge) in parsed.application.forges.iter().enumerate() {
        let admitted = match setup {
            Some(checked) => match checked.setup.entry(forge.number) {
                Some(entry) => {
                    checked.records_only.contains(&forge.number)
                        || entry.mode == (temper_forge_setup::setup::Mode::Managed { admin_token: None })
                }
                None => false,
            },
            None => false,
        };
        if forge.kind == settings::Kind::Forgejo && forge.records_only && !admitted {
            return Err(problem(
                Path::root()
                    .member(b"application")
                    .member(b"forges")
                    .child(Segment::Index(u32::try_from(index).expect("bounded forges"))),
                What::Jig(config::What::Refers),
            ));
        }
    }
    check_files(&parsed, files)?;
    let mut problems = Problems::empty();
    let resolved = application::resolve(&parsed.application, files, &mut problems);
    if !problems.is_empty() {
        return Err(problems);
    }
    let setup_values = setup_values(setup, files, &mut problems, parsed.application.limits.records.value_bytes);
    if !problems.is_empty() {
        return Err(problems);
    }
    let mut resolved = resolved.expect("resolved files checked");
    if let Some(setup) = setup {
        application::git_programs(&mut resolved.forges, &setup.setup);
    }
    let own = &parsed.application.limits;
    checks::turns(Some(&parsed.engine.limits), &parsed.workers, own, agent, &mut problems);
    let remote = u32::try_from(checks::longest_remote(&resolved.forges, own.forge.name_bytes).unwrap_or(u64::MAX)).ok();
    let engine_bounds = temper_protocol_worker_wire::Bounds {
        repositories: own.forge.resources_per_task,
        name_bytes: own.forge.name_bytes,
        remote_bytes: remote.unwrap_or(u32::MAX),
        conflicts: own.checkout.conflicts,
        path_bytes: own.checkout.path_bytes,
        token_bytes: temper_protocol_worker_wire::v1::CEILINGS.token_token,
        entries: own.forges.installations,
        entry_token_bytes: own.forge_component.token_bytes,
        text_bytes: temper_protocol_worker_wire::v1::CEILINGS.identity_name,
    };
    let worker_bounds = temper_protocol_worker_wire::Bounds {
        repositories: own.checkout.repositories,
        name_bytes: own.checkout.name_bytes,
        remote_bytes: own.checkout.name_bytes,
        ..engine_bounds
    };
    checks::channel(
        Some(&engine_bounds),
        if agent.is_some() { Some(&worker_bounds) } else { None },
        &resolved.forges,
        own,
        &mut problems,
    );
    if let Some(wire) = crate::translate::limits(own) {
        checks::identities(&resolved.identities, &wire, &mut problems);
    }
    if let Some(agent) = agent {
        checks::agent(&parsed.engine, agent, &mut problems);
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let mut core = config::core(&parsed.engine, seeds.deployment, seeds.seed);
    read_instructions(&mut core, files);
    if let Some(agent) = agent {
        checks::admission(&core, &parsed.engine, agent, &mut problems);
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let mut not_set_up =
        List::with_capacity(u32::try_from(parsed.application.asked_owned.len()).expect("asked forges"));
    for number in &parsed.application.asked_owned {
        if application::missing_asked_files(&parsed.application, files, *number) {
            not_set_up.push(*number).expect("asked forges");
        }
    }
    let (worker, bytes, worker_operations) = if let Some(agent) = agent {
        let worker = parsed.workers.into_iter().next().expect("one worker checked");
        let own = parsed.application.workers.into_iter().next().expect("one application worker checked");
        let worker = settings::Worker { jig: worker, root: own.root, git: own.git };
        checks::worker(&crate::worker_settings::root_limits(&worker, agent, &parsed.application.limits), &mut problems);
        checks::grants(&parsed.engine, &worker.jig.limits, agent, &mut problems);
        if !problems.is_empty() {
            return Err(problems);
        }
        if let Err(found) = crate::worker_settings::prepare(&worker, agent, &parsed.application.limits) {
            return Err(Problems::jig(found, &Path::root()));
        }
        let bytes = crate::worker_worst_case(&worker, agent, &parsed.application.limits)
            .ok_or(problem(indexed(0).member(b"memory_bytes"), What::Jig(config::What::Bound)))?;
        let worker_operations = crate::worker_settings::operations(&parsed.application.limits.git_component)
            .ok_or(problem(indexed(0).member(b"limits"), What::Jig(config::What::Bound)))?;
        (Some(worker), bytes, worker_operations)
    } else {
        (None, 0, 0)
    };
    let mut engine = settings::Engine {
        jig: parsed.engine,
        clients: parsed.application.clients,
        sign_in: resolved.sign_in,
        webhooks: resolved.webhooks,
        files: settings::Files { entries: files.entries.clone() },
        forges: resolved.forges,
        demo: parsed.application.demo,
        setup: setup_values,
        // Records-only local starts wait for the root to restore.
        records_only: match setup {
            Some(checked) => checked.records_only.clone(),
            None => Box::new([]),
        },
        identities: resolved.identities,
        landing: parsed.application.landing,
        charter: parsed.application.charter,
        limits: parsed.application.limits,
    };
    let root = engine_limits(&mut engine.jig.limits, &engine.limits)?;
    checks::root(core, &engine, seeds, &parsed.application.landing_projects, &root, &mut problems);
    if !problems.is_empty() {
        return Err(problems);
    }
    let store = crate::store::limits(&root, &engine.jig.limits.core, &root.forge, &engine.jig.limits.store)
        .ok_or(problem(Path::root().member(b"engine").member(b"limits"), What::Jig(config::What::Bound)))?;
    let io = match &worker {
        Some(worker) => checks::process_io(&engine.jig.limits.io, Some(&worker.jig.limits.io)),
        None => checks::process_io(&engine.jig.limits.io, None),
    }
    .ok_or(problem(Path::root().member(b"engine").member(b"limits").member(b"io"), What::Jig(config::What::Bound)))?;
    let prepared_forges = prepare_forges(&engine, forges)?;
    let engine_bytes = match crate::system::startup_worst_case(
        &engine,
        &root,
        &store,
        accounts,
        worker_operations,
        &io,
        (prepared_forges.1, prepared_forges.2, &prepared_forges.0),
    ) {
        Ok(bytes) => bytes,
        Err(found) => return Err(Problems::jig(found, &Path::root())),
    };
    Ok(CheckedStart {
        not_set_up,
        engine,
        worker,
        requirements: Requirements { engine_bytes, worker_bytes: bytes },
        io,
        forges: prepared_forges,
    })
}
fn read_instructions(core: &mut jig_domain_core::Config, files: &settings::Files) {
    for charter in &mut core.defaults.charters {
        if let Some(contents) = files.contents(&charter.policy.instructions) {
            charter.policy.instructions = Box::from(contents);
        }
    }
}
/// Build the engine and worker together from the parsed document and read files.
#[expect(clippy::too_many_arguments, reason = "startup receives the same directories as named plus the runtime facts")]
pub fn startup(
    document: &[u8],
    directories: &config::Directories,
    files: &settings::Files,
    agent: &settings::Agent,
    seeds: &settings::Seeds,
    accounts: Option<AccountsRuntime>,
    runtime: WorkerRuntime,
    forges: List<crate::ForgeRuntime>,
) -> Result<Service, Problems> {
    with_setup(document, directories, files, Some(agent), seeds, accounts, Some(runtime), forges, None, false)
}
/// Build the standalone service with the binary's checked, resolved forge setup.
#[expect(clippy::too_many_arguments, reason = "startup receives the checked setup beside the same runtime facts")]
pub fn with_setup(
    document: &[u8],
    directories: &config::Directories,
    files: &settings::Files,
    agent: Option<&settings::Agent>,
    seeds: &settings::Seeds,
    accounts: Option<AccountsRuntime>,
    runtime: Option<WorkerRuntime>,
    forges: List<crate::ForgeRuntime>,
    setup: Option<&temper_forge_setup::check::Checked>,
    under_box: bool,
) -> Result<Service, Problems> {
    let CheckedStart { engine, worker, requirements, io, forges, not_set_up } =
        checked_start(document, directories, files, agent, seeds, accounts.as_ref(), forges, setup)?;
    let mut problems = Problems::empty();
    let worker_memory = if let Some(worker) = &worker { worker.jig.memory } else { 0 };
    checks::memory(
        requirements.engine_bytes,
        engine.jig.memory,
        requirements.worker_bytes,
        worker_memory,
        &mut problems,
    );
    if !problems.is_empty() {
        return Err(problems);
    }
    let worker = match worker {
        Some(worker) => match Worker::new(
            &worker,
            agent.expect("checked local agent"),
            &engine.limits,
            files,
            runtime.ok_or(problem(indexed(0), What::Standalone))?,
            seeds.seed,
        ) {
            Ok(worker) => Some(worker),
            Err(found) => return Err(Problems::jig(found, &Path::root())),
        },
        None => None,
    };
    match Service::prepared(engine, seeds, accounts, forges, worker, requirements.worker_bytes, io, under_box) {
        Ok(mut service) => {
            for number in &not_set_up {
                crate::setup::lifecycle::state(
                    service.engine.as_mut().expect("engine startup"),
                    *number,
                    temper_engine_domain_forge::Connection::NotSetUp,
                );
            }
            Ok(service)
        }
        Err(found) => Err(Problems::jig(found, &Path::root())),
    }
}

fn prepare_forges(
    engine: &settings::Engine,
    forges: List<crate::ForgeRuntime>,
) -> Result<(skein_forge_connection::Settings, u64, u64), Problems> {
    let (settings, roots) = match crate::forge::prepare(engine, forges) {
        Ok(prepared) => prepared,
        Err(found) => return Err(Problems::jig(found, &Path::root())),
    };
    let owned = skein_forge_connection::settings::owned_bytes(&settings)
        .ok_or(problem(Path::root().member(b"engine").member(b"memory_bytes"), What::Jig(config::What::Bound)))?;
    Ok((settings, roots, owned))
}

/// The worker-only document, with no deployment or engine declarations.
#[derive(Debug)]
pub struct WorkerPart {
    pub worker: settings::Worker,
    pub limits: crate::limits::Limits,
    pub named: NamedWorker,
    pub files: Box<[config::File]>,
}
/// Read exactly a worker's part for serve and program-login (protocol/README.md, 15.1).
pub fn worker_part(input: &[u8], directories: &config::Directories) -> Result<WorkerPart, Problems> {
    if shape(input, directories)? != Shape::WorkerAlone {
        return Err(problem(Path::root(), What::WorkerAlone));
    }
    let document = config::document(
        input,
        &config::DocumentLimits {
            bytes: 1_048_576,
            tokens: u32::try_from(input.len()).expect("checked document"),
            depth: 32,
        },
    )
    .expect("checked shape document");
    let at = member(&document, 0, b"workers").expect("checked workers");
    let ats = read::list(&document, at, document.len()).expect("checked worker list");
    let jig = config::worker(&document, *ats.get(0).expect("one worker"), directories).expect("checked worker");
    let mut problems = Problems::empty();
    let (own, limits) = application::worker_alone(
        &document,
        member(&document, 0, b"application").expect("checked application"),
        directories,
        &mut problems,
    )
    .expect("checked worker part");
    let mut files = config::worker_files(&jig);
    for file in &mut files {
        let mut path = indexed(0);
        for segment in &file.at.segments {
            path = path.child(segment.clone());
        }
        file.at = path;
    }
    Ok(WorkerPart {
        named: NamedWorker { root: own.root.clone(), agent: jig.agent.configuration.clone() },
        worker: settings::Worker { jig, root: own.root, git: own.git },
        limits,
        files,
    })
}
/// Construct a worker-only service; its complete envelope belongs to the worker.
pub fn worker_alone(
    part: WorkerPart,
    files: &settings::Files,
    agent: &settings::Agent,
    runtime: WorkerRuntime,
    seed: u64,
) -> Result<Service, Problems> {
    check_worker_part(&part, files, agent)?;
    match Service::worker_alone(&part.worker, &part.limits, agent, files, runtime, seed) {
        Ok(service) => Ok(service),
        Err(found) => Err(Problems::jig(found, &Path::root())),
    }
}

fn check_worker_part(part: &WorkerPart, files: &settings::Files, agent: &settings::Agent) -> Result<(), Problems> {
    let mut problems = Problems::empty();
    for file in &part.files {
        match files.contents(&file.path) {
            Some(bytes) if bytes.len() <= usize::try_from(file.max).expect("file bound") => {}
            Some(_) => problems.push(file.at.clone(), What::Jig(config::What::Bound)),
            None => problems.push(file.at.clone(), What::Jig(config::What::Unreadable)),
        }
    }
    checks::worker(&crate::worker_settings::root_limits(&part.worker, agent, &part.limits), &mut problems);
    checks::turns(None, core::slice::from_ref(&part.worker.jig), &part.limits, Some(agent), &mut problems);
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(())
}
