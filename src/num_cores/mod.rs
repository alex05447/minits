#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use win::{get_num_logical_cores, get_num_physical_cores};
