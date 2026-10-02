use std::collections::BTreeMap;
use std::collections::btree_map;
use std::fmt::Debug;

/// Requests in flight, each ended once (programming-model.md, 11: one
/// terminal event per request, every `ReplyTo` answered): opened under a key
/// of its own, with what the world keeps of it, and ended by its terminal, its
/// reply or its withdrawal.
#[derive(Debug)]
pub struct Ledger<K, V> {
    /// What the requests are, for the contracts' messages.
    what: &'static str,
    open: BTreeMap<K, V>,
}

impl<K: Ord + Copy + Debug, V> Ledger<K, V> {
    #[must_use]
    pub fn new(what: &'static str) -> Ledger<K, V> {
        Ledger { what, open: BTreeMap::new() }
    }

    /// Opens the request `key`, which no other in flight has.
    pub fn open(&mut self, key: K, value: V) {
        let what = self.what;
        assert!(self.open.insert(key, value).is_none(), "each {what} in flight has a key of its own: {key:?}");
    }

    /// Ends the request `key`, which is in flight: its one end.
    pub fn end(&mut self, key: K) -> V {
        let what = self.what;
        self.open.remove(&key).unwrap_or_else(|| panic!("a {what} ends once, while in flight: {key:?}"))
    }

    /// Ends the request `key` if it is still in flight: for an end that may
    /// have lost a race with another.
    pub fn take(&mut self, key: K) -> Option<V> {
        self.open.remove(&key)
    }

    #[must_use]
    pub fn get(&self, key: K) -> Option<&V> {
        self.open.get(&key)
    }

    pub fn get_mut(&mut self, key: K) -> Option<&mut V> {
        self.open.get_mut(&key)
    }

    #[must_use]
    pub fn contains(&self, key: K) -> bool {
        self.open.contains_key(&key)
    }

    /// What the world keeps of the requests in flight, by key.
    pub fn values(&self) -> btree_map::Values<'_, K, V> {
        self.open.values()
    }

    /// The keys of the requests in flight, in order.
    pub fn keys(&self) -> btree_map::Keys<'_, K, V> {
        self.open.keys()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    /// Checks that every request has ended, once the world has settled.
    pub fn assert_settled(&self) {
        let what = self.what;
        let open: Vec<&K> = self.open.keys().collect();
        assert!(open.is_empty(), "every {what} has ended: {open:?} have not");
    }
}
