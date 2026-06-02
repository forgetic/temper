//! Phase B1 proof: a real LLM-backed engineer drives its workflow action on a
//! real Forgejo, mutating state only through `RoleTools`.
//!
//! This boots a throwaway Forgejo server + host-mode runner, provisions the org /
//! repo / per-role users + tokens / labels / CI workflow (the same Temper Phase
//! 4 uses), seeds the **happy-path** scenario (one untriaged issue), then runs an
//! in-process two-worker world:
//!
//! - a **fake** architect (triages `untriaged` → `code` + `ready`), and
//! - the quarantined reference-delivery **real** [`LlmEngineer`] test fixture
//!   from `temper-testing` (decides via the selected LLM auth mode; acts through
//!   [`RoleTools`]).
//!
//! Production workers use compiled role manifests. This e2e intentionally keeps
//! the fixed legacy engineer under `temper-testing` so production
//! `temper-agents` does not ship hard-coded workflow-role prompts.
//!
//! It ticks until the engineer has opened the implementation PR (parented to the
//! triaged code issue) through the idempotent `open_pull_request` seam — the
//! engineer's load-bearing real action. A single converged step is enough for
//! B1; later phases drive the full merge.
//!
//! Doubly gated: `#[ignore]`d **and** requires **both** `TEMPER_FORGEJO_E2E=1`
//! (boots a server/runner, as the rest of the Forgejo e2e suite does) **and**
//! `TEMPER_FORGEJO_AGENTS=1` (real LLM calls — non-deterministic). The default
//! `cargo test` runs neither. Run with:
//!
//! ```sh
//! TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 \
//!   cargo test -p temper-agents --test forgejo_engineer_e2e -- --ignored
//! ```
//!
//! The driver is a sync `#[test]` (matching `tests/forgejo_multiprocess.rs`): the
//! server fixture polls readiness with a *blocking* reqwest client whose nested
//! runtime must live off any async reactor, and async backend work is driven on a
//! one-shot current-thread Tokio runtime via [`block_on`].
//!
//! Per the cost policy this defaults to **ChatGPT OAuth** (the flat
//! subscription), reading the shared `~/.pi/agent/auth.json` — **no DeepSeek
//! tokens are spent**. Opt into Anthropic OAuth with
//! `TEMPER_AGENTS_AUTH=anthropic-oauth` (after `pi /login anthropic`) or opt back
//! to DeepSeek with `TEMPER_AGENTS_AUTH=deepseek` (then the key is read from
//! `.cache/deepseek-api-key` or `TEMPER_DEEPSEEK_API_KEY*`). Credentials are
//! never logged or committed.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use temper_agents::{AuthChoice, ProviderConfig};
use temper_forge::{CreateIssue, CreatePullRequest, Forge, PullRequestQuery, RepositoryId};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_runner::{Agent, AgentError, RoleTools, RoleWorker, Worker};
use temper_testing::agents::FakeArchitect;
use temper_testing::forgejo_server::{
    ForgejoRunner, ForgejoServer, Provisioned, commit_ci_sentinel, prepare_pull_request_head,
    provision,
};
use temper_testing::legacy_llm::{EngineerPrep, LlmEngineer};
use temper_testing::{runner_config, workflow};
use temper_workflow::{RoleId, parse_metadata_block};

/// Time budget for the architect + engineer to converge on an opened PR. Real
/// LLM latency dominates; provisioning + git + HTTP add the rest.
const CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(180);
/// Tick cadence for the in-process workers.
const TICK_INTERVAL: Duration = Duration::from_secs(2);

/// Returns whether both opt-ins are present; prints a skip note when not.
fn enabled() -> bool {
    let e2e = std::env::var("TEMPER_FORGEJO_E2E").ok().as_deref() == Some("1");
    let agents = std::env::var("TEMPER_FORGEJO_AGENTS").ok().as_deref() == Some("1");
    if e2e && agents {
        return true;
    }
    eprintln!(
        "skipping Forgejo LLM-engineer e2e: set BOTH TEMPER_FORGEJO_E2E=1 and \
         TEMPER_FORGEJO_AGENTS=1 (boots a real Forgejo + runner and makes real, \
         non-deterministic LLM calls)"
    );
    false
}

