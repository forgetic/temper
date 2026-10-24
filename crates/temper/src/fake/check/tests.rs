use super::*;
use crate::directories::tests::scratch;
use std::fs;

fn store_limits() -> jig_protocol_store::Limits {
    let mut limits = jig_protocol_config::profile(jig_protocol_config::Profile::Small).store;
    // Skein's real KV fixture, with chunk >= key + value + 20, and room
    // for the format and the eight rows of the tag-filter case.
    limits.store = skein_kv::Limits {
        key: 64,
        value: 64,
        ops: 8,
        commit: 1024,
        queued: 8,
        queued_bytes: 8192,
        budget: 8192,
        segment: 4096,
        snapshot_after: 4096,
        chunk: 256,
        page: skein_kv::Page { rows: 16, bytes: 1024 },
        deadline: skein_lib::Duration::from_secs(5),
    };
    limits
}
fn rows(keys: &[u8], tag: Option<u8>) -> Rows {
    keys.iter()
        .map(|key| {
            let key = match tag {
                Some(tag) => Box::from([tag, *key]),
                None => Box::from([*key]),
            };
            (key, Box::from(b"value".as_slice()))
        })
        .collect()
}
fn seed(path: &Path, limits: &jig_protocol_store::Limits, format: Option<Format>, rows: Rows) {
    fs::create_dir(path).expect("store directory");
    let mut rows = rows.into_vec();
    if let Some(format) = format {
        rows.push((
            keys::encode_own(&keys::Own::Format, limits.store.key).expect("format key"),
            format.encode().expect("format"),
        ));
    }
    let mut host = StoreHost::new(limits.store);
    host.seed = Some(rows.into_boxed_slice());
    run_store(path, host).expect("seed through the real store");
}
enum Copy {
    Store { format: Option<Format>, rows: Rows },
    Absent,
    File,
}
#[test]
fn store() {
    // The row-count page bound forces three pages for five temper rows.
    const SMALL_PAGE: u32 = 2;
    let root = scratch("fake-check-store");
    let current = format::current(temper_protocol_store::STORE_VERSION, [0; 16]);
    let mut application = current;
    application.application += 1;
    let mut component = current;
    component.component += 1;
    let temper = &[9, 10, 12];
    let many = &[9, 10, 11, 12, 16];
    let mixed = [(0, 9), (1, 10), (2, 9), (2, 10), (2, 12), (3, 12)]
        .into_iter()
        .map(|(tag, key)| (Box::from([tag, key]), Box::from(b"value".as_slice())))
        .collect();
    let limits = store_limits();
    let page_rows = limits.store.page.rows;
    for (name, copy, page, expected) in [
        (
            "good",
            Copy::Store { format: Some(current), rows: rows(temper, Some(keys::APPLICATION)) },
            page_rows,
            Ok(rows(temper, None)),
        ),
        ("absent", Copy::Absent, page_rows, Err(format!("opening {}: errno 2", root.join("absent").display()))),
        ("file", Copy::File, page_rows, Err(format!("opening {}: errno 20", root.join("file").display()))),
        ("empty", Copy::Store { format: None, rows: Box::new([]) }, page_rows, Err("format: Got None".to_owned())),
        (
            "application",
            Copy::Store { format: Some(application), rows: Box::new([]) },
            page_rows,
            Err(format!(
                "format check: Format {{ family: Application, found: {}, wanted: {} }}",
                temper_protocol_store::STORE_VERSION.checked_add(1).expect("application version increment fits u32"),
                temper_protocol_store::STORE_VERSION,
            )),
        ),
        (
            "component",
            Copy::Store { format: Some(component), rows: Box::new([]) },
            page_rows,
            Err(format!(
                "format check: Format {{ family: Component, found: {}, wanted: {} }}",
                component.component, current.component,
            )),
        ),
        ("tags", Copy::Store { format: Some(current), rows: mixed }, page_rows, Ok(rows(temper, None))),
        (
            "pages",
            Copy::Store { format: Some(current), rows: rows(many, Some(keys::APPLICATION)) },
            SMALL_PAGE,
            Ok(rows(many, None)),
        ),
    ] {
        let path = root.join(name);
        let mut limits = limits;
        limits.store.page.rows = page;
        match copy {
            Copy::Store { format, rows } => seed(&path, &limits, format, rows),
            Copy::Absent => {}
            Copy::File => fs::write(&path, b"file").expect("file"),
        }
        assert_eq!(read_store(&path, &limits), expected, "{name}");
    }
    fs::remove_dir_all(root).expect("scratch removed");
}

