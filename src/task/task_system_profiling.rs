use crate::TaskSystem;

impl TaskSystem {
    pub(crate) fn profile_before_execute_task(&self, task_name: Option<&str>) {
        // println!(
        //     ">>> begin_cpu_sample `{}` (started)",
        //     task_name.unwrap_or("<unnamed>")
        // );
        self.profiler_begin_scope(task_name);
    }

    #[allow(unused_variables)]
    pub(crate) fn profile_after_execute_task(&self, task_name: Option<&str>) {
        // println!(
        //     "<<< end_cpu_sample `{}` (finished)",
        //     task_name.unwrap_or("<unnamed>")
        // );
        self.profiler_end_scope();
    }

    pub(crate) fn profile_wait_start(&self) {
        if !self.is_main_task() {
            // println!(
            //     "<<< end_cpu_sample `{}` (pre-wait)",
            //     self.task_name_or_unnamed()
            // );
            self.profiler_end_scope();
        }
    }

    pub(crate) fn profile_wait_end(&self) {
        if !self.is_main_task() {
            let task_name = self.task_name();

            // println!(
            //     ">>> begin_cpu_sample `{}` (post-wait)",
            //     task_name.unwrap_or("<unnamed>")
            // );
            self.profiler_begin_scope(task_name);
        }
    }

    pub(super) fn profiler_begin_scope(&self, scope_name: Option<&str>) {
        if let Some(profiler) = self.profiler.as_ref() {
            profiler.begin_scope(scope_name);
        }
    }

    pub(super) fn profiler_end_scope(&self) {
        if let Some(profiler) = self.profiler.as_ref() {
            profiler.end_scope();
        }
    }
}
