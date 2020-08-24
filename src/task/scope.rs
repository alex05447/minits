use {
    crate::{Handle, TaskPanics, TaskSystem},
    std::{
        borrow::Cow,
        ops::{Index, Range},
        panic::resume_unwind,
    },
};

#[cfg(feature = "graph")]
use minigraph::{TaskGraph, VertexID};

/// Trait for a one-off task, executed by the [`TaskSystem`] -
/// a function/closure which takes a single [`TaskSystem`] argument, returns nothing, executes once,
/// and must be safe to send to another thread.
///
/// [`TaskSystem`]: struct.TaskSystem.html
pub trait TaskFn<'t>: FnOnce(&TaskSystem) + Send + 't {}

impl<'t, F: FnOnce(&TaskSystem) + Send + 't> TaskFn<'t> for F {}

/// A temporary struct returned by [`Scope::task`] from a [`task`] closure / function,
/// used to build / initialize other task parameters before actually [`submitting`] it.
///
/// [`Scope::task`]: struct.Scope.html#method.task
/// [`task`]: trait.TaskFn.html
/// [`submitting`]: #method.submit
pub struct ScopedTask<'t, 'h, 'ts, 'n, F>
where
    F: TaskFn<'h>,
{
    scope: &'t mut Scope<'h, 'ts>,
    task: Option<F>,
    main_thread: bool,
    name: Option<Cow<'n, str>>,
}

impl<'t, 'h, 'ts, 'n, F> ScopedTask<'t, 'h, 'ts, 'n, F>
where
    F: TaskFn<'h>,
{
    fn new(scope: &'t mut Scope<'h, 'ts>, f: F) -> Self {
        Self {
            scope,
            main_thread: false,
            task: Some(f),
            name: None,
        }
    }

    /// Makes it so the task will only run in the "main" thread.
    ///
    /// By default, tasks may be executed on any thread.
    pub fn main_thread(mut self) -> Self {
        self.main_thread = true;
        self
    }

    /// Sets the task's `name`.
    ///
    /// `name` may be retrieved by [`TaskSystem::task_name`] within the task closure.
    ///
    /// NOTE - requires "task_names" feature; otherwise the `name` is ignored.
    ///
    /// [`TaskSystem::task_name`]: struct.TaskSystem.html#method.task_name
    #[allow(unused_variables, unused_mut)]
    pub fn name<N: Into<Cow<'n, str>>>(mut self, name: N) -> Self {
        if cfg!(feature = "task_names") {
            self.name = Some(name.into());
        }
        self
    }

    /// Consumes and submits the fully initialized task for execution by the [`TaskSystem`].
    /// The task is guaranteed to be finished by the time the associated [`Scope`] goes out of scope.
    ///
    /// NOTE - an alternative way to submit the task is to just let this `ScopedTask` object drop.
    ///
    /// [`TaskSystem`]: struct.TaskSystem.html
    /// [`Scope`]: struct.Scope.html
    pub fn submit(mut self) {
        self.submit_impl();
    }

    fn take_name(&mut self) -> Option<String> {
        self.name.take().map(|name| name.into_owned())
    }

    fn submit_impl(&mut self) {
        if let Some(task) = self.task.take() {
            let name = self.take_name();
            self.scope.submit(task, self.main_thread, name);
        }
    }
}

impl<'t, 'h, 'ts, 'n, F> Drop for ScopedTask<'t, 'h, 'ts, 'n, F>
where
    F: TaskFn<'h>,
{
    fn drop(&mut self) {
        self.submit_impl();
    }
}

/// The type representing a range of [`tasks`] to be executed by the [`TaskSystem`] in parallel.
///
/// [`tasks`]: trait.RangeTaskFn.html
/// [`TaskSystem`]: struct.TaskSystem.html
pub type TaskRange = Range<u32>;

