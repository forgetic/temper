//! io's git against the fake forge: working trees on the fake disk, and
//! their remotes reached through the world's route to the forge, which keeps
//! git's rules and fails as the world scripts it.

use temper_fake_checkout::Checkout;
use temper_fake_checkout::git::{self, Created, Fault, NotFetched, Pushed, Remote, Tree, Want, What};
use temper_worker_checkout_world::forge::{Forge, Move};

fn tree(files: &[(&[u8], &[u8])]) -> Tree {
    files.iter().map(|(path, content)| (path.to_vec(), content.to_vec())).collect()
}

/// A forge with `temper` at a first commit of `files`, cloned and checked
/// out at `w/temper`.
fn cloned(files: &[(&[u8], &[u8])]) -> (Forge, Checkout, u64) {
    let mut forge = Forge::new(1);
    let first = forge.repository(b"forge/temper", b"main", tree(files));
    let mut checkout = Checkout::new();
    git::clone_repository(&mut forge, &mut checkout, b"forge/temper", b"w/temper").expect("reachable");
    git::check_out(&forge, &mut checkout, b"w/temper", first).expect("a clone has its forge's commits");
    (forge, checkout, first)
}

/// The files of the working tree at `w/temper`, less its git directories.
fn files(checkout: &Checkout) -> Tree {
    let mut tree = checkout.tree(b"w/temper");
    tree.retain(|path, _| !temper_fake_checkout::in_git(path));
    tree
}

fn moved(branch: &[u8], from: Option<u64>, to: u64) -> Move {
    Move { remote: b"forge/temper".to_vec(), branch: branch.to_vec(), from, to }
}

#[test]
fn a_check_out_replaces_the_tree_and_leaves_the_git_directories() {
    let (forge, mut checkout, first) = cloned(&[(b"README", b"hello"), (b"src/lib.rs", b"fn f() {}")]);
    checkout.write(b"w/temper/stray", b"left over");
    checkout.write(b"w/temper/.git/HEAD", b"ref");
    checkout.write(b"w/temper/vendor/lib/.git/HEAD", b"nested");
    checkout.write(b"w/temper/vendor/lib/code", b"stray too");
    git::check_out(&forge, &mut checkout, b"w/temper", first).expect("the clone has it");
    assert_eq!(files(&checkout), tree(&[(b"README", b"hello"), (b"src/lib.rs", b"fn f() {}")]));
    assert_eq!(checkout.content(b"w/temper/.git/HEAD"), Some(&b"ref"[..]));
    assert_eq!(checkout.content(b"w/temper/vendor/lib/.git/HEAD"), Some(&b"nested"[..]), "a nested one stays too");
    assert!(checkout.exists(b"w/temper/vendor/lib"), "with the directories on the way to it");
}

#[test]
fn a_commit_snapshots_the_tree_less_the_git_directories_and_only_if_it_changed() {
    let (mut forge, mut checkout, first) = cloned(&[(b"README", b"hello")]);
    checkout.write(b"w/temper/.git/index", b"binary");
    checkout.write(b"w/temper/sub/.git/index", b"binary");
    assert_eq!(git::commit(&mut forge, &mut checkout, b"w/temper", first), Ok(None), "unchanged");
    checkout.write(b"w/temper/README", b"hello, world");
    let second =
        git::commit(&mut forge, &mut checkout, b"w/temper", first).expect("it has its parent").expect("changed");
    assert_eq!(forge.tree(second), tree(&[(b"README", b"hello, world")]));
    assert_eq!(forge.parent(second), Some(first));
    assert!(forge.moves().is_empty(), "a commit moves no branch");
}

#[test]
fn checking_out_or_committing_on_a_commit_never_fetched_fails() {
    let (mut forge, mut checkout, first) = cloned(&[(b"README", b"hello")]);
    let theirs = forge.advance(b"forge/temper", b"main", b"README", b"theirs");
    assert_eq!(git::check_out(&forge, &mut checkout, b"w/temper", theirs), Err(NotFetched));
    assert_eq!(git::commit(&mut forge, &mut checkout, b"w/temper", theirs), Err(NotFetched));
    assert_eq!(git::fetch(&mut forge, &mut checkout, b"forge/temper", b"w/temper", Want::Branch(b"main")), Ok(theirs));
    git::check_out(&forge, &mut checkout, b"w/temper", theirs).expect("fetched");
    assert_eq!(files(&checkout), tree(&[(b"README", b"theirs")]));
    git::check_out(&forge, &mut checkout, b"w/temper", first).expect("still there");
    // A workspace made again has nothing.
    checkout.remove(b"w");
    git::clone_repository(&mut forge, &mut checkout, b"forge/temper", b"w/temper").expect("reachable");
    git::check_out(&forge, &mut checkout, b"w/temper", theirs).expect("cloned with it");
}

