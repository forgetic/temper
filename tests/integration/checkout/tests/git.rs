//! The fake forge keeps git's rules.

use temper_checkout_fake::Checkout;
use temper_checkout_fake::git::{Created, Fault, Forge, Move, Pushed, Tree, Want, What};

fn tree(files: &[(&[u8], &[u8])]) -> Tree {
    files.iter().map(|(path, content)| (path.to_vec(), content.to_vec())).collect()
}

#[test]
fn a_check_out_replaces_the_tree_and_leaves_the_git_directory() {
    let mut forge = Forge::new();
    let first = forge.repository(b"temper", b"main", tree(&[(b"README", b"hello"), (b"src/lib.rs", b"fn f() {}")]));
    let mut checkout = Checkout::new();
    forge.clone_repository(&mut checkout, b"temper", b"ws/1/temper").expect("reachable");
    checkout.write(b"ws/1/temper/stray", b"left over");
    checkout.write(b"ws/1/temper/.git/HEAD", b"ref");
    forge.check_out(&mut checkout, b"ws/1/temper", first);
    let expected = tree(&[(b".git/HEAD", b"ref"), (b"README", b"hello"), (b"src/lib.rs", b"fn f() {}")]);
    assert_eq!(checkout.tree(b"ws/1/temper"), expected);
}

#[test]
fn a_commit_snapshots_the_tree_less_the_git_directory_and_only_if_it_changed() {
    let mut forge = Forge::new();
    let first = forge.repository(b"temper", b"main", tree(&[(b"README", b"hello")]));
    let mut checkout = Checkout::new();
    forge.clone_repository(&mut checkout, b"temper", b"w/temper").expect("reachable");
    forge.check_out(&mut checkout, b"w/temper", first);
    checkout.write(b"w/temper/.git/index", b"binary");
    assert_eq!(forge.commit(&checkout, b"w/temper", first), None, "unchanged");
    checkout.write(b"w/temper/README", b"hello, world");
    let second = forge.commit(&checkout, b"w/temper", first).expect("changed");
    assert_eq!(forge.object(second).tree, tree(&[(b"README", b"hello, world")]));
    assert_eq!(forge.object(second).parent, Some(first));
}

#[test]
fn a_push_is_a_fast_forward_or_rejected() {
    let mut forge = Forge::new();
    let first = forge.repository(b"temper", b"main", tree(&[(b"README", b"hello")]));
    let mut checkout = Checkout::new();
    forge.clone_repository(&mut checkout, b"temper", b"w/temper").expect("reachable");
    forge.check_out(&mut checkout, b"w/temper", first);
    checkout.write(b"w/temper/README", b"mine");
    let mine = forge.commit(&checkout, b"w/temper", first).expect("changed");
    assert_eq!(forge.fetch(b"temper", Want::Commit(mine)), Err(Fault::Missing(What::Commit)), "not pushed yet");
    let theirs = forge.advance(b"temper", b"main", b"README", b"theirs");
    assert_eq!(forge.push(b"temper", mine, b"main"), Ok(Pushed::Rejected));
    assert_eq!(forge.branch(b"temper", b"main"), Some(theirs));
    assert_eq!(forge.push(b"temper", mine, b"feature"), Ok(Pushed::Pushed), "a new branch is created");
    assert_eq!(forge.fetch(b"temper", Want::Commit(mine)), Ok(mine));
    checkout.write(b"w/temper/README", b"mine, again");
    let next = forge.commit(&checkout, b"w/temper", mine).expect("changed");
    assert_eq!(forge.push(b"temper", next, b"feature"), Ok(Pushed::Pushed), "a fast-forward");
    assert_eq!(forge.push(b"temper", next, b"feature"), Ok(Pushed::Pushed), "up to date");
    assert_eq!(
        forge.moves(),
        &[
            Move { repository: b"temper".to_vec(), branch: b"main".to_vec(), from: Some(first), to: theirs },
            Move { repository: b"temper".to_vec(), branch: b"feature".to_vec(), from: None, to: mine },
            Move { repository: b"temper".to_vec(), branch: b"feature".to_vec(), from: Some(mine), to: next },
        ]
    );
}

#[test]
fn a_branch_is_created_only_where_there_is_none() {
    let mut forge = Forge::new();
    let first = forge.repository(b"temper", b"main", tree(&[]));
    assert_eq!(forge.fetch(b"temper", Want::Branch(b"base")), Err(Fault::Missing(What::Branch)));
    assert_eq!(forge.fetch(b"temper", Want::Default), Ok(first));
    assert_eq!(forge.create(b"temper", b"base", first), Ok(Created::Created));
    let moved = forge.advance(b"temper", b"base", b"x", b"y");
    assert_eq!(forge.create(b"temper", b"base", first), Ok(Created::Exists));
    assert_eq!(forge.branch(b"temper", b"base"), Some(moved), "left where it is");
}

#[test]
fn the_forge_fails_as_scripted() {
    let mut forge = Forge::new();
    let first = forge.repository(b"temper", b"main", tree(&[]));
    let mut checkout = Checkout::new();
    let clone = forge.clone_repository(&mut checkout, b"other", b"w/other");
    assert_eq!(clone, Err(Fault::Missing(What::Repository)));
    forge.set_reachable(b"temper", false);
    assert_eq!(forge.fetch(b"temper", Want::Default), Err(Fault::Unreachable));
    forge.set_reachable(b"temper", true);
    forge.set_refusing(b"temper", true);
    assert_eq!(forge.push(b"temper", first, b"x"), Err(Fault::Refused));
    assert_eq!(forge.create(b"temper", b"x", first), Err(Fault::Refused));
    assert_eq!(forge.fetch(b"temper", Want::Default), Ok(first), "fetching is not refused");
}
