use std::mem;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread;
use std::time::Duration;

#[cfg(feature = "asyncio")]
use std::fs;

#[cfg(feature = "asyncio")]
use std::os::windows::io::AsRawHandle;

use crate::num_cores;

#[cfg(feature = "profiling")]
use crate::profiler::{profiler_scope, Profiler, ProfilerScope};

#[cfg(feature = "remotery")]
use crate::profiler::RemoteryProfiler;

use super::fiber_pool::{FiberPool, FiberPoolParams};
use super::reference_holder::ReferenceHolder;
use super::task_context::TaskContext;
use super::task_handle::TaskHandle;
use super::task_queue::TaskQueue;
use super::task_scope::TaskScope;
use super::thread_context::ThreadContext;
use super::yield_queue::FiberAndTask;

#[cfg(feature = "asyncio")]
use super::fs::IOHandle;

use minifiber::Fiber;
use minithreadlocal::ThreadLocal;

#[cfg(feature = "asyncio")]
use miniiocp::{IOCPResult, IOCP};

#[cfg(feature = "profiling")]
pub type TaskSystemProfiler = dyn Profiler + Send + Sync;

#[cfg(feature = "graph")]
use minigraph::{TaskGraph, TaskVertex, VertexID};

pub(super) type ThreadInitFiniCallback = dyn Fn(usize) + Send + Sync + 'static;

/// Used to configure the task system.
/// Creates a task system instance via [`build`].
///
/// Used by [`init_task_system`].
///
/// [`build`]: #method.build
/// [`init_task_system`]: ../task_system_singleton/fn.init_task_system.html
pub struct TaskSystemBuilder {
    num_worker_threads: usize,
    num_fibers: usize,
    allow_inline_tasks: bool,
    fiber_stack_size: usize,

    thread_init: Option<Arc<ThreadInitFiniCallback>>,
    thread_fini: Option<Arc<ThreadInitFiniCallback>>,

    #[cfg(feature = "profiling")]
    profiler: Option<Box<TaskSystemProfiler>>,
}

impl TaskSystemBuilder {
    fn default() -> Self {
        let num_physical_cores = num_cores::get_num_physical_cores().max(1);

        let num_worker_threads = num_physical_cores - 1;
        let num_fibers = num_physical_cores * 4;
        let allow_inline_tasks = true;
        let fiber_stack_size = 1024 * 1024; // 1 Mb

        #[cfg(feature = "profiling")] {
            Self {
                num_worker_threads,
                num_fibers,
                allow_inline_tasks,
                fiber_stack_size,
                thread_init: None,
                thread_fini: None,
                profiler: None,
            }
        }

        #[cfg(not(feature = "profiling"))] {
            Self {
                num_worker_threads,
                num_fibers,
                allow_inline_tasks,
                fiber_stack_size,
                thread_init: None,
                thread_fini: None,
            }
        }
    }

    /// Creates a new [`TaskSystemBuilder`] with default settings:
    ///
    /// spawn a worker thread per physical core less one,
    /// 1 Mb of stack per worker thread/fiber,
    /// 4 fibers per thread,
    /// allow inline tasks.
    ///
    /// [`TaskSystemBuilder`]: struct.TaskSystemBuilder.html
    pub fn new() -> Self {
        let builder = TaskSystemBuilder::default();

        #[cfg(feature = "profiling")] {
            #[cfg(feature = "remotery")] {
                if let Some(profiler) = RemoteryProfiler::new() {
                    let profiler: Box<TaskSystemProfiler> = Box::new(profiler);

                    builder.profiler = Some(profiler);
                }
            }
        }

        builder
    }

    /// Set the number of additional worker threads the task system will spawn.
    ///
    /// Keep in mind that the "main" thread (the thread which called [`TaskSystemBuilder::build()`] / [`init_task_system()`])
    /// also executes tasks.
    ///
    /// `0` is a valid value and means no worker threads will be spawned -
    /// might be useful when debugging.
    ///
    /// [`TaskSystemBuilder::build()`]: struct.TaskSystemBuilder.html#method.build
    /// [`init_task_system()`]: ../task_system_singleton/fn.init_task_system.html
    pub fn num_worker_threads(mut self, num_worker_threads: usize) -> Self {
        self.num_worker_threads = num_worker_threads;
        self
    }

    /// Number of fibers the task system will preallocate in the pool.
    ///
    /// Will be at least `num_worker_threads`.
    /// The fiber pool never grows or shrinks at runtime.
    ///
    /// If `num_fibers` is `0`, the fiber pool is not allocated and
    /// the task system will execute all tasks inline, on the "main" and worker threads' stacks.
    pub fn num_fibers(mut self, num_fibers: usize) -> Self {
        self.num_fibers = num_fibers;
        self
    }

    /// Used only if `num_fibers` is positive.
    /// If `true`, the task system will execute tasks inline on worker thread stack if it runs out of free fibers.
    /// If `false`, the task system alwais executes tasks in dedicated fibers,
    /// even if it means waiting for a fiber to become available.
    pub fn allow_inline_tasks(mut self, allow_inline_tasks: bool) -> Self {
        self.allow_inline_tasks = allow_inline_tasks;
        self
    }

    /// Worker fiber stack size in bytes.
    /// If `num_fibers` == `0` or `allow_inline_tasks` == `true`,
    /// this value is used for worker thread stack size instead.
    pub fn fiber_stack_size(mut self, fiber_stack_size: usize) -> Self {
        self.fiber_stack_size = fiber_stack_size;
        self
    }

    /// A closure to be called in the context of each worker thread spawned by the task system
    /// before any tasks are executed by it.
    ///
    /// The closure is passed the worker thread index in range `1 ..= num_worker_threads`.
    ///
    /// NOTE - not called in the `main` thread.
    pub fn thread_init<F: Fn(usize) + Send + Sync + 'static>(mut self, thread_init: F) -> Self {
        self.thread_init = Some(Arc::new(thread_init));
        self
    }

    /// A closure to be called in the context of each worker thread spawned by the task system
    /// before the thread exits on task system shutdown.
    ///
    /// The closure is passed the worker thread index in range `1 ..= num_worker_threads`.
    ///
    /// NOTE - not called in the `main` thread.
    pub fn thread_fini<F: Fn(usize) + Send + Sync + 'static>(mut self, thread_fini: F) -> Self {
        self.thread_fini = Some(Arc::new(thread_fini));
        self
    }

    #[cfg(feature = "profiling")]
    pub fn profiler(mut self, profiler: Box<TaskSystemProfiler>) -> Self {
        self.profiler = Some(profiler);
        self
    }

    /// Allocates and returns a new task system instance, configured by the builder.
    pub fn build(self) -> Box<TaskSystem> {
        let num_worker_threads = self.num_worker_threads;
        let task_wait_timeout = TASK_WAIT_TIMEOUT;

        let mut task_system = Box::new(TaskSystem::empty(num_worker_threads, task_wait_timeout));

        let num_fibers = self.num_fibers;
        let stack_size = self.fiber_stack_size;

        // Skip fiber pool initialization if we're not using fibers.
        if num_fibers > 0 {
            let num_fibers = num_fibers + num_worker_threads;
            task_system.init_fiber_pool(num_fibers, stack_size);
        }

        // If we use fibers, worker threads only need a small stack.
        // Otherwise tasks will be executed on the worker threads' stacks
        // (and the "main" thread stack - but we have no control over that)
        // and we need to respect the user-provided stack size.
        let need_large_worker_thread_stack = num_fibers == 0;
        let worker_stack_size = if need_large_worker_thread_stack {
            stack_size
        } else {
            SCHEDULER_STACK_SIZE
        };

        task_system.create_worker_threads(num_worker_threads, worker_stack_size, self.thread_init, self.thread_fini);

        #[cfg(feature = "asyncio")]
        task_system.create_fs_thread(SCHEDULER_STACK_SIZE);

        let main_thread_name = "Main thread";

        #[cfg(feature = "profiling")]
        task_system.init_profiler(self.profiler, main_thread_name);

        task_system.init_main_thread(main_thread_name);

        task_system.start_threads();

        task_system
    }
}

