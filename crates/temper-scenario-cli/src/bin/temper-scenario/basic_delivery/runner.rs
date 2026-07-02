// SPDX-License-Identifier: MPL-2.0

use std::path::Path;
use std::sync::Arc;

use temper_forge_memory::MemoryForge;
use temper_forge_model::CreateIssue;
use temper_runner::{InProcessStage, Scenario, Stage, run_scenario_with_budget};

use super::SCENARIO_NAME;
use super::evidence::read_evidence;
use super::fixture::{Fixture, load_fixture};
use super::model::RunOutcome;

const BUDGET: u64 = 64;

pub(super) async fn run_basic_delivery(
    scenario_path: &Path,
    manifest_path: &Path,
) -> Result<RunOutcome, String> {
    let fixture = load_fixture(scenario_path, manifest_path)?;
    let workflow = temper_testing::resolve_workflow(Some(&fixture.workflow_path))
        .map_err(|error| error.to_string())?;
    let config = temper_testing::runner_config_for_workflow(&workflow);
    let forge = MemoryForge::new();
    let stage = InProcessStage::with_identity(
        forge,
        workflow,
        config,
        temper_testing::agents::basic_fake_registry(),
        |forge, binding| forge.as_user(binding.user.clone()),
    )
    .await
    .map_err(|error| error.to_string())?
    .with_extra_worker_factory(temper_testing::world::memory_ci_worker);

    let scenario = scenario(fixture.clone());
    let report = run_scenario_with_budget(&stage, &scenario, BUDGET)
        .await
        .map_err(|error| error.to_string())?;
    let evidence = read_evidence(stage.forge(), stage.repo(), &fixture.intake, &fixture.repo)
        .await
        .map_err(|error| error.to_string())?;

    Ok(RunOutcome {
        scenario_name: SCENARIO_NAME.to_string(),
        evidence,
        report,
    })
}

fn scenario(fixture: Fixture) -> Scenario {
    let fixture = Arc::new(fixture);
    let fixture_for_seed = Arc::clone(&fixture);
    let fixture_for_assert = Arc::clone(&fixture);
    Scenario::new(
        SCENARIO_NAME,
        Box::new(move |forge, repo| {
            let fixture = Arc::clone(&fixture_for_seed);
            Box::pin(async move {
                forge
                    .create_issue(
                        repo,
                        CreateIssue {
                            title: fixture.intake.title.clone(),
                            body: fixture.intake.body.clone(),
                            labels: fixture.intake.labels.clone(),
                            assignees: Vec::new(),
                        },
                    )
                    .await?;
                Ok(())
            })
        }),
        Box::new(move |forge, repo| {
            let fixture = Arc::clone(&fixture_for_assert);
            Box::pin(async move {
                read_evidence(forge, repo, &fixture.intake, &fixture.repo).await?;
                Ok(())
            })
        }),
    )
}
