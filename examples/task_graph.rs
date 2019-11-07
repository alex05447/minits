use minits;
use minigraph::{TaskGraph, SystemDesc, build_system_graph};

#[cfg(feature = "tracing")]
use std::{io::Write, sync::Once};

#[cfg(feature = "tracing")]
extern crate log;

#[cfg(feature = "tracing")]
extern crate env_logger;

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

#[derive(Clone, Copy, PartialEq, Debug)]
struct ResourceID(pub usize);

#[derive(Clone, Copy, PartialEq, Debug)]
struct SystemID(pub usize);

fn main() {
    #[cfg(feature = "tracing")]
    setup_logger();

    // 0    1    3 <- these are the roots and may be scheduled to run immediately, in parallel.
    // |\  /    /|
    // | \/    / |
    // | 2    /  4 <- these require (different) dependencies to be fulfilled, and may run in parallel.
    // |_|_ _/_  |
    //   |  /  \ |
    //    6      5 <- and so on.

    let systems = [
        SystemDesc::new(SystemID(0), &[], &[ResourceID(0)]),
        SystemDesc::new(SystemID(1), &[], &[ResourceID(1)]),
        SystemDesc::new(SystemID(2), &[ResourceID(0)], &[ResourceID(1)]),
        SystemDesc::new(SystemID(3), &[], &[ResourceID(2)]),
        SystemDesc::new(SystemID(4), &[ResourceID(2)], &[ResourceID(3)]),
        SystemDesc::new(SystemID(5), &[ResourceID(3)], &[ResourceID(0)]),
        SystemDesc::new(SystemID(6), &[ResourceID(2)], &[ResourceID(1)]),
    ];

    let task_graph: TaskGraph::<usize, _> = build_system_graph(&systems).unwrap();

    let builder = minits::TaskSystemBuilder::new()
        //.num_worker_threads(0)
        //.num_fibers(0)
    ;

    minits::init_task_system(builder);

    minits::task_system().task_graph(
        &task_graph,
        |system_id, _| {
            let system = systems.iter().find(|system| system.id == *system_id).unwrap();

            println!("Execute system {:?}: read {:#?}, write {:#?}.", system_id, system.read, system.write);
        }
    );

    minits::fini_task_system();
}