# The view

Provisional, 2026-10-05. `temper-web-view`: the domain's state as a
tree of a closed vocabulary, the person's actions back as domain inputs,
and the diff between the last tree and the next (`architecture.md`,
section 4). How the crate is organised, its tree, how pages and cards
are built, tokens, the diff, words and Markdown. The pages follow the
mockups (`docs/design/web/mockups/`) for what each shows and where; their
looks are the stylesheet's, which the view names by class. Overview:
README.md; the domain it reads: 01-domain.md.

## 1. In one page

- **State in, tree out.** Each page and each card is a function from the
  domain's state to tree nodes, through a builder, one node a call. A
  card has one function, called wherever it shows (`ux/README.md`, 5.2).
- **A closed vocabulary.** Elements, roles, classes and states are enums;
  text is owned bytes. Every node a person acts on has a role and an
  accessible name, since that is how the person finds it (03-testing.md,
  section 2).
- **Bindings, not closures.** A node a person acts on carries a typed
  binding. The shell reports an event with the node's id; the view finds
  the node in its last tree and decodes its binding into a domain action.
- **Diffed, not replaced.** The view keeps the last tree, builds the next
  when the domain's `shown` count moved, matches nodes by key in lists
  and by position elsewhere, and emits patches. A matched node keeps its
  id, so the browser keeps focus, caret and scroll; an input's value is
  written only when the domain wrote it.
- **Sized from the domain's limits,** so a tree always fits and a diff's
  patches are bounded by the two trees.
- **Words are the view's.** Phases, holds, amounts and times are said in
  words here, from the tables of `ux/`; Markdown agents and people write
  is read by a bounded reader of the view's own.

## 2. The crate

```
crates/temper-web-view/src/
├── lib.rs           its doc; re-exports
├── tree.rs          Tree, Node, NodeId, NodeKey, Element, Role, Class, States, Value
├── builder.rs       Builder: open, close, text, attributes, keys, bindings
├── binding.rs       Binding, DomEvent; decode
├── diff.rs          the diff: matching, ids, patches; Patch
├── render.rs        View; render: build when shown moved, diff, swap
├── limits.rs        Limits derived from the domain's; worst_case
├── words.rs         Text, a bounded writer; phases, holds, executors, amounts, times in words
├── markdown.rs      the Markdown subset, read into nodes
├── frame.rs         what every page shows: the navigation, the inbox's count, the project, the link's state
├── confirm.rs       the confirmation dialog: what an intent will do, its fields
├── notices.rs
├── cards.rs         the cards' shared parts: a card's head, its context line, its state (deciding, refused, leaving)
├── cards/
│   ├── chip.rs  escalation.rs  result.rs  proposal.rs  question.rs  person.rs  refusal.rs  change.rs
├── pages.rs         page: the frame, then the page at the address, then the dialog and notices
├── pages/
│   ├── signin.rs  missing.rs  chats.rs  inbox.rs  board.rs  changes.rs
│   ├── task.rs        the universal page and its leads
│   └── task/          header.rs  overview.rs  conversation.rs  runs.rs  plan.rs  history.rs  budget.rs  steer.rs
└── tests.rs         step tests, a module per part under tests/
```

W2 lands `tree`, `builder`, `binding`, `diff`, `render`, `limits`,
`words`, `frame`, `notices`, `pages` with the sign-in, missing and chats
pages; W4 adds `markdown`, `confirm`, the task page's header, overview
and conversation in their first form, and the escalation, result and chip
cards; each later increment adds the files of its pages and cards.

## 3. The tree

