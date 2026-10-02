//! What a kit may do and where: the checkout's repositories, which of them
//! may be written, and the families of tools granted. The run decides it from
//! its charter, and it is copied into the session, and from there into its
//! kit, when the session opens.

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{List, Token};

use crate::call::{Call, Outcome};
use crate::limits::Limits;
use crate::path::{self, Name, Path, Place};

/// What a kit is given when it opens.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Authority {
    /// Where relative paths start: an absolute path, as names.
    pub cwd: Box<[Name]>,
    /// The repositories of the checkout. Paths outside them all are refused.
    pub repos: Box<[Repo]>,
    pub grants: Grants,
}

/// A repository of the checkout.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Repo {
    /// Where it is: the absolute path of its root, as names. A repository
    /// mounted inside another holds what is beneath its own mount.
    pub mount: Box<[Name]>,
    /// io's name for its root directory, beneath which io resolves every
    /// place in it.
    pub root: Token,
    pub writable: bool,
}

/// The families of tools a kit may use.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Grants {
    /// Read, list and search.
    pub inspect: bool,
    /// Write and edit, in the writable repositories.
    pub modify: bool,
    /// Run commands.
    pub shell: bool,
}

/// An authority as a kit keeps it, with its mounts joined for choosing among
/// them.
#[derive(PartialEq, Eq, Hash, Debug)]
pub(crate) struct Checkout {
    pub(crate) cwd: Box<[Name]>,
    pub(crate) mounts: Box<[Mount]>,
    pub(crate) grants: Grants,
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub(crate) struct Mount {
    /// The repository's mount, its names joined by `/`.
    pub(crate) at: Box<[u8]>,
    pub(crate) root: Token,
    pub(crate) writable: bool,
}

/// Where a path is, and whether the kit may write there.
#[derive(PartialEq, Eq, Debug)]
pub(crate) struct Located {
    pub(crate) place: Place,
    pub(crate) writable: bool,
}

/// The checkout for `authority`, or `None` if it does not fit `limits`: too
/// many repositories, or a path longer than the tools take.
pub(crate) fn admit(authority: Authority, limits: &Limits) -> Option<Checkout> {
    let repos = u32::try_from(authority.repos.len()).ok()?;
    if repos > limits.repos {
        return None;
    }
    let cwd = path::refs(&authority.cwd)?;
    if path::joined(cwd.as_slice())? > usize::try_from(limits.path_bytes).ok()? {
        return None;
    }
    let mut mounts = List::with_capacity(repos);
    for repo in &authority.repos {
        let names = path::refs(&repo.mount)?;
        let at = path::join(names.as_slice(), limits.path_bytes)?;
        let mount = Mount { at, root: repo.root, writable: repo.writable };
        mounts.push(mount).expect("room for every repository");
    }
    Some(Checkout { cwd: authority.cwd, mounts: mounts.into_boxed(), grants: authority.grants })
}

/// Whether `grants` cover `call`.
pub(crate) const fn granted(grants: Grants, call: &Call) -> bool {
    match call {
        Call::Read { .. } | Call::List { .. } | Call::Search { .. } => grants.inspect,
        Call::Write { .. } | Call::Edit { .. } => grants.modify,
        Call::Shell { .. } => grants.shell,
    }
}

/// Where `path` is in `checkout`, or the outcome that refuses it: `TooLong`
/// past `max` bytes, `Outside` beyond every repository.
pub(crate) fn locate(checkout: &Checkout, path: &Path, max: u32) -> Result<Located, Outcome> {
    let Some(at) = path::normalise(&checkout.cwd, path, max) else {
        return Err(Outcome::TooLong);
    };
    let Some(index) = choose(&checkout.mounts, &at) else {
        return Err(Outcome::Outside);
    };
    let mount = checkout.mounts.get(index).expect("chosen among the mounts");
    let start = beneath(&mount.at, &at).expect("the chosen mount holds the path");
    let path = copy_of(at.get(start..).expect("the path beneath a mount is within it"));
    Ok(Located { place: Place { root: mount.root, path }, writable: mount.writable })
}

/// The repository that holds `at`, an absolute path's names joined: the one
/// with the longest mount that is `at` or a directory above it, and the first
/// of those mounted there; `None` if `at` is outside them all.
pub(crate) fn choose(mounts: &[Mount], at: &[u8]) -> Option<usize> {
    let mut chosen: Option<(usize, usize)> = None;
    for (index, mount) in mounts.iter().enumerate() {
        if beneath(&mount.at, at).is_none() {
            continue;
        }
        let longer = match chosen {
            Some((_, len)) => mount.at.len() > len,
            None => true,
        };
        if longer {
            chosen = Some((index, mount.at.len()));
        }
    }
    let (index, _) = chosen?;
    Some(index)
}

/// Where the path beneath `mount` starts in `at`, or `None` if `mount` does not
/// hold `at`.
pub(crate) fn beneath(mount: &[u8], at: &[u8]) -> Option<usize> {
    // The root holds everything.
    if mount.is_empty() {
        return Some(0);
    }
    let rest = at.strip_prefix(mount)?;
    match rest.first() {
        None => Some(mount.len()),
        Some(b'/') => mount.len().checked_add(1),
        Some(_) => None,
    }
}