#[test]
fn a_clone_has_every_commit_the_branches_reach_and_a_fetch_what_its_tree_lacks() {
    let (mut forge, mut checkout, first) = cloned(&[(b"README", b"hello")]);
    let second = forge.advance(b"forge/temper", b"main", b"README", b"second");
    assert_eq!(forge.create_branch(b"forge/temper", b"side", first), Ok(Created::Created));
    let side = forge.advance(b"forge/temper", b"side", b"README", b"side");
    let third = forge.advance(b"forge/temper", b"main", b"README", b"third");
    let objects =
        |checkout: &Checkout| -> Vec<Vec<u8>> { checkout.tree(b"w/temper/.git/objects").into_keys().collect() };
    assert_eq!(git::fetch(&mut forge, &mut checkout, b"forge/temper", b"w/temper", Want::Default), Ok(third));
    assert_eq!(objects(&checkout), [first, second, third].map(|commit| commit.to_string().into_bytes()));
    let version = checkout.version(b"w/temper/.git/objects/1");
    git::fetch(&mut forge, &mut checkout, b"forge/temper", b"w/temper", Want::Commit(side)).expect("it has it");
    assert_eq!(checkout.version(b"w/temper/.git/objects/1"), version, "what the tree has is not fetched again");
    checkout.remove(b"w");
    git::clone_repository(&mut forge, &mut checkout, b"forge/temper", b"w/temper").expect("reachable");
    let mut every = [first, second, side, third].map(|commit| commit.to_string().into_bytes());
    every.sort();
    assert_eq!(objects(&checkout), every);
}

#[test]
fn a_push_is_a_fast_forward_or_rejected() {
    let (mut forge, mut checkout, first) = cloned(&[(b"README", b"hello")]);
    checkout.write(b"w/temper/README", b"mine");
    let mine = git::commit(&mut forge, &mut checkout, b"w/temper", first).expect("it has its parent").expect("changed");
    let fetch = git::fetch(&mut forge, &mut checkout, b"forge/temper", b"w/temper", Want::Commit(mine));
    assert_eq!(fetch, Err(Fault::Missing(What::Commit)), "not pushed yet");
    let theirs = forge.advance(b"forge/temper", b"main", b"README", b"theirs");
    let push = |forge: &mut Forge, checkout: &Checkout, commit, branch: &[u8]| {
        git::push(forge, checkout, b"forge/temper", b"w/temper", commit, branch)
    };
    assert_eq!(push(&mut forge, &checkout, mine, b"main"), Ok(Pushed::Rejected));
    assert_eq!(forge.branch(b"forge/temper", b"main"), Some(theirs));
    assert_eq!(push(&mut forge, &checkout, mine, b"feature"), Ok(Pushed::Pushed), "a new branch is created");
    let fetch = git::fetch(&mut forge, &mut checkout, b"forge/temper", b"w/temper", Want::Commit(mine));
    assert_eq!(fetch, Ok(mine));
    checkout.write(b"w/temper/README", b"mine, again");
    let next = git::commit(&mut forge, &mut checkout, b"w/temper", mine).expect("it has its parent").expect("changed");
    assert_eq!(push(&mut forge, &checkout, next, b"feature"), Ok(Pushed::Pushed), "a fast-forward");
    assert_eq!(push(&mut forge, &checkout, next, b"feature"), Ok(Pushed::Pushed), "up to date");
    assert_eq!(
        forge.moves(),
        [moved(b"main", Some(first), theirs), moved(b"feature", None, mine), moved(b"feature", Some(mine), next)]
    );
}

#[test]
fn a_branch_is_created_only_where_there_is_none() {
    let (mut forge, mut checkout, first) = cloned(&[]);
    let fetch = git::fetch(&mut forge, &mut checkout, b"forge/temper", b"w/temper", Want::Branch(b"base"));
    assert_eq!(fetch, Err(Fault::Missing(What::Branch)));
    assert_eq!(git::fetch(&mut forge, &mut checkout, b"forge/temper", b"w/temper", Want::Default), Ok(first));
    assert_eq!(git::create(&mut forge, b"forge/temper", b"base", first), Ok(Created::Created));
    let moved = forge.advance(b"forge/temper", b"base", b"x", b"y");
    assert_eq!(git::create(&mut forge, b"forge/temper", b"base", first), Ok(Created::Exists));
    assert_eq!(forge.create_branch(b"forge/temper", b"base", first), Ok(Created::Exists), "another party's too");
    assert_eq!(forge.branch(b"forge/temper", b"base"), Some(moved), "left where it is");
}

