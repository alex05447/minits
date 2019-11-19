use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex, MutexGuard,
};
use std::time::Duration;

use minievent::{wait_for_one, waitable::Waitable, Event};

use super::task_context::TaskContext;
use super::yield_queue::{FiberAndTask, YieldQueue};

pub(super) struct TaskQueue {
    wait_timeout: Duration,

    main_task_queue: Mutex<VecDeque<TaskContext>>,
    worker_task_queue: Mutex<VecDeque<TaskContext>>,

    yield_queue: Mutex<YieldQueue>,

    main_resumed_queue: Mutex<VecDeque<FiberAndTask>>,
    worker_resumed_queue: Mutex<VecDeque<FiberAndTask>>,

    // Manual reset event. Signaled to wake up the main thread when
    // a main thread task is pushed to the queue.
    main_event: Event,

    // Manual reset event. Signaled to wake up a worker (or the main) thread when
    // a worker thread task is pushed to the queue.
    worker_event: Event,

    // The number of threads blocked on an empty queue.
    num_waiting_threads: AtomicUsize,
}

impl TaskQueue {
    pub(super) fn new(wait_timeout: Duration) -> Self {
        Self {
            wait_timeout,

            main_task_queue: Mutex::new(VecDeque::new()),
            worker_task_queue: Mutex::new(VecDeque::new()),

            yield_queue: Mutex::new(YieldQueue::new()),

            main_resumed_queue: Mutex::new(VecDeque::new()),
            worker_resumed_queue: Mutex::new(VecDeque::new()),

            main_event: Event::new_auto(false, None),
            worker_event: Event::new_auto(false, None),

            num_waiting_threads: AtomicUsize::new(0),
        }
    }

    pub(super) fn add_task(&self, task: TaskContext) {
        let (mut task_queue, event) = self.get_task_queue_and_event(task.main_thread);

        task_queue.push_back(task);

        // If the wait counter is positive, wake one thread.
        if self.need_to_wake() {
            event.set();
        }
    }

    /// Try to check if there's any fibers ready to be resumed.
    /// If `is_main_thread` == `true`, checks the main thread queue first, then the worker thread queue.
    /// Else checks the worker thread queue only.
    pub(super) fn try_get_resumed_fiber_and_task(
        &self,
        is_main_thread: bool,
    ) -> Option<FiberAndTask> {
        if is_main_thread {
            if let Some(fiber_and_task) = self.main_resumed_queue.lock().unwrap().pop_front() {
                return Some(fiber_and_task);
            }
        }

        self.worker_resumed_queue.lock().unwrap().pop_front()
    }

    pub(super) fn return_resumed_fiber_and_task(&self, fiber_and_task: FiberAndTask) {
        if fiber_and_task.task.main_thread {
            self.main_resumed_queue
                .lock()
                .unwrap()
                .push_back(fiber_and_task);
        } else {
            self.worker_resumed_queue
                .lock()
                .unwrap()
                .push_back(fiber_and_task);
        }
    }

    /// "Main" thread checks the main thread task queue first before checking the worker thread queue.
    /// Worker threads check the worker thread queue only.
    pub(super) fn try_get_task(&self, is_main_thread: bool) -> Option<TaskContext> {
        if is_main_thread {
            if let Some(task) = self.main_task_queue.lock().unwrap().pop_front() {
                return Some(task);
            }
        }

        self.worker_task_queue.lock().unwrap().pop_front()
    }

    pub(super) fn wait(&self, is_main_thread: bool) {
        // #[cfg(feature = "profiling")]
        // let _scope = Remotery::scope("<wait for task>", rmtSampleFlags::RMTSF_None);

        self.before_wait();

        // Main thread wait for both events.
        // We want to be woken up ASAP when a main thread task is enqueued / fiber is resumed,
        // but we also help out worker threads when waiting.
        if is_main_thread {
            let waitables = [
                &self.worker_event as &dyn Waitable,
                &self.main_event as &dyn Waitable,
            ];
            wait_for_one(&waitables, self.wait_timeout);

        // Worker threads only care about worker thread tasks / fibers.
        } else {
            self.worker_event.wait(self.wait_timeout);
        };

        self.after_wait();
    }

    pub(super) fn lock_yield_queue(&self) -> MutexGuard<'_, YieldQueue> {
        self.yield_queue.lock().unwrap()
    }

    pub(super) fn try_resume_fiber_and_task(
        &self,
        yield_key: NonZeroUsize,
    ) -> Option<FiberAndTask> {
        self.lock_yield_queue().try_resume_fiber_and_task(yield_key)
    }

    pub(super) fn push_resumed_fiber_and_task(&self, fiber_and_task: FiberAndTask) {
        let (mut resumed_queue, event) =
            self.get_resumed_queue_and_event(fiber_and_task.task.main_thread);

        resumed_queue.push_back(fiber_and_task);

        // If the wait counter is positive, wake one thread.
        if self.need_to_wake() {
            event.set();
        }
    }

    fn before_wait(&self) {
        self.num_waiting_threads.fetch_add(1, Ordering::SeqCst);
    }

    fn after_wait(&self) {
        self.num_waiting_threads.fetch_sub(1, Ordering::SeqCst);
    }

    fn get_task_queue_and_event(
        &self,
        is_main_thread: bool,
    ) -> (MutexGuard<'_, VecDeque<TaskContext>>, &Event) {
        if is_main_thread {
            (self.main_task_queue.lock().unwrap(), &self.main_event)
        } else {
            (self.worker_task_queue.lock().unwrap(), &self.worker_event)
        }
    }

    fn get_resumed_queue_and_event(
        &self,
        is_main_thread: bool,
    ) -> (MutexGuard<'_, VecDeque<FiberAndTask>>, &Event) {
        if is_main_thread {
            (self.main_resumed_queue.lock().unwrap(), &self.main_event)
        } else {
            (
                self.worker_resumed_queue.lock().unwrap(),
                &self.worker_event,
            )
        }
    }

    fn need_to_wake(&self) -> bool {
        self.num_waiting_threads.load(Ordering::SeqCst) > 0
    }

    pub(super) fn wake_up_worker(&self) {
        self.worker_event.set();
    }
}
