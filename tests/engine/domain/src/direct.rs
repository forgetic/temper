//! Direct root fixture shared by focused routes and the ending sweep.

use jig_core_accounts as accounts;
use jig_core_views as views;
use skein_lib::{Env, Queue, ReplyTo, Time, Token, Wall};
use std::collections::VecDeque;
use temper_engine_domain::{Delivery, Write, engine};
use temper_engine_domain_people as people;

use crate::commits::Store;
use crate::walking::{config, limits};

pub struct Driver {
    pub root: engine::Domain,
    pub env: Env<engine::Limits>,
    pub out: Queue<engine::Request>,
    pub events: VecDeque<engine::Event>,
    pub store: Store,
    pub delivered: Vec<Delivery>,
    pub viewed: Vec<views::Request>,
    pub serial: u64,
    pub accounts: Vec<accounts::Request>,
    pub stopped: bool,
    pub fail_archive_once: bool,
    pub transactions: Vec<Vec<Write>>,
    pub result_loads: Vec<(u32, usize)>,
    pub call_busy: Vec<Token>,
}

impl Driver {
    #[must_use]
    pub fn new(store: Store) -> Driver {
        Driver::configured(store, config(91), &limits())
    }

    #[must_use]
    pub fn configured(store: Store, config: engine::Config, limits: &engine::Limits) -> Driver {
        Driver {
            root: engine::Domain::new(config, limits),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: *limits },
            out: Queue::with_capacity(engine::max_out(limits)),
            events: VecDeque::from([engine::Event::Start]),
            store,
            delivered: Vec::new(),
            viewed: Vec::new(),
            serial: 100,
            accounts: Vec::new(),
            stopped: false,
            fail_archive_once: false,
            transactions: Vec::new(),
            result_loads: Vec::new(),
            call_busy: Vec::new(),
        }
    }

    pub fn send(&mut self, event: engine::Event) {
        engine::step(&mut self.root, &self.env, event);
        engine::release(&mut self.root, &self.env, &mut self.out);
        self.collect();
        self.root.reclaim();
    }

    pub fn collect(&mut self) {
        for _ in 0..self.out.len() {
            match self.out.pop().expect("output count") {
                engine::Request::Commit { number, writes } => {
                    self.transactions.push(writes.to_vec());
                    self.store.pending.push_back((number, writes));
                }
                engine::Request::Load { owner, range, after, most, bytes } => {
                    let (rows, next) = self.store.page(range, after, most);
                    if range == temper_engine_domain::Range::EndedResults {
                        self.result_loads.push((most, rows.len()));
                    }
                    assert!(rows.len() <= usize::try_from(most).expect("small count"));
                    assert!(bytes >= self.env.limits.loads.bytes);
                    if self.fail_archive_once && matches!(range, temper_engine_domain::Range::EscalationDecision { .. })
                    {
                        self.fail_archive_once = false;
                        self.events.push_back(engine::Event::Unloaded { owner });
                    } else {
                        self.events.push_back(engine::Event::Loaded { owner, rows, next });
                    }
                }
                engine::Request::Deliver(delivery) => self.delivered.push(delivery),
                engine::Request::View(request) => match request {
                    views::Request::Watching { .. }
                    | views::Request::Refused { .. }
                    | views::Request::Deliver { .. }
                    | views::Request::Ended { .. } => self.viewed.push(request),
                },
                engine::Request::WatchRefused { .. } => panic!("root route test did not request an invalid watch"),
                engine::Request::Account(request) => self.accounts.push(request),
                engine::Request::Forge { .. } => {
                    panic!("root route fixture did not adopt forge")
                }
                engine::Request::Stop => self.stopped = true,
                engine::Request::CallBusy { call, .. } => self.call_busy.push(call),
                engine::Request::TurnBusy { .. } | engine::Request::AnswerBusy { .. } => {
                    panic!("unexpected route refusal")
                }
            }
        }
    }

    pub fn advance(&mut self, apply: bool) {
        if let Some(event) = self.events.pop_front() {
            self.send(event);
        }
        if apply && !self.store.pending.is_empty() {
            let number = self.store.apply();
            self.send(engine::Event::Committed { number });
        }
        engine::release(&mut self.root, &self.env, &mut self.out);
        self.collect();
        self.root.reclaim();
    }

    pub fn settle(&mut self) {
        for _ in 0..200 {
            self.advance(true);
            if self.root.quiescent() && self.events.is_empty() && self.store.pending.is_empty() {
                return;
            }
        }
        panic!("direct root routes did not settle: {:?}", self.root);
    }

    pub fn sign_in(&mut self) {
        self.serial += 1;
        self.send(engine::Event::SignedIn {
            reply_to: ReplyTo::new(Token::new(self.serial)),
            identity: people::Identity {
                key: people::IdentityKey { provider: 0, subject: 7_u64.to_be_bytes().into() },
                login: b"person".as_slice().into(),
                name: b"Person".as_slice().into(),
            },
        });
    }

    #[must_use]
    pub fn session(&self) -> u64 {
        self.delivered
            .iter()
            .find_map(|delivery| match delivery {
                Delivery::WebReply { sign_in, reply: people::Reply::SignedIn { .. }, .. } => *sign_in,
                Delivery::WebReply { .. }
                | Delivery::Reply { .. }
                | Delivery::Acknowledge { .. }
                | Delivery::AcknowledgeTurn { .. }
                | Delivery::Cancel { .. }
                | Delivery::Result { .. }
                | Delivery::View(_)
                | Delivery::Fleet(_)
                | Delivery::Assigned { .. }
                | Delivery::Refuse { .. }
                | Delivery::EscalationReply { .. }
                | Delivery::ReadEscalationDecision { .. }
                | Delivery::ReadResult { .. }
                | Delivery::TurnBusy { .. }
                | Delivery::Relay { .. }
                | Delivery::Inbound { .. }
                | Delivery::Load { .. }
                | Delivery::ResultReply { .. }
                | Delivery::InboxPage { .. }
                | Delivery::InboxView { .. }
                | Delivery::BeginInboxView { .. }
                | Delivery::CallAnswer { .. }
                | Delivery::ForgeCommitted { .. }
                | Delivery::ForgeCall { .. }
                | Delivery::Procedure { .. } => None,
            })
            .expect("durable sign-in reply")
    }

    // Leave the newly durable callback behind three later unanswered commits.
    pub fn pressure(&mut self) {
        assert_eq!(self.store.pending.len(), 1);
        self.sign_in();
        self.sign_in();
        assert_eq!(self.store.pending.len(), 3);
        let number = self.store.apply();
        self.send(engine::Event::Committed { number });
        self.sign_in();
        assert_eq!(self.store.pending.len(), 3);
        for _ in 0..20 {
            self.advance(false);
        }
        assert!(!self.root.quiescent(), "owed callback and store terminals prevent completion");
        assert_eq!(self.store.pending.len(), 3, "no callback write entered the full journal");
    }
}
