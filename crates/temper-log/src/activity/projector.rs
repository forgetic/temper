use std::collections::HashMap;
use std::sync::Arc;

use temper_protocol_activity::{AgentActivityEventV1 as Event, AgentRunEventV1, UsageV1};

use super::helpers::*;

use super::{
    ActivitySpanAttributes, ActivitySpanExporter, ActivitySpanKind, ActivitySpanStart,
    ActivitySpanStatus, ProjectedActivitySpan,
};

struct ActiveSpan {
    start: ActivitySpanStart,
    started_elapsed_ms: u64,
    attributes: ActivitySpanAttributes,
    pending_finish: Option<PendingFinish>,
}

struct PendingFinish {
    ended_at: String,
    duration_ms: u64,
    status: ActivitySpanStatus,
}

#[derive(Default)]
struct RunState {
    active: HashMap<String, ActiveSpan>,
}

/// Stateful, replay-safe projection from canonical journal events to spans.
///
/// Sequence deduplication means callers may replay a complete durable journal
/// after restart and then continue with newly ingested events. Exporter panics
/// are contained so observability can never change an assignment outcome.
pub struct CanonicalActivityProjector {
    exporter: Arc<dyn ActivitySpanExporter>,
    runs: HashMap<String, RunState>,
    last_seq: HashMap<String, u64>,
}

impl CanonicalActivityProjector {
    pub fn new(exporter: Arc<dyn ActivitySpanExporter>) -> Self {
        Self {
            exporter,
            runs: HashMap::new(),
            last_seq: HashMap::new(),
        }
    }

    pub fn project_all(&mut self, events: &[AgentRunEventV1]) {
        for event in events {
            self.project(event);
        }
    }

    pub fn project(&mut self, event: &AgentRunEventV1) {
        let last_seq = self.last_seq.entry(event.run_id.clone()).or_default();
        if event.seq <= *last_seq {
            return;
        }
        *last_seq = event.seq;
        self.runs.entry(event.run_id.clone()).or_default();

        match &event.event {
            Event::RunStarted(_) => self.start_span(
                event,
                run_span_id(event),
                None,
                ActivitySpanKind::Run,
                ActivitySpanAttributes::default(),
            ),
            Event::ScopeStarted(_) => {
                let mut attributes = scoped_attributes(event);
                attributes.turn = None;
                self.start_span(
                    event,
                    scope_span_id(event),
                    scope_parent_span_id(event),
                    ActivitySpanKind::Scope,
                    attributes,
                );
            }
            Event::TurnStarted(_) => self.start_span(
                event,
                turn_span_id(event),
                Some(scope_span_id(event)),
                ActivitySpanKind::Turn,
                scoped_attributes(event),
            ),
            Event::ModelCallStarted(model) => {
                let mut attributes = scoped_attributes(event);
                attributes.call_id = Some(model.call_id.clone());
                attributes.provider = Some(model.provider.clone());
                attributes.model = Some(model.model.clone());
                attributes.attempt = Some(model.attempt);
                self.start_span(
                    event,
                    model_span_id(event, &model.call_id, model.attempt),
                    Some(operation_parent_id(event)),
                    ActivitySpanKind::ModelCall,
                    attributes,
                );
            }
            Event::ToolStarted(tool) => {
                let mut attributes = scoped_attributes(event);
                attributes.call_id = Some(tool.call_id.clone());
                attributes.tool_name = Some(tool.name.clone());
                self.start_span(
                    event,
                    tool_span_id(event, &tool.call_id),
                    Some(operation_parent_id(event)),
                    ActivitySpanKind::Tool,
                    attributes,
                );
            }
            Event::Usage(usage) => self.record_usage(event, usage),
            Event::TraceGap(gap) => self.record_gap(event, gap),
            Event::ModelCallRetrying(retry) => self.finish_retry(event, retry),
            Event::ModelCallFinished(model) => self.finish_model(event, model),
            Event::ToolFinished(tool) => self.finish_span(
                event,
                &tool_span_id(event, &tool.call_id),
                tool.duration_ms,
                tool_status(tool.status),
            ),
            Event::TurnFinished(turn) => {
                self.with_active(event, &turn_span_id(event), |span| {
                    span.attributes.stop_reason = Some(stop_reason(turn.stop_reason));
                });
                self.finish_span(
                    event,
                    &turn_span_id(event),
                    turn.duration_ms,
                    stop_status(turn.stop_reason),
                );
            }
            Event::ScopeFinished(scope) => {
                self.flush_pending(event, Some(event.scope.id.as_str()));
                self.with_active(event, &scope_span_id(event), |span| {
                    span.attributes.terminal_reason = scope.terminal_reason;
                });
                self.finish_span(
                    event,
                    &scope_span_id(event),
                    scope.duration_ms,
                    scope_status(scope.status),
                );
            }
            Event::RunFinished(run) => {
                self.flush_children(event, run_status(run.status));
                self.with_active(event, &run_span_id(event), |span| {
                    span.attributes.stop_reason = run.stop_reason.map(stop_reason);
                });
                self.finish_span(
                    event,
                    &run_span_id(event),
                    run.duration_ms,
                    run_status(run.status),
                );
                self.runs.remove(&event.run_id);
            }
            Event::RunFailed(_) => {
                self.flush_children(event, ActivitySpanStatus::Error);
                self.finish_span(
                    event,
                    &run_span_id(event),
                    event.elapsed_ms,
                    ActivitySpanStatus::Error,
                );
                self.runs.remove(&event.run_id);
            }
            Event::PromptPrepared(_)
            | Event::AssistantMessage(_)
            | Event::OutputTextDelta(_)
            | Event::OutputThinkingDelta(_)
            | Event::SteeringApplied(_) => {}
        }
    }