/// The agent auth mode: ChatGPT OAuth by default (the cost policy — a flat
/// subscription, not pay-per-token DeepSeek), overridable to Anthropic OAuth with
/// `TEMPER_AGENTS_AUTH=anthropic-oauth` or DeepSeek with
/// `TEMPER_AGENTS_AUTH=deepseek`.
fn agents_auth_choice() -> AuthChoice {
    match std::env::var("TEMPER_AGENTS_AUTH").ok().as_deref() {
        Some("deepseek") => AuthChoice::DeepSeek,
        Some("anthropic-oauth") => AuthChoice::AnthropicOAuth,
        _ => AuthChoice::ChatGptOAuth,
    }
}

#[test]
#[ignore = "boots a real Forgejo + runner and makes real LLM calls; \
            run with TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 -- --ignored"]
fn llm_engineer_opens_pr_through_role_tools() {
    if !enabled() {
        return;
    }

    // Fail fast with a clear message if the credential is missing, before booting
    // a server. The error never includes secret bytes. Defaults to ChatGPT OAuth
    // (cost policy); TEMPER_AGENTS_AUTH selects Anthropic OAuth or DeepSeek.
    let auth = agents_auth_choice();
    let provider = ProviderConfig::from_auth(auth, None, None).expect(
        "LLM provider config builds (ChatGPT OAuth: run `pi /login openai-codex`; \
         Anthropic OAuth: run `pi /login anthropic`; DeepSeek: place .cache/deepseek-api-key)",
    );

    let server = ForgejoServer::start().expect("forgejo server boots");
    let mut runner = ForgejoRunner::register(&server).expect("forgejo runner registers");
    assert!(runner.is_running(), "runner daemon exited immediately");

    let provisioned = block_on(provision(&server)).expect("provisioning succeeds");

    // Seed the happy-path scenario (one untriaged issue) via an admin handle.
    let admin = admin_forge(&server, &provisioned);
    block_on(seed_untriaged_issue(&admin, &provisioned.repository));

    // Build the engineer's own handle (its token => its `current_user`) plus the
    // backend prep hook the LLM-agnostic engineer needs on real Forgejo.
    let engineer_identity = provisioned
        .role(&RoleId::new("engineer"))
        .expect("engineer role is provisioned");
    let engineer_config = ForgejoConfig::new(server.base_url(), &engineer_identity.token)
        .with_default_repo(&provisioned.owner, &provisioned.name)
        .with_web_ui_credentials(&engineer_identity.user, &engineer_identity.password);
    let engineer_forge = ForgejoForge::new(engineer_config);

    let prep: Arc<dyn EngineerPrep<dyn Forge>> = Arc::new(ForgejoEnginePrep {
        base_url: server.base_url().to_string(),
        token: engineer_identity.token.clone(),
        owner: provisioned.owner.clone(),
        name: provisioned.name.clone(),
    });
    let llm_engineer = LlmEngineer::with_prep(provider, prep);

    // Wire two in-process role workers over the compiled workflow.
    let validated = workflow();
    let compiled = validated.compile();
    let config = runner_config();

    let architect_role = RoleId::new("architect");
    let engineer_role = RoleId::new("engineer");

    let architect = RoleWorker::new(
        &validated,
        &compiled,
        &admin as &dyn Forge,
        &provisioned.repository,
        architect_role.clone(),
        Arc::new(FakeArchitect) as Arc<dyn Agent<dyn Forge>>,
        config.execution_context(&architect_role),
    );
    let engineer = RoleWorker::new(
        &validated,
        &compiled,
        &engineer_forge as &dyn Forge,
        &provisioned.repository,
        engineer_role.clone(),
        Arc::new(llm_engineer) as Arc<dyn Agent<dyn Forge>>,
        config.execution_context(&engineer_role),
    );

    // Tick architect then engineer until an implementation PR parented to the
    // triaged code issue exists, or we time out.
    let deadline = Instant::now() + CONVERGENCE_TIMEOUT;
    let mut converged = None;
    while Instant::now() < deadline {
        tick(&architect);
        tick(&engineer);

        if let Some((pr, parent)) = block_on(implementation_pr(&admin, &provisioned.repository)) {
            converged = Some((pr, parent));
            break;
        }
        std::thread::sleep(TICK_INTERVAL);
    }

    let outcome = converged;
    drop(runner);
    drop(server);

    match outcome {
        Some((pr, parent)) => {
            eprintln!(
                "engineer opened PR #{pr} parented to code issue #{parent} through RoleTools"
            );
        }
        None => panic!(
            "the LLM engineer did not open an implementation PR within {CONVERGENCE_TIMEOUT:?}"
        ),
    }
}

