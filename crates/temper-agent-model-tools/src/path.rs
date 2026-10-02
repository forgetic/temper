//! Paths: how the LLM names files, and where they are.
//!
//! The protocol layer splits the path the LLM wrote at its slashes into typed
//! parts, a [`Path`]. The model resolves it against the kit's working
//! directory and drops `.` and `..` ([`normalise`]), then picks the repository
//! it falls in (`authority::choose`). What comes out is a [`Place`]: a
//! repository's root, as io names it, and a path beneath it with no `.` or
//! `..` left, which io resolves beneath that root, so that a symbolic link
//! leading out of the repository is caught there.
//!
//! `..` is resolved by the names, not by the files: `a/link/..` is `a`
//! wherever `link` points, and `..` at the root stays at the root.

use alloc::boxed::Box;

use temper_lib::{List, Token, Writer};

/// One component of a path: the name of a file or a directory. Never empty,
/// `.` or `..`, and free of `/` and NUL, so names joined by `/` read back as
/// the same names.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Name(Box<[u8]>);

impl Name {
    /// `bytes` as a name, or `None` if they cannot be one.
    #[must_use]
    pub fn new(bytes: Box<[u8]>) -> Option<Name> {
        if bytes.is_empty() || *bytes == *b"." || *bytes == *b".." {
            return None;
        }
        for byte in &bytes {
            if *byte == b'/' || *byte == 0 {
                return None;
            }
        }
        Some(Name(bytes))
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// A path as the LLM wrote it, split at its slashes by the protocol layer.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Path {
    /// It starts at the root, rather than at the kit's working directory.
    pub absolute: bool,
    /// What is between the slashes, in order, without the empty parts that
    /// doubled slashes leave.
    pub parts: Box<[Part]>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Part {
    /// A file or a directory.
    Name { name: Name },
    /// `.`
    Current,
    /// `..`
    Parent,
}

/// Where a file is: a repository's root, as io names it, and the path beneath
/// it, names joined by `/`, empty for the root itself. Made by the model from a
/// [`Path`]; io resolves it beneath the root.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Place {
    pub root: Token,
    pub path: Box<[u8]>,
}

/// The absolute path `path` names, read from `cwd` when it is relative, as
/// names joined by `/` with no leading one (empty for the root); or `None` if
/// that is longer than `max` bytes.
pub(crate) fn normalise(cwd: &[Name], path: &Path, max: u32) -> Option<Box<[u8]>> {
    // Which parts are kept, found from the last: each `..` takes out the
    // nearest name before it that is still kept.
    let parts = u32::try_from(path.parts.len()).ok()?;
    let mut kept = List::with_capacity(parts);
    for _ in 0..parts {
        kept.push(false).expect("room for a flag per part");
    }
    let mut parents: usize = 0;
    for index in (0..parts).rev() {
        match path.parts.get(usize::try_from(index).ok()?)? {
            Part::Name { .. } => match parents.checked_sub(1) {
                Some(left) => parents = left,
                None => *kept.get_mut(index)? = true,
            },
            Part::Current => {}
            Part::Parent => parents = parents.checked_add(1)?,
        }
    }
    // A relative path starts at the working directory, less the names the
    // `..`s left over take out; an absolute one at the root, where they stay.
    let base: &[Name] = if path.absolute { &[] } else { cwd.get(..cwd.len().saturating_sub(parents))? };
    let mut names = List::with_capacity(u32::try_from(base.len()).ok()?.checked_add(parts)?);
    for name in base {
        names.push(name).expect("room for every name");
    }
    for (part, keep) in path.parts.iter().zip(&kept) {
        match part {
            Part::Name { name } if *keep => names.push(name).expect("room for every name"),
            Part::Name { .. } | Part::Current | Part::Parent => {}
        }
    }
    join(names.as_slice(), max)
}

/// `names` joined by `/`, or `None` if that is longer than `max` bytes.
pub(crate) fn join(names: &[&Name], max: u32) -> Option<Box<[u8]>> {
    let len = joined(names)?;
    if len > usize::try_from(max).ok()? {
        return None;
    }
    let mut writer = Writer::new(len);
    for (index, name) in names.iter().enumerate() {
        if index > 0 {
            writer.put(b"/").expect("the length counts every slash");
        }
        writer.put(&name.0).expect("the length counts every name");
    }
    Some(writer.finish())
}

/// The length of `names` joined by `/`, or `None` past a `usize`.
pub(crate) fn joined(names: &[&Name]) -> Option<usize> {
    let mut len: usize = 0;
    for name in names {
        len = len.checked_add(name.0.len())?.checked_add(1)?;
    }
    // No slash after the last.
    Some(len.saturating_sub(1))
}

/// Whether `path`, names joined by `/`, has a git directory among its names:
/// `.git`, in any ASCII case, for a file system that folds case would take
/// `.GIT` for it.
pub(crate) fn in_git(path: &[u8]) -> bool {
    let mut start: usize = 0;
    for (index, byte) in path.iter().enumerate() {
        if *byte == b'/' {
            if is_git(path.get(start..index)) {
                return true;
            }
            start = index.saturating_add(1);
        }
    }
    is_git(path.get(start..))
}

fn is_git(name: Option<&[u8]>) -> bool {
    match name {
        Some(name) => name.eq_ignore_ascii_case(b".git"),
        None => false,
    }
}

/// References to each of `names`, to join them.
pub(crate) fn refs(names: &[Name]) -> Option<List<&Name>> {
    let mut refs = List::with_capacity(u32::try_from(names.len()).ok()?);
    for name in names {
        refs.push(name).expect("room for every name");
    }
    Some(refs)
}
