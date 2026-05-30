//! Hand-rolled argument parsing for the `harness-testing-worker` binary.
//!
//! A heavyweight CLI framework would be the only reason to add a new dependency
//! to this crate, so the parser here is deliberately small and table-free: it
//! walks `--flag value` pairs and validates them into a [`WorkerArgs`] value.
//! Keep it dependency-light; if the surface grows past a handful of flags,
//! reconsider a small lockfile crate rather than hand-rolling more.

use chrono::Duration;
use std::fmt;
use std::path::PathBuf;

/// Which worker a binary invocation should run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerKind {
    /// One-shot: create the repository and upsert every workflow label.
    Provision,
    /// Per-role worker servicing a single workflow role.
    Role {
        /// Workflow role id this worker services.
        role: String,
        /// Forge user handle the worker acts as.
        user: String,
        /// Which fake agent variants populate this worker's registry.
        behavior: RoleBehavior,
    },
    /// Controller-plane mechanical reconcile/apply worker.
    Mechanical,
    /// Test-only fake CI producer.
    Ci {
        /// CI verdict policy.
        policy: CiPolicyKind,
    },
}

/// CI producer policy selectable from the command line via `--ci`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum CiPolicyKind {
    /// Every visited pull request passes (the default).
    #[default]
    Pass,
    /// Fail the first verdict per head, then pass.
    FailThenPass,
    /// Always fail.
    FixedFail,
}

/// Which fake architect variant a `role` worker registers (`--architect`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum ArchitectKind {
    /// Reconciles landed PRs but leaves produced parent issues open (default).
    #[default]
    Default,
    /// Also closes a merged PR's produced parent issues, unblocking dependents.
    Closing,
}

/// Which fake reviewer variant a `role` worker registers (`--reviewer`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum ReviewerKind {
    /// Approves on the first review (default).
    #[default]
    Default,
    /// Requests changes on the first review, approves on the next.
    RequestChangesThenApprove,
}

/// The fake agent variants that populate a `role` worker's registry.
///
/// Only the architect and reviewer have behavior variants; every other role
/// uses its single fake. These map one-to-one onto the in-process scenario
/// wiring in `harness-runner/tests/end_to_end.rs` so the same scenarios converge
/// across both topologies (see `docs/how-to/run-multiprocess-e2e.md`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub struct RoleBehavior {
    /// Architect variant.
    pub architect: ArchitectKind,
    /// Reviewer variant.
    pub reviewer: ReviewerKind,
}

/// Which clock a poll-loop worker drives its ticks from.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum ClockKind {
    /// Deterministic [`ManualClock`](harness_runner::ManualClock) seeded near the
    /// filesystem backend logical-clock origin (the default).
    #[default]
    Deterministic,
    /// Wall-clock time, the mode a real provider backend would use.
    Wall,
}

/// Fully parsed and validated worker invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerArgs {
    /// Which worker to run.
    pub kind: WorkerKind,
    /// Filesystem store root shared by every worker process.
    pub root: PathBuf,
    /// Repository owner.
    pub owner: String,
    /// Repository name.
    pub name: String,
    /// Poll cadence between ticks.
    pub poll_interval: Duration,
    /// Sentinel file whose existence stops the run loop.
    pub stop_file: Option<PathBuf>,
    /// Maximum wall-clock seconds to run before stopping; `None` runs until the
    /// stop file appears.
    pub run_secs: Option<u64>,
    /// Clock fidelity for poll-loop ticks.
    pub clock: ClockKind,
}

/// An argument-parsing failure with a user-facing message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArgsError(String);

impl ArgsError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ArgsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ArgsError {}

/// Outcome of parsing the raw argument vector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseOutcome {
    /// A fully validated worker invocation.
    Run(Box<WorkerArgs>),
    /// `--help` was requested; the caller should print usage and exit zero.
    Help,
}

