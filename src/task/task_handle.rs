use std::sync::atomic::{AtomicUsize, Ordering};

/// Used by the task system to provide means for calling code to wait for task completion.
/// Multiple tasks may be associated with one `TaskHandle`.
#[repr(transparent)]
pub struct TaskHandle(AtomicUsize);

impl TaskHandle {
    /// Creates a new handle with no tasks associated with it.
    /// Waiting for it returns immediately.
    pub(super) fn new() -> TaskHandle {
        TaskHandle(AtomicUsize::new(0))
    }

    /// Increments the number of unfinished tasks associated with the `TaskHandle`.
    pub(super) fn inc(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    /// Decrements the number of unfinished tasks associated with the `TaskHandle`.
    /// Returns the decremented count.
    pub(super) fn dec(&self) -> usize {
        let prev = self.0.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(prev > 0);
        prev - 1
    }

    /// Returns `true` if all tasks associated with this `TaskHandle` have finished
    /// and the counter value is `0`.
    pub(super) fn is_complete(&self) -> bool {
        self.0.load(Ordering::SeqCst) == 0
    }

    #[cfg(feature = "tracing")]
    pub(super) fn load(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}
