use std::cmp;
use std::fs;
use std::io;
use std::mem;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{ AtomicBool, Ordering };

use winapi::shared::minwindef::DWORD;
use winapi::shared::winerror::ERROR_IO_PENDING;
use winapi::um::errhandlingapi::GetLastError;
use winapi::um::fileapi::{ReadFile, WriteFile};
use winapi::um::ioapiset::GetOverlappedResult;
use winapi::um::minwinbase::OVERLAPPED;
use winapi::um::winbase::{
    SetFileCompletionNotificationModes, FILE_FLAG_OVERLAPPED, FILE_SKIP_COMPLETION_PORT_ON_SUCCESS,
    FILE_SKIP_SET_EVENT_ON_HANDLE,
};

use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;

use crate::task::task_system::{TaskSystem, YieldData};

/// Used by the task system to provide means for calling code to wait for IO operation completion.
/// Only one IO operation may be associated with one `IOHandle`.
/// The order of fields is important - we rely on `OVERLAPPED` being first.
#[repr(C)]
pub(crate) struct IOHandle {
    pub(crate) overlapped: OVERLAPPED,
    pub(crate) ready_flag: AtomicBool,
}

impl IOHandle {
    pub(crate) fn new() -> Self {
        Self {
            overlapped: unsafe { mem::zeroed() },
            ready_flag: AtomicBool::new(false),
        }
    }
}

pub struct File<'task_system> {
    file: fs::File,
    task_system: &'task_system TaskSystem,

    #[cfg(feature = "tracing")]
    file_name: String,
}

