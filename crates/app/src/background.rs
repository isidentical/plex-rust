//! Work that must not block the main thread.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::MainThreadMarker;
use tokio::runtime::Runtime;
use tokio::sync::oneshot;

use crate::error::{Error, Result};

/// Runs network and disk work on its own threads and hands the results back
/// to the main thread. This is the only way work leaves the main thread.
pub struct Background {
    runtime: Runtime,
}

/// Work started with [`Background::run`] whose result may turn out not
/// to be wanted. Dropping this changes nothing: the work goes on.
pub struct Task {
    stop: oneshot::Sender<()>,
    /// Read on the main thread just before the result is handed over. The
    /// work may be done and its result on the way there when it is
    /// cancelled, which stopping the work can no longer catch.
    cancelled: Arc<AtomicBool>,
}

impl Task {
    /// Gives the work up where it stands. Its request is abandoned, and
    /// what was to hear the result is not called from here on.
    pub fn cancel(self) {
        self.cancelled.store(true, Ordering::Relaxed);
        // Fails only when the work is already done.
        let _ = self.stop.send(());
    }
}

impl Background {
    pub fn new() -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(Error::Runtime)?;
        Ok(Self { runtime })
    }

    /// Runs `work` and waits for it, blocking the calling thread. Only for
    /// the moment the app quits, when nothing will run afterwards.
    pub fn wait<T>(&self, work: impl Future<Output = T>) -> T {
        self.runtime.block_on(work)
    }

    /// Runs `work` off the main thread, then calls `then` with its result on
    /// the main thread, unless the returned task was cancelled first.
    pub fn run<T: Send + 'static>(
        &self,
        main_thread: MainThreadMarker,
        work: impl Future<Output = T> + Send + 'static,
        then: impl FnOnce(T) + 'static,
    ) -> Task {
        // `then` may hold views, which must not be touched or dropped off
        // the main thread. The wrapper lets it travel without being used.
        let then = MainThreadBound::new(then, main_thread);
        let (stop, stopped) = oneshot::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let unwanted = Arc::clone(&cancelled);
        self.runtime.spawn(async move {
            let result = tokio::select! {
                biased;
                // An error here is the task having been dropped, not
                // cancelled, and then the work goes on.
                Ok(()) = stopped => None,
                result = work => Some(result),
            };
            // Back on the main thread even with nothing to hand over:
            // that is the only place `then` may be dropped.
            DispatchQueue::main().exec_async(move || {
                let main_thread = MainThreadMarker::new()
                    .unwrap_or_else(|| panic!("the main queue runs on the main thread"));
                let then = then.into_inner(main_thread);
                if let Some(result) = result.filter(|_| !unwanted.load(Ordering::Relaxed)) {
                    then(result);
                }
            });
        });
        Task { stop, cancelled }
    }
}
