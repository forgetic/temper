//! The small profile and checked field overrides (protocol/configuration.md, section 7).
mod names;
use skein_lib::Duration;
use temper_engine_domain as engine;
use temper_engine_domain_forge as forge;
use temper_worker_domain_checkout as checkout;
/// Temper's profile for one person's machine.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Profile {
    Small,
}
/// Root's own limits, beyond jig's child limits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Root {
    pub journal: engine::JournalLimits,
    pub loads: engine::loads::Limits,
    pub brief: engine::BriefLimits,
}
/// The clients' listener's own limits: the browser's bundle (protocol/configuration.md, 7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Clients {
    /// The bundle's files, its page among them.
    pub bundle_files: u32,
    /// The bundle's files' bytes in all, held in memory while `temper serve` runs.
    pub bundle_bytes: u32,
}
/// People's sign-ins (protocol/sign-in.md, 6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SignIn {
    pub members: u32,
    pub pending: u32,
    pub deadline: Duration,
    pub fetches: u32,
    pub body_bytes: u32,
    pub url_bytes: u32,
    pub secret_bytes: u32,
}
/// Repositories read in one picker page (protocol/configuration.md, 7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Picker {
    pub page: u32,
}
/// Temper's domain limits, each under its owner's prefix.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub setup: temper_forge_setup::limits::Host,
    pub routes: Routes,
    pub records: Records,
    pub forges: Forges,
    pub forge_component: skein_forge_connection::Limits,
    pub root: Root,
    pub forge: forge::Limits,
    pub checkout: checkout::Limits,
    pub git_component: temper_worker_protocol_git::Limits,
    pub clients: Clients,
    pub sign_in: SignIn,
    pub picker: Picker,
}
/// Forge routes' query bound (protocol/configuration.md, 7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Routes {
    pub query_bytes: u32,
}
/// Routing bounds (protocol/configuration.md, 7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Records {
    pub value_bytes: u32,
}

