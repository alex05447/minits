#[cfg(feature = "logging")]
#[macro_use]
extern crate log;

mod num_cores;
mod task;
mod util;

#[cfg(feature = "profiling")]
mod profiler;

pub use {
    num_cores::{get_num_logical_cores, get_num_physical_cores},
    task::{
        fini_task_system, init_task_system, task_system, Handle, PanicPayload, RangeTaskFn, Scope,
        ScopedRangeTask, ScopedTask, SliceTaskFn, TaskFn, TaskPanicInfo, TaskPanics, TaskRange,
        TaskSystem, TaskSystemBuilder, ThreadInitFiniCallback, ForkMethod
    },
};

#[cfg(feature = "profiling")]
pub use profiler::{Profiler, ProfilerScope, TaskSystemProfiler};

#[cfg(feature = "remotery")]
pub use profiler::remotery::RemoteryProfiler;

#[cfg(feature = "asyncio")]
pub use task::{File, OpenOptions};

#[cfg(feature = "graph")]
pub use task::{GraphTaskFn, ScopedGraphTask};
