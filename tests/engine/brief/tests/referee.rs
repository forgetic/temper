use skein_lib::Token;
use temper_engine_brief_world::referee;
use temper_engine_domain_brief::{Core, GatherPlaced, GatherRequest};

#[test]
fn a_completed_brief_over_its_budget_fails_the_referee() {
    let wrong = [GatherRequest::Complete {
        brief: Token::new(1),
        order: Box::new([
            GatherPlaced::Core { kind: Core::Task, text: b"long task".as_slice().into() },
            GatherPlaced::Connector { connector: 3, kind: 7, token: Token::new(2), size: 8 },
        ]),
    }];
    assert!(!referee::within_budget(&wrong, 10));
}