/// Admit a moved file on jig's wire; setup still checks temper's values against
/// `records.value_bytes` (protocol/configuration.md, 7; jig's protocol/client.md, 5).
pub(crate) fn client_value_bytes(jig: &jig_protocol_config::Limits, own: &Limits) -> u32 {
    let value = u64::from(own.records.value_bytes).max(crate::clients::moved_bytes(jig));
    u32::try_from(u64::from(jig.clients.wire.keyed_value_some).min(value)).expect("bounded by the wire ceiling")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Forges {
    pub installations: u32,
    /// Maximum configured job-log origins per forge (protocol/configuration.md, 7).
    pub log_origins: u32,
}
/// Refusal of an override; the caller retains its name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refused {
    /// Unknown field path.
    Unknown,
    /// Value outside the field's representation.
    Range,
}
/// `forge_component.installations` default; temper-01 adds the configuration row.
pub const FORGE_INSTALLATIONS: u32 = 8;
/// Default bound for a token minted by the forge component.
pub const FORGE_TOKEN_BYTES: u32 = 512;
const CLIENT: skein_forge_client::Limits = skein_forge_client::Limits {
    // Logical calls retained, queued and in flight, from the engine forge world fixture.
    pending: 4,
    // Calls in flight, from the engine forge world fixture.
    calls: 2,
    // Requests allowed per rate window, from the engine forge world fixture.
    rate: 100,
    // Requests reserved for writes, from the engine forge world fixture.
    reserve: 0,
    // Rate window duration, from the engine forge world fixture.
    window: Duration::from_secs(10),
    // Owned bytes of one operation, from the engine forge world fixture.
    op_bytes: 512,
    // Decoded bytes of one answer: the small profile's 65,536-byte ceiling.
    answer_bytes: 65_536,
    // Rows in one answer page, from the engine forge world fixture.
    rows: 8,
    // Comment and review delivery stamps per resource, from the engine forge world fixture.
    inbox: 16,
    // Attempts of a read, from the engine forge world fixture.
    read_attempts: 3,
    // First read retry delay, from the engine forge world fixture.
    backoff: Duration::from_secs(1),
    // Maximum read retry delay, from the engine forge world fixture.
    backoff_max: Duration::from_secs(4),
    // Durable outbox entries, from the engine forge world fixture.
    entries: 8,
    // Attempts of a write, from the engine forge world fixture.
    write_attempts: 3,
    // Longest time a sent write may arrive, from the engine forge world fixture.
    lifetime: Duration::from_secs(10),
    // Allowance for clock movement, from the engine forge world fixture.
    clock_margin: Duration::from_secs(1),
    // Live forge resources, from the engine forge world fixture.
    resources: 8,
    // Repositories retained, from the engine forge world fixture.
    repositories: 2,
    // Normal resource polling interval, from the engine forge world fixture.
    poll: Duration::from_secs(5),
    // Maximum resource polling interval, from the engine forge world fixture.
    poll_max: Duration::from_secs(30),
    // Delay before a hinted read, from the engine forge world fixture.
    hinted: Duration::from_secs(1),
    // Delay before a slow read, from the engine forge world fixture.
    slow: Duration::from_secs(40),
    // Bound Forgejo's settling of mergeability and hinted CI after a move.
    settle: Duration::from_secs(30),
    // Facts awaiting drain, from the engine forge world fixture.
    facts: 16,
};
const FORGE: forge::Limits = forge::Limits {
    // Repositories retained, from the engine forge world fixture.
    repositories: 2,
    // Tasks participating in the forge, from the engine forge world fixture.
    tasks: 8,
    // Held task resources, from the engine forge world fixture.
    holds: 8,
    // Subscriptions to forge topics, from the engine forge world fixture.
    subscriptions: 8,
    // Durable outbox entries, from the engine forge world fixture.
    entries: 8,
    // At least jig small core.call_records: the root's worst case.
    named_calls: 32,
    // Live changes, from the engine forge world fixture.
    changes: 8,
    // Live issue projections, from the engine forge world fixture.
    issues: 8,
    // Held to the agent's two checkout repositories.
    resources_per_task: 2,
    // Paths retained by one subscription, from the engine forge world fixture.
    paths_per_subscription: 4,
    // Leaves 122 bytes for a git base within the checkout's 256 bytes.
    name_bytes: 64,
    // Output records reserved per forge step, from the engine forge world fixture.
    output: 80,
    // Facts awaiting drain, from the engine forge world fixture.
    facts: 16,
    // Adoptions in flight, from the engine forge world fixture.
    adoptions: 2,
    // Keyed application requests retained, from the engine forge world fixture.
    applications: 4,
    // protocol/sign-in.md, 5: small permits 64 durable member bindings.
    bindings: 64,
    // Collaborators kept by the forge, from the engine forge world fixture.
    collaborators: 8,
    // Landing operations retained, from the engine forge world fixture.
    landings: 8,
    // Project judge tables, from the engine forge world fixture.
    judge_projects: 2,
    // Criteria in each judge table, from the engine forge world fixture.
    judge_criteria: 8,
    // Forge brief sections gathering at once, from the engine forge world fixture.
    brief_sections: 32,
    // Bytes of one gathered forge section: at least the root's brief.read_bytes.
    brief_bytes: 4608,
    // Forge client calls, retries and outbox room.
    client: CLIENT,
    issue_policy: temper_engine_domain_forge_issues::Limits {
        // Items in an issue plan, from the engine forge world fixture.
        plan_items: 8,
        // Milestones in an issue projection, from the engine forge world fixture.
        milestones: 8,
        // Bytes in an issue title, from the engine forge world fixture.
        title_bytes: 128,
        // Bytes in an issue body, from the engine forge world fixture.
        body_bytes: 512,
        // Bytes in an issue comment, from the engine forge world fixture.
        comment_bytes: 256,
        // Interval between issue projection checks, from the engine forge world fixture.
        interval: Duration::from_secs(5),
    },
    change_policy: temper_engine_domain_forge_change::Limits {
        // Repair attempts, from the engine forge world fixture.
        repairs: 3,
        // Resolution attempts, from the engine forge world fixture.
        resolutions: 3,
        // Update attempts, from the engine forge world fixture.
        updates: 3,
        // Outlast ordinary CI; report a silent forge within the same sitting (domain/forge.md, 8.4).
        stall: Duration::from_secs(30 * 60),
        // Gates retained by a change, from the engine forge world fixture.
        gates: 8,
        // Clean branch heads retained, from the engine forge world fixture.
        clean_heads: 8,
    },
    // Time to gather a landing queue, from the engine forge world fixture.
    queue_window: Duration::from_secs(30),
};
const CHECKOUT: checkout::Limits = checkout::Limits {
    // Cached or held checkout workspaces, from the checkout world fixture.
    workspaces: 3,
    // Held to the agent's two repository directories.
    repositories: 2,
    // Bytes in a checkout key, name, remote or branch: the 256-byte ceiling.
    name_bytes: 256,
    // Holds the agent's largest commit-message argument.
    message_bytes: 4096,
    // Conflicted paths per repository, from the checkout merges world.
    conflicts: 4,
    // Bytes per conflicted path: four NUL-terminated paths fit stdout_bytes 1,024.
    path_bytes: 255,
    // Time for an operation reaching a forge, from the checkout world fixture.
    remote_timeout: Duration::from_secs(60),
    // Time for a local checkout operation, from the checkout world fixture.
    local_timeout: Duration::from_secs(10),
    // Facts awaiting drain, from the checkout world fixture.
    facts: 256,
};
/// Root room follows the domain world; byte carriers take jig's small profile
/// bounds where its children require more than the world's reduced fixtures.
const ROOT: Root = Root {
    journal: engine::JournalLimits {
        // Commits awaiting durability, from the root world.
        commits: 3,
        // At least deliveries * 3: the root's worst case.
        held: 1536,
        // At least `route_bound` plus `call_records`: the root's worst case.
        writes: 12288,
        // At least jig small fleet.workers * fleet.max_out + 1: the root's worst case.
        deliveries: 512,
        // At least row_bound, including forge.client.answer_bytes: the root's worst case.
        transcript_bytes: 65_536,
        // At least brief.brief_bytes and jig small people.words: the root's worst case.
        result_bytes: 8192,
        // Run charter carrier in the root's worst case; jig small core.run_bytes.
        run_bytes: 16_384,
    },
    loads: engine::loads::Limits {
        // Concurrent loads, from the root world.
        loads: 2,
        // Rows in one page, from the root world.
        rows: 2,
        // Kept page bytes in loads' worst case; jig small core.load_bytes.
        bytes: 65_536,
        // At least loads.bytes: loads' worst case; jig small core.load_bytes.
        reply_bytes: 65_536,
        // One authenticated read's owned bytes; jig small core.load_bytes.
        read_bytes: 65_536,
        // Deep row admission bound for the root's largest forge.client.answer_bytes.
        transcript_bytes: 65_536,
    },
    brief: engine::BriefLimits {
        // Concurrent gathers, from the root world.
        briefs: 2,
        // Sections per gather, from the root world.
        sections: 3,
        // Parts per section, from the root world.
        parts: 2,
        // One section's read bytes: at least the task section's.
        read_bytes: 4608,
        budgets: engine::BriefBudgets {
            // Task section's bytes: a whole jig small spec (4096) with its contract and
            // requester lines, so an agent always reads all of what it was asked.
            task: 4608,
            // Dependencies section's bytes, from the root world.
            dependencies: 128,
            // CI section's bytes, from the root world.
            ci: 128,
            // Reviews section's bytes, from the root world.
            reviews: 128,
            // Pull section's bytes, from the root world.
            pull: 128,
            // Attempts section's bytes, from the root world.
            attempts: 128,
            // Plan section's bytes, from the root world.
            plan: 128,
            // Notes section's bytes, from the root world.
            notes: 64,
        },
        // Assembled brief bytes: the task section and room for the others.
        brief_bytes: 5376,
        // Outlast sequential forge reads and a retry; bound a stuck connector (domain/root.md, 10).
        gather: Duration::from_secs(30),
    },
};
const CLIENTS: Clients = Clients {
    // Files of a bundle, the page among them: far above the shell's dev bundle (decision 6 of browser-1).
    bundle_files: 128,
    // Bytes of a bundle in all, read into memory at start.
    bundle_bytes: 16 << 20,
};
/// The standalone process's socket admission.
pub const SOCKETS: u32 = 64;