/// One-line usage string for `--help` and error context.
pub const USAGE: &str = concat!(
    "harness-testing-worker --kind <provision|role|mechanical|ci> --root <path> ",
    "--repo <owner/name> [--role <id> --user <handle>] ",
    "[--architect <default|closing>] [--reviewer <default|request-changes-then-approve>] ",
    "[--ci <pass|fail-then-pass|fixed-fail>] ",
    "[--poll-ms <n>] [--stop-file <path>] [--run-secs <max>] [--clock <deterministic|wall>]",
);

/// Parses the process argument vector (excluding the program name).
pub fn parse<I>(args: I) -> Result<ParseOutcome, ArgsError>
where
    I: IntoIterator<Item = String>,
{
    let raw = RawArgs::collect(args)?;
    if raw.help {
        return Ok(ParseOutcome::Help);
    }
    raw.into_worker_args()
        .map(|args| ParseOutcome::Run(Box::new(args)))
}

/// Raw, loosely typed flag values before cross-field validation.
struct RawArgs {
    help: bool,
    kind: Option<String>,
    root: Option<String>,
    repo: Option<String>,
    role: Option<String>,
    user: Option<String>,
    architect: Option<String>,
    reviewer: Option<String>,
    ci: Option<String>,
    poll_ms: Option<String>,
    stop_file: Option<String>,
    run_secs: Option<String>,
    clock: Option<String>,
}

impl RawArgs {
    fn collect<I>(args: I) -> Result<Self, ArgsError>
    where
        I: IntoIterator<Item = String>,
    {
        let mut raw = RawArgs {
            help: false,
            kind: None,
            root: None,
            repo: None,
            role: None,
            user: None,
            architect: None,
            reviewer: None,
            ci: None,
            poll_ms: None,
            stop_file: None,
            run_secs: None,
            clock: None,
        };
        let mut iter = args.into_iter();
        while let Some(flag) = iter.next() {
            match flag.as_str() {
                "--help" | "-h" => raw.help = true,
                "--kind" => raw.kind = Some(value_for(&flag, &mut iter)?),
                "--root" => raw.root = Some(value_for(&flag, &mut iter)?),
                "--repo" => raw.repo = Some(value_for(&flag, &mut iter)?),
                "--role" => raw.role = Some(value_for(&flag, &mut iter)?),
                "--user" => raw.user = Some(value_for(&flag, &mut iter)?),
                "--architect" => raw.architect = Some(value_for(&flag, &mut iter)?),
                "--reviewer" => raw.reviewer = Some(value_for(&flag, &mut iter)?),
                "--ci" => raw.ci = Some(value_for(&flag, &mut iter)?),
                "--poll-ms" => raw.poll_ms = Some(value_for(&flag, &mut iter)?),
                "--stop-file" => raw.stop_file = Some(value_for(&flag, &mut iter)?),
                "--run-secs" => raw.run_secs = Some(value_for(&flag, &mut iter)?),
                "--clock" => raw.clock = Some(value_for(&flag, &mut iter)?),
                other => {
                    return Err(ArgsError::new(format!(
                        "unrecognized argument '{other}'\nusage: {USAGE}"
                    )))
                }
            }
        }
        Ok(raw)
    }

    fn into_worker_args(self) -> Result<WorkerArgs, ArgsError> {
        let kind = self.parse_kind()?;
        let root = PathBuf::from(require(self.root, "--root")?);
        let (owner, name) = parse_repo(&require(self.repo, "--repo")?)?;
        let poll_interval = match self.poll_ms {
            Some(raw) => Duration::milliseconds(parse_i64(&raw, "--poll-ms")?),
            None => Duration::milliseconds(50),
        };
        let stop_file = self.stop_file.map(PathBuf::from);
        let run_secs = self
            .run_secs
            .map(|raw| parse_u64(&raw, "--run-secs"))
            .transpose()?;
        let clock = parse_clock(self.clock.as_deref())?;
        Ok(WorkerArgs {
            kind,
            root,
            owner,
            name,
            poll_interval,
            stop_file,
            run_secs,
            clock,
        })
    }

