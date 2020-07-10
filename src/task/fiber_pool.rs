use {
    minifiber::{Fiber, FiberEntryPoint},
    std::{
        sync::{Condvar, Mutex},
        time::Duration,
    },
};

struct LockedFiberPool {
    pool: Vec<Fiber>,
    // Number of threads of this type blocked / sleeping / waiting
    // for a new / resumed task to be added to this queue.
    num_waiters: u32,
}

impl LockedFiberPool {
    fn new() -> Self {
        Self {
            pool: Vec::new(),
            num_waiters: 0,
        }
    }

    fn inc_num_waiters(&mut self) {
        self.num_waiters += 1;
    }

    fn dec_num_waiters(&mut self) {
        debug_assert!(self.num_waiters > 0);
        self.num_waiters -= 1;
    }
}

/// A thread-safe free fiber pool.
/// Just a `Mutex`-protected `Vec` of `Fiber`'s.
pub(crate) struct FiberPool {
    wait_timeout: Duration,
    pool: Mutex<LockedFiberPool>,
    condvar: Condvar,
}

/// `FiberPool` initialization parameters.
pub(crate) struct FiberPoolParams<F: FiberEntryPoint> {
    /// Number of fibers to allocate.
    pub(crate) num_fibers: u32,
    /// Fiber stack size.
    pub(crate) stack_size: usize,
    /// Fiber entry point.
    pub(crate) entry_point: F,
}

impl FiberPool {
    /// Creates a new empty fiber pool.
    pub(crate) fn new(wait_timeout: Duration) -> Self {
        Self {
            wait_timeout,
            pool: Mutex::new(LockedFiberPool::new()),
            condvar: Condvar::new(),
        }
    }

    /// Initialize the fiber pool with `params`, allocating the fibers.
    /// NOTE - the user guarantees this is only called once, before the fiber pool is used.
    pub(crate) fn initialize<F: FiberEntryPoint + Clone>(&mut self, params: FiberPoolParams<F>) {
        debug_assert!(params.num_fibers > 0);

        let pool = &mut self.pool.lock().unwrap().pool;

        debug_assert!(pool.is_empty());
        pool.reserve_exact(params.num_fibers as _);

        for idx in 0..params.num_fibers {
            pool.push(
                Fiber::new(
                    params.stack_size,
                    format!("Worker fiber {}", idx),
                    params.entry_point.clone(),
                )
                .unwrap(),
            );
        }
    }

    /// Try to get a free fiber from the pool.
    ///
    /// If the pool is empty and `wait` is `true`, the method blocks / sleeps the calling thread
    /// until a fiber is freed to the pool and is successfully returned.
    pub(crate) fn get_fiber(&self, wait: bool) -> Option<Fiber> {
        let mut pool = self.pool.lock().unwrap();

        loop {
            if let Some(fiber) = pool.pool.pop() {
                return Some(fiber);
            }

            // No free fibers in the pool and we don't want to wait - return `None`.
            if !wait {
                return None;
            }

            // Otherwise wait.
            // Loop until we pop a free fiber.

            pool.inc_num_waiters();
            pool = self
                .condvar
                .wait_timeout(pool, self.wait_timeout)
                .unwrap()
                .0;
            pool.dec_num_waiters();
        }
    }

    /// Return a now-free fiber back to the fiber pool.
    ///
    /// Will wake up a single thread waiting for a free fiber, if there are any.
    pub(crate) fn free_fiber(&self, f: Fiber) {
        let mut pool = self.pool.lock().unwrap();

        pool.pool.push(f);

        if pool.num_waiters > 0 {
            self.condvar.notify_one();
        }
    }
}