const START: &[u8] = b"{\"observations_lost\":0}\n";
const LOST: &[u8] = b"{\"observations_lost\":7}\n";
const DELETED: &[u8] =
    b"{\"observation\":\"deleted\",\"repository\":\"org/repo\",\"branch\":\"work\",\"at\":7,\"by\":2}\n";
const MISSING: &str = "observations: no checkpoint";
const REFUSED: &str = "line 1: decode: Refusal { variant: None, member: [111, 98, 115, 101, 114, 118, 97, 116, 105, 111, 110], reason: Json }";
fn encoded(records: Records) -> Result<Vec<Vec<u8>>, String> {
    records
        .map(|record| {
            let mut bytes = skein_lib::List::with_capacity(u32::try_from(DELETED.len()).expect("fixture line length"));
            skein_fake_forge_protocol::observations::encode(&record?, &mut bytes).expect("fixture record fits");
            Ok(bytes.as_slice().to_vec())
        })
        .collect()
}
#[test]
fn observations() {
    let root = scratch("fake-check-observations");
    let bound = usize::try_from(temper_fake::limits::OBSERVATION_BYTES).expect("u32 fits usize");
    // Exactly one byte beyond the line bound (including LF).
    let mut overlong = vec![b'x'; bound];
    overlong.push(b'\n');
    let overlong_error = format!("line 1: line past {bound} bytes");
    let missing_file =
        format!("observations {}: {}", root.join("absent").display(), std::io::Error::from_raw_os_error(2));
    for (name, bytes, expected) in [
        ("start", Some(START.to_vec()), Ok((1, 0, vec![START.to_vec()]))),
        (
            "github",
            Some(
                [
                    START,
                    b"{\"observation\":\"git_call\",\"installation\":1,\"mint\":2,\"writes\":true,\"status\":200}\n",
                    START,
                ]
                .concat(),
            ),
            Ok((3, 0, vec![START.to_vec(), START.to_vec()])),
        ),
        (
            "github-full",
            Some([START, b"{\"observation\":\"git_calls_full\",\"capacity\":128}\n"].concat()),
            Err(GIT_FULL),
        ),
        ("partial", Some([DELETED, START, b"{partial"].concat()), Ok((2, 0, vec![DELETED.to_vec(), START.to_vec()]))),
        ("refused-tail", Some([DELETED, START, b"{\n"].concat()), Ok((2, 0, vec![DELETED.to_vec(), START.to_vec()]))),
        ("refused-prefix", Some([b"{\n", START].concat()), Err(REFUSED)),
        ("no-checkpoint", Some(DELETED.to_vec()), Err(MISSING)),
        ("empty", Some(vec![]), Err(MISSING)),
        ("absent", None, Err(missing_file.as_str())),
        ("overlong-prefix", Some([overlong.as_slice(), START].concat()), Err(overlong_error.as_str())),
        ("overlong-tail", Some([START, overlong.as_slice()].concat()), Ok((1, 0, vec![START.to_vec()]))),
        (
            "checkpoints",
            Some([START, DELETED, LOST, b"partial"].concat()),
            Ok((3, 7, vec![START.to_vec(), DELETED.to_vec(), LOST.to_vec()])),
        ),
        (
            "checkpoint-tail",
            Some([START, DELETED, b"{\"observations_lost\":7}"].concat()),
            Ok((1, 0, vec![START.to_vec()])),
        ),
    ] {
        let path = root.join(name);
        if let Some(bytes) = bytes {
            fs::write(&path, bytes).expect("observations fixture");
        }
        let found = Observations::open(&path).and_then(|observations| {
            Ok((observations.lines(), observations.lost(), encoded(observations.records()?)?))
        });
        assert_eq!(found.as_ref().map_err(String::as_str), expected.as_ref().map_err(|why| *why), "{name}");
    }
    // The second pass detects a changed prefix, without reading beyond its
    // saved boundary. Appends are the running fake's ordinary case.
    for (name, replacement, expected) in [
        (
            "shortened",
            b"{\"observations_lost\":".as_slice(),
            Err("observations: file shorter than checkpoint boundary"),
        ),
        ("changed", b"xxxxxxxxxxxxxxxxxxxxxxx\n", Err("decode:")),
        ("grown", [START, b"ignored\n"].concat().as_slice(), Ok(vec![START.to_vec()])),
    ] {
        let path = root.join(name);
        fs::write(&path, START).expect("initial prefix");
        let observations = Observations::open(&path).expect("boundary");
        fs::write(&path, replacement).expect("changed file");
        match expected {
            Ok(expected) => assert_eq!(encoded(observations.records().expect(name)).expect(name), expected, "{name}"),
            Err(expected) => {
                assert!(encoded(observations.records().expect(name)).expect_err(name).starts_with(expected), "{name}");
            }
        }
    }
    fs::remove_dir_all(root).expect("scratch removed");
}

