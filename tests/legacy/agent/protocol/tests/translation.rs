use temper_channel::wire::Provider;
use temper_legacy_agent_domain::{llm, tools};
use temper_legacy_agent_protocol::{render, tools as grammar, translate};
use temper_legacy_agent_protocol_world::fixture;
use temper_legacy_llm_openai as openai;

const GRANTS: tools::Grants = tools::Grants { inspect: true, modify: true, shell: true };
fn decode(name: &[u8], input: &[u8]) -> llm::Decoded {
    grammar::decode(
        &Provider::OpenAi,
        GRANTS,
        &[llm::Served::Finish, llm::Served::SubAgent],
        name,
        input,
        false,
        &fixture::limits(),
    )
}
#[test]
fn strict_required_fields_duplicates_paths_and_numbers() {
    let bad = [
        (
            b"read".as_slice(),
            br#"{"path":"a","path":"b"}"#.as_slice(),
            llm::Problem::BadValue { field: b"path".as_slice().into() },
        ),
        (b"read", br#"{"path":""}"#, llm::Problem::BadValue { field: b"path".as_slice().into() }),
        (b"read", br#"{"path":"a//b"}"#, llm::Problem::BadValue { field: b"path".as_slice().into() }),
        (b"read", br#"{"path":"a\u0000b"}"#, llm::Problem::BadValue { field: b"path".as_slice().into() }),
        (b"read", br#"{"path":true}"#, llm::Problem::WrongType { field: b"path".as_slice().into() }),
        (b"read", b"{}", llm::Problem::Missing { field: b"path".as_slice().into() }),
        (b"read", br#"{"path":"a","skip":4294967296}"#, llm::Problem::BadValue { field: b"skip".as_slice().into() }),
        (
            b"shell",
            br#"{"command":"true","timeout":1.5}"#,
            llm::Problem::BadValue { field: b"timeout".as_slice().into() },
        ),
        (
            b"shell",
            br#"{"command":"true","timeout":"1"}"#,
            llm::Problem::WrongType { field: b"timeout".as_slice().into() },
        ),
        (
            b"shell",
            br#"{"command":"true","timeout":18446744073709551615}"#,
            llm::Problem::BadValue { field: b"timeout".as_slice().into() },
        ),
    ];
    for (name, input, problem) in bad {
        assert_eq!(decode(name, input), llm::Decoded::Invalid { problem }, "{input:?}");
    }
    assert_eq!(decode(b"unoffered", b"{"), llm::Decoded::Invalid { problem: llm::Problem::UnknownTool });
    assert_eq!(decode(b"read", b"[]"), llm::Decoded::Invalid { problem: llm::Problem::NotAnObject });
    let llm::Decoded::Owned { call: tools::Call::Read { skip, lines, path } } =
        decode(b"read", br#"{"path":"../a","skip":2,"lines":4,"future":{"deep":[true]}}"#)
    else {
        panic!("valid read");
    };
    assert_eq!((skip, lines, path.parts.len()), (2, Some(4), 2));
}
#[test]
fn offer_fences_names_and_oversized_input() {
    let limits = fixture::limits();
    let offer = grammar::offer(&Provider::OpenAi, GRANTS, &[llm::Served::Finish], &limits).expect("bounded schemas");
    assert_eq!(offer.len(), 7);
    for spec in offer {
        assert!(openai::Json::from_bytes(&spec.parameters, &limits.openai()).is_ok());
    }
    let none = tools::Grants { inspect: false, modify: false, shell: false };
    assert_eq!(
        grammar::decode(&Provider::OpenAi, none, &[], b"read", b"{}", false, &limits),
        llm::Decoded::Invalid { problem: llm::Problem::UnknownTool }
    );
    assert_eq!(
        grammar::decode(&Provider::OpenAi, GRANTS, &[], b"read", b"", true, &limits),
        llm::Decoded::Invalid { problem: llm::Problem::TooLarge }
    );
}
#[test]
fn typed_finish_and_subagent_preserve_declared_values() {
    assert_eq!(
        decode(b"finish", br#"{"title":"a","verdict":"report","body":"b"}"#),
        llm::Decoded::Invalid { problem: llm::Problem::BadValue { field: b"title".as_slice().into() } }
    );
    let llm::Decoded::Served { ask: temper_legacy_agent_domain::run::Ask::Finish { outcome: temper_legacy_agent_domain::run::outcome::Declared::Verdict(verdict) } } = decode(b"finish", br#"{"verdict":"request-changes","body":"Fix","children":[{"kind":"blocking","fields":{"path":"a","body":"b"}}]}"#) else { panic!("valid verdict"); };
    assert_eq!((verdict.children.len(), &*verdict.children[0].fields[0].value), (1, b"a".as_slice()));
    let llm::Decoded::Served {
        ask: temper_legacy_agent_domain::run::Ask::SubAgent { families, share: Some(share), .. },
    } = decode(b"subagent", br#"{"brief":"look","tools":["inspect"],"agents":true,"share":{"turns":3,"output":7}}"#)
    else {
        panic!("valid subagent");
    };
    assert!(families.tools.inspect && families.agents && !families.tools.modify);
    assert_eq!((share.turns, share.output, share.input), (3, 7, u64::MAX));
    assert_eq!(
        decode(b"subagent", br#"{"brief":"a","tools":["inspect","inspect"]}"#),
        llm::Decoded::Invalid { problem: llm::Problem::BadValue { field: b"tools".as_slice().into() } }
    );
}
#[test]
fn reads_number_lines_then_summarize_and_directories_have_slashes() {
    for (content, cut, expected) in [
        (b"a\nb\n".as_slice(), false, b"3: a\n4: b\nshown lines 3-4 of 9\n".as_slice()),
        (b"a\nb".as_slice(), true, b"3: a\n4: b\nshown lines 3-4 of 9 (last line cut)\n".as_slice()),
    ] {
        let returned = llm::Returned::Owned {
            outcome: tools::Outcome::Read { content: content.into(), skipped: 2, lines: 2, total: 9, cut },
        };
        assert_eq!(render::result(&returned, 128).expect("read rendered"), (expected.into(), false));
    }
    let listed = llm::Returned::Owned {
        outcome: tools::Outcome::Listed {
            entries: Box::new([
                tools::Entry {
                    name: tools::Name::new(b"src".as_slice().into()).expect("name"),
                    kind: tools::Kind::Directory,
                },
                tools::Entry { name: tools::Name::new(b"a".as_slice().into()).expect("name"), kind: tools::Kind::File },
            ]),
            more: 2,
        },
    };
    assert_eq!(&*render::result(&listed, 128).expect("listing rendered").0, b"src/\na\n[2 more entries]\n");
}
#[test]
fn arbitrary_diagnostics_and_cut_utf8_are_valid_json_text() {
    let returned = llm::Returned::Owned {
        outcome: tools::Outcome::Exited {
            exit: tools::Exit::Code { code: 0 },
            head: b"ok \xF0\x9F\x92\xA9\xFF".as_slice().into(),
            tail: b"\xE2\x82".as_slice().into(),
            dropped: 4,
        },
    };
    let (text, failed) = render::result(&returned, 128).expect("bounded rendering");
    assert!(!failed);
    assert_eq!(&*text, b"exit 0\nok \xF0\x9F\x92\xA9\\xFF\n[4 bytes omitted]\n\\xE2\\x82");
    assert!(std::str::from_utf8(&text).is_ok());
    assert!(render::result(&returned, 4).is_err());
}
fn prompt(content: Box<[llm::Block]>) -> llm::Prompt {
    llm::Prompt {
        endpoint: llm::Endpoint(0),
        model: b"fake".as_slice().into(),
        system: b"system".as_slice().into(),
        tools: GRANTS,
        served: Box::new([llm::Served::Finish]),
        messages: Box::new([
            llm::Message { role: llm::Role::Assistant, content },
            llm::Message {
                role: llm::Role::User,
                content: Box::new([llm::Block::Text { text: b"next".as_slice().into() }]),
            },
        ]),
        max_tokens: 100,
    }
}
#[test]
fn opaque_head_reasoning_and_raw_call_replay_in_position() {
    let limits = fixture::limits();
    let input = br#"{ "path" : "a", "future": 7 }"#;
    let content = Box::new([
        llm::Block::Opaque { bytes: br#"{"type":"reasoning","id":"r","encrypted_content":"proof"}"#.as_slice().into() },
        llm::Block::Opaque { bytes: br#"{"id":"m","phase":"commentary"}"#.as_slice().into() },
        llm::Block::Text { text: b"looking".as_slice().into() },
        llm::Block::ToolCall {
            id: b"call|item".as_slice().into(),
            name: b"read".as_slice().into(),
            input: input.as_slice().into(),
        },
    ]);
    let request = translate::openai(prompt(content), &fixture::endpoint(Provider::OpenAi), b"session", &limits)
        .expect("valid prompt");
    assert!(matches!(request.input[0], openai::Input::Opaque { .. }));
    let openai::Input::Message { id, phase, .. } = &request.input[1] else {
        panic!("message");
    };
    assert_eq!((id.as_deref(), phase.as_deref()), (Some(b"m".as_slice()), Some(b"commentary".as_slice())));
    let openai::Input::FunctionCall { call_id, item_id, arguments, .. } = &request.input[2] else {
        panic!("call");
    };
    assert_eq!(
        (&**call_id, item_id.as_deref(), &**arguments),
        (b"call".as_slice(), Some(b"item".as_slice()), input.as_slice())
    );
}
