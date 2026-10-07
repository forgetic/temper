//! Connector additions checked against the groundwork's observed behavior.
#![expect(clippy::disallowed_methods, reason = "step tests are ordinary Rust (testing-strategy.md, section 4)")]
use super::*;

fn opened(h: &mut Harness, head: &[u8]) -> u64 {
    let Answer::Created(number) = h.open(head).unwrap() else { unreachable!("a created pull") };
    number
}

#[test]
fn clean_update_keeps_two_parents_reports_hooks_and_checks_the_new_head() {
    let mut configured = setup();
    configured.checks.cue = Some(Cue { path: copy_of(b"ci"), green: copy_of(b"green") });
    let mut h = Harness::with(CALM, configured);
    let work = h.commit(FIRST, &[(b"README", b"changed")]);
    h.push(ENGINE, b"work", work).unwrap();
    let number = opened(&mut h, b"work");
    h.settle();
    assert_eq!(h.pull(number).statuses[0].state, Check::Passed);
    let base = crate::advance(&mut h.domain, &h.env, REPOSITORY, MAIN, b"ci", b"red", MAINTAINER).unwrap();
    assert_eq!(h.call(ENGINE, write(Write::Update { number })), Ok(Answer::Done));
    let pull = h.pull(number);
    assert_ne!(pull.commit, work);
    assert_eq!(pull.statuses[0].state, Check::Pending);
    let object = h.domain.object(pull.commit).unwrap();
    assert_eq!(object.parent, Some(work));
    assert_eq!(object.merge_parent, Some(base));
    assert_eq!(&**object.tree.get(&b"README"[..]).unwrap(), b"changed");
    assert_eq!(&**object.tree.get(&b"ci"[..]).unwrap(), b"red");
    h.settle();
    assert_eq!(h.pull(number).statuses[0].state, Check::Failed, "CI belongs to the new head");
    assert!(h.hooks.iter().any(|hook| hook.0 == Change::Push && hook.3 == Some(pull.commit)));
    assert!(h.hooks.iter().any(|hook| hook.0 == Change::Pull && hook.1 == Some(number)));
    let remaining = h.domain.room().commits;
    assert_eq!(h.call(ENGINE, write(Write::Update { number })), Ok(Answer::Done));
    assert_eq!(h.domain.room().commits, remaining, "an already up-to-date update makes no extra commit");
    assert_eq!(h.branch(b"work"), Some(pull.commit));
}

#[test]
fn conflicting_update_changes_neither_head_nor_objects() {
    let mut h = Harness::new(CALM);
    let work = h.commit(FIRST, &[(b"README", b"ours")]);
    h.push(ENGINE, b"work", work).unwrap();
    let number = opened(&mut h, b"work");
    crate::advance(&mut h.domain, &h.env, REPOSITORY, MAIN, b"README", b"theirs", MAINTAINER).unwrap();
    let remaining = h.domain.room().commits;
    assert_eq!(h.call(ENGINE, write(Write::Update { number })), Err(Error::Conflict));
    assert_eq!(h.branch(b"work"), Some(work));
    assert_eq!(h.pull(number).commit, work);
    assert_eq!(h.domain.room().commits, remaining);
    assert_eq!(h.call(PERSON, write(Write::Update { number })), Err(Error::Forbidden));
    h.call(ENGINE, write(Write::Close { number })).unwrap();
    assert_eq!(h.call(ENGINE, write(Write::Update { number })), Err(Error::Closed));
}

#[test]
fn pull_file_pages_are_distinct_and_name_the_exact_head() {
    let mut h = Harness::new(CALM);
    let work = crate::commit(&mut h.domain, &CALM, FIRST, files(&[(b"new", b"one")]), b"").unwrap().unwrap();
    h.push(ENGINE, b"work", work).unwrap();
    let number = opened(&mut h, b"work");
    let mut names = skein_lib::Set::with_capacity(3);
    for page in 1..=4_u32 {
        let Answer::PullFiles { head, files, more } = h.ok(PERSON, read(Read::PullFiles { number, page, limit: 1 }))
        else {
            unreachable!("a file page")
        };
        assert_eq!(head, work);
        if page == 4 {
            assert!(files.is_empty() && !more);
            continue;
        }
        assert_eq!(files.len(), 1);
        assert_eq!(more, page < 3);
        let file = files.first().unwrap();
        assert!(names.insert(file.path.clone()).unwrap());
        if *file.path == *b"new" {
            assert_eq!(file.before, None);
            assert_eq!(file.after.as_deref(), Some(&b"one"[..]));
        } else {
            assert!(file.before.is_some() && file.after.is_none(), "deleted files retain their before contents");
        }
    }
    assert_eq!(names.len(), 3);
}

