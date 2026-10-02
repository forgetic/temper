//! The translations between the model's vocabulary and its sub-models' (4.5):
//! small total functions, one per direction, each an exhaustive match, so that
//! a variant added on either side breaks the build here.
//!
//! Today every event is for the session sub-model and every request comes from
//! it. Once the run sub-model sits between the protocol layer and the sessions,
//! the protocol's records route to the run, and the run and the session meet
//! through translations here.

use temper_agent_model_session as session;

use crate::boundary::{Event, Request};

/// The session's event for one of ours.
pub(crate) fn event(event: Event) -> session::Event {
    match event {
        Event::Open { opener, spec } => session::Event::Open { opener, spec },
        Event::Continue { session, content } => session::Event::Continue { session, content },
        Event::Close { session } => session::Event::Close { session },
        Event::Completed { owner, completion } => session::Event::Completed { owner, completion },
        Event::Failed { owner, failure } => session::Event::Failed { owner, failure },
        Event::Cancelled { owner } => session::Event::Cancelled { owner },
        Event::ToolDone { owner, outcome } => session::Event::ToolDone { owner, outcome },
        Event::ToolCancelled { owner } => session::Event::ToolCancelled { owner },
    }
}

/// Our request for one of the session's.
pub(crate) fn request(request: session::Request) -> Request {
    match request {
        session::Request::Opened { opener, session } => Request::Opened { opener, session },
        session::Request::Yielded { opener, stop, text } => Request::Yielded { opener, stop, text },
        session::Request::Used { opener, usage } => Request::Used { opener, usage },
        session::Request::Ended { opener, end, turns, usage } => Request::Ended { opener, end, turns, usage },
        session::Request::Complete { owner, prompt, timeout } => Request::Complete { owner, prompt, timeout },
        session::Request::Cancel { owner } => Request::Cancel { owner },
        session::Request::Tool { owner, call, deadline } => Request::Tool { owner, call, deadline },
        session::Request::CancelTool { owner } => Request::CancelTool { owner },
    }
}