impl<'task_system> File<'task_system> {
    /// See [`open`](https://doc.rust-lang.org/std/fs/struct.File.html#method.open).
    pub fn open<P: AsRef<Path>>(
        task_system: &'task_system TaskSystem,
        path: P,
    ) -> io::Result<File<'task_system>> {
        OpenOptions::new().read(true).open(task_system, path)
    }

    /// See [`create`](https://doc.rust-lang.org/std/fs/struct.File.html#method.create).
    pub fn create<P: AsRef<Path>>(
        task_system: &'task_system TaskSystem,
        path: P,
    ) -> io::Result<File<'task_system>> {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(task_system, path)
    }

    /// See [`metadata`](https://doc.rust-lang.org/std/fs/struct.File.html#method.metadata).
    pub fn metadata(&self) -> io::Result<fs::Metadata> {
        self.file.metadata()
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        self.write_at(buf, 0)
    }

    pub fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        let mut io_handle = IOHandle::new();

        unsafe {
            io_handle.overlapped.u.s_mut().Offset = offset as u32;
            io_handle.overlapped.u.s_mut().OffsetHigh = (offset >> 32) as u32;
        }

        let len = cmp::min(buf.len(), <DWORD>::max_value() as usize) as DWORD;

        let mut written: DWORD = 0;

        let result = unsafe {
            WriteFile(
                self.file.as_raw_handle(),
                buf.as_ptr() as _,
                len,
                &mut written,
                &mut io_handle.overlapped, // Same as `&mut io_handle`.
            )
        };

        // The write completed synchronously.
        if result != 0 {
            return Ok(written as usize);
        } else {
            let error = unsafe { GetLastError() };

            // Completed asynchronously.
            if error == ERROR_IO_PENDING {
                self.wait_for_io_start(self.file_name(), false);

                let yield_data = WaitForIOData {
                    io_handle: &io_handle,
                };

                // Yield the current task and wait for the FS thread to resume us...
                self.task_system.yield_current_task(yield_data);
                // ... and we're back.

                self.wait_for_io_end(self.file_name(), false);

                let result = unsafe {
                    GetOverlappedResult(
                        self.file.as_raw_handle(),
                        &mut io_handle.overlapped,
                        &mut written,
                        0,
                    )
                };

                // Success.
                if result != 0 {
                    return Ok(written as usize);

                // Failure.
                } else {
                    return Err(io::Error::last_os_error());
                }
            } else {
                return Err(io::Error::from_raw_os_error(error as i32));
            }
        }
    }

    pub fn read_all(&self) -> io::Result<Vec<u8>> {
        self.read_all_at(0)
    }

    pub fn read_all_at(&self, offset: u64) -> io::Result<Vec<u8>> {
        let file_size = self.file.metadata().map(|m| m.len() as usize).unwrap_or(0);

        let mut bytes = Vec::with_capacity(file_size);

        unsafe {
            bytes.set_len(file_size);
        }

        self.read_at(&mut bytes, offset).map(|bytes_read| {
            assert!(bytes_read <= file_size);
            unsafe {
                bytes.set_len(bytes_read);
            }
            bytes
        })
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_at(buf, 0)
    }

    pub fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        let mut io_handle = IOHandle::new();

        unsafe {
            io_handle.overlapped.u.s_mut().Offset = offset as u32;
            io_handle.overlapped.u.s_mut().OffsetHigh = (offset >> 32) as u32;
        }

        let len = cmp::min(buf.len(), <DWORD>::max_value() as usize) as DWORD;

        let mut read: DWORD = 0;

        let result = unsafe {
            ReadFile(
                self.file.as_raw_handle(),
                buf.as_mut_ptr() as _,
                len,
                &mut read,
                &mut io_handle.overlapped, // Same as `&mut io_handle`.
            )
        };

        // The read completed synchronously.
        if result != 0 {
            return Ok(read as usize);
        } else {
            let error = unsafe { GetLastError() };

            // Completed asynchronously.
            if error == ERROR_IO_PENDING {
                self.wait_for_io_start(self.file_name(), true);

                let yield_data = WaitForIOData {
                    io_handle: &io_handle,
                };

                // Yield the current task and wait for the FS thread to resume us...
                self.task_system.yield_current_task(yield_data);
                // ... and we're back.

                self.wait_for_io_end(self.file_name(), true);

                let result = unsafe {
                    GetOverlappedResult(
                        self.file.as_raw_handle(),
                        &mut io_handle.overlapped,
                        &mut read,
                        0,
                    )
                };

                // Success.
                if result != 0 {
                    return Ok(read as usize);

                // Failure.
                } else {
                    return Err(io::Error::last_os_error());
                }
            } else {
                return Err(io::Error::from_raw_os_error(error as i32));
            }
        }
    }

    fn wait_for_io_start(&self, _file_name: Option<&str>, _read: bool) {
        #[cfg(any(feature = "tracing", feature = "profiling"))]
        let _task_name = self.task_system.current_task_name_or_unnamed();

        #[cfg(feature = "tracing")]
        {
            let s = format!(
                "[Wait start] For file {}: `{}` (cur. task: `{}`, thread: `{}`, fiber: `{}`)",
                _file_name.unwrap_or("<unnamed>"),
                if _read { "read" } else { "write" },
                _task_name,
                self.task_system.thread_name_or_unnamed(),
                self.task_system.fiber_name_or_unnamed()
            );
            trace!("{}", s);
        }

        #[cfg(feature = "profiling")]
        {
            if !self.task_system.is_main_task() {
                //println!("<<< end_cpu_sample `{}` (pre-wait)", _task_name);
                self.task_system.profiler_end_scope();
            }
        }
    }

    fn wait_for_io_end(&self, _file_name: Option<&str>, _read: bool) {
        #[cfg(any(feature = "tracing", feature = "profiling"))]
        let _task_name = self.task_system.current_task_name_or_unnamed();

        #[cfg(feature = "tracing")]
        {
            let s = format!(
                "[Wait end] For file {}: `{}` (cur. task: `{}`, thread: `{}`, fiber: `{}`)",
                if _read { "read" } else { "write" },
                _file_name.unwrap_or("<unnamed>"),
                _task_name,
                self.task_system.thread_name_or_unnamed(),
                self.task_system.fiber_name_or_unnamed()
            );
            trace!("{}", s);
        }

        #[cfg(feature = "profiling")]
        {
            if !self.task_system.is_main_task() {
                //println!(">>> begin_cpu_sample `{}` (post-wait)", _task_name);
                self.task_system.profiler_begin_scope(_task_name);
            }
        }
    }

    fn new(file: fs::File, task_system: &'task_system TaskSystem, _file_name: String) -> Self {
        #[cfg(feature = "tracing")]
        {
            Self {
                file,
                task_system,
                file_name: _file_name,
            }
        }

        #[cfg(not(feature = "tracing"))]
        {
            Self { file, task_system }
        }
    }

    fn file_name(&self) -> Option<&str> {
        #[cfg(feature = "tracing")]
        {
            return Some(&self.file_name);
        }
        #[cfg(not(feature = "tracing"))]
        None
    }
}

