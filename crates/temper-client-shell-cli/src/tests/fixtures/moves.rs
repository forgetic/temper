//! The fake engine scripts answers; no move reaches a real engine.
use super::*;
pub const RECORD: &[u8] = b"JIGM\0\x01opaque-fake-moved-secret\n";
pub const ACCOUNT: u32 = 7;
pub const NUMBER: u32 = 83;
pub enum Answer {
    Moving(&'static [u8]),
    Done,
    MovedIn(bool),
    Refused(w::AccountMoveWhy),
    Lost,
    Unsent,
}
impl Frame {
    pub(super) fn deliver_move_accounts(&mut self, watch: u64, subject: &w::Subject) {
        let value = match subject {
            w::Subject::Accounts => w::DeliveryValue::OwnerAccounts(jig_client_test_records::accounts_record(None)),
            w::Subject::Deployment => {
                w::DeliveryValue::OwnerDeployment(jig_client_test_records::owner_deployment_record())
            }
            w::Subject::Me
            | w::Subject::Task(_)
            | w::Subject::Tree(_)
            | w::Subject::Chips(_)
            | w::Subject::Inbox(_)
            | w::Subject::Tasks(_)
            | w::Subject::Run(_)
            | w::Subject::Settings => panic!("move watches only accounts and deployment"),
        };
        let document = w::Owned::new(
            &w::CEILINGS,
            w::OwnedParts { owner: w::Owner::Jig, version: w::VERSION, record: encode!(value) },
        )
        .expect("accounts");
        let event = w::StreamEvent::Delivery(
            w::Delivery::new(&w::CEILINGS, w::DeliveryParts { watch, document }).expect("delivery"),
        );
        self.sockets.get_mut(&self.stream.expect("stream")).expect("stream socket").reply.extend(sse(&event));
    }
    pub(super) fn move_reply(&mut self, fd: Fd, keyed: &w::Keyed) {
        let ask = w::Ask::decode(&w::CEILINGS, &mut skein_lib::Reader::new(keyed.request())).expect("ask");
        let w::Ask::AccountChange(change) = ask else { panic!("account change") };
        self.file_steps.push(match change.change() {
            w::AccountChange::Written(_) => "written",
            w::AccountChange::MoveOut(_) => "move-out",
            w::AccountChange::MoveIn(_) => "move-in",
            w::AccountChange::FinishSignIn(_)
            | w::AccountChange::AddKey(_)
            | w::AccountChange::ReplaceKey(_)
            | w::AccountChange::RemoveKey(_)
            | w::AccountChange::Resume(_)
            | w::AccountChange::Remove(_)
            | w::AccountChange::MoveAccount(_)
            | w::AccountChange::MoveSource(_)
            | w::AccountChange::SetUses(_)
            | w::AccountChange::DeclareProgram(_)
            | w::AccountChange::CheckProgram(_) => panic!("move request"),
        });
        self.move_requests.push((change.holder().clone(), change.change().clone()));
        let Some(answer) = self.move_answers.pop_front() else { return };
        match answer {
            Answer::Lost => {
                self.sockets.get_mut(&fd).expect("lost socket").eof = Some(());
                return;
            }
            Answer::Unsent => {
                self.reply(fd, &encode!(w::KeyedAnswer::NotReady));
                return;
            }
            Answer::Moving(_) | Answer::Done | Answer::MovedIn(_) | Answer::Refused(_) => {}
        }
        let answer = final_answer(answer);
        let owned = w::Owned::new(
            &w::CEILINGS,
            w::OwnedParts { owner: w::Owner::Jig, version: w::VERSION, record: encode!(answer) },
        )
        .expect("answer");
        self.reply(fd, &encode!(w::KeyedAnswer::FinalAnswer(owned)));
    }
    pub fn no_record_leaked(&self) {
        for bytes in [self.stdout(), self.stderr()].into_iter().chain(self.store_history.iter().map(Vec::as_slice)) {
            assert!(skein_lib::bytes::find(bytes, b"opaque-fake-moved-secret").is_none());
            assert!(skein_lib::bytes::find(bytes, RECORD).is_none());
        }
    }
    pub(super) fn record_write(&mut self, fd: Fd, bytes: &[u8], from: u32, at: u64) -> Result<(), Error> {
        if Some(fd) == self.record_fd {
            self.file_steps.push("write");
            assert_eq!(at, 0);
            if self.record_fault == Some("write") {
                return Err(Error::NoSpace);
            }
            self.record_data.extend_from_slice(&bytes[from as usize..]);
        }
        Ok(())
    }
    pub(super) fn record_sync(&mut self, fd: Fd) -> Result<Done, Error> {
        let step = if Some(fd) == self.record_fd { "sync-file" } else { "sync-directory" };
        self.file_steps.push(step);
        if self.record_fault == Some(step) { Err(Error::NoSpace) } else { Ok(Done::Nothing) }
    }
    pub(super) fn record_remove(&mut self, dir: Fd) -> Result<Done, Error> {
        assert_eq!(dir, FILE_PARENT);
        self.file_steps.push("remove");
        if self.record_fault == Some("remove") {
            return Err(Error::Permission);
        }
        self.record_exists = false;
        self.record_removed = true;
        Ok(Done::Nothing)
    }
}
fn final_answer(answer: Answer) -> w::Final {
    match answer {
        Answer::Lost | Answer::Unsent => unreachable!("transport answers handled"),
        Answer::Moving(bytes) => w::Final::Done(
            w::Done::new(
                &w::CEILINGS,
                w::DoneParts {
                    outcome: w::Outcome::Made(
                        w::OutcomeMade::new(
                            &w::CEILINGS,
                            w::OutcomeMadeParts {
                                what: w::Made::MovingOut(
                                    w::MadeMovingOut::new(
                                        &w::CEILINGS,
                                        w::MadeMovingOutParts {
                                            account: ACCOUNT,
                                            number: NUMBER,
                                            record: skein_lib::Secret::new(bytes.into()),
                                        },
                                    )
                                    .expect("moving"),
                                ),
                            },
                        )
                        .expect("made"),
                    ),
                },
            )
            .expect("done"),
        ),
        Answer::Done => w::Final::Done(
            w::Done::new(
                &w::CEILINGS,
                w::DoneParts {
                    outcome: w::Outcome::Owner(
                        w::OutcomeOwner::new(
                            &w::CEILINGS,
                            w::OutcomeOwnerParts { value: w::OwnerOutcome::AccountChanged },
                        )
                        .expect("changed"),
                    ),
                },
            )
            .expect("done"),
        ),
        Answer::MovedIn(renewed) => w::Final::Done(
            w::Done::new(
                &w::CEILINGS,
                w::DoneParts {
                    outcome: w::Outcome::Owner(
                        w::OutcomeOwner::new(
                            &w::CEILINGS,
                            w::OutcomeOwnerParts {
                                value: w::OwnerOutcome::AccountMovedIn(
                                    w::OwnerOutcomeAccountMovedIn::new(
                                        &w::CEILINGS,
                                        w::OwnerOutcomeAccountMovedInParts { account: ACCOUNT, renewed },
                                    )
                                    .expect("moved in"),
                                ),
                            },
                        )
                        .expect("owner"),
                    ),
                },
            )
            .expect("done"),
        ),
        Answer::Refused(why) => w::Final::Refused(
            w::Refused::new(
                &w::CEILINGS,
                w::RefusedParts {
                    refusal: w::Refusal::AccountMove(
                        w::RefusalAccountMove::new(&w::CEILINGS, w::RefusalAccountMoveParts { why })
                            .expect("move refusal"),
                    ),
                    at: None,
                },
            )
            .expect("refused"),
        ),
    }
}
