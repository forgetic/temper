// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

use crate::{
    Assign, CancelAttempts, ContextResponse, FetchContext, Heartbeat, JobResult, LeaseAck, Poll,
    ProtocolError, Register, Release, WorkerActivityAcknowledgement, WorkerActivityBatch,
};

pub const WORKER_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum WorkerProtocolMessage {
    Register(Register),
    Poll(Poll),
    Assign(Assign),
    Heartbeat(Heartbeat),
    CancelAttempts(CancelAttempts),
    Result(JobResult),
    Release(Release),
    LeaseAck(LeaseAck),
    FetchContext(FetchContext),
    ContextResponse(ContextResponse),
    ActivityBatch(WorkerActivityBatch),
    ActivityAck(WorkerActivityAcknowledgement),
    Error(ProtocolError),
}
