use std::mem;
use std::sync::atomic::{AtomicU32, Ordering};

#[cfg(feature = "tracing")]
use std::{
    io::Write,
    sync::{Once, ONCE_INIT},
};

#[cfg(feature = "task_names")]
use std::sync::atomic::AtomicUsize;

#[cfg(feature = "tracing")]
extern crate log;

#[cfg(feature = "tracing")]
extern crate env_logger;

use minits::{TaskSystem, TaskSystemBuilder};

#[cfg(feature = "tracing")]
fn setup_logger_internal() {
    static INIT: Once = ONCE_INIT;

    INIT.call_once(|| {
        let mut builder = env_logger::Builder::new();

        builder
            .format(|buf, record| writeln!(buf, "{}", record.args()))
            .filter(None, log::LevelFilter::Trace)
            .init();
    });
}

fn setup_logger() {
    #[cfg(feature = "tracing")]
    setup_logger_internal();
}

fn setup(num_worker_threads: usize, num_fibers: usize) -> Box<TaskSystem> {
    setup_logger();

    TaskSystemBuilder::new()
        .num_worker_threads(num_worker_threads)
        .num_fibers(num_fibers)
        .allow_inline_tasks(true)
        .fiber_stack_size(1024 * 1024)
        .build()
}

fn setup_default() -> Box<TaskSystem> {
    setup(3, 32)
}

// cargo test --features=tracing -- --test-threads=1

#[test]
fn one_task() {
    let ts = setup_default();

    let mut arg = 7;

    let handle = ts.handle();
    let mut scope = ts.scope(&handle);

    scope.task(|_| {
        arg = 9;
    });

    mem::drop(scope);

    assert!(arg == 9);
}

#[test]
fn one_task_main_thread() {
    let ts = setup_default();

    let mut arg = 7;

    let handle = ts.handle();
    let mut scope = ts.scope(&handle);

    scope.task_main_thread(|ts| {
        assert_eq!(ts.thread_index(), 0);
        arg = 9;
    });

    mem::drop(scope);

    assert!(arg == 9);
}

#[test]
fn one_task_fn() {
    let ts = setup_default();

    static ARG: usize = 7;
    static mut RES: usize = 0;

    let handle = ts.handle();
    let mut scope = ts.scope(&handle);

    fn task_fn(_: &TaskSystem) {
        assert!(ARG == 7);

        unsafe {
            RES = ARG;
        }
    }

    scope.task(task_fn);

    mem::drop(scope);

    unsafe {
        assert!(RES == ARG);
    }
}

#[cfg(feature = "task_names")]
#[test]
fn one_task_named() {
    let ts = setup_default();

    let mut arg = 7;

    let handle = ts.handle();
    let mut scope = ts.scope_named(&handle, "Scope 0");

    scope.task_named("Task 0", |ts| {
        assert_eq!(ts.task_name().unwrap(), "Task 0");
        assert_eq!(ts.scope_name().unwrap(), "Scope 0");

        arg = 9;
    });

    mem::drop(scope);

    assert!(arg == 9);
}

#[cfg(feature = "task_names")]
#[test]
fn one_task_named_main_thread() {
    let ts = setup_default();

    let mut arg = 7;

    let handle = ts.handle();
    let mut scope = ts.scope_named(&handle, "Scope 0");

    scope.task_named("Task 0", |ts| {
        assert_eq!(ts.thread_index(), 0);
        assert_eq!(ts.task_name().unwrap(), "Task 0");
        assert_eq!(ts.scope_name().unwrap(), "Scope 0");

        arg = 9;
    });

    mem::drop(scope);

    assert!(arg == 9);
}

#[test]
fn one_task_st() {
    let ts = setup(0, 0);

    let mut arg = 7;

    let handle = ts.handle();
    let mut scope = ts.scope(&handle);

    scope.task(|ts| {
        assert_eq!(ts.thread_index(), 0);
        arg = 9;
    });

    mem::drop(scope);

    assert!(arg == 9);
}

