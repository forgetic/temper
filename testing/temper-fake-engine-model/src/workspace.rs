//! The workspaces the fake assigns, drawn from its random state and its
//! configuration.
//!
//! - Each configured workstream holds `spread_min` to `spread_max` of the
//!   configured repositories (as many as there are, at most), consecutive in
//!   the configured list from a place drawn at random, each writable with the
//!   configured chance. Every item of the workstream works in those, so a
//!   worker's checkout of the workstream serves them all.
//! - An item works in a workstream drawn at random. Each of its repositories
//!   starts from a commit ([`COMMIT`]), from the workstream's branch
//!   (`temper/<key>`), or from the base branch ([`BASE`]), with the
//!   configured chances; a writable one pushes to the workstream's branch, as
//!   [`IDENTITY`].
//! - An item saves unfinished work with the configured chance, to
//!   `saved/<key>/<n>`, where `n` is its place among the items.
//! - With the configured chance, an item's workspace is beyond what a worker
//!   takes, the same way at every attempt: a repository named twice, a name
//!   that is not one safe path component (empty, `..`, `.Git`, or holding a
//!   `/`), or an empty workstream key.

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{List, Rng, Writer};

use crate::api::{Access, Repository, Start, Workspace};
use crate::model::Config;

/// The commit a repository may start from, which the world's forge may or may
/// not hold.
pub const COMMIT: &[u8] = b"5eed5eed5eed5eed5eed5eed5eed5eed5eed5eed";

/// The base branch a repository may start from.
pub const BASE: &[u8] = b"main";

/// The push identity of every writable repository.
pub const IDENTITY: &[u8] = b"temper";

/// A workstream: its key and its repositories.
#[derive(Debug)]
pub(crate) struct Stream {
    key: Box<[u8]>,
    members: Box<[Member]>,
}

/// A repository of a workstream.
#[derive(Debug)]
struct Member {
    name: Box<[u8]>,
    remote: Box<[u8]>,
    writable: bool,
}

/// The configured workstreams, each with its repositories drawn.
pub(crate) fn streams(rng: &mut Rng, config: &Config) -> Box<[Stream]> {
    let origins = &config.repositories;
    let count = u32::try_from(origins.len()).expect("a configured list fits a u32");
    let mut streams =
        List::with_capacity(u32::try_from(config.workstreams.len()).expect("a configured list fits a u32"));
    for key in &config.workstreams {
        let size = rng.between(u64::from(config.spread_min.min(count)), u64::from(config.spread_max.min(count)));
        let first = rng.below(u64::from(count));
        let mut members = List::with_capacity(u32::try_from(size).expect("drawn below a u32"));
        for offset in 0..size {
            let place = first.saturating_add(offset).checked_rem(u64::from(count)).expect("a repository is configured");
            let origin =
                origins.get(usize::try_from(place).expect("a u32 fits in a usize")).expect("a place in the list");
            let member = Member {
                name: origin.name.clone(),
                remote: origin.remote.clone(),
                writable: rng.chance(config.writable),
            };
            members.push(member).expect("room for the workstream's repositories");
        }
        streams.push(Stream { key: key.clone(), members: members.into_boxed() }).expect("room for every workstream");
    }
    streams.into_boxed()
}

/// The workspace of the item at `place` among the items, its saved-work
/// branch if it saves, and its workstream's place among the configured ones.
pub(crate) fn draw(
    rng: &mut Rng,
    config: &Config,
    streams: &[Stream],
    place: u32,
) -> (Workspace, Option<Box<[u8]>>, u32) {
    let count = u64::try_from(streams.len()).expect("a configured list fits a u64");
    let drawn = rng.below(count);
    let stream = streams.get(usize::try_from(drawn).expect("drawn below a usize")).expect("a workstream is configured");
    let branch = joined(&[b"temper/", &stream.key]);
    let room = u32::try_from(stream.members.len()).expect("a configured list fits a u32").saturating_add(1);
    let mut repositories = List::with_capacity(room);
    for member in &stream.members {
        let start = if rng.chance(config.commits) {
            Start::Commit { commit: copy_of(COMMIT) }
        } else if rng.chance(config.branches) {
            Start::Branch { branch: branch.clone() }
        } else {
            Start::Base { branch: copy_of(BASE) }
        };
        let access = if member.writable {
            Access::Writable { push: branch.clone(), identity: copy_of(IDENTITY) }
        } else {
            Access::ReadOnly
        };
        let repository = Repository { name: member.name.clone(), remote: member.remote.clone(), start, access };
        repositories.push(repository).expect("room for the workstream's repositories");
    }
    let save =
        if rng.chance(config.saves) { Some(joined(&[b"saved/", &stream.key, b"/", &decimal(place)])) } else { None };
    let mut key = stream.key.clone();
    if rng.chance(config.invalid) {
        breach(rng, &mut key, &mut repositories);
    }
    let drawn = u32::try_from(drawn).expect("a configured list fits a u32");
    (Workspace { key, repositories: repositories.into_boxed() }, save, drawn)
}

/// Breaks one of the rules a workspace must keep, drawn at random.
fn breach(rng: &mut Rng, key: &mut Box<[u8]>, repositories: &mut List<Repository>) {
    let first = repositories.get_mut(0).expect("a workspace has a repository");
    match rng.below(6) {
        0 => *key = Box::new([]),
        1 => first.name = Box::new([]),
        2 => first.name = copy_of(b".."),
        3 => first.name = copy_of(b".Git"),
        4 => first.name = joined(&[&first.name, b"/", &first.name]),
        _ => {
            let twice = first.clone();
            repositories.push(twice).expect("room for one repository more");
        }
    }
}

/// `parts`, one after another.
fn joined(parts: &[&[u8]]) -> Box<[u8]> {
    let mut len: usize = 0;
    for part in parts {
        len = len.checked_add(part.len()).expect("a name's length fits a usize");
    }
    let mut writer = Writer::new(len);
    for part in parts {
        writer.put(part).expect("the writer is as long as the parts");
    }
    writer.finish()
}

/// `number` in decimal.
fn decimal(number: u32) -> Box<[u8]> {
    let mut digits = [b'0'; 10];
    let mut rest = number;
    let mut first = digits.len().saturating_sub(1);
    for (place, digit) in digits.iter_mut().enumerate().rev() {
        *digit = b'0'.saturating_add(u8::try_from(rest.checked_rem(10).expect("ten is not zero")).expect("a digit"));
        rest = rest.checked_div(10).expect("ten is not zero");
        if *digit != b'0' {
            first = place;
        }
    }
    copy_of(digits.get(first..).expect("a place among the digits"))
}