```rust
/// A page as nodes in preorder; each node says how many follow it below.
#[derive(Debug)]
pub struct Tree {
    nodes: List<Node>,
    index: Map<NodeId, u32>,         // where each id is, for decoding and diffing
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Node {
    pub id: NodeId,                  // stable across builds while the diff matches it
    pub element: Element,
    pub key: Option<NodeKey>,        // in lists: what the row is, so lists diff by object
    pub role: Option<Role>,          // where it is not the element's own
    pub name: Option<Box<[u8]>>,     // the accessible name, where the text does not give it
    pub classes: Classes,            // a bit set of the stylesheet's classes
    pub states: States,              // disabled, expanded, busy, current, invalid, submits on enter...
    pub text: Option<Box<[u8]>>,     // only on Element::Text
    pub value: Option<Value>,        // an input's value, with the domain's written count
    pub href: Option<Address>,       // a link's address; the shell's codec writes the URL
    pub binding: Option<Binding>,    // what acting on it means; never sent to the browser
    pub size: u32,                   // this node and those below it
}

/// A closed set; adding one is a build error wherever elements are matched
/// (the shell's patcher, later; the tree face's queries).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Element {
    Main, Header, Nav, Section, Article, Aside, Footer, Div, Span,
    Heading(Level), Paragraph, List, OrderedList, Item,
    Link, Button, Form, Label, TextArea, Input(InputKind), Radio, Dialog, Details, Summary,
    Strong, Emphasis, Code, Pre, Quote, Time, Progress,
    Text,                            // a text node
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Role {
    Button, Link, Dialog, Status, Alert, Navigation, Region, Log,
    List, ListItem, Tree, TreeItem, TabList, Tab, TextBox, Radio, RadioGroup, Group,
}

/// In lists, what a row is: the diff matches keyed rows by key.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum NodeKey {
    Object(ObjectKey),               // a card, a chip, a plan's line
    Turn { run: u64, turn: u32 },    // a conversation's turn
    Line(u64),                       // a history line
    Run(u64),                        // a run's heading
}
```

- **Class** names the stylesheet's classes, which start as the mockups'
  (`style.css`, `terminal-chat.css`, `terminal-pages.css`); a class the
  stylesheet loses is a build error here, once the shell's build checks
  the two against each other (later).
- **Text is owned bytes** a text node holds, never built with `format!`
  (section 8).
- **No inline styles.** The mockups' few (a progress bar's width) become a
  `Progress` element with a value, or a class from a small closed set.

## 4. Building

```rust
/// Builds one tree, one node a call. Owns the tree it builds (no lifetime
/// parameter): `finish` hands it back.
#[derive(Debug)]
pub struct Builder {
    tree: Tree,
    open: Stack<u32>,                // the nodes open, by position
}

impl Builder {
    pub fn open(&mut self, element: Element);
    pub fn close(&mut self);
    pub fn text(&mut self, text: &[u8]);             // a text node, copied in
    pub fn words(&mut self, text: Text);             // a text node the view composed (section 8)
    pub fn key(&mut self, key: NodeKey);
    pub fn role(&mut self, role: Role);
    pub fn name(&mut self, name: &[u8]);
    pub fn class(&mut self, class: Class);
    pub fn state(&mut self, state: State);
    pub fn value(&mut self, field: &Field);
    pub fn href(&mut self, address: Address);
    pub fn bind(&mut self, binding: Binding);
    pub fn finish(self) -> Tree;
}
```

Building past the tree's capacity is an assertion: the capacity comes
from the domain's limits (section 5), so overflowing it is the view's
bug. A card, as a plain function over the builder:

```rust
/// A held task's escalation (ux/inbox.md, 4.2; ux/tasks.md, section 7): one
/// function, wherever it shows.
pub fn escalation(b: &mut Builder, domain: &Domain, id: Id<Object>, object: &Object, held: &Escalation, place: Place) {
    b.open(Element::Article);
    b.key(NodeKey::Object(object.key));
    b.class(Class::Card);
    b.role(Role::Region);
    b.name(&held.title);
    cards::head(b, domain, Kind::Held, held.task, held.waiting_since, place);
    b.open(Element::Heading(Level::Three));
    b.words(words::hold(&held.reason));               // "Review task ran out of tries"
    b.close();
    b.open(Element::Paragraph);
    b.words(words::release_does(&held.reason));       // "Releasing it will start a new run and reset its tries."
    b.close();
    match &object.card {
        Card::Open => actions(b, id, &held.offers),
        Card::Deciding { .. } => cards::deciding(b),
        Card::Refused { refusal } => {
            cards::refused(b, refusal);
            actions(b, id, &held.offers);
        }
        Card::Leaving { why, .. } => cards::leaving(b, domain, why),
    }
    b.close();
}

fn actions(b: &mut Builder, id: Id<Object>, offers: &Offers) {
    b.open(Element::Div);
    b.class(Class::ButtonRow);
    if offers.release {
        cards::button(b, Class::Primary, Binding::Intend { intent: Intent::Release, object: id }, b"Release task");
    }
    if offers.leave_held {
        cards::button(b, Class::Plain, Binding::Intend { intent: Intent::LeaveHeld, object: id }, b"Leave held");
    }
    b.close();
}
```