#[test]
fn comparisons_ignore_paging_and_refuse_incomplete_data_explicitly() {
    let mut h = Harness::new(CALM);
    let one = h.commit(FIRST, &[(b"one", b"one")]);
    let two = h.commit(one, &[(b"two", b"two")]);
    h.push(ENGINE, b"work", two).unwrap();
    let first = h.ok(PERSON, read(Read::Compare { base: FIRST, head: two, page: 1, limit: 1 }));
    assert_eq!(first, h.ok(PERSON, read(Read::Compare { base: FIRST, head: two, page: 4, limit: 1 })));
    let Answer::Comparison { base, head, contains_base: _, files: changed, commits } = first else {
        unreachable!("a comparison")
    };
    assert_eq!((base, head), (FIRST, two));
    assert_eq!(&*commits, &[one, two]);
    assert_eq!(changed.len(), 2);
    let excessive = crate::commit(&mut h.domain, &CALM, FIRST, files(&[(b"a", b"a"), (b"b", b"b"), (b"c", b"c")]), b"")
        .unwrap()
        .unwrap();
    h.push(ENGINE, b"other", excessive).unwrap();
    assert_eq!(
        h.call(PERSON, read(Read::Compare { base: FIRST, head: excessive, page: 1, limit: 1 })),
        Err(Error::TooLarge)
    );
    assert_eq!(
        h.call(PERSON, read(Read::Compare { base: FIRST, head: u64::MAX, page: 1, limit: 1 })),
        Err(Error::Missing(What::Commit))
    );
}

#[test]
fn adoption_metadata_and_protection_permission_match_the_probe() {
    let mut configured = setup();
    let protection = Protection { branch: copy_of(MAIN), contexts: names(&[b"ci"]), approvals: 1, dismiss_stale: true };
    configured.protection = Some(protection.clone());
    let mut h = Harness::with(CALM, configured);
    assert_eq!(h.call(ENGINE, read(Read::Protection { branch: copy_of(MAIN) })), Err(Error::Forbidden));
    assert_eq!(h.call(ENGINE, read(Read::Protection { branch: copy_of(b"absent") })), Err(Error::Forbidden));
    assert_eq!(h.ok(ADMIN, read(Read::Protection { branch: copy_of(MAIN) })), Answer::Protection(Some(protection)));
    assert_eq!(h.ok(ADMIN, read(Read::Protection { branch: copy_of(b"absent") })), Answer::Protection(None));
    let metadata = crate::api::Settings { default: copy_of(MAIN), merge: false, rebase: true, squash: false };
    crate::settings(&mut h.domain, REPOSITORY, metadata.clone());
    assert_eq!(h.ok(PERSON, read(Read::Settings)), Answer::Settings(metadata));
    let Answer::Collaborators(users) = h.ok(PERSON, read(Read::Collaborators)) else { unreachable!("collaborators") };
    assert_eq!(users.len(), 5);
    assert!(users.iter().any(|user| user.user == ENGINE && user.permission == Permission::Write));
    assert!(users.iter().any(|user| user.user == ADMIN && user.permission == Permission::Admin));
    assert_eq!(
        h.call(PERSON, write(Write::CreateBranch { branch: copy_of(b"new"), commit: FIRST })),
        Err(Error::Forbidden)
    );
    assert_eq!(
        h.ok(ENGINE, write(Write::CreateBranch { branch: copy_of(b"new"), commit: FIRST })),
        Answer::Branch(Created::Created)
    );
    assert_eq!(h.branch(b"new"), Some(FIRST));
    assert_eq!(
        h.ok(ENGINE, write(Write::CreateBranch { branch: copy_of(b"new"), commit: FIRST })),
        Answer::Branch(Created::Exists)
    );
    assert_eq!(
        h.call(ENGINE, write(Write::CreateBranch { branch: copy_of(MAIN), commit: FIRST })),
        Err(Error::Protected)
    );
}

