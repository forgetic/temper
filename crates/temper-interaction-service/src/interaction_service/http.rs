//! Profile-neutral HTTP transport over [`InteractionService`].

use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};

use serde_json::json;
use temper_interaction::ProposalId;

use crate::interaction_api::{ApiError, CreateConversationRequest, SendTurnRequest, parse_json};
use crate::interaction_service::{
    HttpRequest, HttpResponse, InteractionService, InteractionServiceError,
};

/// Profile-neutral HTTP app over [`InteractionService`].
pub struct InteractionHttpApp {
    service: InteractionService,
    service_token: Option<String>,
}

impl InteractionHttpApp {
    pub fn new(service: InteractionService, service_token: Option<String>) -> Self {
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
        if request.method == "POST" && path == "/conversations" {
            let body = parse_json::<CreateConversationRequest>(&request)?;
            let response = self
                .service
                .create_conversation(body.profile_id, body.transcript_issue, json!({}))
                .await?;
            return Ok(HttpResponse::json(201, &response));
        }
        let segments: Vec<&str> = path
            .trim_matches('/')
            .split('/')
            .filter(|segment| !segment.is_empty())
            .collect();
        self.route_conversation(&request, segments.as_slice()).await
    }

    async fn route_conversation(
        &self,
        request: &HttpRequest,
        segments: &[&str],
    ) -> Result<HttpResponse, ApiError> {
        match (request.method.as_str(), segments) {
            ("GET", ["conversations", id]) => Ok(HttpResponse::json(
                200,
                &self.service.get_conversation(id).await?,
            )),
            ("POST", ["conversations", id, "turns"]) => {
                let body = parse_json::<SendTurnRequest>(request)?.into_body()?;
                Ok(HttpResponse::json(
                    200,
                    &self.service.send_turn(id, body).await?,
                ))
            }
            ("GET", ["conversations", id, "proposals"]) => Ok(HttpResponse::json(
                200,
                &self.service.latest_proposals(id).await?,
            )),
            ("GET", ["conversations", id, "events"]) => Ok(HttpResponse::json(
                200,
                &self.service.conversation_events(id).await?,
            )),
            ("POST", ["conversations", id, "proposals", proposal_id, "accept"]) => {
                let proposal_id = ProposalId::new((*proposal_id).to_string())
                    .map_err(|error| ApiError::bad_request(error.to_string()))?;
                Ok(HttpResponse::json(
                    200,
                    &self.service.accept_proposal(id, proposal_id).await?,
                ))
            }
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

pub fn run_http(
    bind: SocketAddr,
    app: InteractionHttpApp,
    runtime: &temper_engine_io::EngineRuntime,
) -> Result<(), InteractionServiceError> {
    let listener = TcpListener::bind(bind)?;
    tracing::info!(
        target: "temper_interaction",
        %bind,
        "interaction: serving on {bind}"
    );
    let app = std::sync::Arc::new(app);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = handle_connection(stream, &app, runtime) {
                    tracing::warn!(target: "temper_interaction", %error, "request handling failed");
                }
            }
            Err(error) => tracing::warn!(target: "temper_interaction", %error, "accept failed"),
        }
    }
    Ok(())
}

fn handle_connection(
    mut stream: TcpStream,
    app: &std::sync::Arc<InteractionHttpApp>,
    runtime: &temper_engine_io::EngineRuntime,
) -> Result<(), InteractionServiceError> {
    let request = HttpRequest::read_from(&mut stream)?;
    // Each request runs as one engine task: responder subprocess calls and
    // their deadlines are engine I/O requests needing a task context.
    let app = std::sync::Arc::clone(app);
    let response = temper_engine_io::runtime::block_on_runtime(runtime, async move {
        app.handle_http_request(request).await
    });
    stream.write_all(response.to_http().as_bytes())?;
    Ok(())
}
