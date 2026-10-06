use super::*;
use skein_lib::{Duration, Time};
use temper_web_view::NodeId;

struct FaceNow(Found);
impl Face for FaceNow {
    fn find(&mut self, _: &Find) -> Found {
        self.0
    }
}

#[test]
fn scripted_person_waits_and_times_out() {
    let mut person = Person::new(Box::from([Step::See {
        find: Find::named(Role::Button, b"Go"),
        within: Duration::from_millis(10),
    }]));
    let mut face = FaceNow(Found::Absent);
    assert_eq!(person.next(&mut face, Time::ZERO), Next::Wait { until: Some(Time::from_nanos(10_000_000)) });
    face.0 = Found::Node(NodeId(9));
    assert_eq!(person.next(&mut face, Time::from_nanos(9_000_000)), Next::Wait { until: None });
    assert_eq!(person.next(&mut face, Time::from_nanos(10_000_000)), Next::Done);
}

#[test]
fn absent_press_fails_at_its_step() {
    let mut person = Person::new(Box::from([Step::Press { find: Find::named(Role::Button, b"Go") }]));
    assert_eq!(person.next(&mut FaceNow(Found::Absent), Time::ZERO), Next::Failed { step: 0, why: Why::Missing });
}
