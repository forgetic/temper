//! The cache: the workspaces on the worker's disk, keyed by workstream
//! (worker-domain.md, 5), bounded by count.
//!
//! A workspace is a directory on the disk and the workstream it is for. It is
//! held by one hold at a time, or idle. A prepare finds its workstream's
//! workspace, if the cache has one that no hold holds; otherwise it takes a
//! new one while there is room, and then evicts the least recently used idle
//! workspace, which is made again, empty, for the new workstream. A workspace is never
//! removed from the cache once made: eviction gives it to another workstream,
//! so the cache holds at most `Limits::workspaces`, and a prepare that finds
//! them all held is refused.
//!
//! Nothing local is authoritative: a workspace is reused only for its own
//! workstream, and only when it holds exactly the repositories the spec names
//! (each by its directory and its remote), all cloned; a prepare then fetches
//! and checks each out afresh. Otherwise what it holds is not known (it is
//! being built, or an operation on it broke, ran out of time or was cancelled,
//! so git may have left it damaged), and the next prepare makes it again,
//! empty. A failure that leaves the disk as it was (the forge unreachable, or
//! without what was asked, or refusing) keeps what it holds.

use alloc::boxed::Box;

use skein_lib::bytes::copy_of;
use skein_lib::{Id, List, Map, Slab};

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
    /// These repositories, each cloned, side by side.
    Holds { repositories: Box<[Cloned]> },
    /// Not known: it is made again, empty, before it is used.
    Unknown,
}

/// A repository cloned in a workspace: its directory, and where it was cloned
/// from.
#[derive(Debug)]
pub(crate) struct Cloned {
    name: Box<[u8]>,
    remote: Box<[u8]>,
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

    /// The workstream of the `nth` workspace whose disk is known, in the order
    /// of the keys.
    pub(crate) fn key(&self, nth: u32) -> Option<&[u8]> {
        let mut known = 0_u32;
        for (key, id) in &self.keys {
            let workspace = self.workspaces.get(*id).expect("a key names a workspace of the cache");
            match workspace.disk {
                Disk::Holds { .. } => {}
                Disk::Unknown => continue,
            }
            if known == nth {
                return Some(key);
            }
            known = known.checked_add(1).expect("fewer workspaces than a u32 counts");
        }
        None
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
        let since = *since;
        let id = *evicted;
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
        let mut cloned = List::with_capacity(count(repositories.len()));
        for repository in repositories {
            let clone = Cloned { name: copy_of(&repository.name), remote: copy_of(&repository.remote) };
            cloned.push(clone).expect("room for every repository");
        }
        workspace.disk = Disk::Holds { repositories: cloned.into_boxed() };
    }

    /// What the workspace `id` holds is no longer known: an operation on it
    /// broke, ran out of time or was cancelled.
    pub(crate) fn spoil(&mut self, id: Id<Workspace>) {
        let workspace = self.workspaces.get_mut(id).expect("a hold's workspace is of the cache");
        workspace.disk = Disk::Unknown;
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

/// Whether `disk` holds exactly the repositories named, each in its directory
/// and cloned from its remote. A spec names a directory once.
fn holds_exactly(disk: &Disk, repositories: &[Repository]) -> bool {
    let cloned = match disk {
        Disk::Holds { repositories } => repositories,
        Disk::Unknown => return false,
    };
    if cloned.len() != repositories.len() {
        return false;
    }
    for repository in repositories {
        if !holds(cloned, repository) {
            return false;
        }
    }
    true
}

fn holds(cloned: &[Cloned], repository: &Repository) -> bool {
    for clone in cloned {
        if clone.name == repository.name {
            return clone.remote == repository.remote;
        }
    }
    false
}

pub(crate) fn count(items: usize) -> u32 {
    u32::try_from(items).expect("validated against a u32 limit")
}
