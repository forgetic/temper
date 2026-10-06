//! Between the checkout's operations and the fakes, as the protocol layer
//! and io would translate them: io names a workspace's directory by its token,
//! and a repository's by the workspace's and the repository's name; a commit
//! is the fake forge's count, in the first bytes of the domain's hash.

use skein_lib::Token;
use temper_fake_checkout::Checkout;
use temper_fake_checkout::git::{self, Created, Pushed, Remote, What};
use temper_worker_domain_checkout::git::{Commit, Done, Fault, Missing, Op, Place, Want};

/// The domain's name for the fake's commit `fake`.
#[must_use]
pub fn commit(fake: u64) -> Commit {
    let mut raw = [0; 32];
    raw[..8].copy_from_slice(&fake.to_be_bytes());
    Commit::new(raw)
}

/// The fake's name for the domain's `commit`.
#[must_use]
pub fn fake(commit: Commit) -> u64 {
    let raw = commit.raw();
    let mut count = [0; 8];
    count.copy_from_slice(&raw[..8]);
    assert!(raw[8..].iter().all(|byte| *byte == 0), "a commit the forge named");
    u64::from_be_bytes(count)
}

/// Where io keeps the workspace `workspace`.
#[must_use]
pub fn dir(workspace: Token) -> Vec<u8> {
    format!("ws/{}", workspace.raw()).into_bytes()
}

/// Where io keeps the repository at `place`.
#[must_use]
pub fn path(place: &Place) -> Vec<u8> {
    [dir(place.workspace).as_slice(), b"/", &place.repository].concat()
}

/// The workspace an operation is in.
#[must_use]
pub fn workspace(op: &Op) -> Token {
    match op {
        Op::Make { workspace } => *workspace,
        Op::Clone { at, .. }
        | Op::Fetch { at, .. }
        | Op::Create { at, .. }
        | Op::CheckOut { at, .. }
        | Op::Merge { at, .. }
        | Op::Commit { at, .. }
        | Op::Push { at, .. } => at.workspace,
    }
}

/// The identity an operation acts as, if it names one.
#[must_use]
pub fn identity(op: &Op) -> Option<u32> {
    match op {
        Op::Clone { identity, .. }
        | Op::Fetch { identity, .. }
        | Op::Create { identity, .. }
        | Op::Commit { identity, .. }
        | Op::Push { identity, .. } => Some(*identity),
        Op::Make { .. } | Op::CheckOut { .. } | Op::Merge { .. } => None,
    }
}

/// The repository a remote operation reaches, by the forge's address for it.
#[must_use]
pub fn remote(op: &Op) -> Option<&[u8]> {
    match op {
        Op::Clone { remote, .. } | Op::Fetch { remote, .. } | Op::Create { remote, .. } | Op::Push { remote, .. } => {
            Some(remote)
        }
        Op::Make { .. } | Op::CheckOut { .. } | Op::Merge { .. } | Op::Commit { .. } => None,
    }
}

