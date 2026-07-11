// SPDX-License-Identifier: MPL-2.0

//! `temper plan` — a read-only preview over the canonical deployment bundle.

mod args;
mod inspection;
mod report;

pub use args::{PLAN_USAGE, PlanOptions, plan_main_with_options};
pub use report::DeploymentPlanReport;

use crate::deployment::load_deployment;
use inspection::ForgePlanInspector;
use report::build_report;

/// Builds a deployment plan report using the production read-only Forge adapter.
pub fn run_plan(opts: &PlanOptions) -> Result<DeploymentPlanReport, String> {
    let bundle = load_deployment(&opts.options, &opts.env, &opts.paths, opts.existing_repo)
        .map_err(|error| error.to_string())?;
    let mut inspector = ForgePlanInspector::from_bundle(&bundle);
    build_report(&bundle, &mut inspector)
}

#[cfg(test)]
mod tests;
