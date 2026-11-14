//! Work that must not block the main thread.

use std::future::Future;

use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::MainThreadMarker;
use tokio::runtime::Runtime;

use crate::error::{Error, Result};

/// Runs network and disk work on its own threads and hands the results back
/// to the main thread. This is the only way work leaves the main thread.
pub struct Background {
    runtime: Runtime,
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

    /// Runs `work` off the main thread, then calls `then` with its result on
    /// the main thread.
    pub fn run<T: Send + 'static>(
        &self,
        main_thread: MainThreadMarker,
        work: impl Future<Output = T> + Send + 'static,
        then: impl FnOnce(T) + 'static,
    ) {
        // `then` may hold views, which must not be touched or dropped off
        // the main thread. The wrapper lets it travel without being used.
        let then = MainThreadBound::new(then, main_thread);
        self.runtime.spawn(async move {
            let result = work.await;
            DispatchQueue::main().exec_async(move || {
                let main_thread = MainThreadMarker::new()
                    .unwrap_or_else(|| panic!("the main queue runs on the main thread"));
                then.into_inner(main_thread)(result);
            });
        });
    }
}
