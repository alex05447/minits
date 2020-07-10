use {
    super::{
        fiber_pool::{FiberPool, FiberPoolParams},
        task::Task,
        task_queue::{NewOrResumedTask, TaskQueue},
        thread::Thread,
        util::SendPtr,
        yield_queue::TaskAndFiber,
    },
    crate::{Handle, RangeTaskFn, Scope, TaskFn, TaskRange, ThreadInitFiniCallback},
    minifiber::Fiber,
    minithreadlocal::ThreadLocal,
    std::{
        mem,
        num::NonZeroUsize,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Barrier,
        },
        thread,
        time::Duration,
    },
};

#[cfg(feature = "asyncio")]
use miniiocp::IOCP;

#[cfg(feature = "profiling")]
use crate::profiler::TaskSystemProfiler;

/// Task system singleton object that manages the task system internal resources
/// like the thread pool, the fiber pool and the task queues,
/// and provides the public API to spawn and wait for tasks.
pub struct TaskSystem {
    task_queue: TaskQueue,

    /// Thread-local state used to communicate between the fibers
    /// during context switches.
    pub(crate) thread: ThreadLocal<Thread>,

    /// Used to synchronize the worker / FS thread(s) at initialization time
    /// to prevent them from accessing an uninitialized task system instance.
    thread_barrier: Barrier,

    /// Worker thread pool. Initiated on startup. Never grows or shrinks.
    worker_threads: Vec<thread::JoinHandle<()>>,

    /// Worker fiber pool. Initiated on startup. Never grows or shrinks.
    fiber_pool: FiberPool,
    /// If `true`, we use fibers and the fiber pool was initialized on startup with `> 0` fibers.
    use_fiber_pool: bool,
    allow_inline_tasks: bool,

    /// Used to wait for completion of all tasks added to the task system.
    global_task_handle: Handle,

    /// Signals the worker threads to break the scheduler loop and exit
    /// when the task system finalizes.
    exit_flag: AtomicBool,

    /// Fake "main" task handle. Technically unnecessary, but helps
    /// with the uniformoty of pre/postcondition task system asserts.
    /// TODO - fix this.
    main_task_handle: Handle,

    #[cfg(feature = "profiling")]
    pub(crate) profiler: Option<Box<TaskSystemProfiler>>,

    #[cfg(feature = "asyncio")]
    pub(crate) iocp: IOCP,

    #[cfg(feature = "asyncio")]
    pub(crate) fs_thread: Option<thread::JoinHandle<()>>,
}

/// Returned by `try_get_fiber_to_switch_to`.
enum FiberToSwitchTo {
    /// Returned a previously yielded, now resumed task and its fiber.
    Resumed(TaskAndFiber),
    /// Returned a free worker fiber from the fiber pool.
    Free(Fiber),
}

impl TaskSystem {
    /// Creates a new [`Handle`] for use with a [`Scope`].
    ///
    /// [`Handle`]: struct.Handle.html
    /// [`Scope`]: struct.Scope.html
    pub fn handle(&self) -> Handle {
        Handle::new()
    }

    /// Creates a new [`Scope`] object which provides borrow-safe task system functionality.
    /// Requires a [`Handle`] created by a previous call to [`handle`].
    ///
    /// [`Handle`]: struct.Handle.html
    /// [`Scope`]: struct.Scope.html
    /// [`handle`]: #method.handle
    pub fn scope<'h, 'ts>(&'ts self, handle: &'h Handle) -> Scope<'h, 'ts> {
        Scope::new(handle, &self, None)
    }

    /// Creates a new named [`Scope`] object which provides borrow-safe task system functionality.
    /// Requires a [`Handle`] created by a previous call to [`handle`].
    ///
    /// `name` may be retrieved by [`scope_name`] within the task closure.
    /// Requires "task_names" feature.
    ///
    /// [`Handle`]: struct.Handle.html
    /// [`Scope`]: struct.Scope.html
    /// [`handle`]: #method.handle
    /// [`scope_name`]: #method.scope_name
    pub fn scope_named<'h, 'ts, N: Into<String>>(
        &'ts self,
        handle: &'h Handle,
        name: N,
    ) -> Scope<'h, 'ts> {
        Scope::new(handle, &self, name.into())
    }

    /// Returns the name, if any, of the task in whose scope this function is called.
    /// Requires "task_names" feature.
    pub fn task_name(&self) -> Option<&str> {
        self.thread().task_name()
    }

    /// Returns the name, if any, of the [`task scope`] in which this function is called.
    /// Requires "task_names" feature.
    ///
    /// [`task scope`]: struct.Scope.html
    pub fn scope_name(&self) -> Option<&str> {
        self.thread().scope_name()
    }

    /// Returns the index of the task system thread in which this function is called.
    ///
    /// `0` means the "main" thread,
    /// value in range `1 ..= num_worker_threads` means one of the worker threads.
    pub fn thread_index(&self) -> u32 {
        self.thread().index()
    }

    /// Returns `true` if the method is called from the "main" thread.
    pub fn is_main_thread(&self) -> bool {
        self.thread_index() == 0
    }

    /// Returns the number of worker threads, spawned by the [`TaskSystem`]
    /// (as set by [`TaskSytemBuilder::num_worker_threads`] or default).
    ///
    /// [`TaskSystem`]: struct.TaskSystem.html
    /// [`TaskSytemBuilder::num_worker_threads`]: struct.TaskSytemBuilder.html#method.num_worker_threads
    pub fn num_worker_threads(&self) -> usize {
        self.worker_threads.len()
    }

