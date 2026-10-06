//! Actions decoded from the view's nodes; no DOM or URL parsing lives here.
use crate::{Address, Object};
use alloc::boxed::Box;
use skein_lib::Id;

/// A typed action taken by the person.
#[derive(PartialEq, Eq, Debug)]
pub enum Action {
    Go { address: Address },
    Edit { field: FieldRef, text: Box<[u8]> },
    Submit { form: Form },
    Intend { intent: Intent, object: Id<Object> },
    Confirm,
    Dismiss,
    SignIn,
}

/// A card action considered before confirmation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Intent {
    Release,
    LeaveHeld,
    PassUp,
}

/// A field's stable purpose; the browser's text is never echoed on edit.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum FieldRef {
    NewChat,
    Reason,
}

/// A form submitted by the person.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Form {
    NewChat,
}
