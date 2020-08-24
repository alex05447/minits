use {
    super::{panic::fmt_panic, task::Task, task_system::YieldKey, yield_queue::TaskAndFiber},
    crate::{Handle, PanicPayload, TaskRange, TaskSystem},
};

impl TaskSystem {
    pub(crate) fn trace_added_task(&self, task: &Task) {
        let ctx = self.thread();
        let h: &Handle = &task.handle;

        let s = format!(
            "[Added] Task: '{}', scope: '{}' [{:p} <{}>] (cur. task: '{}', scope: '{}' [{:p} <{}>], thread: '{}', fiber: '{}')",
            task.task_name_or_unnamed(),
            task.scope_name_or_unnamed(),
            h,
            h.load(),
            ctx.task_name_or_unnamed(),
            ctx.scope_name_or_unnamed(),
            ctx.handle(),
            ctx.handle().load(),
            ctx.name_or_unnamed(),
            ctx.fiber_name_or_unnamed(),
        );
        trace!("{}", s);
    }

    pub(crate) fn trace_picked_up_task(
        &self,
        h: &Handle,
        task_name: Option<&str>,
        scope_name: Option<&str>,
    ) {
        let ctx = self.thread();

        let s = format!(
            "[Picked up new] Task: '{}', scope: '{}' [{:p} <{}>], by thread: '{}', fiber: '{}'",
            task_name.unwrap_or("<unnamed>"),
            scope_name.unwrap_or("<unnamed>"),
            h,
            h.load(),
            ctx.name_or_unnamed(),
            ctx.fiber_name_or_unnamed()
        );
        trace!("{}", s);
    }

    pub(crate) fn trace_wait_start(&self, h: &Handle, scope_name: Option<&str>) {
        let ctx = self.thread();

        let s = format!(
            "[Wait start] For scope: '{}' [{:p} <{}>] (cur. task: '{}', scope: '{}' [{:p} <{}>], thread: '{}', fiber: '{}')",
            scope_name.unwrap_or("<unnamed>"),
            h,
            h.load(),
            ctx.task_name_or_unnamed(),
            ctx.scope_name_or_unnamed(),
            ctx.handle(),
            ctx.handle().load(),
            ctx.name_or_unnamed(),
            ctx.fiber_name_or_unnamed()
        );
        trace!("{}", s);
    }

    pub(crate) fn trace_wait_end(&self, h: &Handle, scope_name: Option<&str>) {
        let ctx = self.thread();

        let s = format!(
            "[Wait end] For scope: '{}' [{:p}] (cur. task: '{}', scope: '{}' [{:p} <{}>], thread: '{}', fiber: '{}')",
            scope_name.unwrap_or("<unnamed>"),
            h,
            ctx.task_name_or_unnamed(),
            ctx.scope_name_or_unnamed(),
            ctx.handle(),
            ctx.handle().load(),
            ctx.name_or_unnamed(),
            ctx.fiber_name_or_unnamed()
        );
        trace!("{}", s);
    }

    pub(crate) fn trace_yielded_task(&self, task: &TaskAndFiber, yield_key: YieldKey) {
        let ctx = self.thread();
        let h: &Handle = &task.task.handle;

        let yield_key: *const () = yield_key.get() as _;

        let s = format!(
            "[Yielded] Task: '{}', scope: '{}' [{:p} <{}>], fiber '{}', waiting for [{:p}], by thread: '{}'"
            ,task.task.task_name_or_unnamed()
            ,task.task.scope_name_or_unnamed()
            ,h
            ,h.load()
            ,task.fiber.name().unwrap_or("<unnamed>")
            ,yield_key
            ,ctx.name_or_unnamed()
        );
        trace!("{}", s);
    }

    pub(crate) fn trace_resumed_task(&self, task: &TaskAndFiber, yield_key: YieldKey) {
        let ctx = self.thread();
        let h: &Handle = &task.task.handle;

        let yield_key: *const () = yield_key.get() as _;

        let s = format!(
            "[Resumed] Task: '{}', scope: '{}' [{:p} <{}>], fiber '{}', waited for [{:p}], by thread: '{}', fiber: '{}'"
            ,task.task.task_name_or_unnamed()
            ,task.task.scope_name_or_unnamed()
            ,h
            ,h.load()
            ,task.fiber.name().unwrap_or("<unnamed>")
            ,yield_key
            ,ctx.name_or_unnamed()
            ,ctx.fiber_name_or_unnamed()
        );
        trace!("{}", s);
    }

    pub(crate) fn trace_picked_up_resumed_task(&self, task: &TaskAndFiber) {
        let ctx = self.thread();
        let h: &Handle = &task.task.handle;

        let s = format!(
            "[Picked up resumed] Task: '{}', scope: '{}' [{:p} <{}>], fiber '{}', by thread: '{}', fiber: '{}'",
            task.task.task_name_or_unnamed(),
            task.task.scope_name_or_unnamed(),
            h,
            h.load(),
            task.fiber.name().unwrap_or("<unnamed>"),
            ctx.name_or_unnamed(),
            ctx.fiber_name_or_unnamed(),
        );
        trace!("{}", s);
    }

    pub(crate) fn trace_finished_task(
        &self,
        h: &Handle,
        task_name: Option<&str>,
        scope_name: Option<&str>,
        panic: &Option<PanicPayload>,
    ) {
        let ctx = self.thread();

        let s = if let Some(panic) = panic {
            let mut s = format!(
                "[Panicked] Task: '{}', scope: '{}' [{:p} <{}>], in thread: '{}', fiber: '{}'",
                task_name.unwrap_or("<unnamed>"),
                scope_name.unwrap_or("<unnamed>"),
                h,
                h.load() - 1,
                ctx.name_or_unnamed(),
                ctx.fiber_name_or_unnamed(),
            );
            fmt_panic(&mut s, panic, 0).unwrap();
            s
        } else {
            format!(
                "[Finished] Task: '{}', scope: '{}' [{:p} <{}>], by thread: '{}', fiber: '{}'",
                task_name.unwrap_or("<unnamed>"),
                scope_name.unwrap_or("<unnamed>"),
                h,
                h.load() - 1,
                ctx.name_or_unnamed(),
                ctx.fiber_name_or_unnamed()
            )
        };

        trace!("{}", s);
    }

    pub(crate) fn trace_fork_task_range(
        &self,
        h: &Handle,
        range: &TaskRange,
        num_chunks: u32,
        task_name: Option<&str>,
        scope_name: Option<&str>,
    ) {
        let s = format!(
            "[Fork] Task: '{}', range: `[{}..{})`, chunks: '{}', scope: '{}' [{:p} <{}>]",
            task_name.unwrap_or("<unnamed>"),
            range.start,
            range.end,
            num_chunks,
            scope_name.unwrap_or("<unnamed>"),
            h,
            h.load()
        );
        trace!("{}", s);
    }
}
