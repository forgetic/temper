use temper_engine_tasks_world::run_story;
#[test]
fn random_faults_and_restart_cuts_preserve_lifecycle_and_reach_every_initial_ending() {
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..256 {
        let (trace, results) = run_story(seed);
        assert_eq!(results, 5);
        for ending in ["status: Done", "status: Failed", "status: Cancelled", "Held", "Release", "Refused", "Restore"] {
            if trace.iter().any(|line| line.contains(ending)) {
                seen.insert(ending);
            }
        }
    }
    // Results are independent referee observations; hold/release, refusal and
    // restore markers come from the store and actual input boundaries.
    for ending in ["status: Done", "status: Failed", "status: Cancelled", "Held", "Release", "Refused", "Restore"] {
        assert!(seen.contains(ending), "sweep reaches {ending}");
    }
}
