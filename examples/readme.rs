use std::mem;
use std::sync::atomic::{AtomicU32, Ordering};

#[cfg(feature = "asyncio")]
use std::fs;

#[cfg(feature = "tracing")]
use std::{io::Write, sync::Once};

#[cfg(feature = "tracing")]
extern crate log;

#[cfg(feature = "tracing")]
extern crate env_logger;

use minits;

#[cfg(feature = "tracing")]
fn setup_logger() {
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        let mut builder = env_logger::Builder::new();

        builder
            .format(|buf, record| writeln!(buf, "{}", record.args()))
            .filter(None, log::LevelFilter::Trace)
            .init();
    });
}

fn main() {
    #[cfg(feature = "tracing")]
    setup_logger();

    // Default task system parameters:
    // spawn a worker thread per physical core less one,
    // 1 Mb of stack per worker thread/fiber,
    // 4 fibers per thread.
    //let params = minits::TaskSystemParams::default();

    // Equivalent to:

    let num_cores = minits::get_num_physical_cores().max(1);
    let num_worker_threads = num_cores - 1;
    let num_fibers = num_cores * 4;
    let allow_inline_tasks = true;
    let fiber_stack_size = 1024 * 1024;

    let params = minits::TaskSystemParams::new(
        num_worker_threads,
        num_fibers,
        allow_inline_tasks,
        fiber_stack_size,
    );

    // Initializes the global task system singleton.
    // Call once before the task system needs to be used,
    // e.g. at application startup.
    // The task system may be used from the thread which
    // called `init_task_system` and from any spawned tasks.
    minits::init_task_system(params);

    {
        // This value will be borrowed by tasks immutably.
        let immutable_borrow = "I'm an immutably borrowed string.";

        // These values will be borrowed by tasks mutably.
        let mut mutable_borrow = false;
        let mut another_mutable_borrow = "Hello".to_owned();

        // Creates a `TaskScope` object through which tasks may be added to the task system.
        // Any borrows must live longer than this scope.
        // This means that either
        // 1) all borrows must be declared above the `scope`, as they are here, or
        // 2) scope must be explicitly dropped before any of the borrows are.
        // Scope names require "task_names" feature.
        let handle = minits::task_system().handle();
        let mut scope = minits::task_system().scope_named(&handle, "My task scope");

        // This adds a task to the task system.
        // A task is just a closure that takes a `&TaskSystem` argument and returns nothing.
        // It may run in any of the worker threads at any point during
        // or after the call to this function.
        scope.task_named(
            "My task", // Task names require "task_names" feature.
            |task_system| {
                // You can use the `task_system` to spawn nested tasks
                // (or use the global singleton if you initialized it).

                // This code will run in the worker thread.

                // You may access the task and scope names within the task body
                // (if "task_names" feature is enabled).
                #[cfg(feature = "task_names")]
                {
                    let scope_name = task_system.scope_name().unwrap();
                    assert_eq!(scope_name, "My task scope");
                    let task_name = task_system.task_name().unwrap();
                    assert_eq!(task_name, "My task");
                }

                // You may use immutable borrows of values on the spawning stack.
                assert_eq!(immutable_borrow, "I'm an immutably borrowed string.");

                // You may use mutable borrows as well, subject to standard Rust rules.
                assert!(!mutable_borrow);
                mutable_borrow = true;

                // You may spawn tasks from within tasks.
                let handle = task_system.handle();
                let mut scope = task_system.scope_named(&handle, "Nested task scope");

                scope.task_named("Nested task", |_task_system| {
                    // More nested tasks.
                    #[cfg(feature = "task_names")]
                    {
                        let scope_name = task_system.scope_name().unwrap();
                        assert_eq!(scope_name, "Nested task scope");
                        let task_name = task_system.task_name().unwrap();
                        assert_eq!(task_name, "Nested task");
                    }

                    // Same as above.
                    assert_eq!(immutable_borrow, "I'm an immutably borrowed string.");

                    // Same as above - only one mutable borrow allowed.
                    another_mutable_borrow.push_str(" world!");
                });

                // Works with function pointers too, but passing arguments is harder.
                fn simple_task(_task_system: &minits::TaskSystem) {
                    println!("Hi, I'm a simple function.");
                }

                scope.task(simple_task);

                // The current task completes when its closure returns, which
                // includes waiting for all nested tasks to complete.
            },
        );

        // Parallel-for example:
        let sum_parallel = AtomicU32::new(0);

        scope.task_range_named(
            "My very own parallel-for",
            // The range to split.
            0..32,
            // Split into `1` * `num_threads` chunks.
            // More chunks means better load-balancing, but more
            // task system overhead.
            1,
            // Function to call on each range chunk.
            // Takes a `range` argument which corresponds to the invocation's chunk.
            // The closure must be clonable because it IS cloned to each chunk.
            |range, _| {
                // Second argument is `&TaskSystem`.
                let local_sum = range.fold(0, |sum, val| sum + val);
                sum_parallel.fetch_add(local_sum, Ordering::SeqCst);
            },
        );
        // Implementation uses recursive range subdivision with forking
        // to achieve maximum CPU utilisation

        // When `scope` goes out of scope, this thread will wait for all tasks
        // associated with it to complete.

        // Rust does not allow us to use any values borrowed by the tasks above
        // until the tasks comlete.

        // Do this in order to explicitly wait for the tasks above to finish:
        mem::drop(scope);
        // Now the tasks above are guranteed to be complete, and we may use the
        // borrowed values again.

        // The tasks have modified the values on the stack above.
        assert!(mutable_borrow);
        assert_eq!(another_mutable_borrow, "Hello world!");

        let sum_serial = (0..32).fold(0, |sum, val| sum + val);
        assert_eq!(sum_parallel.load(Ordering::SeqCst), sum_serial);
    }

    // This feature enables async file IO support.
    #[cfg(feature = "asyncio")]
    {
        let file_name = "test.txt";

        // Use `minits::File::create` instead of `std::fs::File::create`,
        // pass the task system reference as the first argument.
        // Everything else is the same.
        // You can also use `minits::OpenOptions` same way as `std::fs::OpenOptions`.

        // Let's create a new file in the working directory.
        let file = minits::File::create(minits::task_system(), file_name).unwrap();

        let write_data = b"asdf 1234";

        // Write some data. The current task (/fiber, if any) will yield if the
        // operation does not complete synchronously.
        match file.write(write_data) {
            Ok(bytes_written) => {
                assert_eq!(bytes_written, write_data.len());
            }
            Err(err) => {
                panic!("Write failed: {}", err);
            }
        }

        // Let's open the same file for reading.
        // Previous file closed on drop, same as `std::fs::File`.
        let file = minits::File::open(minits::task_system(), file_name).unwrap();

        let mut read_data = [0u8; 9];

        // Read `read_data.len()` bytes from the file start.
        match file.read(&mut read_data) {
            Ok(bytes_read) => {
                assert_eq!(bytes_read, write_data.len());
                assert_eq!(read_data, *write_data);
            }
            Err(err) => {
                panic!("Read failed: {}", err);
            }
        }

        // Read the entire file contents into a `Vec::<u8>`.
        match file.read_all() {
            Ok(bytes_read) => {
                assert_eq!(bytes_read.len(), write_data.len());
                assert_eq!(bytes_read, *write_data);
            }
            Err(err) => {
                panic!("Read failed: {}", err);
            }
        }

        fs::remove_file(file_name).unwrap();
    }

    // Call once after the task system is no longer necessary,
    // e.g. before application shutdown.
    // Implicitly waits for all tasks to complete.
    // Call from the same thread which called `init_task_system` above.
    minits::fini_task_system();
    // Do not use the task system past this point.
}
