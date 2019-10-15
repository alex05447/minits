use super::reference_holder::ReferenceHolder;
use super::task_handle::TaskHandle;
use super::task_system::TaskSystem;

use miniclosure::ClosureHolder;

pub(super) struct TaskContext {
    /// Stores the user closure associated with this task.
    pub(super) closure: ClosureHolder,

    /// Reference to the waitable task handle this task is associated with.
    /// Users may wait on this handle for this task to complete.
    pub(super) task_handle: ReferenceHolder<TaskHandle>,

    pub(super) is_main_task: bool,

    #[cfg(feature = "task_names")]
    pub(super) task_name: Option<String>,

    #[cfg(feature = "task_names")]
    pub(super) scope_name: Option<String>,
}

impl TaskContext {
    pub(super) unsafe fn new<'any, F>(
        _task_name: Option<&str>,
        _scope_name: Option<&str>,
        h: &'any TaskHandle,
        f: F,
    ) -> Self
    where
        F: FnOnce(&TaskSystem) + 'any,
    {
        TaskContext {
            closure: ClosureHolder::new(f),
            task_handle: ReferenceHolder::from_ref(h),
            is_main_task: false,

            #[cfg(feature = "task_names")]
            task_name: _task_name.map(|s| String::from(s)),

            #[cfg(feature = "task_names")]
            scope_name: _scope_name.map(|s| String::from(s)),
        }
    }

    /// Executes the task's user closure.
    /// Panics if the closure holder is empty.
    pub(super) unsafe fn execute<'any>(&mut self, task_system: &'any TaskSystem) {
        debug_assert!(self.closure.is_some());
        self.closure.execute(task_system);
    }

    /// Returns `true` if this was the last task associated with its handle
    /// and any waits on the handle are now compete.
    pub(super) fn dec(&self) -> bool {
        let h = unsafe { self.task_handle.as_ref() };
        h.dec() == 0
    }

    /*
        pub(super) fn is_complete(&self) -> bool {
            self.task_handle.as_ref().is_complete()
        }
    */

    pub(super) fn task_name(&self) -> Option<&str> {
        #[cfg(feature = "task_names")]
        return self.task_name.as_ref().map(|s| s.as_str());

        #[cfg(not(feature = "task_names"))]
        return None;
    }

    pub(super) fn scope_name(&self) -> Option<&str> {
        #[cfg(feature = "task_names")]
        return self.scope_name.as_ref().map(|s| s.as_str());

        #[cfg(not(feature = "task_names"))]
        return None;
    }

    #[cfg(any(feature = "profiling", feature = "tracing"))]
    pub(super) fn task_name_or_unnamed(&self) -> &str {
        self.task_name().unwrap_or("<unnamed>")
    }

    #[cfg(feature = "tracing")]
    pub(super) fn scope_name_or_unnamed(&self) -> &str {
        self.scope_name().unwrap_or("<unnamed>")
    }
}