/// Trait for a "range" task, executed by the [`TaskSystem`] -
/// a function/closure which takes a [`TaskRange`] and a [`TaskSystem`] argument, returns nothing, may execute multiple times, is cloneable
/// and must be safe to send to another thread.
///
/// [`TaskRange`]: type.TaskRange.html
/// [`TaskSystem`]: struct.TaskSystem.html
pub trait RangeTaskFn<'t>: Fn(TaskRange, &TaskSystem) + Send + Clone + 't {}

impl<'t, F: Fn(TaskRange, &TaskSystem) + Send + Clone + 't> RangeTaskFn<'t> for F {}

/// Trait for a "slice" task, executed by the [`TaskSystem`] -
/// a function/closure which takes a reference to a slice element and a [`TaskSystem`] argument, returns nothing, may execute multiple times, is cloneable
/// and must be safe to send to another thread.
///
/// [`TaskSystem`]: struct.TaskSystem.html
pub trait SliceTaskFn<'t, T>: Fn(&T, &TaskSystem) + Send + Clone + 't {}

impl<'t, T, F: Fn(&T, &TaskSystem) + Send + Clone + 't> SliceTaskFn<'t, T> for F {}

/// A temporary struct returned by [`Scope::task_range`] from a [`task range`] and a [`task`] closure / function,
/// used to build / initialize other task parameters before actually [`submitting`] it.
///
/// [`Scope::task_range`]: struct.Scope.html#method.task_range
/// [`task range`]: type.TaskRange.html
/// [`task`]: trait.RangeTaskFn.html
/// [`submitting`]: #method.submit
pub struct ScopedRangeTask<'t, 'h, 'ts, 'n, F>
where
    F: RangeTaskFn<'h>,
{
    scope: &'t mut Scope<'h, 'ts>,
    task: Option<F>,
    range: TaskRange,
    multiplier: u32,
    name: Option<Cow<'n, str>>,
}

impl<'t, 'h, 'ts, 'n, F> ScopedRangeTask<'t, 'h, 'ts, 'n, F>
where
    F: RangeTaskFn<'h>,
{
    fn new(scope: &'t mut Scope<'h, 'ts>, f: F, range: TaskRange) -> Self {
        Self {
            scope,
            task: Some(f),
            range,
            multiplier: 1,
            name: None,
        }
    }

    /// Sets the task's range subdivision `multiplier`.
    ///
    /// The task's [`range`] is distributed over the number of [`TaskSystem`] threads (including the "main" thread),
    /// multiplied by `multiplier` (i.e., `multiplier` == `2` means "divide the range into `2` chunks per thread").
    ///
    /// By default, the `multiplier` is `1`.
    ///
    /// [`range`]: type.TaskRange.html
    /// [`TaskSystem`]: struct.TaskSystem.html
    pub fn multiplier(mut self, multiplier: u32) -> Self {
        self.multiplier = multiplier;
        self
    }

    /// Sets the task's `name`.
    ///
    /// `name` may be retrieved by [`TaskSystem::task_name`] within the task closure.
    ///
    /// NOTE - requires "task_names" feature; otherwise the `name` is ignored.
    ///
    /// [`TaskSystem::task_name`]: struct.TaskSystem.html#method.task_name
    #[allow(unused_variables, unused_mut)]
    pub fn name<N: Into<Cow<'n, str>>>(mut self, name: N) -> Self {
        if cfg!(feature = "task_names") {
            self.name = Some(name.into());
        }
        self
    }

    /// Consumes and submits the fully initialized task for execution by the [`TaskSystem`].
    /// The task is guaranteed to be finished by the time the associated [`Scope`] goes out of scope.
    ///
    /// NOTE - an alternative way to submit the task is to just let this `ScopedRangeTask` object drop.
    ///
    /// [`TaskSystem`]: struct.TaskSystem.html
    /// [`Scope`]: struct.Scope.html
    pub fn submit(mut self) {
        self.submit_impl();
    }

    fn take_name(&mut self) -> Option<String> {
        self.name.take().map(|name| name.into_owned())
    }

    fn submit_impl(&mut self) {
        if let Some(task) = self.task.take() {
            let range = self.range.clone();
            let name = self.take_name();
            self.scope.submit_range(task, range, self.multiplier, name);
        }
    }
}

