use std::ptr;
use std::mem::MaybeUninit;
use std::sync::atomic::{ AtomicUsize, Ordering };

#[repr(usize)]
enum SingletonState {
    Uninitialized = 0,
    Initializing = 1,
    Initialized = 2,
    Uninitializing = 3,
}

pub struct Singleton<T>
    where T : Send + Sync
{
    value :MaybeUninit<T>,
    state :AtomicUsize,
}

impl<T> Singleton<T>
    where T : Send + Sync
{
    pub fn uninitialized() -> Self {
        Self {
            value : MaybeUninit::uninit(),
            state : AtomicUsize::new(SingletonState::Uninitialized as _),
        }
    }

    pub fn initialize<F>(&self, f :F)
        where F : FnOnce() -> T
    {
        let state = self.state.compare_and_swap(
            SingletonState::Uninitialized as _,
            SingletonState::Initializing as _,
            Ordering::SeqCst,
        );

        // We are the first thread to start initializing the singleton.
        if state == SingletonState::Uninitialized as _ {
            self.initialize_impl(f);

        // The singleton is already being finalized by some other thread.
        } else if state == SingletonState::Uninitializing as _ {

        // The singleton is already being initialized / was already initialized.
        } else if state == SingletonState::Initializing as _ || state == SingletonState::Initialized as _ {
            panic!("Singleton initialized more than once.");
        }
    }

    pub fn finalize(&self) {
        let state = self.state.compare_and_swap(
            SingletonState::Initialized as _,
            SingletonState::Uninitializing as _,
            Ordering::SeqCst,
        );

        // We are the first thread to start finalizing the singleton.
        if state == SingletonState::Initialized as _ {
            self.finalize_impl();

        // The singleton is already being initialized by some other thread.
        } else if state == SingletonState::Initializing as _ {

        // The singleton is already being finalized / was already finalized.
        } else if state == SingletonState::Uninitializing as _ || state == SingletonState::Uninitialized as _ {
            panic!("Singleton finalized more than once.");
        }
    }

    pub fn get(&self) -> &T {
        debug_assert!(self.state.load(Ordering::SeqCst) == SingletonState::Initialized as _, "Tried to access an uninitialized singleton.");

        unsafe {
            &*self.value.as_ptr()
        }
    }

    pub fn get_mut(&mut self) -> &mut T {
        debug_assert!(self.state.load(Ordering::SeqCst) == SingletonState::Initialized as _, "Tried to access an uninitialized singleton.");

        unsafe {
            &mut *self.value.as_mut_ptr()
        }
    }

    fn initialize_impl<F>(&self, f :F)
        where F : FnOnce() -> T
    {
        unsafe {
            ptr::write( self.value.as_ptr() as *mut _, f() );
        }

        let state = self.state.compare_and_swap(
            SingletonState::Initializing as _,
            SingletonState::Initialized as _,
            Ordering::SeqCst,
        );

        if state != SingletonState::Initializing as _ {
            panic!("Error during singleton initialization - finalize() called prematurely?");
        }
    }

    fn finalize_impl(&self) {
        let _ = unsafe { ptr::read::<T>( self.value.as_ptr() ) };

        let state = self.state.compare_and_swap(
            SingletonState::Uninitializing as _,
            SingletonState::Uninitialized as _,
            Ordering::SeqCst,
        );

        if state != SingletonState::Uninitializing as _ {
            panic!("Error during singleton finalization - initialize() called again?");
        }
    }
}

impl<T> Drop for Singleton<T>
    where T : Send + Sync
{
    fn drop(&mut self) {
        self.finalize();
    }
}

use std::sync::Once;

pub(crate) struct Singleton2<T> {
    ptr :*mut T,
    once :Once,
}

impl<T> Singleton2<T> {
    pub(crate) const fn new() -> Self {
        Self {
            ptr : 0 as *mut _,
            once : Once::new(),
        }
    }
}