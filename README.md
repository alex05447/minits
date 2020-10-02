# minits

A minimalistic task (a.k.a job) system written in Rust.

Created as a learning project. No guarantees with regards to safety. Bugs, undefined behaviour and unidiomatic code abound.

Not the first project inspired by the excellent [`Parallelizing the Naughty Dog engine using fibers`](http://twvideo01.ubm-us.net/o1/vault/gdc2015/presentations/Gyrling_Christian_Parallelizing_The_Naughty.pdf) talk by Naughty Dog's Christian Gyrling (also see the [`talk video`](https://www.gdcvault.com/play/1022186/Parallelizing-the-Naughty-Dog-Engine)).

## Building

Windows only.

Requires some path dependencies in the parent directory - see `Dependencies` section.

## Usage

See `examples\readme.rs`.

---
```rust
use {
    minithreadlocal::ThreadLocal,
    minits,
    std::{
        mem,
        sync::atomic::{AtomicU32, Ordering},
    },
};

#[cfg(feature = "asyncio")]
use std::fs;

fn main() {
    // Default task system parameters:
    //
    // spawn a worker thread per physical core less one,
    // 1 Mb of stack per worker thread/fiber,
    // 4 fibers per thread,
    // allow inline tasks.

    // let builder = minits::TaskSystemBuilder::new();

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
    let mut thread_local = ThreadLocal::<u32>::new();

    let thread_init = move |thread_index: u32| {
        println!("Hi from thread {}!", thread_index);

        thread_local.store(thread_index);
    };

    let thread_fini = move |thread_index: u32| {
        println!("Bye from thread {}!", thread_index);

        assert_eq!(thread_local.take().unwrap().unwrap(), thread_index);
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

        // Capturing a large object by value will require a dynamic memory allocation,
        // but smaller closures won't have this overhead.
        let big_capture = [7u8; 64];

        // Creates a `Scope` object through which tasks may be added to the task system.
        // Any borrows must live longer than this scope.
        // This means that either
        // 1) all borrows must be declared above the `scope`, as they are here, or
        // 2) scope must be explicitly dropped before any of the borrows are.
        // Scope names require "task_names" feature.
        minits::task_scope_named!(scope, "My task scope".to_owned());

        // This adds a task to the task system.
        // A task is just a closure that takes a `&TaskSystem` argument and returns nothing.
        // It may run in any of the worker threads at any point during
        // or after the call to this function.
        scope
            .task(|ts| {
                // You can use the `ts` to spawn nested tasks
                // (or use the global singleton if you initialized it).

                // This code will run in the worker thread.

                // You may access the task and scope names within the task body
                // (if "task_names" feature is enabled).
                if cfg!(feature = "task_names") {
                    assert_eq!(ts.scope_name().unwrap(), "My task scope");
                    assert_eq!(ts.task_name().unwrap(), "My task");
                }

                // You may use immutable borrows of values on the spawning stack.
                assert_eq!(immutable_borrow, "I'm an immutably borrowed string.");

                // You may use mutable borrows as well, subject to standard Rust rules.
                assert!(!mutable_borrow);
                mutable_borrow = true;

                // You may spawn tasks from within tasks using nested `Scope`'s.
                minits::task_scope_named!(scope, "Nested task scope".to_owned(), ts);

                scope
                    .task(|ts| {
                        // More nested tasks.
                        if cfg!(feature = "task_names") {
                            assert_eq!(ts.scope_name().unwrap(), "Nested task scope");
                            assert_eq!(ts.task_name().unwrap(), "Nested task");
                        }

                        // Same as above.
                        assert_eq!(immutable_borrow, "I'm an immutably borrowed string.");

                        // Same as above - only one mutable borrow allowed.
                        another_mutable_borrow.push_str(" world!");

                        // Spawn another nested task which will panic.
                        minits::task_scope_named!(
                            scope,
                            "Panicking task scope".to_owned(),
                            ts
                        );

                        scope.task(|_| {
                            panic!("foo");
                        });

                        // Consumes the scope, waiting for the associated task to complete
                        // and returning any panics (only one panic in this case).
                        // If we would just drop the scope, it would print the caught panic in the task
                        // and re-throw it, panicking in our application.
                        if let Err(panics) = scope.wait() {
                            println!("{}", panics);
                        }
                    })
                    .name("Nested task");

                // Works with function pointers too, but passing arguments is harder.
                fn simple_task(_: &minits::TaskSystem) {
                    println!("Hi, I'm a simple function.");
                }

                scope.task(simple_task);

                scope.task(move |_| {
                    println!("Task with a large ({}b) capture.", big_capture.len());
                });

                // The current task completes when its closure returns, which
                // includes waiting for all nested tasks to complete.
            })
            .name("My task"); // Task names require "task_names" feature.

        // Parallel-for example:
        let sum_parallel = AtomicU32::new(0);

        scope
            .task_range(
                // The range to split.
                0..32,
                // Function to call on each range chunk.
                // Takes a `range` argument which corresponds to the invocation's chunk.
                // The closure must be clonable because it IS cloned to each chunk.
                // Second argument is `&TaskSystem`.
                |range, _| {
                    let local_sum = range.fold(0, |sum, val| sum + val);
                    sum_parallel.fetch_add(local_sum, Ordering::SeqCst);
                },
            )
            // Split into `1` * `num_threads` chunks.
            // More chunks means better load-balancing, but more
            // task system overhead.
            .fork_method(minits::ForkMethod::ChunksPerThread(1))
            .name("My very own parallel-for");

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

        // Write some data. The current task (or fiber, if any) will yield if the
        // operation does not complete synchronously.
        match file.write(write_data) {
            Ok(bytes_written) => {
                assert_eq!(bytes_written, write_data.len());
            }
            Err(err) => {
                panic!("async write failed: {}", err);
            }
        }

        // Let's open the same file for reading.
        // Previous file closed on drop, same as `std::fs::File`.
        let file = minits::File::open(minits::task_system(), file_name).unwrap();

        let mut read_data = [0u8; 9];
        assert_eq!(read_data.len(), write_data.len());

        // Read `read_data.len()` bytes from the file start.
        match file.read(&mut read_data) {
            Ok(bytes_read) => {
                assert_eq!(bytes_read, write_data.len());
                assert_eq!(read_data, *write_data);
            }
            Err(err) => {
                panic!("async read failed: {}", err);
            }
        }

        // Read the entire file contents into a `Vec::<u8>`.
        match file.read_all() {
            Ok(bytes_read) => {
                assert_eq!(bytes_read.len(), write_data.len());
                assert_eq!(bytes_read, *write_data);
            }
            Err(err) => {
                panic!("async read failed: {}", err);
            }
        }

        // Clean up the file.
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

    // The task system may be initialized again, if so desired, with any parameters.
}
```
---

## Features

Most of these are just for debugging:

- `"task_names"` - task and task scope names are stored in each individual task. Useful for logging / debugging. May decrease the amount of memory available for closure captures.

- `"logging"` - uses the `log` crate to `trace!` task addition/execution/waiting/resuming/competion. Useful for debugging. Works with `"task_names"` above. Usual `log` crate integration rules apply - e.g., initialize `env_logger` at the start of your application to see the `trace!` output.

- `"profiling"` - enables the `Profiler` trait and the `TaskSystemBuilder::set_profiler()` method to allow installing a profiler trait object. Useful for debugging. Works with `"task_names"`. See the documentation for `Profiler` for more info.

- `"remotery"` - requires `"profiling"`. Provides an implementation of the `Profiler` trait for the `remotery` profiler (wrapped via [`miniremotery`](https://github.com/alex05447/miniremotery)); used by `TaskSystemBuilder::default()`. See the documentation for more info.

Actual features:

- `"asyncio"` - Win32 only. Provides (almost drop-in) `std::fs::File`, `std::fs::OpenOptions` replacements which allow async reading/writing from within tasks which *look* like synchronous operations. Uses Win32 IO completion ports via [`miniiocp`](https://github.com/alex05447/miniiocp).

    Currently `File` does NOT inplement Rust's `Read` \ `Write` traits. Instead several methods are provided which do exactly what it says on the tin: `read` (read X bytes at offset 0), `read_at` (read X bytes at offset Y), `read_all` (read whole file), `read_all_at` (read till EOF from offset X), `write` (write X bytes at offset 0), `write_at` (write X bytes at offset Y).

- `"graph"`. Requires [`minigraph`](https://github.com/alex05447/minigraph). Provides an API to execute a dependency graph in topological order, with non-dependant tasks executing in parallel.

## Architecture overview

- An instance of the `TaskSystem` struct is created by the user. Safe to use from the "main" thread (in which it was created) and from within spawned tasks. A singleton interface is provided for convenience.
- Spawns some number of worker threads (minimum `0`), allocates a pool of worker fibers (minimum `0`). Both pools do not grow at runtime.
- `0` worker threads - single-threaded mode. Only the main thread executes tasks. Useful for debugging.
- `0` fibers - tasks are executed by the worker threads of the thread pool directly. Otherwise fibers are used to execute each task, enabling lightweight context-switching and independent individual task completion.
- `Handle` + `Scope` combos[^1] may be created by the `TaskSystem` object, or via a helper macro.
- `Scope` provides safe methods to *add* tasks/task ranges (parallel-for loops)/task graphs to the `TaskSystem`.
- Tasks are just closures which take a `TaskSystem` reference (and task range for tange tasks, slice element for slice tasks, graph vertex payload for graph tasks) and have no outputs, but may reference or take ownership their environment (bounded by the `Scope`'s lifetime) according to Rust borrow rules.
- When added to the system, the task may run in any of the worker threads at any point during or after the call to the function (unless the task was marked as a "main" thread task - these only run on the "main" thread).
- When the `Scope` goes out of scope, all tasks associated with it (and its handle) are *waited* on, i.e. the thread blocks until all the tasks have completed. The waiting thread might execute the tasks itself if there are any available in the queue.
- If any of the tasks associated with a `Scope` (or any nested child tasks) panic, waiting on a `Scope` returns a list of all such panics. It's up to the caller to handle (or ignore) them.
- When fibers are used, thread waiting for a `Scope` or async IO completion is implemented via **fiber yielding** / switching / resuming on dependency completion.
- There exists one global mutex-locked FIFO **task queue** for all worker threads (and the main thread). This might change in the future to use lock-free task-stealing queues.
- Tasks are pushed at one end when added to the system and picked up by worker/main threads from the other end. Threads do not busy-wait, but use the OS synchronisation primitives, so they might have to be *woken up* by the OS. This might or might not be desirable.
- Task ranges (a.k.a parallel-for) split the user task range in half, pushing the left half to the task queue for other threads to pick up and subdivide, and recursing on the right half until the range cannot be split anymore, as determined by number of threads / user range / user split multiplier.
- When the `TaskSystem` instance (or singleton) is finalized, it waits for all tasks to complete and cleans up all of its resources. It may be initialized again later if necessary.
- When `"asyncio"` feature is enabled, an additional FS thread is spawned whose only job is to block on an IO completion port, listening for IO operation completion on associated file handles, and to resume any tasks yielded waiting for these operations.

## Problems / missing features

- Win32 only, but other platform support is possible via existing crates. See design notes for more info.
- Clunky `Handle` usage related to how stack values are dropped in Rust. No idea how to fix without `Box`'ing everything. Currently using a helper macro similar to `stackpin` to create a shadow a `Handle` on the stack.
- No task affinities (yet). Well, there *does* exist the concept of "main" thread tasks, but it is not exposed in the public API.
- No task priorities.
- No task stack size requirements.
- No task groups (yet) for coarser level synchronisation.
- ???

## Dependencies

- [`winapi`](https://docs.rs/winapi/*/winapi/) via crates.io.
- If `"logging"` feature is enabled, [`log`](https://docs.rs/log/*/log/) via crates.io.
- Win32 primitive wrappers via [`minifiber`](https://github.com/alex05447/minifiber), [`minithreadlocal`](https://github.com/alex05447/minithreadlocal) as path dependencies (TODO - github dependency?).
- [`miniclosure`](https://github.com/alex05447/miniclosure) as a path dependency - used to represent task closures and their captures with no dynamic memory allocation when the closure occupies less than X bytes, where X ~ 56b.
- If `"remotery"` feature is enabled, `remotery` wrapper via [`miniremotery`](https://github.com/alex05447/miniremotery) as a path dependency (TODO - github dependency?).
- If `"asyncio"` feature is enabled, Win32 IO completion port wrapper via [`miniiocp`](https://github.com/alex05447/miniiocp) as a path dependency (TODO - github dependency?).
- If `"graph"` feature is enabled, [`minigraph`](https://github.com/alex05447/minigraph) as a path dependency (TODO - github dependency?).

[^1]: See design notes for why the `Handle` is necessary. Might change in the future.