impl<'t, 'h, 'ts, 'n, F> Drop for ScopedRangeTask<'t, 'h, 'ts, 'n, F>
where
    F: RangeTaskFn<'h>,
{
    fn drop(&mut self) {
        self.submit_impl();
    }
}

/// Trait for a "graph" task, executed by the [`TaskSystem`] -
/// a function/closure which takes a reference to a [`task graph`] vertex payload and a [`TaskSystem`] argument, returns nothing, may execute multiple times, is cloneable
/// and must be safe to send to another thread.
///
/// [`TaskSystem`]: struct.TaskSystem.html
/// [`task graph`]: struct.TaskGraph.html
#[cfg(feature = "graph")]
pub trait GraphTaskFn<'t, T>: Fn(&T, &TaskSystem) + Send + Clone + 't {}

#[cfg(feature = "graph")]
impl<'t, T, F: Fn(&T, &TaskSystem) + Send + Clone + 't> GraphTaskFn<'t, T> for F {}

/// A temporary struct returned by [`Scope::graph`] from a [`task graph`] and a [`graph`] closure / function,
/// used to build / initialize other task parameters before actually [`submitting`] it.
///
/// [`Scope::graph`]: struct.Scope.html#method.graph
/// [`task graph`]: struct.TaskGraph.html
/// [`graph`]: trait.GraphTaskFn.html
/// [`submitting`]: #method.submit
#[cfg(feature = "graph")]
pub struct ScopedGraphTask<'t, 'h, 'ts, 'n, VID, T, F>
where
    VID: VertexID + Send + Sync,
    T: Send + Sync,
    F: GraphTaskFn<'h, T>,
{
    scope: &'t mut Scope<'h, 'ts>,
    task: Option<F>,
    graph: &'h TaskGraph<VID, T>,
    name: Option<Cow<'n, str>>,
}

#[cfg(feature = "graph")]
impl<'t, 'h, 'ts, 'n, VID, T, F> ScopedGraphTask<'t, 'h, 'ts, 'n, VID, T, F>
where
    VID: VertexID + Send + Sync,
    T: Send + Sync,
    F: GraphTaskFn<'h, T>,
{
    fn new(scope: &'t mut Scope<'h, 'ts>, f: F, graph: &'h TaskGraph<VID, T>) -> Self {
        Self {
            scope,
            task: Some(f),
            graph,
            name: None,
        }
    }

    /// Sets the task's `name`.
    ///
    /// `name` may be retrieved by [`TaskSystem::task_name`] within the task closure.
    ///
    /// NOTE - requires "task_names" feature; otherwise the `name` is ignored.
    ///
    /// [`TaskSystem::task_name`]: struct.TaskSystem.html#method.task_name
    #[allow(unused_variables, unused_mut)]
    pub fn name<N: Into<Cow<'n, str>>>(mut self, name: N) -> Self {
        if cfg!(feature = "task_names") {
            self.name = Some(name.into());
        }
        self
    }

    /// Consumes and submits the fully initialized task for execution by the [`TaskSystem`].
    /// The task is guaranteed to be finished by the time the associated [`Scope`] goes out of scope.
    ///
    /// NOTE - an alternative way to submit the task is to just let this `ScopedGraphTask` object drop.
    ///
    /// [`TaskSystem`]: struct.TaskSystem.html
    /// [`Scope`]: struct.Scope.html
    pub fn submit(mut self) {
        self.submit_impl();
    }

    fn take_name(&mut self) -> Option<String> {
        self.name.take().map(|name| name.into_owned())
    }

    fn submit_impl(&mut self) {
        if let Some(task) = self.task.take() {
            let name = self.take_name();
            self.scope.submit_graph(task, self.graph, name);
        }
    }
}

