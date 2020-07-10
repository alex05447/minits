use std::{
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

#[cfg(feature = "task_names")]
use std::{
    slice::{from_raw_parts, from_raw_parts_mut},
    str::from_utf8_unchecked,
};

/// Simple unsafe wrapper for a simple (non-fat) pointer which the user
/// guarantees is safe to send to another thread.
pub(crate) struct SendPtr<T>(NonNull<T>);

impl<T> SendPtr<T> {
    pub(crate) unsafe fn new(ptr: &T) -> Self {
        Self(NonNull::new_unchecked(ptr as *const _ as _))
    }
}

// We solemnly swear whatever this points to is safe to send to other threads.
unsafe impl<T> Send for SendPtr<T> {}

impl<T> Clone for SendPtr<T> {
    fn clone(&self) -> Self {
        Self(self.0)
    }
}

impl<T> Copy for SendPtr<T> {}

impl<T> Deref for SendPtr<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.0.as_ref() }
    }
}

impl<T> DerefMut for SendPtr<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.0.as_mut() }
    }
}

#[cfg(feature = "task_names")]
pub(crate) mod send_slice {
    use super::*;

    /// Simple unsafe wrapper for a slice (fat pointer) which the user
    /// guarantees is safe to send to another thread.
    pub(crate) struct SendSlice<T> {
        data: NonNull<T>,
        len: usize,
    }

    impl<T> SendSlice<T> {
        pub(crate) unsafe fn new(slice: &[T]) -> Self {
            Self {
                data: NonNull::new_unchecked(slice as *const _ as *mut _),
                len: slice.len(),
            }
        }
    }

    impl SendSlice<u8> {
        pub(crate) unsafe fn as_str(&self) -> &str {
            from_utf8_unchecked(self.deref())
        }
    }

    // We solemnly swear whatever this points to is safe to send to other threads.
    unsafe impl<T> Send for SendSlice<T> {}

    impl<T> Clone for SendSlice<T> {
        fn clone(&self) -> Self {
            Self {
                data: self.data,
                len: self.len,
            }
        }
    }

    impl<T> Copy for SendSlice<T> {}

    impl<T> Deref for SendSlice<T> {
        type Target = [T];

        fn deref(&self) -> &Self::Target {
            unsafe { from_raw_parts(self.data.as_ptr(), self.len) }
        }
    }

    impl<T> DerefMut for SendSlice<T> {
        fn deref_mut(&mut self) -> &mut Self::Target {
            unsafe { from_raw_parts_mut(self.data.as_ptr(), self.len) }
        }
    }
}
