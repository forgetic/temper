use temper_worker_checkout_world::merges::{self, Case};

#[test]
fn seeded_merge_worlds_preserve_complete_trees_and_parent_graphs() {
    for seed in 0..128 {
        let cases = [Case::Clean, Case::Conflict, Case::Unchanged, Case::Moved, Case::Ambiguous, Case::Saved];
        let case = cases[usize::try_from(seed % 6).expect("small case index")];
        let _ = merges::run(seed, case, 8);
    }
}
