The report consumer currently repeats the decision field without checking the
evidence kind. Run the opt-in benchmark fixture and deliver its report as
`graph-artifact-report.json`. Fix `artifact_recommendation` so synthetic reports
always defer, even when their decision field claims a pilot. Native reports may
return `bounded-ci-pilot-candidate` only with complete main/failure accounting,
passing correctness, and default persistence disabled. Keep the harness and
validation fixtures unchanged. Run the real product tests before submitting.
