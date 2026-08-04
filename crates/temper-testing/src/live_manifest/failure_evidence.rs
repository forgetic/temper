// SPDX-License-Identifier: MPL-2.0

//! Generic loopback ordinary-failure evidence service for live scenarios.
//!
//! The service is enabled only by declarative manifest data. A protected real
//! Actions workflow publishes an already signed statement through POST; Temper
//! acquires the same record through the production GET transport. Runtime
//! credentials and signatures never enter retained evidence.

use std::fmt::Write as _;
use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use temper_forge_forgejo::{
    EngineHttpClient, ForgejoFailureEvidenceConfig, HttpClient, HttpMethod, HttpRequest,
};
use toml::Value as TomlValue;

pub(super) fn fixture_from_manifest(
    manifest: &TomlValue,
) -> Result<Option<CiFailureEvidenceFixture>, String> {
    let Some(value) = manifest
        .get("live_harness")
        .and_then(TomlValue::as_table)
        .and_then(|table| table.get("ci_failure_evidence"))
    else {
        return Ok(None);
    };
    let table = value
        .as_table()
        .ok_or_else(|| "live_harness.ci_failure_evidence must be a table".to_string())?;
    for key in table.keys() {
        if !matches!(key.as_str(), "issuer" | "protected_producers") {
            return Err(format!(
                "live_harness.ci_failure_evidence.{key} is unsupported"
            ));
        }
    }
    let issuer = table
        .get("issuer")
        .and_then(TomlValue::as_str)
        .map(str::trim)
        .filter(|value| valid_evidence_identity(value))
        .ok_or_else(|| {
            "live_harness.ci_failure_evidence.issuer must be a bounded ASCII identity".to_string()
        })?
        .to_string();
    let protected_producers = table
        .get("protected_producers")
        .and_then(TomlValue::as_array)
        .ok_or_else(|| {
            "live_harness.ci_failure_evidence.protected_producers must be an array".to_string()
        })?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| valid_evidence_identity(value))
                .map(str::to_string)
                .ok_or_else(|| {
                    "live_harness.ci_failure_evidence.protected_producers must contain bounded ASCII identities".to_string()
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if protected_producers.is_empty() {
        return Err(
            "live_harness.ci_failure_evidence.protected_producers must not be empty".to_string(),
        );
    }
    Ok(Some(CiFailureEvidenceFixture {
        issuer,
        protected_producers,
    }))
}

fn valid_evidence_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
        })
}

#[path = "failure_evidence/http.rs"]
mod http;

use super::process::engine_block_on;
use super::{CiRequestEvidence, RepoFixture};

const SERVICE_PATH: &str = "/v1/forgejo-failures";
const READ_SECRET_NAME: &str = "live-ci-evidence-reader";
const HMAC_SECRET_NAME: &str = "live-ci-evidence-hmac";
const MAX_REQUEST_BYTES: usize = 64 * 1024;