    /// Submit the task for execution by the task system.
    /// It's up to the caller to ensure the task's closure does not outlive the task handle.
    pub(crate) unsafe fn submit<'h, F>(
        &self,
        h: &Handle,
        f: F,
        main_thread: bool,
        task_name: Option<String>,
        scope_name: Option<&str>,
    ) where
        F: TaskFn<'h>,
    {
        // Increment the task counters.
        self.global_task_handle.inc();
        h.inc();

        let task = Task::new(h, f, main_thread, task_name, scope_name);

        self.on_task_added(&task);

        self.task_queue.add_new_task(task, self.is_main_thread());
    }

    pub(crate) unsafe fn submit_range<'h, F>(
        &self,
        h: &'h Handle,
        f: F,
        range: TaskRange,
        multiplier: u32,
        task_name: Option<&str>,
        scope_name: Option<&str>,
    ) where
        F: RangeTaskFn<'h>,
    {
        // Empty range.
        if range.end <= range.start {
            return;
        }

        let range_size = range.end - range.start;

        // Including the main thread.
        let num_threads = (self.worker_threads.len() + 1) as u32;

        let num_chunks = (num_threads * multiplier).min(range_size);

        #[cfg(feature = "tracing")]
        self.trace_fork_task_range(h, &range, num_chunks, task_name, scope_name);

        self.fork_task_range(h, f, range, num_chunks, task_name, scope_name, false);
    }

    #[allow(unused_variables)]
    fn on_task_added(&self, task: &Task) {
        debug_assert!(!task.is_none());

        #[cfg(feature = "tracing")]
        self.trace_added_task(&task);
    }

    /// Block the current thread until all tasks associated with the task handle `h` are complete.
    /// Attempts to yield the current fiber, if any; otherwise attempts to execute the tasks inline.
    pub(crate) unsafe fn wait_for_handle(&self, h: &Handle) {
        self.wait_for_handle_impl(h, None);
    }

    /// Block the current thread until all tasks associated with the task handle `h` are complete.
    /// Attempts to yield the current fiber, if any; otherwise attempts to execute the tasks inline.
    pub(crate) unsafe fn wait_for_handle_named(&self, h: &Handle, scope_name: &str) {
        self.wait_for_handle_impl(h, Some(scope_name));
    }

    unsafe fn wait_for_handle_impl(&self, handle: &Handle, scope_name: Option<&str>) {
        // Trivial case - the waited-on handle is already complete.
        if handle.is_complete() {
            return;
        }

        let scope_name = scope_name.into();

        self.wait_for_handle_start(handle, scope_name);

        let yield_data = WaitForTaskData(handle);

        // Yield the fiber waiting for the task(s) to complete ...
        // >>>>>>>> (POTENTIAL) FIBER SWITCH >>>>>>>>
        self.yield_current_task(yield_data);
        // <<<<<<<< (POTENTIAL) FIBER SWITCH <<<<<<<<
        // ... and we're back.

        self.wait_for_handle_end(handle, scope_name);
    }

    /// Tracing and profiling hooks on wait start go here.
    #[allow(unused_variables)]
    fn wait_for_handle_start(&self, h: &Handle, scope_name: Option<&str>) {
        #[cfg(feature = "profiling")]
        self.profile_wait_start();

        #[cfg(feature = "tracing")]
        self.trace_wait_start(h, scope_name);
    }

    /// Tracing and profiling hooks on wait end go here.
    #[allow(unused_variables)]
    fn wait_for_handle_end(&self, h: &Handle, scope_name: Option<&str>) {
        #[cfg(feature = "tracing")]
        self.trace_wait_end(h, scope_name);

        #[cfg(feature = "profiling")]
        self.profile_wait_end();
    }

    /// Attempts to yield the current fiber, if any; otherwise attempts to execute the tasks inline,
    /// using the `yield_data` to determine completion, and the `yield_data` `YieldKey` to resume the yielded fiber, if any.
    pub(crate) fn yield_current_task<Y: YieldData>(&self, yield_data: Y) {
        // We use fibers and yield from a main thread fiber / worker thread scheduler fiber / worker fiber.
        if self.has_current_fiber() {
            debug_assert!(self.use_fiber_pool);

            // Loop until the dependency is complete.
            //
            // This might or might not have pathological scheduling issues in some corner cases.
            // Specifically, a question for case 3):
            // is it possible to create a task graph such that there are no free fibers awailable
            // and *all* threads are blocked on the fiber pool, awaiting a free fiber, leading to a deadlock?
            // TODO - look at this again. Maybe it would be possible to restructure things to
            // make such cases more well-behaved.
            //
            // Case 1): if we picked up a resumed task, we yield the current fiber and switch to it; all is good;
            // Case 2): if we picked up a free fiber, we yield the current fiber and switch to it,
            // running the scheduler loop there; all is good;
            // Case 3): no resumed fibers and we ran out of free fibers, and we don't allow inline task execution - we'll sleep
            // on the fiber pool, awaiting a free fiber; this is not very good as technically we are missing out on potential resumed tasks
            // which would otherwise be sufficient for our yield to make progress, but it's difficult to wait on two separate entities -
            // the resumed task part of the task queue, and the fiber pool - with two separate condvars, the former is not even independent
            // (it is shared with the new task part of the task queue);
            // Case 4): no resumed fibers on the first try and we ran out of free fibers, but we allow inline task execution and
            // we picked up a resumed task - same as case 1);
            // Case 5): no resumed fibers and we ran out of free fibers, but we allow inline task execution
            // and we picked up a new task - we'll execute it inline and hope by the time we're done
            // the dependency is complete; this is not too good, but OK - we're at least making progress;
            // Case 6): no resumed fibers, we ran out of free fibers, we allow inline task execution and did not pick up any tasks
            // - we'll run a busy loop (with a tiny `yield_now()`) until we hit any of the cases above;
            // this is similar to case 3) above; while case 3) sleeps *too much* and might miss a resumed task becoming available,
            // this case does not sleep *at all*.
            loop {
                let main_thread = self.is_main_thread();

                // We have a resumed/free fiber available - switch to it.
                // By the time `yield_to_fiber` returns, the dependency will be complete.
                if let Some(fiber) = self.try_get_fiber_to_switch_to(main_thread) {
                    self.yield_to_fiber(fiber, &yield_data, main_thread);
                    return;

                // We have no resumed fibers / ran out of free fibers, and allow inline execution.
                } else {
                    debug_assert!(self.allow_inline_tasks);

                    if let Some(task) = self.task_queue.get_task(main_thread, false) {
                        match task {
                            // Popped a new task - execute it inline.
                            NewOrResumedTask::New(task) => {
                                // Save the current task.
                                let current_task = self.take_task();

                                // Set the thread's current task and run it.
                                // The task may yield.
                                // >>>>>>>> (POTENTIAL) FIBER SWITCH >>>>>>>>
                                self.execute_task(task);
                                // <<<<<<<< (POTENTIAL) FIBER SWITCH <<<<<<<<
                                // The task is finished.
                                // We might be in a different thread if the task yielded.

                                // Take the current task, decrement the task handle(s),
                                // maybe resume a yielded fiber waiting for this task's handle.
                                self.finish_task(self.take_task());

                                // Restore the current task.
                                self.set_task(current_task);

                                // We finished the right task / were resumed via other means - break the loop.
                                // Otherwise keep going.
                                if yield_data.is_complete() {
                                    return;
                                }
                            }
                            // Popped a resumed task - switch to it.
                            // By the time `yield_to_fiber` returns, the dependency will be complete.
                            NewOrResumedTask::Resumed(task) => {
                                self.yield_to_fiber(
                                    FiberToSwitchTo::Resumed(task),
                                    &yield_data,
                                    main_thread,
                                );
                                return;
                            }
                        }
                    }

                    // TODO - FIXME
                    thread::yield_now();
                }
            } // loop

        // We're not using fibers at all.
        } else {
            debug_assert!(!self.use_fiber_pool);

            // Save the current task.
            let current_task = self.take_task();

            // (Busy) loop, executing tasks inline until the yield dependency completes.
            // The tasks may not yield.
            loop {
                if let Some(task) = self.task_queue.get_task(self.is_main_thread(), true) {
                    match task {
                        // Popped a new task - execute it inline.
                        NewOrResumedTask::New(task) => {
                            // Set the thread's current task and run it.
                            self.execute_task(task);
                            // The task is finished.

                            // Take the current task, decrement the task handle(s).
                            self.finish_task(self.take_task());

                            // We finished the right task / were resumed via other means - break the loop.
                            // Otherwise keep going.
                            if yield_data.is_complete() {
                                break;
                            }
                        }
                        // This may not happen if we're not using fibers.
                        NewOrResumedTask::Resumed(_) => {
                            unreachable!();
                        }
                    }
                }

                // TODO - FIXME
                thread::yield_now();
            } // loop

            // Restore the current task.
            self.set_task(current_task);
        }
    }

    fn split_range(range: TaskRange, num_chunks: u32) -> ((TaskRange, u32), (TaskRange, u32)) {
        debug_assert!(num_chunks > 1);

        let num_chunks_l = num_chunks / 2;
        let num_chunks_r = num_chunks - num_chunks_l;

        debug_assert!(num_chunks_l >= 1);
        debug_assert!(num_chunks_r >= 1);

        debug_assert!(range.end > range.start);
        let range_size = range.end - range.start;
        let range_l = range.start..range.start + range_size / 2;
        let range_r = range_l.end..range.end;

        debug_assert!(range_l.end > range_l.start);
        debug_assert!(range_r.end > range_r.start);

        // println!(
        //     "Split {}..{} into {}..{} and {}..{}"
        //     ,range.start
        //     ,range.end
        //     ,range_l.start
        //     ,range_l.end
        //     ,range_r.start
        //     ,range_r.end
        // );

        ((range_l, num_chunks_l), (range_r, num_chunks_r))
    }

    #[allow(unused_variables)]
    unsafe fn fork_task_range<'h, F>(
        &self,
        h: &Handle,
        f: F,
        range: TaskRange,
        num_chunks: u32,
        task_name: Option<&str>,
        scope_name: Option<&str>,
        inline: bool,
    ) where
        F: RangeTaskFn<'h>,
    {
        // Empty range.
        if range.end <= range.start {
            return;
        }

        let range_size = range.end - range.start;
        debug_assert!(num_chunks <= range_size);

        // Trivial cases where the range is `1` (maybe don't use task ranges in this case)
        // or `num_chunks` is `1` (single-threaded mode).
        if range_size == 1 || num_chunks == 1 {
            self.execute_task_range(h, f, range, task_name, scope_name, inline);

        // Else subdivide the range in half.
        } else {
            let ((range_l, num_chunks_l), (range_r, num_chunks_r)) =
                TaskSystem::split_range(range.clone(), num_chunks);

            // If left subrange cannot be further divided, delegate its execution to a different thread in a task.
            if num_chunks_l == 1 {
                self.execute_task_range(h, f.clone(), range_l, task_name, scope_name, false);

            // Else delegate its further subdivision to a different thread.
            } else {
                let fork_task_name = if cfg!(feature = "task_names") {
                    Some(format!(
                        "Fork `{}` [{}..{})",
                        task_name.unwrap_or("<unnamed>"),
                        range_l.start,
                        range_l.end
                    ))
                } else {
                    None
                };

                let f = f.clone();

                self.submit(
                    h,
                    move |ts: &TaskSystem| {
                        ts.fork_task_range(
                            ts.task_handle(),
                            f,
                            range_l,
                            num_chunks_l,
                            task_name,
                            ts.scope_name(),
                            true,
                        );
                    },
                    false,
                    fork_task_name,
                    scope_name,
                );
            }

            // If right subrange cannot be further divided, execute it inline, if possible.
            if num_chunks_r == 1 {
                self.execute_task_range(h, f, range_r, task_name, scope_name, inline);

            // Else subdivide it further inline.
            } else {
                self.fork_task_range(h, f, range_r, num_chunks_r, task_name, scope_name, inline);
            }
        }
    }

    unsafe fn execute_task_range<'h, F>(
        &self,
        h: &Handle,
        f: F,
        range: TaskRange,
        task_name: Option<&str>,
        scope_name: Option<&str>,
        inline: bool,
    ) where
        F: RangeTaskFn<'h>,
    {
        let task_name = if cfg!(feature = "task_names") {
            Some(format!(
                "{} [{}..{})",
                task_name.unwrap_or("<unnamed>"),
                range.start,
                range.end
            ))
        } else {
            None
        };

        // Execute the range inline.
        if inline {
            #[cfg(feature = "profiling")]
            let _scope = self.profiler_scope(
                task_name
                    .as_ref()
                    .map(|n| n.as_str())
                    .unwrap_or("<unnamed>"),
            );

            f(range, self);

        // Spawn a task to execute the range.
        } else {
            self.submit(
                h,
                move |ts: &TaskSystem| {
                    #[cfg(feature = "profiling")]
                    let _scope = ts.profiler_scope(ts.task_name_or_unnamed());

                    f(range, ts);
                },
                false,
                task_name,
                scope_name,
            );
        }
    }

    pub(crate) fn task_handle(&self) -> &Handle {
        self.thread().handle()
    }

    #[cfg(feature = "profiling")]
    pub(crate) fn task_name_or_unnamed(&self) -> &str {
        self.thread().task_name_or_unnamed()
    }

    #[cfg(feature = "profiling")]
    pub(crate) fn is_main_task(&self) -> bool {
        self.thread().is_main_task()
    }

    /// Creates an uninitialized task system instance to be boxed and fully initialized later.
    pub(crate) fn new(
        num_worker_threads: u32,
        task_wait_timeout: Duration,
        fiber_wait_timeout: Duration,
        allow_inline_tasks: bool,
    ) -> Self {
        let num_threads = if cfg!(feature = "asyncio") {
            num_worker_threads + 2
        } else {
            num_worker_threads + 1
        };

        Self {
            task_queue: TaskQueue::new(task_wait_timeout),

            thread: ThreadLocal::new().unwrap(),

            thread_barrier: Barrier::new(num_threads as _),

            worker_threads: Vec::with_capacity(num_worker_threads as _),

            fiber_pool: FiberPool::new(fiber_wait_timeout),
            use_fiber_pool: false,
            allow_inline_tasks,

            global_task_handle: Handle::new(),

            exit_flag: AtomicBool::new(false),

            main_task_handle: Handle::new(),

            #[cfg(feature = "profiling")]
            profiler: None,

            #[cfg(feature = "asyncio")]
            iocp: IOCP::new(1).unwrap(),

            #[cfg(feature = "asyncio")]
            fs_thread: None,
        }
    }

    /// Initialize the fiber pool with `num_fibers` > `0` worker fibers with `stack_size` bytes of stack space.
    /// NOTE - `self` must be allocated on the heap as the pointer is used by the fiber entry point.
    pub(crate) fn init_fiber_pool(&mut self, num_fibers: u32, stack_size: usize) {
        debug_assert!(num_fibers > 0);

        // Worker fiber entry point.
        let task_system = unsafe { SendPtr::new(self) };
        let entry_point = move || {
            task_system.fiber_entry_point();
        };

        let fiber_pool_params = FiberPoolParams {
            num_fibers,
            stack_size,
            entry_point,
        };

        self.fiber_pool.initialize(fiber_pool_params);

        self.use_fiber_pool = true;
    }

    /// Initialize the thread pool with `num_worker_threads` worker threads with `stack_size` bytes of stack space.
    /// NOTE - `self` must be allocated on the heap as the pointer is used by the worker thread entry point.
    pub(crate) fn create_worker_threads(
        &mut self,
        num_worker_threads: u32,
        stack_size: usize,
        thread_init: Option<Arc<dyn ThreadInitFiniCallback>>,
        thread_fini: Option<Arc<dyn ThreadInitFiniCallback>>,
    ) {
        let task_system = unsafe { SendPtr::new(self) };

        for worker_index in 0..num_worker_threads {
            let thread_init = thread_init.clone();
            let thread_fini = thread_fini.clone();

            self.worker_threads.push(
                thread::Builder::new()
                    .stack_size(stack_size)
                    .name(Self::worker_thread_name(worker_index as _))
                    .spawn(move || {
                        task_system.worker_entry_point(worker_index as _, thread_init, thread_fini);
                    })
                    .expect("worker thread creation failed"),
            );
        }
    }

    /// `worker_index` in range `0 .. num_worker_threads`.
    fn worker_thread_name(worker_index: u32) -> String {
        format!("Worker thread {}", worker_index)
    }

    #[cfg(feature = "profiling")]
    pub(crate) fn init_profiler(
        &mut self,
        profiler: Option<Box<TaskSystemProfiler>>,
        main_thread_name: &str,
    ) {
        self.profiler = profiler;

        if let Some(profiler) = self.profiler.as_ref() {
            profiler.init_thread(main_thread_name);
        }
    }

    /// Initializes the "main" thread context, converting the main thread to a fiber if necessary.
    /// NOTE - `self` must be allocated on the heap as the pointer to `main_task_handle` is used by the main task.
    pub(crate) fn init_main_thread(&self, main_thread_name: &str) {
        let mut thread_context = Thread::new(main_thread_name.to_owned(), 0, None);

        // No need to convert to a fiber if we're not using fibers.
        if self.use_fiber_pool {
            let main_fiber = Fiber::from_thread("Main thread fiber".to_owned()).unwrap();
            thread_context.set_fiber(main_fiber);
        }

        // Dummy main thread task, never actually executed.
        let main_task = unsafe {
            Task::new(
                &self.main_task_handle,
                |_: &TaskSystem| {
                    unreachable!();
                },
                true,
                Some("Main task".to_owned()),
                None,
            )
        };

        thread_context.set_task(main_task);

        self.thread.store(thread_context).unwrap();
    }

    /// Signals the spawned worker / FS thread(s) to enter their scheduler loops.
    pub(crate) fn start_threads(&self) {
        self.thread_barrier.wait();
    }

    /// Worker fiber entry point.
    fn fiber_entry_point(&self) {
        // If this fiber was just picked up, we might need to cleanup from the previous fiber.
        self.cleanup_previous_fiber();

        // Run the scheduler loop.
        // >>>>>>>> (POTENTIAL) FIBER SWITCH >>>>>>>>
        self.scheduler_loop();
        // <<<<<<<< (POTENTIAL) FIBER SWITCH <<<<<<<<

        // We're out of the scheduler loop - switch back to the thread's fiber for cleanup.
        self.switch_to_thread_fiber();

        unreachable!("worker fiber must never exit");
    }

    /// Worker threads wait for the task system to start them,
    /// call the user-provided `thread_init` closure,
    /// initialize their thread-local contexts,
    /// optionally convert themselves to fibers
    /// and run the scheduler loop
    /// until task system shutdown.
    ///
    /// `worker_index` in range `0 .. num_worker_threads`.
    fn worker_entry_point(
        &self,
        worker_index: u32,
        thread_init: Option<Arc<dyn ThreadInitFiniCallback>>,
        thread_fini: Option<Arc<dyn ThreadInitFiniCallback>>,
    ) {
        self.wait_for_thread_start();

        if let Some(thread_init) = thread_init {
            thread_init(worker_index + 1);
        }

        self.init_worker_thread_context(worker_index, thread_fini);

        // Run the scheduler loop.

        // If we're using fibers, get and switch to a new worker fiber.
        if self.use_fiber_pool {
            // Must succeed - we allocated enough for all threads.
            let fiber = FiberToSwitchTo::Free(self.fiber_pool.get_fiber(false).unwrap());

            // >>>>>>>> FIBER SWITCH >>>>>>>>
            self.switch_to_fiber(fiber);
        // <<<<<<<< FIBER SWITCH <<<<<<<<

        // If we're not using fibers, run the scheduler loop inline.
        } else {
            self.scheduler_loop();
        }

        // We're back from the scheduler loop - the task system is shutting down.

        self.fini_worker_thread_context();
    }

    /// Blocks the worker / FS thread until the main thread allows it to proceed
    /// to avoid accessing the uninitialized task system.
    pub(crate) fn wait_for_thread_start(&self) {
        self.thread_barrier.wait();
    }

    /// Called at worker thread startup.
    ///
    /// `worker_index` in range `0 .. num_worker_threads`.
    fn init_worker_thread_context(
        &self,
        worker_index: u32,
        thread_fini: Option<Arc<dyn ThreadInitFiniCallback>>,
    ) {
        let worker_thread_name = Self::worker_thread_name(worker_index);

        #[cfg(feature = "profiling")]
        if let Some(profiler) = self.profiler.as_ref() {
            profiler.init_thread(&worker_thread_name);
        }

        let mut thread_context = Thread::new(worker_thread_name, worker_index + 1, thread_fini);

        // Don't convert to a scheduler fiber if we're not using fibers.
        if self.use_fiber_pool {
            let fiber_name = format!("Worker thread {} fiber", worker_index);
            thread_context.set_thread_fiber(Fiber::from_thread(fiber_name).unwrap());
        }

        self.thread.store(thread_context).unwrap();
    }

    /// Called before worker thread exit.
    /// Calls the user-provided thread finalization callback and cleans up the thread context thread local object.
    fn fini_worker_thread_context(&self) {
        let mut thread_context = unsafe { self.thread.take_unchecked() };

        thread_context.execute_thread_fini();

        mem::drop(thread_context);
    }

    /// Returns `true` when the "main" thread signalled it's time to exit the scheduler loop.
    pub(crate) fn scheduler_exit(&self) -> bool {
        self.exit_flag.load(Ordering::Acquire)
    }

    /// Worker fiber / thread scheduler loop.
    fn scheduler_loop(&self) {
        while !self.scheduler_exit() {
            // >>>>>>>> (POTENTIAL) FIBER SWITCH >>>>>>>>
            self.scheduler_loop_iter();
            // <<<<<<<< (POTENTIAL) FIBER SWITCH <<<<<<<<
        }
    }

    /// A single iteration of the worker fiber / thread scheduler loop.
    fn scheduler_loop_iter(&self) {
        let main_thread = self.is_main_thread();

        if let Some(task) = self.get_task(main_thread) {
            match task {
                // Popped a new task - execute it inline, in this worker thread / fiber.
                NewOrResumedTask::New(task) => {
                    debug_assert!(!task.is_none());

                    // Set the thread's current task and run it.
                    // The task may yield if we're using fibers.
                    // >>>>>>>> (POTENTIAL) FIBER SWITCH >>>>>>>>
                    self.execute_task(task);
                    // <<<<<<<< (POTENTIAL) FIBER SWITCH <<<<<<<<
                    // The task is finished.
                    // We might be in a different thread if the task yielded.

                    // Take the current task, decrement the task handle(s),
                    // maybe resume a yielded fiber waiting for this task's handle.
                    self.finish_task(self.take_task());
                }
                NewOrResumedTask::Resumed(task) => {
                    debug_assert!(self.use_fiber_pool);

                    // Yes, there's a resumed fiber - prepare the context and switch to it.
                    // >>>>>>>> FIBER SWITCH >>>>>>>>
                    self.switch_to_resumed_task(task);
                    // <<<<<<<< FIBER SWITCH <<<<<<<<
                    // We're back (maybe in a different thread) because somebody needs a free fiber to run tasks.

                    // Don't forget to free / make ready to be resumed the previous fiber this thread ran.
                    self.cleanup_previous_fiber();
                }
            }
        }
    }

    /// Blocks the thread, waiting for a new or resumed task to become available in the task queue.
    ///
    /// `main_thread` is `true` if this is called from the "main" thread
    /// and we may thus pop "main" thread tasks.
    fn get_task(&self, main_thread: bool) -> Option<NewOrResumedTask> {
        self.task_queue.get_task(main_thread, true)
    }

    /// Switch to the current thread's thread fiber.
    /// Called from worker fibers on task system shutdown.
    fn switch_to_thread_fiber(&self) {
        self.thread_mut().switch_to_thread_fiber();
    }

    /// If the previous fiber run by this thread needs to be freed (i.e. it resumed the current fiber), free it.
    /// If the previous fiber run by this thread was yielded, make it ready to be resumed, or resume it if yield dependency is already complete.
    /// Only ever called if we use fibers.
    fn cleanup_previous_fiber(&self) {
        debug_assert!(self.use_fiber_pool);

        let thread_context = self.thread_mut();

        // Free the previous fiber if necessary.
        if let Some(fiber_to_free) = thread_context.take_fiber_to_free() {
            self.fiber_pool.free_fiber(fiber_to_free);
        }

        // Make ready / resume the previous yielded fiber, if any.
        if let Some(yield_key) = thread_context.take_yield_key() {
            self.try_resume_task(yield_key);
        }
    }

    /// Only ever called if we use fibers.
    #[allow(unused_variables)]
    fn resume_task(&self, task: TaskAndFiber, yield_key: YieldKey) {
        debug_assert!(self.use_fiber_pool);

        #[cfg(feature = "tracing")]
        self.trace_resumed_task(&task, yield_key);

        self.task_queue
            .add_resumed_task(task, self.is_main_thread());
    }

    /// Prepare the thread context - current task / fiber - and switch to the resumed fiber.
    /// Current fiber will be returned to the pool.
    /// Only ever called if we use fibers.
    fn switch_to_resumed_task(&self, task: TaskAndFiber) {
        debug_assert!(self.use_fiber_pool);

        #[cfg(feature = "tracing")]
        self.trace_picked_up_resumed_task(&task);

        let thread_context = self.thread_mut();

        thread_context.free_fiber(); // actually freed in `cleanup_previous_fiber()`

        thread_context.set_fiber(task.fiber);
        thread_context.set_task(task.task);

        // Switch to the resumed task fiber, cleanup the current fiber, resume executing the task.
        // >>>>>>>> FIBER SWITCH >>>>>>>>
        thread_context.switch_to_current_fiber();
        // <<<<<<<< FIBER SWITCH <<<<<<<<

        // We're back (maybe in a different thread) because somebody needs a free fiber to run tasks.
        // Current fiber must be set.
        debug_assert!(self.thread().has_current_fiber(), "current fiber not set");
    }

    /// Set the thread's current task and execute it.
    /// Tracing / profiling of task execution goes here.
    fn execute_task(&self, task: Task) {
        self.before_execute_task(&task);

        // >>>>>>>> (POTENTIAL) FIBER SWITCH >>>>>>>>
        self.thread_mut().execute_task(task, self);
        // <<<<<<<< (POTENTIAL) FIBER SWITCH <<<<<<<<

        // NOTE - we might be in a different thread.

        self.after_execute_task(self.thread().task());
    }

    /// Decrements the task counters.
    /// If the last task for the `Handle` is comlete,
    /// check if the `Handle` was waited on and a yielded task added to the yield queue.
    /// If it was waited on, we'll try to resume the yielded fiber.
    /// Otherwise the wait has not started yet or the yielded fiber is not ready to be resumed yet.
    fn finish_task(&self, task: Task) {
        debug_assert!(task.is_none());

        let finished = task.dec();
        self.global_task_handle.dec();

        if finished && self.use_fiber_pool {
            let yield_key = task.handle.yield_key();

            self.try_resume_task(yield_key);
        }
    }

    /// Make `Ready` the task with the `yield_key`.
    /// If already `Ready`, pop from the yield queue and push to resumed queue.
    /// Only ever called if we use fibers.
    pub(crate) fn try_resume_task(&self, yield_key: YieldKey) {
        debug_assert!(self.use_fiber_pool);

        if let Some(task) = self.task_queue.try_resume_task(yield_key) {
            self.resume_task(task, yield_key);
        }
    }

    /// Trace / profile at the start of a new task.
    fn before_execute_task(&self, task: &Task) {
        debug_assert!(!task.is_none());

        #[cfg(feature = "tracing")]
        self.trace_picked_up_task(task);

        #[cfg(feature = "profiling")]
        self.profile_before_execute_task(task.task_name_or_unnamed());
    }

    /// Trace / profile after the task is finished.
    fn after_execute_task(&self, task: &Task) {
        debug_assert!(task.is_none());

        #[cfg(feature = "profiling")]
        self.profile_after_execute_task(task.task_name_or_unnamed());

        #[cfg(feature = "tracing")]
        self.trace_finished_task(task);
    }

    fn set_task(&self, task: Task) {
        self.thread_mut().set_task(task);
    }

    fn take_task(&self) -> Task {
        self.thread_mut().take_task()
    }

    fn has_current_fiber(&self) -> bool {
        if !self.use_fiber_pool {
            false
        } else {
            self.thread().has_current_fiber()
        }
    }

    /// We're trying to yield the current fiber/task and need a new fiber for the thread to switch to.
    ///
    /// Returns either a previously yielded, now resumed fiber, or a new fiber from the pool.
    /// May return `None` if we ran out of fibers in the fiber pool and we allow inline task execution.
    ///
    /// If `main_thread` is `false`, does not return resumed "main" thread tasks.
    ///
    /// Only ever called if we use fibers.
    fn try_get_fiber_to_switch_to(&self, main_thread: bool) -> Option<FiberToSwitchTo> {
        debug_assert!(self.use_fiber_pool);

        let wait = !self.allow_inline_tasks;

        // Check if any yielded fibers are ready to be resumed.
        self.task_queue
            .try_get_resumed_task(main_thread)
            .map(FiberToSwitchTo::Resumed)
            // Else try to get a free fiber.
            // This is a blocking call if we don't allow inline task execution.
            .or_else(|| self.try_get_free_fiber(wait).map(FiberToSwitchTo::Free))
        // Otherwise no resumed or free fibers, and we allow inline task execution.
    }

    /// Attempt to yield the thread's current fiber and switch to `fiber`,
    /// unless `yield_data` dependency is not already complete.
    ///
    /// When this returns, `yield_data` is guaranteed to be complete.
    ///
    /// `main_thread` is `true` if this is called from the "main" thread,
    /// regardless if the `task` is a "main" thread task or not.
    ///
    /// Only ever called if we're using fibers.
    fn yield_to_fiber<Y: YieldData>(
        &self,
        fiber: FiberToSwitchTo,
        yield_data: &Y,
        main_thread: bool,
    ) {
        debug_assert!(self.use_fiber_pool);

        // If the yield dependency is still incomplete, we push the current fiber/task to the yield queue ...
        if self.try_yield_current_task(yield_data) {
            // Otherwise the dependency is already complete and we're done.

            // Don't forget to return the resumed / free fiber we ended up not needing.
            self.return_fiber_to_switch_to(fiber, main_thread);

            return;
        }
        // ... and switch to the resumed / new fiber.

        // >>>>>>>> FIBER SWITCH >>>>>>>>
        self.switch_to_fiber(fiber);
        // <<<<<<<< FIBER SWITCH <<<<<<<<

        // The dependency is complete and we're resumed, maybe in a different thread.
        debug_assert!(yield_data.is_complete());

        // Don't forget to free / make ready to be resumed the previous fiber this thread ran.
        self.cleanup_previous_fiber();
    }

    /// Try to get a free fiber from the pool.
    ///
    /// If the pool is empty and `wait` is `true`, the method blocks / sleeps the calling thread
    /// until a fiber is freed to the pool and is successfully returned.
    ///
    /// Only ever called if we're using fibers.
    fn try_get_free_fiber(&self, wait: bool) -> Option<Fiber> {
        debug_assert!(self.use_fiber_pool);

        self.fiber_pool.get_fiber(wait)
    }

    fn take_fiber(&self) -> Fiber {
        debug_assert!(self.use_fiber_pool);

        self.thread_mut().take_fiber()
    }

    /// Marks the current fiber as yielded in the thread context.
    /// Only ever called if we use fibers.
    fn set_yield_key(&self, yield_key: YieldKey) {
        debug_assert!(self.use_fiber_pool);

        self.thread_mut().set_yield_key(yield_key)
    }

    /// Returns `true` if the `yield_data` dependency is complete and there's no need to yield the current task.
    /// Otherwise takes the current task / fiber, adds them to the yield queue with the `yield_data`'s `YieldKey` and returns `false`.
    /// Only ever called if we use fibers.
    fn try_yield_current_task<Y: YieldData>(&self, yield_data: &Y) -> bool {
        debug_assert!(self.use_fiber_pool);

        // It's important to lock the yield queue first, before calling `YieldData::is_complete()`.
        let mut yield_queue = self.task_queue.lock_yield_queue();

        if yield_data.is_complete() {
            true
        } else {
            let task = TaskAndFiber {
                task: self.take_task(),
                fiber: self.take_fiber(),
            };

            let yield_key = yield_data.yield_key();

            #[cfg(feature = "tracing")]
            self.trace_yielded_task(&task, yield_key);

            yield_queue.yield_task(task, yield_key); // Push to `yield_queue`, status `NotReady`

            self.set_yield_key(yield_key);

            false
        }
    }

    /// `main_thread` is `true` if this is called from the "main" thread,
    /// regardless if the `task` is a "main" thread task or not.
    ///
    /// Only ever called if we use fibers.
    fn return_fiber_to_switch_to(&self, fiber: FiberToSwitchTo, main_thread: bool) {
        debug_assert!(self.use_fiber_pool);

        match fiber {
            FiberToSwitchTo::Resumed(task) => {
                self.task_queue.add_resumed_task(task, main_thread);
            }
            FiberToSwitchTo::Free(fiber) => {
                self.fiber_pool.free_fiber(fiber);
            }
        }
    }

    /// Switch to the previously yielded, now resumed task / fiber;
    /// or to a free scheduler / worker fiber.
    ///
    /// Only ever called if we use fibers.
    fn switch_to_fiber(&self, fiber: FiberToSwitchTo) {
        debug_assert!(self.use_fiber_pool);

        let thread_context = self.thread_mut();

        match fiber {
            FiberToSwitchTo::Resumed(task) => {
                thread_context.set_task(task.task);
                thread_context.set_fiber(task.fiber);
            }
            FiberToSwitchTo::Free(fiber) => {
                thread_context.set_fiber(fiber);
            }
        }

        // >>>>>>>> FIBER SWITCH >>>>>>>>
        thread_context.switch_to_current_fiber();
        // <<<<<<<< FIBER SWITCH <<<<<<<<
    }

    /// Signals the worker threads to break the scheduler loop, finalize
    /// their per-thread state and exit.
    fn shutdown_worker_threads(&mut self) {
        // Set the exit flag.
        self.exit_flag.store(true, Ordering::Release);

        // Wake up all worker threads and signal them to exit the task wait loops.
        self.task_queue.wake_up_workers();

        // Wait for the worker threads to exit.
        while let Some(thread) = self.worker_threads.pop() {
            thread.join().unwrap();
        }
    }

    pub(crate) fn thread(&self) -> &Thread {
        unsafe { self.thread.as_ref_unchecked() }
    }

    fn thread_mut(&self) -> &mut Thread {
        unsafe { self.thread.as_mut_unchecked() }
    }
}

