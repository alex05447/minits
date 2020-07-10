use {
    crate::task::task_system::{TaskSystem, YieldData, YieldKey},
    std::{
        cmp, fs, io, mem,
        num::NonZeroUsize,
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
        path::Path,
        sync::atomic::{AtomicBool, Ordering},
    },
    winapi::{
        shared::{minwindef::DWORD, winerror::ERROR_IO_PENDING},
        um::{
            errhandlingapi::GetLastError,
            fileapi::{ReadFile, WriteFile},
            ioapiset::GetOverlappedResult,
            minwinbase::OVERLAPPED,
            winbase::{
                SetFileCompletionNotificationModes, FILE_FLAG_OVERLAPPED,
                FILE_SKIP_COMPLETION_PORT_ON_SUCCESS, FILE_SKIP_SET_EVENT_ON_HANDLE,
            },
        },
    },
};

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

/// See [`std::fs::File`](https://doc.rust-lang.org/std/fs/struct.File.html).
pub struct File<'ts> {
    file: fs::File,
    task_system: &'ts TaskSystem,

    #[cfg(feature = "tracing")]
    file_name: String,
}

impl<'ts> File<'ts> {
    /// See [`std::fs::File::open`](https://doc.rust-lang.org/std/fs/struct.File.html#method.open).
    pub fn open<P: AsRef<Path>>(task_system: &'ts TaskSystem, path: P) -> io::Result<File<'ts>> {
        OpenOptions::new().read(true).open(task_system, path)
    }

    /// See [`std::fs::File::create`](https://doc.rust-lang.org/std/fs/struct.File.html#method.create).
    pub fn create<P: AsRef<Path>>(task_system: &'ts TaskSystem, path: P) -> io::Result<File<'ts>> {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(task_system, path)
    }

    /// See [`std::fs::File::metadata`](https://doc.rust-lang.org/std/fs/struct.File.html#method.metadata).
    pub fn metadata(&self) -> io::Result<fs::Metadata> {
        self.file.metadata()
    }

    /// See [`std::fs::File`](https://doc.rust-lang.org/std/fs/struct.File.html),
    /// [`std::fs::File::write`](https://doc.rust-lang.org/std/io/trait.Write.html#tymethod.write).
    ///
    /// Write the whole `buf` to the file at offset `0`.
    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        self.write_at(buf, 0)
    }

    /// See [`std::fs::File`](https://doc.rust-lang.org/std/fs/struct.File.html),
    /// [`std::fs::File::write`](https://doc.rust-lang.org/std/io/trait.Write.html#tymethod.write).
    ///
    /// Write the whole `buf` to the file at `offset`.
    pub fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        let mut io_handle = IOHandle::new();

        unsafe {
            io_handle.overlapped.u.s_mut().Offset = offset as _;
            io_handle.overlapped.u.s_mut().OffsetHigh = (offset >> 32) as _;
        }

        let len = cmp::min(buf.len(), <DWORD>::max_value() as _) as _;

        let mut written = 0;

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
            return Ok(written as _);

        // Else the write either completed asynchronously or there was an error.
        } else {
            let error = unsafe { GetLastError() };

            // Completed asynchronously.
            if error == ERROR_IO_PENDING {
                self.wait_for_io_start(self.file_name(), false);

                let yield_data = WaitForIOData(&io_handle);

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
                    return Ok(written as _);

                // Failure.
                } else {
                    return Err(io::Error::last_os_error());
                }
            // Some actual error.
            } else {
                return Err(io::Error::from_raw_os_error(error as _));
            }
        }
    }

    /// See [`std::fs::File`](https://doc.rust-lang.org/std/fs/struct.File.html),
    /// [`std::fs::File::read`](https://doc.rust-lang.org/std/io/trait.Write.html#tymethod.read).
    ///
    /// Read the whole contents of the file to a Vec of bytes.
    pub fn read_all(&self) -> io::Result<Vec<u8>> {
        self.read_all_at(0)
    }

    /// See [`std::fs::File`](https://doc.rust-lang.org/std/fs/struct.File.html),
    /// [`std::fs::File::read`](https://doc.rust-lang.org/std/io/trait.Write.html#tymethod.read).
    ///
    /// Read the whole contents of the file past `offset` to a Vec of bytes.
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

    /// See [`std::fs::File`](https://doc.rust-lang.org/std/fs/struct.File.html),
    /// [`std::fs::File::read`](https://doc.rust-lang.org/std/io/trait.Write.html#tymethod.read).
    ///
    /// Read `buf.len()` bytes from the file at offset `0`.
    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_at(buf, 0)
    }

    /// See [`std::fs::File`](https://doc.rust-lang.org/std/fs/struct.File.html),
    /// [`std::fs::File::read`](https://doc.rust-lang.org/std/io/trait.Write.html#tymethod.read).
    ///
    /// Read `buf.len()` bytes from the file at `offset`.
    pub fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        let mut io_handle = IOHandle::new();

        unsafe {
            io_handle.overlapped.u.s_mut().Offset = offset as _;
            io_handle.overlapped.u.s_mut().OffsetHigh = (offset >> 32) as _;
        }

        let len = cmp::min(buf.len(), <DWORD>::max_value() as _) as _;

        let mut read = 0;

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
            return Ok(read as _);

        // Else the read either completed asynchronously or there was an error.
        } else {
            let error = unsafe { GetLastError() };

            // Completed asynchronously.
            if error == ERROR_IO_PENDING {
                self.wait_for_io_start(self.file_name(), true);

                let yield_data = WaitForIOData(&io_handle);

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
                    return Ok(read as _);

                // Failure.
                } else {
                    return Err(io::Error::last_os_error());
                }
            // Some actual error.
            } else {
                return Err(io::Error::from_raw_os_error(error as _));
            }
        }
    }

    #[allow(unused_variables)]
    fn wait_for_io_start(&self, file_name: Option<&str>, read: bool) {
        let ctx = self.task_system.thread();

        #[cfg(any(feature = "profiling", feature = "tracing"))]
        let task_name = ctx.task_name_or_unnamed();

        #[cfg(feature = "tracing")]
        {
            let s = format!(
                "[Wait start] For file {}: `{}` (cur. task: `{}`, thread: `{}`, fiber: `{}`)",
                file_name.unwrap_or("<unnamed>"),
                if read { "read" } else { "write" },
                task_name,
                ctx.name_or_unnamed(),
                ctx.fiber_name_or_unnamed()
            );
            trace!("{}", s);
        }

        #[cfg(feature = "profiling")]
        if !ctx.is_main_task() {
            println!("<<< end_cpu_sample `{}` (pre-wait)", task_name);
            self.task_system.profiler_end_scope();
        }
    }

    #[allow(unused_variables)]
    fn wait_for_io_end(&self, file_name: Option<&str>, read: bool) {
        let ctx = self.task_system.thread();

        #[cfg(any(feature = "profiling", feature = "tracing"))]
        let task_name = ctx.task_name_or_unnamed();

        #[cfg(feature = "tracing")]
        {
            let s = format!(
                "[Wait end] For file {}: `{}` (cur. task: `{}`, thread: `{}`, fiber: `{}`)",
                if read { "read" } else { "write" },
                file_name.unwrap_or("<unnamed>"),
                task_name,
                ctx.name_or_unnamed(),
                ctx.fiber_name_or_unnamed()
            );
            trace!("{}", s);
        }

        #[cfg(feature = "profiling")]
        if !ctx.is_main_task() {
            println!(">>> begin_cpu_sample `{}` (post-wait)", task_name);
            self.task_system.profiler_begin_scope(task_name);
        }
    }

    #[allow(unused_variables)]
    fn new(file: fs::File, task_system: &'ts TaskSystem, file_name: String) -> Self {
        Self {
            file,
            task_system,

            #[cfg(feature = "tracing")]
            file_name,
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

/// See [`std::fs::OpenOptions`](https://doc.rust-lang.org/std/fs/struct.OpenOptions.html).
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
    pub fn open<'ts, P: AsRef<Path>>(
        &self,
        task_system: &'ts TaskSystem,
        path: P,
    ) -> io::Result<File<'ts>> {
        let file_name = String::from(path.as_ref().to_str().unwrap());

        let file = self.0.open(path)?;

        // Do not notify the IOCP if the operation actually completed synchronously.
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

/// A yield operation representing a wait for async IO completion.
struct WaitForIOData<'h>(&'h IOHandle);

impl<'h> YieldData for WaitForIOData<'h> {
    /// The address on the stack of the `OVERLAPPED` / `IOHandle` struct, associated with the waited-on async IO
    /// is a perfect `YieldKey` candidate for a wait on the IO handle.
    fn yield_key(&self) -> YieldKey {
        unsafe { NonZeroUsize::new_unchecked(self.0 as *const _ as _) }
    }

    /// The wait on the IO handle is complete when its `ready_flag` is set by the IOCP thread.
    fn is_complete(&self) -> bool {
        self.0.ready_flag.load(Ordering::Acquire)
    }
}