#[test]
fn multiple_tasks() {
    let ts = setup_default();

    let arg = AtomicU32::new(0);

    let handle = ts.handle();
    let mut scope = ts.scope(&handle);

    let num_tasks = 10;

    for _ in 0..num_tasks {
        scope.task(|_| {
            arg.fetch_add(7, Ordering::SeqCst);
        });
    }

    mem::drop(scope);

    assert!(arg.load(Ordering::SeqCst) == 7 * num_tasks);
}

#[test]
fn multiple_tasks_main_thread() {
    let ts = setup_default();

    let arg = AtomicU32::new(0);

    let handle = ts.handle();
    let mut scope = ts.scope(&handle);

    let num_tasks = 10;

    for _ in 0..num_tasks {
        scope.task_main_thread(|ts| {
            assert_eq!(ts.thread_index(), 0);
            arg.fetch_add(7, Ordering::SeqCst);
        });
    }

    mem::drop(scope);

    assert!(arg.load(Ordering::SeqCst) == 7 * num_tasks);
}

#[cfg(feature = "task_names")]
#[test]
fn multiple_tasks_named() {
    let ts = setup_default();

    let arg = AtomicUsize::new(0);

    let handle = ts.handle();
    let mut scope = ts.scope_named(&handle, "Scope 0");

    let num_tasks = 10;

    for i in 0..num_tasks {
        let task_name = format!("Task {}", i);

        let arg = &arg;

        scope.task_named(&task_name, move |ts| {
            let task_name = format!("Task {}", i);

            assert_eq!(ts.task_name().unwrap(), task_name);
            assert_eq!(ts.scope_name().unwrap(), "Scope 0");

            arg.fetch_add(7, Ordering::SeqCst);
        });
    }

    mem::drop(scope);

    assert!(arg.load(Ordering::SeqCst) == 7 * num_tasks);
}

#[cfg(feature = "task_names")]
#[test]
fn multiple_tasks_named_main_thread() {
    let ts = setup_default();

    let arg = AtomicUsize::new(0);

    let handle = ts.handle();
    let mut scope = ts.scope_named(&handle, "Scope 0");

    let num_tasks = 10;

    for i in 0..num_tasks {
        let task_name = format!("Task {}", i);

        let arg = &arg;

        scope.task_named_main_thread(&task_name, move |ts| {
            assert_eq!(ts.thread_index(), 0);

            let task_name = format!("Task {}", i);

            assert_eq!(ts.task_name().unwrap(), task_name);
            assert_eq!(ts.scope_name().unwrap(), "Scope 0");

            arg.fetch_add(7, Ordering::SeqCst);
        });
    }

    mem::drop(scope);

    assert!(arg.load(Ordering::SeqCst) == 7 * num_tasks);
}

#[test]
fn multiple_tasks_st() {
    let ts = setup(0, 0);

    let arg = AtomicU32::new(0);

    let handle = ts.handle();
    let mut scope = ts.scope(&handle);

    let num_tasks = 10;

    for _ in 0..num_tasks {
        scope.task(|ts| {
            assert_eq!(ts.thread_index(), 0);

            arg.fetch_add(7, Ordering::SeqCst);
        });
    }

    mem::drop(scope);

    assert!(arg.load(Ordering::SeqCst) == 7 * num_tasks);
}

#[test]
fn nested_tasks() {
    let ts = setup_default();

    let arg = AtomicU32::new(0);

    let handle = ts.handle();
    let mut scope = ts.scope(&handle);

    let num_tasks = 10;
    let num_nested_tasks = 10;

    for _ in 0..num_tasks {
        scope.task(|ts| {
            arg.fetch_add(7, Ordering::SeqCst);

            let handle = ts.handle();
            let mut scope = ts.scope(&handle);

            for _ in 0..num_nested_tasks {
                scope.task(|_| {
                    arg.fetch_add(9, Ordering::SeqCst);
                });
            }
        });
    }

    mem::drop(scope);

    assert!(arg.load(Ordering::SeqCst) == 7 * num_tasks + num_tasks * num_nested_tasks * 9);
}

