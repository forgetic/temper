use jig_core::RequestView;

pub(super) fn assert_guidance(view: &RequestView, fragments: &[&str]) {
    for fragment in fragments {
        assert!(
            view.messages.iter().any(|message| {
                message.role == "user"
                    && message.content.contains("[Decision guidance:")
                    && message.content.contains(fragment)
            }),
            "decision guidance omitted {fragment:?}",
        );
    }
}
