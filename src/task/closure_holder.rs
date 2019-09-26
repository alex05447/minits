use std::mem::{self, MaybeUninit};
use std::ptr;

/// Total amount of memory used by the [`StaticClosureHolder`]
/// x86: 4b (executor function pointer) + 60b (closure storage) = 64b
/// x64: 8b (executor function pointer) + 56b (closure storage) = 64b
/// [`StaticClosureHolder`]: #struct.StaticClosureHolder
const CLOSURE_HOLDER_SIZE: usize = 64;
const CLOSURE_PTR_SIZE: usize = mem::size_of::<fn()>();
const CLOSURE_STORAGE_SIZE: usize = CLOSURE_HOLDER_SIZE - CLOSURE_PTR_SIZE;

type StaticClosureStorage = [u8; CLOSURE_STORAGE_SIZE];
type ClosureExecutor = fn(&[u8], *const ());

/// Opaque fixed-size storage for a closure/function pointer with signature `FnOnce(*const ())`.
///
/// # Safety
///
/// Argument type is erased during storage with [`new`] \ [`store`]. It's entirely up to the user to ensure the stored
/// closure is passed the correct argument type when calling [`execute`].
/// Stores closures with any lifetime with [`new`] \ [`store`]. It is up to the caller to guarantee that any
/// borrows live until the call to [`execute`].
///
/// Static storage size (`CLOSURE_STORAGE_SIZE`) is determined by `CLOSURE_HOLDER_SIZE` constant and the platform function pointer size.
/// Namely, up to `CLOSURE_HOLDER_SIZE - mem::size_of<fn()>` bytes are used to store the closure in the object.
/// Closures larger than `CLOSURE_STORAGE_SIZE` are stored on the heap.
///
/// [`new`]: #method.new
/// [`store`]: #method.store
/// [`execute`]: #method.execute
pub(super) struct ClosureHolder {
    storage: ClosureStorage,
    executor: Option<ClosureExecutor>,
}

enum ClosureStorage {
    Static(MaybeUninit<StaticClosureStorage>),
    Dynamic(Option<Box<[u8]>>),
}

assert_eq_size!(closure_storage; StaticClosureStorage, [u8; CLOSURE_STORAGE_SIZE]);

impl ClosureHolder {
    /// Creates an empty holder.
    pub(super) fn empty() -> Self {
        ClosureHolder {
            executor: None,
            storage: ClosureStorage::Static(MaybeUninit::<StaticClosureStorage>::uninit()),
        }
    }

    /// Creates a holder which contains the closure `f` for later execution.
    ///
    /// # Safety
    ///
    /// The caller guarantees that the closure
    /// does not outlive its borrows, if any, until the following call to [`execute`].
    /// The caller guarantees that the following call to [`execute`] passes the correct argument type.
    ///
    /// [`execute`]: #method.execute
    pub(super) unsafe fn new<'any, F, ARG>(f: F) -> Self
    where
        F: FnOnce(&ARG) + 'any,
    {
        let mut result = ClosureHolder::empty();
        result.store(f);
        result
    }

    /// Stores the closure `f` in the holder for later execution.
    ///
    /// # Safety
    ///
    /// The caller guarantees that the closure
    /// does not outlive its borrows, if any, until the following call to [`execute`].
    /// The caller guarantees that the following call to [`execute`] passes the correct argument type.
    ///
    /// # Panics
    ///
    /// Panics if the holder already contains a closure.
    ///
    /// [`execute`]: #method.execute
    pub(super) unsafe fn store<'any, F, ARG>(&mut self, f: F)
    where
        F: FnOnce(&ARG) + 'any,
    {
        assert!(!self.is_some());

        let size = mem::size_of::<F>();

        if size > CLOSURE_STORAGE_SIZE {
            let storage = vec![0u8; size].into_boxed_slice();
            let ptr = &*storage as *const _ as *mut F;

            ptr::write(ptr, f);

            self.storage = ClosureStorage::Dynamic(Some(storage));
        } else {
            let mut storage = MaybeUninit::<StaticClosureStorage>::uninit();
            let ptr = storage.as_mut_ptr() as *mut F;

            ptr::write(ptr, f);

            self.storage = ClosureStorage::Static(storage);
        }

        let executor = |storage: &[u8], arg: *const ()| {
            let arg: &ARG = mem::transmute(arg);

            let f = ptr::read::<F>(storage.as_ptr() as *const F);

            f(arg);
        };

        self.executor = Some(executor);
    }