impl Drop for TaskSystem {
    // NOTE - `self` is on the heap.
    fn drop(&mut self) {
        // Make sure all tasks are finished.
        unsafe {
            self.wait_for_handle_named(&self.global_task_handle, "Global task handle");
        }

        self.shutdown_worker_threads();

        #[cfg(feature = "asyncio")]
        self.shutdown_fs_thread();

        // Free the thread context.
        let mut thread_context = unsafe { self.thread.take_unchecked() };

        // Drop the main thread's fiber, converting it back to a normal thread.
        if self.use_fiber_pool {
            let current_fiber = thread_context.take_fiber();
            debug_assert!(current_fiber.is_thread_fiber());
            debug_assert_eq!(current_fiber.name().unwrap(), "Main thread fiber");
        }

        self.thread.free_index().unwrap();
    }
}

/// Type for a unique identifier of a yield operation.
pub(crate) type YieldKey = NonZeroUsize;

/// A trait which represents a thread / fiber yield operation
/// when waiting for some condition to be fulfilled
/// (e.g., waiting for a task or for async file IO).
pub(crate) trait YieldData {
    /// Check whether the wait condition is fullfilled.
    fn is_complete(&self) -> bool;

    /// Returns the unique identifier of this yield operation,
    /// used by the task system internal bookkeeping to resume the yielded fiber / thread
    /// when [`is_complete`] returns `true`.
    ///
    /// [`is_complete`]: #method.is_complete
    fn yield_key(&self) -> YieldKey;
}

