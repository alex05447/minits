use super::task_handle::TaskHandle;
use super::task_system::{TaskRange, TaskSystem};

pub struct TaskScope<'scope, 'task_system> {
    task_handle: &'scope TaskHandle,
    task_system: &'task_system TaskSystem,

    #[cfg(feature = "task_names")]
    scope_name: Option<String>,
}

impl<'scope, 'task_system> TaskScope<'scope, 'task_system> {
    /// Adds a new task closure to the task system.
    /// The closure takes a single `&TaskSystem` parameter and returns no values.
    /// The task may run immediately and is guaranteed to finish when this `TaskScope` goes out of scope.
    pub fn task<F>(&mut self, f: F)
    where
        F: FnOnce(&TaskSystem) + Send + 'scope,
    {
        unsafe {
            if let Some(scope_name) = self.name() {
                self.task_system
                    .task_scoped(scope_name, self.task_handle, f);
            } else {
                self.task_system.task(self.task_handle, f);
            }
        }
    }

    /// Adds a new task closure to the task system.
    /// The closure takes a single `&TaskSystem` parameter and returns no values.
    /// The task may run immediately and is guaranteed to finish when this `TaskScope` goes out of scope.
    ///
    /// `task_name` may be retrieved by [`TaskSystem::task_name`] within the task closure.
    /// Requires "task_names" feature.
    ///
    /// [`TaskSystem::task_name`]: task_system/struct.TaskSystem.html#method.task_name
    pub fn task_named<F>(&mut self, task_name: &str, f: F)
    where
        F: FnOnce(&TaskSystem) + Send + 'scope,
    {
        unsafe {
            if let Some(scope_name) = self.name() {
                self.task_system
                    .task_named_scoped(task_name, scope_name, self.task_handle, f);
            } else {
                self.task_system.task_named(task_name, self.task_handle, f);
            }
        }
    }

    /// Adds a range of task closures to the task system.
    /// Non-empty `range` is distributed over the number of task system threads (including the main thread),
    /// multiplied by `multiplier` (i.e., `multiplier` == `2` means "divide the range into `2` chunks per thread").
    /// The closure takes a `std::ops::Range`, a `&TaskSystem` and returns no values.
    /// The closure must be clonable.
    /// The task may run immediately and is guaranteed to finish when this `TaskScope` goes out of scope.
    ///
    /// # Panics
    ///
    /// Panics if `range` is empty.
    pub fn task_range<F>(&mut self, range: TaskRange, multiplier: u32, f: F)
    where
        F: Fn(TaskRange, &TaskSystem) + Send + Clone + 'scope,
    {
        unsafe {
            if let Some(scope_name) = self.name() {
                self.task_system.task_range_scoped(
                    scope_name,
                    self.task_handle,
                    range,
                    multiplier,
                    f,
                );
            } else {
                self.task_system
                    .task_range(self.task_handle, range, multiplier, f);
            }
        }
    }

    /// Adds a range of task closures to the task system.
    /// Non-empty `range` is distributed over the number of task system threads (including the main thread),
    /// multiplied by `multiplier` (i.e., `multiplier` == `2` means "divide the range into `2` chunks per thread").
    /// The closure takes a `std::ops::Range`, a `&TaskSystem` and returns no values.
    /// The closure must be clonable.
    /// The task may run immediately and is guaranteed to finish when this `TaskScope` goes out of scope.
    ///
    /// `task_name` may be retrieved by [`TaskSystem::task_name`] within the task closure.
    /// Requires "task_names" feature.
    ///
    /// # Panics
    ///
    /// Panics if `range` is empty.
    ///
    /// [`TaskSystem::task_name`]: task_system/struct.TaskSystem.html#method.task_name
    pub fn task_range_named<F>(&mut self, task_name: &str, range: TaskRange, multiplier: u32, f: F)
    where
        F: Fn(TaskRange, &TaskSystem) + Send + Clone + 'scope,
    {
        unsafe {
            if let Some(scope_name) = self.name() {
                self.task_system.task_range_named_scoped(
                    task_name,
                    scope_name,
                    self.task_handle,
                    range,
                    multiplier,
                    f,
                );
            } else {
                self.task_system.task_range_named(
                    task_name,
                    self.task_handle,
                    range,
                    multiplier,
                    f,
                );
            }
        }
    }

