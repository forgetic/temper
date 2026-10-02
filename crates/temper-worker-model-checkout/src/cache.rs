//! The cache: the workspaces on the worker's disk, keyed by workstream
//! (worker-model.md, 5), bounded by count.
//!
//! A workspace is a directory on the disk and the workstream it is for. It is
//! held by one hold at a time, or idle. A prepare finds its workstream's
//! workspace, if the cache has one that no hold holds; otherwise it takes a
//! new one while there is room, and then evicts the least recently used idle
//! workspace, which is made again for the new workstream. A workspace is never
//! removed from the cache once made: eviction gives it to another workstream,
//! so the cache holds at most `Limits::workspaces`, and a prepare that finds
//! them all held is refused.
//!
//! Nothing local is authoritative: a workspace is reused only for its own
//! workstream, and only when it holds exactly the repositories the spec names,
//! all cloned; a prepare then fetches and checks each out afresh. Otherwise
//! what it holds is not known (it is about to be removed, or a build failed or
//! was aborted), and the next prepare removes it and makes it again.

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{Id, List, Map, Slab};

use crate::boundary::{Refusal, Repository};
use crate::facts::Cached;

#[derive(Debug)]
pub(crate) struct Workspace {
    /// The workstream it is for, also its key in [`Cache::keys`].
    key: Box<[u8]>,
    disk: Disk,
    used: Use,
}

/// What a workspace's directory holds.
#[derive(Debug)]
enum Disk {
    /// The repositories named, each cloned, side by side.
    Holds { names: Box<[Box<[u8]>]> },
    /// Not known: it is removed before it is used.
    Unknown,
}

/// Whether a workspace is held, or idle since the release that `since`
/// counts, its place in the order of eviction.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Use {
    Held,
    Idle { since: u64 },
}

#[derive(Debug)]
pub(crate) struct Cache {
    workspaces: Slab<Workspace>,
    /// Every workspace, by its workstream.
    keys: Map<Box<[u8]>, Id<Workspace>>,
    /// The idle workspaces, least recently used first.
    idle: Map<u64, Id<Workspace>>,
    /// Releases so far, which order the idle workspaces.
    releases: u64,
}

impl Cache {
    pub(crate) fn with_capacity(workspaces: u32) -> Cache {
        Cache {
            workspaces: Slab::with_capacity(workspaces),
            keys: Map::with_capacity(workspaces),
            idle: Map::with_capacity(workspaces),
            releases: 0,
        }
    }

    /// Workspaces made so far.
    pub(crate) fn len(&self) -> u32 {
        self.workspaces.len()
    }

    /// Workspaces no hold holds.
    pub(crate) fn idle(&self) -> u32 {
        self.idle.len()
    }

    /// The workstream of the `nth` workspace, in the order of the keys.
    pub(crate) fn key(&self, nth: u32) -> Option<&[u8]> {
        let (key, _) = self.keys.iter().nth(usize::try_from(nth).ok()?)?;
        Some(key)
    }

    /// Holds a workspace for the workstream `key`, whose spec names
    /// `repositories`: its own if it has one, a new one, or the least
    /// recently used idle one, evicted. Refused if its own is held, or if
    /// every workspace is.
    pub(crate) fn hold(
        &mut self,
        key: Box<[u8]>,
        repositories: &[Repository],
    ) -> Result<(Id<Workspace>, Cached), Refusal> {
        if let Some(found) = self.keys.get(&*key) {
            let id = *found;
            let workspace = self.workspaces.get_mut(id).expect("a key names a workspace of the cache");
            let since = match workspace.used {
                Use::Held => return Err(Refusal::Busy),
                Use::Idle { since } => since,
            };
            self.idle.remove(&since);
            workspace.used = Use::Held;
            if holds_exactly(&workspace.disk, repositories) {
                return Ok((id, Cached::Reused));
            }
            workspace.disk = Disk::Unknown;
            return Ok((id, Cached::Rebuilt));
        }
        if !self.workspaces.is_full() {
            let workspace = Workspace { key: copy_of(&key), disk: Disk::Unknown, used: Use::Held };
            let id = self.workspaces.insert(workspace).expect("checked for room above");
            let made = self.keys.insert(key, id).expect("a key for each workspace");
            assert!(made.is_none(), "a workstream has one workspace");
            return Ok((id, Cached::New));
        }
        let Some((since, evicted)) = self.idle.first() else {
            return Err(Refusal::Full);
        };
        let (since, id) = (*since, *evicted);
        self.idle.remove(&since);
        let workspace = self.workspaces.get_mut(id).expect("an idle workspace is of the cache");
        let gone = self.keys.remove(&*workspace.key);
        assert!(gone == Some(id), "a workspace is keyed by its workstream");
        *workspace = Workspace { key: copy_of(&key), disk: Disk::Unknown, used: Use::Held };
        let made = self.keys.insert(key, id).expect("room for the key of the workspace evicted");
        assert!(made.is_none(), "a workstream has one workspace");
        Ok((id, Cached::Evicted))
    }

    /// The workspace `id` holds `repositories`, each cloned.
    pub(crate) fn cloned(&mut self, id: Id<Workspace>, repositories: &[Repository]) {
        let workspace = self.workspaces.get_mut(id).expect("a hold's workspace is of the cache");
        let mut names = List::with_capacity(count(repositories.len()));
        for repository in repositories {
            names.push(copy_of(&repository.name)).expect("room for every name");
        }
        workspace.disk = Disk::Holds { names: names.into_boxed() };
    }

    /// The workspace `id` is idle again, the most recently used.
    pub(crate) fn release(&mut self, id: Id<Workspace>) {
        let workspace = self.workspaces.get_mut(id).expect("a hold's workspace is of the cache");
        assert!(workspace.used == Use::Held, "a workspace is released once for each hold");
        self.releases = self.releases.checked_add(1).expect("fewer releases than a u64 counts");
        workspace.used = Use::Idle { since: self.releases };
        self.idle.insert(self.releases, id).expect("room for every workspace to be idle");
    }
}

/// Whether `disk` holds exactly the repositories named, which are named once
/// each.
fn holds_exactly(disk: &Disk, repositories: &[Repository]) -> bool {
    let names = match disk {
        Disk::Holds { names } => names,
        Disk::Unknown => return false,
    };
    if names.len() != repositories.len() {
        return false;
    }
    for repository in repositories {
        if !names.contains(&repository.name) {
            return false;
        }
    }
    true
}

pub(crate) fn count(items: usize) -> u32 {
    u32::try_from(items).expect("validated against a u32 limit")
}
