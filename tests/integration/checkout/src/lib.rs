//! A fake checkout: directories, files, symbolic links and special files in
//! memory, and scripted commands that run in it, standing in for io's files
//! and processes in the agent's model worlds (the tools' now, the session's
//! and the whole agent's later). It shares no types with the model: a world
//! translates between them, as a protocol layer would.
//!
//! Paths are absolute, as bytes without the leading slash (`work/temper/src`;
//! the root is empty). The operations io offers the model resolve a path
//! beneath a registered root as io does with `openat2(RESOLVE_BENEATH)`: a
//! `..` that would climb above the root, or an absolute symbolic link, escapes
//! and is refused; relative links are followed, up to a limit. Every change to
//! a file gives it a new version, never reused, as a file renamed into place
//! gets a new inode.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

/// The most symbolic links one resolution follows, as Linux does.
const LINKS: u32 = 40;

#[derive(Debug, Default)]
pub struct Checkout {
    /// Everything there is, by absolute path.
    nodes: BTreeMap<Vec<u8>, Node>,
    /// The directories operations resolve beneath, by their names.
    roots: BTreeMap<u64, Vec<u8>>,
    /// The last version given to a file.
    versions: u64,
    /// What each scripted command does.
    programs: BTreeMap<Vec<u8>, Program>,
}

/// What a scripted command does: how long it runs, what it writes, how it
/// ends, and the files it changes as it goes, by absolute path: written with
/// their new content, or removed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Program {
    pub duration: Duration,
    pub output: Vec<u8>,
    pub exit: Exit,
    pub changes: Vec<(Vec<u8>, Option<Vec<u8>>)>,
}

/// What a search found: the file each line is in (beneath the path
/// searched), its number counting from 1, and its text; and how many more
/// lines matched.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Found {
    pub hits: Vec<(Vec<u8>, usize, Vec<u8>)>,
    pub more: u64,
}

/// Why a search found nothing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Searched {
    /// rg could not read the pattern, and said so on standard error.
    Unreadable(Vec<u8>),
    Failed(Failure),
}

/// How a command ends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Exit {
    Code(u8),
    Signal(u8),
}

/// A command started: what it will do, and the roots it sees, by path, with
/// whether it may change files beneath them.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Process {
    pub program: Program,
    roots: Vec<(Vec<u8>, bool)>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Node {
    Directory,
    File {
        content: Vec<u8>,
        version: u64,
    },
    Link {
        target: Vec<u8>,
    },
    /// A device, a socket, a pipe.
    Special,
}

/// What an entry of a directory is, without following links.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Kind {
    File,
    Directory,
    Link,
    Special,
}

/// Why an operation did nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Failure {
    /// Nothing is there.
    Missing,
    /// What is there is not a regular file (nor, to store, a link).
    NotFile,
    /// What is there, or a directory on the way, is not a directory.
    NotDirectory,
    /// The file is larger than asked for.
    TooLarge { size: u64 },
    /// A `..` or a link leads out of the root.
    Escapes,
    /// To store: a part of the path is a link, which a store does not follow.
    Linked,
    /// Too many links on the way.
    Loop,
    /// To store: the file is not as expected.
    Conflict { now: Option<u64> },
}

/// How an operation resolves its path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Resolve {
    /// Following links, as a load or a scan does.
    Follow,
    /// Following none, and making the directories missing on the way, as a
    /// store does.
    Store,
}

/// What a store expects to replace.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Expect {
    /// Nothing: the file is created.
    Absent,
    /// The file at this version.
    Is(u64),
}

/// A directory's entries, the first in name order, and how many more it has.
pub type Listing = (Vec<(Vec<u8>, Kind)>, u64);

impl Checkout {
    #[must_use]
    pub fn new() -> Checkout {
        Checkout::default()
    }