    /// If the holder is not empty, returns `true`; otherwise returns `false`.
    pub(super) fn is_some(&self) -> bool {
        self.executor.is_some()
    }

    /// If the holder is not empty, executes the stored closure and returns `true`; otherwise returns `false`.
    ///
    /// # Safety
    ///
    /// The caller guarantees that the call to passes the same argument type
    /// as the obe used in the previous call to [`new`] \ [`store`].
    ///
    /// [`new`]: #method.new
    /// [`store`]: #method.store
    #[cfg(test)]
    pub(super) unsafe fn try_execute<'any, ARG>(&mut self, arg: &'any ARG) -> bool {
        if self.is_some() {
            self.execute(arg);
            true
        } else {
            false
        }
    }

    /// Executes the stored closure unconditionally.
    ///
    /// # Safety
    ///
    /// The caller guarantees that the call to passes the same argument type
    /// as the obe used in the previous call to [`new`] \ [`store`].
    ///
    /// # Panics
    ///
    /// Panics if the holder is empty.
    ///
    /// [`new`]: #method.new
    /// [`store`]: #method.store
    pub(super) unsafe fn execute<'any, ARG>(&mut self, arg: &'any ARG) {
        let arg: *const () = mem::transmute(arg);

        match &mut self.storage {
            ClosureStorage::Static(storage) => {
                let storage = &*storage.as_ptr();
                (self
                    .executor
                    .take()
                    .expect("Tried to execute an empty closure."))(storage, arg);
            }
            ClosureStorage::Dynamic(storage) => {
                let storage = &*storage.take().expect("Tried to execute an empty closure.");
                (self
                    .executor
                    .take()
                    .expect("Tried to execute an empty closure."))(storage, arg);
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty() {
        let mut h = ClosureHolder::empty();
        let arg = 7usize;

        assert!(!h.is_some());

        unsafe {
            assert!(!h.try_execute(&arg));
        }
    }

    #[test]
    #[should_panic(expected = "Tried to execute an empty closure.")]
    fn empty_execute() {
        let mut h = ClosureHolder::empty();
        let arg = 7usize;

        assert!(!h.is_some());

        unsafe {
            assert!(!h.try_execute(&arg));
            h.execute(&arg);
        }
    }

    #[test]
    fn basic() {
        // Move closure.
        let x = 7;
        let mut y = 0;

        let mut h = unsafe {
            ClosureHolder::new(move |_arg: &usize| {
                y = x + 24;
                assert_eq!(x, 7);
                assert_eq!(y, 7 + 24);
            })
        };

        let arg = 7usize;

        assert!(h.is_some());

        unsafe {
            h.execute(&arg);
        }

        assert!(!h.is_some());

        unsafe {
            assert!(!h.try_execute(&arg));
        }

        assert_eq!(x, 7);
        assert_eq!(y, 0);

        unsafe {
            h.store(move |_arg: &usize| {
                y = x + 9;
                assert_eq!(x, 7);
                assert_eq!(y, 7 + 9);
            });
        }

        assert!(h.is_some());

        unsafe {
            h.execute(&arg);
        }

        assert!(!h.is_some());

        unsafe {
            assert!(!h.try_execute(&arg));
        }

        assert_eq!(x, 7);
        assert_eq!(y, 0);

        // Static closure.
        static HELLO: &'static str = "Hello";
        static FORTY_TWO: usize = 42;

        let mut g = unsafe {
            ClosureHolder::new(|_arg: &usize| {
                println!("{} {}", HELLO, FORTY_TWO);
            })
        };

        assert!(g.is_some());

        unsafe {
            g.execute(&arg);
        }

        assert!(!g.is_some());

        unsafe {
            assert!(!g.try_execute(&arg));

            g.store(|_arg: &usize| {
                println!("{} {}", "Hello", 42);
            });
        }

        assert!(g.is_some());

        unsafe {
            g.execute(&arg);
        }

        assert!(!g.is_some());

        unsafe {
            assert!(!g.try_execute(&arg));
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn storage_overflow() {
        let large_capture = [0u8; CLOSURE_HOLDER_SIZE];

        let _h =
            unsafe { ClosureHolder::new(move |_arg: &usize| println!("{}", large_capture.len())) };
    }
}