#[cfg(feature = "graph")]
impl<'t, 'h, 'ts, 'n, VID, T, F> Drop for ScopedGraphTask<'t, 'h, 'ts, 'n, VID, T, F>
where
    VID: VertexID + Send + Sync,
    T: Send + Sync,
    F: GraphTaskFn<'h, T>,
{
    fn drop(&mut self) {
        self.submit_impl();
    }
}

/// Provides borrow-safe task system functionality.
///
/// Ensures that
/// 1) all tasks associated with the `Scope` are finished (and all their borrows released)
/// before the `Scope` goes out of scope (is dropped);
/// 2) no mutable borrow aliasing is allowed for tasks associated with this `Scope`,
/// and borrow lifetimes are enforced accorfing to safe Rust rules.
///
/// There are two ways to wait on the scope -
/// calling [`wait`] on it, which returns any and all [`panics`] which occured in children tasks,
/// or dropping it, which logs (requires "logging" feature) / prints the panics, if any, and re-throws the [`panics`].
///
/// NOTE - it is unsafe to `std::mem::forget()` the `Scope`, as its drop handler will not run
/// and thus there's no guarantee any tasks associated with the `Scope` will be complete.
///
/// [`wait`]: #method.wait
/// [`panics`]: struct.TaskPanics.html
pub struct Scope<'h, 'ts> {
    /// The scope's task handle.
    /// Needed for the stable address of its atomic task counter, incl. in the `Scope`'s `Drop` handler.
    handle: &'h Handle,
    task_system: &'ts TaskSystem,

    #[cfg(feature = "task_names")]
    name: Option<String>,
}

impl<'h, 'ts> Scope<'h, 'ts> {
    #[allow(unused_variables)]
    pub(crate) fn new<N: Into<Option<String>>>(
        handle: &'h Handle,
        task_system: &'ts TaskSystem,
        name: N,
    ) -> Self {
        Self {
            handle,
            task_system,

            #[cfg(feature = "task_names")]
            name: name.into(),
        }
    }

    /// Takes a [`task`] closure / function and returns a [`task builder object`]
    /// which is used to configure the task's other paramters, if necessary,
    /// and then to [`submit`] it for execution by the [`TaskSystem`].
    ///
    /// NOTE - an alternative way to submit the task is to just let the returned `ScopedTask` object to drop.
    ///
    /// [`task`]: trait.TaskFn.html
    /// [`task builder object`]: struct.ScopedTask.html
    /// [`submit`]: struct.ScopedTask.html#method.submit
    /// [`TaskSystem`]: struct.TaskSystem.html
    pub fn task<F>(&mut self, f: F) -> ScopedTask<'_, 'h, 'ts, '_, F>
    where
        //F: TaskFn<'h>,
        // NOTE - for some reason, cannot use the `TaskFn` closure trait alias here and have the compiler figure out
        // the type of the closure's argument (`&TaskSystem`).
        // Workaround is to use the full trait bound here.
        F: FnOnce(&TaskSystem) + Send + 'h,
    {
        ScopedTask::new(self, f)
    }

    /// Takes a [`range task`] closure / function `f` and a [`range`] and returns a [`task builder object`]
    /// which is used to configure the task's other paramters, if necessary,
    /// and then to [`submit`] it for execution by the [`TaskSystem`].
    ///
    /// NOTE - an alternative way to submit the task is to just let the returned `ScopedRangeTask` object to drop.
    ///
    /// A non-empty [`range`] is distributed over the number of [`TaskSystem`] threads (including the "main" thread),
    /// multiplied by [`multiplier`] (i.e., [`multiplier`] == `2` means "divide the range into `2` chunks per thread").
    ///
    /// [`task`]: trait.RangeTaskFn.html
    /// [`range`]: type.TaskRange.html
    /// [`task builder object`]: struct.ScopedRangeTask.html
    /// [`submit`]: struct.ScopedRangeTask.html#method.submit
    /// [`TaskSystem`]: struct.TaskSystem.html
    /// [`multiplier`]: struct.ScopedRangeTask.html#method.multiplier
    pub fn task_range<F>(&mut self, range: TaskRange, f: F) -> ScopedRangeTask<'_, 'h, 'ts, '_, F>
    where
        //F: RangeTaskFn<'h>,
        // NOTE - for some reason, cannot use the `RangeTaskFn` closure trait alias here and have the compiler figure out
        // the type of the closure's arguments (`TaskRange`, `&TaskSystem`).
        // Workaround is to use the full trait bound here.
        F: Fn(TaskRange, &TaskSystem) + Send + Clone + 'h,
    {
        ScopedRangeTask::new(self, f, range)
    }

