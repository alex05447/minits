#[cfg(feature = "tracing")]
#[macro_use]
extern crate log;

mod num_cores;
mod task;

#[cfg(feature = "profiling")]
mod profiler;

pub use num_cores::{get_num_logical_cores, get_num_physical_cores};
pub use task::{
    fini_task_system, handle, init_task_system, scope, scope_named, task_system, TaskHandle,
    TaskScope, TaskSystem, TaskSystemParams,
};

#[cfg(feature = "profiling")]
pub use task::TaskSystemProfiler;

#[cfg(feature = "profiling")]
pub use profiler::{Profiler, ProfilerScope};

#[cfg(feature = "remotery")]
pub use profiler::remotery::RemoteryProfiler;

#[cfg(feature = "asyncio")]
pub use task::{File, OpenOptions};
