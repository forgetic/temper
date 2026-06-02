//! Loopback HTTP API for product-manager chat.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use temper_agents::{
    AuthChoice, ProductManagerAgent, ProductManagerDraftIssue, ProviderConfig, ProviderError,
};
use temper_forge::{Forge, Issue, ItemNumber, RepositoryPath};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use tokio::sync::Mutex;

use crate::product_chat::{
    ProductChatError, ProductChatOpenOptions, ProductChatSession, ProductManagerResponder,
};
use crate::product_chat_args::{AuthKind, ProductChatServeArgs};
use crate::product_chat_commands::{render_drafts, ProductChatCommand, COMMAND_HELP};

const MAX_REQUEST_BYTES: usize = 1_048_576;
type DynSession = ProductChatSession<dyn Forge, dyn Forge, dyn ProductManagerResponder>;

#[derive(Debug)]
pub enum ProductChatServiceError {
    ProductChat(ProductChatError),
    Io(std::io::Error),
}

impl fmt::Display for ProductChatServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProductChatServiceError::ProductChat(error) => write!(formatter, "{error}"),
            ProductChatServiceError::Io(error) => write!(formatter, "service I/O failed: {error}"),
        }
    }
}

impl std::error::Error for ProductChatServiceError {}

impl From<ProductChatError> for ProductChatServiceError {
    fn from(error: ProductChatError) -> Self {
        Self::ProductChat(error)
    }
}

impl From<ProviderError> for ProductChatServiceError {
    fn from(error: ProviderError) -> Self {
        Self::ProductChat(ProductChatError::Provider(error))
    }
}

impl From<std::io::Error> for ProductChatServiceError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn run_serve(args: &ProductChatServeArgs) -> Result<(), ProductChatServiceError> {
    validate_serve_safety(args)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| ProductChatError::Runtime(error.to_string()))?;
    let provider = ProviderConfig::from_auth(
        auth_choice(args.auth),
        args.codex_model.clone(),
        args.auth_file.clone(),
    )?;
    let service = ProductChatService::new(
        args.base_url.clone(),
        args.repo.clone(),
        Arc::new(build_forge(&args.base_url, &args.human_token)) as Arc<dyn Forge>,
        Arc::new(build_forge(&args.base_url, &args.product_manager_token)) as Arc<dyn Forge>,
        Arc::new(ProductManagerAgent::new(provider)) as Arc<dyn ProductManagerResponder>,
    );
    let app = ProductChatHttpApp::new(service, args.service_token.clone());
    run_http(args.bind, app, &runtime)
}

fn validate_serve_safety(args: &ProductChatServeArgs) -> Result<(), ProductChatError> {
    if args.bind.ip().is_loopback() {
        return Ok(());
    }
    if !args.allow_non_loopback {
        return Err(ProductChatError::Runtime(
            "non-loopback bind requires --allow-non-loopback".into(),
        ));
    }
    if args
        .service_token
        .as_deref()
        .filter(|token| !token.trim().is_empty())
        .is_none()
    {
        return Err(ProductChatError::Runtime(
            "non-loopback bind requires TEMPER_PRODUCT_CHAT_SERVICE_TOKEN".into(),
        ));
    }
    Ok(())
}

fn build_forge(base_url: &str, token: &str) -> ForgejoForge {
    ForgejoForge::new(ForgejoConfig::new(base_url.to_string(), token.to_string()))
}

fn auth_choice(auth: AuthKind) -> AuthChoice {
    match auth {
        AuthKind::DeepSeek => AuthChoice::DeepSeek,
        AuthKind::ChatGptOAuth => AuthChoice::ChatGptOAuth,
        AuthKind::AnthropicOAuth => AuthChoice::AnthropicOAuth,
    }
}