    /// Takes a [`slice task`] closure / function `f`, which operates on a single `slice` element,
    /// and a `slice`, and returns a [`task builder object`]
    /// which is used to configure the task's other paramters, if necessary,
    /// and then to [`submit`] it for execution by the [`TaskSystem`].
    ///
    /// This is a utility wrapper over [`task_range`], which invokes the user closure / function on each `slice` element.
    ///
    /// NOTE - an alternative way to submit the task is to just let the returned `ScopedRangeTask` object to drop.
    ///
    /// A non-empty `slice` is distributed over the number of [`TaskSystem`] threads (including the "main" thread),
    /// multiplied by [`multiplier`] (i.e., [`multiplier`] == `2` means "divide the range into `2` chunks per thread").
    ///
    /// [`slice task`]: trait.SliceTaskFn.html
    /// [`task builder object`]: struct.ScopedRangeTask.html
    /// [`submit`]: struct.ScopedRangeTask.html#method.submit
    /// [`TaskSystem`]: struct.TaskSystem.html
    /// [`multiplier`]: struct.ScopedRangeTask.html#method.multiplier
    /// [`task_range`]: #method.task_range
    pub fn slice_range<F, T: Send + Sync>(
        &mut self,
        slice: &'h [T],
        f: F,
    ) -> ScopedRangeTask<'_, 'h, 'ts, '_, impl RangeTaskFn<'h>>
    where
        //F: SliceTaskFn<'h, T>,
        // NOTE - for some reason, cannot use the `SliceTaskFn` closure trait alias here and have the compiler figure out
        // the type of the closure's arguments (`&T`, `&TaskSystem`).
        // Workaround is to use the full trait bound here.
        F: Fn(&T, &TaskSystem) + Send + Clone + 'h,
    {
        ScopedRangeTask::new(
            self,
            move |task_range: TaskRange, task_system: &TaskSystem| {
                for i in task_range {
                    f(slice.index(i as usize), task_system);
                }
            },
            0..slice.len() as u32,
        )
    }

    /// Takes a [`graph task`] closure / function `f`, which operates on a single [`graph`] vertex payload,
    /// and a task [`graph`], and returns a [`task builder object`]
    /// which is used to configure the task's other paramters, if necessary,
    /// and then to [`submit`] it for execution by the [`TaskSystem`].
    ///
    /// NOTE - an alternative way to submit the task is to just let the returned `ScopedGraphTask` object to drop.
    ///
    /// The [`TaskSystem`] executes the closure `f` over the [`graph`], root-to-leaf,
    /// executing the independent vertices in parallel.
    ///
    /// Dependent vertices are only processed when all of their dependencies have been fulfilled.
    ///
    /// [`graph task`]: trait.GraphTaskFn.html
    /// [`graph`]: struct.TaskGraph.html
    /// [`task builder object`]: struct.ScopedGraphTask.html
    /// [`submit`]: struct.ScopedGraphTask.html#method.submit
    /// [`TaskSystem`]: struct.TaskSystem.html
    #[cfg(feature = "graph")]
    pub fn graph<VID, T, F>(
        &mut self,
        graph: &'h TaskGraph<VID, T>,
        f: F,
    ) -> ScopedGraphTask<'_, 'h, 'ts, '_, VID, T, F>
    where
        VID: VertexID + Send + Sync,
        T: Send + Sync,
        //F: GraphTaskFn<'h, T>
        // NOTE - for some reason, cannot use the `GraphTaskFn` closure trait alias here and have the compiler figure out
        // the type of the closure's arguments (`&T`, `&TaskSystem`).
        // Workaround is to use the full trait bound here.
        F: Fn(&T, &TaskSystem) + Clone + Send + 'h,
    {
        ScopedGraphTask::new(self, f, graph)
    }