    fn start_span(
        &mut self,
        event: &AgentRunEventV1,
        span_id: String,
        parent_span_id: Option<String>,
        kind: ActivitySpanKind,
        attributes: ActivitySpanAttributes,
    ) {
        let start = ActivitySpanStart {
            run_id: event.run_id.clone(),
            span_id: span_id.clone(),
            parent_span_id,
            kind,
            started_at: event.occurred_at.clone(),
            assignment: event.assignment.clone(),
            agent_session_id: event.agent_session_id.clone(),
            remote_parent: (kind == ActivitySpanKind::Run)
                .then(|| event.assignment.trace_context.clone())
                .flatten(),
            attributes: attributes.clone(),
        };
        safe_export(|| self.exporter.span_started(&start));
        self.runs
            .get_mut(&event.run_id)
            .expect("run state created before projection")
            .active
            .insert(
                span_id,
                ActiveSpan {
                    start,
                    started_elapsed_ms: event.elapsed_ms,
                    attributes,
                    pending_finish: None,
                },
            );
    }

    fn finish_span(
        &mut self,
        event: &AgentRunEventV1,
        span_id: &str,
        duration_ms: u64,
        status: ActivitySpanStatus,
    ) {
        self.finish_at(
            &event.run_id,
            span_id,
            event.occurred_at.clone(),
            duration_ms,
            status,
        );
    }

    fn finish_at(
        &mut self,
        run_id: &str,
        span_id: &str,
        ended_at: String,
        duration_ms: u64,
        status: ActivitySpanStatus,
    ) {
        let Some(active) = self
            .runs
            .get_mut(run_id)
            .and_then(|run| run.active.remove(span_id))
        else {
            return;
        };
        let finished = ProjectedActivitySpan {
            start: active.start,
            ended_at,
            duration_ms,
            status,
            attributes: active.attributes,
        };
        safe_export(|| self.exporter.span_finished(finished));
    }

    fn with_active(
        &mut self,
        event: &AgentRunEventV1,
        span_id: &str,
        update: impl FnOnce(&mut ActiveSpan),
    ) {
        if let Some(span) = self
            .runs
            .get_mut(&event.run_id)
            .and_then(|run| run.active.get_mut(span_id))
        {
            update(span);
        }
    }

