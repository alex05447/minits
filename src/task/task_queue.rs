use {
    super::{
        task::Task,
        task_system::YieldKey,
        yield_queue::{TaskAndFiber, YieldQueue},
    },
    std::{
        collections::VecDeque,
        sync::{Condvar, Mutex, MutexGuard},
        time::Duration,
    },
};

/// A task, ready for execution, returned from the task queue.
pub(crate) enum NewOrResumedTask {
    /// A newly added task, not yet ran.
    New(Task),
    /// A resumed task, previously already ran and yielded.
    /// Only ever used if we use fibers.
    Resumed(TaskAndFiber),
}

impl NewOrResumedTask {
    /// Rerturns `true` if the task must only run in the "main" thread.
    fn main_thread(&self) -> bool {
        match self {
            NewOrResumedTask::New(task) => task.main_thread,
            NewOrResumedTask::Resumed(task) => task.task.main_thread,
        }
    }
}

/// A single thread type's ("main" or worker) mutex-protected task queue.
struct LockedTaskQueue {
    // New tasks.
    new: VecDeque<Task>,
    // Resumed tasks.
    // Only ever used if we use fibers.
    resumed: VecDeque<TaskAndFiber>,
    // Number of threads of this type blocked / sleeping / waiting
    // for a new / resumed task to be added to this queue.
    num_waiters: u32,
    // Set to `true` when the "main" thread is shutting down the task system
    // and waking up the worker threads.
    exit: bool,
}

impl LockedTaskQueue {
    fn new() -> Self {
        Self {
            new: VecDeque::new(),
            resumed: VecDeque::new(),
            num_waiters: 0,
            exit: false,
        }
    }

    // Called before a blocking wait.
    fn inc_num_waiters(&mut self) {
        self.num_waiters += 1;
    }

    // Called after a blocking wait.
    fn dec_num_waiters(&mut self) {
        debug_assert!(self.num_waiters > 0);
        self.num_waiters -= 1;
    }
}

/// A single thread type's ("main" or worker) waitable task queue.
struct ThreadTaskQueue {
    queue: Mutex<LockedTaskQueue>,
    condvar: Condvar,
}

impl ThreadTaskQueue {
    fn new() -> Self {
        Self {
            queue: Mutex::new(LockedTaskQueue::new()),
            condvar: Condvar::new(),
        }
    }

    /// Adds a new or resumed task to the task queue.
    /// Returns `true` if a single thread, blocked waiting on this queue was woken up.
    /// Returns `false` if no threads were blocked and thus were not woken up.
    fn add_task(&self, task: NewOrResumedTask) -> bool {
        let mut queue = self.queue.lock().unwrap();

        match task {
            NewOrResumedTask::New(task) => {
                debug_assert!(!task.is_none());
                queue.new.push_back(task);
            }
            NewOrResumedTask::Resumed(task) => {
                // NOTE - resumed "main" task is very much not `is_none()`.
                queue.resumed.push_back(task);
            }
        }

        // Wake up a waiting thread, if any.
        if queue.num_waiters > 0 {
            self.wake_up_one();
            true
        } else {
            false
        }
    }

    fn try_get_resumed_task(&self) -> Option<TaskAndFiber> {
        self.lock().resumed.pop_front()
    }

    /// Tries to pop a resumed task from the queue;
    /// failing that tries to pop a new task.
    fn try_get_task(&self) -> Option<NewOrResumedTask> {
        Self::try_get_task_locked(&mut self.lock())
    }

    /// Tries to pop a resumed task from the `queue`;
    /// failing that tries to pop a new task.
    fn try_get_task_locked(
        queue: &mut MutexGuard<'_, LockedTaskQueue>,
    ) -> Option<NewOrResumedTask> {
        queue
            .resumed
            .pop_front()
            .map(NewOrResumedTask::Resumed)
            .or_else(|| queue.new.pop_front().map(NewOrResumedTask::New))
    }

    fn lock(&self) -> MutexGuard<'_, LockedTaskQueue> {
        self.queue.lock().unwrap()
    }

    fn wake_up_one(&self) {
        self.condvar.notify_one();
    }

    /// Wakes up and signals all worker threads to exit on task system shutdown.
    fn wake_up_all(&self) {
        loop {
            self.condvar.notify_all();

            let mut queue = self.lock();
            queue.exit = true;

            if queue.num_waiters == 0 {
                break;
            }
        }
    }
}

/// The `TaskSystem`'s task queue.
///
/// Contains the new tasks added to the task system which are ready to run;
/// yielded tasks, currently waiting for spawned children tasks / IO completion
/// (including the "main" task representing the `TaskSystem` "main" / owner thread
/// whenever the "main thread waits for a spawned task / IO completion);
/// resumed tasks and their fibers (only if we use fibers).
///
/// Currently implemented as a simple `Mutex`-protected `VecDeque`, no lockless stuff going on.
pub(crate) struct TaskQueue {
    // Timeout duration of a wait when no new / resumed tasks are available.
    // Should be as high as possible.
    // Low values might lead to too many spurious wakeups and decreased efficiency.
    // High values might expose bugs in thread wakeup logic that would otherwise be hidden.
    wait_timeout: Duration,

    // "Main" and worker thread new and resumed task queues.
    main_queue: ThreadTaskQueue,
    worker_queue: ThreadTaskQueue,

    // Yielded task / fiber queue.
    // Only used if we are using fibers.
    yield_queue: Mutex<YieldQueue>,
}

impl TaskQueue {
    pub(crate) fn new(wait_timeout: Duration) -> Self {
        Self {
            wait_timeout,

            main_queue: ThreadTaskQueue::new(),
            worker_queue: ThreadTaskQueue::new(),

            yield_queue: Mutex::new(YieldQueue::new()),
        }
    }