#[cfg(feature = "task_names")]
#[test]
fn nested_tasks_named() {
    let ts = setup_default();

    let arg = AtomicUsize::new(0);

    let handle = ts.handle();
    let mut scope = ts.scope_named(&handle, "Scope 0");

    let num_tasks = 10;
    let num_nested_tasks = 10;

    for i in 0..num_tasks {
        let task_name = format!("Task {}", i);

        let arg = &arg;

        scope.task_named(&task_name, move |ts| {
            let task_name = format!("Task {}", i);

            assert_eq!(ts.task_name().unwrap(), task_name);
            assert_eq!(ts.scope_name().unwrap(), "Scope 0");

            arg.fetch_add(7, Ordering::SeqCst);

            let handle = ts.handle();
            let mut scope = ts.scope_named(&handle, "Nested scope 0");

            for j in 0..num_nested_tasks {
                let task_name = format!("Nested task {}", j);

                let arg = arg;

                scope.task_named(&task_name, move |ts| {
                    let task_name = format!("Nested task {}", j);

                    assert_eq!(ts.task_name().unwrap(), task_name);
                    assert_eq!(ts.scope_name().unwrap(), "Nested scope 0");

                    arg.fetch_add(9, Ordering::SeqCst);
                });
            }
        });
    }

    mem::drop(scope);

    assert!(arg.load(Ordering::SeqCst) == 7 * num_tasks + num_tasks * num_nested_tasks * 9);
}

#[test]
fn nested_tasks_st() {
    let ts = setup(0, 0);

    let arg = AtomicU32::new(0);

    let handle = ts.handle();
    let mut scope = ts.scope(&handle);

    let num_tasks = 10;
    let num_nested_tasks = 10;

    for _ in 0..num_tasks {
        scope.task(|ts| {
            assert_eq!(ts.thread_index(), 0);

            arg.fetch_add(7, Ordering::SeqCst);

            let handle = ts.handle();
            let mut scope = ts.scope(&handle);

            for _ in 0..num_nested_tasks {
                scope.task(|ts| {
                    assert_eq!(ts.thread_index(), 0);

                    arg.fetch_add(9, Ordering::SeqCst);
                });
            }
        });
    }

    mem::drop(scope);

    assert!(arg.load(Ordering::SeqCst) == 7 * num_tasks + num_tasks * num_nested_tasks * 9);
}

#[test]
fn task_range() {
    let ts = setup_default();

    let range = 1..64 * 1024;

    let sum_serial = range.clone().fold(0, |sum, val| sum + val);

    let sum_parallel = AtomicU32::new(0);

    let handle = ts.handle();
    let mut scope = ts.scope(&handle);

    scope.task_range(range, 1, |range, _| {
        let local_sum = range.fold(0, |sum, val| sum + val);
        sum_parallel.fetch_add(local_sum, Ordering::SeqCst);
    });

    mem::drop(scope);

    let sum_parallel = sum_parallel.load(Ordering::SeqCst);

    assert_eq!(sum_serial, sum_parallel);
}

#[test]
fn task_range_range() {
    use minits::RangeTaskRange;

    let ts = setup_default();

    let range = 1..64 * 1024;

    let sum_serial = range.clone().fold(0, |sum, val| sum + val);

    let sum_parallel = AtomicU32::new(0);

    let handle = ts.handle();
    let mut scope = ts.scope(&handle);

    range.task_range(&mut scope, 1, |el, _| {
        sum_parallel.fetch_add(el, Ordering::SeqCst);
    });

    mem::drop(scope);

    let sum_parallel = sum_parallel.load(Ordering::SeqCst);

    assert_eq!(sum_serial, sum_parallel);
}

