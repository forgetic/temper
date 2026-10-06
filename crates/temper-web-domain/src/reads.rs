//! Paged reads and their page-generation ownership.
use crate::Query;

#[derive(Debug)]
pub(crate) struct ReadSlot {
    pub query: Query,
    pub generation: u32,
    pub abandoned: bool,
}
