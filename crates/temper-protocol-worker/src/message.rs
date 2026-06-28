// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

use crate::{Assign, Heartbeat, JobResult, LeaseAck, Poll, ProtocolError, Register, Release};

pub const WORKER_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum WorkerProtocolMessage {
    Register(Register),
    Poll(Poll),
    Assign(Assign),
    Heartbeat(Heartbeat),
    Result(JobResult),
    Release(Release),
    LeaseAck(LeaseAck),
    Error(ProtocolError),
}
