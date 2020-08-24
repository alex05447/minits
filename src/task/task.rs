use {
    super::util::SendPtr,
    crate::{Handle, TaskFn},
    miniclosure::ClosureHolder,
};

#[cfg(feature = "task_names")]
use super::util::send_slice::SendSlice;

/// A single task's context / state, associated with it throughout its lifetime in the `TaskSystem`.
pub(crate) struct Task {
    pub(crate) task: Option<ClosureHolder>,
    // Points to the `Scope`'s `Handle` on the stack.
    // We guarantee the task does not live longer then the `Handle` so that this pointer is always valid.
    pub(crate) handle: SendPtr<Handle>,
    pub(crate) main_thread: bool,

    #[cfg(feature = "task_names")]
    pub(crate) task_name: Option<String>,

    #[cfg(feature = "task_names")]
    // Points to the `Scope`'s name `String`, if `Some`.
    // We guarantee the task does not live longer then the `Scope` so that this pointer is always valid.
    pub(crate) scope_name: Option<SendSlice<u8>>,
}

// We solemnly swear the `Task` is safe to send / reference from other threads -
// we guarantee only one thread ever picks it up / executes it.
unsafe impl Send for Task {}
unsafe impl Sync for Task {}

impl Task {
    #[allow(unused_variables)]
    pub(crate) unsafe fn new<'h, F>(
        h: &Handle,
        f: F,
        main_thread: bool,
        task_name: Option<String>,
        scope_name: Option<&str>,
    ) -> Self
    where
        F: TaskFn<'h>,
    {
        Task {
            task: Some(ClosureHolder::once(f)),
            handle: SendPtr::new(h),
            main_thread,

            #[cfg(feature = "task_names")]
            task_name,

            #[cfg(feature = "task_names")]
            scope_name: scope_name.map(|s| SendSlice::new(s.as_bytes())),
        }
    }

    /// Extracts the task closure from the task context.
    /// Needed so that (if we use fibers) the closure lives on the fiber stack when it's being executed,
    /// not in thread-local storage,
    /// so that bad things don't happen after a fiber switches threads.
    pub(crate) fn take_task(&mut self) -> ClosureHolder {
        self.task.take().expect("current task not set")
    }

    pub(crate) fn is_none(&self) -> bool {
        self.task.is_none()
    }

    /// Returns `true` if this was the last task associated with its handle
    /// and any waits on the handle are now compete.
    pub(crate) fn dec(&self) -> bool {
        self.handle.dec() == 0
    }

    pub(crate) fn task_name(&self) -> Option<&str> {
        #[cfg(feature = "task_names")]
        {
            self.task_name.as_ref().map(|s| s.as_str())
        }

        #[cfg(not(feature = "task_names"))]
        {
            None
        }
    }

    pub(crate) fn take_task_name(&mut self) -> Option<String> {
        #[cfg(feature = "task_names")]
        {
            self.task_name.take()
        }

        #[cfg(not(feature = "task_names"))]
        {
            None
        }
    }

    pub(crate) fn scope_name(&self) -> Option<&str> {
        #[cfg(feature = "task_names")]
        {
            self.scope_name.as_ref().map(|s| unsafe { s.as_str() })
        }

        #[cfg(not(feature = "task_names"))]
        {
            None
        }
    }

    #[cfg(feature = "logging")]
    pub(crate) fn task_name_or_unnamed(&self) -> &str {
        self.task_name().unwrap_or("<unnamed>")
    }

    #[cfg(feature = "logging")]
    pub(crate) fn scope_name_or_unnamed(&self) -> &str {
        self.scope_name().unwrap_or("<unnamed>")
    }
}
