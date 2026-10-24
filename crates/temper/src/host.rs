//! The shell's adapter over the temper composition (jig's protocol/service.md, 7).
use crate::directories::Directories;
use skein_io::kernel::{Complete, Exit, Submit};
use skein_lib::{Queue, Time, Wall};
use std::io::{self, Write};
use temper_service::{End, Service, Stop};

pub struct Host {
    pub service: Service,
    directories: Directories,
    pub(super) store: Box<[u8]>,
    output: Box<dyn Write>,
    output_failed: bool,
}
impl Host {
    #[must_use]
    pub fn new(service: Service, directories: Directories) -> Self {
        let store = directories.data.clone();
        Self { service, directories, store, output: Box::new(io::stdout()), output_failed: false }
    }
}
impl skein_shell::Host for Host {
    fn iterate(&mut self, now: Time, wall: Wall) {
        temper_service::iterate(&mut self.service, now, wall);
    }
    fn completions(&mut self) -> &mut Queue<Complete> {
        self.service.completions()
    }
    fn submissions(&mut self) -> &mut Queue<Submit> {
        self.service.submissions()
    }
    fn work_pending(&self, now: Time) -> bool {
        self.service.work_pending(now)
    }
    fn next_deadline(&self) -> Option<Time> {
        self.service.next_deadline()
    }
    fn next_policy_deadline(&self) -> Option<Time> {
        self.service.next_policy_deadline()
    }
    fn is_empty(&self) -> bool {
        self.service.is_empty()
    }
    fn worst_case(&self) -> u64 {
        self.service.worst_case()
    }
    fn operations(&self) -> u32 {
        self.service.operations()
    }
    fn exit(&self) -> Option<Exit> {
        if !self.service.is_empty() {
            return None;
        }
        let (lines, code) = ending(self.service.end(), self.output_failed, &self.directories, &self.store);
        eprint!("{lines}");
        Some(Exit::Code(code))
    }
    fn drain(&mut self) {
        while let Some(read) = self.service.take_handover_read() {
            let answer = read_handover(&read);
            self.service.handover_read(read.forge, read.run, answer);
        }
        while let Some(remove) = self.service.take_handover_removal() {
            let removed = remove_handover(&remove);
            self.service.handover_removed(remove.forge, remove.run, removed);
        }
        while let Some(note) = self.service.listening().pop() {
            let lines = note_line(note);
            eprint!("{lines}");
        }
        while let Some(note) = self.service.notes().pop() {
            let lines = note_line(note);
            eprint!("{lines}");
        }
        while let Some(link) = self.service.links().pop() {
            if self
                .output
                .write_all(&link)
                .and_then(|()| self.output.write_all(b"\n"))
                .and_then(|()| self.output.flush())
                .is_err()
            {
                self.output_failed = true;
                self.service.shutdown();
            }
        }
    }
}

pub(crate) fn read_handover(
    read: &temper_service::HandoverRead,
) -> Result<Option<skein_lib::Secret>, temper_service::HandoverUnread> {
    use temper_service::HandoverUnread;
    match skein_shell::read_private_file(crate::path(&read.path), read.max, skein_shell::effective_user()) {
        Ok(value) => {
            let bytes = value.expose();
            let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
            Ok(Some(skein_lib::Secret::new(bytes.into())))
        }
        Err(skein_shell::Unread::Absent) => Ok(None),
        Err(skein_shell::Unread::NotAFile) => Err(HandoverUnread::NotAFile),
        Err(skein_shell::Unread::TooLarge { .. }) => Err(HandoverUnread::TooLarge),
        Err(skein_shell::Unread::Unsafe(why)) => Err(HandoverUnread::Unsafe(why)),
        Err(skein_shell::Unread::Kernel(error)) => Err(HandoverUnread::Kernel(crate::errno(error))),
    }
}
fn remove_handover(remove: &temper_service::HandoverRemoval) -> Result<(), i32> {
    match std::fs::remove_file(crate::path(&remove.path)) {
        Ok(()) => Ok(()),
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(why) => Err(why.raw_os_error().expect("unlink returns an errno")),
    }
}