- **`Place`** (the inbox, a conversation, a task page) changes a card's
  context line only ("temper ▸ T21 Refresh token handling ▸ T27"), never
  its actions: one object, one card.
- **Pages are the same,** a function per page reading the page's state and
  calling the cards' functions for the handles it names:

```rust
pub fn page(b: &mut Builder, domain: &Domain) {
    frame::frame(b, domain);
    b.open(Element::Main);
    match domain.page() {
        Page::Starting => pages::starting(b, domain),
        Page::SignIn { then } => signin::page(b, domain, *then),
        Page::Missing { address } => missing::page(b, domain, *address),
        Page::Chats(chats) => chats::page(b, domain, chats),
        Page::Task(task) => task::page(b, domain, task),
        Page::Inbox(inbox) => inbox::page(b, domain, inbox),
        Page::Board(board) => board::page(b, domain, board),
        Page::Changes(changes) => changes::page(b, domain, changes),
    }
    b.close();
    confirm::dialog(b, domain);
    notices::notices(b, domain);
}
```

### 4.1 Pages, after the mockups

| Page | Mockup | What it builds |
|---|---|---|
| the frame | every page's top bar | brand; the project and a switch; Inbox with its count, Chats, Board, Changes, Tasks; live, behind or offline; go to a task by number |
| chats | `chats.html` | start a chat: project, a composer, Enter to start; the person's chats, live or closed, with each one's phase in words, spend, delegates, last activity |
| a task, led by its conversation | `index.html` | the chat bar: number, title, phase, spend, stop and close; the conversation: the person's words with their state, replies, tool calls folded, inline cards; the composer |
| a task, led by its activity | `agent-task.html` | lineage; number, title, phase, charter, spend, amend and stop; its brief; its runs, each with how it began and ended; the live turn, streaming, uncommitted; words to it |
| a task, led by its plan | `task.html` | the header with its toolbar; overview, plan, activity, history, budget; the plan as a tree with folds and "held only"; a held notice; history for the task or its subtree; budget and authority, links, related chips |
| inbox | `inbox.html` | needs you, with its count and filters, oldest first; updates, newest first, new marked |
| board | `board.html` | the period's numbers; the goals by priority with progress, spend and held below; live, waiting, recently ended; set a goal; the project's health |
| changes | `changes.html` | each landing branch's queue and its rule; not ready, by step; held; a change's way to landing and gates at its head; the branch's health; recent landings |
| confirmation | the mockups' dialogs | what the intent will do, its fields (a reason, a choice, each delegate's disposal), cancel and confirm, confirm marked dangerous for cancel and reject |

Where the mockups simulate (a toast saying "Request pending…" for
every request), the view follows `ux/` instead: a card shows its own
request as being decided, and notices are kept for what has no card.

### 4.2 Accessibility, because the person needs it

Every node the person acts on, or reads to decide, has a role and a
name: cards are named regions; buttons are named by their words; the
conversation is a log; the plan is a tree whose items say whether they
are expanded; the link's state and notices are status regions; the
confirmation is a named dialog. The tree face finds nodes only this way
(03-testing.md, section 2), so a node it cannot find is a defect of the
page's accessibility, and the test says so.

## 5. Sized from the domain's limits

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub nodes: u32,                  // a tree's capacity
    pub patches: u32,                // a diff's most: nodes of the old tree plus the new
    pub depth: u32,                  // nesting, the builder's stack and the diff's
    pub markdown_depth: u32,         // lists and quotes nested in Markdown
}