#[test]
fn the_forge_fails_as_scripted() {
    let (mut forge, mut checkout, first) = cloned(&[]);
    let clone = git::clone_repository(&mut forge, &mut checkout, b"forge/other", b"w/other");
    assert_eq!(clone, Err(Fault::Missing(What::Repository)));
    assert!(!checkout.exists(b"w/other"), "a clone that failed leaves nothing");
    assert_eq!(forge.branch(b"forge/other", b"main"), None);
    forge.set_reachable(b"forge/temper", false);
    let fetch = git::fetch(&mut forge, &mut checkout, b"forge/temper", b"w/temper", Want::Default);
    assert_eq!(fetch, Err(Fault::Unreachable));
    forge.set_reachable(b"forge/temper", true);
    forge.set_refusing(b"forge/temper", true);
    assert_eq!(git::push(&mut forge, &checkout, b"forge/temper", b"w/temper", first, b"x"), Err(Fault::Refused));
    assert_eq!(git::create(&mut forge, b"forge/temper", b"x", first), Err(Fault::Refused));
    assert_eq!(forge.create_branch(b"forge/temper", b"x", first), Err(Fault::Refused), "another party's too");
    let fetch = git::fetch(&mut forge, &mut checkout, b"forge/temper", b"w/temper", Want::Default);
    assert_eq!(fetch, Ok(first), "fetching is not refused");
    assert!(forge.moves().is_empty(), "what failed moved nothing");
}

#[test]
fn a_clean_merge_combines_independent_lines_and_records_two_parents() {
    let (mut forge, mut checkout, first) = cloned(&[(b"code", b"one\ntwo\nthree\n")]);
    checkout.write(b"w/temper/code", b"ONE\ntwo\nthree\n");
    let ours = git::commit(&mut forge, &mut checkout, b"w/temper", first).expect("local parent").expect("changed");
    let theirs = forge.advance(b"forge/temper", b"main", b"code", b"one\ntwo\nTHREE\n");
    git::fetch(&mut forge, &mut checkout, b"forge/temper", b"w/temper", Want::Commit(theirs)).expect("remote parent");
    let merged = git::merge(&forge, &mut checkout, b"w/temper", theirs).expect("fetched");
    assert!(merged.conflicts.is_empty());
    assert_eq!(files(&checkout), tree(&[(b"code", b"ONE\ntwo\nTHREE\n")]));
    let commit = git::commit_merging(&mut forge, &mut checkout, b"w/temper", ours, theirs).expect("clean merge");
    assert_eq!(forge.parent(commit), Some(ours));
    assert_eq!(forge.merge_parent(commit), Some(theirs));
    assert_eq!(
        git::push_expected(&mut forge, &checkout, b"forge/temper", b"w/temper", commit, b"main", theirs),
        Ok(Pushed::Pushed)
    );
    // Clone only the merge tip: both sides of its converging graph must arrive.
    let mut fresh = Checkout::new();
    git::clone_repository(&mut forge, &mut fresh, b"forge/temper", b"new/temper").expect("clone");
    for ancestor in [first, ours, theirs, commit] {
        git::check_out(&forge, &mut fresh, b"new/temper", ancestor).expect("both parents were cloned");
    }
    let mut fetched = Checkout::new();
    fetched.mkdir(b"fetched/temper/.git");
    git::fetch(&mut forge, &mut fetched, b"forge/temper", b"fetched/temper", Want::Commit(commit))
        .expect("fetch merge");
    for ancestor in [first, ours, theirs, commit] {
        git::check_out(&forge, &mut fetched, b"fetched/temper", ancestor).expect("both parents were fetched");
    }
}

#[test]
fn conflicts_preserve_markers_and_refuse_commit_until_resolved_or_deleted() {
    let (mut forge, mut checkout, first) = cloned(&[(b"code", b"old\n")]);
    checkout.write(b"w/temper/code", b"ours\n");
    let ours = git::commit(&mut forge, &mut checkout, b"w/temper", first).expect("parent").expect("changed");
    let theirs = forge.advance(b"forge/temper", b"main", b"code", b"theirs\n");
    git::fetch(&mut forge, &mut checkout, b"forge/temper", b"w/temper", Want::Commit(theirs)).expect("fetch");
    assert_eq!(git::merge(&forge, &mut checkout, b"w/temper", theirs).expect("merge").conflicts, [b"code".to_vec()]);
    assert_eq!(checkout.content(b"w/temper/code"), Some(&b"<<<<<<< ours\nours\n=======\ntheirs\n>>>>>>> theirs\n"[..]));
    assert_eq!(
        git::commit_merging(&mut forge, &mut checkout, b"w/temper", ours, theirs),
        Err(git::CommitFailure::Unresolved { files: vec![b"code".to_vec()] })
    );
    assert_eq!(forge.branch(b"forge/temper", b"main"), Some(theirs));
    checkout.remove(b"w/temper/code");
    let resolved =
        git::commit_merging(&mut forge, &mut checkout, b"w/temper", ours, theirs).expect("deletion resolves");
    assert!(forge.tree(resolved).is_empty());
    assert_eq!(resolved, theirs + 1, "refused commit made no object");
}

