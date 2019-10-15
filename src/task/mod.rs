mod task_handle;

pub use task_handle::TaskHandle;

mod fiber_pool;
mod reference_holder;
mod task_context;
mod task_queue;
mod thread_context;
mod yield_queue;

#[cfg(feature = "asyncio")]
mod fs;

pub mod task_scope;
pub mod task_system;
pub mod task_system_singleton;

pub use task_scope::TaskScope;
pub use task_system::{TaskSystem, TaskSystemParams};
pub use task_system_singleton::{
    fini_task_system, handle, init_task_system, scope, scope_named, task_system,
};

#[cfg(feature = "profiling")]
pub use task_system::TaskSystemProfiler;

#[cfg(feature = "asyncio")]
pub use fs::{File, OpenOptions};
