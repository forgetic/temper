//! Between the checkout's operations and the fakes, as the protocol layer
//! and io would translate them: io names a workspace's directory by its token,
//! and a repository's by the workspace's and the repository's name; a commit
//! is the fake's count, in the first bytes of the model's hash.

use temper_checkout_fake::Checkout;
use temper_checkout_fake::git::{self, Created, Forge, Pushed, What};
use temper_lib::Token;
use temper_worker_model_checkout::git::{Commit, Done, Fault, Missing, Op, Place, Want};

/// The model's name for the fake's commit `fake`.
#[must_use]
pub fn commit(fake: u64) -> Commit {
    let mut raw = [0; 32];
    raw[..8].copy_from_slice(&fake.to_be_bytes());
    Commit::new(raw)
}

/// The fake's name for the model's `commit`.
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
        Op::Make { workspace } | Op::Remove { workspace } => *workspace,
        Op::Clone { at, .. }
        | Op::Fetch { at, .. }
        | Op::Create { at, .. }
        | Op::CheckOut { at, .. }
        | Op::Commit { at, .. }
        | Op::Push { at, .. } => at.workspace,
    }
}

/// The identity an operation acts as, if it names one.
#[must_use]
pub fn identity(op: &Op) -> Option<&[u8]> {
    match op {
        Op::Clone { identity, .. }
        | Op::Fetch { identity, .. }
        | Op::Create { identity, .. }
        | Op::Commit { identity, .. }
        | Op::Push { identity, .. } => Some(identity),
        Op::Make { .. } | Op::Remove { .. } | Op::CheckOut { .. } => None,
    }
}

/// The repository a remote operation reaches.
#[must_use]
pub fn remote(op: &Op) -> Option<&[u8]> {
    match op {
        Op::Clone { at, .. } | Op::Fetch { at, .. } | Op::Create { at, .. } | Op::Push { at, .. } => {
            Some(&at.repository)
        }
        Op::Make { .. } | Op::Remove { .. } | Op::CheckOut { .. } | Op::Commit { .. } => None,
    }
}

/// Runs `op` on the fakes, as the protocol layer would run its git
/// invocation, and says how it ended.
pub fn perform(forge: &mut Forge, disk: &mut Checkout, op: Op) -> Done {
    match op {
        Op::Make { workspace } => {
            let dir = dir(workspace);
            assert!(!disk.exists(&dir), "a workspace is made where nothing is");
            disk.mkdir(&dir);
            Done::Succeeded
        }
        Op::Remove { workspace } => {
            disk.remove(&dir(workspace));
            Done::Succeeded
        }
        Op::Clone { at, identity: _ } => {
            assert!(disk.exists(&dir(at.workspace)), "a repository is cloned into a workspace made");
            match forge.clone_repository(disk, &at.repository, &path(&at)) {
                Ok(()) => Done::Succeeded,
                Err(fault) => failed(fault),
            }
        }
        Op::Fetch { at, want, identity: _ } => {
            assert_cloned(disk, &at);
            let want = match &want {
                Want::Branch { branch } => git::Want::Branch(branch),
                Want::Commit { commit } => git::Want::Commit(fake(*commit)),
                Want::Default => git::Want::Default,
            };
            match forge.fetch(&at.repository, want) {
                Ok(fetched) => Done::Fetched { commit: commit(fetched) },
                Err(fault) => failed(fault),
            }
        }
        Op::Create { at, branch, commit, identity: _ } => {
            assert_cloned(disk, &at);
            match forge.create(&at.repository, &branch, fake(commit)) {
                Ok(Created::Created) => Done::Succeeded,
                Ok(Created::Exists) => Done::Exists,
                Err(fault) => failed(fault),
            }
        }
        Op::CheckOut { at, commit } => {
            assert_cloned(disk, &at);
            forge.check_out(disk, &path(&at), fake(commit));
            Done::Succeeded
        }
        Op::Commit { at, parent, title, body: _, identity: _ } => {
            assert_cloned(disk, &at);
            assert!(!title.is_empty(), "a commit has a title");
            match forge.commit(disk, &path(&at), fake(parent)) {
                Some(committed) => Done::Committed { commit: commit(committed) },
                None => Done::Unchanged,
            }
        }
        Op::Push { at, commit, branch, identity: _ } => {
            assert_cloned(disk, &at);
            match forge.push(&at.repository, fake(commit), &branch) {
                Ok(Pushed::Pushed) => Done::Succeeded,
                Ok(Pushed::Rejected) => Done::Rejected,
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
