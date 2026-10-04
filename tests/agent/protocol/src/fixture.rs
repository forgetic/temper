//! Small fixed bounds and loopback descriptors, never deployment identity.
use skein_lib::Duration;
use temper_agent_protocol::Limits;
use temper_channel::wire::{Address, EndpointDescriptor, Provider};

#[must_use]
pub fn limits() -> Limits {
    Limits {
        calls: 2,
        endpoints: 4,
        accounts: 2,
        token_bytes: 256,
        name_bytes: 128,
        request_bytes: 16_384,
        head_bytes: 1024,
        headers: 24,
        event_bytes: 16_384,
        string_bytes: 8192,
        answer_bytes: 4096,
        parts: 64,
        input_bytes: 1024,
        opaque_bytes: 1024,
        error_bytes: 1024,
        detail_bytes: 128,
        render_bytes: 2048,
        depth: 16,
        tokens: 2048,
        chunk: 128,
        connect: Duration::from_nanos(1_000_000_000),
        handshake: Duration::from_nanos(1_000_000_000),
        head: Duration::from_nanos(1_000_000_000),
        idle: Duration::from_nanos(1_000_000_000),
        keep_idle: Duration::from_nanos(1_000_000_000),
        skew: Duration::ZERO,
    }
}
#[must_use]
pub fn endpoint(provider: Provider) -> EndpointDescriptor {
    EndpointDescriptor {
        endpoint: 0,
        provider,
        host: b"localhost".as_slice().into(),
        address: Address::V4 { bytes: [127, 0, 0, 1] },
        port: 8000,
        path: b"/responses".as_slice().into(),
        account: 0,
        effort: Box::new([]),
        thinking: None,
    }
}