#[test]
fn a_manually_reported_failure_has_no_ci_job() {
    let mut h = Harness::new(CALM);
    h.call(ENGINE, status(FIRST, b"ci", Check::Failed)).unwrap();
    let Answer::Checks(checks) = h.ok(PERSON, read(Read::Checks { commit: FIRST })) else { unreachable!("checks") };
    assert_eq!(checks.len(), 1);
    let check = checks.first().unwrap();
    assert_eq!(check.status.state, Check::Failed);
    assert_eq!(&*check.description, b"CI failed");
    assert_eq!(&*check.link, b"/job/1");
    assert_eq!(check.job, None);
    assert_eq!(
        h.call(PERSON, read(Read::Job { commit: FIRST, run: FIRST, job: 0, attempt: 1, max_bytes: 64 })),
        Err(Error::Missing(What::Job))
    );
}

#[test]
fn a_failed_ci_job_has_a_bounded_log_for_its_current_attempt() {
    let mut setup = setup();
    setup.checks.passes = 0;
    let mut h = Harness::with(CALM, setup);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).unwrap();
    opened(&mut h, b"work");
    h.settle();
    let Answer::Checks(checks) = h.ok(PERSON, read(Read::Checks { commit: work })) else { unreachable!("checks") };
    let job = checks.first().unwrap().job.expect("CI job");
    assert_eq!(job.run, work);
    let full = h
        .ok(PERSON, read(Read::Job { commit: work, run: job.run, job: job.job, attempt: job.attempt, max_bytes: 256 }));
    let Answer::File(log) = full else { unreachable!("full log") };
    assert!(log.len() > 8);
    let bounded =
        h.ok(PERSON, read(Read::Job { commit: work, run: job.run, job: job.job, attempt: job.attempt, max_bytes: 8 }));
    let Answer::File(bounded) = bounded else { unreachable!("bounded log") };
    assert_eq!(bounded.len(), 9, "one extra byte signals truncation to the adapter");
    assert_eq!(&*bounded, log.get(log.len() - 9..).unwrap());
    assert_eq!(
        h.call(PERSON, read(Read::Job { commit: work, run: job.run, job: job.job, attempt: 2, max_bytes: 8 })),
        Err(Error::Missing(What::Job))
    );
}

#[test]
fn oversized_api_branch_creation_is_refused_before_a_delayed_write_can_hold_it() {
    let config = Config { landing: 1000, ..CALM };
    let mut h = Harness::new(config);
    let name = skein_lib::bytes::zeroed(usize::try_from(config.limits.name_bytes.checked_add(1).unwrap()).unwrap());
    assert_eq!(h.call(ENGINE, write(Write::CreateBranch { branch: name, commit: FIRST })), Err(Error::TooLarge));
    assert_eq!(h.domain.tally().landed, 0);
}

#[test]
fn api_branch_creation_does_not_depend_on_git_transport_reachability() {
    let mut h = Harness::new(CALM);
    crate::set_reachable(&mut h.domain, REPOSITORY, false);
    assert_eq!(h.call(ENGINE, Op::Git(Git::Clone)), Err(Error::Unreachable));
    assert_eq!(
        h.call(ENGINE, write(Write::CreateBranch { branch: copy_of(b"rest"), commit: FIRST })),
        Ok(Answer::Branch(Created::Created))
    );
    assert_eq!(h.branch(b"rest"), Some(FIRST));
}

#[test]
fn oversized_update_repository_is_not_retained_for_a_delayed_write() {
    let config = Config { landing: 1000, ..CALM };
    let mut h = Harness::new(config);
    let name = skein_lib::bytes::zeroed(usize::try_from(config.limits.name_bytes.checked_add(1).unwrap()).unwrap());
    assert_eq!(h.call_on(&name, ENGINE, write(Write::Update { number: 1 })), Err(Error::Missing(What::Repository)));
    assert_eq!(h.domain.tally().landed, 0);
}
