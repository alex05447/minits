use {
    minigraph::{Graph, TaskGraph},
    minits,
};

#[cfg(feature = "logging")]
use std::{io::Write, sync::Once};

#[cfg(feature = "logging")]
extern crate log;

#[cfg(feature = "logging")]
extern crate env_logger;

#[cfg(feature = "logging")]
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

type SystemVertexID = usize;
type ResourceID = usize;
type SystemID = usize;

#[derive(Clone, Copy)]
pub struct SystemDesc<'a> {
    pub id: SystemID,
    pub read: &'a [ResourceID],
    pub write: &'a [ResourceID],
}

impl<'a> SystemDesc<'a> {
    pub fn new(id: SystemID, read: &'a [ResourceID], write: &'a [ResourceID]) -> Self {
        Self { id, read, write }
    }
}

/// Takes an array of "system" descriptors and returns a `TaskGraph`,
/// representing the system dependencies and
/// providing the root-to-leaf iteration API for system scheduling.
pub fn build_system_graph<'a>(systems: &[SystemDesc<'a>]) -> TaskGraph<SystemVertexID, SystemID> {
    let mut systems_and_vertex_ids = Vec::with_capacity(systems.len());
    let mut graph = Graph::new();

    for system in systems {
        add_system_to_graph(*system, &mut graph, &mut systems_and_vertex_ids);
    }

    graph.task_graph().unwrap()
}

fn add_system_to_graph<'a>(
    system: SystemDesc<'a>,
    graph: &mut Graph<SystemVertexID, SystemID>,
    systems_and_vertex_ids: &mut Vec<(SystemDesc<'a>, SystemVertexID)>,
) {
    // Duplicate system id.
    if let Some(_) = systems_and_vertex_ids
        .iter()
        .find(|(added_system, _)| added_system.id == system.id)
    {
        panic!("duplicate system ID");
    }

    // Add to the graph.
    let vertex_id = graph.add_vertex(system.id);

    // For each of the system's reads check if it was written by a previous system.
    // If true, add an edge from the latest writer to the system.
    for read in system.read.iter() {
        for (writer, writer_vertex_id) in systems_and_vertex_ids.iter().rev() {
            if writer.write.contains(read) {
                // Reverse direction edge already exists - we have a cyclic graph.
                if graph.has_edge(vertex_id, *writer_vertex_id).unwrap() {
                    panic!("cyclic graph");
                }

                graph.add_edge(*writer_vertex_id, vertex_id).unwrap();
                break;
            }
        }
    }

    // For each of the system's writes check if it was written by a previous system.
    // If true, add an edge from the latest writer to the system.
    for write in system.write.iter() {
        for (writer, writer_vertex_id) in systems_and_vertex_ids.iter().rev() {
            if writer.write.contains(write) {
                // Reverse direction edge already exists - we have a cyclic graph.
                if graph.has_edge(vertex_id, *writer_vertex_id).unwrap() {
                    panic!("cyclic graph");
                }

                graph.add_edge(*writer_vertex_id, vertex_id).unwrap();
                break;
            }
        }
    }

    systems_and_vertex_ids.push((system, vertex_id));
}

fn main() {
    #[cfg(feature = "logging")]
    setup_logger();

    let builder = minits::TaskSystemBuilder::new()
        .num_worker_threads(1)
        .num_fibers(10);

    minits::init_task_system(builder);

    {
        // 0    1    3 <- these are the roots and may be scheduled to run immediately, in parallel.
        // |\  /    /|
        // | \/    / |
        // | 2    /  4 <- these require (different) dependencies to be fulfilled, and may run in parallel.
        // |_|_ _/_  |
        //   |  /  \ |
        //    6      5 <- and so on.

        let systems = [
            SystemDesc::new(0, &[], &[0]),
            SystemDesc::new(1, &[], &[1]),
            SystemDesc::new(2, &[0], &[1]),
            SystemDesc::new(3, &[], &[2]),
            SystemDesc::new(4, &[2], &[3]),
            SystemDesc::new(5, &[3], &[0]),
            SystemDesc::new(6, &[2], &[1]),
        ];

        let task_graph = build_system_graph(&systems);

        let h = minits::task_system().handle();
        let mut scope = minits::task_system().scope_named(&h, "Task graph scope");

        scope
            .graph(&task_graph, |system_id, _| {
                let system = systems
                    .iter()
                    .find(|system| system.id == *system_id)
                    .unwrap();

                println!(
                    "{}",
                    format!(
                        "Execute system {:?}: read {:#?}, write {:#?}.",
                        system_id, system.read, system.write
                    )
                );
            })
            .name("My task graph")
            .submit();
    }

    minits::fini_task_system();
}
