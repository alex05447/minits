use std::mem;
use std::ops::Range;
use std::sync::mpsc;
use std::time;

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

    let num_cores =
        minits::get_num_physical_cores().max(1);
    //  minits::get_num_logical_cores().max(1);

    let num_worker_threads = num_cores - 1;

    let builder = minits::TaskSystemBuilder::new()
        .num_worker_threads(num_worker_threads);

    minits::init_task_system(builder);

    let range = 0..500_000;

    let num_serial_runs = 4;
    let num_parallel_runs = 4;

    let mut avg_time_serial = time::Duration::from_secs(0);
    let mut total_time_serial = time::Duration::from_secs(0);

    let mut avg_time_parallel = time::Duration::from_secs(0);
    let mut total_time_parallel = time::Duration::from_secs(0);

    let mut primes_serial = Vec::new();

    for run in 0..num_serial_runs {
        println!("Serial start run {}", run);
        let timer_serial = time::Instant::now();

        primes_serial = primes_in_range_serial(&range);

        let elapsed_serial = timer_serial.elapsed();
        total_time_serial += elapsed_serial;
        avg_time_serial = (elapsed_serial + run * avg_time_serial) / (run + 1);

        println!("Serial end. Elapsed: {:?}", elapsed_serial);
    }

    let mut primes_parallel = Vec::new();

    for run in 0..num_parallel_runs {
        println!("Prallel start run {}", run);
        let timer_parallel = time::Instant::now();

        primes_parallel = primes_in_range_parallel(&range, 8);

        let elapsed_parallel = timer_parallel.elapsed();
        total_time_parallel += elapsed_parallel;
        avg_time_parallel = (elapsed_parallel + run * avg_time_parallel) / (run + 1);

        println!("Parallel end. Elapsed: {:?}", elapsed_parallel);
    }

    assert!(primes_serial.len() == primes_parallel.len());

    for (i, prime) in primes_serial.iter().enumerate() {
        assert!(*prime == primes_parallel[i]);
    }

    let avg_time_serial_sec =
        avg_time_serial.as_secs() as f64 + avg_time_serial.subsec_millis() as f64 / 1000.0;
    let avg_time_parallel_sec =
        avg_time_parallel.as_secs() as f64 + avg_time_parallel.subsec_millis() as f64 / 1000.0;
    let speedup = if avg_time_parallel_sec > 0.0 {
        avg_time_serial_sec / avg_time_parallel_sec
    } else {
        9_999_999.9
    };

    println!(
        "Serial: {:?} (total: {:?} / {} runs), parallel: {:?} (total: {:?} / {} runs)",
        avg_time_serial,
        total_time_serial,
        num_serial_runs,
        avg_time_parallel,
        total_time_parallel,
        num_parallel_runs
    );
    println!("Speedup: {:.2}x", speedup);

    minits::fini_task_system();
}

fn is_prime(num: u32) -> bool {
    if num == 2 {
        return true;
    }
    if num % 2 == 0 {
        return false;
    }
    let mut divisor = 3;
    while divisor < (num / 2) {
        if num % divisor == 0 {
            return false;
        }
        divisor += 2;
    }
    true
}

fn primes_in_range_internal<F>(range: &Range<u32>, f: F)
where
    F: FnMut(u32),
{
    let range = range.clone();
    let mut f = f;

    for num in range {
        if is_prime(num) {
            f(num);
        }
    }
}

fn primes_in_range_serial(range: &Range<u32>) -> Vec<u32> {
    let mut primes = Vec::new();

    primes_in_range_internal(range, |num| primes.push(num));

    primes
}

fn primes_in_range_parallel(range: &Range<u32>, multiplier: u32) -> Vec<u32> {
    let range = range.clone();
    let mut primes = Vec::new();

    let (tx, rx) = mpsc::channel();

    let handle = minits::task_system().handle();
    let mut scope = minits::task_system().scope(&handle);

    scope.task_range_named("Parallel primes", range, multiplier, move |range, _| {
        primes_in_range_internal(&range, |num| tx.send(num).unwrap())
    });

    // If `true`, main thread helps the worker threads by executing spawned tasks.
    // After all tasks are complete, we have a full queue of generated primes.
    // Pros: primes are generated faster.
    // Cons: higher memory use for the queue.
    // If `false`, main thread blocks on the receiver popping generated primes
    // and does not execute tasks.
    // Pros: less memory used for the queue.
    // Cons: primes are generated slower.
    let use_main_thread = true;

    if use_main_thread {
        // Wait for the tasks to finish.
        mem::drop(scope);

        // Main thread consumes generated primes, breaking the loop when all primes are popped.
        // All senders we cloned got dropped earlier when the tasks completed.
        loop {
            match rx.recv() {
                Ok(prime) => primes.push(prime),
                Err(_) => {
                    break;
                } // All senders dropped - we're finished.
            }
        }
    } else {
        // Main thread consumes generated primes, breaking the loop when all tasks complete
        // because all senders we cloned get dropped.
        loop {
            match rx.recv() {
                Ok(prime) => primes.push(prime),
                Err(_) => {
                    break;
                } // All senders dropped - we're finished.
            }
        }

        // This is a no-op.
        mem::drop(scope);
    }

    // Or we could spawn a task that will block one thread just gathering primes.
    // scope.task(
    //     || {
    //         loop {
    //             match rx.recv() {
    //                 Ok(prime) => primes.push(prime),
    //                 Err(_) => { break; }, // All senders dropped - we're finished.
    //             }
    //         }
    //     }
    // );
    // mem::drop(scope);

    // Generated primes are overplapped, so let's sort them.
    primes.sort();

    primes
}
