// SPDX-License-Identifier: MPL-2.0

//! Verified webhook intake and conversion into bounded wake requests.

use std::collections::{BTreeMap, BTreeSet};

use temper_engine_io::http::{HttpRequestData, HttpResponder, HttpResponseData};

use crate::webhook::{
    WebhookDisposition, WebhookError, parse_verified_webhook, webhook_accepted_log_line,
    webhook_suppressed_log_line,
};

use super::machine::{DaemonMachine, DaemonRequest};
use super::wake_coordinator::{WakeLane, WakeRequest};

impl DaemonMachine {
    pub(super) fn handle_webhook_delivery(
        &mut self,
        request: &HttpRequestData,
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        let config = self
            .webhook
            .as_ref()
            .expect("webhook config checked")
            .clone();
        let headers: BTreeMap<String, String> = request
            .headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
            .collect();

        match parse_verified_webhook(&headers, &request.body, &config.secret) {
            Ok(verified) => {
                let mut requests = vec![
                    DaemonRequest::Log(webhook_accepted_log_line(&verified.hint)),
                    DaemonRequest::Respond {
                        responder,
                        response: HttpResponseData::status_only(202),
                    },
                ];
                match verified.disposition {
                    WebhookDisposition::Schedule => {
                        let unresolved_lanes = config
                            .targets
                            .iter()
                            .map(|target| WakeLane::Role(target.role.clone()))
                            .collect::<BTreeSet<_>>();
                        if !unresolved_lanes.is_empty() {
                            let configured_repository_limit = config
                                .targets
                                .iter()
                                .map(|target| target.repo.as_str())
                                .collect::<BTreeSet<_>>()
                                .len();
                            self.wake_coordinator.configure_unresolved_repositories(
                                unresolved_lanes,
                                configured_repository_limit,
                            );
                        }
                        let lanes = config
                            .targets
                            .iter()
                            .filter(|target| target.path == verified.hint.repo)
                            .map(|target| WakeLane::Role(target.role.clone()))
                            .collect::<BTreeSet<_>>();
                        if !lanes.is_empty() {
                            self.wake_coordinator
                                .configure_repository(verified.hint.repo.clone(), lanes);
                        }
                        requests.extend(self.schedule_wake(WakeRequest::from_hint(verified.hint)));
                    }
                    WebhookDisposition::SuppressHeartbeat => requests.push(DaemonRequest::Log(
                        webhook_suppressed_log_line(&verified.hint),
                    )),
                }
                requests
            }
            Err(WebhookError::InvalidSignature) => vec![DaemonRequest::Respond {
                responder,
                response: HttpResponseData::status_only(401),
            }],
            Err(WebhookError::BadPayload(_)) => vec![DaemonRequest::Respond {
                responder,
                response: HttpResponseData::status_only(400),
            }],
        }
    }
}