    pub(crate) fn new(
        task_handle: &'scope TaskHandle,
        task_system: &'task_system TaskSystem,
        _scope_name: Option<&str>,
    ) -> Self {
        Self {
            task_handle,
            task_system,

            #[cfg(feature = "task_names")]
            scope_name: _scope_name.map(|s| s.to_owned()),
        }
    }

    pub(crate) fn name(&self) -> Option<&str> {
        #[cfg(feature = "task_names")]
        return self.scope_name.as_ref().map(|s| s.as_str());

        #[cfg(not(feature = "task_names"))]
        return None;
    }
}

impl<'scope, 'task_system> Drop for TaskScope<'scope, 'task_system> {
    fn drop(&mut self) {
        unsafe {
            if let Some(name) = self.name() {
                self.task_system
                    .wait_for_handle_named(self.task_handle, name);
            } else {
                self.task_system.wait_for_handle(self.task_handle);
            }
        }
    }
}

pub trait ArrayTaskRange<'scope, 'task_system> {
    type Item;

    fn task_range<F>(
        &'scope self,
        task_scope: &mut TaskScope<'scope, 'task_system>,
        multiplier: u32,
        f: F
    )
    where
        F: Fn(&'scope Self::Item, &TaskSystem) + Send + Clone + 'scope;

    fn task_range_named<F>(
        &'scope self,
        task_scope: &mut TaskScope<'scope, 'task_system>,
        task_name: &str,
        multiplier: u32,
        f: F
    )
    where
        F: Fn(&'scope Self::Item, &TaskSystem) + Send + Clone + 'scope;
}

impl<'scope, 'task_system, T: Sized + Send + Sync> ArrayTaskRange<'scope, 'task_system> for [T] {
    type Item = T;

    fn task_range<F>(
        &'scope self,
        task_scope: &mut TaskScope<'scope, 'task_system>,
        multiplier: u32,
        f: F
    )
    where
        F: Fn(&'scope Self::Item, &TaskSystem) + Send + Clone + 'scope
    {
        use std::ops::Index;

        let array = self;

        task_scope.task_range(
            0 .. self.len() as u32,
            multiplier,
            move |task_range: TaskRange, task_system: &TaskSystem| {
                for i in task_range {
                    f(array.index(i as usize), task_system);
                }
            }
        );
    }

    fn task_range_named<F>(
        &'scope self,
        task_scope: &mut TaskScope<'scope, 'task_system>,
        task_name: &str,
        multiplier: u32,
        f: F
    )
    where
        F: Fn(&'scope Self::Item, &TaskSystem) + Send + Clone + 'scope
    {
        use std::ops::Index;

        let array = self;

        task_scope.task_range_named(
            task_name,
            0 .. self.len() as u32,
            multiplier,
            move |task_range: TaskRange, task_system: &TaskSystem| {
                for i in task_range {
                    f(array.index(i as usize), task_system);
                }
            }
        );
    }
}

pub trait RangeTaskRange<'scope, 'task_system> {
    type Item;

    fn task_range<F>(
        &self,
        task_scope: &mut TaskScope<'scope, 'task_system>,
        multiplier: u32,
        f: F
    )
    where
        F: Fn(Self::Item, &TaskSystem) + Send + Clone + 'scope;

    fn task_range_named<F>(
        &self,
        task_scope: &mut TaskScope<'scope, 'task_system>,
        task_name: &str,
        multiplier: u32,
        f: F
    )
    where
        F: Fn(Self::Item, &TaskSystem) + Send + Clone + 'scope;

}

impl<'scope, 'task_system> RangeTaskRange<'scope, 'task_system> for TaskRange {
    type Item = u32;

    fn task_range<F>(
        &self,
        task_scope: &mut TaskScope<'scope, 'task_system>,
        multiplier: u32,
        f: F
    )
    where
        F: Fn(Self::Item, &TaskSystem) + Send + Clone + 'scope
    {
        task_scope.task_range(
            self.start .. self.end,
            multiplier,
            move |task_range: TaskRange, task_system: &TaskSystem| {
                for i in task_range {
                    f(i, task_system);
                }
            }
        );
    }

    fn task_range_named<F>(
        &self,
        task_scope: &mut TaskScope<'scope, 'task_system>,
        task_name: &str,
        multiplier: u32,
        f: F
    )
    where
        F: Fn(Self::Item, &TaskSystem) + Send + Clone + 'scope
    {
        task_scope.task_range_named(
            task_name,
            self.start .. self.end,
            multiplier,
            move |task_range: TaskRange, task_system: &TaskSystem| {
                for i in task_range {
                    f(i, task_system);
                }
            }
        );
    }
}