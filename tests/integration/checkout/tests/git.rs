//! The fake forge keeps git's rules.

use temper_checkout_fake::Checkout;
use temper_checkout_fake::git::{self, Created, Fault, Forge, Move, NotFetched, Pushed, Tree, Want, What};

fn tree(files: &[(&[u8], &[u8])]) -> Tree {
    files.iter().map(|(path, content)| (path.to_vec(), content.to_vec())).collect()
}

/// A forge with `temper` at a first commit of `files`, cloned and checked
/// out at `w/temper`.
fn cloned(files: &[(&[u8], &[u8])]) -> (Forge, Checkout, u64) {
    let mut forge = Forge::new();
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
    assert_eq!(forge.object(second).tree, tree(&[(b"README", b"hello, world")]));
    assert_eq!(forge.object(second).parent, Some(first));
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
        &[
            Move { remote: b"forge/temper".to_vec(), branch: b"main".to_vec(), from: Some(first), to: theirs },
            Move { remote: b"forge/temper".to_vec(), branch: b"feature".to_vec(), from: None, to: mine },
            Move { remote: b"forge/temper".to_vec(), branch: b"feature".to_vec(), from: Some(mine), to: next },
        ]
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
    assert_eq!(forge.branch(b"forge/temper", b"base"), Some(moved), "left where it is");
}

#[test]
fn the_forge_fails_as_scripted() {
    let (mut forge, mut checkout, first) = cloned(&[]);
    let clone = git::clone_repository(&mut forge, &mut checkout, b"forge/other", b"w/other");
    assert_eq!(clone, Err(Fault::Missing(What::Repository)));
    forge.set_reachable(b"forge/temper", false);
    let fetch = git::fetch(&mut forge, &mut checkout, b"forge/temper", b"w/temper", Want::Default);
    assert_eq!(fetch, Err(Fault::Unreachable));
    forge.set_reachable(b"forge/temper", true);
    forge.set_refusing(b"forge/temper", true);
    assert_eq!(git::push(&mut forge, &checkout, b"forge/temper", b"w/temper", first, b"x"), Err(Fault::Refused));
    assert_eq!(git::create(&mut forge, b"forge/temper", b"x", first), Err(Fault::Refused));
    let fetch = git::fetch(&mut forge, &mut checkout, b"forge/temper", b"w/temper", Want::Default);
    assert_eq!(fetch, Ok(first), "fetching is not refused");
}