    /// Makes the directory `at` a root that operations resolve beneath, and
    /// names it.
    pub fn root(&mut self, at: &[u8]) -> u64 {
        self.mkdir(at);
        let root = u64::try_from(self.roots.len()).expect("few roots") + 1;
        self.roots.insert(root, at.to_vec());
        root
    }

    /// Where the root `root` is.
    #[must_use]
    pub fn root_path(&self, root: u64) -> &[u8] {
        self.roots.get(&root).expect("a root the checkout named")
    }

    /// Scripts `command`: from now on it does what `program` says.
    pub fn program(&mut self, command: &[u8], program: Program) {
        self.programs.insert(command.to_vec(), program);
    }

    /// Starts `command` with the shell in the directory at `cwd` beneath
    /// `root`, with the environment `env` and nothing else, able to change
    /// files only where the deepest of `roots` that holds them is writable.
    /// `env` prints the environment; a command never scripted is not found.
    pub fn spawn(
        &self,
        root: u64,
        cwd: &[u8],
        command: &[u8],
        env: &[(Vec<u8>, Vec<u8>)],
        roots: &[(u64, bool)],
    ) -> Result<Process, Failure> {
        let (at, _) = self.resolve(root, cwd, Resolve::Follow)?;
        match self.nodes.get(&at) {
            None => return Err(Failure::Missing),
            Some(Node::Directory) => {}
            Some(Node::File { .. } | Node::Special) => return Err(Failure::NotDirectory),
            Some(Node::Link { .. }) => unreachable!("links are followed"),
        }
        let program = if command == b"env" {
            let mut output = Vec::new();
            for (name, value) in env {
                output.extend_from_slice(&[&name[..], b"=", value, b"\n"].concat());
            }
            Program { duration: Duration::from_millis(1), output, exit: Exit::Code(0), changes: Vec::new() }
        } else if let Some(program) = self.programs.get(command) {
            program.clone()
        } else {
            let output = [b"sh: ", command, b": not found\n"].concat();
            Program { duration: Duration::from_millis(1), output, exit: Exit::Code(127), changes: Vec::new() }
        };
        let roots = roots.iter().map(|(root, writable)| (self.root_path(*root).to_vec(), *writable)).collect();
        Ok(Process { program, roots })
    }

    /// The changes `process` made as it ran: those where the deepest root
    /// that holds them is writable, outside any git directory (`.git` in any
    /// case). Changes elsewhere fail, as on a read-only file system.
    pub fn finish(&mut self, process: &Process) {
        for (path, content) in &process.program.changes {
            let deepest = process
                .roots
                .iter()
                .filter(|(root, _)| {
                    root.is_empty()
                        || path.strip_prefix(root.as_slice()).is_some_and(|rest| rest.first() == Some(&b'/'))
                })
                .max_by_key(|(root, _)| root.len());
            let Some((_, true)) = deepest else {
                continue;
            };
            if in_git(path) {
                continue;
            }
            match content {
                Some(content) => drop(self.write(path, content)),
                None => self.remove(path),
            }
        }
    }

