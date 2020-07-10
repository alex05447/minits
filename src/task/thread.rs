use {
    super::{task::Task, task_system::YieldKey},
    crate::{Handle, TaskSystem, ThreadInitFiniCallback},
    minifiber::Fiber,
    std::sync::Arc,
};

/// Per-thread task system state.
/// Intended to be thread-local.
pub(crate) struct Thread {
    /// "Main" thread - `0`.
    /// Worker threads - `1 ..= num_worker_threads`.
    index: u32,

    /// Thread name for tracing / debugging, initiated on thread startup.
    #[cfg(feature = "tracing")]
    name: Option<String>,

    /// If we're using fibers, it's important to switch back to the fiber
    /// created from the worker thread before the thread exits
    /// for thread cleanup to work properly.
    /// Worker threads keep their initial fiber here.
    thread_fiber: Option<Fiber>,

    /// Worker thread finalizer callback is stored here to be ran when the worker thread is about to exit.
    fini: Option<Arc<dyn ThreadInitFiniCallback>>,

    /// Current fiber ran by the thread, set before the switch.
    fiber: Option<Fiber>,

    /// Current task ran by the thread.
    task: Option<Task>,

    /// Previous fiber ran by the thread, to be returned to the fiber pool.
    fiber_to_free: Option<Fiber>,

    /// Yield key of the previous fiber ran by the thread, now yielded.
    yield_key: Option<YieldKey>,
}

impl Thread {
    #[allow(unused_variables)]
    pub(crate) fn new<N: Into<Option<String>>>(
        name: N,
        index: u32,
        fini: Option<Arc<dyn ThreadInitFiniCallback>>,
    ) -> Self {
        Self {
            index,

            #[cfg(feature = "tracing")]
            name: name.into(),

            thread_fiber: None,

            fini,

            fiber: None,
            task: None,

            fiber_to_free: None,

            yield_key: None,
        }
    }

    /// Returns the index of the task system thread in which this function is called.
    ///
    /// `0` means `main thread`,
    /// value in range `1 ..= num_worker_threads` means one of the worker threads.
    pub(crate) fn index(&self) -> u32 {
        self.index
    }

    #[cfg(feature = "tracing")]
    pub(crate) fn name_or_unnamed(&self) -> &str {
        self.name
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or("<unnamed>")
    }

    pub(crate) fn set_thread_fiber(&mut self, fiber: Fiber) {
        if self.thread_fiber.replace(fiber).is_some() {
            panic!("thread fiber set more than once");
        }
    }

    /// Takes the thread fiber and switches the thread to it.
    /// Called once on task system shutdown.
    pub(crate) fn switch_to_thread_fiber(&mut self) {
        // >>>>>>>> FIBER SWITCH >>>>>>>>
        self.thread_fiber
            .take()
            .expect("thread fiber not set")
            .switch_to();
        // <<<<<<<< FIBER SWITCH <<<<<<<<
    }

    #[cfg(feature = "tracing")]
    pub(crate) fn fiber_name_or_unnamed(&self) -> &str {
        self.fiber
            .as_ref()
            .map_or("<none>", |fiber| fiber.name().unwrap_or("<unnamed>"))
    }

    pub(crate) fn has_current_fiber(&self) -> bool {
        self.fiber.is_some()
    }

    /// Set the thread's current fiber.
    /// Called before switching to it.
    pub(crate) fn set_fiber(&mut self, fiber: Fiber) {
        if self.fiber.replace(fiber).is_some() {
            panic!("current fiber set more than once")
        }
    }

    pub(crate) fn switch_to_current_fiber(&self) {
        // >>>>>>>> FIBER SWITCH >>>>>>>>
        self.fiber
            .as_ref()
            .expect("current fiber not set")
            .switch_to();
        // <<<<<<<< FIBER SWITCH <<<<<<<<
    }

    pub(crate) fn take_fiber(&mut self) -> Fiber {
        self.fiber.take().expect("current fiber not set")
    }

    /// Set the thread's current task.
    /// Called before executing it.
    pub(crate) fn set_task(&mut self, task: Task) {
        if self.task.replace(task).is_some() {
            panic!("current task set more than once");
        }
    }

    /// Set the thread's current task and execute it.
    /// The task may yield if we're using fibers.
    pub(crate) fn execute_task(&mut self, mut task: Task, task_system: &TaskSystem) {
        // Extract the task closure from the task context so that it lives on the fiber stack here.
        debug_assert!(!task.is_none());
        let mut closure = task.take_task();
        debug_assert!(task.is_none());
        debug_assert!(closure.is_once());

        // Set the rest of the task context as the thread's current task.
        self.set_task(task);

        // Call the task closure, passing it the task system.
        // The task may yield.
        // >>>>>>>> (POTENTIAL) FIBER SWITCH >>>>>>>>
        unsafe { closure.execute_once_unchecked(task_system) };
        // <<<<<<<< (POTENTIAL) FIBER SWITCH <<<<<<<<
        // The task is finished.
        // We might be in a different thread if the task yielded.
    }

    pub(crate) fn task(&self) -> &Task {
        self.task.as_ref().expect("current task not set")
    }

    #[cfg(feature = "profiling")]
    pub(crate) fn is_main_task(&self) -> bool {
        self.task().main_thread
    }

    pub(crate) fn take_task(&mut self) -> Task {
        self.task.take().expect("current task not set")
    }

    pub(crate) fn task_name(&self) -> Option<&str> {
        self.task().task_name()
    }

    #[cfg(any(feature = "tracing", feature = "profiling"))]
    pub(crate) fn task_name_or_unnamed(&self) -> &str {
        self.task().task_name_or_unnamed()
    }

    #[cfg(feature = "tracing")]
    pub(crate) fn scope_name_or_unnamed(&self) -> &str {
        self.task().scope_name_or_unnamed()
    }

    pub(crate) fn scope_name(&self) -> Option<&str> {
        self.task().scope_name()
    }

    pub(crate) fn handle(&self) -> &Handle {
        &self.task().handle
    }

    /// Take the currently set fiber and assign it to to-free fiber,
    /// to be returned to the fiber pool by the next fiber ran by this thread.
    pub(crate) fn free_fiber(&mut self) {
        let fiber = self.take_fiber();
        if self.fiber_to_free.replace(fiber).is_some() {
            panic!("fiber to free set more than once");
        }
    }

    pub(crate) fn take_fiber_to_free(&mut self) -> Option<Fiber> {
        self.fiber_to_free.take()
    }

    /// Set the yield key of the fiber to be freed.
    pub(crate) fn set_yield_key(&mut self, yield_key: YieldKey) {
        if self.yield_key.replace(yield_key).is_some() {
            panic!("yield key set more than once");
        }
    }

    pub(crate) fn take_yield_key(&mut self) -> Option<YieldKey> {
        self.yield_key.take()
    }

    /// Ran once at task system shutdown.
    pub(crate) fn execute_thread_fini(&mut self) {
        if let Some(fini) = self.fini.take() {
            fini(self.index);
        }
    }
}