/// Task system singleton object that manages the task system internal resources
/// like the thread pool, fiber pool and task queues,
/// and provides the public API to spawn and wait for tasks.
pub struct TaskSystem {
    /// Contains the new tasks added to the task system
    /// and the tasks currently waiting for spawned children tasks / IO completion,
    /// including the "main" task representing the `TaskSystem` owner thread
    /// whenever the owner thread waits for a spawned task / IO completion.
    task_queue: TaskQueue,

    /// Thread-local state used to communicate between the scheduler fibers and worker fibers.
    thread_context: ThreadLocal<ThreadContext>,

    /// Worker thread pool. Initiated on startup. Never grows or shrinks.
    worker_threads: Vec<thread::JoinHandle<()>>,

    /// Worker fiber pool. Initiated on startup. Never grows or shrinks.
    fiber_pool: FiberPool,
    use_fiber_pool: bool,
    allow_inline_tasks: bool,

    /// Used to wait for completion of all tasks added to the task system.
    global_task_handle: TaskHandle,

    /// Signals the worker threads to break the scheduler loop and exit
    /// when the task system finalizes.
    exit_flag: AtomicBool,

    /// Used once at task system startup as the worker thread barrier.
    thread_start_flag: AtomicBool,

    /// Fake "main" task handle. Technically unnecessary, but helps
    /// with the uniformoty of pre/postcondition task system asserts.
    /// TODO - fix this.
    main_task_handle: TaskHandle,

    #[cfg(feature = "profiling")]
    profiler: Option<Box<TaskSystemProfiler>>,

    #[cfg(feature = "asyncio")]
    iocp: IOCP,

    #[cfg(feature = "asyncio")]
    fs_thread: Option<thread::JoinHandle<()>>,
}

/// How much time the scheduler threads (including the main thread if it's waiting for a task)
/// will wait for a new task/wakeup event to be signaled.
const TASK_WAIT_TIMEOUT: Duration = Duration::from_secs(999);

#[cfg(feature = "asyncio")]
const FS_WAIT_TIMEOUT: Duration = Duration::from_secs(999);

/// Stack size in bytes of the worker threads (in case they don't run tasks).
const SCHEDULER_STACK_SIZE: usize = 64 * 1024;

pub type TaskRange = Range<u32>;

enum FiberToSwitchTo {
    Resumed(FiberAndTask),
    Free(Fiber),
}

impl TaskSystem {
    /// Creates a new [`TaskHandle`] for use with a [`TaskScope`].
    ///
    /// [`TaskHandle`]: struct.TaskHandle.html
    /// [`TaskScope`]: struct.TaskScope.html
    pub fn handle(&self) -> TaskHandle {
        TaskHandle::new()
    }

    /// Creates a new [`TaskScope`] object which provides safe task system functionality.
    /// Requires a [`TaskHandle`] created by a previous call to [`handle`].
    ///
    /// [`TaskHandle`]: struct.TaskHandle.html
    /// [`TaskScope`]: struct.TaskScope.html
    /// [`handle`]: #method.handle
    pub fn scope<'scope, 'task_system>(
        &'task_system self,
        task_handle: &'scope TaskHandle,
    ) -> TaskScope<'scope, 'task_system> {
        TaskScope::new(task_handle, &self, None)
    }

    /// Creates a new named [`TaskScope`] object which provides safe task system functionality.
    /// Requires a [`TaskHandle`] created by a previous call to [`handle`].
    /// `scope_name` may be retrieved by [`TaskSystem::scope_name`] within the task closure.
    /// Requires "task_names" feature.
    ///
    /// [`TaskHandle`]: struct.TaskHandle.html
    /// [`TaskScope`]: struct.TaskScope.html
    /// [`handle`]: #method.handle
    /// [`TaskSystem::scope_name`]: task_system/struct.TaskSystem.html#method.scope_name
    pub fn scope_named<'scope, 'task_system>(
        &'task_system self,
        task_handle: &'scope TaskHandle,
        scope_name: &str,
    ) -> TaskScope<'scope, 'task_system> {
        TaskScope::new(task_handle, &self, Some(scope_name))
    }

    /// Returns the name, if any, of the task in whose scope this function is called.
    /// Requires "task_names" feature.
    pub fn task_name(&self) -> Option<&str> {
        self.thread_context.as_ref().current_task_name()
    }

    /// Returns the name, if any, of the task scope in which this function is called.
    /// Requires "task_names" feature.
    pub fn scope_name(&self) -> Option<&str> {
        self.thread_context.as_ref().scope_name()
    }

    /// Returns the index of the task system thread in which this function is called.
    ///
    /// `0` means `main thread`,
    /// value in range `1 ..= num_worker_threads` means one of the worker threads.
    pub fn thread_index(&self) -> usize {
        self.thread_context.as_ref().thread_index()
    }

