//! The signed-in person and navigation shared by every page.
use alloc::boxed::Box;
use skein_lib::List;

/// Secret-free identity shown in the frame.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Person {
    pub number: u64,
    pub name: Box<[u8]>,
}

/// A project the person may visit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Project {
    pub number: u32,
    pub name: Box<[u8]>,
}

/// Shared page header data, replaced by the person watch's snapshot.
#[derive(Debug)]
pub struct Frame {
    pub person: Option<Person>,
    pub projects: List<Project>,
    pub project: Option<u32>,
    pub inbox_count: u32,
}

impl Frame {
    pub(crate) fn new(projects: u32) -> Frame {
        Frame { person: None, projects: List::with_capacity(projects), project: None, inbox_count: 0 }
    }
}