impl Limits {
    /// The most nodes any page builds under the domain's limits: the frame,
    /// the largest page with every window full and every text at its most
    /// bytes, the confirmation and the notices. `None` on overflow.
    pub fn of(domain: &temper_web_domain::Limits) -> Option<Limits>;
}

#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64>;   // two trees, their indexes, the diff's scratch
```

Each page's function has a companion that counts the most nodes it may
build from the domain's limits; `Limits::of` takes the largest. A
Markdown text builds at most a fixed number of nodes per byte, so a
text's nodes are bounded by its bytes, which the domain bounds.

## 6. Rendering and the diff

```rust
#[derive(Debug)]
pub struct View {
    last: Tree,
    spare: Tree,                     // the next tree is built in this one, then they swap
    built: Option<u64>,              // the domain's shown count at the last build
    next_id: u32,                    // node ids, never reused
    scratch: Scratch,                // the diff's bounded maps
}

impl View {
    pub fn new(limits: &Limits) -> View;
    pub fn tree(&self) -> &Tree;     // what the tree face reads
}

/// When the domain's shown count moved: build the next tree, diff it against
/// the last into `out`, and keep it. The caller reserves `limits.patches`.
pub fn render(view: &mut View, domain: &Domain, limits: &Limits, out: &mut Queue<Patch>);

#[derive(PartialEq, Eq, Debug)]
pub enum Patch {
    /// A subtree in preorder, without bindings: the shell makes its elements.
    Insert { parent: NodeId, before: Option<NodeId>, nodes: Box<[Made]> },
    Remove { node: NodeId },
    Move { node: NodeId, parent: NodeId, before: Option<NodeId> },
    Text { node: NodeId, text: Box<[u8]> },
    Set { node: NodeId, attribute: Attribute },     // classes, states, name, role, href, bound or not
    /// An input's value, only when the domain wrote it (01-domain.md, 6.5).
    Value { node: NodeId, text: Box<[u8]> },
}
```

- **Matching.** The roots match. Below two matched nodes, keyed children
  match by key and unkeyed ones by position among siblings of the same
  element. A matched node keeps its id, and gets `Set` and `Text` patches
  for what changed; an unmatched old node is removed, an unmatched new one
  inserted with fresh ids; a keyed child whose order among its matched
  siblings changed is moved, and one that kept it is not.
- **Never replaced while it can be updated,** so the shell keeps focus,
  caret, selection, composition and scroll across a patch
  (`architecture.md`, section 4); the browser tier checks it.
- **Values.** A matched input whose `written` count is unchanged gets no
  `Value`, whatever its text: what the person is typing stays theirs.
- **Bindings are the view's.** A changed binding needs no patch, since the
  browser holds only node ids and whether a node is bound.
- **Bounded.** Each node of either tree takes part in at most one patch
  beyond its attributes, so a diff's patches fit `limits.patches`; the
  diff walks both trees once, with keyed matching through `Scratch`, a
  bounded map from key to position cleared per parent.

## 7. Bindings and decoding

```rust
/// What acting on a node means: an action's template, completed by the
/// event (an input's text).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Binding {
    Go(Address),
    Edit(FieldRef),
    Number(FieldRef),
    Submit(Form),
    Intend { intent: Intent, object: Id<Object> },
    Choose(Choice),
    Confirm,
    Dismiss,
    Fold(Fold),
    Filter(Filter),
    More { window: WindowRef, older: bool },
    SignIn,
}

/// What the shell reports: the node acted on, by its id.
#[derive(PartialEq, Eq, Debug)]
pub enum DomEvent {
    Press { node: NodeId },
    Input { node: NodeId, text: Box<[u8]> },
    /// Enter without Shift in a node marked as submitting on Enter; a form's submit.
    Submit { node: NodeId },
    Toggle { node: NodeId, open: bool },
}