/// Runs `op` on the fakes, as the protocol layer would run its git
/// invocation, git reaching the forge through `forge`, and says how it ended.
pub fn perform(forge: &mut impl Remote, disk: &mut Checkout, op: Op) -> Done {
    match op {
        Op::Make { workspace } => {
            // Empty: whatever is there goes first.
            let dir = dir(workspace);
            disk.remove(&dir);
            disk.mkdir(&dir);
            Done::Succeeded
        }
        Op::Clone { at, remote, identity: _ } => {
            assert!(disk.exists(&dir(at.workspace)), "a repository is cloned into a workspace made");
            match git::clone_repository(forge, disk, &remote, &path(&at)) {
                Ok(()) => Done::Succeeded,
                Err(fault) => failed(fault),
            }
        }
        Op::Fetch { at, remote, want, identity: _ } => {
            assert_cloned(disk, &at);
            let want = match &want {
                Want::Branch { branch } => git::Want::Branch(branch),
                Want::Commit { commit } => git::Want::Commit(fake(*commit)),
                Want::Default => git::Want::Default,
            };
            match git::fetch(forge, disk, &remote, &path(&at), want) {
                Ok(fetched) => Done::Fetched { commit: commit(fetched) },
                Err(fault) => failed(fault),
            }
        }
        Op::Create { at, remote, branch, commit, identity: _ } => {
            assert_cloned(disk, &at);
            match git::create(forge, &remote, &branch, fake(commit)) {
                Ok(Created::Created) => Done::Succeeded,
                Ok(Created::Exists) => Done::Exists,
                Err(fault) => failed(fault),
            }
        }
        Op::CheckOut { at, commit } => {
            assert_cloned(disk, &at);
            let checked_out = git::check_out(forge, disk, &path(&at), fake(commit));
            checked_out.expect("a checkout is of a commit fetched into the repository");
            Done::Succeeded
        }
        Op::Commit { at, parent, merging, title, body, identity: _ } => {
            assert_cloned(disk, &at);
            assert!(!title.is_empty(), "a commit has a title");
            let message =
                if body.is_empty() { title.to_vec() } else { [title.as_ref(), b"\n\n", body.as_ref()].concat() };
            match merging {
                None => match git::commit(forge, disk, &path(&at), fake(parent), &message)
                    .expect("parent is locally fetched")
                {
                    Some(committed) => Done::Committed { commit: commit(committed) },
                    None => Done::Unchanged,
                },
                Some(second) => {
                    match git::commit_merging(forge, disk, &path(&at), fake(parent), fake(second), &message) {
                        Ok(committed) => Done::Committed { commit: commit(committed) },
                        Err(git::CommitFailure::Unresolved { files }) => {
                            Done::Conflicted { files: files.into_iter().map(Vec::into_boxed_slice).collect() }
                        }
                        Err(git::CommitFailure::NotFetched) => panic!("both merge parents are locally fetched"),
                    }
                }
            }
        }
        Op::Merge { at, theirs } => {
            assert_cloned(disk, &at);
            let merged = git::merge(forge, disk, &path(&at), fake(theirs)).expect("merge parent is locally fetched");
            if merged.conflicts.is_empty() {
                Done::Merged
            } else {
                Done::Conflicted { files: merged.conflicts.into_iter().map(Vec::into_boxed_slice).collect() }
            }
        }
        Op::Push { at, remote, commit, branch, expected, identity: _ } => {
            assert_cloned(disk, &at);
            let pushed = match expected {
                Some(expected) => {
                    git::push_expected(forge, disk, &remote, &path(&at), fake(commit), &branch, fake(expected))
                }
                None => git::push(forge, disk, &remote, &path(&at), fake(commit), &branch),
            };
            match pushed {
                Ok(Pushed::Pushed) => Done::Succeeded,
                Ok(Pushed::Rejected) => Done::Rejected,
                Err(git::Fault::Refused) => Done::FailedWithOutput {
                    fault: Fault::Refused,
                    diagnostic: temper_worker_domain_checkout::git::PushDiagnostic::new(b"remote: push refused", 0),
                },
                Err(fault) => failed(fault),
            }
        }
    }
}

fn failed(fault: git::Fault) -> Done {
    let fault = match fault {
        git::Fault::Missing(What::Repository) => Fault::Missing { missing: Missing::Repository },
        git::Fault::Missing(What::Branch) => Fault::Missing { missing: Missing::Branch },
        git::Fault::Missing(What::Commit) => Fault::Missing { missing: Missing::Commit },
        git::Fault::Refused => Fault::Refused,
        git::Fault::Unreachable => Fault::Unreachable,
    };
    Done::Failed { fault }
}

fn assert_cloned(disk: &Checkout, at: &Place) {
    let git = [path(at).as_slice(), b"/.git"].concat();
    assert!(disk.exists(&git), "an operation in a repository runs in a clone: {:?}", String::from_utf8_lossy(&git));
}
