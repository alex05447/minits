use miniremotery::Remotery;

use crate::profiler::Profiler;

pub struct RemoteryProfiler {
    _remotery: Remotery,
}

unsafe impl Send for RemoteryProfiler {}
unsafe impl Sync for RemoteryProfiler {}

impl RemoteryProfiler {
    pub fn new() -> Option<Self> {
        if let Ok(_remotery) = Remotery::initialize() {
            Some(Self { _remotery })
        } else {
            None
        }
    }
}

impl Profiler for RemoteryProfiler {
    fn init_thread(&self, thread_name: &str) {
        Remotery::set_current_thread_name(thread_name);
    }

    fn begin_scope(&self, name: &str) {
        Remotery::begin_cpu_sample(name, miniremotery::rmtSampleFlags::RMTSF_None);
    }

    fn end_scope(&self) {
        Remotery::end_cpu_sample();
    }
}