    /// Add a new task to the task queue.
    /// `main_thread` is `true` if this is called from the "main" thread,
    /// regardless if the `task` is a "main" thread task or not.
    pub(crate) fn add_new_task(&self, task: Task, main_thread: bool) {
        self.add_task(NewOrResumedTask::New(task), main_thread)
    }

    /// Add a resumed task to the task queue.
    /// `main_thread` is `true` if this is called from the "main" thread,
    /// regardless if the `task` is a "main" thread task or not.
    pub(crate) fn add_resumed_task(&self, task: TaskAndFiber, main_thread: bool) {
        self.add_task(NewOrResumedTask::Resumed(task), main_thread)
    }

    /// Try to check if there's any tasks / fibers ready to be resumed.
    /// If `main_thread` == `true`, checks the main thread queue first, then the worker thread queue.
    /// Else checks the worker thread queue only.
    /// Only ever called if we use fibers.
    pub(crate) fn try_get_resumed_task(&self, main_thread: bool) -> Option<TaskAndFiber> {
        if main_thread {
            self.main_queue.try_get_resumed_task()
        } else {
            None
        }
        .or_else(|| self.worker_queue.try_get_resumed_task())
    }

    /// (Try) to get a new or resumed task from the task queue.
    ///
    /// `main_thread` is `true` if this is called from the "main" thread
    /// and we may thus pop "main" thread tasks.
    ///
    /// If `wait` is `true`, the method waits / blocks / sleeps on the queue
    /// until a task becomes available, or until the (worker) thread gets signaled to exit.
    pub(crate) fn get_task(&self, main_thread: bool, wait: bool) -> Option<NewOrResumedTask> {
        if main_thread {
            self.get_task_main_thread(wait)
        } else {
            self.get_task_worker_thread(wait)
        }
    }

    pub(crate) fn lock_yield_queue(&self) -> MutexGuard<'_, YieldQueue> {
        self.yield_queue.lock().unwrap()
    }

    /// Try to resume a `Ready` task and its fiber with the `yield_key`,
    /// or mark a `NotReady` task with the `yield_key` as now `Ready`.
    /// Only ever called if we use fibers.
    pub(crate) fn try_resume_task(&self, yield_key: YieldKey) -> Option<TaskAndFiber> {
        self.lock_yield_queue().try_resume_task(yield_key)
    }

    /// Wakes up and signals all worker threads to exit on task system shutdown.
    pub(crate) fn wake_up_workers(&self) {
        self.worker_queue.wake_up_all()
    }

    /// Add a new or resumed task to the task queue.
    /// `main_thread` is `true` if this is called from the "main" thread,
    /// regardless if the `task` is a "main" thread task or not.
    fn add_task(&self, task: NewOrResumedTask, main_thread: bool) {
        // If it's a "main" thread task, just add it to the "main" thread queue
        // and wake up the main thread if it was waiting / sleeping / blocked.
        if task.main_thread() {
            self.main_queue.add_task(task);

        // If it's a worker thread task, add it to the worker thread queue
        // and wake up one worker thread if there was one was waiting / sleeping / blocked.
        // However, if there were no waiting worker threads, the task system might be fully
        // busy, and we also try to wake up the main thread to help out, in case it was waiting
        // (unless we are currently in the main thread, of course).
        } else {
            let woken_up = self.worker_queue.add_task(task);

            if !woken_up && !main_thread {
                self.main_queue.wake_up_one();
            }
        }
    }

    /// (Try) to get a new or resumed task from the "main" (first) or worker (second) task queue.
    ///
    /// May only return `None` if `wait` is `false` and there are no tasks in any queue;
    /// otherwise blocks until a task is available.
    pub fn get_task_main_thread(&self, wait: bool) -> Option<NewOrResumedTask> {
        let main_queue = &self.main_queue;
        let worker_queue = &self.worker_queue;

        let mut queue = main_queue.lock();

        // Never set for "main" thread.
        debug_assert!(!queue.exit);

        loop {
            if let Some(task) = ThreadTaskQueue::try_get_task_locked(&mut queue)
                .or_else(|| worker_queue.try_get_task())
            {
                return Some(task);
            }

            // No tasks in both queues and we don't want to wait - return `None`.
            if !wait {
                return None;
            }

            // Otherwise wait.
            // Loop until we pop a "main" / worker thread task.
            // NOTE - we are in the "main" thread and thus are never signaled to exit.

            queue.inc_num_waiters();
            queue = main_queue
                .condvar
                .wait_timeout(queue, self.wait_timeout)
                .unwrap()
                .0;
            queue.dec_num_waiters();

            debug_assert!(!queue.exit);
        }
    }

    /// (Try) to get a new or resumed task from the worker task queue.
    ///
    /// Will return `None` if `wait` is `false` and there are no tasks in any queue;
    /// or if the worker thread(s) were signaled to exit by the main thread.
    pub fn get_task_worker_thread(&self, wait: bool) -> Option<NewOrResumedTask> {
        let worker_queue = &self.worker_queue;

        let mut queue = worker_queue.lock();

        loop {
            if queue.exit {
                return None;
            }

            if let Some(task) = ThreadTaskQueue::try_get_task_locked(&mut queue) {
                return Some(task);
            }

            // No tasks in the worker queue and we don't want to wait - return `None`.
            if !wait {
                return None;
            }

            // Otherwise wait.
            // Loop until we pop a task, or are signaled to exit.

            queue.inc_num_waiters();
            queue = worker_queue
                .condvar
                .wait_timeout(queue, self.wait_timeout)
                .unwrap()
                .0;
            queue.dec_num_waiters();
        }
    }
}
