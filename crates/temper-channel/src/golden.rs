#![expect(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "ordinary Rust test-only fixture tooling uses filesystem paths and manifest sets"
)]
//! Shared by the payload and wire fixture tests; never built into the codec.

use alloc::collections::BTreeSet;
use std::path::PathBuf;

pub(super) struct Run {
    regenerate: bool,
    visited: BTreeSet<String>,
}

pub(super) type Fixture = (&'static str, fn(&mut Run));

impl Run {
    pub(super) fn checking() -> Self {
        Self { regenerate: false, visited: BTreeSet::new() }
    }
}

// The manifest names the existing fixture functions, so values have one source.
macro_rules! fixtures {
    ($($fixture:ident,)+) => {
        const FIXTURES: &[golden::Fixture] = &[
            $((concat!(stringify!($fixture), ".bin"), $fixture),)+
        ];

        mod golden_cases {
            use super::golden;

            $(
                #[test]
                fn $fixture() {
                    super::$fixture(&mut golden::Run::checking());
                }
            )+
        }
    };
}

pub(super) use fixtures;

fn directory(schema: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src").join(schema).join("golden")
}

pub(super) fn bytes(schema: &str, filename: &str, run: &mut Run, encoded: &[u8]) -> Box<[u8]> {
    assert!(run.visited.insert(filename.to_owned()), "duplicate fixture: {filename}");
    let path = directory(schema).join(filename);
    if run.regenerate {
        std::fs::write(&path, encoded).expect("write regenerated fixture");
    }
    let checked_in = std::fs::read(&path).expect("read golden fixture; see golden/README.md to regenerate");
    assert_eq!(encoded, checked_in, "codec output drift in {}", path.display());
    checked_in.into_boxed_slice()
}

pub(super) fn check(schema: &str, fixtures: &[Fixture], regenerate: bool) {
    let mut expected = BTreeSet::new();
    for (filename, _) in fixtures {
        assert!(expected.insert((*filename).to_owned()), "duplicate manifest entry: {filename}");
    }
    let mut run = Run { regenerate, visited: BTreeSet::new() };
    for (_, fixture) in fixtures {
        fixture(&mut run);
    }
    assert_eq!(run.visited, expected, "manifest must cover exactly the invoked fixture names");
    let mut files = BTreeSet::new();
    for entry in std::fs::read_dir(directory(schema)).expect("read fixture directory") {
        let path = entry.expect("read fixture entry").path();
        if path.extension().is_some_and(|extension| extension == "bin") {
            files.insert(path.file_name().unwrap().to_str().unwrap().to_owned());
        }
    }
    assert_eq!(files, expected, "binary files must match the fixture manifest; remove stale files explicitly");
}