fn run_http(
    bind: SocketAddr,
    app: ProductChatHttpApp,
    runtime: &tokio::runtime::Runtime,
) -> Result<(), ProductChatServiceError> {
    let listener = TcpListener::bind(bind)?;
    eprintln!("temper-product-manager-chat: serving on {bind}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = handle_connection(stream, &app, runtime) {
                    eprintln!("temper-product-manager-chat: {error}");
                }
            }
            Err(error) => eprintln!("temper-product-manager-chat: accept failed: {error}"),
        }
    }
    Ok(())
}

fn handle_connection(
    mut stream: TcpStream,
    app: &ProductChatHttpApp,
    runtime: &tokio::runtime::Runtime,
) -> Result<(), ProductChatServiceError> {
    let request = HttpRequest::read_from(&mut stream)?;
    let response = runtime.block_on(app.handle_http_request(request));
    stream.write_all(response.to_http().as_bytes())?;
    Ok(())
}

pub struct ProductChatService {
    base_url: String,
    repo_path: RepositoryPath,
    human_forge: Arc<dyn Forge>,
    product_forge: Arc<dyn Forge>,
    responder: Arc<dyn ProductManagerResponder>,
    sessions: Mutex<HashMap<String, DynSession>>,
}

impl ProductChatService {
    pub fn new(
        base_url: String,
        repo_path: RepositoryPath,
        human_forge: Arc<dyn Forge>,
        product_forge: Arc<dyn Forge>,
        responder: Arc<dyn ProductManagerResponder>,
    ) -> Self {
        Self {
            base_url,
            repo_path,
            human_forge,
            product_forge,
            responder,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    async fn create_session(
        &self,
        transcript_issue: Option<u64>,
    ) -> Result<SessionResponse, ProductChatError> {
        let session = ProductChatSession::open(
            Arc::clone(&self.human_forge),
            Arc::clone(&self.product_forge),
            Arc::clone(&self.responder),
            ProductChatOpenOptions {
                base_url: self.base_url.clone(),
                repo_path: self.repo_path.clone(),
                transcript_issue: transcript_issue.map(ItemNumber::new),
            },
        )
        .await?;
        let response = session_response(&session);
        self.sessions
            .lock()
            .await
            .insert(response.id.clone(), session);
        Ok(response)
    }

    async fn get_session(&self, id: &str) -> Result<SessionResponse, ApiError> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(id)
            .ok_or_else(|| ApiError::not_found("session not found"))?;
        Ok(session_response(session))
    }

    async fn send_message(&self, id: &str, message: String) -> Result<MessageResponse, ApiError> {
        if message.trim().is_empty() {
            return Err(ApiError::bad_request("message must not be empty"));
        }
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| ApiError::not_found("session not found"))?;
        if let Some(command) = ProductChatCommand::parse(&message) {
            return handle_local_message_command(session, command);
        }
        let response = session.send_human_turn(&message).await?;
        Ok(MessageResponse {
            reply: response.reply,
            drafts: response.drafts,
            transcript_url: session.transcript_url(),
        })
    }

    async fn file_draft(&self, id: &str, slug: &str) -> Result<FileDraftResponse, ApiError> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(id)
            .ok_or_else(|| ApiError::not_found("session not found"))?;
        let outcome = session.file_draft_slug(slug).await?;
        Ok(FileDraftResponse {
            created: outcome.created,
            issue: filed_issue_response(
                &outcome.issue,
                session.issue_url_for(outcome.issue.number),
            ),
            transcript_url: session.transcript_url(),
        })
    }
}

fn handle_local_message_command(
    session: &mut DynSession,
    command: ProductChatCommand<'_>,
) -> Result<MessageResponse, ApiError> {
    let reply = match command {
        ProductChatCommand::Help => COMMAND_HELP.to_string(),
        ProductChatCommand::Drafts => render_drafts(session.latest_drafts()),
        ProductChatCommand::Issue => session.transcript_url(),
        ProductChatCommand::Quit => "Close the client to end this product conversation.".into(),
        ProductChatCommand::File(_) => {
            return Err(ApiError::bad_request(
                "file drafts through POST /sessions/{id}/drafts/{slug}/file",
            ))
        }
        ProductChatCommand::Unknown(command) => {
            return Err(ApiError::bad_request(format!(
                "unknown command '{command}'; try /help"
            )))
        }
    };
    Ok(MessageResponse {
        reply,
        drafts: session.latest_drafts().to_vec(),
        transcript_url: session.transcript_url(),
    })
}

