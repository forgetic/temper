//! Neutral fields of Forgejo's v15 documents; timestamps are source metadata.
use alloc::boxed::Box;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObjectFormat {
    Sha1,
    Sha256,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Open,
    Closed,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemKind {
    Issue,
    Pull,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Check {
    Pending,
    Success,
    Error,
    Failure,
    Warning,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReviewState {
    Approved,
    RequestChanges,
    Comment,
    Pending,
    Dismissed,
    Requested,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Permission {
    None,
    Read,
    Write,
    Admin,
    Owner,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct User {
    pub id: u64,
    pub login: Box<[u8]>,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Label {
    pub id: u64,
    pub name: Box<[u8]>,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RepositoryInfo {
    pub id: u64,
    pub full_name: Box<[u8]>,
    pub object_format: ObjectFormat,
    pub has_wiki: bool,
    pub default_branch: Box<[u8]>,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Item {
    pub number: u64,
    pub kind: ItemKind,
    pub state: State,
    pub user: User,
    pub title: Box<[u8]>,
    pub body: Box<[u8]>,
    pub labels: Box<[Label]>,
    pub created: u64,
    pub updated: u64,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Comment {
    pub id: u64,
    pub user: User,
    pub body: Box<[u8]>,
    pub issue_url: Box<[u8]>,
    pub created: u64,
    pub updated: u64,
    pub revision: u64,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Pull {
    pub number: u64,
    pub state: State,
    pub head: Box<[u8]>,
    pub base: Box<[u8]>,
    pub commit: [u8; 32],
    pub base_commit: [u8; 32],
    pub merged: bool,
    pub merge_commit: Option<[u8; 32]>,
    pub mergeable: bool,
    pub reviewers: Box<[User]>,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Review {
    pub id: u64,
    pub user: User,
    pub state: ReviewState,
    pub commit: [u8; 32],
    pub body: Box<[u8]>,
    pub submitted: u64,
    pub official: bool,
    /// Kept apart from its verdict; dismissal policy belongs to the engine.
    pub dismissed: bool,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Status {
    pub context: Box<[u8]>,
    pub state: Check,
    pub creator: User,
    pub description: Box<[u8]>,
    pub target_url: Box<[u8]>,
    pub created: u64,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Remark {
    pub id: u64,
    pub user: User,
    pub body: Box<[u8]>,
    pub path: Box<[u8]>,
    pub diff_hunk: Box<[u8]>,
    pub position: u64,
    pub commit: [u8; 32],
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WikiPage {
    pub title: Box<[u8]>,
    pub content: Option<Box<[u8]>>,
    pub sha: [u8; 32],
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Dependency {
    pub number: u64,
    pub owner: Box<[u8]>,
    pub repository: Box<[u8]>,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Document {
    Settings { max_response_items: u32, default_paging_num: u32 },
    User(User),
    Users(Box<[User]>),
    Repository(RepositoryInfo),
    Labels(Box<[Label]>),
    Item(Item),
    Items(Box<[Item]>),
    Comment(Comment),
    Comments(Box<[Comment]>),
    Pull(Pull),
    Review(Review),
    Reviews(Box<[Review]>),
    Statuses { state: Check, total_count: u64, statuses: Box<[Status]> },
    Remarks(Box<[Remark]>),
    Permission(Permission),
    Branch([u8; 32]),
    Pages(Box<[WikiPage]>),
    Page(WikiPage),
    Dependencies(Box<[Dependency]>),
    Error { message: Box<[u8]> },
    Done,
}