/// Constant small-profile values in each owner's limits.
#[must_use]
pub fn profile(profile: Profile) -> Limits {
    match profile {
        Profile::Small => Limits {
            picker: Picker { page: 30 },
            routes: Routes { query_bytes: 2048 },
            setup: temper_forge_setup::limits::host(),
            records: Records { value_bytes: 4096 },
            forges: Forges { installations: FORGE_INSTALLATIONS, log_origins: 4 },
            forge_component: crate::forge::small(&FORGE),
            root: ROOT,
            forge: FORGE,
            checkout: CHECKOUT,
            git_component: GIT,
            clients: CLIENTS,
            sign_in: SignIn {
                members: 64,
                pending: 64,
                deadline: Duration::from_secs(600),
                fetches: 8,
                body_bytes: 65_536,
                url_bytes: 2048,
                secret_bytes: 512,
            },
        },
    }
}
/// Set one dotted field name byte for byte.
pub fn apply(limits: &mut Limits, name: &[u8], value: u64) -> Result<(), Refused> {
    names::apply(limits, name, value)
}
/// Build the root's limits from jig's children and temper's own room.
#[must_use]
pub fn engine(jig: &jig_protocol_config::Limits, limits: &Limits) -> Option<engine::Limits> {
    let root = engine::Limits {
        journal: limits.root.journal,
        loads: limits.root.loads,
        brief: limits.root.brief,
        tasks: jig.core.tasks,
        authority: jig.core.authority,
        people: jig.core.people,
        fleet: jig.core.fleet,
        accounts: jig.core.accounts,
        notes: jig.core.notes,
        documents: jig.core.documents,
        templates: jig.core.templates,
        settings: jig.core.settings,
        call_records: jig.core.call_records,
        forge: limits.forge,
    };
    engine::worst_case(&root)?;
    Some(root)
}

