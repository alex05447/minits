#[cfg(windows)]
mod win;

#[cfg(windows)]
pub(crate) use win::IOHandle;

#[cfg(windows)]
pub use win::{File, OpenOptions};
