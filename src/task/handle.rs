use {
    super::{panic::PanicPayload, task_system::YieldKey},
    crate::{util::AtomicLinkedList, TaskPanicInfo, TaskPanics},
    std::sync::atomic::{AtomicUsize, Ordering},
};

/// Used by the task system to provide means for calling code to check / wait for task completion,
/// as well as to report any panics which may have happened in the spawned tasks or their children tasks.
///
/// Multiple tasks may be associated with one `Handle` via a [`Scope`], but only a single [`Scope`].
///
/// This object is allocated on the stack for use by the [`Scope`].
/// This is done because the address of the `Handle` on stack must be stable for the entire lifetime
/// of the [`Scope`] associated with it, including the [`Scope`]'s `Drop` handler.
///
/// [`Scope`]: struct.Scope.html
pub struct Handle {
    // Task completion flag / counter.
    handle: AtomicUsize,
    // Atomic linked list containing the panics for all tasks
    // associated with the `Scope` which owns this `Handle`.
    panic: AtomicLinkedList<TaskPanicInfo>,
}

impl Handle {
    /// Creates a new `Handle` with no tasks associated with it.
    /// Waiting for it returns immediately.
    pub(crate) fn new() -> Handle {
        Handle {
            handle: AtomicUsize::new(0),
            panic: AtomicLinkedList::new(),
        }
    }

    /// Increments the number of unfinished tasks associated with the `Handle`.
    pub(crate) fn inc(&self) {
        self.handle.fetch_add(1, Ordering::SeqCst);
    }

    /// Decrements the number of unfinished tasks associated with the `Handle`.
    /// Returns the decremented count.
    pub(crate) fn dec(&self) -> usize {
        let prev = self.handle.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(prev > 0);
        prev - 1
    }

    /// Returns `true` if all tasks associated with this `Handle` have finished
    /// and the counter value is `0`.
    pub(crate) fn is_complete(&self) -> bool {
        self.handle.load(Ordering::SeqCst) == 0
    }

    /// Returns the address of the handle as its `YieldKey`.
    pub(crate) fn yield_key(&self) -> YieldKey {
        unsafe { YieldKey::new_unchecked(self as *const _ as _) }
    }

    pub(crate) fn load(&self) -> usize {
        self.handle.load(Ordering::SeqCst)
    }

    /// Atomically pushes a new panic to the head of the panic linked list.
    /// May be called from multiple threads.
    pub(crate) fn add_panic(
        &self,
        panic: PanicPayload,
        task: Option<String>,
        scope: Option<String>,
    ) {
        self.panic.push(TaskPanicInfo::new(panic, task, scope))
    }

    /// Pops all panics off the linked list.
    /// Called from a single thread, waiting for the scope / handle.
    pub(crate) fn take_panics(&self) -> Option<TaskPanics> {
        self.panic.take().map(TaskPanics)
    }
}