fn ending(end: Option<&End>, output_failed: bool, _directories: &Directories, store: &[u8]) -> (String, u8) {
    let lines = match end {
        Some(End::Stopped(why)) => match why {
            Stop::Start(found) => crate::problems(&temper_service::startup::Problems::jig(
                found.clone(),
                &jig_protocol_config::Path::root(),
            )),
            Stop::StoreOpen { why } => {
                let deployment = crate::path(store);
                format!("temper serve: {}\n", why.words().join(&deployment.display().to_string()))
            }
            Stop::CommitFailed { number } => format!("temper serve: commit {number} failed\n"),
            Stop::Hooks => "temper serve: webhooks listener failed\n".into(),
            Stop::Listener => "temper serve: client listener failed\n".into(),
        },
        None | Some(End::Drained | End::Aborted) => String::new(),
    };
    (lines, u8::from(output_failed || end != Some(&End::Drained)))
}

fn note_line(note: temper_service::Note) -> String {
    match note {
        temper_service::Note::Patched { of, patch, counts } => {
            format!("temper serve: {of:?} patch {patch}: {} records, {} commits\n", counts.records, counts.commits)
        }
        temper_service::Note::Listening { listener, addr } => {
            let name = match listener {
                temper_service::Listening::Clients => "clients",
                temper_service::Listening::Workers => "workers",
                temper_service::Listening::Webhooks => "webhooks",
            };
            format!("listening {name} {addr}\n")
        }
        temper_service::Note::Linked { worker, address } => {
            format!("linked {} {address}\n", String::from_utf8_lossy(&worker))
        }
        temper_service::Note::Refused { worker, address, reason } => {
            let reason = refusal(reason);
            format!("refused {} {address} {reason}\n", String::from_utf8_lossy(&worker))
        }
        temper_service::Note::ForgeLacking { number, names } => {
            format!("temper serve: forge {number}: App lacks {}\n", String::from_utf8_lossy(&names))
        }
        temper_service::Note::SetUpRefused { number, why } => {
            use temper_engine_domain_forge::SetUpRefusal;
            let why = match why {
                SetUpRefusal::NotUnderTheBox => "not under the box",
                SetUpRefusal::NotOffered => "not offered",
                SetUpRefusal::Busy => "busy",
                SetUpRefusal::NewProject => "a new project waits on jig's projects at runtime",
                SetUpRefusal::Invalid => "invalid: the organization, repository or project name is empty or too long",
            };
            format!("temper serve: forge {number}: set up refused: {why}\n")
        }
        temper_service::Note::Demo { line } | temper_service::Note::BoxLink { line } => {
            format!("{}\n", String::from_utf8_lossy(&line))
        }
        temper_service::Note::ForgeStopped { number, why } => stopped_line(number, why),
        temper_service::Note::ForgeSetupStopped { number, why } => {
            use temper_service::{SetupPlan, SetupStopped, SetupWhy};
            let why = match why {
                SetupStopped::NotPrepared => "prepare it with `temper forge prepare`".into(),
                SetupStopped::Missing => "setup host missing".into(),
                SetupStopped::Busy => "setup host busy".into(),
                SetupStopped::Removal { path, errno } => {
                    format!("removing the handover: {}: errno {errno}", crate::path(&path).display())
                }
                SetupStopped::Unreadable { path, why } => {
                    format!("the handover is unreadable: {}: {why:?}", crate::path(&path).display())
                }
                SetupStopped::Stopped { plan, step } => {
                    let reason = match step.why {
                        SetupWhy::Refused(status) => format!("refused {status}"),
                        why @ (SetupWhy::Missing
                        | SetupWhy::NotAdmin
                        | SetupWhy::BotIsAdmin
                        | SetupWhy::Unmarked
                        | SetupWhy::OtherUser
                        | SetupWhy::OtherApp
                        | SetupWhy::KeyForm
                        | SetupWhy::Unknown
                        | SetupWhy::Busy
                        | SetupWhy::Crowded
                        | SetupWhy::Down
                        | SetupWhy::Unexpected) => format!("{why:?}"),
                    };
                    let action = match plan {
                        SetupPlan::Reconcile => "reconciling",
                        SetupPlan::TakeOver => "taking the handover",
                        SetupPlan::Adopt => "adopting",
                        SetupPlan::Create => "making",
                    };
                    format!("{action} stopped at {:?}: {reason}", step.stage)
                }
            };
            format!("temper serve: forge {number}: Not connected: {why}\n")
        }
        temper_service::Note::ForgeNotConnected { number, why } => {
            format!("temper serve: forge {number}: not connected: {why}\n")
        }
        temper_service::Note::ForgeUnconfigured { number } => {
            format!("temper serve: forge {number}: its records are not configured\n")
        }
        temper_service::Note::ForgeUnkept { number, installation } => {
            format!("temper serve: forge {number}: installation {installation}: not kept\n")
        }
    }
}
fn refusal(reason: temper_service::Refusal) -> String {
    use temper_service::WorkerRefusalReason as Reason;
    match reason {
        temper_service::Refusal::Terms => "terms".into(),
        temper_service::Refusal::Malformed => "malformed".into(),
        temper_service::Refusal::Peer(code) => match Reason::from_code(code) {
            Some(Reason::Unauthorized) => "unauthorized".into(),
            Some(Reason::Application) => "application".into(),
            Some(Reason::Busy) => "busy".into(),
            Some(Reason::Replaced) => "replaced".into(),
            Some(Reason::NotAdmitted) => "not-admitted".into(),
            Some(Reason::Stopping) => "stopping".into(),
            None => format!("code-{code}"),
        },
    }
}
fn stopped_line(number: u16, why: skein_forge_connection::Stopped) -> String {
    use skein_forge_connection::{StartRead, Stopped, Value};
    match why {
        Stopped::Invalid { value } => {
            let member = match value {
                Value::Credential | Value::Token => "api_token",
                Value::User => "user",
                Value::Key => "app_key",
                Value::Secret => "webhook_secret",
            };
            format!("temper serve: forge {number}: {member}: invalid\n")
        }
        Stopped::KeyRefused => format!("temper serve: forge {number}: app_key: refused\n"),
        Stopped::TokenRefused { installation, status } => {
            format!("temper serve: forge {number}: /app/installations/{installation}/access_tokens: status {status}\n")
        }
        Stopped::OtherUser { user } => {
            format!("temper serve: forge {number}: start answered as user {user}\n")
        }
        Stopped::Refused { read, status } => {
            let path = match read {
                StartRead::Installations => "/app/installations",
                StartRead::Mint => "/app/installations/{id}/access_tokens",
                StartRead::Viewer => "/graphql viewer",
                StartRead::User => "/api/v1/user",
                StartRead::ApiSettings => "/api/v1/settings/api",
            };
            format!("temper serve: forge {number}: {path}: status {status}\n")
        }
    }
}

