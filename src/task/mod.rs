mod builder;
mod handle;
mod panic;
mod scope;
mod task;
mod task_queue;
mod task_system;
mod task_system_singleton;
mod thread;
mod util;

#[cfg(feature = "fibers")]
mod fiber_pool;

#[cfg(feature = "fibers")]
mod yield_queue;

#[cfg(feature = "logging")]
mod task_system_logging;

#[cfg(feature = "profiling")]
mod task_system_profiling;

#[cfg(feature = "graph")]
mod task_system_graph;

#[cfg(feature = "asyncio")]
mod fs;

#[cfg(feature = "asyncio")]
mod task_system_io;

pub use {
    builder::{TaskSystemBuilder, ThreadInitFiniCallback},
    handle::Handle,
    panic::{PanicPayload, TaskPanicInfo, TaskPanics},
    scope::{RangeTaskFn, Scope, ScopedRangeTask, ScopedTask, SliceTaskFn, TaskFn, TaskRange, ForkMethod},
    task_system::TaskSystem,
    task_system_singleton::{fini_task_system, init_task_system, task_system},
};

#[cfg(feature = "asyncio")]
pub use fs::{File, OpenOptions};

#[cfg(feature = "graph")]
pub use scope::{GraphTaskFn, ScopedGraphTask};