use skein_forge::api::{Repository, Write as ForgeWrite};
use skein_forge_client::{Condition, Effect, Entry, Writer};
use temper_engine_domain_forge as d;
use temper_protocol_store::{keys as temper_keys, records};

const REPO: Repository = Repository { forge: 1, repository: 2 };
fn repository() -> d::Stored {
    d::Stored::Repository(d::Repository {
        project: 1,
        home: true,
        provider: REPO,
        host: b"forge".as_slice().into(),
        owner: b"playground".as_slice().into(),
        name: b"widget".as_slice().into(),
        prefix: b"temper/".as_slice().into(),
        role: d::Role::Owned,
        ci: true,
        checks: Box::new([]),
        kinds: d::Kinds {
            read: true,
            push: true,
            open: true,
            land: true,
            review: false,
            status: true,
            comment: true,
            issue: true,
            branch: true,
        },
        protection: d::Protection::Absent,
        settings: skein_forge::api::Settings {
            default_branch: b"main".as_slice().into(),
            merge: true,
            squash: true,
            rebase: true,
        },
    })
}
fn entry() -> d::Stored {
    d::Stored::Entry {
        entry: Entry {
            number: 1,
            task: 4,
            repository: REPO,
            effect: Effect {
                write: ForgeWrite::Merge { number: 1, head: skein_fake_forge_protocol::routes::object(2).bytes },
                condition: Condition::None,
            },
            start: None,
            attempt: None,
            failures: 0,
            queued: false,
        },
        held: false,
    }
}
fn deployment(merge: bool) -> Rows {
    let mut rows = vec![d::Stored::Writer(Writer { forge: 1, author: 7 }), repository()];
    if merge {
        rows.push(entry());
    }
    stored_rows(&rows)
}
fn stored_rows(rows: &[d::Stored]) -> Rows {
    rows.iter()
        .map(|row| {
            let key = temper_keys::encode(&d::stored_key(row), u32::MAX).expect("fixture key");
            ([&[keys::APPLICATION], key.as_ref()].concat().into_boxed_slice(), records::encode(row))
        })
        .collect()
}
const BOUNDARY: &[u8] = b"{\"boundary\":{\"lines\":1,\"lost\":0}}\n";
const TWO_LINES: &[u8] = b"{\"boundary\":{\"lines\":2,\"lost\":0}}\n";
const PROFILE: &[u8] = b"{\"profile\":{\"client\":7,\"marker\":[\"<!-- temper:key \",\" -->\\n\"],\"owned_branches\":[\"temper/\"],\"allowed\":[\"comment\",\"merge\",\"close\"],\"bounds\":{\"objects\":256,\"decisions\":0}},\"decisions_complete\":true}\n";
const DECIDED_PROFILE: &[u8] = b"{\"profile\":{\"client\":7,\"marker\":[\"<!-- temper:key \",\" -->\\n\"],\"owned_branches\":[\"temper/\"],\"allowed\":[\"comment\",\"merge\",\"close\"],\"bounds\":{\"objects\":256,\"decisions\":1}},\"decisions_complete\":true}\n";
const DECISION: &[u8] = b"{\"decision\":{\"repository\":\"playground/widget\",\"number\":1,\"head\":2}}\n";
const MERGE: &[u8] = b"{\"observation\":\"merged\",\"repository\":\"playground/widget\",\"number\":1,\"base\":\"main\",\"head\":2,\"commit\":3,\"by\":7}\n";
const ORDER: &[u8] =
    b"{\"fail\":\"promise/order/playground%2Fwidget/1/2\",\"summary\":\"a merge with no decision at its head\"}\n";
