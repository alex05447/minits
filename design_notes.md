# **Task system design in Rust**

Implementing a task system from scratch and learning Rust in the process.

## **Motivation**

We like paralellizing computationally intensive tasks/jobs/whatever you want to call them to take full use of all available CPU cores. We want to execute arbitrary [^1] functions/closures a.k.a tasks in parallel with some explicit synchronization betwen them. We don't care, at least initially, about support for file system/network IO or other blocking operations in these tasks. It's just some code that needs to read some data, do some calculations and write some data out.

A commonly seen "parallel for" loop may be implemented on top of the task system. Executing a graph of ECS systems with optional producer-consumer dependencies between them is a good example. Even async IO systems may be built on top of a task system, allowing one to write serial-looking code that is actually async, with no language support (a.k.a async/await) required. Obviously this requires one to use library-provided primitives for reading/writing files/sockets.

Plus, implemeting something as complex as a task system requires one to use almost every aspect of the language, including its darkest corners, and allows one to appreciate (or be disappointed by) its features and their interaction with the general design of the library.

## **Requirements**

These will be our requirements for the task system:

- **Closure captures**: must support closures with captures: `&`, `&mut`, `move`, including arbitrary lifetimes, safely. We want the compiler to produce an error if we dangle a reference to a local borrow. We don't want to be limited to `'static` closures [^2].

- **Memory**: closures shold be *unboxed*. We'd like to avoid dynamic memory allocation as much as possible.

- **Synchronisation**. Ideally we want to spawn multiple tasks and synchronize (wait for) with each individual task / all of them.

- **Task dependencies**. Tasks within tasks must be supported with no extra api clutter `.and_then(...)`-style.

- **Performance**. Must support lots of small tasks with minimal overhead.

- **Simplicity**. Api must be as simple and unobtrusive as possible. We just want to run a function, not to build a `future`-style state machine. No macros or attributes.

- **(Optional) Portability**. It sure would be nice if the task system would work out of the box on Win/Linux/Mac/whatever Rust supports. Win32 takes precedence, simply because that's the platform I'm familiar with.

### **Closure captures**

- `move || { \*code*\}` - all used environment variables are moved into the closure body. Closure has full ownership, no borrow issues.
- `|| { \*code*\}` - borrows local variables. Most useful case. Safe Rust does not allow the closure to live longer than the shortest living borrowed variable. It also doesn't allow multiple mutable references to one variable[^3].
- only captures `'static` variables / simple function pointer - simplest case, but may be less useful due to inability to directly communicate results. No borrow issues.

    There exists an *unsafe* [`std::thread::Builder::spawn_unchecked`], which requires the caller to ensure that the thread handle is joined before any borrowed variables go out of scope. Both api's internally box the closure.

    There also exists [`crossbeam::Scope::spawn`] which allows safe local borrows. However, the api is convoluted, and the closure is boxed.

### **Memory**

We *can* bend Rust safety rules and store *any* closure unboxed, with captures or not, for later execution with unsafe code (explicitly or implicitly) a-la C++[^4]. However, we'd like to benefit from Rust's compile time safety guarantees.

### **Return values**

Closures may return their results directly via mutable borrows, via moved variables through interior mutability, or via implicit back channels.

### **Performance**

Handling lots of small tasks efficiently usually implies 1) no or minimal dynamic memory allocations and 2) minimal thread contention on task system data structures.

### **Simplicity**

We'd prefer not to have to pass some `TaskSystem` object to each function/method that wants to spawn tasks, or at least to have an option not to have to do so. If used as part of a framework, the task system may be considered a core piece of functionality akin to a logger and it's desirable to be able to use it globally.

Singletons are tricky in Rust, especially considering we need *explicit* `initialize()` / `finalize()`, and the fact that the task system is inherently multithreaded.

### **Fibers or not**

Fiber pros:
- allows tasks in separate fibers to complete independently, preventing the problem with thread-pool based task systems where waiting tasks may be "buried" deep on the stack until all the tasks above complete, even though the original task might have had all of its dependencies fulfilled.
- low-cost context switch (as opposed to "parking"/"waiting" the thread on a synchronisation primitive (event/semaphore/thread handle), if such synchronisation mechanism is even used).
- async IO code may be written like sync code. When waiting for file/socket/whatever IO via the task-system provided API is implemented as a fiber switch, resumed on IO completion, the code looks exactly like the synchronous, thread-blocking version.

Fiber cons:
- memory use for fiber stacks.
- low-cost context switch (as opposed to no context switch at all when waiting for task completion is effectively a busywait).
- ???