    /// Searches the files at and beneath `path` beneath `root` for lines
    /// holding `pattern`, as rg would for a pattern without special
    /// characters: in path order, following no link beneath `path`, skipping
    /// hidden files and directories, in those whose names end as `glob` does
    /// after its `*` if given. At most `hits` lines, with at most `bytes` of
    /// paths and text between them, the last one's text cut to fit; and how
    /// many more matched. A
    /// pattern with an unclosed `(` is one rg cannot read.
    pub fn search(
        &self,
        root: u64,
        path: &[u8],
        pattern: &[u8],
        glob: Option<&[u8]>,
        (hits, bytes): (usize, usize),
    ) -> Result<Found, Searched> {
        if pattern.contains(&b'(') && !pattern.contains(&b')') {
            return Err(Searched::Unreadable(b"rg: regex parse error: unclosed group\n".to_vec()));
        }
        let (at, _) = self.resolve(root, path, Resolve::Follow).map_err(Searched::Failed)?;
        let files: Vec<(&Vec<u8>, &Vec<u8>)> = match self.nodes.get(&at) {
            None => return Err(Searched::Failed(Failure::Missing)),
            Some(Node::File { content, .. }) => vec![(&at, content)],
            Some(Node::Special) => Vec::new(),
            Some(Node::Directory) => {
                let prefix = if at.is_empty() { Vec::new() } else { [&at[..], b"/"].concat() };
                let mut files = Vec::new();
                for (file, node) in self.nodes.range(prefix.clone()..) {
                    let Some(beneath) = file.strip_prefix(prefix.as_slice()) else { break };
                    let hidden = beneath.split(|byte| *byte == b'/').any(|name| name.first() == Some(&b'.'));
                    match node {
                        Node::File { content, .. } if !hidden => files.push((file, content)),
                        Node::File { .. } | Node::Directory | Node::Link { .. } | Node::Special => {}
                    }
                }
                files
            }
            Some(Node::Link { .. }) => unreachable!("links are followed"),
        };
        let mut found = Found { hits: Vec::new(), more: 0 };
        let mut left = bytes;
        for (file, content) in files {
            let name = file.rsplit(|byte| *byte == b'/').next().unwrap_or(file);
            if let Some(glob) = glob
                && !name.ends_with(glob.strip_prefix(b"*").unwrap_or(glob))
            {
                continue;
            }
            let beneath = file.strip_prefix(at.as_slice()).unwrap_or(file);
            let beneath = beneath.strip_prefix(b"/").unwrap_or(beneath).to_vec();
            for (index, line) in content.split(|byte| *byte == b'\n').enumerate() {
                if !line.windows(pattern.len().max(1)).any(|window| window == pattern) {
                    continue;
                }
                // A hit costs its path and its text.
                if found.hits.len() >= hits || left <= beneath.len() {
                    found.more += 1;
                    left = 0;
                    continue;
                }
                left -= beneath.len();
                let text = line[..line.len().min(left)].to_vec();
                left -= text.len();
                found.hits.push((beneath.clone(), index + 1, text));
            }
        }
        Ok(found)
    }

    // What anything else on the machine does: an outsider changing the
    // checkout, or a test setting it up.

    /// Makes the directory `at`, and those on the way.
    pub fn mkdir(&mut self, at: &[u8]) {
        for end in boundaries(at) {
            match self.nodes.get(&at[..end]) {
                None => drop(self.nodes.insert(at[..end].to_vec(), Node::Directory)),
                Some(Node::Directory) => {}
                Some(node) => panic!("{:?} is a {node:?}, not a directory", String::from_utf8_lossy(&at[..end])),
            }
        }
    }

    /// Makes `at` a file that holds `content`, at a new version, and returns
    /// it.
    pub fn write(&mut self, at: &[u8], content: &[u8]) -> u64 {
        self.parents(at);
        let version = self.next_version();
        let node = Node::File { content: content.to_vec(), version };
        match self.nodes.insert(at.to_vec(), node) {
            None | Some(Node::File { .. }) => version,
            Some(node) => panic!("{:?} was a {node:?}", String::from_utf8_lossy(at)),
        }
    }

    /// Makes `at` a symbolic link to `target`.
    pub fn link(&mut self, at: &[u8], target: &[u8]) {
        self.parents(at);
        self.nodes.insert(at.to_vec(), Node::Link { target: target.to_vec() });
    }

    /// Makes `at` a special file.
    pub fn special(&mut self, at: &[u8]) {
        self.parents(at);
        self.nodes.insert(at.to_vec(), Node::Special);
    }

    /// Removes what is at `at`, and everything beneath it.
    pub fn remove(&mut self, at: &[u8]) {
        let beneath = [at, b"/"].concat();
        self.nodes.retain(|path, _| path != at && !path.starts_with(&beneath));
    }