const GIT: temper_worker_protocol_git::Limits = temper_worker_protocol_git::Limits {
    // One operation per cached workspace, plus the component's startup and sweep.
    operations: 3,
    // At least every conflict path including its terminating NUL.
    stdout_bytes: 1024,
    // The tail retained for classification and the checkout's diagnostic.
    stderr_bytes: 512,
    // Each stdout/stderr demand, bounded further by io's read size.
    read_bytes: 4096,
    // One page of trash entries held while sweeping.
    sweep_entries: 16,
    // Every name in that page at the kernel's longest name.
    sweep_bytes: 4080,
};

/// Limits decoded by the setup and box owners, kept with their values.
#[must_use]
pub fn handed_on(name: &[u8]) -> bool {
    (name.starts_with(b"setup.") && name != b"setup.runs" && name != b"setup.states" && name != b"setup.state_life_ms")
        || name.starts_with(b"box.")
}
/// Setup bounds also consumed by the engine's host; the old shell keeps its four overrides.
#[must_use]
pub fn host_setup(name: &[u8]) -> bool {
    [
        b"setup.orgs".as_slice(),
        b"setup.deadline_ms",
        b"setup.backoff_ms",
        b"setup.backoff_max_ms",
        b"setup.runs",
        b"setup.states",
        b"setup.state_life_ms",
    ]
    .contains(&name)
}

/// Server bounds follow jig's shipped small profile, with its HTTP intake bounds.
#[must_use]
pub fn server(jig: &jig_protocol_config::Limits) -> crate::server::Limits {
    crate::server::Limits {
        connections: 8,
        http: skein_http::server::Limits {
            head: 4096,
            headers: 32,
            body: u64::from(jig.clients_http.body),
            read: jig.clients_http.read,
            response: 4096,
            send: jig.clients_http.send,
        },
        stall: Duration::from_secs(30),
    }
}
