//! The deadline table each layer keeps for its own timers (section 9).

use alloc::collections::{BTreeMap, BTreeSet};
use core::mem::{align_of, size_of};

use crate::Time;

/// Timers, each named by a key of the owning layer's choosing, with at most
/// one timer per key.
///
/// Arm and cancel are synchronous. A stage fires its expired timers after its
/// input events, one `expire` at a time; firing removes the timer before its
/// handler runs, so a timer ends exactly once, fired or cancelled, and
/// cancelling one that already fired is a stale name, ignored.
///
/// Timers that fall due at the same time fire in key order.
///
/// The capacity bounds the table, but unlike a slab or a queue it is not
/// allocated up front: the two B-trees allocate nodes as they fill (6.1), and
/// [`Deadlines::worst_case`] counts those nodes.
#[derive(Debug)]
pub struct Deadlines<K> {
    by_time: BTreeSet<(Time, K)>,
    by_key: BTreeMap<K, Time>,
    capacity: u32,
}

impl<K: Ord + Copy> Deadlines<K> {
    #[must_use]
    pub fn with_capacity(capacity: u32) -> Deadlines<K> {
        Deadlines { by_time: BTreeSet::new(), by_key: BTreeMap::new(), capacity }
    }

    /// The most heap a table of `capacity` timers takes, or `None` past a
    /// `u64`.
    #[must_use]
    pub fn worst_case(capacity: u32) -> Option<u64> {
        // A set's values take no room.
        let by_time = btree(capacity, size_of::<(Time, K)>(), 0, align_of::<(Time, K)>())?;
        let by_key = btree(capacity, size_of::<K>(), size_of::<Time>(), align_of::<(K, Time)>())?;
        by_time.checked_add(by_key)
    }

    #[must_use]
    pub fn len(&self) -> u32 {
        u32::try_from(self.by_key.len()).expect("no more timers than the capacity")
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// Arms the timer `key` to fall due at `at`, moving it if it is already
    /// armed. Hands the key back when the table is full.
    pub fn arm(&mut self, key: K, at: Time) -> Result<(), K> {
        match self.by_key.get(&key) {
            Some(&old) => {
                let armed = self.by_time.remove(&(old, key));
                assert!(armed, "both indexes hold every timer");
            }
            None => {
                if self.len() >= self.capacity {
                    return Err(key);
                }
            }
        }
        let previous = self.by_key.insert(key, at);
        let fresh = self.by_time.insert((at, key));
        assert!(fresh, "both indexes hold every timer");
        let _: Option<Time> = previous;
        Ok(())
    }

    /// Cancels the timer `key`, if it is armed.
    pub fn cancel(&mut self, key: K) {
        if let Some(at) = self.by_key.remove(&key) {
            let armed = self.by_time.remove(&(at, key));
            assert!(armed, "both indexes hold every timer");
        }
    }

    /// When the earliest timer falls due.
    #[must_use]
    pub fn next(&self) -> Option<Time> {
        let (at, _) = self.by_time.first()?;
        Some(*at)
    }

    /// Removes the earliest timer due at `now` and returns its key.
    pub fn expire(&mut self, now: Time) -> Option<K> {
        let &(at, key) = self.by_time.first()?;
        if at > now {
            return None;
        }
        let popped = self.by_time.pop_first();
        assert!(popped == Some((at, key)), "the first timer is the one popped");
        let armed = self.by_key.remove(&key);
        assert!(armed == Some(at), "both indexes hold every timer");
        Some(key)
    }
}

/// The standard library's B-tree nodes (`B = 6` in `alloc::collections::btree`)
/// hold at most 11 entries, and every node but the root at least 5. Nothing
/// checks these against the library but a test with a counting allocator.
const NODE_CAPACITY: usize = 11;
const NODE_MIN: u64 = 5;

/// The most heap a B-tree of `entries` keys and values of `key` and `value`
/// bytes takes: a node for every `NODE_MIN` entries and one for the root, each
/// priced as an internal node, the larger kind.
fn btree(entries: u32, key: usize, value: usize, align: usize) -> Option<u64> {
    let word = size_of::<usize>();
    let align = align.max(word);
    // A leaf node: its parent link, its index in the parent and its length,
    // then its keys and values. Each part is rounded up to the alignment, so
    // the bound holds whatever order the fields are laid out in.
    let header = word.checked_add(size_of::<u16>().checked_mul(2)?)?;
    let leaf = round_up(header, align)?
        .checked_add(round_up(key.checked_mul(NODE_CAPACITY)?, align)?)?
        .checked_add(round_up(value.checked_mul(NODE_CAPACITY)?, align)?)?;
    // An internal node: a leaf and an edge either side of each entry.
    let edges = word.checked_mul(NODE_CAPACITY.checked_add(1)?)?;
    let internal = u64::try_from(leaf.checked_add(round_up(edges, align)?)?).ok()?;
    let nodes = (u64::from(entries) / NODE_MIN).checked_add(1)?;
    nodes.checked_mul(internal)
}

fn round_up(bytes: usize, align: usize) -> Option<usize> {
    bytes.checked_next_multiple_of(align)
}

#[cfg(test)]
mod tests {
    use super::Deadlines;
    use crate::Time;

    fn at(nanos: u64) -> Time {
        Time::from_nanos(nanos)
    }

    #[test]
    fn timers_fire_in_order_once_due() {
        let mut timers = Deadlines::with_capacity(3);
        timers.arm('b', at(20)).expect("room");
        timers.arm('a', at(10)).expect("room");
        timers.arm('c', at(10)).expect("room");
        assert_eq!(timers.next(), Some(at(10)));
        assert_eq!(timers.expire(at(9)), None);
        assert_eq!(timers.expire(at(15)), Some('a'));
        assert_eq!(timers.expire(at(15)), Some('c'));
        assert_eq!(timers.expire(at(15)), None);
        assert_eq!(timers.expire(at(20)), Some('b'));
        assert!(timers.is_empty());
    }

    #[test]
    fn rearming_moves_a_timer_and_cancelling_is_idempotent() {
        let mut timers = Deadlines::with_capacity(1);
        timers.arm('a', at(10)).expect("room");
        timers.arm('a', at(30)).expect("re-arming takes no new room");
        assert_eq!(timers.arm('b', at(5)), Err('b'));
        assert_eq!(timers.expire(at(10)), None);
        timers.cancel('a');
        timers.cancel('a');
        assert_eq!(timers.next(), None);
    }
}
