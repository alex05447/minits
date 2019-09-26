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

use minits::TaskSystem;

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

    let params = minits::TaskSystemParams::new(num_worker_threads, num_fibers, true, 1024 * 1024);

    TaskSystem::new(params)
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

#[test]
fn one_task_st() {
    let ts = setup(0, 0);

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

#[test]
fn multiple_tasks_st() {
    let ts = setup(0, 0);

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

#[test]
fn range_task() {
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
fn range_task_st() {
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