const ACTION_ENDPOINT_SECRET: &str = "TEMPER_CI_EVIDENCE_ENDPOINT";
const ACTION_PUBLISH_TOKEN_SECRET: &str = "TEMPER_CI_EVIDENCE_PUBLISH_TOKEN";
const ACTION_HMAC_SECRET: &str = "TEMPER_CI_EVIDENCE_HMAC_KEY";
const ACTION_ISSUER_SECRET: &str = "TEMPER_CI_EVIDENCE_ISSUER";
const ACTION_PRODUCER_SECRET: &str = "TEMPER_CI_EVIDENCE_PRODUCER";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CiFailureEvidenceFixture {
    pub issuer: String,
    pub protected_producers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveCiFailureEvidence {
    pub endpoint_path: String,
    pub issuer: String,
    pub protected_producers: Vec<String>,
    pub published_proofs: usize,
    pub requests: Vec<CiRequestEvidence>,
    pub log_path: PathBuf,
}

#[derive(Default)]
struct ServiceState {
    records: Vec<StoredRecord>,
    requests: Vec<CiRequestEvidence>,
}

struct StoredRecord {
    signed: SignedStatement,
    statement: FailureStatement,
}

#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct SignedStatement {
    statement: String,
    hmac_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceDocument {
    schema_version: u16,
    records: Vec<SignedStatement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FailureStatement {
    schema_version: u16,
    category: String,
    repository_id: String,
    pull_request_id: String,
    commit_sha: String,
    run_id: String,
    job_id: String,
    attempt: String,
    task_id: String,
    producer_id: String,
    issuer_id: String,
    created_at: String,
    expires_at: String,
}

pub(super) struct FailureEvidenceServer {
    fixture: CiFailureEvidenceFixture,
    endpoint: String,
    read_token: String,
    publish_token: String,
    hmac_key: String,
    state: Arc<Mutex<ServiceState>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    log_path: PathBuf,
}

impl FailureEvidenceServer {
    pub(super) fn start(
        fixture: CiFailureEvidenceFixture,
        log_path: PathBuf,
    ) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| format!("bind CI failure evidence service: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("set CI failure evidence service nonblocking: {error}"))?;
        let addr = listener
            .local_addr()
            .map_err(|error| format!("read CI failure evidence service address: {error}"))?;
        let endpoint = format!("http://{addr}{SERVICE_PATH}");
        let read_token = random_secret("reader")?;
        let publish_token = random_secret("publisher")?;
        let hmac_key = random_secret("HMAC")?;
        let state = Arc::new(Mutex::new(ServiceState::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_state = Arc::clone(&state);
        let thread_stop = Arc::clone(&stop);
        let thread_fixture = fixture.clone();
        let thread_read = read_token.clone();
        let thread_publish = publish_token.clone();
        let thread_hmac = hmac_key.clone();
        let thread_log = log_path.clone();
        let thread = thread::Builder::new()
            .name("temper-live-ci-failure-evidence".to_string())
            .spawn(move || {
                while !thread_stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let status = http::handle_connection(
                                &mut stream,
                                &thread_fixture,
                                &thread_read,
                                &thread_publish,
                                &thread_hmac,
                                &thread_state,
                            );
                            http::append_log(&thread_log, status);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20));
                        }
                        Err(error) => {
                            http::append_log(
                                &thread_log,
                                ("ACCEPT", "<socket>".to_string(), 500, Vec::new()),
                            );
                            if !thread_stop.load(Ordering::SeqCst) {
                                eprintln!("CI failure evidence service accept failed: {error}");
                            }
                        }
                    }
                }
            })
            .map_err(|error| format!("spawn CI failure evidence service: {error}"))?;
        Ok(Self {
            fixture,
            endpoint,
            read_token,
            publish_token,
            hmac_key,
            state,
            stop,
            thread: Some(thread),
            log_path,
        })
    }

    pub(super) fn configure_temper_bundle(&self, bundle: &Path) -> Result<(), String> {
        let config_path = bundle.join("config.toml");
        let mut config: TomlValue = fs::read_to_string(&config_path)
            .map_err(|error| format!("read {}: {error}", config_path.display()))?
            .parse()
            .map_err(|error| format!("parse {}: {error}", config_path.display()))?;
        let forge = config
            .get_mut("forge")
            .and_then(TomlValue::as_table_mut)
            .ok_or_else(|| "generated config has no [forge] table".to_string())?;
        forge.insert(
            "ci_failure_evidence".to_string(),
            TomlValue::Table(toml::Table::from_iter([
                (
                    "endpoint".to_string(),
                    TomlValue::String(self.endpoint.clone()),
                ),
                (
                    "issuer".to_string(),
                    TomlValue::String(self.fixture.issuer.clone()),
                ),
                (
                    "protected_producers".to_string(),
                    TomlValue::Array(
                        self.fixture
                            .protected_producers
                            .iter()
                            .cloned()
                            .map(TomlValue::String)
                            .collect(),
                    ),
                ),
                (
                    "bearer_token".to_string(),
                    TomlValue::String(READ_SECRET_NAME.to_string()),
                ),
                (
                    "hmac_key".to_string(),
                    TomlValue::String(HMAC_SECRET_NAME.to_string()),
                ),
            ])),
        );
        fs::write(
            &config_path,
            toml::to_string_pretty(&config)
                .map_err(|error| format!("serialize tuned evidence config: {error}"))?,
        )
        .map_err(|error| format!("write {}: {error}", config_path.display()))?;

        let credentials_path = bundle.join("credentials.toml");
        let mut credentials: TomlValue = fs::read_to_string(&credentials_path)
            .map_err(|error| format!("read {}: {error}", credentials_path.display()))?
            .parse()
            .map_err(|error| format!("parse {}: {error}", credentials_path.display()))?;
        let root = credentials
            .as_table_mut()
            .ok_or_else(|| "generated credentials root is not a table".to_string())?;
        let secrets = root
            .entry("secrets")
            .or_insert_with(|| TomlValue::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or_else(|| "generated credentials [secrets] is not a table".to_string())?;
        secrets.insert(
            READ_SECRET_NAME.to_string(),
            TomlValue::String(self.read_token.clone()),
        );
        secrets.insert(
            HMAC_SECRET_NAME.to_string(),
            TomlValue::String(self.hmac_key.clone()),
        );
        fs::write(
            &credentials_path,
            toml::to_string_pretty(&credentials)
                .map_err(|error| format!("serialize tuned credentials: {error}"))?,
        )
        .map_err(|error| format!("write {}: {error}", credentials_path.display()))
    }

    pub(super) fn configure_repository_actions(
        &self,
        base_url: &str,
        admin_token: &str,
        repo: &RepoFixture,
    ) -> Result<(), String> {
        let producer = self
            .fixture
            .protected_producers
            .first()
            .expect("fixture parser requires a producer");
        for (name, value) in [
            (ACTION_ENDPOINT_SECRET, self.endpoint.as_str()),
            (ACTION_PUBLISH_TOKEN_SECRET, self.publish_token.as_str()),
            (ACTION_HMAC_SECRET, self.hmac_key.as_str()),
            (ACTION_ISSUER_SECRET, self.fixture.issuer.as_str()),
            (ACTION_PRODUCER_SECRET, producer.as_str()),
        ] {
            let request = HttpRequest {
                method: HttpMethod::Put,
                path: format!(
                    "/api/v1/repos/{}/{}/actions/secrets/{name}",
                    repo.owner, repo.name
                ),
                query: Vec::new(),
                headers: vec![
                    ("Authorization".to_string(), format!("token {admin_token}")),
                    ("Accept".to_string(), "application/json".to_string()),
                    ("Content-Type".to_string(), "application/json".to_string()),
                ],
                body: Some(json!({ "data": value }).to_string()),
            };
            let response = engine_block_on(EngineHttpClient::new(base_url).execute(request))
                .map_err(|error| format!("configure Actions secret {name}: {error}"))?;
            if !response.is_success() {
                return Err(format!(
                    "configure Actions secret {name} returned HTTP {}: {}",
                    response.status, response.body
                ));
            }
        }
        Ok(())
    }

    pub(super) fn backend_config(&self) -> Result<ForgejoFailureEvidenceConfig, String> {
        ForgejoFailureEvidenceConfig::new(
            &self.endpoint,
            &self.read_token,
            &self.hmac_key,
            &self.fixture.issuer,
            self.fixture.protected_producers.iter().map(String::as_str),
        )
    }

    pub(super) fn evidence(&self) -> LiveCiFailureEvidence {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        LiveCiFailureEvidence {
            endpoint_path: SERVICE_PATH.to_string(),
            issuer: self.fixture.issuer.clone(),
            protected_producers: self.fixture.protected_producers.clone(),
            published_proofs: state.records.len(),
            requests: state.requests.clone(),
            log_path: self.log_path.clone(),
        }
    }
}

impl Drop for FailureEvidenceServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(
            self.endpoint
                .strip_prefix("http://")
                .and_then(|value| value.split('/').next())
                .unwrap_or("127.0.0.1:0"),
        );
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn random_secret(label: &str) -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("generate per-run CI evidence {label} secret: {error}"))?;
    let mut secret = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut secret, "{byte:02x}").expect("writing hexadecimal to String cannot fail");
    }
    Ok(secret)
}
