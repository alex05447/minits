use {
    super::{task::Task, task_system::YieldKey},
    minifiber::Fiber,
};

/// A pair of task context / its fiber,
/// added to the `YieldQueue` when yielded,
/// removed from the `YieldQueue` when resumed.
pub(crate) struct TaskAndFiber {
    pub(crate) task: Task,
    pub(crate) fiber: Fiber,
}

enum YieldStatus {
    /// Pushed to the yield queue, but may not be switched to yet.
    NotReady,
    /// May be picked up and switched to.
    Ready,
}

/// Contains yielded task contexts, their fibers and yield keys.
pub(crate) struct YieldQueue {
    tasks: Vec<TaskAndFiber>,
    keys: Vec<YieldKey>,
    statuses: Vec<YieldStatus>,
}

impl YieldQueue {
    pub(crate) fn new() -> Self {
        Self {
            tasks: Vec::new(),
            keys: Vec::new(),
            statuses: Vec::new(),
        }
    }

    /// Add a yielded task and its fiber to the yield queue with the `yield_key`,
    /// with the `NotReady` status.
    pub(crate) fn yield_task(&mut self, task: TaskAndFiber, yield_key: YieldKey) {
        self.tasks.push(task);
        debug_assert!(!self.keys.contains(&yield_key), "non-unique yield key");
        self.keys.push(yield_key);
        self.statuses.push(YieldStatus::NotReady)
    }

    /// Try to resume a `Ready` task and its fiber with the `yield_key`,
    /// or mark a `NotReady` task with the `yield_key` as now `Ready`.
    pub(crate) fn try_resume_task(&mut self, yield_key: YieldKey) -> Option<TaskAndFiber> {
        self.keys
            .iter()
            .position(|y| *y == yield_key)
            .and_then(|idx| {
                let status = unsafe { self.statuses.get_unchecked_mut(idx) };

                match status {
                    // Fiber was yielded but is not ready to be switched to.
                    // We signal the yielding thread that the fiber is ready to be resumed.
                    YieldStatus::NotReady => {
                        *status = YieldStatus::Ready;
                        None
                    }

                    // Fiber was yielded and is ready to be switched to - remove and return it.
                    YieldStatus::Ready => {
                        let task = self.tasks.swap_remove(idx);
                        self.keys.swap_remove(idx);
                        self.statuses.swap_remove(idx);
                        Some(task)
                    }
                }
            })
        // Else no fiber was yielded with this yield key yet.
    }
}