    fn record_usage(&mut self, event: &AgentRunEventV1, usage: &UsageV1) {
        for span_id in [run_span_id(event), turn_span_id(event)] {
            self.with_active(event, &span_id, |span| {
                add_usage(&mut span.attributes.usage, usage)
            });
        }

        // The canonical producer emits turn usage immediately after the model
        // attempt boundary. Keep a completed model span pending until that
        // required event arrives so model, turn, and run spans all carry the
        // same token accounting without reading provider call sites directly.
        let model_spans = self
            .runs
            .get(&event.run_id)
            .map(|run| {
                run.active
                    .iter()
                    .filter(|(_, span)| {
                        span.start.kind == ActivitySpanKind::ModelCall
                            && span.attributes.scope_id.as_deref() == Some(event.scope.id.as_str())
                            && span.attributes.turn == event.turn
                    })
                    .map(|(span_id, _)| span_id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for span_id in model_spans {
            let mut pending = None;
            self.with_active(event, &span_id, |span| {
                add_usage(&mut span.attributes.usage, usage);
                pending = span.pending_finish.take();
            });
            if let Some(pending) = pending {
                self.finish_at(
                    &event.run_id,
                    &span_id,
                    pending.ended_at,
                    pending.duration_ms,
                    pending.status,
                );
            }
        }
    }

    fn record_gap(&mut self, event: &AgentRunEventV1, gap: &temper_protocol_activity::TraceGapV1) {
        self.with_active(event, &run_span_id(event), |span| {
            span.attributes.dropped_events = span
                .attributes
                .dropped_events
                .saturating_add(gap.dropped_events);
            span.attributes.dropped_bytes = span
                .attributes
                .dropped_bytes
                .saturating_add(gap.dropped_bytes);
            for kind in &gap.kinds {
                let kind = match kind {
                    temper_protocol_activity::DroppedEventKindV1::TextDelta => "text_delta",
                    temper_protocol_activity::DroppedEventKindV1::ThinkingDelta => "thinking_delta",
                };
                if !span
                    .attributes
                    .dropped_kinds
                    .iter()
                    .any(|existing| existing == kind)
                {
                    span.attributes.dropped_kinds.push(kind.to_string());
                }
            }
        });
    }

    fn finish_model(
        &mut self,
        event: &AgentRunEventV1,
        model: &temper_protocol_activity::ModelCallFinishedV1,
    ) {
        let span_id = model_span_id(event, &model.call_id, model.attempt);
        self.with_active(event, &span_id, |span| {
            span.attributes.time_to_first_token_ms = model.time_to_first_token_ms;
            span.attributes.stop_reason = model.stop_reason.map(stop_reason);
            span.attributes.model_failure = model.failure.clone();
            span.pending_finish = Some(PendingFinish {
                ended_at: event.occurred_at.clone(),
                duration_ms: model.duration_ms,
                status: model_status(model.status, model.stop_reason),
            });
        });
    }

    fn finish_retry(
        &mut self,
        event: &AgentRunEventV1,
        retry: &temper_protocol_activity::ModelCallRetryingV1,
    ) {
        let attempt = retry.next_attempt.saturating_sub(1);
        let span_id = model_span_id(event, &retry.call_id, attempt);
        let mut pending = None;
        self.with_active(event, &span_id, |span| {
            span.attributes.retry_count = span.attributes.retry_count.saturating_add(1);
            span.attributes.retry_delay_ms = retry.delay_ms;
            pending = span.pending_finish.take();
        });
        if let Some(pending) = pending {
            self.finish_at(
                &event.run_id,
                &span_id,
                pending.ended_at,
                pending.duration_ms,
                pending.status,
            );
        }
        self.with_active(event, &run_span_id(event), |span| {
            span.attributes.retry_count = span.attributes.retry_count.saturating_add(1);
            span.attributes.retry_delay_ms = span
                .attributes
                .retry_delay_ms
                .saturating_add(retry.delay_ms);
        });
    }

    fn flush_pending(&mut self, event: &AgentRunEventV1, scope_id: Option<&str>) {
        let pending = self
            .runs
            .get(&event.run_id)
            .map(|run| {
                run.active
                    .iter()
                    .filter(|(_, span)| {
                        span.pending_finish.is_some()
                            && scope_id.is_none_or(|scope| {
                                span.attributes.scope_id.as_deref() == Some(scope)
                            })
                    })
                    .map(|(id, _)| id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for span_id in pending {
            let pending = self
                .runs
                .get_mut(&event.run_id)
                .and_then(|run| run.active.get_mut(&span_id))
                .and_then(|span| span.pending_finish.take());
            if let Some(pending) = pending {
                self.finish_at(
                    &event.run_id,
                    &span_id,
                    pending.ended_at,
                    pending.duration_ms,
                    pending.status,
                );
            }
        }
    }

    fn flush_children(&mut self, event: &AgentRunEventV1, status: ActivitySpanStatus) {
        self.flush_pending(event, None);
        let mut open = self
            .runs
            .get(&event.run_id)
            .map(|run| {
                run.active
                    .iter()
                    .filter(|(_, span)| span.start.kind != ActivitySpanKind::Run)
                    .map(|(id, span)| {
                        (
                            span.start.kind.close_rank(),
                            id.clone(),
                            span.started_elapsed_ms,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        open.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
        for (_, span_id, started_elapsed_ms) in open {
            self.finish_at(
                &event.run_id,
                &span_id,
                event.occurred_at.clone(),
                event.elapsed_ms.saturating_sub(started_elapsed_ms),
                status,
            );
        }
    }
}
