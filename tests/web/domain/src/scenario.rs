//! Data the scripted engine begins with.
use skein_lib::Wall;
use temper_web_domain::{ChatLine, Chip, Escalation, HoldReason, Offers, Person, Project, TaskPhase, TaskSnapshot};

#[derive(Clone, Debug)]
pub struct Scenario {
    pub signed_in: bool,
    pub person: Person,
    pub projects: Vec<Project>,
    pub chats: Vec<ChatLine>,
    pub tasks: Vec<TaskSnapshot>,
}

impl Default for Scenario {
    fn default() -> Self {
        Scenario {
            signed_in: true,
            person: Person { number: 1, name: Box::from(b"Ada".as_slice()) },
            projects: vec![Project { number: 7, name: Box::from(b"Temper".as_slice()) }],
            chats: Vec::new(),
            tasks: Vec::new(),
        }
    }
}

impl Scenario {
    #[must_use]
    pub fn held(task: u64, title: &[u8]) -> Scenario {
        let mut scenario = Scenario::default();
        scenario.chats.push(ChatLine {
            task,
            project: 7,
            title: Box::from(title),
            live: true,
            last_activity: Wall::EPOCH,
        });
        scenario.tasks.push(TaskSnapshot {
            chip: Chip {
                task,
                project: 7,
                title: Box::from(title),
                phase: TaskPhase::Held(HoldReason::Tries),
                spent: 0,
                budget: 100,
                revision: 1,
            },
            first_words: Box::from(title),
            escalation: Some(Escalation {
                task,
                reason: HoldReason::Tries,
                waiting_since: Wall::EPOCH,
                offers: Offers { release: true, leave_held: true, pass_up: false },
                revision: 1,
            }),
            result: None,
        });
        scenario
    }
}
