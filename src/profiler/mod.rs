#[cfg(feature = "remotery")]
pub mod remotery;

#[cfg(feature = "remotery")]
pub use remotery::RemoteryProfiler;

pub trait Profiler {
    fn init_thread(&self, thread_name: &str);
    fn begin_scope(&self, name: &str);
    fn end_scope(&self);
}

pub fn profiler_scope<'a>(profiler: &'a dyn Profiler, name: &str) -> ProfilerScope<'a> {
    profiler.begin_scope(name);

    ProfilerScope { profiler }
}

pub struct ProfilerScope<'a> {
    profiler: &'a dyn Profiler,
}

impl<'a> Drop for ProfilerScope<'a> {
    fn drop(&mut self) {
        self.profiler.end_scope();
    }
}