    fn parse_kind(&self) -> Result<WorkerKind, ArgsError> {
        let kind = self
            .kind
            .as_deref()
            .ok_or_else(|| ArgsError::new(format!("missing required --kind\nusage: {USAGE}")))?;
        match kind {
            "provision" => Ok(WorkerKind::Provision),
            "mechanical" => Ok(WorkerKind::Mechanical),
            "ci" => Ok(WorkerKind::Ci {
                policy: parse_ci(self.ci.as_deref())?,
            }),
            "role" => {
                let role = require_ref(self.role.as_deref(), "--role (required for --kind role)")?;
                let user = require_ref(self.user.as_deref(), "--user (required for --kind role)")?;
                let behavior = RoleBehavior {
                    architect: parse_architect(self.architect.as_deref())?,
                    reviewer: parse_reviewer(self.reviewer.as_deref())?,
                };
                Ok(WorkerKind::Role {
                    role,
                    user,
                    behavior,
                })
            }
            other => Err(ArgsError::new(format!(
                "unknown --kind '{other}'; expected provision|role|mechanical|ci"
            ))),
        }
    }
}

fn value_for<I>(flag: &str, iter: &mut I) -> Result<String, ArgsError>
where
    I: Iterator<Item = String>,
{
    iter.next()
        .ok_or_else(|| ArgsError::new(format!("flag '{flag}' expects a value")))
}

fn require(value: Option<String>, flag: &str) -> Result<String, ArgsError> {
    value.ok_or_else(|| ArgsError::new(format!("missing required {flag}\nusage: {USAGE}")))
}

fn require_ref(value: Option<&str>, flag: &str) -> Result<String, ArgsError> {
    value
        .map(str::to_string)
        .ok_or_else(|| ArgsError::new(format!("missing required {flag}\nusage: {USAGE}")))
}

fn parse_repo(repo: &str) -> Result<(String, String), ArgsError> {
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| ArgsError::new(format!("--repo must be owner/name, got '{repo}'")))?;
    if owner.is_empty() || name.is_empty() {
        return Err(ArgsError::new(format!(
            "--repo must be owner/name with non-empty parts, got '{repo}'"
        )));
    }
    Ok((owner.to_string(), name.to_string()))
}

fn parse_ci(ci: Option<&str>) -> Result<CiPolicyKind, ArgsError> {
    match ci {
        None | Some("pass") => Ok(CiPolicyKind::Pass),
        Some("fail-then-pass") => Ok(CiPolicyKind::FailThenPass),
        Some("fixed-fail") => Ok(CiPolicyKind::FixedFail),
        Some(other) => Err(ArgsError::new(format!(
            "unknown --ci '{other}'; expected pass|fail-then-pass|fixed-fail"
        ))),
    }
}

fn parse_architect(architect: Option<&str>) -> Result<ArchitectKind, ArgsError> {
    match architect {
        None | Some("default") => Ok(ArchitectKind::Default),
        Some("closing") => Ok(ArchitectKind::Closing),
        Some(other) => Err(ArgsError::new(format!(
            "unknown --architect '{other}'; expected default|closing"
        ))),
    }
}

fn parse_reviewer(reviewer: Option<&str>) -> Result<ReviewerKind, ArgsError> {
    match reviewer {
        None | Some("default") => Ok(ReviewerKind::Default),
        Some("request-changes-then-approve") => Ok(ReviewerKind::RequestChangesThenApprove),
        Some(other) => Err(ArgsError::new(format!(
            "unknown --reviewer '{other}'; expected default|request-changes-then-approve"
        ))),
    }
}

fn parse_clock(clock: Option<&str>) -> Result<ClockKind, ArgsError> {
    match clock {
        None | Some("deterministic") => Ok(ClockKind::Deterministic),
        Some("wall") => Ok(ClockKind::Wall),
        Some(other) => Err(ArgsError::new(format!(
            "unknown --clock '{other}'; expected deterministic|wall"
        ))),
    }
}

fn parse_i64(raw: &str, flag: &str) -> Result<i64, ArgsError> {
    raw.parse::<i64>()
        .map_err(|_| ArgsError::new(format!("{flag} must be an integer, got '{raw}'")))
}

