//! The view's closed vocabulary and preorder tree (02-view.md, section 3).

use alloc::boxed::Box;
use skein_lib::List;
use temper_web_domain::{Address, ObjectKey};

use crate::binding::Binding;

/// A stable id for a node while the diff matches it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NodeId(pub u32);

/// A row's identity among siblings.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum NodeKey {
    Chat(u64),
    Object(ObjectKey),
}

/// Heading level.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    One,
    Two,
    Three,
}

/// Input kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputKind {
    Text,
    Number,
}

/// Elements the shell may create.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Element {
    Main,
    Header,
    Nav,
    Section,
    Article,
    Aside,
    Footer,
    Div,
    Span,
    Heading(Level),
    Paragraph,
    List,
    OrderedList,
    Item,
    Link,
    Button,
    Form,
    Label,
    TextArea,
    Input(InputKind),
    Radio,
    Dialog,
    Details,
    Summary,
    Strong,
    Emphasis,
    Code,
    Pre,
    Quote,
    Time,
    Progress,
    Text,
}

/// Accessible roles queried by a person.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Button,
    Link,
    Dialog,
    Status,
    Alert,
    Navigation,
    Region,
    Log,
    List,
    ListItem,
    Tree,
    TreeItem,
    TabList,
    Tab,
    TextBox,
    Radio,
    RadioGroup,
    Group,
}

/// Stylesheet classes in the first slice's frame and chats page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Topbar,
    Brand,
    Project,
    Navigation,
    Count,
    LinkState,
    User,
    Page,
    Narrow,
    Welcome,
    SectionHeading,
    Composer,
    ChatList,
    ChatPreview,
    Card,
    Meta,
    Tag,
    Live,
    Closed,
    Pending,
    Notice,
    Primary,
    ButtonRow,
}

/// Compact classes; the enum is the only way to set one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Classes(pub u64);

impl Classes {
    pub fn add(&mut self, class: Class) {
        let index = match class {
            Class::Topbar => 0,
            Class::Brand => 1,
            Class::Project => 2,
            Class::Navigation => 3,
            Class::Count => 4,
            Class::LinkState => 5,
            Class::User => 6,
            Class::Page => 7,
            Class::Narrow => 8,
            Class::Welcome => 9,
            Class::SectionHeading => 10,
            Class::Composer => 11,
            Class::ChatList => 12,
            Class::ChatPreview => 13,
            Class::Card => 14,
            Class::Meta => 15,
            Class::Tag => 16,
            Class::Live => 17,
            Class::Closed => 18,
            Class::Pending => 19,
            Class::Notice => 20,
            Class::Primary => 21,
            Class::ButtonRow => 22,
        };
        let bit = 1_u64.checked_shl(index).expect("class fits the bitset");
        self.0 |= bit;
    }
}

/// A state on an interactive element.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Disabled,
    Expanded,
    Busy,
    Current,
    Invalid,
    SubmitOnEnter,
}

/// Compact element states.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct States(pub u16);

impl States {
    pub fn add(&mut self, state: State) {
        let index = match state {
            State::Disabled => 0,
            State::Expanded => 1,
            State::Busy => 2,
            State::Current => 3,
            State::Invalid => 4,
            State::SubmitOnEnter => 5,
        };
        let bit = 1_u16.checked_shl(index).expect("state fits the bitset");
        self.0 |= bit;
    }
}

/// The domain's last deliberate write to an input.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Value {
    pub text: Box<[u8]>,
    pub written: u32,
}

/// One tree node; `size` includes this node and all descendants.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Node {
    pub id: NodeId,
    pub element: Element,
    pub key: Option<NodeKey>,
    pub role: Option<Role>,
    pub name: Option<Box<[u8]>>,
    pub classes: Classes,
    pub states: States,
    pub text: Option<Box<[u8]>>,
    pub value: Option<Value>,
    pub href: Option<Address>,
    /// A checked http(s) Markdown link, opened in a new tab by the shell.
    pub external: Option<Box<[u8]>>,
    pub binding: Option<Binding>,
    pub size: u32,
}

impl Node {
    #[must_use]
    pub fn new(element: Element) -> Node {
        Node {
            id: NodeId(0),
            element,
            key: None,
            role: None,
            name: None,
            classes: Classes(0),
            states: States(0),
            text: None,
            value: None,
            href: None,
            external: None,
            binding: None,
            size: 1,
        }
    }
}

/// A page as nodes in preorder.
#[derive(Debug)]
pub struct Tree {
    pub(crate) nodes: List<Node>,
}

impl Tree {
    #[must_use]
    pub fn new(capacity: u32) -> Tree {
        Tree { nodes: List::with_capacity(capacity) }
    }
    #[must_use]
    pub fn nodes(&self) -> &[Node] {
        self.nodes.as_slice()
    }
    #[must_use]
    pub fn get(&self, index: u32) -> Option<&Node> {
        self.nodes.get(index)
    }
    #[must_use]
    pub fn find(&self, id: NodeId) -> Option<&Node> {
        let mut index = 0;
        while index < self.len() {
            let node = self.get(index).expect("index is in tree");
            if node.id == id {
                return Some(node);
            }
            index = index.checked_add(1).expect("tree length fits u32");
        }
        None
    }
    #[must_use]
    pub fn len(&self) -> u32 {
        self.nodes.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}
