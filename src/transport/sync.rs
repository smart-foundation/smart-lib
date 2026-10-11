//! A minimal, correct mutual-exclusion primitive for `no_std` + `alloc`.
//!
//! The transport inboxes (InProcess) and the per-channel write scratch
//! (UnixSocket/Tcp) need shared mutability through `&self`. `std` is not
//! available in the core profile (charter law #1) and no synchronization
//! crate is in the dependency graph (the skeleton's graph is `capnp` +
//! `capnpc` only), so the seam carries this ~40-line spin mutex.
//!
//! The guarantee we need is narrow and stated here honestly: critical
//! sections are a handful of pointer moves (queue push/pop, buffer reuse),
//! never I/O, never allocation except the queue's own growth. The mutex
//! spins only under contention. The InProcess inbox stress test
//! (`tests/inprocess.rs::inbox_thread_stress`) exercises it from many
//! threads; the `std`-feature builds could swap this for `std::sync::Mutex`
//! without changing a line of transport code.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// A spin mutex over `T`. `T` must be `Send` for the mutex to be `Sync`.
pub struct Mutex<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: access to `data` is exclusive while `locked` is true, and the
// lock/unlock pair establishes happens-before between successive holders.
unsafe impl<T: Send> Send for Mutex<T> {}
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    /// A new mutex around `data`.
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// Acquire the lock, spinning (with a spin-loop hint) until it is free.
    /// Acquire the lock (see the module docs for the guarantee).
    pub fn lock(&self) -> MutexGuard<'_, T> {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        MutexGuard { mutex: self }
    }

    /// Consume the mutex and return the inner value.
    /// Consume the mutex and return the inner value.
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

/// RAII guard returned by [`Mutex::lock`].
pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard exists exactly while the lock is held.
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard exists exactly while the lock is held.
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.locked.store(false, Ordering::Release);
    }
}
