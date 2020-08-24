use {
    super::{thread::Thread, util::SendPtr},
    crate::TaskSystem,
    std::{mem, num::NonZeroUsize, sync::atomic::Ordering, thread, time::Duration},
};

#[cfg(feature = "asyncio")]
use {
    super::fs::IOHandle,
    miniiocp::IOCPResult,
    std::{fs, os::windows::io::AsRawHandle},
};

/// How much time the FS thread will wait for the IOCP to be signaled.
const FS_WAIT_TIMEOUT: Duration = Duration::from_secs(999);

impl TaskSystem {
    /// Initialize the FS async IO thread with `stack_size` bytes of stack space.
    /// NOTE - `self` must be allocated on the heap as the pointer is used by the worker thread entry point.
    pub(crate) fn create_fs_thread(&mut self, stack_size: usize) {
        let task_system = unsafe { SendPtr::new(self) };

        self.fs_thread = Some(
            thread::Builder::new()
                .stack_size(stack_size)
                .name("FS thread".to_owned())
                .spawn(move || {
                    task_system.fs_thread_entry_point();
                })
                .expect("FS thread creation failed"),
        );
    }

    pub(crate) fn associate_file(&self, file: &fs::File) {
        self.iocp.associate_handle(file.as_raw_handle()).unwrap();
    }

    fn fs_thread_entry_point(&self) {
        self.wait_for_thread_start();

        self.init_fs_thread_context();

        self.fs_scheduler_loop();

        self.fini_fs_thread_context();
    }

    /// Called at FS thread startup.
    fn init_fs_thread_context(&self) {
        let thread_name = "FS thread".to_owned();

        // `1` because any value other than `0` ("main thread") is fine here.
        let thread_context = Thread::new(thread_name, 1, None);

        self.thread.store(thread_context).unwrap();
    }

    /// FS thread scheduler loop.
    fn fs_scheduler_loop(&self) {
        while !self.scheduler_exit() {
            self.fs_scheduler_loop_iter();
        }
    }

    /// A single iteration of the FS thread scheduler loop.
    fn fs_scheduler_loop_iter(&self) {
        match self.iocp.wait(FS_WAIT_TIMEOUT).unwrap() {
            // FS thread was waken up by the IOCP on aync IO completion.
            IOCPResult::IOComplete(result) => {
                // Set the `ready_flag` in the IO handle on the waiting thread's / fiber's stack.
                let io_handle: *const IOHandle = result.overlapped.as_ptr() as _;

                unsafe {
                    (*io_handle).ready_flag.store(true, Ordering::Release);
                };

                // Try to resume a yielded fiber waiting on this IO handle, if any.
                let yield_key = unsafe { NonZeroUsize::new_unchecked(io_handle as _) };

                self.try_resume_task(yield_key);
            }
            _ => {}
        }
    }

    /// Called before thread exit.
    fn fini_fs_thread_context(&self) {
        let thread_context = unsafe { self.thread.take_unchecked() };
        mem::drop(thread_context);
    }

    pub(crate) fn shutdown_fs_thread(&mut self) {
        // Wake up the FS thread.
        self.iocp.set();

        // Wait for the FS thread to exit.
        self.fs_thread.take().unwrap().join().unwrap();
    }
}
