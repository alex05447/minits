use std::marker::PhantomData;
use std::mem;

/// Holds an unsafe reference (raw pointer) to an object of type `T`.
/// It's up to the user to ensure the `ReferenceHolder` lives longer than the referenced value.
#[derive(Copy)]
pub(super) struct ReferenceHolder<T> {
    pub(super) reference: usize,
    _marker: PhantomData<T>,
}

impl<T> Clone for ReferenceHolder<T> {
    fn clone(&self) -> Self {
        Self {
            reference: self.reference,
            _marker: PhantomData,
        }
    }
}

impl<T> ReferenceHolder<T> {
    #[cfg(test)]
    pub(super) fn empty() -> Self {
        Self {
            reference: 0,
            _marker: PhantomData,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.reference == 0
    }

    pub(super) unsafe fn from_ref(h: &T) -> Self {
        Self {
            reference: mem::transmute(h),
            _marker: PhantomData,
        }
    }

    #[cfg(feature = "asyncio")]
    pub(super) unsafe fn from_address(address: usize) -> Self {
        Self {
            reference: mem::transmute(address),
            _marker: PhantomData,
        }
    }

    /// Using the referenced value when it is out of scope is UB.
    pub(super) unsafe fn as_ref(&self) -> &T {
        debug_assert!(!self.is_empty(), "Tried to deref an empty ReferenceHolder.");
        mem::transmute(self.reference)
    }

    pub(super) unsafe fn as_address(&self) -> usize {
        debug_assert!(!self.is_empty(), "Tried to deref an empty ReferenceHolder.");
        self.reference
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "Tried to deref an empty ReferenceHolder.")]
    fn empty() {
        let r = ReferenceHolder::<usize>::empty();
        let _v = unsafe { r.as_ref() };
    }

    #[test]
    fn non_empty() {
        let x = 7;
        let r = unsafe { ReferenceHolder::from_ref(&x) };
        unsafe {
            assert_eq!(*r.as_ref(), x);
        }

        fn nested_func(r: ReferenceHolder<usize>) {
            let r = r;
            unsafe {
                assert_eq!(*r.as_ref(), 7);
            }
        }

        nested_func(r);
    }
}
