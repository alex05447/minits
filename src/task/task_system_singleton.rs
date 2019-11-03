use std::sync::atomic::{AtomicBool, Ordering};

use super::task_handle::TaskHandle;
use super::task_scope::TaskScope;
use super::task_system::{TaskSystem, TaskSystemBuilder};

static mut TASK_SYSTEM: *mut TaskSystem = 0 as *mut _;
static TASK_SYSTEM_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Initializes the global task system singleton.
/// Call once at program startup from the "main" thread.
/// After this call the task system API may be used from this "main" thread
/// and from within any tasks spawned by it.
///
/// # Panics
///
/// Panics if the task system has already been initialized.
pub fn init_task_system(builder: TaskSystemBuilder) {
    if TASK_SYSTEM_INITIALIZED.load(Ordering::SeqCst) {
        panic!("Task system already initialized.");
    }

    let task_system = builder.build();

    unsafe {
        TASK_SYSTEM = Box::into_raw(task_system);
    }

    TASK_SYSTEM_INITIALIZED.store(true, Ordering::SeqCst);
}

/// Finalizes the global task system singleton.
/// Call once after the task system is no longer necessary.
/// May only be called from the "main" thread (same thread [`init_task_system`] was called from),
/// and from within any tasks spawned by it.
///
/// # Panics
///
/// Panics if the task system has not been initialized or has already been finalized.
///
/// [`init_task_system`]: fn.init_task_system.html
pub fn fini_task_system() {
    if !TASK_SYSTEM_INITIALIZED.load(Ordering::SeqCst) {
        panic!("Task system has not been initialized or is being finalized more than once.");
    }

    let _task_system = unsafe { Box::from_raw(TASK_SYSTEM) };

    TASK_SYSTEM_INITIALIZED.store(false, Ordering::SeqCst);

    unsafe {
        TASK_SYSTEM = 0 as *mut _;
    }
}

/// Returns a reference to the global task system singleton
/// which provides the task system API, such as [`handle`] and [`scope`].
/// May only be called from the "main" thread (same thread [`init_task_system`] was called from),
/// as the task system API may not be used from other threads.
///
/// # Panics
///
/// Panics if the task system has not been initialized or has already been finalized.
///
/// [`handle`]: fn.handle.html
/// [`scope`]: fn.scope.html
/// [`init_task_system`]: fn.init_task_system.html
pub fn task_system() -> &'static TaskSystem {
    if !TASK_SYSTEM_INITIALIZED.load(Ordering::SeqCst) {
        panic!("Task system has not been initialized.");
    }

    unsafe { &*TASK_SYSTEM }
}

/// Creates a new [`TaskHandle`] for use with a [`TaskScope`].
///
/// # Panics
///
/// Panics if the task system has not been initialized or has already been finalized.
///
/// [`TaskHandle`]: struct.TaskHandle.html
/// [`TaskScope`]: struct.TaskScope.html
pub fn handle() -> TaskHandle {
    task_system().handle()
}

/// Creates a new [`TaskScope`] object which provides safe task system functionality.
/// Requires a [`TaskHandle`] created by a previous call to [`handle`].
///
/// # Panics
///
/// Panics if the task system has not been initialized or has already been finalized.
///
/// [`TaskHandle`]: struct.TaskHandle.html
/// [`TaskScope`]: struct.TaskScope.html
/// [`handle`]: fn.handle.html
pub fn scope<'scope>(task_handle: &'scope TaskHandle) -> TaskScope<'scope, 'static> {
    task_system().scope(task_handle)
}

/// Creates a new [`TaskScope`] object which provides all safe task system functionality.
/// Requires a task handle created by a previous call to [`handle`].
/// May only be called from the "main" thread (same thread [`init_task_system`] was called from),
/// as the task system API may not be used from other threads.
///
/// # Panics
///
/// Panics if the task system has not been initialized or has already been finalized.
///
/// [`TaskScope`]: struct.TaskScope.html
/// [`handle`]: fn.handle.html
/// [`init_task_system`]: fn.init_task_system.html
pub fn scope_named<'scope>(
    task_handle: &'scope TaskHandle,
    name: &str,
) -> TaskScope<'scope, 'static> {
    task_system().scope_named(task_handle, name)
}
