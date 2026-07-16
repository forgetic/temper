//! Join-on-drop adapter for tongs' filesystem tools.
//!
//! Tongs correctly keeps filesystem work off the runtime thread, but dropping
//! its `spawn_blocking` join future does not prove that the blocking closure has
//! stopped. Temper runs each invocation in a dedicated joined owner so generic
//! tool cancellation cannot report run quiescence while a filesystem task is
//! still touching the checkout.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread::{self, JoinHandle};

use async_trait::async_trait;
use tongs::error::{Error, Result};
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolUpdate};

pub fn joined_filesystem_tool(tool: Box<dyn Tool>) -> Box<dyn Tool> {
    Box::new(JoinedFilesystemTool {
        inner: Arc::from(tool),
    })
}

struct JoinedFilesystemTool {
    inner: Arc<dyn Tool>,
}

#[async_trait]
impl Tool for JoinedFilesystemTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn label(&self) -> &str {
        self.inner.label()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters(&self) -> serde_json::Value {
        self.inner.parameters()
    }

    fn effects(&self) -> ToolEffects {
        self.inner.effects()
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        input: serde_json::Value,
        on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> Result<ToolOutput> {
        JoinedFilesystemCall::spawn(
            Arc::clone(&self.inner),
            tool_call_id.to_string(),
            input,
            on_update,
        )?
        .await
    }
}

struct CallState {
    result: Option<Result<ToolOutput>>,
    waker: Option<Waker>,
}

struct JoinedFilesystemCall {
    state: Arc<Mutex<CallState>>,
    thread: Option<JoinHandle<()>>,
}

impl JoinedFilesystemCall {
    fn spawn(
        tool: Arc<dyn Tool>,
        tool_call_id: String,
        input: serde_json::Value,
        on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> Result<Self> {
        let state = Arc::new(Mutex::new(CallState {
            result: None,
            waker: None,
        }));
        let thread_state = Arc::clone(&state);
        let name = format!("temper-fs-{}", tool.name());
        let thread = thread::Builder::new()
            .name(name)
            .spawn(move || {
                let result = temper_agent_io::block_on(async move {
                    tool.execute(&tool_call_id, input, on_update).await
                });
                let waker = {
                    let mut state = thread_state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.result = Some(result);
                    state.waker.take()
                };
                if let Some(waker) = waker {
                    waker.wake();
                }
            })
            .map_err(|error| Error::tool("filesystem", format!("start tool owner: {error}")))?;
        Ok(Self {
            state,
            thread: Some(thread),
        })
    }

    fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Future for JoinedFilesystemCall {
    type Output = Result<ToolOutput>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let result = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.result.is_none()
                && !state
                    .waker
                    .as_ref()
                    .is_some_and(|waker| waker.will_wake(cx.waker()))
            {
                state.waker = Some(cx.waker().clone());
            }
            state.result.take()
        };
        match result {
            Some(result) => {
                self.join();
                Poll::Ready(result)
            }
            None => Poll::Pending,
        }
    }
}

impl Drop for JoinedFilesystemCall {
    fn drop(&mut self) {
        // Filesystem syscalls are not generally preemptible. Joining is the
        // conservative contract: deadline publication waits until the blocking
        // operation can no longer mutate the checkout.
        self.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Condvar;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    struct BlockingTool {
        release: Arc<(Mutex<bool>, Condvar)>,
        finished: Arc<AtomicBool>,
    }

    #[async_trait]
    impl Tool for BlockingTool {
        fn name(&self) -> &str {
            "blocking_fs"
        }

        fn description(&self) -> &str {
            "test"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type":"object"})
        }

        fn effects(&self) -> ToolEffects {
            ToolEffects::read()
        }

        async fn execute(
            &self,
            _tool_call_id: &str,
            _input: serde_json::Value,
            _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
        ) -> Result<ToolOutput> {
            let (lock, wake) = &*self.release;
            let mut released = lock.lock().expect("release lock");
            while !*released {
                released = wake.wait(released).expect("release wait");
            }
            self.finished.store(true, Ordering::Release);
            Ok(ToolOutput::text("finished"))
        }
    }

    #[test]
    fn deadline_drop_joins_the_blocking_filesystem_owner() {
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let finished = Arc::new(AtomicBool::new(false));
        let tool = joined_filesystem_tool(Box::new(BlockingTool {
            release: Arc::clone(&release),
            finished: Arc::clone(&finished),
        }));
        let release_for_thread = Arc::clone(&release);
        let releaser = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            let (lock, wake) = &*release_for_thread;
            *lock.lock().expect("release lock") = true;
            wake.notify_all();
        });

        let outcome = temper_agent_io::block_on(async move {
            temper_agent_io::timeout(
                Duration::from_millis(10),
                tool.execute("call", serde_json::json!({}), None),
            )
            .await
        });
        assert!(outcome.is_err());
        assert!(
            finished.load(Ordering::Acquire),
            "deadline returned before the blocking filesystem owner joined"
        );
        releaser.join().expect("release thread");
    }
}