const INVALID_DOCUMENT: &[u8] = b"temper fake forge check: engine: missing\ntemper fake forge check: workers: missing\ntemper fake forge check: application: missing\n";
fn expected(status: u8, stdout: &[u8], stderr: &[u8]) -> Outcome {
    Outcome { status, stdout: stdout.into(), stderr: stderr.into() }
}
fn diagnostic(why: &str) -> Vec<u8> {
    format!("temper fake forge check: {why}\n").into_bytes()
}
fn refused(why: &str) -> Outcome {
    expected(1, b"", &diagnostic(why))
}
fn undecided(word: &[u8], stderr: &[u8]) -> Outcome {
    expected(2, &[b"{\"undecided\":\"", word, b"\"}\n"].concat(), stderr)
}
fn missing_observations(why: &str) -> Outcome {
    undecided(b"observations missing", &diagnostic(why))
}
fn unreadable_store(path: &Path, errno: u8) -> Outcome {
    undecided(
        b"store unreadable",
        &[BOUNDARY, &diagnostic(&format!("opening {}: errno {errno}", path.display()))].concat(),
    )
}
#[test]
fn verb() {
    let root = scratch("fake-check-verb");
    let document = crate::tests::configuration::check_document(&root);
    let mut no_entry = document.clone();
    no_entry["application"]["setup"] = serde_json::json!([]);
    let mut managed = crate::tests::configuration::old_form_check_document(&root);
    managed["application"]["setup"][0]["orgs"] = serde_json::json!(["playground"]);
    managed["application"]["setup"][0]["mode"] = serde_json::json!({"managed":{"admin_token":"admin_token"}});
    let mut owned = crate::tests::configuration::old_form_check_document(&root);
    owned["application"]["setup"][0]["mode"] = serde_json::json!({"owned":{
        "forgejo":"/fixture/forgejo", "runner":null, "listen":{"socket":"127.0.0.1:9"},
        "person":{"login":"ada","email":"ada@example.test"}
    }});
    let mut bad_setup = document.clone();
    bad_setup["application"]["setup"][0]["forge"] = 7.into();
    let current = Some(format::current(temper_protocol_store::STORE_VERSION, [0; 16]));
    let good = || Copy::Store { format: current, rows: deployment(false) };
    let doc = || Some(document.clone());
    let checkpoint = || Some(START.to_vec());
    let path = |name: &str, file: &str| root.join(name).join(file);
    for (name, document, observations, copy, expected) in [
        ("pass", doc(), checkpoint(), good(), expected(0, b"", &[BOUNDARY, PROFILE].concat())),
        ("fail", doc(), Some([MERGE, START].concat()), good(), expected(1, ORDER, &[TWO_LINES, PROFILE].concat())),
        (
            "decision",
            doc(),
            Some([MERGE, START].concat()),
            Copy::Store { format: current, rows: deployment(true) },
            expected(0, b"", &[TWO_LINES, DECIDED_PROFILE, DECISION].concat()),
        ),
        (
            "absent-observations",
            doc(),
            None,
            good(),
            missing_observations(&format!(
                "observations {}: {}",
                path("absent-observations", "observations").display(),
                std::io::Error::from_raw_os_error(2)
            )),
        ),
        ("no-checkpoint", doc(), Some(MERGE.to_vec()), good(), missing_observations(MISSING)),
        ("refused-observations", doc(), Some([b"{\n", START].concat()), good(), missing_observations(REFUSED)),
        ("absent-store", doc(), checkpoint(), Copy::Absent, unreadable_store(&path("absent-store", "copy"), 2)),
        ("not-store", doc(), checkpoint(), Copy::File, unreadable_store(&path("not-store", "copy"), 20)),
        (
            "client-unknown",
            doc(),
            checkpoint(),
            Copy::Store { format: current, rows: stored_rows(&[repository()]) },
            undecided(b"client unknown", BOUNDARY),
        ),
        (
            "absent-document",
            None,
            checkpoint(),
            Copy::Absent,
            refused(&format!("configuration {}: absent", path("absent-document", "temper.json").display())),
        ),
        ("no-entry", Some(no_entry), checkpoint(), Copy::Absent, refused("forge 1: the setup has no entry for it")),
        ("managed", Some(managed), checkpoint(), Copy::Absent, refused("forge 1: it is managed, not fake")),
        ("owned", Some(owned), checkpoint(), Copy::Absent, refused("forge 1: it is owned, not fake")),
        ("invalid-document", Some(serde_json::json!({})), None, Copy::Absent, expected(1, b"", INVALID_DOCUMENT)),
        (
            "invalid-setup",
            Some(bad_setup),
            None,
            Copy::Absent,
            refused("application.setup[0].forge: unknown reference"),
        ),
    ] {
        let directory = root.join(name);
        fs::create_dir(&directory).expect("row directory");
        let directories = crate::directories::tests::given(&directory);
        let bytes = document.map(|document| serde_json::to_vec(&document).expect("fixture document"));
        if let Some(bytes) = &bytes {
            fs::write(directories.document(), bytes).expect("document file");
        }
        let check = Check { forge: 1, observations: directory.join("observations"), store: directory.join("copy") };
        if let Some(observations) = observations {
            fs::write(&check.observations, observations).expect("observations file");
        }
        match copy {
            Copy::Store { format, rows } => {
                let limits =
                    temper_service::startup::store_limits(bytes.as_ref().expect("seed document"), &directories.jig())
                        .expect("store limits");
                seed(&check.store, &limits.store, format, rows);
            }
            Copy::Absent => {}
            Copy::File => fs::write(&check.store, b"not a store").expect("non-store"),
        }
        assert_eq!(outcome(&directories, &check), expected, "{name}");
        assert!(!directory.join("data").exists(), "{name}: creates no data directory");
        assert!(!directory.join("cache").exists(), "{name}: creates no cache directory");
    }
    fs::remove_dir_all(root).expect("scratch removed");
}

#[test]
fn installation_records_are_validated_and_full_is_terminal_past_a_checkpoint() {
    let root = scratch("fake-check-installations");
    let path = root.join("observations");
    let created = b"{\"observation\":\"installation_created\",\"installation\":1}\n";
    let deleted = b"{\"observation\":\"installation_deleted\",\"installation\":1}\n";
    fs::write(&path, [created.as_slice(), deleted, START].concat()).expect("installation observation fixture");
    let observations = Observations::open(&path).expect("installation observation fixture");
    assert_eq!(
        observations.records().expect("installation observation fixture").count(),
        1,
        "only checkpoint goes to domain judge"
    );
    fs::write(&path, [START, b"{\"observation\":\"installations_full\",\"capacity\":2}\n"].concat())
        .expect("installation observation fixture");
    assert_eq!(Observations::open(&path).err().as_deref(), Some(INSTALLATIONS_FULL));
    fs::write(
        &path,
        [b"{\"observation\":\"installation_created\",\"installation\":1,\"token\":\"leak\"}\n", START].concat(),
    )
    .expect("installation observation fixture");
    assert!(Observations::open(&path).is_err(), "unknown fields cannot be skipped as observations");
    fs::remove_dir_all(root).expect("installation observation fixture");
}