    #[cfg(feature = "graph")]
    pub fn execute_graph<VID: VertexID + Send + Sync, T: Send + Sync, F>(
        &self,
        graph: &TaskGraph<VID, T>,
        f: F
    )
    where
        F: Fn(&T) + Clone + Send
    {
        graph.reset();

        let task_handle = TaskHandle::new();
        let task_handle = &task_handle;

        let num_roots = graph.num_roots() as u32;

        unsafe {
            self.task_range(
                task_handle,
                0 .. num_roots,
                1,
                move |roots: TaskRange, ts: &TaskSystem| {
                    for (vertex_id, task_vertex) in roots.map(|idx| graph.get_root_unchecked(idx as usize)) {
                        ts.execute_graph_vertex(
                            task_handle,
                            graph,
                            *vertex_id,
                            task_vertex,
                            f.clone()
                        );
                    }
                }
            );

            self.wait_for_handle(task_handle);
        }

        graph.reset();
    }

    #[cfg(feature = "graph")]
    fn execute_graph_vertex<VID: VertexID + Send + Sync, T: Send + Sync, F>(
        &self,
        task_handle: &TaskHandle,
        graph: &TaskGraph<VID, T>,
        vertex_id: VID,
        task_vertex: &TaskVertex<T>,
        mut f: F
    )
    where
        F: FnMut(&T) + Clone + Send
    {
        if !task_vertex.is_ready() {
            return;
        }

        f(task_vertex.vertex());

        let num_dependencies = graph.num_dependencies(vertex_id).unwrap() as u32;

        if num_dependencies > 0 {
            unsafe {
                self.task_range(
                    task_handle,
                    0 .. num_dependencies,
                    1,
                    move |dependencies: TaskRange, ts: &TaskSystem| {
                        for (vertex_id, task_vertex) in dependencies.map(|idx| graph.get_dependency_unchecked(vertex_id, idx as usize)) {
                            ts.execute_graph_vertex(
                                task_handle,
                                graph,
                                *vertex_id,
                                task_vertex,
                                f.clone()
                            );
                        }
                    }
                );
            }
        }
    }