#[test]
fn github_start_refusals_name_their_paths() {
    use skein_forge_connection::{StartRead, Stopped};
    for (why, words) in [
        (Stopped::KeyRefused, "app_key: refused"),
        (Stopped::TokenRefused { installation: 7, status: 403 }, "/app/installations/7/access_tokens: status 403"),
        (Stopped::Refused { read: StartRead::Installations, status: 401 }, "/app/installations: status 401"),
        (Stopped::Refused { read: StartRead::Mint, status: 401 }, "/app/installations/{id}/access_tokens: status 401"),
        (Stopped::Refused { read: StartRead::Viewer, status: 401 }, "/graphql viewer: status 401"),
    ] {
        assert_eq!(stopped_line(1, why), format!("temper serve: forge 1: {words}\n"));
    }
}

#[test]
fn records_and_installation_notes_name_the_forge_and_reason() {
    use temper_service::{NotConnected, Note};
    assert_eq!(
        note_line(Note::ForgeNotConnected { number: 7, why: NotConnected::Unreadable }),
        "temper serve: forge 7: not connected: its records are unreadable\n"
    );
    assert_eq!(
        note_line(Note::ForgeNotConnected {
            number: 7,
            why: NotConnected::OtherForge {
                kind: skein_forge_connection::settings::Kind::Forgejo,
                api: Box::from("fake.test:443")
            }
        }),
        "temper serve: forge 7: not connected: its records were made for forgejo at fake.test:443\n"
    );
    assert_eq!(
        note_line(Note::ForgeUnconfigured { number: 8 }),
        "temper serve: forge 8: its records are not configured\n"
    );
    assert_eq!(
        note_line(Note::ForgeUnkept { number: 7, installation: 93 }),
        "temper serve: forge 7: installation 93: not kept\n"
    );
}