/// The domain's input for an event; `None` when the node has gone from the
/// last tree (a press on a node the last patches removed), which the
/// client counts as a stale fact and drops.
pub fn decode(view: &View, event: DomEvent) -> Option<Action>;
```

Two kinds of stale, both refused, neither an error: a node gone from the
tree is dropped here; a node still there whose object has gone from the
domain's working set carries a handle that fails its generation, and the
domain refuses the action (01-domain.md, 6.2). `Number` bindings decode
an input's digits into `Action::Number`, `None` when it is not a number:
the view is where typed input becomes typed, as the protocol layer is for
the engine's bytes.

## 8. Words

```rust
/// A bounded text the view composes: fixed phrases, names copied in, and
/// numbers written by skein-lib's Decimal. No `format!`, no `String`.
#[derive(Debug)]
pub struct Text {
    writer: Writer,
}

impl Text {
    pub fn new(capacity: u32) -> Text;
    pub fn push(&mut self, bytes: &[u8]);
    pub fn number(&mut self, value: u64);
    pub fn amount(&mut self, amount: Amount);         // "14.2", the domain's units
    pub fn span(&mut self, span: Duration);           // "4 min", "1 h"
    pub fn clock(&mut self, at: Wall, offset: Offset); // "09:41", "Yesterday"
}

pub fn phase(task: &Chip) -> Text;           // ux/tasks.md, section 3: "Waiting for T24", "Held · out of tries"
pub fn doing(chat: &ChatLine) -> Text;       // ux/chats.md, 4.3: "writing", "parked; your words resume it"
pub fn hold(reason: &Hold) -> Text;          // ux/tasks.md, section 7
pub fn release_does(reason: &Hold) -> Text;
pub fn step(change: &forge::Step) -> Text;   // ux/forge.md, 2.1
pub fn funding(funding: &Funding) -> Text;   // "Funds 40 from your pool: 120 left this period, 80 after"
pub fn waited(since: Wall, now: Wall) -> Text;
```

Every function over a domain enum matches it whole, so a new phase or
hold is a build error here until it is said in words. Each `Text`'s
capacity is the longest its phrases and names can make, computed beside
it; overflowing it is an assertion.

## 9. Markdown

What agents and people write (turns, results, specs: `domain/people.md`,
section 11) is read into nodes by `markdown::read(text, limits, b)`:

- **The subset:** paragraphs and line breaks; headings, shown below the
  page's own levels; emphasis, strong and inline code; fenced code blocks;
  lists, bulleted and numbered, nested to `markdown_depth`; block quotes;
  links whose target is http or https, which open on the forge or
  elsewhere in a new tab, and are shown as text otherwise.
- **The rest is text:** tables, HTML, images, a delimiter never closed, a
  list nested too deep are shown as the characters they are, never dropped
  (`architecture.md`, section 10). No HTML is ever made from what was
  written.
- **A step machine of the view's own:** one pass over the lines, with an
  explicit stack of open blocks (`skein_lib::Stack`, bounded by
  `markdown_depth`) and, within a line, a bounded stack of open emphasis;
  no recursion, no loop but `for` over the bytes.
- **Fed real text.** Its step tests include agents' turns and results kept
  as files beside them, each with the nodes it must read to; and a fuzz
  target, later, with the other decoders.

## 10. Step tests

`src/tests.rs`, a module per part:

- **builder and tree:** nesting and sizes; the capacity of `Limits::of`
  holds a page with every window full and every text at its most;
- **cards and pages:** for each object's states (open, deciding, refused,
  leaving), the nodes a person would find: by role, name and text, with
  the small query helpers the tests share (the fake person depends on the
  view, not the other way round);
- **the diff:** generated trees from a seed, checking that applying the
  patches to a model of the old tree gives the new one, ids included; no
  patch for an unchanged tree; a reordered list moves only what changed
  order; a value only when `written` moved; the patches within
  `limits.patches`;
- **decoding:** every binding decodes to its action; a removed node to
  `None`; digits to numbers, and what is not a number to `None`;
- **words:** each table of `ux/` row by row; times across midnight and
  offsets; amounts;
- **Markdown:** the subset; the rest as text; nesting past its depth;
  the files of real text.

How the view behaves across a story (a card that follows its object, a
composer kept through a stream's updates) is the worlds' (03-testing.md):
every world builds and diffs every page it shows.
