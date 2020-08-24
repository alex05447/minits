use {
    super::scope::GraphTaskFn,
    crate::{Handle, TaskRange, TaskSystem},
    minigraph::{TaskGraph, TaskVertex, VertexID},
};

impl TaskSystem {
    pub(crate) unsafe fn submit_graph<'h, VID, T, F>(
        &self,
        h: &'h Handle,
        f: F,
        graph: &'h TaskGraph<VID, T>,
        task_name: Option<String>,
        scope_name: Option<&str>,
    ) where
        VID: VertexID + Send + Sync,
        T: Send + Sync,
        F: GraphTaskFn<'h, T>,
    {
        graph.reset();

        let num_roots = graph.num_roots() as _;

        self.submit_range(
            h,
            move |roots: TaskRange, ts: &TaskSystem| {
                for (vertex_id, vertex) in roots.map(|idx| graph.get_root_unchecked(idx as _)) {
                    ts.execute_graph_vertex(ts.task_handle(), f.clone(), graph, *vertex_id, vertex);
                }
            },
            0..num_roots,
            1,
            task_name.as_ref().map(String::as_str),
            scope_name,
        );
    }

    unsafe fn execute_graph_vertex<'h, VID, T, F>(
        &self,
        h: &Handle,
        f: F,
        graph: &'h TaskGraph<VID, T>,
        vertex_id: VID,
        vertex: &TaskVertex<T>,
    ) where
        VID: VertexID + Send + Sync,
        T: Send + Sync,
        F: GraphTaskFn<'h, T>,
    {
        if !vertex.is_ready() {
            return;
        }

        f(vertex.vertex(), self);

        let num_dependencies = graph.num_dependencies(vertex_id).unwrap() as _;

        if num_dependencies > 0 {
            self.submit_range(
                h,
                move |dependencies: TaskRange, ts: &TaskSystem| {
                    for (vertex_id, vertex) in
                        dependencies.map(|idx| graph.get_dependency_unchecked(vertex_id, idx as _))
                    {
                        ts.execute_graph_vertex(
                            ts.task_handle(),
                            f.clone(),
                            graph,
                            *vertex_id,
                            vertex,
                        );
                    }
                },
                0..num_dependencies,
                1,
                self.task_name(),
                self.scope_name(),
            );
        }
    }
}
