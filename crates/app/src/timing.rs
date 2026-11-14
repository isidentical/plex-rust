//! When things happened, for measuring what the viewer waits for.

use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

/// Starts the clock. Until this is called [`mark`] prints nothing, so only
/// a run that asks for timings gets them.
pub fn start() {
    // Cannot fail in a way that matters: a second call keeps the first start.
    let _ = START.set(Instant::now());
}

pub fn is_on() -> bool {
    START.get().is_some()
}

/// Prints `what` with the milliseconds since [`start`].
pub fn mark(what: &str) {
    if let Some(start) = START.get() {
        eprintln!("timing {:9.1} ms  {what}", start.elapsed().as_secs_f64() * 1e3);
    }
}
