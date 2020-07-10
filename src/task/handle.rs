use {
    super::task_system::YieldKey,
    std::sync::atomic::{AtomicUsize, Ordering},
};

/// Used by the task system to provide means for calling code to check / wait for task completion.
/// Multiple tasks may be associated with one `Handle`, but only a single [`Scope`].
///
/// This object is allocated on the stack for use by the [`Scope`].
/// This is done because the address of the `Handle` on stack must be stable for the entire lifetime
/// of the [`Scope`] associated with it, including the [`Scope`]'s `Drop` handler.
///
/// [`Scope`]: struct.Scope.html
pub struct Handle(AtomicUsize);

impl Handle {
    /// Creates a new `Handle` with no tasks associated with it.
    /// Waiting for it returns immediately.
    pub(crate) fn new() -> Handle {
        Handle(AtomicUsize::new(0))
    }

    /// Increments the number of unfinished tasks associated with the `Handle`.
    pub(crate) fn inc(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    /// Decrements the number of unfinished tasks associated with the `Handle`.
    /// Returns the decremented count.
    pub(crate) fn dec(&self) -> usize {
        let prev = self.0.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(prev > 0);
        prev - 1
    }

    /// Returns `true` if all tasks associated with this `Handle` have finished
    /// and the counter value is `0`.
    pub(crate) fn is_complete(&self) -> bool {
        self.0.load(Ordering::SeqCst) == 0
    }

    pub(crate) fn yield_key(&self) -> YieldKey {
        unsafe { YieldKey::new_unchecked(self as *const _ as _) }
    }

    #[cfg(feature = "tracing")]
    pub(crate) fn load(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}