/// Drives a worker once, tolerating transient backend errors (real Forgejo under
/// load occasionally returns 5xx); a tick error just means "no progress".
fn tick<W: Worker>(worker: &W) {
    if let Err(error) = block_on(worker.tick(chrono::Utc::now())) {
        eprintln!(
            "worker '{}' tick failed (will retry): {error}",
            worker.name()
        );
    }
}

/// Builds an admin-owner handle (admin token, default repo, role web-UI creds for
/// any CI reads) used to seed, assert, and drive the fake architect.
fn admin_forge(server: &ForgejoServer, provisioned: &Provisioned) -> ForgejoForge {
    let config = ForgejoConfig::new(server.base_url(), &provisioned.admin_token)
        .with_default_repo(&provisioned.owner, &provisioned.name);
    let config = match provisioned.role(&RoleId::new("engineer")) {
        Some(role) => config.with_web_ui_credentials(&role.user, &role.password),
        None => config,
    };
    ForgejoForge::new(config)
}

/// Seeds the happy-path scenario's single untriaged issue.
async fn seed_untriaged_issue(forge: &dyn Forge, repo: &RepositoryId) {
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: "Ship the end-to-end happy path".into(),
                body: "A human asks the team to implement one small change.".into(),
                labels: vec!["untriaged".into()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("seeding the untriaged issue succeeds");
}

/// Returns `(pr_number, parent_code_issue_number)` for the implementation PR, if
/// one exists with a workflow-metadata parent. This is the proof the engineer
/// performed its real action through the `open_pull_request` seam.
async fn implementation_pr(forge: &dyn Forge, repo: &RepositoryId) -> Option<(u64, u64)> {
    let prs = forge
        .list_pull_requests(repo, PullRequestQuery::default())
        .await
        .ok()?;
    for pr in prs {
        if let Ok(Some(metadata)) = parse_metadata_block(&pr.body) {
            if let Some(parent) = metadata.parents.first() {
                return Some((pr.number.get(), parent.number.get()));
            }
        }
    }
    None
}

/// Drives a future to completion on a one-shot current-thread Tokio runtime. The
/// real backend's futures park on network IO, so a no-op block_on cannot drive
/// them; matches `tests/forgejo_multiprocess.rs`.
fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime builds")
        .block_on(future)
}

/// The Forgejo-specific [`EngineerPrep`] for the test: it makes the PR head a real
/// branch with a differing commit and seeds the `ci-ok` sentinel so the PR is
/// openable and passes CI — exactly the side effects Temper's
/// `ForgejoEngineer` performs, but driven by the LLM engineer's decision.
struct ForgejoEnginePrep {
    base_url: String,
    token: String,
    owner: String,
    name: String,
}

#[async_trait]
impl<F: Forge + ?Sized> EngineerPrep<F> for ForgejoEnginePrep {
    async fn before_open_pr(
        &self,
        _tools: &RoleTools<'_, F>,
        input: &CreatePullRequest,
    ) -> Result<(), AgentError> {
        prepare_pull_request_head(&self.base_url, &self.token, &self.owner, &self.name, input)
            .await
            .map_err(|error| AgentError::message(format!("forgejo PR prep failed: {error}")))?;
        commit_ci_sentinel(
            &self.base_url,
            &self.token,
            &self.owner,
            &self.name,
            input.source.branch.as_str(),
        )
        .await
        .map_err(|error| AgentError::message(format!("forgejo CI sentinel seed failed: {error}")))
    }
}