/// See [`OpenOptions`](https://doc.rust-lang.org/std/fs/struct.OpenOptions.html).
pub struct OpenOptions(fs::OpenOptions);

impl OpenOptions {
    /// See [`new`](https://doc.rust-lang.org/std/fs/struct.OpenOptions.html#method.new).
    pub fn new() -> Self {
        let mut open_options = fs::OpenOptions::new();
        open_options.custom_flags(FILE_FLAG_OVERLAPPED);
        OpenOptions(open_options)
    }

    /// See [`read`](https://doc.rust-lang.org/std/fs/struct.OpenOptions.html#method.read).
    pub fn read(&mut self, read: bool) -> &mut Self {
        self.0.read(read);
        self
    }

    /// See [`write`](https://doc.rust-lang.org/std/fs/struct.OpenOptions.html#method.write).
    pub fn write(&mut self, write: bool) -> &mut Self {
        self.0.write(write);
        self
    }

    /// See [`append`](https://doc.rust-lang.org/std/fs/struct.OpenOptions.html#method.append).
    pub fn append(&mut self, append: bool) -> &mut Self {
        self.0.append(append);
        self
    }

    /// See [`truncate`](https://doc.rust-lang.org/std/fs/struct.OpenOptions.html#method.truncate).
    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.0.truncate(truncate);
        self
    }

    /// See [`create`](https://doc.rust-lang.org/std/fs/struct.OpenOptions.html#method.create).
    pub fn create(&mut self, create: bool) -> &mut Self {
        self.0.create(create);
        self
    }

    /// See [`create_new`](https://doc.rust-lang.org/std/fs/struct.OpenOptions.html#method.create_new).
    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.0.create_new(create_new);
        self
    }

    /// See [`open`](https://doc.rust-lang.org/std/fs/struct.OpenOptions.html#method.open).
    pub fn open<'task_system, P: AsRef<Path>>(
        &self,
        task_system: &'task_system TaskSystem,
        path: P,
    ) -> io::Result<File<'task_system>> {
        let file_name = String::from(path.as_ref().to_str().unwrap());

        let file = self.0.open(path)?;

        // Do not notify the IOCP if the operation actually completed synchronously
        // (e.g., the file was opened for sync IO).
        // Also do not set the file handle / explicit event in `OVERLAPPED` on completion.
        let result = unsafe {
            SetFileCompletionNotificationModes(
                file.as_raw_handle(),
                FILE_SKIP_COMPLETION_PORT_ON_SUCCESS | FILE_SKIP_SET_EVENT_ON_HANDLE,
            )
        };
        assert!(result > 0);

        task_system.associate_file(&file);

        Ok(File::new(file, task_system, file_name))
    }
}

struct WaitForIOData<'io_handle> {
    io_handle: &'io_handle IOHandle,
}

impl<'fs_handle> YieldData for WaitForIOData<'fs_handle> {
    fn yield_key(&self) -> NonZeroUsize {
        NonZeroUsize::new(self.io_handle as *const _ as usize).unwrap()
    }

    fn is_complete(&self) -> bool {
        self.io_handle.ready_flag.load(Ordering::SeqCst)
    }
}
