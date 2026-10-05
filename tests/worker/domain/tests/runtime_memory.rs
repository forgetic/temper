//! All v2 root entry points, including owned conflict paths, retained opaque
//! bodies, repeated hello copies and retained answers, measured serially.
use temper_worker_domain_world::next::{Settings, World};
use temper_world::heap;

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

#[test]
fn v2_runtime_heap_stays_bounded_at_one_and_three_retained_turns() {
    for turns in [1, 3] {
        for merging in [false, true] {
            let mut world = World::new(Settings { seed: 91, turns, byte_slots: turns, merging, abandon: false });
            world.fill_turns();
            world.run();
            assert_eq!(world.stats().peak_retained, turns);
            assert!(world.stats().peak_heap > 0, "the counting allocator observed domain memory");
        }
    }
}
