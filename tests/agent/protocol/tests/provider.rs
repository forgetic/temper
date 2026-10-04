use skein_http::Header;
use skein_lib::{Duration, Time};
use temper_agent_domain::llm;
use temper_agent_protocol::exchange::Event;
use temper_agent_protocol_world::{
    http,
    provider::{self, World},
};
use temper_channel::wire::Provider;
use temper_fake_llm_domain::api;
use temper_fake_llm_protocol::{documents, provider as service};

fn answer(provider: &Provider) -> api::Answer {
    let opaque = match provider {
        Provider::Anthropic => {
            br#"{"type":"thinking","thinking":"private","signature":"signed","future":true}"#.as_slice()
        }
        Provider::OpenAi => {
            br#"{"id":"reasoning-1","type":"reasoning","encrypted_content":"signed","summary":[],"future":true}"#
                .as_slice()
        }
    };
    let read_name = match provider {
        Provider::Anthropic => temper_llm_anthropic::identity::READ_TOOL,
        Provider::OpenAi => temper_llm_openai::identity::READ_TOOL,
    };
    api::Answer {
        parts: Box::new([
            api::Part::Opaque { bytes: opaque.into() },
            api::Part::Text { text: b"hello".as_slice().into() },
            api::Part::ToolCall {
                id: b"call-1".as_slice().into(),
                name: read_name.into(),
                arguments: br#"{"path":"repo/a"}"#.as_slice().into(),
            },
        ]),
        finish: api::Finish::ToolCalls,
        usage: api::Usage { prompt_tokens: 3, cached_tokens: 2, cache_creation_tokens: 1, completion_tokens: 4 },
    }
}
#[test]
fn actual_provider_documents_stream_to_production_translation_with_slow_output() {
    for provider in [Provider::Anthropic, Provider::OpenAi] {
        for slice in [1, 17, 4096] {
            let mut world = World::new(provider.clone(), slice);
            world.drive();
            assert_eq!(world.queries.len(), 1);
            assert_eq!(world.queries[0].model.as_ref(), b"model");
            assert_eq!(world.queries[0].max_tokens, 100, "explicit fake model ceiling on Responses");
            assert!(!world.queries[0].tools.is_empty());
            world.ledger.room_enabled = false;
            world.answer(Ok(answer(&provider)));
            world.drive();
            assert!(!world.client.events.iter().any(|event| matches!(event, Event::Completed { .. })));
            world.ledger.room_enabled = true;
            world.drive();
            let completions: Vec<_> = world
                .client
                .events
                .iter()
                .filter_map(|event| match event {
                    Event::Completed { completion, .. } => Some(completion),
                    Event::Failed { .. } | Event::Close | Event::Closed { .. } | Event::Idle => None,
                })
                .collect();
            assert_eq!(completions.len(), 1);
            let completion = completions[0];
            assert_eq!(completion.stop, llm::Stop::ToolUse);
            assert!(
                matches!(&completion.content[0], llm::Said::Opaque { bytes } if bytes.windows(6).any(|s| s == b"signed"))
            );
            let (text, tool, id) = match provider {
                Provider::Anthropic => (1, 2, b"call-1".as_slice()),
                Provider::OpenAi => {
                    assert!(
                        matches!(&completion.content[1], llm::Said::Opaque { bytes } if bytes.windows(8).any(|s| s == b"item_1_1"))
                    );
                    (2, 3, b"call-1|item_1_2".as_slice())
                }
            };
            assert!(matches!(&completion.content[text], llm::Said::Text { text } if text.as_ref() == b"hello"));
            assert!(
                matches!(&completion.content[tool], llm::Said::ToolCall { id: name, call: llm::Decoded::Owned { .. }, .. } if name.as_ref() == id)
            );
            assert_eq!(completion.usage.input_tokens, 3);
            assert_eq!(completion.usage.cache_read_tokens, 2);
            assert_eq!(completion.usage.cache_write_tokens, u64::from(provider == Provider::Anthropic));
            assert!(world.client.exchange.is_idle());
            assert_eq!(world.service.count(), 1);
            world.close();
        }
    }
}
#[test]
fn same_server_validates_each_reused_request_and_issued_token_expiry() {
    for provider in [Provider::Anthropic, Provider::OpenAi] {
        let mut world = World::new(provider.clone(), 4096);
        world.drive();
        world.answer(Ok(answer(&provider)));
        world.drive();
        world.client.begin(http::prepared_value(provider.clone(), 2, provider::ACCESS, provider::ACCOUNT));
        world.drive();
        assert_eq!(world.service.count(), 2);
        assert_eq!(world.queries.len(), 2);
        world.answer(Ok(answer(&provider)));
        world.drive();
        assert_eq!(world.client.events.iter().filter(|e| matches!(e, Event::Completed { .. })).count(), 2);
        world.env.now = Time::from_nanos(61_000_000_000);
        world.client.env.now = world.env.now;
        world.client.begin(http::prepared_at(provider.clone(), 3, provider::ACCESS, provider::ACCOUNT, world.env.now));
        world.drive();
        assert_eq!(world.service.count(), 3);
        assert_eq!(world.queries.len(), 2, "expired bearer never reaches neutral domain");
        assert!(
            world.client.events.iter().any(|e| matches!(e, Event::Failed { failure: llm::Failure::Unauthorized, .. }))
        );
        world.close();
    }
}
#[test]
fn provider_specific_error_documents_preserve_exhaustion_and_context() {
    for provider in [Provider::Anthropic, Provider::OpenAi] {
        for (error, expected) in [
            (
                api::Error::Exhausted { retry_after: Duration::from_secs(9) },
                llm::Failure::Exhausted { retry_after: Duration::from_secs(9) },
            ),
            (api::Error::ContextTooLong, llm::Failure::ContextTooLong),
        ] {
            let mut world = World::new(provider.clone(), 1);
            world.drive();
            world.answer(Err(error));
            world.drive();
            assert!(
                world
                    .client
                    .events
                    .iter()
                    .any(|event| matches!(event, Event::Failed { failure, .. } if *failure == expected))
            );
            world.close();
        }
    }
}
#[test]
fn provider_config_rejects_unbounded_and_injected_headers_before_allocation() {
    let limits = provider::limits();
    for headers in [
        vec![Header { name: b"identity".as_slice().into(), value: vec![b'x'; limits.http.head as usize].into() }],
        vec![Header { name: b"identity\r\nInjected".as_slice().into(), value: b"x".as_slice().into() }],
        vec![Header { name: b"identity".as_slice().into(), value: b"x\r\nInjected: y".as_slice().into() }],
        vec![
            Header { name: b"identity".as_slice().into(), value: b"x".as_slice().into() },
            Header { name: b"IDENTITY".as_slice().into(), value: b"y".as_slice().into() },
        ],
    ] {
        assert!(
            service::Service::new(
                service::Config {
                    provider: documents::Provider::Anthropic,
                    path: b"/responses".as_slice().into(),
                    headers: headers.into()
                },
                &limits
            )
            .is_err()
        );
    }
    let issuer = provider::issued();
    assert!(issuer.authorize(provider::ACCESS, Time::ZERO));
    assert!(!issuer.authorize(provider::ACCESS, Time::from_nanos(60_000_000_000)));
}
