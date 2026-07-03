use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::scope::ProjectIndexState;

#[derive(Clone, Debug)]
pub(super) struct BackgroundIndex {
    inner: Arc<BackgroundIndexInner>,
}

#[derive(Debug)]
struct BackgroundIndexInner {
    state: Mutex<BackgroundIndexState>,
    completed: Condvar,
}

#[derive(Debug)]
struct BackgroundIndexState {
    completed: bool,
    actual_project: Option<String>,
    index_state: ProjectIndexState,
    error: Option<String>,
}

impl BackgroundIndex {
    pub(super) fn new() -> Self {
        Self {
            inner: Arc::new(BackgroundIndexInner {
                state: Mutex::new(BackgroundIndexState {
                    completed: false,
                    actual_project: None,
                    index_state: ProjectIndexState::BackgroundInProgress,
                    error: None,
                }),
                completed: Condvar::new(),
            }),
        }
    }

    pub(super) fn complete_success(&self, actual_project: Option<String>) {
        self.complete(BackgroundIndexState {
            completed: true,
            actual_project,
            index_state: ProjectIndexState::Fresh,
            error: None,
        });
    }

    pub(super) fn complete_error(&self, message: String) {
        self.complete(BackgroundIndexState {
            completed: true,
            actual_project: None,
            index_state: ProjectIndexState::IndexFailed,
            error: Some(message),
        });
    }

    pub(super) fn actual_project(&self) -> Option<String> {
        self.inner
            .state
            .lock()
            .expect("background index state lock")
            .actual_project
            .clone()
    }

    pub(super) fn index_state(&self) -> ProjectIndexState {
        self.inner
            .state
            .lock()
            .expect("background index state lock")
            .index_state
    }

    pub(super) fn wait(&self, timeout: Duration) -> std::result::Result<(), String> {
        let state = self.inner.state.lock().map_err(|_| {
            "background index state lock poisoned while waiting for completion".to_string()
        })?;
        let (state, _timeout) = self
            .inner
            .completed
            .wait_timeout_while(state, timeout, |state| !state.completed)
            .map_err(|_| {
                "background index state lock poisoned while waiting for completion".to_string()
            })?;
        if !state.completed {
            return Err(format!(
                "background indexing is still in progress after {:.3}s",
                timeout.as_secs_f64()
            ));
        }
        if let Some(error) = &state.error {
            return Err(format!("background indexing failed: {error}"));
        }
        Ok(())
    }

    fn complete(&self, next: BackgroundIndexState) {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("background index state lock");
        *state = next;
        self.inner.completed.notify_all();
    }
}
