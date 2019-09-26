use std::sync::Mutex;
use std::thread;

use minifiber::Fiber;

pub(super) struct FiberPool {
    pool: Mutex<Vec<Fiber>>,
}

pub(super) struct FiberPoolParams<F>
where
    F: FnOnce() + 'static,
{
    pub(super) num_fibers: usize,
    pub(super) stack_size: usize,
    pub(super) fiber_entry_point: F,
}

impl FiberPool {
    pub(super) fn empty() -> Self {
        Self {
            pool: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn initialize<F>(&mut self, params: FiberPoolParams<F>)
    where
        F: FnOnce() + Clone + 'static,
    {
        let mut pool = self.pool.lock().unwrap();

        debug_assert!(pool.len() == 0);
        pool.reserve_exact(params.num_fibers);

        for idx in 0..params.num_fibers {
            pool.push(Fiber::new(
                params.stack_size,
                Some(&format!("Worker fiber {}", idx)),
                params.fiber_entry_point.clone(),
            ));
        }
    }

    pub(super) fn get_fiber(&self, wait: bool) -> Option<Fiber> {
        if wait {
            loop {
                if let Some(fiber) = self.pool.lock().unwrap().pop() {
                    return Some(fiber);
                } else {
                    thread::yield_now();
                }
            }
        } else {
            self.pool.lock().unwrap().pop()
        }
    }

    pub(super) fn free_fiber(&self, f: Fiber) {
        self.pool.lock().unwrap().push(f);
    }
}