#[test]
fn task_range_array() {
    use minits::ArrayTaskRange;

    let ts = setup_default();

    let array = [1; 64];

    let sum_serial = array.iter().fold(0, |sum, val| sum + val);

    let sum_parallel = AtomicU32::new(0);

    let handle = ts.handle();
    let mut scope = ts.scope(&handle);

    array.task_range(&mut scope, 1, |el, _| {
        sum_parallel.fetch_add(*el, Ordering::SeqCst);
    });

    mem::drop(scope);

    let sum_parallel = sum_parallel.load(Ordering::SeqCst);

    assert_eq!(sum_serial, sum_parallel);
}

#[test]
fn task_range_st() {
    let ts = setup(0, 0);

    let range = 1..64 * 1024;

    let sum_serial = range.clone().fold(0, |sum, val| sum + val);

    let sum_parallel = AtomicU32::new(0);

    let handle = ts.handle();
    let mut scope = ts.scope(&handle);

    scope.task_range(range, 1, |range, _| {
        let local_sum = range.fold(0, |sum, val| sum + val);
        sum_parallel.fetch_add(local_sum, Ordering::SeqCst);
    });

    mem::drop(scope);

    let sum_parallel = sum_parallel.load(Ordering::SeqCst);

    assert_eq!(sum_serial, sum_parallel);
}

#[test]
fn task_range_st_range() {
    use minits::RangeTaskRange;

    let ts = setup(0, 0);

    let range = 1..64 * 1024;

    let sum_serial = range.clone().fold(0, |sum, val| sum + val);

    let sum_parallel = AtomicU32::new(0);

    let handle = ts.handle();
    let mut scope = ts.scope(&handle);

    range.task_range(&mut scope, 1, |el, _| {
        sum_parallel.fetch_add(el, Ordering::SeqCst);
    });

    mem::drop(scope);

    let sum_parallel = sum_parallel.load(Ordering::SeqCst);

    assert_eq!(sum_serial, sum_parallel);
}

#[test]
fn task_range_st_array() {
    use minits::ArrayTaskRange;

    let ts = setup(0, 0);

    let array = [1; 64];

    let sum_serial = array.iter().fold(0, |sum, val| sum + val);

    let sum_parallel = AtomicU32::new(0);

    let handle = ts.handle();
    let mut scope = ts.scope(&handle);

    array.task_range(&mut scope, 1, |el, _| {
        sum_parallel.fetch_add(*el, Ordering::SeqCst);
    });

    mem::drop(scope);

    let sum_parallel = sum_parallel.load(Ordering::SeqCst);

    assert_eq!(sum_serial, sum_parallel);
}

#[test]
fn thread_init_fini() {
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    use minithreadlocal::ThreadLocal;

    setup_logger();

    let num_worker_threads = 3;
    let worker_thread_counter = Arc::new(AtomicUsize::new(0));

    let mut thread_local = ThreadLocal::<usize>::new();

    let worker_thread_counter_clone = worker_thread_counter.clone();

    let thread_init = move |thread_index: usize| {
        assert!((1 ..= num_worker_threads).contains(&thread_index));

        thread_local.store(thread_index);

        worker_thread_counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    };

    let thread_fini = move |thread_index: usize| {
        assert!((1 ..= num_worker_threads).contains(&thread_index));

        let thread_local = thread_local.take();

        assert_eq!(thread_local, thread_index);
    };

    {
        let _ts = TaskSystemBuilder::new()
            .num_worker_threads(num_worker_threads)
            .num_fibers(32)
            .allow_inline_tasks(true)
            .fiber_stack_size(1024 * 1024)
            .thread_init(thread_init)
            .thread_fini(thread_fini)
            .build();
    }

    assert_eq!(worker_thread_counter.load(Ordering::SeqCst), num_worker_threads);

    thread_local.free_index();
}