pub struct ProductChatHttpApp {
    service: ProductChatService,
    service_token: Option<String>,
}

impl ProductChatHttpApp {
    pub fn new(service: ProductChatService, service_token: Option<String>) -> Self {
        Self {
            service,
            service_token,
        }
    }

    pub(crate) async fn handle_http_request(&self, request: HttpRequest) -> HttpResponse {
        if let Err(error) = self.authorize(&request) {
            return error.into_response();
        }
        match self.route(request).await {
            Ok(response) => response,
            Err(error) => error.into_response(),
        }
    }

    async fn route(&self, request: HttpRequest) -> Result<HttpResponse, ApiError> {
        let path = request.path.split('?').next().unwrap_or(&request.path);
        if request.method == "GET" && path == "/health" {
            return Ok(HttpResponse::json(200, &json!({ "ok": true })));
        }
        if request.method == "POST" && path == "/sessions" {
            let body = parse_json::<CreateSessionRequest>(&request)?;
            let response = self.service.create_session(body.transcript_issue).await?;
            return Ok(HttpResponse::json(201, &response));
        }
        let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
        match (request.method.as_str(), segments.as_slice()) {
            ("GET", ["sessions", id]) => Ok(HttpResponse::json(
                200,
                &self.service.get_session(id).await?,
            )),
            ("POST", ["sessions", id, "messages"]) => {
                let body = parse_json::<SendMessageRequest>(&request)?;
                Ok(HttpResponse::json(
                    200,
                    &self.service.send_message(id, body.message).await?,
                ))
            }
            ("POST", ["sessions", id, "drafts", slug, "file"]) => Ok(HttpResponse::json(
                200,
                &self.service.file_draft(id, slug).await?,
            )),
            _ => Err(ApiError::not_found("endpoint not found")),
        }
    }

    fn authorize(&self, request: &HttpRequest) -> Result<(), ApiError> {
        let Some(token) = self.service_token.as_deref() else {
            return Ok(());
        };
        let supplied = request
            .headers
            .get("authorization")
            .and_then(|value| value.strip_prefix("Bearer "));
        if supplied == Some(token) {
            Ok(())
        } else {
            Err(ApiError::unauthorized("missing or invalid bearer token"))
        }
    }
}

#[derive(Debug)]
pub(crate) struct HttpRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl HttpRequest {
    #[cfg(test)]
    pub(crate) fn new(method: &str, path: &str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            method: method.to_string(),
            path: path.to_string(),
            headers: BTreeMap::new(),
            body: body.into(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers
            .insert(name.to_ascii_lowercase(), value.to_string());
        self
    }

    fn read_from(stream: &mut TcpStream) -> Result<Self, ProductChatServiceError> {
        let mut raw = Vec::new();
        let mut buf = [0_u8; 4096];
        loop {
            let n = stream.read(&mut buf)?;
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&buf[..n]);
            if raw.len() > MAX_REQUEST_BYTES {
                return Err(ProductChatError::Runtime("HTTP request is too large".into()).into());
            }
            if let Some(header_end) = find_header_end(&raw) {
                let (method, path, headers) = parse_headers(&raw[..header_end])?;
                let body_start = header_end + 4;
                let content_len = header(&headers, "content-length")
                    .and_then(|raw| raw.parse::<usize>().ok())
                    .unwrap_or(0);
                if body_start + content_len > MAX_REQUEST_BYTES {
                    return Err(
                        ProductChatError::Runtime("HTTP request is too large".into()).into(),
                    );
                }
                while raw.len() < body_start + content_len {
                    let n = stream.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                }
                if raw.len() < body_start + content_len {
                    return Err(ProductChatError::Runtime("incomplete HTTP request".into()).into());
                }
                return Ok(Self {
                    method,
                    path,
                    headers,
                    body: raw[body_start..body_start + content_len].to_vec(),
                });
            }
        }
        Err(ProductChatError::Runtime("incomplete HTTP request".into()).into())
    }
}