#[test]
fn even_an_unchanged_merge_tree_is_committed_and_a_stale_expected_head_refuses() {
    let (mut forge, mut checkout, first) = cloned(&[(b"code", b"old")]);
    let theirs = forge.advance(b"forge/temper", b"main", b"code", b"new");
    git::fetch(&mut forge, &mut checkout, b"forge/temper", b"w/temper", Want::Commit(theirs)).expect("fetch");
    assert!(git::merge(&forge, &mut checkout, b"w/temper", theirs).expect("merge").conflicts.is_empty());
    let merged =
        git::commit_merging(&mut forge, &mut checkout, b"w/temper", first, theirs).expect("unchanged records merge");
    assert_eq!(forge.tree(merged), forge.tree(theirs));
    assert_eq!(forge.parent(merged), Some(first));
    assert_eq!(forge.merge_parent(merged), Some(theirs));
    assert_eq!(
        git::push_expected(&mut forge, &checkout, b"forge/temper", b"w/temper", merged, b"main", first),
        Ok(Pushed::Rejected)
    );
    assert_eq!(forge.branch(b"forge/temper", b"main"), Some(theirs));
    assert_eq!(
        git::push_expected(&mut forge, &checkout, b"forge/temper", b"w/temper", merged, b"main", theirs),
        Ok(Pushed::Pushed)
    );
}

#[test]
fn an_unfetched_merge_parent_refuses_without_touching_the_tree() {
    let (mut forge, mut checkout, first) = cloned(&[(b"code", b"old")]);
    let theirs = forge.advance(b"forge/temper", b"main", b"code", b"new");
    assert_eq!(git::merge(&forge, &mut checkout, b"w/temper", theirs), Err(NotFetched));
    assert_eq!(
        git::commit_merging(&mut forge, &mut checkout, b"w/temper", first, theirs),
        Err(git::CommitFailure::NotFetched)
    );
    assert_eq!(files(&checkout), tree(&[(b"code", b"old")]));
    assert!(!checkout.exists(b"w/temper/.git/MERGE_HEAD"));
}

#[test]
fn merges_preserve_additions_deletions_and_modify_delete_conflicts() {
    let (mut forge, mut checkout, first) = cloned(&[(b"clean", b"old"), (b"conflict", b"old")]);
    checkout.write(b"w/temper/conflict", b"modified");
    checkout.write(b"w/temper/ours", b"added");
    let ours = git::commit(&mut forge, &mut checkout, b"w/temper", first).expect("parent").expect("changed");
    let theirs = forge.store(first, None, tree(&[(b"theirs", b"added too")])).expect("deletions change tree");
    assert_eq!(forge.push(b"forge/temper", b"main", theirs, None), Ok(Pushed::Pushed));
    git::fetch(&mut forge, &mut checkout, b"forge/temper", b"w/temper", Want::Commit(theirs)).expect("fetch");
    let merged = git::merge(&forge, &mut checkout, b"w/temper", theirs).expect("merge");
    assert_eq!(merged.conflicts, [b"conflict".to_vec()]);
    assert!(!checkout.exists(b"w/temper/clean"), "the unchanged file takes the other side's deletion");
    assert_eq!(checkout.content(b"w/temper/ours"), Some(&b"added"[..]));
    assert_eq!(checkout.content(b"w/temper/theirs"), Some(&b"added too"[..]));
    assert_eq!(
        checkout.content(b"w/temper/conflict"),
        Some(&b"<<<<<<< ours\nmodified\n=======\n\n>>>>>>> theirs\n"[..])
    );
    checkout.write(b"w/temper/conflict", b"resolved");
    let committed = git::commit_merging(&mut forge, &mut checkout, b"w/temper", ours, theirs).expect("resolved");
    assert_eq!(
        forge.tree(committed),
        tree(&[(b"conflict", b"resolved"), (b"ours", b"added"), (b"theirs", b"added too")])
    );
}