    /// Block the current thread until all tasks associated with the [`scope`] are complete.
    /// Attempts to yield the current fiber, if any; otherwise attempts to execute the tasks inline.
    ///
    /// Returns the [`task panics`] if any occured in tasks associated with the [`scope`],
    /// and any of their children tasks as well.
    ///
    /// NOTE: dropping the [`scope`] achieves the same goal, but does not allow you to handle the panics.
    ///
    /// [`scope`]: struct.Scope.html
    /// [`task panics`]: struct.TaskPanics.html
    pub fn wait(self) -> Result<(), TaskPanics> {
        self.wait_impl()
    }

    fn wait_impl(&self) -> Result<(), TaskPanics> {
        unsafe {
            self.task_system
                .wait_for_handle_impl(self.handle, self.name())
        }
    }

    /// Actually submits the fully initialized [`task`] for execution by the [`TaskSystem`].
    /// The task is guaranteed to be finished by the time this `Scope` goes out of scope.
    ///
    /// [`task`]: struct.ScopedTask.html
    /// [`TaskSystem`]: struct.TaskSystem.html
    fn submit<F>(&mut self, f: F, main_thread: bool, name: Option<String>)
    where
        F: TaskFn<'h>,
    {
        unsafe {
            self.task_system
                .submit(self.handle, f, main_thread, name, self.name());
        }
    }

    /// Actually submits the fully initialized [`range task`] for execution by the [`TaskSystem`].
    /// The task is guaranteed to be finished by the time this `Scope` goes out of scope.
    ///
    /// [`range task`]: struct.ScopedRangeTask.html
    /// [`TaskSystem`]: struct.TaskSystem.html
    fn submit_range<F>(&mut self, f: F, range: TaskRange, multiplier: u32, name: Option<String>)
    where
        F: RangeTaskFn<'h>,
    {
        unsafe {
            self.task_system.submit_range(
                self.handle,
                f,
                range,
                multiplier,
                name.as_ref().map(String::as_str),
                self.name(),
            );
        }
    }

    /// Actually submits the fully initialized [`graph task`] for execution by the [`TaskSystem`].
    /// The task is guaranteed to be finished by the time this `Scope` goes out of scope.
    ///
    /// [`graph task`]: struct.ScopedGraphTask.html
    /// [`TaskSystem`]: struct.TaskSystem.html
    #[cfg(feature = "graph")]
    fn submit_graph<VID, T, F>(&mut self, f: F, graph: &'h TaskGraph<VID, T>, name: Option<String>)
    where
        VID: VertexID + Send + Sync,
        T: Send + Sync,
        F: GraphTaskFn<'h, T>,
    {
        unsafe {
            self.task_system
                .submit_graph(self.handle, f, graph, name, self.name())
        }
    }

    pub(crate) fn name(&self) -> Option<&str> {
        #[cfg(feature = "task_names")]
        return self.name.as_ref().map(|s| s.as_str());

        #[cfg(not(feature = "task_names"))]
        return None;
    }
}

impl<'h, 'ts> Drop for Scope<'h, 'ts> {
    fn drop(&mut self) {
        if let Err(panics) = self.wait_impl() {
            #[cfg(feature = "logging")]
            {
                error!("{}", format!("{}", panics));
            }
            #[cfg(not(feature = "logging"))]
            {
                eprintln!("{}", format!("{}", panics));
            }

            resume_unwind(Box::new(panics));
        }
    }
}
