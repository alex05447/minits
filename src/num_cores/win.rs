use std::mem;
use std::ptr;
use std::sync::Once;

use winapi::shared::winerror::ERROR_INSUFFICIENT_BUFFER;
use winapi::um::errhandlingapi::GetLastError;
use winapi::um::sysinfoapi::{GetLogicalProcessorInformation, GetSystemInfo, SYSTEM_INFO};
use winapi::um::winnt::{RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION};

/// Returns the number of system's logical processors.
pub fn get_num_logical_cores() -> usize {
    static mut NUM_LOGICAL_CORES: usize = 1;
    static INIT_NUM_LOGICAL_CORES: Once = Once::new();

    unsafe {
        INIT_NUM_LOGICAL_CORES.call_once(|| {
            NUM_LOGICAL_CORES = get_num_logical_cores_internal();
        });

        NUM_LOGICAL_CORES
    }
}

/// Returns the number of system's physical processors.
pub fn get_num_physical_cores() -> usize {
    static mut NUM_PHYSICAL_CORES: usize = 1;
    static INIT_NUM_PHYSICAL_CORES: Once = Once::new();

    unsafe {
        INIT_NUM_PHYSICAL_CORES.call_once(|| {
            NUM_PHYSICAL_CORES = get_num_physical_cores_internal();
        });

        NUM_PHYSICAL_CORES
    }
}

fn get_num_logical_cores_internal() -> usize {
    let mut system_info: SYSTEM_INFO = unsafe { mem::zeroed() };

    unsafe {
        GetSystemInfo(&mut system_info);
    }

    system_info.dwNumberOfProcessors as usize
}

fn get_num_physical_cores_internal() -> usize {
    let mut buffer_size = 0;

    let result =
        unsafe { GetLogicalProcessorInformation(ptr::null_mut(), &mut buffer_size) as usize };

    let error = unsafe { GetLastError() };

    let struct_size = mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>() as u32;

    if result != 0
        || error != ERROR_INSUFFICIENT_BUFFER
        || buffer_size < struct_size
        || buffer_size % buffer_size != 0
    {
        panic!("Failed to get the number of physical cores.");
    }

    let num_structs = buffer_size / struct_size;

    let mut structs = Vec::with_capacity(num_structs as usize);

    let result =
        unsafe { GetLogicalProcessorInformation(structs.as_mut_ptr(), &mut buffer_size) as usize };

    if result == 0 {
        panic!("Failed to get the number of physical cores.");
    }

    unsafe {
        structs.set_len(num_structs as usize);
    }

    let num_physical_cores = structs
        .iter()
        .filter(|info| info.Relationship == RelationProcessorCore)
        .count();

    num_physical_cores
}
