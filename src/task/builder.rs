use {
    crate::{num_cores::get_num_physical_cores, TaskSystem},
    std::{sync::Arc, time::Duration},
};

#[cfg(feature = "profiling")]
use crate::TaskSystemProfiler;

#[cfg(feature = "remotery")]
use crate::RemoteryProfiler;

/// Stack size in bytes of the worker threads (in case they don't run tasks).
const SCHEDULER_STACK_SIZE: usize = 64 * 1024;

/// How much time the scheduler threads (including the "main" thread if it's waiting for a task)
/// will wait for a new task / wakeup event to be signaled.
const TASK_WAIT_TIMEOUT: Duration = Duration::from_secs(999);

/// How much time the scheduler threads (including the "main" thread if it's waiting for a free fiber)
/// will wait for a free fiber event to be signaled.
/// Only used if we don't allow inline task execution.
const FIBER_WAIT_TIMEOUT: Duration = Duration::from_secs(999);

/// The type for a closure to be called in the context of each worker thread spawned by the [`TaskSystem`]
/// once before any tasks are executed by it, or once after the last task is executed by it.
///
/// The closure is passed the worker thread index in range `1 ..= num_worker_threads`.
///
/// NOTE - the closure is not called in the "main" thread.
///
/// [`TaskSystem`]: struct.TaskSystem.html
pub trait ThreadInitFiniCallback: Fn(u32) + Send + Sync + 'static {}

impl<F: Fn(u32) + Send + Sync + 'static> ThreadInitFiniCallback for F {}

/// Builder object used to configure the [`TaskSystem`] parameters.
///
/// Creates the [`TaskSystem`] instance via [`build`].
///
/// Used by [`init_task_system`].
///
/// [`TaskSystem`]: struct.TaskSystem.html
/// [`build`]: #method.build
/// [`init_task_system`]: fn.init_task_system.html
pub struct TaskSystemBuilder {
    num_worker_threads: u32,

    #[cfg(feature = "fibers")]
    num_fibers: u32,
    #[cfg(feature = "fibers")]
    allow_inline_tasks: bool,
    #[cfg(feature = "fibers")]
    fiber_stack_size: usize,

    thread_init: Option<Arc<dyn ThreadInitFiniCallback>>,
    thread_fini: Option<Arc<dyn ThreadInitFiniCallback>>,

    #[cfg(feature = "profiling")]
    profiler: Option<Box<TaskSystemProfiler>>,
}

impl TaskSystemBuilder {
    fn default() -> Self {
        let num_physical_cores = get_num_physical_cores().max(1) as u32;

        // TODO - look at / tweak these params (esp. `num_fibers`, `fiber_stack_size`).
        let num_worker_threads = num_physical_cores - 1;

        #[cfg(feature = "fibers")]
        let num_fibers = num_physical_cores * 4;
        #[cfg(feature = "fibers")]
        let allow_inline_tasks = true;
        #[cfg(feature = "fibers")]
        let fiber_stack_size = 1024 * 1024; // 1 Mb

        Self {
            num_worker_threads,

            #[cfg(feature = "fibers")]
            num_fibers,
            #[cfg(feature = "fibers")]
            allow_inline_tasks,
            #[cfg(feature = "fibers")]
            fiber_stack_size,

            thread_init: None,
            thread_fini: None,

            #[cfg(feature = "profiling")]
            profiler: None,
        }
    }

    /// Creates a new [`TaskSystemBuilder`] with default settings:
    ///
    /// spawn a worker thread per physical core less one,
    /// 1 Mb of stack per worker thread/fiber,
    /// 4 fibers per thread,
    /// allow inline tasks.
    ///
    /// If `"profiling"` and `"remotery"` features are enabled, this initializes
    /// the `miniremotery`-based profiler.
    ///
    /// [`TaskSystemBuilder`]: struct.TaskSystemBuilder.html
    pub fn new() -> Self {
        #[allow(unused_mut)]
        let mut builder = TaskSystemBuilder::default();

        #[cfg(all(feature = "profiling", feature = "remotery"))]
        if let Some(profiler) = RemoteryProfiler::new() {
            builder.profiler = Some(Box::new(profiler));
        }

        builder
    }

    /// Set the number of additional worker threads the [`task system`] will spawn.
    ///
    /// Keep in mind that the "main" thread (the thread which called [`build`] / [`init_task_system`])
    /// also executes tasks (unless the task explicitly opts out).
    ///
    /// If `num_worker_threads` is `0`, no worker threads will be spawned.
    ///
    /// [`task system`]: struct.TaskSystem.html
    /// [`build`]: struct.TaskSystemBuilder.html#method.build
    /// [`init_task_system`]: fn.init_task_system.html
    pub fn num_worker_threads(mut self, num_worker_threads: u32) -> Self {
        self.num_worker_threads = num_worker_threads;
        self
    }

