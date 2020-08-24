use std::{
    marker::PhantomData,
    ptr::{null_mut, NonNull},
    sync::atomic::{AtomicPtr, Ordering},
};

/// A heap-allocated (atomic or not) (single-) linked list node.
struct LinkedListNode<T> {
    val: T,
    next: Option<NonNull<LinkedListNode<T>>>,
}

impl<T> LinkedListNode<T> {
    /// Allocates on the heap and returns a linked list node for `val`.
    fn new(val: T) -> NonNull<Self> {
        NonNull::new(Box::into_raw(Box::new(Self { val, next: None }))).unwrap()
    }
}

/// Atomic (single-) linked list / stack with heap-allocated nodes.
///
/// Filled / pushed onto by a number of threads atomically via [`push`](#method.push),
/// then emptied / popped atomically by a single thread via [`take`](#method.take).
pub(crate) struct AtomicLinkedList<T>(AtomicPtr<LinkedListNode<T>>);

impl<T> AtomicLinkedList<T> {
    pub(crate) fn new() -> Self {
        Self(AtomicPtr::new(null_mut()))
    }

    /// Adds / pushes the `val` to the linked list / stack atomically.
    ///
    /// Heap-allocates a node for the value and updates the list head atomically.
    pub(crate) fn push(&self, val: T) {
        let mut val = LinkedListNode::new(val);

        let mut head = self.0.load(Ordering::Relaxed);

        loop {
            unsafe { val.as_mut() }.next = NonNull::new(head);

            match self
                .0
                .compare_exchange(head, val.as_ptr(), Ordering::Release, Ordering::Relaxed)
            {
                Ok(_) => return,
                Err(actual_head) => head = actual_head,
            }
        }
    }

    /// Takes / pops all values off the list / stack atomically, leaving it empty.
    ///
    /// Returns a [`non-atomic list`](struct.LinkedList.html) instance which owns all of its nodes
    /// and is safe to iterate over in the calling thread
    /// (or `None` if the list was empty).
    pub(crate) fn take(&self) -> Option<LinkedList<T>> {
        self.take_head().map(|head| LinkedList::new(Some(head)))
    }

    fn take_head(&self) -> Option<NonNull<LinkedListNode<T>>> {
        NonNull::new(self.0.swap(null_mut(), Ordering::Acquire))
    }
}

impl<T> Drop for AtomicLinkedList<T> {
    fn drop(&mut self) {
        self.take().map(|list| list.into_iter()); // All values iterated and dropped here.
    }
}

/// Non-atomic, single threaded (single-) linked list / stack with heap-allocated nodes.
///
/// Returned by [`AtomicLinkedList::take`](struct.AtomicLinkedList.html#method.take).
pub(crate) struct LinkedList<T>(Option<NonNull<LinkedListNode<T>>>);

impl<T> LinkedList<T> {
    /// Returns an immutable iterator over the values in the list.
    pub(crate) fn iter(&self) -> LinkedListIter<'_, T> {
        LinkedListIter {
            head: self.0,
            _marker: PhantomData,
        }
    }

    fn new(head: Option<NonNull<LinkedListNode<T>>>) -> Self {
        Self(head)
    }
}

impl<T> Drop for LinkedList<T> {
    fn drop(&mut self) {
        LinkedListIntoIter(self.0); // All values iterated and dropped here.
    }
}

impl<T> IntoIterator for LinkedList<T> {
    type Item = T;
    type IntoIter = LinkedListIntoIter<T>;

    fn into_iter(mut self) -> Self::IntoIter {
        LinkedListIntoIter(self.0.take())
    }
}

/// Immutable iterator over the values in the list.
pub(crate) struct LinkedListIter<'a, T> {
    head: Option<NonNull<LinkedListNode<T>>>,
    _marker: PhantomData<&'a T>,
}

impl<'a, T> Clone for LinkedListIter<'a, T> {
    fn clone(&self) -> Self {
        Self {
            head: self.head,
            _marker: PhantomData,
        }
    }
}

impl<'a, T> Iterator for LinkedListIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(node) = self.head {
            self.head = unsafe { node.as_ref().next };

            let node = unsafe { &*node.as_ptr() };

            Some(&node.val)
        } else {
            None
        }
    }
}

/// Iterator which pops and returns the values from the linked list.
///
/// Frees the allocated node memory each iteration.
pub(crate) struct LinkedListIntoIter<T>(Option<NonNull<LinkedListNode<T>>>);

impl<T> Iterator for LinkedListIntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(node) = self.0 {
            self.0 = unsafe { node.as_ref().next };

            let boxed = unsafe { Box::from_raw(node.as_ptr()) };

            Some(boxed.val) // `boxed` node dropped here.
        } else {
            None
        }
    }
}

impl<T> Drop for LinkedListIntoIter<T> {
    fn drop(&mut self) {
        while let Some(_) = self.next() {} // All values iterated and dropped here.
    }
}