    pub(crate) unsafe fn task<'scope, F>(&self, h: &'scope TaskHandle, f: F)
    where
        F: FnOnce(&TaskSystem) + Send + 'scope,
    {
        self.task_internal(None, None, h, f);
    }

    pub(crate) unsafe fn task_named<'scope, F>(&self, task_name: &str, h: &'scope TaskHandle, f: F)
    where
        F: FnOnce(&TaskSystem) + Send + 'scope,
    {
        self.task_internal(Some(task_name), None, h, f);
    }

    pub(crate) unsafe fn task_scoped<'scope, F>(
        &self,
        scope_name: &str,
        h: &'scope TaskHandle,
        f: F,
    ) where
        F: FnOnce(&TaskSystem) + Send + 'scope,
    {
        self.task_internal(None, Some(scope_name), h, f);
    }

    pub(crate) unsafe fn task_named_scoped<'scope, F>(
        &self,
        task_name: &str,
        scope_name: &str,
        h: &'scope TaskHandle,
        f: F,
    ) where
        F: FnOnce(&TaskSystem) + Send + 'scope,
    {
        self.task_internal(Some(task_name), Some(scope_name), h, f);
    }

    pub(crate) unsafe fn task_range<'scope, F>(
        &self,
        h: &'scope TaskHandle,
        range: TaskRange,
        multiplier: u32,
        f: F,
    ) where
        F: Fn(TaskRange, &TaskSystem) + Send + Clone + 'scope,
    {
        self.task_range_internal(None, None, h, range, multiplier, f);
    }

    pub(crate) unsafe fn task_range_named<'scope, F>(
        &self,
        task_name: &str,
        h: &'scope TaskHandle,
        range: TaskRange,
        multiplier: u32,
        f: F,
    ) where
        F: Fn(TaskRange, &TaskSystem) + Send + Clone + 'scope,
    {
        self.task_range_internal(Some(task_name), None, h, range, multiplier, f);
    }

    pub(crate) unsafe fn task_range_scoped<'scope, F>(
        &self,
        scope_name: &str,
        h: &'scope TaskHandle,
        range: TaskRange,
        multiplier: u32,
        f: F,
    ) where
        F: Fn(TaskRange, &TaskSystem) + Send + Clone + 'scope,
    {
        self.task_range_internal(None, Some(scope_name), h, range, multiplier, f);
    }

    pub(crate) unsafe fn task_range_named_scoped<'scope, F>(
        &self,
        task_name: &str,
        scope_name: &str,
        h: &'scope TaskHandle,
        range: TaskRange,
        multiplier: u32,
        f: F,
    ) where
        F: Fn(TaskRange, &TaskSystem) + Send + Clone + 'scope,
    {
        self.task_range_internal(Some(task_name), Some(scope_name), h, range, multiplier, f);
    }

    pub(crate) unsafe fn wait_for_handle<'scope>(&self, h: &'scope TaskHandle) {
        self.wait_for_handle_internal(h, None);
    }

    pub(crate) unsafe fn wait_for_handle_named<'scope>(
        &self,
        h: &'scope TaskHandle,
        scope_name: &str,
    ) {
        self.wait_for_handle_internal(h, Some(scope_name));
    }

    fn task_internal<'scope, F>(
        &self,
        task_name: Option<&str>,
        scope_name: Option<&str>,
        h: &'scope TaskHandle,
        f: F,
    ) where
        F: FnOnce(&TaskSystem) + Send + 'scope,
    {
        // Increment the task counters.
        self.global_task_handle.inc();
        h.inc();

        let task = unsafe { TaskContext::new(task_name, scope_name, h, f) };

        self.on_task_added(&task);

        self.task_queue.add_task(task);
    }

    fn on_task_added(&self, task: &TaskContext) {
        debug_assert!(task.closure.is_once());
        debug_assert!(!task.task_handle.is_empty());

        #[cfg(feature = "tracing")]
        {
            self.trace_add_task(&task);
        }
    }

    #[cfg(feature = "tracing")]
    fn trace_add_task(&self, task: &TaskContext) {
        let thread_context = self.thread_context.as_ref();

        let thread_name = thread_context.thread_name_or_unnamed();
        let fiber_name = thread_context.fiber_name_or_unnamed();

        let h = unsafe { task.task_handle.as_ref() };

        let s = format!(
            "[Added] Task: `{}`, scope: `{}` [{:p} <{}>], by thread: `{}`, fiber: `{}`",
            task.task_name_or_unnamed(),
            task.scope_name_or_unnamed(),
            h,
            h.load(),
            thread_name,
            fiber_name
        );
        trace!("{}", s);
    }

    fn task_range_internal<'scope, F>(
        &self,
        task_name: Option<&str>,
        scope_name: Option<&str>,
        h: &'scope TaskHandle,
        range: TaskRange,
        multiplier: u32,
        f: F,
    ) where
        F: Fn(TaskRange, &TaskSystem) + Send + Clone + 'scope,
    {
        assert!(range.end > range.start, "Expected a non-empty task range.");

        let range_size = range.end - range.start;

        // Including the main thread.
        let num_threads = (self.worker_threads.len() + 1) as u32;

        let num_chunks = (num_threads * multiplier).min(range_size);

        #[cfg(feature = "tracing")]
        self.trace_fork_task_range(&task_name, &scope_name, &range, num_chunks, h);

        self.fork_task_range(task_name, scope_name, h, range, num_chunks, f, false);
    }

    #[cfg(feature = "tracing")]
    fn trace_fork_task_range(
        &self,
        task_name: &Option<&str>,
        scope_name: &Option<&str>,
        range: &TaskRange,
        num_chunks: u32,
        h: &TaskHandle,
    ) {
        let s = format!(
            "[Fork] Task: `{}`, range: `[{}..{})`, chunks: `{}`, scope: `{}` [{:p} <{}>]",
            task_name.unwrap_or("<unnamed>"),
            range.start,
            range.end,
            num_chunks,
            scope_name.unwrap_or("<unnamed>"),
            h,
            h.load()
        );
        trace!("{}", s);
    }

    fn fork_task_range<'scope, F>(
        &self,
        task_name: Option<&str>,
        scope_name: Option<&str>,
        h: &'scope TaskHandle,
        range: TaskRange,
        num_chunks: u32,
        f: F,
        inline: bool,
    ) where
        F: FnMut(TaskRange, &TaskSystem) + Send + Clone + 'scope,
    {
        debug_assert!(range.end > range.start, "Expected a non-empty task range.");

        let range_size = range.end - range.start;
        debug_assert!(num_chunks <= range_size);

        // Trivial cases where the range is `1` (maybe don't use task ranges in this case)
        // or `num_chunks` is `1` (single-threaded mode).
        if range_size == 1 || num_chunks == 1 {
            self.execute_task_range(task_name, scope_name, h, range, f, inline);

        // Else subdivide the range in half.
        } else {
            let ((range_l, num_chunks_l), (range_r, num_chunks_r)) =
                TaskSystem::split_range(range.clone(), num_chunks);

            // If left subrange cannot be further divided, delegate its execution to a different thread in a task.
            if num_chunks_l == 1 {
                self.execute_task_range(task_name, scope_name, h, range_l, f.clone(), false);

            // Else delegate its further subdivision to a different thread.
            } else {
                let fork_task_name = format!(
                    "Fork `{}` [{}..{})",
                    task_name.unwrap_or("<unnamed>"),
                    range_l.start,
                    range_l.end
                );

                let fork_task_name = if cfg!(feature = "task_names") {
                    Some(fork_task_name.as_str())
                } else {
                    None
                };

                let task_name = String::from(task_name.unwrap_or("<unnamed>"));
                let f = f.clone();

                self.task_internal(fork_task_name, scope_name, h, move |task_system| {
                    let task_name = if cfg!(feature = "task_names") {
                        Some(task_name.as_str())
                    } else {
                        None
                    };

                    task_system.fork_task_range(
                        task_name,
                        task_system.scope_name(),
                        task_system.current_task_handle(),
                        range_l,
                        num_chunks_l,
                        f,
                        true,
                    );
                });
            }

            // If right subrange cannot be further divided, execute it inline, if possible.
            if num_chunks_r == 1 {
                self.execute_task_range(task_name, scope_name, h, range_r, f, inline);

            // Else subdivide it further inline, if possible.
            } else {
                self.fork_task_range(task_name, scope_name, h, range_r, num_chunks_r, f, inline);
            }
        }
    }

    fn current_task_handle(&self) -> &TaskHandle {
        self.thread_context.as_ref().current_task_handle()
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

    fn execute_task_range<'scope, F>(
        &self,
        task_name: Option<&str>,
        scope_name: Option<&str>,
        h: &'scope TaskHandle,
        range: TaskRange,
        f: F,
        inline: bool,
    ) where
        F: FnMut(TaskRange, &TaskSystem) + Send + Clone + 'scope,
    {
        let mut f = f;

        let task_name = format!(
            "{} [{}..{})",
            task_name.unwrap_or("<unnamed>"),
            range.start,
            range.end
        );

        // Execute the range inline.
        if inline {
            #[cfg(feature = "profiling")]
            let _scope = self.profiler_scope(&task_name);

            f(range, self);

        // Spawn a task to execute the range.
        } else {
            let task_name = if cfg!(feature = "task_names") {
                Some(task_name.as_str())
            } else {
                None
            };

            self.task_internal(task_name, scope_name, h, move |task_system| {
                #[cfg(feature = "profiling")]
                let _scope = self.profiler_scope(task_system.task_name().unwrap_or("<unnamed>"));

                f(range, task_system);
            });
        }
    }

    unsafe fn wait_for_handle_internal<'any>(&self, h: &'any TaskHandle, scope_name: Option<&str>) {
        // Trivial case - waited on handle already complete.
        if h.is_complete() {
            return;
        }

        self.wait_for_handle_start(h, scope_name);

        let yield_data = WaitForTaskData { handle: h };

        // Yield the fiber waiting for the task(s) to complete ...
        self.yield_current_task(yield_data);
        // ... and we're back.

        self.wait_for_handle_end(h, scope_name);
    }

    fn wait_for_handle_start(&self, _h: &TaskHandle, _scope_name: Option<&str>) {
        let thread_context = self.thread_context.as_ref();

        let _task_name = thread_context.current_task_name().unwrap_or("<unnamed>");

        #[cfg(feature = "tracing")]
        {
            let thread_name = thread_context.thread_name_or_unnamed();
            let fiber_name = thread_context.fiber_name_or_unnamed();

            let s = format!(
                "[Wait start] For scope: `{}` [{:p} : <{}>] (cur. task: `{}`, thread: `{}`, fiber: `{}`)"
                ,_scope_name.unwrap_or("<unnamed>")
                ,_h
                ,_h.load()
                ,_task_name
                ,thread_name
                ,fiber_name
            );
            trace!("{}", s);
        }

        #[cfg(feature = "profiling")]
        {
            if !thread_context.current_task().is_main_task {
                //println!("<<< end_cpu_sample `{}` (pre-wait)", _task_name);
                self.profiler_end_scope();
            }
        }
    }

    #[cfg(any(feature = "tracing", feature = "profiling"))]
    pub(super) fn current_task_name_or_unnamed(&self) -> &str {
        self.thread_context.as_ref().current_task_name_or_unnamed()
    }

    #[cfg(feature = "tracing")]
    pub(super) fn thread_name_or_unnamed(&self) -> &str {
        self.thread_context.as_ref().thread_name_or_unnamed()
    }

    #[cfg(feature = "tracing")]
    pub(super) fn fiber_name_or_unnamed(&self) -> &str {
        self.thread_context.as_ref().fiber_name_or_unnamed()
    }

    #[cfg(feature = "profiling")]
    pub(super) fn is_main_task(&self) -> bool {
        self.thread_context.as_ref().current_task().is_main_task
    }

    fn wait_for_handle_end(&self, _h: &TaskHandle, _scope_name: Option<&str>) {
        #[cfg(any(feature = "tracing", feature = "profiling"))]
        let _task_name = self.current_task_name_or_unnamed();

        #[cfg(feature = "tracing")]
        {
            let s = format!(
                "[Wait end] For scope: `{}` [{:p}] (cur. task: `{}`, thread: `{}`, fiber: `{}`)",
                _scope_name.unwrap_or("<unnamed>"),
                _h,
                _task_name,
                self.thread_name_or_unnamed(),
                self.fiber_name_or_unnamed()
            );
            trace!("{}", s);
        }

        #[cfg(feature = "profiling")]
        {
            if !self.is_main_task() {
                println!(">>> begin_cpu_sample `{}` (post-wait)", _task_name);
                self.profiler_begin_scope(_task_name);
            }
        }
    }

    fn empty(num_worker_threads: usize, task_wait_timeout: Duration) -> Self {
        Self {
            task_queue: TaskQueue::new(task_wait_timeout),

            thread_context: ThreadLocal::new(),

            worker_threads: Vec::with_capacity(num_worker_threads),

            fiber_pool: FiberPool::empty(),
            use_fiber_pool: false,
            allow_inline_tasks: true,

            global_task_handle: TaskHandle::new(),

            exit_flag: AtomicBool::new(false),
            thread_start_flag: AtomicBool::new(false),

            main_task_handle: TaskHandle::new(),

            #[cfg(feature = "profiling")]
            profiler: None,

            #[cfg(feature = "asyncio")]
            iocp: IOCP::new(1),

            #[cfg(feature = "asyncio")]
            fs_thread: None,
        }
    }

    /// Initializes the fiber pool with `num_fibers` worker fibers with `stack_size` bytes of stack size.
    /// NOTE - `self` must be allocated on the heap
    fn init_fiber_pool(&mut self, num_fibers: usize, stack_size: usize) {
        assert!(num_fibers > 0);

        let task_system = unsafe { ReferenceHolder::from_ref(&*self) };

        let fiber_entry_point = move || {
            let task_system = unsafe { task_system.as_ref() };

            // If this fiber was just picked up, we might need to cleanup from the previous fiber.
            task_system.cleanup_previous_fiber();

            task_system.scheduler_loop();

            // We're out of the scheduler loop - switch back to the thread's fiber for cleanup.
            task_system.switch_to_thread_fiber();

            unreachable!("Worker fiber must never exit.");
        };

        let fiber_pool_params = FiberPoolParams {
            num_fibers,
            stack_size,
            fiber_entry_point,
        };

        self.fiber_pool.initialize(fiber_pool_params);

        self.use_fiber_pool = true;
    }

    /// Run once at startup.
    /// Creates `num_worker_threads` thread pool worker threads.
    /// NOTE - `self` must be allocated on the heap
    fn create_worker_threads(
        &mut self,
        num_worker_threads: usize,
        stack_size: usize,
        thread_init: Option<Arc<ThreadInitFiniCallback>>,
        thread_fini: Option<Arc<ThreadInitFiniCallback>>,
    ) {
        let task_system = unsafe { ReferenceHolder::from_ref(&*self) };

        for i in 0 .. num_worker_threads {
            let task_system = task_system.clone();

            let thread_init = thread_init.clone();
            let thread_fini = thread_fini.clone();

            self.worker_threads.push(
                thread::Builder::new()
                    .stack_size(stack_size)
                    .name(format!("Worker thread {}", i))
                    .spawn(move || {
                        let task_system = unsafe { task_system.as_ref() };
                        task_system.worker_entry_point(i, thread_init, thread_fini);
                    })
                    .expect("Worker thread creation failed."),
            );
        }
    }

    #[cfg(feature = "asyncio")]
    fn create_fs_thread(&mut self, stack_size: usize) {
        let task_system = unsafe { ReferenceHolder::from_ref(&*self) };

        self.fs_thread = Some(
            thread::Builder::new()
                .stack_size(stack_size)
                .name("FS thread".to_owned())
                .spawn(move || {
                    let task_system = unsafe { task_system.as_ref() };
                    task_system.fs_thread_entry_point();
                })
                .expect("FS thread creation failed."),
        );
    }

    #[cfg(feature = "profiling")]
    fn init_profiler(
        &mut self,
        profiler: Option<Box<dyn Profiler + Send + Sync>>,
        main_thread_name: &str,
    ) {
        self.profiler = profiler;

        if let Some(profiler) = self.profiler.as_ref() {
            profiler.init_thread(main_thread_name);
        }
    }

    /// Initializes the "main" thread context, converting the main thread to a fiber if necessary.
    /// NOTE - `self` must be allocated on the heap
    fn init_main_thread(&self, main_thread_name: &str) {
        let mut thread_context = ThreadContext::new(Some(main_thread_name), 0, None);

        // No need to convert to a fiber if we're not using fibers.
        if self.use_fiber_pool {
            let main_fiber = Fiber::from_thread(Some("Main thread fiber"));
            thread_context.set_current_fiber(main_fiber);
        }

        let mut main_task = unsafe {
            TaskContext::new(Some("Main task"), None, &self.main_task_handle, |_| {
                unreachable!();
            })
        };
        main_task.is_main_task = true;

        thread_context.set_current_task(main_task);

        self.thread_context.store(thread_context);
    }

    /// Signals the spawned threads to enter their scheduler loops.
    fn start_threads(&self) {
        self.thread_start_flag.store(true, Ordering::SeqCst);

        for thread in self.worker_threads.iter() {
            thread.thread().unpark();
        }

        #[cfg(feature = "asyncio")]
        {
            self.fs_thread.as_ref().unwrap().thread().unpark();
        }
    }

    // Worker threads wait for the task system to start them,
    // initialize their thread-local contexts,
    // optionally convert themselves to fibers
    // and run the scheduler loop.
    fn worker_entry_point(
        &self,
        index: usize,
        thread_init: Option<Arc<ThreadInitFiniCallback>>,
        thread_fini: Option<Arc<ThreadInitFiniCallback>>,
    ) {
        self.wait_for_thread_start();

        if let Some(thread_init) = thread_init {
            thread_init(index + 1);
        }

        self.init_worker_thread_context(index, thread_fini);

        // If we're using fibers, get and switch to a new worker fiber.
        if self.use_fiber_pool {
            // Must succeed - we allocated enough for all threads.
            self.switch_to_fiber(FiberToSwitchTo::Free(
                self.fiber_pool.get_fiber(false).unwrap(),
            ));
        // We're back - the task system is shutting down.

        // If we're not using fibers, run the scheduler loop inline.
        } else {
            self.scheduler_loop();
        }

        self.fini_worker_thread_context();
    }

    /// Parks the worker/FS thread until the "main" thread resumes it.
    fn wait_for_thread_start(&self) {
        while !self.thread_start_flag.load(Ordering::SeqCst) {
            thread::park();
        }
    }

    /// Called at worker thread startup.
    fn init_worker_thread_context(&self, index: usize, thread_fini: Option<Arc<ThreadInitFiniCallback>>) {
        let thread_name = format!("Worker thread {}", index);

        let mut thread_context = ThreadContext::new(Some(&thread_name), index + 1, thread_fini);

        // Don't convert to a scheduler fiber if we're not using fibers.
        if self.use_fiber_pool {
            let fiber_name = format!("Worker thread {} fiber", index);
            thread_context.set_thread_fiber(Fiber::from_thread(Some(&fiber_name)));
        } else {
            panic!("Oops.");
        }

        self.thread_context.store(thread_context);

        #[cfg(feature = "profiling")]
        {
            if let Some(profiler) = self.profiler.as_ref() {
                profiler.init_thread(&thread_name);
            }
        }
    }

    /// Called before thread exit.
    fn fini_worker_thread_context(&self) {
        let mut thread_context = self.thread_context.take();

        thread_context.execute_thread_fini();

        mem::drop(thread_context);
    }

    /// Returns `true` when the "main" thread signalled it's time to exit the scheduler loop.
    fn scheduler_exit(&self) -> bool {
        self.exit_flag.load(Ordering::Relaxed)
    }

    fn scheduler_loop(&self) {
        while !self.scheduler_exit() {
            self.scheduler_loop_iter();
        }
    }

    fn scheduler_loop_iter(&self) {
        let is_main_thread = self.is_main_thread();

        // Check if any yielded fibers are ready to be resumed.
        // Always `None` if we're not using fibers.
        if let Some(fiber_and_task) = self
            .task_queue
            .try_get_resumed_fiber_and_task(is_main_thread)
        {
            // Yes, there's a resumed fiber - prepare the context and switch to it.
            self.switch_to_resumed_fiber_and_task(fiber_and_task);
            // We're back (maybe in a different thread) because somebody needs a free fiber to run tasks.

            // Don't forget to free / make ready to be resume the previous fiber this thread ran.
            self.cleanup_previous_fiber();

        // Else pop a task from the queue and execute it.
        // The task may yield.
        } else if let Some(task) = self.task_queue.try_get_task(is_main_thread) {
            // Set the thread's current task and run it.
            self.execute_task(task);
            // Task is finished.
            // We might be in a different thread if the task yielded.

            // Take the current task, decrement the task handle(s),
            // maybe resume a yielded fiber waiting for this task's handle.
            self.finish_task(self.take_current_task());

        // No tasks - wait for an event to be signaled.
        // Waits for at most `TASK_WAIT_TIMEOUT`.
        } else {
            self.task_queue.wait(self.is_main_thread());
        }
    }

    // Switch to this thread's fiber.
    // Called from worker fibers on task system shutdown.
    fn switch_to_thread_fiber(&self) {
        self.thread_context.as_mut().switch_to_thread_fiber();
    }

    fn is_main_thread(&self) -> bool {
        self.thread_context.as_ref().is_main_thread()
    }

    // If the previous fiber run by this thread needs to be freed (i.e. it resumed the current fiber), free it.
    // If the previous fiber run by this thread was yielded, make it ready to be resumed, or resume it if yield is already complete.
    fn cleanup_previous_fiber(&self) {
        if !self.use_fiber_pool {
            return;
        }

        let thread_context = self.thread_context.as_mut();

        // Free the previous fiber if necessary.
        if let Some(fiber_to_free) = thread_context.take_fiber_to_free() {
            self.fiber_pool.free_fiber(fiber_to_free);
        }

        // Make ready / resume the previous yielded fiber, if any.
        if let Some(yield_key) = thread_context.take_yield_key() {
            self.try_resume_task(yield_key);
        }
    }

    fn resume_fiber_and_task(&self, fiber_and_task: FiberAndTask, _yield_key: NonZeroUsize) {
        #[cfg(feature = "tracing")]
        self.trace_resumed_fiber_and_task(&fiber_and_task, _yield_key);

        self.task_queue.push_resumed_fiber_and_task(fiber_and_task);
    }

    #[cfg(feature = "tracing")]
    fn trace_resumed_fiber_and_task(&self, fiber_and_task: &FiberAndTask, yield_key: NonZeroUsize) {
        let h = unsafe { fiber_and_task.task.task_handle.as_ref() };

        let yield_key: *const () = unsafe { mem::transmute(yield_key) };

        let s = format!(
            "[Resumed] Task: `{}`, scope: `{}` [{:p}] <{}>, fiber `{}`, waited for [{:p}], by thread: `{}`, fiber: `{}`"
            ,fiber_and_task.task.task_name_or_unnamed()
            ,fiber_and_task.task.scope_name_or_unnamed()
            ,h
            ,h.load()
            ,fiber_and_task.fiber.name().unwrap_or("<unnamed>")
            ,yield_key
            ,self.thread_context.as_ref().thread_name_or_unnamed()
            ,self.thread_context.as_ref().fiber_name_or_unnamed()
        );
        trace!("{}", s);
    }

    // Prepare the thread context - current fiber/task - and switch to the fiber.
    // Current fiber will be returned to the pool.
    // Only ever called if we use fibers.
    fn switch_to_resumed_fiber_and_task(&self, fiber_and_task: FiberAndTask) {
        debug_assert!(self.use_fiber_pool);

        let thread_context = self.thread_context.as_mut();

        thread_context.set_current_fiber_to_free(); // freed in `cleanup_previous_fiber()`

        thread_context.set_current_fiber(fiber_and_task.fiber);
        thread_context.set_current_task(fiber_and_task.task);

        thread_context.switch_to_current_fiber();

        // We're back.
        // Current fiber must be set.
        debug_assert!(
            self.thread_context.as_ref().has_current_fiber(),
            "Current fiber not set."
        );
    }

    // Set the thread's current task and execute it.
    // Tracing / profiling of task executin goes here.
    fn execute_task(&self, task: TaskContext) {
        self.before_execute_task(&task);

        self.thread_context.as_mut().execute_task(task, self);

        self.after_execute_task(self.thread_context.as_ref().current_task());
    }

    fn before_execute_task(&self, _task: &TaskContext) {
        #[cfg(feature = "tracing")]
        self.trace_picked_up_task(_task);

        #[cfg(feature = "profiling")]
        {
            debug_assert!(!_task.is_main_task);

            // println!(
            //     ">>> begin_cpu_sample `{}` (started)",
            //     _task.task_name_or_unnamed()
            // );

            self.profiler_begin_scope(_task.task_name_or_unnamed());
        }
    }

    fn after_execute_task(&self, _task: &TaskContext) {
        #[cfg(feature = "profiling")]
        {
            debug_assert!(!_task.is_main_task);

            // println!(
            //     "<<< end_cpu_sample `{}` (finished)",
            //     _task.task_name_or_unnamed()
            // );

            self.profiler_end_scope();
        }
    }

    #[cfg(feature = "tracing")]
    fn trace_picked_up_task(&self, task: &TaskContext) {
        let thread_context = self.thread_context.as_ref();

        let fiber_name = thread_context.fiber_name_or_unnamed();

        let h = unsafe { task.task_handle.as_ref() };

        let s = format!(
            "[Picked up] Task: `{}` [{:p} <{}>], by thread: `{}`, fiber: `{}`",
            task.task_name().unwrap_or("<unnamed>"),
            h,
            h.load(),
            thread_context.thread_name().unwrap_or("<unnamed>"),
            fiber_name
        );
        trace!("{}", s);
    }

    fn set_current_task(&self, task: TaskContext) {
        self.thread_context.as_mut().set_current_task(task);
    }

    fn take_current_task(&self) -> TaskContext {
        self.thread_context.as_mut().take_current_task()
    }

    // Decrements the task counters.
    // If the last task for the `TaskHandle` is comlete,
    // check if the handle was waited on and a yielded task added to the yield queue.
    // If it was waited on, we'll try to resume the yielded fiber.
    // Otherwise the wait has not started yet or the yielded fiber is not ready to be resumed yet.
    fn finish_task(&self, task: TaskContext) {
        let finished = task.dec();
        self.global_task_handle.dec();

        #[cfg(feature = "tracing")]
        self.trace_finished_task(&task);

        if finished && self.use_fiber_pool {
            let yield_key = unsafe { NonZeroUsize::new(task.task_handle.as_address()).unwrap() };

            self.try_resume_task(yield_key);
        }
    }

    // Make `Ready`. If already `Ready`, pop from the yield_queue and push to resumed_queue.
    fn try_resume_task(&self, yield_key: NonZeroUsize) {
        if let Some(fiber_and_task) = self.task_queue.try_resume_fiber_and_task(yield_key) {
            self.resume_fiber_and_task(fiber_and_task, yield_key);
        }
    }

    #[cfg(feature = "tracing")]
    fn trace_finished_task(&self, task: &TaskContext) {
        let thread_context = self.thread_context.as_ref();

        let h = unsafe { task.task_handle.as_ref() };

        let s = format!(
            "[Finished] Task: `{}`, scope: `{}` [{:p} <{}>], by thread: `{}`, fiber: `{}`",
            task.task_name_or_unnamed(),
            task.scope_name_or_unnamed(),
            h,
            h.load(),
            thread_context.thread_name_or_unnamed(),
            thread_context.fiber_name_or_unnamed()
        );
        trace!("{}", s);
    }

    fn has_current_fiber(&self) -> bool {
        self.thread_context.as_ref().has_current_fiber()
    }

    // We're trying to yield the current fiber/task and need a new fiber for the thread to switch to.
    // Either get a resumed yielded fiber, or a new fiber from the pool.
    // May return `None` if we ran out of fibers in the fiber pool and we allow inline task execution.
    // Only called if we use fibers.
    fn try_get_fiber_to_switch_to(&self, is_main_thread: bool) -> Option<FiberToSwitchTo> {
        debug_assert!(self.use_fiber_pool);

        // Check if any yielded fibers are ready to be resumed.
        // Always `None` if we're not using fibers.
        if let Some(fiber_and_task) = self
            .task_queue
            .try_get_resumed_fiber_and_task(is_main_thread)
        {
            Some(FiberToSwitchTo::Resumed(fiber_and_task))

        // Else try to get a free fiber.
        } else if let Some(fiber) = self.try_get_free_fiber() {
            Some(FiberToSwitchTo::Free(fiber))
        } else {
            debug_assert!(self.allow_inline_tasks);
            None
        }
    }

    fn try_get_free_fiber(&self) -> Option<Fiber> {
        let wait = !self.allow_inline_tasks;
        self.fiber_pool.get_fiber(wait)
    }

    fn take_current_fiber(&self) -> Fiber {
        self.thread_context.as_mut().take_current_fiber()
    }

    fn set_yield_key(&self, yield_key: NonZeroUsize) {
        self.thread_context.as_mut().set_yield_key(yield_key)
    }

    pub(super) fn yield_current_task<Y>(&self, yield_data: Y)
    where
        Y: YieldData,
    {
        // We use fibers and yield from a main thread fiber / worker thread scheduler fiber / worker fiber.
        if self.has_current_fiber() {
            let is_main_thread = self.is_main_thread();

            loop {
                // We have a resumed/free fiber available.
                if let Some(fiber) = self.try_get_fiber_to_switch_to(is_main_thread) {
                    // If the yield dependency is still incomplete, we push the current fiber/task to the yield queue.
                    if !self.try_yield_current_fiber_and_task(yield_data) {
                        // Otherwise we're done.

                        // Don't forget to return the resumed/free fiber we ended up not needing.
                        self.return_fiber_to_switch_to(fiber);

                        return;
                    }

                    self.switch_to_fiber(fiber);
                    // We're resumed, maybe in a different thread.

                    // Don't forget to free / make ready to be resumed the previous fiber this thread ran.
                    self.cleanup_previous_fiber();

                    break;

                // We have no resumed fibers / ran out of free fibers, and allow inline execution.
                } else {
                    debug_assert!(self.allow_inline_tasks);

                    // Pop a task from the queue and execute it inline.
                    // The task may yield.
                    if let Some(task) = self.task_queue.try_get_task(self.is_main_thread()) {
                        // Save the current task.
                        let current_task = self.take_current_task();

                        // Set the thread's current task and run it.
                        self.execute_task(task);
                        // Task is finished.
                        // We might be in a different thread.

                        // Take the current task, decrement the task handle(s),
                        // maybe resume a yielded fiber waiting for this task's handle.
                        self.finish_task(self.take_current_task());

                        // Restore the current task.
                        self.set_current_task(current_task);

                        // We finished the right task / were resumed via other means.
                        if yield_data.is_complete() {
                            break;
                        }
                    }

                    // TODO
                    thread::yield_now();
                }
            } // loop

        // We're not using fibers at all.
        } else {
            debug_assert!(!self.use_fiber_pool);

            // Save the current task.
            let current_task = self.take_current_task();

            // Loop executing tasks inline until the yield complete.
            // The tasks may yield.
            loop {
                if let Some(task) = self.task_queue.try_get_task(self.is_main_thread()) {
                    // Set the thread's current task and run it.
                    self.execute_task(task);
                    // Task is finished.

                    // Take the current task, decrement the task handle(s).
                    self.finish_task(self.take_current_task());

                    // We finished the right task / were resumed via other means.
                    if yield_data.is_complete() {
                        break;
                    }
                }

                // TODO
                thread::yield_now();
            } // loop

            // Restore the current task.
            self.set_current_task(current_task);
        }
    }

    fn try_yield_current_fiber_and_task<Y>(&self, yield_data: Y) -> bool
    where
        Y: YieldData,
    {
        let mut yield_queue = self.task_queue.lock_yield_queue();

        if yield_data.is_complete() {
            false
        } else {
            let yield_key = yield_data.yield_key();

            yield_queue.yield_fiber_and_task(
                FiberAndTask {
                    fiber: self.take_current_fiber(),
                    task: self.take_current_task(),
                },
                yield_key,
            ); // Push to `yield_queue`, status `NotReady`

            self.set_yield_key(yield_key); // Mark current fiber as yielded in the TLS.

            true
        }
    }

    fn return_fiber_to_switch_to(&self, fiber: FiberToSwitchTo) {
        match fiber {
            FiberToSwitchTo::Resumed(fiber_and_task) => {
                self.task_queue
                    .return_resumed_fiber_and_task(fiber_and_task);
            }
            FiberToSwitchTo::Free(fiber) => {
                self.fiber_pool.free_fiber(fiber);
            }
        }
    }

    fn switch_to_fiber(&self, fiber: FiberToSwitchTo) {
        let thread_context = self.thread_context.as_mut();

        match fiber {
            FiberToSwitchTo::Resumed(fiber_and_task) => {
                thread_context.set_current_task(fiber_and_task.task);
                thread_context.set_current_fiber(fiber_and_task.fiber);
            }
            FiberToSwitchTo::Free(fiber) => {
                thread_context.set_current_fiber(fiber);
            }
        }

        thread_context.switch_to_current_fiber();
    }

    /// Signals the worker threads to break the scheduler loop, finalize
    /// their per-thread state and exit.
    fn shutdown_worker_threads(&mut self) {
        // Set the exit flag.
        self.exit_flag.store(true, Ordering::Relaxed);

        // Wake up all workers.
        for _ in 0..self.worker_threads.len() {
            self.task_queue.wake_up_worker();
        }

        // Wait for the worker threads to exit.
        while let Some(thread) = self.worker_threads.pop() {
            thread.join().unwrap();
        }
    }

    #[cfg(feature = "profiling")]
    fn profiler_scope(&self, scope_name: &str) -> Option<ProfilerScope> {
        if let Some(profiler) = self.profiler.as_ref() {
            Some(profiler_scope(&**profiler, scope_name))
        } else {
            None
        }
    }

    #[cfg(feature = "profiling")]
    pub(super) fn profiler_begin_scope(&self, scope_name: &str) {
        if let Some(profiler) = self.profiler.as_ref() {
            profiler.begin_scope(scope_name);
        }
    }

    #[cfg(feature = "profiling")]
    pub(super) fn profiler_end_scope(&self) {
        if let Some(profiler) = self.profiler.as_ref() {
            profiler.end_scope();
        }
    }

    #[cfg(feature = "asyncio")]
    pub(super) fn associate_file(&self, file: &fs::File) {
        self.iocp.associate_handle(file.as_raw_handle());
    }

    #[cfg(feature = "asyncio")]
    fn fs_thread_entry_point(&self) {
        self.wait_for_thread_start();

        self.init_fs_thread_context();

        self.fs_scheduler_loop();

        self.fini_fs_thread_context();
    }

    /// Called at FS thread startup.
    #[cfg(feature = "asyncio")]
    fn init_fs_thread_context(&self) {
        let thread_name = "FS thread";

        let thread_context = ThreadContext::new(Some(&thread_name), 1, None); // `1` because any value other than `0` (`main thread`) is fine here.

        self.thread_context.store(thread_context);

        #[cfg(feature = "profiling")]
        {
            if let Some(profiler) = self.profiler.as_ref() {
                profiler.init_thread(&thread_name);
            }
        }
    }

    #[cfg(feature = "asyncio")]
    fn fs_scheduler_loop(&self) {
        while !self.scheduler_exit() {
            self.fs_scheduler_loop_iter();
        }
    }

    #[cfg(feature = "asyncio")]
    fn fs_scheduler_loop_iter(&self) {
        match self.iocp.wait(FS_WAIT_TIMEOUT) {
            IOCPResult::IOComplete(result) => {
                let yield_key = result.overlapped as usize;

                unsafe {
                    let io_handle = ReferenceHolder::<IOHandle>::from_address(yield_key);
                    io_handle.as_ref().ready_flag.store(true, Ordering::SeqCst);
                };

                self.try_resume_task(NonZeroUsize::new(yield_key).unwrap());
            }
            _ => {}
        }
    }

    /// Called before thread exit.
    #[cfg(feature = "asyncio")]
    fn fini_fs_thread_context(&self) {
        let thread_context = self.thread_context.take();
        mem::drop(thread_context);
    }

    #[cfg(feature = "asyncio")]
    fn shutdown_fs_thread(&mut self) {
        // Wake up the FS thread.
        self.iocp.set();

        // Wake for the FS threa to exit.
        self.fs_thread.take().unwrap().join().unwrap();
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
        let mut thread_context = self.thread_context.take();

        if self.use_fiber_pool {
            let current_fiber = thread_context.take_current_fiber();
            assert!(current_fiber.is_thread_fiber());
            assert_eq!(current_fiber.name().unwrap(), "Main thread fiber");
        }

        self.thread_context.free_index();
    }
}

pub(crate) trait YieldData {
    fn is_complete(&self) -> bool;
    fn yield_key(&self) -> NonZeroUsize;
}

struct WaitForTaskData<'task_handle> {
    handle: &'task_handle TaskHandle,
}

impl<'task_handle> YieldData for WaitForTaskData<'task_handle> {
    fn yield_key(&self) -> NonZeroUsize {
        NonZeroUsize::new(self.handle as *const _ as usize).unwrap()
    }

    fn is_complete(&self) -> bool {
        self.handle.is_complete()
    }
}