#[derive(Debug)]
pub(crate) struct HttpResponse {
    status: u16,
    body: String,
}

impl HttpResponse {
    #[cfg(test)]
    pub(crate) fn status(&self) -> u16 {
        self.status
    }

    #[cfg(test)]
    pub(crate) fn body(&self) -> &str {
        &self.body
    }

    fn json<T: Serialize + ?Sized>(status: u16, value: &T) -> Self {
        let body = serde_json::to_string(value).expect("serializing API response succeeds");
        Self { status, body }
    }

    fn to_http(&self) -> String {
        format!(
            "HTTP/1.1 {} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            self.status,
            reason(self.status),
            self.body.len(),
            self.body
        )
    }
}

#[derive(Debug)]
struct ApiError {
    status: u16,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: 401,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: 404,
            message: message.into(),
        }
    }

    fn into_response(self) -> HttpResponse {
        HttpResponse::json(self.status, &json!({ "error": self.message }))
    }
}

impl From<ProductChatError> for ApiError {
    fn from(error: ProductChatError) -> Self {
        match error {
            ProductChatError::TranscriptNotFound { .. }
            | ProductChatError::DraftNotFound { .. } => ApiError::not_found(error.to_string()),
            ProductChatError::InvalidDraftNumber { .. }
            | ProductChatError::TranscriptNotProduct { .. }
            | ProductChatError::RepositoryNotFound { .. } => {
                ApiError::bad_request(error.to_string())
            }
            other => ApiError {
                status: 500,
                message: other.to_string(),
            },
        }
    }
}

#[derive(Deserialize)]
struct CreateSessionRequest {
    #[serde(default)]
    transcript_issue: Option<u64>,
}

#[derive(Deserialize)]
struct SendMessageRequest {
    message: String,
}

#[derive(Serialize)]
struct SessionResponse {
    id: String,
    transcript_issue: u64,
    transcript_url: String,
    drafts: Vec<ProductManagerDraftIssue>,
}

#[derive(Serialize)]
struct MessageResponse {
    reply: String,
    drafts: Vec<ProductManagerDraftIssue>,
    transcript_url: String,
}

#[derive(Serialize)]
struct FileDraftResponse {
    created: bool,
    issue: FiledIssueResponse,
    transcript_url: String,
}

#[derive(Serialize)]
struct FiledIssueResponse {
    number: u64,
    url: String,
    title: String,
}

fn session_response(session: &DynSession) -> SessionResponse {
    SessionResponse {
        id: session.session_key().to_string(),
        transcript_issue: session.transcript_issue().number.get(),
        transcript_url: session.transcript_url(),
        drafts: session.latest_drafts().to_vec(),
    }
}

fn filed_issue_response(issue: &Issue, url: String) -> FiledIssueResponse {
    FiledIssueResponse {
        number: issue.number.get(),
        url,
        title: issue.title.clone(),
    }
}

fn parse_json<T: for<'de> Deserialize<'de>>(request: &HttpRequest) -> Result<T, ApiError> {
    if request.body.is_empty() {
        serde_json::from_str("{}").map_err(|error| ApiError::bad_request(error.to_string()))
    } else {
        serde_json::from_slice(&request.body)
            .map_err(|error| ApiError::bad_request(format!("invalid JSON body: {error}")))
    }
}

fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_headers(
    raw: &[u8],
) -> Result<(String, String, BTreeMap<String, String>), ProductChatError> {
    let text = std::str::from_utf8(raw)
        .map_err(|_| ProductChatError::Runtime("HTTP headers are not UTF-8".into()))?;
    let mut lines = text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| ProductChatError::Runtime("missing request line".into()))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut headers = BTreeMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    Ok((method, path, headers))
}

fn header<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    headers.get(name).map(String::as_str)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    }
}
