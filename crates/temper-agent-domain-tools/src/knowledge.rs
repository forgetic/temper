//! What a kit's LLM knows of the checkout: each file it has read, at the
//! version it read, or wrote last. A change needs the current version read,
//! and io compares the version with the real file just before it stores, so a
//! file changed since by anyone is caught.
//!
//! It is bounded: past `Limits::known_files`, the file read longest ago is
//! forgotten, and must be read again before it is changed.

use temper_lib::Map;

use crate::boundary::Version;
use crate::path::Place;

#[derive(Debug)]
pub(crate) struct Knowledge {
    seen: Map<Place, Seen>,
    /// Ticks once per record, to tell which was read longest ago.
    clock: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Seen {
    version: Version,
    /// The clock when it was recorded.
    at: u64,
}

impl Knowledge {
    /// Room for `capacity` files.
    pub(crate) const fn new(capacity: u32) -> Knowledge {
        Knowledge { seen: Map::with_capacity(capacity), clock: 0 }
    }

    /// The version of the file at `place` the LLM knows, if it knows one.
    pub(crate) fn version(&self, place: &Place) -> Option<Version> {
        let seen = self.seen.get(place)?;
        Some(seen.version)
    }

    /// The LLM now knows the file at `place` at `version`, having read it or
    /// written it, forgetting the file read longest ago if there is no room.
    pub(crate) fn record(&mut self, place: Place, version: Version) {
        let seen = Seen { version, at: self.clock };
        self.clock = self.clock.saturating_add(1);
        if let Some(known) = self.seen.get_mut(&place) {
            *known = seen;
            return;
        }
        // A kit that may know no file remembers nothing.
        if self.seen.capacity() == 0 {
            return;
        }
        if self.seen.len() >= self.seen.capacity() {
            self.forget_oldest();
        }
        let fresh = self.seen.insert(place, seen).expect("room was made above");
        assert!(fresh.is_none(), "the place was not known");
    }

    /// The LLM knows there is no file at `place`.
    pub(crate) fn forget(&mut self, place: &Place) {
        let _: Option<Seen> = self.seen.remove(place);
    }

    fn forget_oldest(&mut self) {
        let mut oldest: Option<(&Place, u64)> = None;
        for (place, seen) in &self.seen {
            let older = match oldest {
                Some((_, at)) => seen.at < at,
                None => true,
            };
            if older {
                oldest = Some((place, seen.at));
            }
        }
        if let Some((place, _)) = oldest {
            let place = place.clone();
            self.forget(&place);
        }
    }
}
