use super::*;

fn prepared(root: &Path) -> Vec<FileUpdate> {
    for name in ["one.txt", "two.txt"] {
        std::fs::write(root.join(name), "old").unwrap();
    }
    let mut files = load(root, ["one.txt", "two.txt"]).unwrap();
    for file in &mut files {
        file.stage(b"new").unwrap();
    }
    files
}

#[test]
fn edit_files_global_preimage_change_prevents_every_replacement() {
    let root = tempfile::tempdir().unwrap();
    let files = prepared(root.path());
    std::fs::write(root.path().join("two.txt"), "external").unwrap();
    assert!(
        commit(root.path(), files, "edited")
            .unwrap_err()
            .contains("target changed")
    );
    assert_eq!(std::fs::read(root.path().join("one.txt")).unwrap(), b"old");
    assert_eq!(
        std::fs::read(root.path().join("two.txt")).unwrap(),
        b"external"
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
}

#[test]
fn edit_files_late_preimage_change_reports_the_committed_prefix() {
    let root = tempfile::tempdir().unwrap();
    let files = prepared(root.path());
    let error = commit_checked(root.path(), files, "edited", |changed, path| {
        if changed == 1 {
            std::fs::write(root.path().join(path), "external").unwrap();
        }
    })
    .unwrap_err();
    assert!(error.contains("after 1 edited files"), "{error}");
    assert_eq!(std::fs::read(root.path().join("one.txt")).unwrap(), b"new");
    assert_eq!(
        std::fs::read(root.path().join("two.txt")).unwrap(),
        b"external"
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
}

#[cfg(unix)]
#[test]
fn edit_files_late_permission_change_rechecks_metadata_before_persist() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    for name in ["one.txt", "two.txt"] {
        std::fs::write(root.path().join(name), "old").unwrap();
        std::fs::set_permissions(root.path().join(name), Permissions::from_mode(0o640)).unwrap();
    }
    let mut files = load(root.path(), ["one.txt", "two.txt"]).unwrap();
    for file in &mut files {
        file.stage(b"new").unwrap();
    }
    let error = commit_checked(root.path(), files, "edited", |changed, path| {
        if changed == 1 {
            std::fs::set_permissions(root.path().join(path), Permissions::from_mode(0o600))
                .unwrap();
        }
    })
    .unwrap_err();
    assert!(error.contains("after 1 edited files"), "{error}");
    assert_eq!(std::fs::read(root.path().join("one.txt")).unwrap(), b"new");
    assert_eq!(
        std::fs::metadata(root.path().join("one.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
    assert_eq!(std::fs::read(root.path().join("two.txt")).unwrap(), b"old");
    assert_eq!(
        std::fs::metadata(root.path().join("two.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[cfg(unix)]
#[test]
fn edit_files_late_parent_symlink_swap_cannot_redirect_the_second_replacement() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("inside")).unwrap();
    std::fs::write(root.path().join("one.txt"), "old").unwrap();
    std::fs::write(root.path().join("inside/two.txt"), "old").unwrap();
    std::fs::write(outside.path().join("two.txt"), "foreign").unwrap();
    let mut files = load(root.path(), ["one.txt", "inside/two.txt"]).unwrap();
    for file in &mut files {
        file.stage(b"new").unwrap();
    }
    let error = commit_checked(root.path(), files, "edited", |changed, _| {
        if changed == 1 {
            std::fs::rename(root.path().join("inside"), root.path().join("retained")).unwrap();
            std::os::unix::fs::symlink(outside.path(), root.path().join("inside")).unwrap();
        }
    })
    .unwrap_err();
    assert!(error.contains("after 1 edited files"), "{error}");
    assert_eq!(std::fs::read(root.path().join("one.txt")).unwrap(), b"new");
    assert_eq!(
        std::fs::read(root.path().join("retained/two.txt")).unwrap(),
        b"old"
    );
    assert_eq!(
        std::fs::read(outside.path().join("two.txt")).unwrap(),
        b"foreign"
    );
}
