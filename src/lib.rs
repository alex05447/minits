#[cfg(feature = "tracing")]
#[macro_use]
extern crate log;

mod num_cores;
mod task;

#[cfg(feature = "profiling")]
mod profiler;

pub use {
    num_cores::{get_num_logical_cores, get_num_physical_cores},
    task::{
        fini_task_system, init_task_system, task_system, Handle, RangeTaskFn, Scope,
        ScopedRangeTask, ScopedTask, SliceTaskFn, TaskFn, TaskRange, TaskSystem, TaskSystemBuilder,
        ThreadInitFiniCallback,
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