Thankfully, implementing both options at once should not be particularly more difficult than any one of them, allowing e.g. a hybrid scenario where we use a small-ish fiber pool, falling back to classic thread pool execution model when we run out of fibers.

Plus fibers are cool.

### **Portability**
1. **Fibers**. Win32 (and XBOX?) has a **very** simple [fiber API](https://docs.microsoft.com/en-us/windows/win32/procthread/fibers), which refelcts the very simple functionality we actually need: 1) create fiber with X Mb of stack, 2) convert thread to fiber, 3) switch to fiber, 4) delete fiber. That's all (other than a certain fiber-related function is implemented as a compiler intrinsic in MSVC and must be reimplemented in assembly, which was a PITA, but also a learning experience).

    For other platforms, including Win32, [context](https://docs.rs/crate/context) exists, but its portability is achieved at the cost of having a much larger amount of custom platform/ABI-specific register-twiddling assembly code that I'm comfortable with. Apparently, Linux/Max does not have a native fiber API. PS4 does have one AFAIK.

    Bottom line - implement for Win32. If so desired, support for other platforms *should* be easy to add, where usual caveats apply to "easy".

2. **Synchronisation primitives**. Standard Rust does not have a waitable/signallable semaphore or an event (which came as a surprise coming from C++/Win32 background). I didn't research further the reasons for this design decision. [parking_lot](https://docs.rs/parking_lot/0.9.0/parking_lot/) / [rsevents](https://docs.rs/rsevents/0.2.0/rsevents/) / [semaphore](https://docs.rs/semaphore/0.4.0/semaphore/) seem to be the portable ways to add the missing functionality. However, `parking_lot` does not seem to use native Win32 primitives under the hood, choosing to reimplement its abstractions.

    Bottom line - implement for Win32. Native auto reset events / semaphores are a perfect fit for our needs.

    Rust `std::sync::Mutex` does seem to use a critical section on Win32, which is good, but does have some poisoning-related overhead, which is less good. A critical section wrapper with a `Mutex` API is trivial to implement and is a good excercise.

    Bottom line - use `std::sync::Mutex` just for the sake of staying as portable as possible.

3. **Async IO**. Win32 has the nice concept of IO completion ports, which make writing async IO handling code in a fiber-based task system pretty straightforward.

    I have no idea if there's a direct equivalent on other OS's, but it appears there isn't (?). Just make it Windows exclusive for now.


## **Use cases**

1. No borrows (`'static`, `move` closure, `fn()` pointer). No wait (fire and forget).
    ```rust
    let ts = TaskSystem::new();

    {
        let f = || -> () {
            /* ... */
        };

        ts.add_task(f);

        // We go on, `f()` might or might not complete as far as we know,
        // unless we do:
        ts.wait_for_all();
    }
    ```

2. No borrows. Explicit wait.
    ```rust
    let ts = TaskSystem::new();

    {
        let h = TaskHandle::new();

        let f = || -> () {
            /* ... */
        };

        ts.add_task(&h, f); // Task 1.

        // Do something else for a while.

        // We may even add another task to the same handle:
        ts.add_task(&h, || {/* ... */} ); // Task 2.

        // Wait:
        ts.wait_for_handle(&h); // Waits for both tasks above.
        // Or:
        ts.wait_for_all(); // Waits for all tasks, including both tasks above.

        // Both taks 1 and 2 are guaranteed to have completed by now.
    }
    ```

3. (Does not work) Local borrow. No wait.
    ```rust
    let ts = TaskSystem::new();

    {
        let local = /* ... */;

        let f = || -> () {
            use_borrowed(&local);
            /* ... */
        };

        // Task is queued for execution but will not complete until later.
        ts.add_task(f);
    }
    // ERROR: `local` does not live long enough.
    ```

    In code above, we want to get a compilation error because `f()` might outlive `local`.
    There's no safe way to have a fire-and-forget task with local borrows. We can bend the rules and force the above to compile with unsafe code within `add_task()`, but we will end up accessing garbage through `local`.

4. (Does not work) Local borrow. Explicit wait.
    ```rust
        let ts = TaskSystem::new();

        {
            let h = TaskHandle::new();

            let local = /* ... */;

            let f = || -> () {
                use_borrowed(&local);
                /* ... */
            };

            ts.add_task(&h, f);

            ts.wait_for_handle(&h);
            // Or:
            ts.wait_for_all();

            // `f()` is guaranteed to have completed by now.
        }
        // ERROR: `local` does not live long enough.
    ```
    Technically, the code above is safe (as in we'll never access garbage via `local`) - `f()` is guaranteed to complete before `local` goes out of scope. However, Rust has no way of knowing that and will complain that `local` does not live long enough.

5. (Unsafe) Local borrow. Explicit wait. Making it compile with unsafe code.
    ```rust
        let ts = TaskSystem::new();

        {
            let h = TaskHandle::new();

            let local = /* ... */;

            let f = || -> () {
                use_borrowed(&local);
                /* ... */
            };

            unsafe { ts.add_task_unsafe(&h, f); }

            // Wait:
            ts.wait_for_handle(&h);
            // Or:
            ts.wait_for_all();

            // `f()` is guaranteed to have completed by now.
        }

        // UNSAFE - if we move the wait here from above, the code will
        // still compile, but now we might access garbage in `local` and `h`.
        ts.wait_for_all();
    ```
    `add_task_unsafe()` is annotated `unsafe` and will take a reference to `h` and `f()` with *any* lifetime. It's possible to make the code above access dangling references by (re)moving the wait call. It's a start, but we'd like to do better.

6. Local borrow. Trivial blocking task spawn.
    ```rust
        let ts = TaskSystem::new();

        {
            let local = /* ... */;

            let f = || -> () {
                use_borrowed(&local);
                /* ... */
            };

            ts.add_task_and_wait(f);
            // Equivalent to:
            f();

            // `f()` is guaranteed to have completed by now.
        }
    ```
    When `add_task_and_wait()` returns, `f()` is guaranteed to have been executed, so the compiler is happy. Implementation might execute the closure inline. This does not look useful, however `f()` might spawn and wait for more tasks itself, creating a dependency graph of tasks.

7. Local borrow. Using task scopes.
    ```rust
        let ts = TaskSystem::new();

        {
            let s = ts.new_task_scope();

            let local = /* ... */;

            let f = || -> () {
                use_borrowed(&local);
                /* ... */
            };

            s.add_task(f); // Task 1.

            // We may even add another task to the same scope:
            s.add_task( || {/* ... */} ); // Task 2.

            // When `s` goes out of scope (or is explicitly dropped),
            // it waits for all tasks added to it.
            std::mem::drop(s);

            // Both taks 1 and 2 are guaranteed to have completed by now.
        }
    ```
    `TaskScope` implements the `Drop` trait and waits for all associated tasks to complete when it goes out of scope.

### **Summary**:

- We don't need to borrow any local variables or wait for task completion - use `TaskSystem::add_task<F>(f :F) where F : FnOnce() + 'static`.
    ```rust
    let ts = TaskSystem::new();
    let arg = 42;
    ts.add_task(/*move*/ || println!("Hello from a task. Arg is {}", arg));
    ```

- We need to borrow some local variables but want to wait for the task to complete - use `TaskSystem::add_task_and_wait(f :F) where F : FnOnce()`,
    ```rust
    let ts = TaskSystem::new();
    let arg = "arg".to_owned();
    let mut result = 0;
    ts.add_task_and_wait(|| {
        println!("Hello from a task. Arg is {}", arg);
        result = 42;
        });
    ```

    or `TaskSystem::new_task_scope() -> TaskScope` and `TaskScope::add_task(f :F) where F : FnOnce()`.

    ```rust
    let ts = TaskSystem::new();
    {
        let s = ts.new_task_scope();
        let arg = "arg".to_owned();
        let mut result = 0;
        s.add_task(|| {
            println!("Hello from a task. Arg is {}", arg);
            result = 42;
        });
    }
    ```

- We don't need to borrow any local variables but want to wait for the task to complete - use same as above, plus `TaskHandle::new_task_handle() -> TaskHandle`, `TaskSystem::add_task<F>(h :&TaskHandle, f :F) where F : FnOnce() + 'static`, `TaskSystem::wait_for_handle(h :&TaskHandle)`.
    ```rust
    let ts = TaskSystem::new();
    {
        let h = TaskHandle::new();
        let arg = 42;
        ts.add_task(&h, || println!("Hello from a task. Arg is {}", arg));
        ts.wait_for_handle(&h);
    }
    ```

## **Problems**

### TaskScope

So we want to go ahead and implement the `TaskScope`. The simplest implementation contains a task handle internally. When we add a task to the task system, we reference this handle. When we `drop()` the `TaskScope`, we `wait_for_handle()` on this handle.

So far so good. This is straightforward to do in C++. When destructors are called for objects on the stack, `this` pointer is the same as it was when the object was constructed. Which is great, because we need the address of/pointer/reference to the internal `TaskHandle` to be the same when we `add_task()` when the task scope is live, and when we `wait_for_handle()` when it is being dropped when it goes out of scope.

Unfortunately, when we translate this to Rust we immediately realize we have a problem:
```rust
struct TaskScope<'ts> {
    handle :TaskHandle,
    task_system :&'ts TaskSystem,
}

// Details omitted...

impl Drop for TaskScope {
    fn drop(&mut self) {
        // This would be just fine in C++ - the object is still on the stack.
        self.task_system.wait_for_handle(&self.task_handle);
    }
}

// ...

let ts = TaskSystem::new();
{
    let s = ts.new_task_scope();
    let arg = "arg".to_owned();
    let mut result = 0;
    s.add_task(|| {
        println!("Hello from a task. Arg is {}", arg);
        result = 42;
    });
    // `s` is dropped here.
    // This works fine - `s` is dropped in-place,
    // and the handle address is stable.

    // But if we do this:
    mem::drop(s);
    // ... there's a problem.
    // This is `mem::drop`:
    // `mem::drop<T>(_t :T) {}
    // The dropped value is MOVED into the function,
    // and the observed `self` address in the drop handler
    // is different to the address used in `add_task()`.
    // We have UB when we decrement the task handle counter
    // on task completion and hang waiting for an invalid counter.
    // Oops.
}
```

My quick research into this did not find any solutions, or even discussion of the topic.

The closest is `std::pin::Pin`, which *seems* to solve a completely different issue by documenting the immovable nature of some value, but does *nothing* by itself, similar to `Send` / `Sync` traits. If an object is `PinBox`'ed, it's guaranteed to have a stable address, by virtue of being allocated on the heap, and pinning it is a no-op. And where we'd like objects to be pinned - on the stack - we can't. Is it even possible to have `drop` not move the object? Maybe I'm missing something, but this is probably my biggest gripe with Rust right now.

So what are our options?
1) Leave things as they are and document that dropping a `TaskScope` prematurely leads to UB.
2) Add some API clutter and do something like this, with an explicit handle allocated on the stack above the call to `new_task_scope()`:
    ```rust
    let ts = TaskSystem::new();
    {
        let h = TaskHandle::new();
        // Explicitly reference a task handle on the stack.
        // Yeah, I really wish `h` could just be contained in `s`.
        let s = ts.new_task_scope(&h);
        let arg = "arg".to_owned();
        let mut result = 0;
        s.add_task(|| {
            println!("Hello from a task. Arg is {}", arg);
            result = 42;
        });

        // This works fine, `h` has a stable address for the entire scope,
        // as does an implicit drop.
        mem::drop(s);
    ```
    However this case behaves somewhat suboptimally:
    ```rust
    let ts = TaskSystem::new();
    let h = TaskHandle::new();
    {
        let s = ts.new_task_scope(&h);
        let arg = "arg".to_owned();
        let mut result = 0;
        s.add_task(|| {
            println!("Hello from a task. Arg is {}", arg);
            result = 42;
        });

        {
            // Share the handle of the parent scope.
            let s_nested = ts.new_task_scope(&h);
            s_nested.add_task(|| {
                println!("Hello from a nested task.");
                result = 42;
            });

            // At this point we'll wait for BOTH tasks to finish.
        }
        // Wait on drop here is a no-op - we have alreaady waited
        // for the shared handle in the nested scope.
    }
    ```
    This makes waiting less granular. The first wait on any of the scopes associated with the handle waits for all tasks associated with that handle through any of the scopes. This might or might not be desirable.

3) Allocate each task handle (inside each task scope or free) on the heap:
    ```rust
    impl TaskHandle {
        fn new() -> Box<TaskHandle> {
            // ...
        }
    }
    ```
    Oviously suboptimal when there's a large number of tasks.

4) Preallocate a pool of `Box<TaskHandle>` in the task system, locked in a `Mutex`.

    Better than above, but still bad.

5) Some sort of generational hanlde indexed array of counters in the task system.

    Pros: no individually heap-allocated atomic counters. Safe to copy around in case of some unexpected UB in the unsafe pointer-wrangling implementation.

    Cons: overhead - requires mutex locking + indirection on each access to the counter (inc on task added, dec on task finished, check on attempt to resume tasks on task finished). All task system operations - creating scopes, adding tasks, waiting for tasks, finishing and resuming tasks - are now serialized on this mutex. Yuck.

No.2 seems to be the best compromise, even though the "unnecessary" explicit handle clutter is really grating from a C++ user's prespective, followed by No.4.

## Context switching / yielding

Overview of fiber yielding mechanics.

**Yield** - an operation that stops the execution of the current task until it is explicitly resumed later. Usually implies **waiting** for some condition - like task(scope) completion or IO completion.

Usually implemented as a function call like `TaskSystem::yield_current_task(\*some_args*\)`. Must synchronise *externally* with the call to `TaskSystem::try_resume_task(\*some_args*\)` (i.e. it's up to the caller to ensure there's a 1-to-1 mapping between the calls) which **resumes** the task, logically returning from the `yield` call on its call stack. `some_args` may contain some opaque (e.g. `usize`) `yield_key`, unique per yielded task, used by `try_resume_task` code to identify the resumed task.

Current yield algorithm is as follows:

Some `thread_A` runs `task_A` in `fiber_A`. In its closure, `task_A` spawns a `task_B` using a `TaskHandle_B` (now incremented to `1`), then waits on `TaskHandle_B`.

Meanwhile in a parallel timeline some worker `thread_B` may pick up `task_B` in fiber `fiber_B`, run its closure until completion and decrement `task_B`'s task handle value by `1`. When the handle value reaches `0`, as it will in this case as we only have one task associated with it, `thread_B` will attempt to **resume** one or none tasks waiting on `TaskHandle_B`.

**Waiting**: `thread_A` checks wait completion - in this case compares `TaskHandle_B` value to `0`. If it's `0` already, we're done. If it's positive, `thread_A` locks the task system yield queue and checks wait completion again. If it's complete now, we're done - unlock the queue and move on. If it's still incomplete, we commit to **yielding** the current task and fiber. `thread_A` pushes a new entry to the yield queue.

```rust
enum YieldStatus {
    NotReady, // Pushed to the queue but may not be switched to yet.
    Ready // May be picked up and switched to.
}

struct YieldedTask {
    task :TaskContext, // Contains the task's fiber as well.
    key :std::num::NonZeroUsize,
    status :YieldStatus,
}
```
`thread_A` initializes the new entry's `key` to the address of `TaskHandle_B` and `status` to `YieldStatus::NotReady` - a.k.a. "please don't pick up and run this fiber as I'm still running it myself". It also stores tke `key` in TLS, indicating there's a yielded fiber which needs to be signaled as ready after the fiber switch and might need to be resumed.

We do it this way, while holding the yield queue lock, because
1) we can't just push the current task/fiber to the yield queue because it might be immediately resumed by `thread_B` and picked up by yet other worker `thread_C` while we're still running it, which would be a *bad thing*, and
2) we can't NOT push anything to the queue because we'd miss task completions that might happen after we unlock the yield queue before proceeding.

By doing this, `thread_A` promises it'll take a look at this entry later and if it finds the status to be changed to `YieldStatus::Ready` by the completing `thread_B`, it'll resume the yielded task.

NOTE: another consideration to have in mind is that pre-context switch `thread_A` may run arbitrary user-provided completion checks before and after we locked the yield queue. This arbitrary completion check closure/trait object does not have to live after the context switch to the new task/scheduler fiber. After the context switch task completion is signaled via `status :YieldStatus` field, synchronizing with `TaskSystem::try_resume_task(\*...*\)`. This allows the task system to support `yield`/`resume` pairs with arbitrary semantics, without having to `Box` any extra data.

`thread_A` then unlocks the queue and executes task scheduler logic - tries to pick up a new task/fiber to execute.

If it finds a new `task_C`/`fiber_C` without waiting, it switches to the `fiber_C`. In `fiber_C` entry point we must check if `key` is set in TLS, and if it is, as is the case in this example, we fulfil our promise - lock the yield queue and check the `status` of entry with `key`.

If `status == YieldStatus::NotReady`, the yield dependency is still unsatisfied (e.g. task is still unfinished) - we set `status = YieldStatus::Ready`, allowing the completing thread to resume the task whenever it happens.

If `status == YieldStatus::Ready`, the yield dependency is satisfied (e.g. the task was finished by `thread_B`) - `thread_A` is now responsible for resuming the task, which it will do by pushing it to the task queue.

In both cases `thread_A` then clears the `key` in TLS and goes on with its life, namely executing `task_C`.

If `thread_A` wasn't able to pick up a new task/fiber without waiting, it switches to the thread's `scheduler_fiber`. In `scheduler_fiber`'s entry point the same TLS `key` check occurs as above.

**Resuming**: `thread_B` will call `TaskSystem::try_resume_task(/*...*/)`, passing it the address of `TaskHandle_B` as `yield_key :std::num::NonZeroUsize`. Task system locks the yield queue, iterates it, looking for a single entry with `yield_queue == &TaskHandle_B`.

If one is not found, the wait has not occured yet and we don't need to do anything - the waiting `thread_A` is guaranteed to see `TaskHandle_B.load() == 0` after `thread_B` unlocks the yield queue.

If one is found, we check its status.

If `yield_status == YieldStatus::NotReady` (the yielding `thread_A` is mid-context switch), set it to `YieldStatus::Ready` and let the yielding thread resume it when it context switches to the new task fiber/scheduler.

If `yield_status == YieldStatus::Ready` (the yielding `thread_A` has already switched to the new fiber/scheduler), we are now responsible for resuming the task, which we will do by pushing it to the task queue.

### **Hot code path**

AR - atomic read
AW - atomic write
CS - enter critical section
PUSH - vec push
POP - vec pop/remove
ITER - vec iterate
CTX - context switch

Here's the best-case synchronisation overhead for the hot path we optimize for - i.e., the waited-on operation (task/IO) completes before we wait for it:

**Waiting thread**:
1) [AR] task handle atomic load and compare to `0` -> success.

**Resuming thread**:
1) [AW] task handle atomic decrement and compare to `0` -> success,
2) [CS] lock the yield queue,
3) [ITER] search the yield key array `yield_keys :Vec<usize>` for entry with `key == &TaskHandle` -> not found.

Seems to be pretty efficient.

### **Cold code path**

This is the worst case, where the waited-on operation (task/IO) will not complete for a while when we wait for it, and there's no resumed fibers ready to be picked up.

**Waiting thread**
1) [AR] task handle atomic load and compare to `0` -> failure,
2) [CS] lock the yield queue,
3) [AR] task handle atomic load and compare to `0` -> failure,
4) [PUSH] add a new entry to the queue with a specific `key` and `status = YieldStatus::NotReady`, unlock the yield queue,
5) [CS] lock the resumed fiber queue, try pop a resumed fiber -> failure, unlock the resumed queue,
6) [CS] lock the fiber pool, try get a new fiber -> success, unlock the fiber pool,
7) [CTX] switch to new fiber,
8) [CS] lock the yield queue,
9) [ITER] search the yield key array `yield_keys :Vec<usize>` for entry with `key` (must succeed), check status -> `status == YieldStatus::NotReady`,
set `status == YieldStatus::Ready`, unlock the yield queue.
10) [resume ready fibers / execute tasks / wait for new tasks/resumed fibers],
11) [CTX] switch to now resumed fiber,
12) cleanup from previous free fiber - [CS] lock the fiber pool and [PUSH] previous free fiber,

    or

    cleanup from previous yielded fiber - [CS] lock the yield queue, [ITER] the yield key array, on `status == YieldStatus::Ready` [POP] the now-resumed fiber, [CS] the resumed queue, [PUSH] the now-resumed fiber.

**Resuming thread**:
1) [AW] task handle atomic decrement and compare to `0` -> success,
2) [CS] lock the yield queue,
3) [ITER] search the yield key array `yield_keys :Vec<usize>` for entry with `key == &TaskHandle` -> found with `status == YieldStatus::Ready`,
4) [POP] remove the found entry, unlock the yield queue,
5) [CS] lock the resumed fiber queue,
6) [PUSH] push the resumed fiber from the removed entry, signal the event, unlock the resumed fiber queue.

As you can see, quite a bit more convoluted. Can it be done better?

## Scheduling algorithm

With above in mind, here's the simplified scheduling algorithm overview.

It's two main ingridients are the scheduler loop and the yield function.

Here's the scheduler loop (`Self` is `TaskSystem`):

```rust
fn scheduler_loop(&self) {
    // If this fiber has been scheduled for the first time, it might need to clean up
    // from the previous fiber.
    self.cleanup_previous_fiber();

    // Run until task system shutdown.
    while !self.exit() {
        // First check if any yielded fibers are ready to be resumed.
        if let Some(resumed_fiber) = self.try_get_resumed_fiber() {
            // Mark current fiber as one ready to be returned to the pool.
            // (We can't just push it to the pool as we're still running it,
            // and a fiber may not be ran by multiple threads).
            // It's the resumed fiber's responsibility to free it -
            // this is handled by `cleanup_previous_fiber()`.
            self.mark_current_fiber_as_free();

            // Prepare the thread context (current task/fiber) and switch
            // to the resumed fiber, continuing its execution where it yielded ...
            self.switch_to_resumed_fiber(resumed_fiber);

            // ... and we're back, maybe in a different thread, because some thread needed
            // a free fiber when it yielded.

            // Don't forget to cleanup from the thread's previous fiber.
            self.cleanup_previous_fiber();

        // Else pop a task from the queue and execute it.
        // The task may yield.
        } else if let Some(task) = self.try_get_task() {
            // Prepare the thread context (current task) and run the task.
            self.execute_task(taks);
            // The task is finished.
            // We might be in a different thread if the task yielded.

            // Take the current task, decrement the task handle(s),
            // maybe resume a yielded fiber waiting for this task.
            self.finish_task(self.take_current_task());

        // No tasks - wait for an event to be signaled.
        // Events are signaled / threads are woken up when a new task is added
        // or a fiber is resumed.
        } else {
            self.wait();
        }
    }
}
```

User tasks may yield during their exexution (see the yielding section above). For example, waiting for a task to finish (`TaskHandle` counter to reach `0`), or an IO operation completion (we also use a `TaskHandle` to poll for completion, but not for resuming).

This is the implementation:

```rust
fn yield_current_task<Y>(&self, yield_data :Y)
where
    Y: YieldData
{
    loop {
        // First check if we have a resumed/free fiber available to switch to.
        if let Some(fiber) = self.try_get_fiber_to_switch_to() {
            let yield_queue = self.lock_yield_queue();

            // We checked yield dependency completion while holding the yield queue lock
            // and it's complete.
            if yield_data.is_complete() {
                // Return the fiber we got back to the resumed queue/fiber pool.
                self.return_fiber_to_switch_to(fiber);

                return;

            // Else the yield dependency is not complete - we commit to yielding this fiber.
            } else {
                // The yield key which the `YieldData` will use to later resume the task.
                let yield_key = yield_data.yield_key();

                // Mark the current fiber as yielded and ready to be resumed,
                // but not ready to be switched to yet (as the current thread is running it).
                // The next fiber must mark it as ready to be resumed (if the yield
                // dependency is *still* incomplete by that point) or actually resume it
                // by pushing it to the resumed queue if the yield dependency *is* complete.
                self.mark_current_fiber_as_yielded(yield_key);

                yield_queue.yield_fiber(self.take_current_fiber(), yield_key);

                // Setup the thread context (current fiber / task) and switch to it.
                // If it's a new fiber, it will run the scheduler loop,
                // picking up resumed /new tasks and running them.
                // If it's a resumed fiber, it will be resumed directly after the call
                // to `switch_to_fiber()` just below.
                self.switch_to_fiber(fiber);
                // ... and we're back. The yield dependency is complete and we were resumed
                // (maybe in a different thread).

                // Don't forget to free / make ready to be resumed
                // the previous fiber this thread ran.
                self.cleanup_previous_fiber();

                // Exit the loop.
                break;
            }

        // We have no resumed fibers / ran out of free fibers,
        // and allow inline task execution.
        } else {
            // Pop a task from the queue and execute it inline.
            // The task may yield.
            if let Some(task) = self.try_get_task() {
                // Save the current task.
                let current_task = self.take_current_task();

                // Prepare the thread context (current task) and run the task.
                self.execute_task(task);
                // The task is finished.
                // We might be in a different thread if the task yielded.

                // Take the current task, decrement the task handle(s),
                // maybe resume a yielded fiber waiting for this task.
                self.finish_task(self.take_current_task());

                // Restore the current task.
                self.set_current_task(current_task);

                // If we finished the right task / were resumed via other means, we're done.
                // Else continue the loop, including checking for resumed/free fibers.
                if yield_data.is_complete() {
                    break;
                }
            }
        }
    }
}

fn cleanup_previous_fiber(&self) {
    // If the previous fiber called `mark_current_fiber_as_free()`.
    if let Some(fiber_to_free) = self.fiber_to_free() {
        // Return to the fiber pool, ready to be reused whenever necessary.
        self.free_fiber(fiber_to_free);
    }

    // If the previous fiber called `mark_current_fiber_as_yielded(yield_key)`.
    if let Some(yield_key) = self.take_yield_key() {
        self.try_resume_fiber(yield_key);
    }
}

// If the yielded fiber was ready to be resumed, push it to the resumed queue,
// wake up a waiting worker thread if necessary.
fn try_resume_fiber(yield_key :NonZeroUsize) {
    if let Some(resumed_fiber) = self.try_resume_yielded_fiber(yield_key) {
        self.enqueue_resumed_fiber(resumed_fiber);
    }
}
```

`YieldData` is a trait defined like this:

```rust
trait YieldData {
    fn is_complete(&self) -> bool;
    fn yield_key(&self) -> NonZeroUsize;
}
```

`is_complete()` is called in `yield_current_task()` before the fiber switch.

`yield_key()` returns an arbitrary non-null `usize` value which may later be used in the call to `try_resume_fiber()`.

E.g., when waiting for task completion, `yield_key` is the address of the `TaskHandle` associated with the task. `is_complete()` loads the `TaskHandle` counter and compares it to `0`. When the task system finishes a task, it calls `try_resume_fiber()` with this `yield_key`.

This mechanism is also used for async IO. Here, `yield_key` is the address of Win32 `OVERLAPPED` struct with an appended atomic counter used for `is_complete()`. When an async IO operation completes, the IO completion port thread decrements the counter and tries to resume a fiber yielded with that yield key.

[^1]: For some definition of arbitrary. We'll assume a fixed signature `FnOnce(()) -> ()`. In C++ we might use `void (*)(void*)` (or a lambda with the same signature) using a pointer to some opaque userdata as the argument, but it's hard (impossible?) to do safely in Rust in a generic fashion, and using closures with captures is more ergonomic and safe.

[^2]: Compare to [`std::thread::Builder::spawn`], which demands `'static` lifetime bound on the closure due to the fact that the spawned thread is *allowed* to outlive the spawning thread.

[^3]: However there exists a trick with closure mutable references.

    Here Rust correctly warns us about multiple mutable borrows:
    ```rust
    fn execute_closure<F>(f :F)
        where F : FnOnce()
    {
        f();
    }

    let mut x = 0;

    let borrow_1 = || { x = 1; };

    // ERROR: cannot borrow `x` as mutable more than once at a time
    let borrow_2 = || { x = 2; };

    execute_closure(borrow_1);
    execute_closure(borrow_2);

    assert_eq!(x, 2);
    ```

    However, this compiles:
    ```rust
    // ...

    let mut x = 0;

    let borrow_1 = || { x = 1; };
    execute_closure(borrow_1);

    let borrow_2 = || { x = 2; };
    execute_closure(borrow_2);

    assert_eq!(x, 2);
    ```

    This *is* fine in single threaded context, and `x` is guaranteed to be `2`. However when these closure are executed in parallel we get mutable aliasing and UB.

    In order to fix this, we need something like this:
    ```rust
    struct ClosureExecutor<'a> {
        _marker :PhantomData<&'a ()>,
    }

    impl<'a> ClosureExecutor<'a> {
        fn new() -> Self {
            Self {
                _marker : PhantomData,
            }
        }

        // Notice `&mut self` - because of mutability, we'll get a compile
        // time error when executing multiple closures
        // with mutable borrows.
        fn execute_closure<F>(&mut self, f :F)
            where F : FnOnce() + 'a
        {
            f();
        }
    }

    let mut e = ClosureExecutor::new();

    let mut x = 0;

    let borrow_1 = || { x = 1; }
    e.execute_closure(borrow_1);

    // ERROR: cannot borrow `x` as mutable more than once at a time.
    let borrow_2 = || { x = 2; }
    e.execute_closure(borrow_2);
    ```

[^4]: We just `memcpy()` the closure object into a fixed-size buffer and use the generic closure type to create a static function which unpacks and calls the closure:
    ```rust
    const BUFFER_SIZE :usize = /* ... */;
    type ClosureStorage = [u8; BUFFER_SIZE];
    type ClosureExecutor = fn(&ClosureStorage);

    let b : ClosureStorage = [0u8; BUFFER_SIZE];
    let e : ClosureExecutor;

    let f = || { /* ... */ };

    fn store_closure<F>(f :F, b :&mut ClosureStorage, e :&mut ClosureExecutor)
        where F : FnOnce()
    {
        // NOTE - make sure the buffer is large enough.

        // `memcpy()` the closure object (struct) to the buffer.
        // `ptr::write()` does not drop `f` which is exactly what we need.
        unsafe {
            ptr::write(b.as_mut_ptr() as *mut F, f);
        }

        // This closure resolves to a static function pointer during
        // monomorphisation.
        *e = |b :&ClosureStorage| {
            unsafe {
                // `f` is moved out of the buffer, executed and dropped.
                let f = ptr::read::<F>(b.as_ptr() as *const F);
                f();
            }
        };
    };

    store_closure(f, &mut b, &mut e);

    // This calls `f`.
    e(&b);
    ```

[`std::thread::Builder::spawn`]: https://doc.rust-lang.org/std/thread/struct.Builder.html#method.spawn
[`std::thread::Builder::spawn_unchecked`]: https://doc.rust-lang.org/std/thread/struct.Builder.html#method.spawn_unchecked
[`crossbeam::Scope::spawn`]: https://docs.rs/crossbeam/0.3.0/crossbeam/struct.Scope.html#method.spawn