/// A yield operation representing a wait for a task handle.
struct WaitForTaskData<'h>(&'h Handle);

impl<'h> YieldData for WaitForTaskData<'h> {
    /// The address on the stack of the task handle associated with the waited-on task
    /// is a perfect `YieldKey` candidate for a wait on the task handle.
    fn yield_key(&self) -> YieldKey {
        self.0.yield_key()
    }

    /// The wait on the task handle is complete when its counter reaches `0`.
    fn is_complete(&self) -> bool {
        self.0.is_complete()
    }
}

/// A helper macro to create the task [`handle`] and the task [`scope`],
/// using the passed-in [`task system`], or the [`task system singleton`]
/// (which must be [`initialized`] beforehand) if none was passed.
///
/// Use of this macro is prefereble to creating the [`handle`] and the [`scope`]
/// via the [`task system`] methods.
///
/// [`handle`]: struct.Handle.html
/// [`scope`]: struct.Scope.html
/// [`task system`]: struct.TaskSystem.html
/// [`task system singleton`]: fn.task_system.html
/// [`initialized`]: fn.init_task_system.html
#[macro_export]
macro_rules! task_scope {
    ($id: ident, $task_system: expr) => {
        let $id = $task_system.handle();
        let mut $id = $task_system.scope(&$id);
    };
    ($id: ident) => {
        let $id = $crate::task_system().handle();
        let mut $id = $crate::task_system().scope(&$id);
    };
}

/// A helper macro to create the task [`handle`] and the named task [`scope`],
/// using the passed-in [`task system`], or the [`task system singleton`]
/// (which must be [`initialized`] beforehand) if none was passed.
///
/// Use of this macro is prefereble to creating the [`handle`] and the [`scope`]
/// via the [`task system`] methods.
///
/// [`handle`]: struct.Handle.html
/// [`scope`]: struct.Scope.html
/// [`task system`]: struct.TaskSystem.html
/// [`task system singleton`]: fn.task_system.html
/// [`initialized`]: fn.init_task_system.html
#[macro_export]
macro_rules! task_scope_named {
    ($id: ident, $name: expr, $task_system: expr) => {
        let $id = $task_system.handle();
        let mut $id = $task_system.scope_named(&$id, $name);
    };
    ($id: ident, $name: expr) => {
        let $id = $crate::task_system().handle();
        let mut $id = $crate::task_system().scope_named(&$id, $name);
    };
}
