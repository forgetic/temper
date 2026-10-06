//! One browser tab: domain, view, storage, address and model DOM.
use crate::model::Model;
use crate::world::Settings;
use skein_lib::{Env, Queue, Time, Wall};
use temper_web_domain::{Action, Address, Domain, Event, Offset, Request, Saved};
use temper_web_view::{DomEvent, Patch, View, decode, render};

#[derive(Debug)]
pub struct Tab {
    pub domain: Domain,
    pub view: View,
    pub model: Model,
    pub saved: Option<Saved>,
    pub address: Address,
    pub epoch: u64,
    pub history: Vec<Address>,
    pub history_at: usize,
}

impl Tab {
    #[must_use]
    pub fn new(settings: &Settings, seed: u64, address: Address) -> Tab {
        Tab {
            domain: Domain::new(&settings.domain, seed),
            view: View::new(&settings.view),
            model: Model::new(),
            saved: None,
            address,
            epoch: 0,
            history: vec![address],
            history_at: 0,
        }
    }

    fn env(settings: &Settings, now: Time) -> Env<temper_web_domain::Limits> {
        Env { now, wall: Wall::from_nanos(now.as_nanos()), limits: settings.domain }
    }

    pub fn start(&mut self, settings: &Settings, now: Time) -> Vec<Request> {
        self.step(settings, now, Event::Start { address: self.address, saved: self.saved.clone(), offset: Offset(0) })
    }

    pub fn step(&mut self, settings: &Settings, now: Time, event: Event) -> Vec<Request> {
        let mut out = Queue::with_capacity(temper_web_domain::max_out(&settings.domain));
        temper_web_domain::step(&mut self.domain, &Self::env(settings, now), event, &mut out);
        self.domain.reclaim();
        self.render(settings);
        std::iter::from_fn(|| out.pop()).collect()
    }

    pub fn fire(&mut self, settings: &Settings, now: Time) -> Vec<Request> {
        let mut out = Queue::with_capacity(temper_web_domain::max_out(&settings.domain));
        temper_web_domain::fire(&mut self.domain, &Self::env(settings, now), &mut out);
        self.domain.reclaim();
        self.render(settings);
        std::iter::from_fn(|| out.pop()).collect()
    }

    pub fn render(&mut self, settings: &Settings) {
        let mut patches = Queue::<Patch>::with_capacity(settings.view.patches);
        render(&mut self.view, &self.domain, &settings.view, &mut patches);
        while let Some(patch) = patches.pop() {
            self.model.apply(patch);
        }
        self.model.check(self.view.tree());
    }

    pub fn event(&mut self, settings: &Settings, now: Time, event: DomEvent) -> Vec<Request> {
        if let DomEvent::Input { node, text } = &event
            && self.view.tree().find(*node).is_some()
        {
            self.model.input(*node, text);
        }
        let Some(action) = decode(&self.view, event) else {
            return Vec::new();
        };
        self.step(settings, now, Event::Act { action })
    }

    pub fn go(&mut self, settings: &Settings, now: Time, address: Address) -> Vec<Request> {
        self.step(settings, now, Event::Act { action: Action::Go { address } })
    }

    pub fn reload(&mut self, settings: &Settings, seed: u64, now: Time) -> Vec<Request> {
        let saved = self.saved.clone();
        let address = self.address;
        let epoch = self.epoch + 1;
        let history = self.history.clone();
        let history_at = self.history_at;
        *self = Tab::new(settings, seed, address);
        self.saved = saved;
        self.epoch = epoch;
        self.history = history;
        self.history_at = history_at;
        self.start(settings, now)
    }

    pub fn address(&mut self, address: Address, push: bool) {
        self.address = address;
        if push {
            self.history.truncate(self.history_at + 1);
            self.history.push(address);
            self.history_at += 1;
        } else {
            self.history[self.history_at] = address;
        }
    }
}