fn parse_u64(raw: &str, flag: &str) -> Result<u64, ArgsError> {
    raw.parse::<u64>().map_err(|_| {
        ArgsError::new(format!(
            "{flag} must be a non-negative integer, got '{raw}'"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    fn run(parts: &[&str]) -> WorkerArgs {
        match parse(argv(parts)).expect("args parse") {
            ParseOutcome::Run(args) => *args,
            ParseOutcome::Help => panic!("unexpected help outcome"),
        }
    }

    #[test]
    fn parses_provision() {
        let args = run(&[
            "--kind",
            "provision",
            "--root",
            "/tmp/x",
            "--repo",
            "acme/service",
        ]);
        assert_eq!(args.kind, WorkerKind::Provision);
        assert_eq!(args.owner, "acme");
        assert_eq!(args.name, "service");
        assert_eq!(args.clock, ClockKind::Deterministic);
    }

    #[test]
    fn parses_role_with_identity() {
        let args = run(&[
            "--kind",
            "role",
            "--role",
            "engineer",
            "--user",
            "engineer",
            "--root",
            "/tmp/x",
            "--repo",
            "acme/service",
            "--poll-ms",
            "10",
            "--clock",
            "wall",
        ]);
        assert_eq!(
            args.kind,
            WorkerKind::Role {
                role: "engineer".into(),
                user: "engineer".into(),
                behavior: RoleBehavior::default(),
            }
        );
        assert_eq!(args.poll_interval, Duration::milliseconds(10));
        assert_eq!(args.clock, ClockKind::Wall);
    }

    #[test]
    fn parses_role_behavior_variants() {
        let args = run(&[
            "--kind",
            "role",
            "--role",
            "reviewer",
            "--user",
            "reviewer",
            "--reviewer",
            "request-changes-then-approve",
            "--architect",
            "closing",
            "--root",
            "/tmp/x",
            "--repo",
            "acme/service",
        ]);
        assert_eq!(
            args.kind,
            WorkerKind::Role {
                role: "reviewer".into(),
                user: "reviewer".into(),
                behavior: RoleBehavior {
                    architect: ArchitectKind::Closing,
                    reviewer: ReviewerKind::RequestChangesThenApprove,
                },
            }
        );
    }

    #[test]
    fn parses_ci_policy() {
        let args = run(&[
            "--kind",
            "ci",
            "--ci",
            "fail-then-pass",
            "--root",
            "/tmp/x",
            "--repo",
            "acme/service",
        ]);
        assert_eq!(
            args.kind,
            WorkerKind::Ci {
                policy: CiPolicyKind::FailThenPass
            }
        );
    }

    #[test]
    fn rejects_bad_reviewer() {
        let error = parse(argv(&[
            "--kind",
            "role",
            "--role",
            "reviewer",
            "--user",
            "reviewer",
            "--reviewer",
            "bogus",
            "--root",
            "/tmp/x",
            "--repo",
            "acme/service",
        ]))
        .unwrap_err();
        assert!(error.to_string().contains("--reviewer"));
    }

    #[test]
    fn help_short_circuits() {
        assert_eq!(parse(argv(&["--help"])), Ok(ParseOutcome::Help));
    }

    #[test]
    fn role_requires_identity() {
        let error = parse(argv(&[
            "--kind",
            "role",
            "--root",
            "/tmp/x",
            "--repo",
            "acme/service",
        ]))
        .unwrap_err();
        assert!(error.to_string().contains("--role"));
    }

    #[test]
    fn rejects_bad_repo() {
        let error = parse(argv(&[
            "--kind",
            "provision",
            "--root",
            "/tmp/x",
            "--repo",
            "no-slash",
        ]))
        .unwrap_err();
        assert!(error.to_string().contains("owner/name"));
    }

    #[test]
    fn rejects_unknown_flag() {
        let error = parse(argv(&["--kind", "provision", "--bogus", "x"])).unwrap_err();
        assert!(error.to_string().contains("unrecognized argument"));
    }
}
