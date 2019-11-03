# minits

A minimalistic task (a.k.a job) system written in Rust.

Created as a learning project by someone who doesn't speak Rust. No guarantees with regards to safety. Bugs, undefined behaviour and unidiomatic code abound.

Not the first project inspired by the excellent [`Parallelizing the Naughty Dog engine using fibers`](http://twvideo01.ubm-us.net/o1/vault/gdc2015/presentations/Gyrling_Christian_Parallelizing_The_Naughty.pdf) talk by Naughty Dog's Christian Gyrling (also see the [`talk video`](https://www.gdcvault.com/play/1022186/Parallelizing-the-Naughty-Dog-Engine)).

## Building

Windows only.

Requires some path dependencies in the parent directory - see `Dependencies` section.

## Usage

See `examples\readme.rs`.

---
```rust
use std::mem;
use std::sync::atomic::{ AtomicU32, Ordering };

#[cfg(feature = "asyncio")]
use std::fs;

use minits;

fn main() {
    // Default task system parameters:
    // spawn a worker thread per physical core less one,
    // 1 Mb of stack per worker thread/fiber,
    // 4 fibers per thread,
    // allow inline tasks.

    //let builder = minits::TaskSystemBuilder::new();

    // Equivalent to:

    let num_cores = minits::get_num_physical_cores().max(1);
    let num_worker_threads = num_cores - 1;
    let num_fibers = num_cores * 4;
    let allow_inline_tasks = true;
    let fiber_stack_size = 1024 * 1024;

    let builder = minits::TaskSystemBuilder::new()
        .num_worker_threads(num_worker_threads)
        .num_fibers(num_fibers)
        .allow_inline_tasks(allow_inline_tasks)
        .fiber_stack_size(fiber_stack_size);

    // These closures will be called in the spawned worker threads
    // before the first task is executed / after the last task is executed.
    // The closures are passed the worker thread index in range `1 ..= num_worker_threads`.

    // E.g. we may initialize some thread-local data.
    let mut thread_local = ThreadLocal::<usize>::new();

    let thread_init = move |thread_index: usize| {
        println!("Hi from thread {}!", thread_index);

        thread_local.store(thread_index);
    };

    let thread_fini = move |thread_index: usize| {
        println!("Bye from thread {}!", thread_index);

        assert_eq!(thread_local.take(), thread_index);
    };

    let builder = builder
        .thread_init(thread_init)
        .thread_fini(thread_fini);

    // Initializes the global task system singleton.
    // Call once before the task system needs to be used,
    // e.g. at application startup.
    // The task system may be used from the thread which
    // called `init_task_system` and from any spawned tasks.
    minits::init_task_system(builder);

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

    // Don't forget to clean up the `ThreadLocal` object.
    thread_local.free_index();
}
```
---

## Features

Most of these are just for debugging:

- "task_names" - task and task scope names are stored in each individual task as `String`'s. Useful for debugging. May decrease the amount of memory available for closure captures.

- "tracing" - uses the `log` crate to `trace!` task addition/execution/waiting/resuming/competion. Useful for debugging. Works with "task_names" above. Usual `log` crate integration rules apply - e.g., initialize `env_logger` at the start of your application to see the `trace!` output.

- "profiling" - enables the `Profiler` trait and the `TaskSystemParams::set_profiler()` method to allow installing a profiler trait object. Useful for debugging. Works with "task_names". See the documentation for `Profiler` for more info.

- "remotery" - requires "profiling". Provides an implementation of the `Profiler` trait for the `remotery` profiler (wrapped via `miniremotery`); used by `TaskSystemParams::default()`. See the documentation for more info.

Actual features:

- "asyncio" - Win32 only. Provides (almost drop-in) `std::fs::File`, `std::fs::OpenOptions` replacements which allow async reading/writing from within tasks which *look* like synchronous operations. Uses Win32 IO completion ports via `miniiocp`.

    Currently `File` does NOT inplement Rust's `Read` \ `Write` traits. Instead several methods are provided which do exactly what it says on the tin: `read` (read X bytes at offset 0), `read_at` (read X bytes at offset Y), `read_all` (read whole file), `read_all_at` (read till EOF from offset X), `write` (write X bytes at offset 0), `write_at` (write X bytes at offset Y).

- "graph". Requires `minigraph`. Provides an API to execute a dependency graph in topological order, with non-dependant tasks executing in parallel.

## Architecture overview

- An instance of the `TaskSystem` struct is created by the user. Safe to use from the "main" thread (in which it was created) and from within spawned tasks. A singleton interface is provided for convenience.
- Spawns some number of worker threads (minimum 0), allocates a pool of worker fibers (minimum 0). Both pools do not grow at runtime.
- 0 worker threads - single-threaded mode. Only the main thread executes tasks. Useful for debugging.
- 0 fibers - tasks are executed by the worker threads of the thread pool directly. Otherwise fibers are used to execute each task, enabling lightweight context-switching and independent individual task completion.
- `TaskHandle` + `TaskScope` combos[^1] may be created by the `TaskSystem` object.
- `TaskScope` provides safe methods to *add* tasks/task ranges (parallel-for loops) to the `TaskSystem`.
- Tasks are just closures which take a `TaskSystem` reference and have no outputs, but may reference or take ownership their environment (bounded by the `TaskScope`'s lifetime) according to Rust borrow rules.
- When added to the system, the task may run in any of the worker threads at any point during or after the call to the function.
- When the `TaskScope` goes out of scope, all tasks associated with it (and its handle) are *waited* on, i.e. the thread blocks until all the tasks have completed. The thread might  execute the tasks itself if there are any available in the queue.
- There exists one global mutex-locked FIFO **task queue** for all worker threads (and the main thread). This might change in the future to use lock-free task-stealing queues.
- Tasks are pushed at one end when added to the system and picked up by worker/main threads from the other end. Threads do not busy-wait, but use the OS synchronisation primitives, so they might have to be *woken up* by the OS. This might or might not be desirable.
- Task ranges (a.k.a parallel-for) split the user task range in half, pushing the left half to the task queue for other threads to pick up and subdivide, and recursing on the right half until the range cannot be split anymore, as determined by number of threads / user range / user split multiplier.
- When the `TaskSystem` instance (or singleton) is finalized, it waits for all tasks to complete and cleans up all of its resources. It may be initialized again later if necessary.
- When "asyncio" feature is enabled, an additional FS thread is spawned whose only job is to block on an IO completion port, listening for IO operation completion on associated file handles, and resume any tasks yielded waiting for these operations.

## Problems / missing features

- Win32 only, but other platform support is possible via existing crates. See design notes for more info.
- Clunky `TaskHandle` usage related to how stack values are dropped in Rust. No idea how to fix without `Box`'ing everything.
- Fixed (and configurable) limit on closure memory footprint and an unergonomic way to report insufficient buffer size - runtime error in `debug` (compilation error would be ideal but seems to be impossible to produce due to how Rust generics work vs. C++ generics), compilation error in `release` with a very long and highly unhelpful message. No idea how to fix without `Box`'ing everything. Current proposed solution - `Box` when too large.
- May not borrow values declared *after* the `TaskScope` is created - problem or not?
- No task affinities (yet). Well, there *does* exist the concept of main thread tasks, but it is not exposed in the public API.
- No task priorities.
- No task stack size requirements.
- No task groups (yet) for coarser level synchronisation.
- ???

## Dependencies

- [`winapi`](https://docs.rs/winapi/0.3.8/winapi/) via crates.io.
- If "tracing" feature is enabled, [`log`](https://docs.rs/log/0.4.8/log/) via crates.io.
- Win32 primitive wrappers via [`minifiber`](https://github.com/alex05447/minifiber), [`minievent`](https://github.com/alex05447/minievent), [`minithreadlocal`](https://github.com/alex05447/minithreadlocal) as path dependencies (TODO - github dependency?).
- If "remotery" feature is enabled, `remotery` wrapper via [`miniremotery`](`https://github.com/alex05447/miniremotery`) as a path dependency (TODO - github dependency?).
- If "asyncio" feature is enabled, Win32 IO completion port wrapper via [`miniiocp`](https://github.com/alex05447/miniiocp) as a path dependency (TODO - github dependency?).
- If "graph" feature is enabled, [`minigraph`](https://github.com/alex05447/minigraph) as a path dependency (TODO - github dependency?).

[^1]: See design notes for why the `TaskHandle` is necessary. Might change in the future.