#[test]
fn older_store_refusals_use_jigs_recovery_words_and_the_store_path() {
    use jig_protocol_store::{Family, Older, OpenRefusal};
    let arguments =
        [b"--config".as_slice(), b"/configuration", b"--data", b"/data", b"--cache", b"/cache"].map(Box::from);
    let (directories, _) = crate::directories::resolve(&arguments, &[], b"/").expect("resolved directories");
    for older in [Older::Unleveled, Older::Unrecorded, Older::Layout(Family::Application), Older::Directory] {
        let why = OpenRefusal::OlderBuild { why: older };
        for store in ["/data/deployment", "/elsewhere/store"] {
            let expected = format!("temper serve: {}\n", why.words().join(store));
            let (line, code) = ending(
                Some(&End::Stopped(Stop::StoreOpen { why: why.clone() })),
                false,
                &directories,
                store.as_bytes(),
            );
            assert_eq!(line, expected);
            assert!(line.contains("this start has changed nothing"));
            assert!(line.contains("together"));
            assert!(line.contains("every client signs in again"));
            assert_eq!(code, 1);
        }
    }
}

#[test]
fn other_endings_keep_their_exit_behavior() {
    let arguments =
        [b"--config".as_slice(), b"/configuration", b"--data", b"/data", b"--cache", b"/cache"].map(Box::from);
    let (directories, _) = crate::directories::resolve(&arguments, &[], b"/").expect("resolved directories");
    let mut problems = jig_protocol_config::Problems::empty();
    problems.push(jig_protocol_config::Path::root().member(b"deployment"), jig_protocol_config::What::Refers);
    for (end, output_failed, expected, code) in [
        (Some(End::Stopped(Stop::Start(problems))), false, "temper serve: deployment: unknown reference\n", 1),
        (Some(End::Stopped(Stop::CommitFailed { number: 7 })), false, "temper serve: commit 7 failed\n", 1),
        (Some(End::Stopped(Stop::Hooks)), false, "temper serve: webhooks listener failed\n", 1),
        (Some(End::Stopped(Stop::Listener)), false, "temper serve: client listener failed\n", 1),
        (Some(End::Drained), false, "", 0),
        (Some(End::Drained), true, "", 1),
        (Some(End::Aborted), false, "", 1),
        (None, false, "", 1),
    ] {
        assert_eq!(
            ending(end.as_ref(), output_failed, &directories, b"/data/store"),
            (expected.into(), code),
            "{end:?}, output failed {output_failed}"
        );
    }
}

#[test]
fn listening_notes_print_each_listener_and_socket_address_exactly() {
    use temper_service::{Listening, Note};
    for (listener, name) in
        [(Listening::Clients, "clients"), (Listening::Workers, "workers"), (Listening::Webhooks, "webhooks")]
    {
        for (addr, printed) in [
            (([127, 0, 0, 1], 41733).into(), "127.0.0.1:41733"),
            (([0, 0, 0, 0, 0, 0, 0, 1], 41733).into(), "[::1]:41733"),
        ] {
            assert_eq!(note_line(Note::Listening { listener, addr }), format!("listening {name} {printed}\n"));
        }
    }
}

