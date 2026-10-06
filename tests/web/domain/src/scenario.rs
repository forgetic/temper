//! Data the scripted engine begins with.
use temper_web_domain::{ChatLine, Person, Project};

#[derive(Clone, Debug)]
pub struct Scenario {
    pub signed_in: bool,
    pub person: Person,
    pub projects: Vec<Project>,
    pub chats: Vec<ChatLine>,
}

impl Default for Scenario {
    fn default() -> Self {
        Scenario {
            signed_in: true,
            person: Person { number: 1, name: Box::from(b"Ada".as_slice()) },
            projects: vec![Project { number: 7, name: Box::from(b"Temper".as_slice()) }],
            chats: Vec::new(),
        }
    }
}
