#[cfg(feature = "remotery")]
pub mod remotery;

#[cfg(feature = "remotery")]
pub use remotery::RemoteryProfiler;

pub trait Profiler {
    fn init_thread(&self, thread_name: &str);
    fn begin_scope(&self, name: Option<&str>);
    fn end_scope(&self);
}

pub struct ProfilerScope<'a>(&'a dyn Profiler);

impl<'a> Drop for ProfilerScope<'a> {
    fn drop(&mut self) {
        self.0.end_scope();
    }
}

pub type TaskSystemProfiler = dyn Profiler + Send + Sync;
