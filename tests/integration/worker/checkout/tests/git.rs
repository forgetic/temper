//! io's git against the fake forge: working trees on the fake disk, and
//! their remotes reached through the world's route to the forge, which keeps
//! git's rules and fails as the world scripts it.

use temper_checkout_fake::Checkout;
use temper_checkout_fake::git::{self, Created, Fault, NotFetched, Pushed, Remote, Tree, Want, What};
use temper_worker_domain_checkout_tests::forge::{Forge, Move};

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
    tree.retain(|path, _| !temper_checkout_fake::in_git(path));
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
