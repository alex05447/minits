use std::num::NonZeroUsize;

use super::task_context::TaskContext;

use minifiber::Fiber;

pub(super) struct FiberAndTask {
    pub(super) fiber: Fiber,
    pub(super) task: TaskContext,
}

enum YieldStatus {
    NotReady, // Pushed to the yield queue but may not be switched to yet.
    Ready,    // May be picked up and switched to.
}

pub(super) struct YieldQueue {
    fibers_and_tasks: Vec<FiberAndTask>,
    keys: Vec<NonZeroUsize>,
    statuses: Vec<YieldStatus>,
}

impl YieldQueue {
    pub(super) fn new() -> Self {
        Self {
            fibers_and_tasks: Vec::new(),
            keys: Vec::new(),
            statuses: Vec::new(),
        }
    }

    pub(super) fn yield_fiber_and_task(
        &mut self,
        fiber_and_task: FiberAndTask,
        yield_key: NonZeroUsize,
    ) {
        self.fibers_and_tasks.push(fiber_and_task);
        self.keys.push(yield_key);
        self.statuses.push(YieldStatus::NotReady)
    }

    pub(super) fn try_resume_fiber_and_task(
        &mut self,
        yield_key: NonZeroUsize,
    ) -> Option<FiberAndTask> {
        if let Some(idx) = self.keys.iter().position(|y| *y == yield_key) {
            let status = &mut self.statuses[idx];

            match status {
                // Fiber was yielded but is not ready to be switched to.
                // We signal the yielding thread that the fiber is ready to be resumed.
                YieldStatus::NotReady => {
                    *status = YieldStatus::Ready;
                    None
                }

                // Fiber was yielded and is ready to be switched to - erase and return it.
                YieldStatus::Ready => {
                    let fiber_and_task = self.fibers_and_tasks.swap_remove(idx);
                    self.keys.swap_remove(idx);
                    self.statuses.swap_remove(idx);
                    Some(fiber_and_task)
                }
            }

        // No fiber was yielded with this key yet.
        } else {
            None
        }
    }
}
