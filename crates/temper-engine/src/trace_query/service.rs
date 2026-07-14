// SPDX-License-Identifier: MPL-2.0

use secrecy::SecretString;
use serde::Serialize;
use temper_engine_io::http::{HttpRequestData, HttpResponseData};

use crate::trace_journal::{AgentTraceJournal, TraceJournalError};

use super::ApiError;
use super::auth::authorize;
use super::cursor::{RunOrderKey, decode_cursor, encode_cursor, run_order_key};
use super::model::{TraceEventPage, TraceRunPage, TraceRunSummary};
use super::projection::project_summary;
use super::request::{EventQuery, RunListQuery, TraceRoute, parse_route};

/// Cloneable executor-side query capability. The token stays secret-wrapped
/// until the authorization comparison and is never formatted or serialized.
#[derive(Clone)]
pub(crate) struct TraceQueryService {
    journal: AgentTraceJournal,
    read_token: SecretString,
}

impl TraceQueryService {
    pub(crate) fn new(journal: AgentTraceJournal, read_token: SecretString) -> Self {
        Self {
            journal,
            read_token,
        }
    }

    pub(crate) fn handle(&self, request: HttpRequestData) -> HttpResponseData {
        if let Err(error) = authorize(&request.headers, &self.read_token) {
            return error_response(error);
        }
        let response = parse_route(&request.method, &request.uri)
            .and_then(|route| self.handle_authorized(route));
        match response {
            Ok(response) => response,
            Err(error) => error_response(error),
        }
    }

    fn handle_authorized(&self, route: TraceRoute) -> Result<HttpResponseData, ApiError> {
        match route {
            TraceRoute::List(query) => self.list_runs(query).map(trace_json),
            TraceRoute::Summary(run_id) => self.one_run(&run_id).map(trace_json),
            TraceRoute::Events(run_id, query) => self.event_page(&run_id, query).map(trace_json),
            TraceRoute::Export(run_id) => self.export(&run_id),
        }
    }

    fn list_runs(&self, query: RunListQuery) -> Result<TraceRunPage, ApiError> {
        let filter_hash = query.filters.hash();
        let after = query
            .cursor
            .as_deref()
            .map(|cursor| decode_cursor(cursor, &filter_hash))
            .transpose()?;
        let runs = self.journal.runs().map_err(store_unavailable)?;
        let mut listed = runs
            .into_iter()
            .filter(|run| query.filters.matches(run))
            .map(|run| {
                let key = run_order_key(&run)?;
                Ok(ListedRun {
                    key,
                    summary: project_summary(&run),
                })
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        listed.sort_by(|left, right| left.key.cmp(&right.key));
        if let Some(after) = after {
            listed.retain(|run| run.key > after);
        }

        let has_more = listed.len() > query.limit;
        listed.truncate(query.limit);
        let next_cursor = if has_more {
            listed
                .last()
                .map(|run| encode_cursor(&run.key, &filter_hash))
                .transpose()?
        } else {
            None
        };
        Ok(TraceRunPage {
            runs: listed.into_iter().map(|run| run.summary).collect(),
            next_cursor,
        })
    }

    fn one_run(&self, run_id: &str) -> Result<TraceRunSummary, ApiError> {
        let run = self
            .journal
            .run(run_id)
            .map_err(store_unavailable)?
            .ok_or(ApiError::NotFound)?;
        Ok(project_summary(&run))
    }

    fn event_page(&self, run_id: &str, query: EventQuery) -> Result<TraceEventPage, ApiError> {
        let run = self
            .journal
            .run(run_id)
            .map_err(store_unavailable)?
            .ok_or(ApiError::NotFound)?;
        let mut events = run
            .events
            .into_iter()
            .filter(|event| event.seq > query.after_seq)
            .collect::<Vec<_>>();
        let has_more = events.len() > query.limit;
        events.truncate(query.limit);
        let next_after_seq = events.last().map_or(query.after_seq, |event| event.seq);
        Ok(TraceEventPage {
            run_id: run_id.to_string(),
            events,
            next_after_seq,
            has_more,
        })
    }

    fn export(&self, run_id: &str) -> Result<HttpResponseData, ApiError> {
        let run = self
            .journal
            .run(run_id)
            .map_err(store_unavailable)?
            .ok_or(ApiError::NotFound)?;
        let mut body = Vec::new();
        for event in run.events {
            serde_json::to_writer(&mut body, &event).map_err(|_| ApiError::Unavailable)?;
            body.push(b'\n');
        }
        Ok(no_store(HttpResponseData {
            status: 200,
            headers: vec![(
                "content-type".to_string(),
                "application/x-ndjson".to_string(),
            )],
            body,
        }))
    }
}

struct ListedRun {
    key: RunOrderKey,
    summary: TraceRunSummary,
}

pub(crate) fn disabled_trace_response() -> HttpResponseData {
    error_response(ApiError::NotFound)
}

fn trace_json<T: Serialize>(value: T) -> HttpResponseData {
    let value = serde_json::to_value(value).expect("trace query DTOs serialize");
    no_store(HttpResponseData::json(200, &value))
}

fn no_store(mut response: HttpResponseData) -> HttpResponseData {
    response
        .headers
        .push(("cache-control".to_string(), "no-store".to_string()));
    response
        .headers
        .push(("x-content-type-options".to_string(), "nosniff".to_string()));
    response
}

fn error_response(error: ApiError) -> HttpResponseData {
    let (status, code, message) = match error {
        ApiError::BadRequest(message) => (400, "invalid_request", message),
        ApiError::Unauthorized => (
            401,
            "authorization_required",
            "a bearer authorization credential is required",
        ),
        ApiError::Forbidden => (
            403,
            "forbidden",
            "the authorization credential is not valid",
        ),
        ApiError::NotFound => (404, "not_found", "the requested resource was not found"),
        ApiError::Unavailable => (
            500,
            "trace_store_unavailable",
            "agent trace data is temporarily unavailable",
        ),
    };
    let mut response = HttpResponseData::json(
        status,
        &serde_json::json!({"error": code, "message": message}),
    );
    if status == 401 {
        response.headers.push((
            "www-authenticate".to_string(),
            "Bearer realm=\"temper-agent-traces\"".to_string(),
        ));
    }
    no_store(response)
}

fn store_unavailable(_error: TraceJournalError) -> ApiError {
    ApiError::Unavailable
}