    /// What the file at `at` holds, not following links.
    #[must_use]
    pub fn content(&self, at: &[u8]) -> Option<&[u8]> {
        match self.nodes.get(at)? {
            Node::File { content, .. } => Some(content),
            Node::Directory | Node::Link { .. } | Node::Special => None,
        }
    }

    /// The version of the file at `at`, not following links.
    #[must_use]
    pub fn version(&self, at: &[u8]) -> Option<u64> {
        match self.nodes.get(at)? {
            Node::File { version, .. } => Some(*version),
            Node::Directory | Node::Link { .. } | Node::Special => None,
        }
    }

    /// Every file, by path, with its content: what a test compares.
    #[must_use]
    pub fn files(&self) -> BTreeMap<&[u8], &[u8]> {
        let mut files = BTreeMap::new();
        for (path, node) in &self.nodes {
            match node {
                Node::File { content, .. } => drop(files.insert(path.as_slice(), content.as_slice())),
                Node::Directory | Node::Link { .. } | Node::Special => {}
            }
        }
        files
    }

    // What io does for the model, beneath a root.

    /// The content and version of the file at `path` beneath `root`, if it
    /// holds at most `max` bytes.
    pub fn load(&self, root: u64, path: &[u8], max: u64) -> Result<(Vec<u8>, u64), Failure> {
        let (at, _) = self.resolve(root, path, Resolve::Follow)?;
        match self.nodes.get(&at) {
            None => Err(Failure::Missing),
            Some(Node::File { content, version }) => {
                let size = u64::try_from(content.len()).expect("a small file");
                if size > max {
                    return Err(Failure::TooLarge { size });
                }
                Ok((content.clone(), *version))
            }
            Some(Node::Directory | Node::Special) => Err(Failure::NotFile),
            Some(Node::Link { .. }) => unreachable!("links are followed"),
        }
    }

    /// The entries of the directory at `path` beneath `root`: at most `max`,
    /// the first in name order.
    pub fn scan(&self, root: u64, path: &[u8], max: usize) -> Result<Listing, Failure> {
        let (at, _) = self.resolve(root, path, Resolve::Follow)?;
        match self.nodes.get(&at) {
            None => return Err(Failure::Missing),
            Some(Node::Directory) => {}
            Some(Node::File { .. } | Node::Special) => return Err(Failure::NotDirectory),
            Some(Node::Link { .. }) => unreachable!("links are followed"),
        }
        let prefix = if at.is_empty() { Vec::new() } else { [&at[..], b"/"].concat() };
        let mut entries = Vec::new();
        let mut more = 0;
        for (path, node) in self.nodes.range(prefix.clone()..) {
            let Some(name) = path.strip_prefix(prefix.as_slice()) else { break };
            if name.is_empty() || name.contains(&b'/') {
                continue;
            }
            if entries.len() < max {
                entries.push((name.to_vec(), kind(node)));
            } else {
                more += 1;
            }
        }
        Ok((entries, more))
    }

    /// Makes the file at `path` beneath `root` hold `content` if it is as
    /// `expect` says, creating the directories on the way, and returns its
    /// new version. No part of `path` may be a link.
    pub fn store(&mut self, root: u64, path: &[u8], content: &[u8], expect: Expect) -> Result<u64, Failure> {
        let (at, missing) = self.resolve(root, path, Resolve::Store)?;
        let now = match self.nodes.get(&at) {
            None => None,
            Some(Node::File { version, .. }) => Some(*version),
            Some(Node::Directory | Node::Special) => return Err(Failure::NotFile),
            Some(Node::Link { .. }) => unreachable!("a store refuses links"),
        };
        let expected = match expect {
            Expect::Absent => None,
            Expect::Is(version) => Some(version),
        };
        if now != expected {
            return Err(Failure::Conflict { now });
        }
        for directory in missing {
            self.nodes.insert(directory, Node::Directory);
        }
        let version = self.next_version();
        self.nodes.insert(at, Node::File { content: content.to_vec(), version });
        Ok(version)
    }

