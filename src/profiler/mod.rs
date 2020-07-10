#[cfg(feature = "remotery")]
pub mod remotery;

#[cfg(feature = "remotery")]
pub use remotery::RemoteryProfiler;

pub trait Profiler {
    fn init_thread(&self, thread_name: &str);
    fn begin_scope(&self, name: &str);
    fn end_scope(&self);
}

pub fn profiler_scope<'p>(profiler: &'p dyn Profiler, name: &str) -> ProfilerScope<'p> {
    profiler.begin_scope(name);

    ProfilerScope(profiler)
}

pub struct ProfilerScope<'a>(&'a dyn Profiler);

impl<'a> Drop for ProfilerScope<'a> {
    fn drop(&mut self) {
        self.0.end_scope();
    }
}

pub type TaskSystemProfiler = dyn Profiler + Send + Sync;