#[test]
fn worker_link_notes_print_each_address_and_refusal_exactly() {
    use temper_service::{Note, Refusal};
    for (address, printed) in
        [(([127, 0, 0, 1], 7100).into(), "127.0.0.1:7100"), (([0, 0, 0, 0, 0, 0, 0, 1], 7100).into(), "[::1]:7100")]
    {
        assert_eq!(
            note_line(Note::Linked { worker: b"worker-a".as_slice().into(), address: Box::new(address) }),
            format!("linked worker-a {printed}\n")
        );
        for (reason, expected) in [
            (Refusal::Peer(256), "unauthorized"),
            (Refusal::Peer(257), "application"),
            (Refusal::Peer(258), "busy"),
            (Refusal::Peer(259), "replaced"),
            (Refusal::Peer(260), "not-admitted"),
            (Refusal::Peer(261), "stopping"),
            (Refusal::Terms, "terms"),
            (Refusal::Malformed, "malformed"),
            (Refusal::Peer(42), "code-42"),
        ] {
            assert_eq!(
                note_line(Note::Refused { worker: b"worker-a".as_slice().into(), address: Box::new(address), reason }),
                format!("refused worker-a {printed} {expected}\n")
            );
        }
    }
}

#[test]
fn setup_stops_print_the_forge_plan_and_typed_step_without_a_subject() {
    use temper_service::{Note, SetupPlan, SetupStage, SetupStep, SetupStopped, SetupWhy};
    for (why, line) in [
        (SetupStopped::NotPrepared, "prepare it with `temper forge prepare`"),
        (SetupStopped::Missing, "setup host missing"),
        (SetupStopped::Busy, "setup host busy"),
        (
            SetupStopped::Stopped {
                plan: SetupPlan::Reconcile,
                step: SetupStep { stage: SetupStage::Repository, why: SetupWhy::Refused(403) },
            },
            "reconciling stopped at Repository: refused 403",
        ),
    ] {
        assert_eq!(
            note_line(Note::ForgeSetupStopped { number: 3, why }),
            format!("temper serve: forge 3: Not connected: {line}\n")
        );
    }
}

#[test]
fn the_shell_removes_only_the_named_handover_and_reports_unlink_failure() {
    use std::os::unix::ffi::OsStrExt;
    let directory =
        std::env::temp_dir().join(format!("temper-handover-{}", skein_shell::seed().expect("scratch name")));
    std::fs::create_dir(&directory).expect("handover fixture");
    let path = directory.join("admin_token");
    let removal = temper_service::HandoverRemoval {
        forge: 1,
        run: skein_lib::Token::new(7),
        path: path.as_os_str().as_bytes().into(),
    };
    assert!(remove_handover(&removal).is_ok(), "an absent handover counts as removed");
    std::fs::create_dir(&path).expect("handover fixture");
    std::fs::write(path.join("held"), b"").expect("handover fixture");
    let errno = remove_handover(&removal).expect_err("an unlink failure is never acknowledged as removed");
    let line = note_line(temper_service::Note::ForgeSetupStopped {
        number: removal.forge,
        why: temper_service::SetupStopped::Removal { path: removal.path.clone(), errno },
    });
    assert!(line.contains("removing the handover:") && line.contains(path.to_str().expect("handover fixture")));
    std::fs::remove_dir_all(&path).expect("handover fixture");
    std::fs::write(&path, b"").expect("handover fixture");
    assert!(remove_handover(&removal).is_ok());
    assert!(!path.exists());
    std::fs::remove_dir(&directory).expect("handover fixture");
}
#[test]
fn the_shell_trims_only_the_handovers_final_newline() {
    use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
    let directory =
        std::env::temp_dir().join(format!("temper-handover-read-{}", skein_shell::seed().expect("scratch name")));
    std::fs::create_dir(&directory).expect("handover fixture");
    let path = directory.join("admin_token");
    std::fs::write(&path, b"test-admin\n").expect("handover fixture");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("handover fixture");
    let read = temper_service::HandoverRead {
        forge: 1,
        run: skein_lib::Token::new(7),
        path: path.as_os_str().as_bytes().into(),
        max: 11,
    };
    let answer = read_handover(&read).expect("handover fixture").expect("handover fixture");
    assert!(answer.expose() == b"test-admin", "the final LF is not part of the handover");
    std::fs::remove_dir_all(directory).expect("handover fixture");
}
