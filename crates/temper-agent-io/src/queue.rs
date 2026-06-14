//! Completion-queue plumbing: an unbounded MPSC channel and a oneshot cell.
//!
//! These are runtime-agnostic (plain `Mutex` + `Waker`) so they can be used
//! from any task — including callbacks deep inside an HTTP client or a spawned
//! blocking job, where no capability context is threaded through. They carry
//! completions from executors back to the engine loop; they are
//! imperative-shell internals and never appear inside a [`crate::Machine`].

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};

struct ChannelInner<T> {
    queue: VecDeque<T>,
    recv_waker: Option<Waker>,
    senders: usize,
    receiver_alive: bool,
}

/// Sending half of the completion queue. Cloneable; submission never blocks.
pub struct CqSender<T>(Arc<Mutex<ChannelInner<T>>>);

/// Receiving half of the completion queue, owned by the engine loop.
pub struct CqReceiver<T>(Arc<Mutex<ChannelInner<T>>>);

/// Create an unbounded completion queue.
pub fn channel<T>() -> (CqSender<T>, CqReceiver<T>) {
    let inner = Arc::new(Mutex::new(ChannelInner {
        queue: VecDeque::new(),
        recv_waker: None,
        senders: 1,
        receiver_alive: true,
    }));
    (CqSender(Arc::clone(&inner)), CqReceiver(inner))
}

impl<T> Clone for CqSender<T> {
    fn clone(&self) -> Self {
        self.0.lock().expect("cq lock").senders += 1;
        Self(Arc::clone(&self.0))
    }
}

impl<T> Drop for CqSender<T> {
    fn drop(&mut self) {
        let waker = {
            let mut inner = self.0.lock().expect("cq lock");
            inner.senders -= 1;
            if inner.senders == 0 {
                inner.recv_waker.take()
            } else {
                None
            }
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<T> CqSender<T> {
    /// Submit a completion. Returns the value back if the engine loop is gone.
    pub fn send(&self, value: T) -> Result<(), T> {
        let waker = {
            let mut inner = self.0.lock().expect("cq lock");
            if !inner.receiver_alive {
                return Err(value);
            }
            inner.queue.push_back(value);
            inner.recv_waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        Ok(())
    }
}

impl<T> Drop for CqReceiver<T> {
    fn drop(&mut self) {
        self.0.lock().expect("cq lock").receiver_alive = false;
    }
}

impl<T> CqReceiver<T> {
    /// Receive the next completion; `None` once all senders are gone.
    pub async fn recv(&mut self) -> Option<T> {
        std::future::poll_fn(|task_cx| {
            let mut inner = self.0.lock().expect("cq lock");
            if let Some(value) = inner.queue.pop_front() {
                return Poll::Ready(Some(value));
            }
            if inner.senders == 0 {
                return Poll::Ready(None);
            }
            inner.recv_waker = Some(task_cx.waker().clone());
            Poll::Pending
        })
        .await
    }

    /// Non-blocking receive, for draining in tests.
    pub fn try_recv(&mut self) -> Option<T> {
        self.0.lock().expect("cq lock").queue.pop_front()
    }
}

struct OneshotInner<T> {
    value: Option<T>,
    waker: Option<Waker>,
    sender_alive: bool,
}

/// Sending half of a oneshot cell (used to route one reply back to the task
/// that is waiting on it).
pub struct OneshotSender<T>(Arc<Mutex<OneshotInner<T>>>);

/// Receiving half of a oneshot cell.
pub struct OneshotReceiver<T>(Arc<Mutex<OneshotInner<T>>>);

/// Create a oneshot cell.
pub fn oneshot<T>() -> (OneshotSender<T>, OneshotReceiver<T>) {
    let inner = Arc::new(Mutex::new(OneshotInner {
        value: None,
        waker: None,
        sender_alive: true,
    }));
    (OneshotSender(Arc::clone(&inner)), OneshotReceiver(inner))
}

impl<T> OneshotSender<T> {
    /// Deliver the value, waking the receiver.
    pub fn send(self, value: T) {
        let waker = {
            let mut inner = self.0.lock().expect("oneshot lock");
            inner.value = Some(value);
            inner.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<T> Drop for OneshotSender<T> {
    fn drop(&mut self) {
        let waker = {
            let mut inner = self.0.lock().expect("oneshot lock");
            inner.sender_alive = false;
            inner.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<T> OneshotReceiver<T> {
    /// Wait for the value; `None` if the sender was dropped without sending.
    pub async fn recv(self) -> Option<T> {
        std::future::poll_fn(|task_cx| {
            let mut inner = self.0.lock().expect("oneshot lock");
            if let Some(value) = inner.value.take() {
                return Poll::Ready(Some(value));
            }
            if !inner.sender_alive {
                return Poll::Ready(None);
            }
            inner.waker = Some(task_cx.waker().clone());
            Poll::Pending
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cq_orders_and_closes() {
        let (tx, mut rx) = channel::<u32>();
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        assert_eq!(rx.try_recv(), Some(1));
        assert_eq!(rx.try_recv(), Some(2));
        assert_eq!(rx.try_recv(), None);
        drop(rx);
        assert_eq!(tx.send(3), Err(3));
    }

    #[test]
    fn cq_recv_returns_none_when_all_senders_drop() {
        use std::future::Future;

        let (tx, mut rx) = channel::<u32>();
        drop(tx);
        let mut fut = std::pin::pin!(rx.recv());
        let waker = std::task::Waker::noop();
        let mut task_cx = std::task::Context::from_waker(waker);
        match fut.as_mut().poll(&mut task_cx) {
            Poll::Ready(None) => {}
            other => panic!("unexpected poll result: {other:?}"),
        }
    }

    #[test]
    fn oneshot_delivers() {
        use std::future::Future;

        let (tx, rx) = oneshot::<&str>();
        tx.send("ok");
        let mut rx = std::pin::pin!(rx.recv());
        let waker = std::task::Waker::noop();
        let mut task_cx = std::task::Context::from_waker(waker);
        match rx.as_mut().poll(&mut task_cx) {
            Poll::Ready(Some("ok")) => {}
            other => panic!("unexpected poll result: {other:?}"),
        }
    }

    #[test]
    fn oneshot_none_when_sender_dropped() {
        use std::future::Future;

        let (tx, rx) = oneshot::<&str>();
        drop(tx);
        let mut rx = std::pin::pin!(rx.recv());
        let waker = std::task::Waker::noop();
        let mut task_cx = std::task::Context::from_waker(waker);
        match rx.as_mut().poll(&mut task_cx) {
            Poll::Ready(None) => {}
            other => panic!("unexpected poll result: {other:?}"),
        }
    }
}
