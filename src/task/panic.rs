use {
    crate::util::{LinkedList, LinkedListIntoIter},
    std::{
        any::Any,
        fmt::{Debug, Display, Formatter, Write},
    },
};

pub type PanicPayload = Box<dyn Any + Send + 'static>;

/// Contains some information about a single panicked task.
///
/// Implements `Debug` / `Display`, printing the panic information.
/// Recurses over panicked children tasks, if any.
pub struct TaskPanicInfo {
    /// Contains the payload with which the panic was invoked.
    pub panic: PanicPayload,
    /// Name of the task which panicked (if any).
    /// Requires "task_names" feature.
    pub task: Option<String>,
    /// Name of the scope whose task has panicked (if any).
    /// Requires "task_names" feature.
    pub scope: Option<String>,
}

impl TaskPanicInfo {
    pub(crate) fn new(panic: PanicPayload, task: Option<String>, scope: Option<String>) -> Self {
        Self { panic, task, scope }
    }
}

impl Display for TaskPanicInfo {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        fmt_panic_info(f, self, 1)
    }
}

impl Debug for TaskPanicInfo {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        <Self as Display>::fmt(self, f)
    }
}

/// Represents a non-empty list of [`TaskPanicInfo`]'s
/// for all panics which occured in the tasks, associated with a [`Scope`], and all their children tasks as well.
///
/// You may iterate over the list, processing each individual [`panic`],
/// or format the list via its `Display` / `Debug` implementation.
///
/// [`TaskPanicInfo`]: struct.TaskPanicInfo.html
/// [`Scope`]: struct.Scope.html
/// [`panic`]: struct.TaskPanicInfohtml
pub struct TaskPanics(pub(crate) LinkedList<TaskPanicInfo>);

// `TaskPanics` must be `Send` for `resume_unwind()`.
unsafe impl Send for TaskPanics {}

impl TaskPanics {
    /// Returns an iterator over the [`TaskPanicInfo`]'s in the list.
    ///
    /// [`TaskPanicInfo`]: struct.TaskPanicInfo.html
    pub fn iter(&self) -> impl Iterator<Item = &'_ TaskPanicInfo> {
        self.0.iter()
    }
}

impl Display for TaskPanics {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        fmt_panics(f, self, 0)
    }
}

impl Debug for TaskPanics {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        <Self as Display>::fmt(self, f)
    }
}

impl IntoIterator for TaskPanics {
    type Item = TaskPanicInfo;
    type IntoIter = TaskPanicsIntoIter;

    fn into_iter(self) -> Self::IntoIter {
        TaskPanicsIntoIter(self.0.into_iter())
    }
}

/// Iterator which pops and returns the [`TaskPanicInfo`]'s from the list.
///
/// [`TaskPanicInfo`]: struct.TaskPanicInfo.html
pub struct TaskPanicsIntoIter(LinkedListIntoIter<TaskPanicInfo>);

impl Iterator for TaskPanicsIntoIter {
    type Item = TaskPanicInfo;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

/// Prints / formats the task panics list to the writer
/// with indentation.
///
/// Recurses over panicked children tasks, if any.
pub(crate) fn fmt_panics<W: Write>(
    w: &mut W,
    panics: &TaskPanics,
    indent: usize,
) -> std::fmt::Result {
    for panic in panics.iter() {
        fmt_panic_info(w, panic, indent)?;
    }
    Ok(())
}

/// Prints / formats the panic info struct to the writer
/// with indentation.
///
/// Recurses over panicked children tasks, if any.
pub(crate) fn fmt_panic_info<W: Write>(
    w: &mut W,
    panic: &TaskPanicInfo,
    indent: usize,
) -> std::fmt::Result {
    let task = panic
        .task
        .as_ref()
        .map(String::as_str)
        .unwrap_or("<unnamed>");
    let scope = panic
        .scope
        .as_ref()
        .map(String::as_str)
        .unwrap_or("<unnamed>");

    for _ in 0..indent {
        write!(w, "\t")?;
    }

    write!(w, "task `{}` (scope `{}`) panicked", task, scope,)?;

    fmt_panic(w, &panic.panic, indent)
}

/// Prints / formats the panic payload to the writer, if it is printable
/// (i.e., it's a panic with a string message, or it's a `TaskPanics` payload),
/// with indentation.
///
/// Recurses over panicked children tasks, if any.
pub(crate) fn fmt_panic<W: Write>(
    w: &mut W,
    panic: &PanicPayload,
    indent: usize,
) -> std::fmt::Result {
    // If it's a string panic message, print it immediately.
    if let Some(msg) = panic.downcast_ref::<&'static str>() {
        write!(w, ": \"{}\"", msg)?;
    } else if let Some(msg) = panic.downcast_ref::<String>() {
        write!(w, ": \"{}\"", msg)?;

    // If it's a `TaskPanics` payload, print it from a new line with increased indentation.
    } else if let Some(panics) = panic.downcast_ref::<TaskPanics>() {
        writeln!(w, ":")?;
        fmt_panics(w, panics, indent + 1)?;
    }

    Ok(())
}
