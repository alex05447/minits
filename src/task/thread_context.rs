use std::num::NonZeroUsize;

use super::task_context::TaskContext;
use super::task_handle::TaskHandle;
use super::task_system::TaskSystem;

use minifiber::Fiber;

/// Per-thread task system state.
/// Intended to be thread-local.
pub(super) struct ThreadContext {
    // Is this thread the "main" thread - the owner of the `TaskSystem`
    // and its spawned threads.
    // Initiated on thread startup.
    is_main_thread: bool,

    // If we're using fibers, it's important to switch back to the fiber
    // created from the worker thread before the thread exits
    // for thread cleanup to work properly.
    // Worker threads keep their onitial fiber here.
    thread_fiber: Option<Fiber>,

    // Initiated on thread startup.
    #[cfg(feature = "tracing")]
    thread_name: Option<String>,

    // Current fiber ran by the thread, set before switch.
    current_fiber: Option<Fiber>,

    // Current task ran by the thread.
    current_task: Option<TaskContext>,

    // Previous fiber ran by the thread, to be returned to the pool.
    fiber_to_free: Option<Fiber>,

    // Yield key of the previous fiber ran by the thread, now yielded.
    yield_key: Option<NonZeroUsize>,
}

impl ThreadContext {
    pub(super) fn new(_thread_name: Option<&str>, is_main_thread: bool) -> Self {
        Self {
            is_main_thread,
            thread_fiber: None,

            #[cfg(feature = "tracing")]
            thread_name: _thread_name.map(|s| String::from(s)),

            current_fiber: None,
            current_task: None,

            fiber_to_free: None,

            yield_key: None,
        }
    }

    /*
        pub(super) fn is_current_fiber_a_thread_fiber(&self) -> bool {
            self.current_fiber.as_ref().expect("Current fiber not set.").is_thread_fiber()
        }
    */

    pub(super) fn set_thread_fiber(&mut self, fiber: Fiber) {
        match self.thread_fiber.replace(fiber) {
            Some(_) => {
                panic!("Thread fiber set more than once.");
            }
            None => {}
        }
    }

    pub(super) fn switch_to_thread_fiber(&mut self) {
        self.thread_fiber
            .take()
            .expect("Thread fiber not set")
            .switch_to();
    }

    pub(super) fn is_main_thread(&self) -> bool {
        self.is_main_thread
    }

    #[cfg(feature = "tracing")]
    pub(super) fn thread_name(&self) -> Option<&str> {
        self.thread_name.as_ref().map(|s| s.as_str())
    }

    #[cfg(feature = "tracing")]
    pub(super) fn thread_name_or_unnamed(&self) -> &str {
        self.thread_name().unwrap_or("<unnamed>")
    }

    #[cfg(feature = "tracing")]
    pub(super) fn fiber_name_or_unnamed(&self) -> &str {
        self.current_fiber
            .as_ref()
            .map_or("<none>", |fiber| fiber.name().unwrap_or("<unnamed>"))
    }

    pub(super) fn has_current_fiber(&self) -> bool {
        self.current_fiber.is_some()
    }

    // Set the thread's current fiber.
    // Called before switching to it.
    pub(super) fn set_current_fiber(&mut self, fiber: Fiber) {
        match self.current_fiber.replace(fiber) {
            Some(_) => panic!("Current fiber set more than once."),
            None => {}
        }
    }

    pub(super) fn switch_to_current_fiber(&self) {
        self.current_fiber
            .as_ref()
            .expect("Current fiber not set.")
            .switch_to();
    }

    pub(super) fn take_current_fiber(&mut self) -> Fiber {
        self.current_fiber
            .take()
            .expect("Expected the current fiber to be set.")
    }

    // Set the thread's current task.
    // Called before executing it.
    pub(super) fn set_current_task(&mut self, task: TaskContext) {
        match self.current_task.replace(task) {
            Some(_) => panic!("Current task set more than once."),
            None => {}
        }
    }

    // Set the thread's current task and execute it.
    pub(super) fn execute_task(&mut self, task: TaskContext, task_system: &TaskSystem) {
        self.set_current_task(task);

        unsafe {
            self.current_task
                .as_mut()
                .expect("Current task not set.")
                .execute(task_system);
        }
    }

    pub(super) fn current_task(&self) -> &TaskContext {
        self.current_task.as_ref().expect("Current task not set.")
    }

    pub(super) fn take_current_task(&mut self) -> TaskContext {
        self.current_task.take().expect("Current task not set.")
    }

    pub(super) fn current_task_name(&self) -> Option<&str> {
        self.current_task().task_name()
    }

    #[cfg(any(feature = "tracing", feature = "profiling"))]
    pub(super) fn current_task_name_or_unnamed(&self) -> &str {
        self.current_task().task_name().unwrap_or("<unnamed>")
    }

    pub(super) fn scope_name(&self) -> Option<&str> {
        self.current_task.as_ref().unwrap().scope_name()
    }

    pub(super) fn current_task_handle(&self) -> &TaskHandle {
        unsafe { self.current_task.as_ref().unwrap().task_handle.as_ref() }
    }

    // Take the currently set fiber and assign it to to-free fiber,
    // to be returned to the fiber pool by the next fiber ran by this thread.
    pub(super) fn set_current_fiber_to_free(&mut self) {
        let current_fiber = self.take_current_fiber();
        match self.fiber_to_free.replace(current_fiber) {
            Some(_) => panic!("Fiber to free set more than once."),
            None => {}
        }
    }

    pub(super) fn take_fiber_to_free(&mut self) -> Option<Fiber> {
        self.fiber_to_free.take()
    }

    pub(super) fn set_yield_key(&mut self, yield_key: NonZeroUsize) {
        match self.yield_key.replace(yield_key) {
            Some(_) => panic!("Yield key set more than once."),
            None => {}
        }
    }

    pub(super) fn take_yield_key(&mut self) -> Option<NonZeroUsize> {
        self.yield_key.take()
    }

    /*
        #[cfg(feature = "tracing")]
        pub(super) fn fiber_name(&self) -> Option<&str> {
            self.current_fiber.as_ref().unwrap().name()
        }
    */
}
