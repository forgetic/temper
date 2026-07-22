use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use temper_agent_core::{CleanupTrigger, ContainedProcess};

pub(super) struct ProcessResources {
    pub(super) process: ContainedProcess,
    pub(super) stdin: Mutex<Option<std::process::ChildStdin>>,
    reader: Mutex<Option<thread::JoinHandle<()>>>,
    cancelled: AtomicBool,
}

impl ProcessResources {
    fn cleanup(&self, trigger: CleanupTrigger) {
        self.cancelled.store(true, Ordering::Release);
        let _report = self.process.cleanup(trigger);
        self.stdin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(reader) = self
            .reader
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = reader.join();
        }
    }
}

struct CleanupRequest {
    trigger: CleanupTrigger,
    joined: Option<mpsc::SyncSender<()>>,
}

pub(in crate::mcp) struct ProcessControl {
    child_id: u32,
    pub(super) resources: Arc<ProcessResources>,
    cleanup: mpsc::Sender<CleanupRequest>,
}

impl ProcessControl {
    pub(super) fn new(
        process: ContainedProcess,
        stdin: std::process::ChildStdin,
        reader: thread::JoinHandle<()>,
    ) -> io::Result<Self> {
        let child_id = process.id();
        let resources = Arc::new(ProcessResources {
            process,
            stdin: Mutex::new(Some(stdin)),
            reader: Mutex::new(Some(reader)),
            cancelled: AtomicBool::new(false),
        });
        let (cleanup, requests) = mpsc::channel::<CleanupRequest>();
        let owner_resources = Arc::clone(&resources);
        if let Err(error) = thread::Builder::new()
            .name(format!("mcp-cleanup-{child_id}"))
            .spawn(move || cleanup_owner(owner_resources, requests))
        {
            // Setup has not returned a client yet, so synchronously restore the
            // spawn invariant on this exceptional path. Once constructed, all
            // Drop paths only enqueue to the dedicated owner above.
            resources.cleanup(CleanupTrigger::Shutdown);
            return Err(error);
        }
        Ok(Self {
            child_id,
            resources,
            cleanup,
        })
    }

    pub(in crate::mcp) fn child_id(&self) -> u32 {
        self.child_id
    }

    pub(in crate::mcp) fn is_cancelled(&self) -> bool {
        self.resources.cancelled.load(Ordering::Acquire)
    }

    /// Requests cleanup without waiting for the request mutex, containment
    /// discovery, direct-child reap, or output-reader join. Drop paths use only
    /// this operation, keeping recursive cleanup off the async runtime thread.
    pub(in crate::mcp) fn request_cleanup(&self, trigger: CleanupTrigger) {
        self.resources.cancelled.store(true, Ordering::Release);
        let _ = self.cleanup.send(CleanupRequest {
            trigger,
            joined: None,
        });
    }

    /// Requests cleanup on the dedicated owner and waits for recursive
    /// emptiness, direct-child reap, and the stdout reader join. Blocking MCP
    /// request threads use this proof boundary before returning terminal I/O.
    pub(in crate::mcp) fn cancel_and_join(&self, trigger: CleanupTrigger) {
        self.resources.cancelled.store(true, Ordering::Release);
        let (joined, wait) = mpsc::sync_channel(0);
        if self
            .cleanup
            .send(CleanupRequest {
                trigger,
                joined: Some(joined),
            })
            .is_ok()
        {
            let _ = wait.recv();
        }
    }
}

impl Drop for ProcessControl {
    fn drop(&mut self) {
        self.request_cleanup(CleanupTrigger::OwnerDrop);
    }
}

fn cleanup_owner(resources: Arc<ProcessResources>, requests: mpsc::Receiver<CleanupRequest>) {
    while let Ok(request) = requests.recv() {
        resources.cleanup(request.trigger);
        if let Some(joined) = request.joined {
            let _ = joined.send(());
        }
    }
}
