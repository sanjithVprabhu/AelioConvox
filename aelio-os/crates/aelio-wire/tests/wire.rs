use aelio_sol::SolValue;
use aelio_wire::{
    decode_length_prefixed, encode_server_frame, negotiate, negotiate_version,
    parse_client_payload, ClientFrame, DeliveryBook, ExpiryDisposition, Protocol,
    ResultDisposition, ServerFrame, ToolOutcome, PROTOCOL_MAJOR,
};

fn call(corr: &str, effect_class: &str) -> ServerFrame {
    ServerFrame::ToolCall {
        corr: corr.into(),
        target: "tool.send@1".into(),
        effect_class: effect_class.into(),
        args: SolValue::Map(Default::default()),
        deadline_ms: 1_000,
        idem_key: Some("idem".into()),
        turn_ref: "turn-1".into(),
    }
}

#[test]
fn handshake_refuses_unknown_major_and_negotiates_minor() {
    let hello = parse_client_payload(
        br#"{"frame":"hello","protocol":{"major":99,"minor":0},"sdk_version":"1","deployer_id":"d","auth":"token"}"#,
    )
    .unwrap();
    assert!(matches!(
        negotiate(&hello, "c1"),
        ServerFrame::HelloReject { .. }
    ));
    let supported = ClientFrame::Hello {
        protocol: Protocol {
            major: PROTOCOL_MAJOR,
            minor: 99,
        },
        sdk_version: "1".into(),
        deployer_id: "d".into(),
        auth: "token".into(),
        last_corr_acked: None,
    };
    assert!(matches!(
        negotiate(&supported, "c1"),
        ServerFrame::HelloAck {
            protocol: Protocol { minor: 0, .. },
            ..
        }
    ));
    assert_eq!(
        negotiate_version(
            Protocol { major: 1, minor: 2 },
            Protocol { major: 1, minor: 4 }
        ),
        Some(Protocol { major: 1, minor: 2 })
    );
    assert_eq!(
        negotiate_version(
            Protocol { major: 2, minor: 0 },
            Protocol { major: 1, minor: 4 }
        ),
        None
    );
}

#[test]
fn frames_are_closed_and_tool_result_outcomes_are_exclusive() {
    assert!(parse_client_payload(br#"{"frame":"ack","corr":"c","unexpected":true}"#).is_err());
    assert!(parse_client_payload(
        br#"{"frame":"tool_result","corr":"c","outcome":"ok","output":{},"err":{},"duration_ms":1}"#
    )
    .is_err());
    assert!(matches!(
        parse_client_payload(
            br#"{"frame":"tool_result","corr":"c","outcome":"ok","output":{"sent":true},"duration_ms":1}"#
        )
        .unwrap(),
        ClientFrame::ToolResult {
            outcome: ToolOutcome::Ok(_),
            ..
        }
    ));
}

#[test]
fn server_encoding_is_canonical_and_length_prefixed() {
    let encoded = encode_server_frame(&ServerFrame::ToolCall {
        corr: "corr-1".into(),
        target: "tool.send@1".into(),
        effect_class: "external".into(),
        args: SolValue::map([("phone", SolValue::str("masked"))]),
        deadline_ms: 1_000,
        idem_key: Some("idem".into()),
        turn_ref: "turn-1".into(),
    })
    .unwrap();
    let payload = decode_length_prefixed(&encoded).unwrap();
    let text = std::str::from_utf8(payload).unwrap();
    assert!(text.starts_with("{\"args\":"));
    assert!(decode_length_prefixed(&encoded[..encoded.len() - 1]).is_err());
}

#[test]
fn delivery_reconnects_deduplicate_and_classify_expiry_safely() {
    let mut book = DeliveryBook::default();
    book.prepare(call("write", "write"), 100).unwrap();
    book.prepare(call("read", "read"), 100).unwrap();
    book.prepare(call("never-sent", "external"), 100).unwrap();

    book.mark_dispatched("write").unwrap();
    book.acknowledge("write").unwrap();
    book.mark_dispatched("read").unwrap();
    assert_eq!(book.unresolved(99).len(), 3);

    assert_eq!(
        book.expire("write", 100).unwrap(),
        Some(ExpiryDisposition::InternalUnknownOutcome)
    );
    assert_eq!(
        book.expire("read", 100).unwrap(),
        Some(ExpiryDisposition::ToolTransient)
    );
    assert_eq!(
        book.expire("never-sent", 100).unwrap(),
        Some(ExpiryDisposition::ToolTransient)
    );
    assert!(book.unresolved(100).is_empty());
    assert_eq!(book.accept_result("write"), ResultDisposition::Late);

    book.prepare(call("ok", "read"), 200).unwrap();
    book.mark_dispatched("ok").unwrap();
    assert_eq!(book.accept_result("ok"), ResultDisposition::Accepted);
    assert_eq!(book.accept_result("ok"), ResultDisposition::Duplicate);
    book.remove_terminal("ok").unwrap();
    assert_eq!(
        book.accept_result("ok"),
        ResultDisposition::UnknownCorrelation
    );
    assert_eq!(
        book.accept_result("not-known"),
        ResultDisposition::UnknownCorrelation
    );

    book.prepare(call("cancel", "external"), 300).unwrap();
    book.cancel_prepared("cancel").unwrap();
    assert_eq!(
        book.accept_result("cancel"),
        ResultDisposition::UnknownCorrelation
    );
    book.prepare(call("cannot-cancel", "external"), 300)
        .unwrap();
    book.mark_dispatched("cannot-cancel").unwrap();
    assert!(book.cancel_prepared("cannot-cancel").is_err());
}
