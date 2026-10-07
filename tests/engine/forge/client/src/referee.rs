//! Assertions are independent of client state: planned effects authorize
//! emitted writes; fake observations prove keyed creations happened once.
use crate::translate;
use skein_lib::Map;
use temper_engine_domain_forge_client::{Entry, api};
use temper_fake_forge_domain::Observation;
#[derive(Debug)]
pub(crate) struct Referee {
    keys: Map<Box<[u8]>, u64>,
    delivered: Map<News, ()>,
    pub creations: u32,
}
impl Referee {
    pub(crate) fn new() -> Referee {
        Referee { keys: Map::with_capacity(64), delivered: Map::with_capacity(128), creations: 0 }
    }
    pub(crate) fn write(planned: &Map<u64, Entry>, durable: &Map<u64, Entry>, write: &api::Write) {
        let mut authorized = false;
        for (_, entry) in planned {
            if entry.effect.write != *write {
                continue;
            }
            let saved = durable.get(&entry.number).expect("top entry durable before write is released");
            assert_eq!(saved.effect, entry.effect);
            assert!(
                saved.start.is_some() && saved.attempt.is_some(),
                "first position and lifetime are durable before HTTP submission"
            );
            authorized = true;
        }
        assert!(authorized, "every submitted write was planned by the parent");
    }
    pub(crate) fn news(&mut self, answer: &api::Answer) {
        match answer {
            api::Answer::Item { comments, .. } => {
                for comment in comments {
                    self.delivered(News::Comment(comment.id, comment.revision));
                }
            }
            api::Answer::Reviews { reviews, .. } => {
                for review in reviews {
                    self.delivered(News::Review(review.id, review.revision));
                }
            }
            api::Answer::Items { .. }
            | api::Answer::Pull(_)
            | api::Answer::Statuses { .. }
            | api::Answer::Remarks { .. }
            | api::Answer::Commit(_)
            | api::Answer::Branches(_)
            | api::Answer::PullFiles { .. }
            | api::Answer::Compare { .. }
            | api::Answer::Checks(_)
            | api::Answer::Job { .. }
            | api::Answer::Protection(_)
            | api::Answer::Settings(_)
            | api::Answer::Collaborators { .. }
            | api::Answer::Permission(_)
            | api::Answer::Created(_)
            | api::Answer::Commented(_)
            | api::Answer::Reviewed(_)
            | api::Answer::Merged(_)
            | api::Answer::Branch(_)
            | api::Answer::Done => {}
        }
    }
    fn delivered(&mut self, news: News) {
        let old = self.delivered.insert(news, ()).expect("world delivery observation cap");
        assert!(old.is_none(), "every observable inbox version is delivered once across restart cuts");
    }
    pub(crate) fn observe(&mut self, observation: &Observation) {
        let (id, body, by) = match observation {
            Observation::Opened { number, body, by, .. } => (*number, body, *by),
            Observation::Commented { id, body, by, .. } | Observation::Reviewed { id, body, by, .. } => {
                (*id, body, *by)
            }
            Observation::Moved { .. }
            | Observation::Deleted { .. }
            | Observation::Closed { .. }
            | Observation::Reopened { .. }
            | Observation::Labelled { .. }
            | Observation::Revised { .. }
            | Observation::Depends { .. }
            | Observation::Requested { .. }
            | Observation::Defined { .. }
            | Observation::Edited { .. }
            | Observation::Removed { .. }
            | Observation::Reported { .. }
            | Observation::Merged { .. }
            | Observation::Refused { .. }
            | Observation::Rejected { .. }
            | Observation::Wiki { .. } => return,
        };
        if by != 1 {
            return;
        }
        if let Some(key) = translate::key_of(body) {
            let old = self.keys.insert(key, id).expect("world keyed-effect limit");
            assert!(old.is_none(), "one creation per key, including late landings and restarts");
            self.creations = self.creations.saturating_add(1);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
enum News {
    Comment(u64, u64),
    Review(u64, u64),
}