    /// Number of worker fibers the [`task system`] will allocate in the pool.
    ///
    /// If `num_fibers` is `0`, the fiber pool is not allocated and
    /// the [`task system`] will execute all tasks inline, on the "main" and worker threads' stacks
    /// ([`allow_inline_tasks`] is ignored in this case).
    ///
    /// The fiber pool never grows or shrinks at runtime.
    ///
    /// [`task system`]: struct.TaskSystem.html
    /// [`allow_inline_tasks`]: #method.allow_inline_tasks
    #[allow(unused_variables, unused_mut)]
    pub fn num_fibers(mut self, num_fibers: u32) -> Self {
        #[cfg(feature = "fibers")] {
            self.num_fibers = num_fibers;
        }
        self
    }

    /// If `true`, the [`task system`] will execute tasks inline on the "main" and worker thread stacks
    /// if it runs out of resumed / free fibers.
    ///
    /// Always `true` if [`num_fibers`] was set to `0` (do not use fibers at all).
    ///
    /// If `false`, the [`task system`] always executes tasks in dedicated fibers,
    /// even if it means waiting for a free fiber to become available.
    ///
    /// NOTE - setting [`allow_inline_tasks`] to `false` might or might not lead to deadlocks
    /// if combined with a small fiber pool size.
    ///
    /// [`task system`]: struct.TaskSystem.html
    /// [`num_fibers`]: #method.num_fibers
    /// [`allow_inline_tasks`]: #method.allow_inline_tasks
    #[allow(unused_variables, unused_mut)]
    pub fn allow_inline_tasks(mut self, allow_inline_tasks: bool) -> Self {
        #[cfg(feature = "fibers")] {
            self.allow_inline_tasks = allow_inline_tasks;
        }
        self
    }

    /// Worker fiber stack size in bytes.
    ///
    /// If [`num_fibers`] == `0` or [`allow_inline_tasks`] == `true`,
    /// this value is used for worker thread stack size instead.
    ///
    /// [`num_fibers`]: #method.num_fibers
    /// [`allow_inline_tasks`]: #method.allow_inline_tasks
    #[allow(unused_variables, unused_mut)]
    pub fn fiber_stack_size(mut self, fiber_stack_size: usize) -> Self {
        #[cfg(feature = "fibers")] {
            self.fiber_stack_size = fiber_stack_size;
        }
        self
    }

    /// A closure to be called in the context of each worker thread spawned by the [`task system`]
    /// before any tasks are executed by it.
    ///
    /// The closure is passed the worker thread index in range `1 ..=` [`num_worker_threads`].
    ///
    /// NOTE - the closure is not called in the "main" thread.
    ///
    /// [`num_worker_threads`]: #method.num_worker_threads
    /// [`task system`]: struct.TaskSystem.html
    pub fn thread_init<F: ThreadInitFiniCallback>(mut self, thread_init: F) -> Self {
        self.thread_init.replace(Arc::new(thread_init));
        self
    }

    /// A closure to be called in the context of each worker thread spawned by the [`task system`]
    /// before the thread exits on [`task system`] shutdown.
    ///
    /// The closure is passed the worker thread index in range `1 ..=` [`num_worker_threads`].
    ///
    /// NOTE - the closure is not called in the "main" thread.
    ///
    /// [`num_worker_threads`]: #method.num_worker_threads
    /// [`task system`]: struct.TaskSystem.html
    pub fn thread_fini<F: ThreadInitFiniCallback>(mut self, thread_fini: F) -> Self {
        self.thread_fini.replace(Arc::new(thread_fini));
        self
    }

    /// Sets the [`profiler`] to be used by the [`task system`].
    ///
    /// [`profiler`]: type.TaskSystemProfiler.html
    /// [`task system`]: struct.TaskSystem.html
    #[cfg(feature = "profiling")]
    pub fn profiler(mut self, profiler: Box<TaskSystemProfiler>) -> Self {
        self.profiler = Some(profiler);
        self
    }

    /// Allocates and returns a new [`task system`] instance, configured by the builder.
    ///
    /// [`task system`]: struct.TaskSystem.html
    pub fn build(self) -> Box<TaskSystem> {
        let num_worker_threads = self.num_worker_threads;
        let task_wait_timeout = TASK_WAIT_TIMEOUT;
        let fiber_wait_timeout = FIBER_WAIT_TIMEOUT;

        let mut task_system = Box::new(TaskSystem::new(
            num_worker_threads,
            task_wait_timeout,
            fiber_wait_timeout,
            self.get_allow_inline_tasks(),
        ));

        let num_fibers = self.get_num_fibers();
        let stack_size = self.get_fiber_stack_size();

        // Skip fiber pool initialization if we're not using fibers.
        #[cfg(feature = "fibers")] {
            if num_fibers > 0 {
                // Otherwise add a fiber per worker thread on top.
                let num_fibers = num_fibers + num_worker_threads;
                task_system.init_fiber_pool(num_fibers, stack_size);
            }
        }

        // If we use fibers and don't allow inline tasks, worker threads only need a small stack.
        // Otherwise tasks will / may be executed on the worker threads' stacks
        // (and the "main" thread stack - but we have no control over that)
        // and we need to respect the user-provided stack size.
        let need_large_worker_thread_stack = num_fibers == 0 || self.get_allow_inline_tasks();
        let worker_stack_size = if need_large_worker_thread_stack {
            stack_size
        } else {
            SCHEDULER_STACK_SIZE
        };

        task_system.create_worker_threads(
            num_worker_threads,
            worker_stack_size,
            self.thread_init,
            self.thread_fini,
        );

        #[cfg(feature = "asyncio")]
        task_system.create_fs_thread(SCHEDULER_STACK_SIZE);

        let main_thread_name = "Main thread";

        #[cfg(feature = "profiling")]
        task_system.init_profiler(self.profiler, main_thread_name);

        task_system.init_main_thread(main_thread_name);

        task_system.start_threads();

        task_system
    }

    fn get_num_fibers(&self) -> u32 {
        #[cfg(feature = "fibers")]
        {
            self.num_fibers
        }

        #[cfg(not(feature = "fibers"))]
        {
            0
        }
    }

    fn get_fiber_stack_size(&self) -> usize {
        #[cfg(feature = "fibers")]
        {
            self.fiber_stack_size
        }

        #[cfg(not(feature = "fibers"))]
        {
            0
        }
    }

    fn get_allow_inline_tasks(&self) -> bool {
        #[cfg(feature = "fibers")]
        {
            self.allow_inline_tasks
        }

        #[cfg(not(feature = "fibers"))]
        {
            false
        }
    }
}
