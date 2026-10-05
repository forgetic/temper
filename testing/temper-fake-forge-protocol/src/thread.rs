//! Server-side unpaged threads: one stored page and one encoded row, never a
//! complete history buffer. Client-side string skipping remains upstream work.
use crate::{config::Config, translate};
use alloc::boxed::Box;
use skein_lib::{Queue, Writer, bytes};
use temper_fake_forge_domain::{self as domain, api};
use temper_forge_forgejo::{
    Limits, response,
    types::{Document, ObjectFormat},
};
#[derive(Debug)]
pub enum Piece {
    Bytes(Box<[u8]>),
    Advanced,
    Finished,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Start,
    Rows,
    End,
    Finished,
}
#[derive(Debug)]
pub struct Thread {
    repository: Box<[u8]>,
    number: u64,
    since: u64,
    format: ObjectFormat,
    rows: Queue<api::Comment>,
    more: bool,
    after: u64,
    first: bool,
    stage: Stage,
}
impl Thread {
    pub fn new(
        repository: Box<[u8]>,
        number: u64,
        since: Option<u64>,
        format: ObjectFormat,
        comments: Box<[api::Comment]>,
        more: bool,
        limits: &Limits,
    ) -> Result<Thread, api::Error> {
        let mut rows = Queue::with_capacity(limits.page);
        if comments.len() > usize::try_from(limits.page).expect("u32 fits") {
            return Err(api::Error::TooLarge);
        }
        for comment in comments {
            if comment.body.len() > usize::try_from(limits.body_bytes).expect("u32 fits") {
                return Err(api::Error::TooLarge);
            }
            rows.push(comment);
        }
        Ok(Thread {
            repository,
            number,
            since: since.unwrap_or(0),
            format,
            rows,
            more,
            after: 0,
            first: true,
            stage: Stage::Start,
        })
    }
    pub fn next(
        &mut self,
        config: &Config,
        forge: &domain::Domain,
        settings: &domain::Config,
        limits: &Limits,
    ) -> Result<Piece, api::Error> {
        match self.stage {
            Stage::Start => {
                self.stage = Stage::Rows;
                Ok(Piece::Bytes(bytes::copy_of(b"[")))
            }
            Stage::Rows => {
                if let Some(comment) = self.rows.pop() {
                    if comment.id <= self.after {
                        return Err(api::Error::Refused);
                    }
                    self.after = comment.id;
                    if comment.edited.unwrap_or(comment.created).as_nanos() < self.since {
                        return Ok(Piece::Advanced);
                    }
                    let row = translate::comment(comment, &self.repository, self.number, config)?;
                    let Ok(encoded) = response::encode(&Document::Comment(row), self.format, limits) else {
                        return Err(api::Error::TooLarge);
                    };
                    let prefix = if self.first { b"".as_slice() } else { b",".as_slice() };
                    self.first = false;
                    let length = encoded.len().checked_add(prefix.len()).ok_or(api::Error::TooLarge)?;
                    let mut out = Writer::new(length);
                    out.put(prefix).expect("measured");
                    out.put(&encoded).expect("measured");
                    Ok(Piece::Bytes(out.finish()))
                } else if self.more {
                    // inspect owns the next page it copies. Admit its source
                    // bounds before asking for that temporary summary/page.
                    if settings.limits.page_size > limits.page
                        || settings.limits.body_bytes > limits.body_bytes
                        || settings.limits.title_bytes > limits.title_bytes
                        || settings.limits.name_bytes > limits.name_bytes
                        || settings.limits.labels > limits.fields
                    {
                        return Err(api::Error::TooLarge);
                    }
                    match forge.inspect(
                        settings,
                        &self.repository,
                        &api::Read::Item { number: self.number, after: self.after },
                    )? {
                        api::Answer::Item { comments, more, .. } => {
                            if comments.is_empty() && more {
                                return Err(api::Error::Refused);
                            }
                            if comments.len() > usize::try_from(limits.page).expect("u32 fits") {
                                return Err(api::Error::TooLarge);
                            }
                            self.more = more;
                            for comment in comments {
                                self.rows.push(comment);
                            }
                            Ok(Piece::Advanced)
                        }
                        api::Answer::Items { .. }
                        | api::Answer::Pull(_)
                        | api::Answer::Statuses(_)
                        | api::Answer::Permission(_)
                        | api::Answer::Comment { .. }
                        | api::Answer::Dependencies(_)
                        | api::Answer::Labels(_)
                        | api::Answer::Commit(_)
                        | api::Answer::Tree(_)
                        | api::Answer::File(_)
                        | api::Answer::Pages { .. }
                        | api::Answer::Page(_)
                        | api::Answer::Created(_)
                        | api::Answer::Commented(_)
                        | api::Answer::Reviewed(_)
                        | api::Answer::Merged(_)
                        | api::Answer::Revision(_)
                        | api::Answer::Done
                        | api::Answer::Cloned { .. }
                        | api::Answer::Pushed(_)
                        | api::Answer::PullFiles { .. }
                        | api::Answer::Comparison { .. }
                        | api::Answer::Checks(_)
                        | api::Answer::Protection(_)
                        | api::Answer::Settings(_)
                        | api::Answer::Collaborators(_)
                        | api::Answer::Branch(_) => Err(api::Error::Refused),
                    }
                } else {
                    self.stage = Stage::End;
                    Ok(Piece::Advanced)
                }
            }
            Stage::End => {
                self.stage = Stage::Finished;
                Ok(Piece::Bytes(bytes::copy_of(b"]")))
            }
            Stage::Finished => Ok(Piece::Finished),
        }
    }
}