    /// Where `path` beneath `root` leads: the absolute path of what is there;
    /// and, to store, the directories missing on the way, which are taken as
    /// there.
    fn resolve(&self, root: u64, path: &[u8], resolve: Resolve) -> Result<(Vec<u8>, Vec<Vec<u8>>), Failure> {
        let base = self.roots.get(&root).expect("a root the checkout named");
        let mut names: Vec<Vec<u8>> = Vec::new();
        let mut todo: VecDeque<Vec<u8>> = parts(path).into();
        let mut missing = Vec::new();
        let mut links = 0;
        while let Some(name) = todo.pop_front() {
            match name.as_slice() {
                b"" | b"." => continue,
                b".." => {
                    names.pop().ok_or(Failure::Escapes)?;
                    continue;
                }
                _ => {}
            }
            let last = todo.is_empty();
            let at = absolute(base, &names, &name);
            match self.nodes.get(&at) {
                None => {
                    // What is missing at the end is what the operation is
                    // about; on the way, it is a directory to make.
                    if !last && !missing.contains(&at) {
                        match resolve {
                            Resolve::Follow => return Err(Failure::Missing),
                            Resolve::Store => missing.push(at),
                        }
                    }
                }
                Some(Node::Directory) => {}
                Some(Node::File { .. } | Node::Special) => {
                    if !last {
                        return Err(Failure::NotDirectory);
                    }
                }
                Some(Node::Link { target }) => {
                    match resolve {
                        Resolve::Follow => {}
                        Resolve::Store => return Err(Failure::Linked),
                    }
                    links += 1;
                    if links > LINKS {
                        return Err(Failure::Loop);
                    }
                    if target.first() == Some(&b'/') {
                        return Err(Failure::Escapes);
                    }
                    for part in parts(target).into_iter().rev() {
                        todo.push_front(part);
                    }
                    continue;
                }
            }
            names.push(name);
        }
        Ok((absolute(base, &names, b""), missing))
    }

    /// Makes the directories on the way to `at`.
    fn parents(&mut self, at: &[u8]) {
        if let Some(slash) = at.iter().rposition(|byte| *byte == b'/') {
            self.mkdir(&at[..slash]);
        }
    }

    fn next_version(&mut self) -> u64 {
        self.versions += 1;
        self.versions
    }
}

fn kind(node: &Node) -> Kind {
    match node {
        Node::Directory => Kind::Directory,
        Node::File { .. } => Kind::File,
        Node::Link { .. } => Kind::Link,
        Node::Special => Kind::Special,
    }
}

/// Whether `path` has a git directory, `.git` in any ASCII case, among its
/// names.
#[must_use]
pub fn in_git(path: &[u8]) -> bool {
    path.split(|byte| *byte == b'/').any(|name| name.eq_ignore_ascii_case(b".git"))
}

/// What is between the slashes of `path`.
fn parts(path: &[u8]) -> Vec<Vec<u8>> {
    path.split(|byte| *byte == b'/').map(<[u8]>::to_vec).collect()
}

/// Where each directory on the way to `at`, and `at` itself, ends.
fn boundaries(at: &[u8]) -> Vec<usize> {
    let mut ends: Vec<usize> = at.iter().enumerate().filter(|(_, byte)| **byte == b'/').map(|(end, _)| end).collect();
    ends.push(at.len());
    ends
}

/// `base`, then `names`, then `name`, joined by slashes.
fn absolute(base: &[u8], names: &[Vec<u8>], name: &[u8]) -> Vec<u8> {
    let mut parts: Vec<&[u8]> = Vec::new();
    for part in std::iter::once(base).chain(names.iter().map(Vec::as_slice)).chain(std::iter::once(name)) {
        if !part.is_empty() {
            parts.push(part);
        }
    }
    parts.join(&b'/')
}
