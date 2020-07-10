use {
    crate::{TaskSystem, TaskSystemBuilder},
    std::{
        mem,
        sync::atomic::{AtomicBool, Ordering},
    },
};

static mut TASK_SYSTEM: *mut TaskSystem = 0 as *mut _;
static TASK_SYSTEM_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Initializes the global [`task system`] singleton.
///
/// Call once at program startup from the "main" thread.
/// After this call the [`task system`] API may be used from this "main" thread
/// and from within any tasks spawned by it.
///
/// # Panics
///
/// Panics if the [`task system`] has already been initialized.
///
/// [`task system`]: struct.TaskSystem.html
pub fn init_task_system(builder: TaskSystemBuilder) {
    if TASK_SYSTEM_INITIALIZED.load(Ordering::Acquire) {
        panic!("task system already initialized");
    }

    let task_system = builder.build();

    unsafe {
        TASK_SYSTEM = Box::into_raw(task_system);
    }

    TASK_SYSTEM_INITIALIZED.store(true, Ordering::Release);
}

/// Finalizes the global [`task system`] singleton.
///
/// Call once after the [`task system`] is no longer necessary.
/// May only be called from the "main" thread (same thread [`init_task_system`] was called from),
/// and from within any tasks spawned by it.
///
/// # Panics
///
/// Panics if the [`task system`] has not been initialized or has already been finalized.
///
/// [`task system`]: struct.TaskSystem.html
/// [`init_task_system`]: fn.init_task_system.html
pub fn fini_task_system() {
    if !TASK_SYSTEM_INITIALIZED.load(Ordering::Acquire) {
        panic!("task system has not been initialized or was already finalized");
    }

    let task_system = unsafe { Box::from_raw(TASK_SYSTEM) };

    TASK_SYSTEM_INITIALIZED.store(false, Ordering::Release);

    unsafe {
        TASK_SYSTEM = 0 as *mut _;
    }

    mem::drop(task_system);
}

/// Returns a reference to the global [`task system`] singleton.
///
/// Safe to call after [`init_task_system`], before [`fini_task_system`].
///
/// # Panics
///
/// Panics if the [`task system`] has not been initialized or has already been finalized.
///
/// [`task system`]: struct.TaskSystem.html
/// [`init_task_system`]: fn.init_task_system.html
/// [`fini_task_system`]: fn.fini_task_system.html
pub fn task_system() -> &'static TaskSystem {
    if !TASK_SYSTEM_INITIALIZED.load(Ordering::Acquire) {
        panic!("task system has not been initialized or was already finalized");
    }

    unsafe { &*TASK_SYSTEM }
}
