//! The observed forge follows branch changes across a pull request's close.

use temper_fake_forge_domain::api::Kind;
use temper_fake_forge_domain::{Branches, Observation};
use temper_legacy_engine_domain_world::mirror::Mirror;

#[test]
fn reopening_a_pull_request_follows_the_branch_that_moved_while_it_was_closed() {
    let mut mirror = Mirror::default();
    let repository = b"acme/one".as_slice();
    let head = b"change".as_slice();
    mirror.observe(&Observation::Opened {
        repository: repository.into(),
        number: 1,
        kind: Kind::Pull,
        title: b"change".as_slice().into(),
        body: Box::new([]),
        labels: Box::new([]),
        branches: Some(Branches { head: head.into(), base: b"main".as_slice().into(), commit: 1 }),
        by: 1,
    });
    mirror.observe(&Observation::Closed { repository: repository.into(), number: 1, by: 1 });
    mirror.observe(&Observation::Moved {
        repository: repository.into(),
        branch: head.into(),
        from: Some(1),
        to: 2,
        by: 1,
    });
    assert_eq!(
        mirror.issue(repository, 1).expect("the observed pull request").pull.as_ref().expect("a pull request").commit,
        1,
        "a closed pull keeps its old head"
    );
    mirror.observe(&Observation::Reopened { repository: repository.into(), number: 1, by: 1 });
    let pull = mirror.issue(repository, 1).expect("the observed pull request");
    assert!(pull.open);
    assert_eq!(pull.pull.as_ref().expect("a pull request").commit, 2, "a reopened pull follows its current branch");
}
