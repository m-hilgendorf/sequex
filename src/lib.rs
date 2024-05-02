use std::{
    cell::UnsafeCell,
    fmt,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    }, thread, time::Duration,
};

/// A sequence-mutex lock, which guarantees locks are acquired in the order in which they
/// were constructed, as opposed to the order in which locks are requested.
pub struct Sequex<T> {
    ticket: u64,
    num_tickets: u64,
    shared: Arc<Shared<T>>,
}

/// An RAII guard that releases the lock when dropped.
pub struct Guard<'a, T> {
    sequex: &'a Sequex<T>,
}

/// An error returned when attempting to acquire a lock that was poisoned.
#[derive(Debug)]
pub struct SequexPoisoned;

// Shared state of the lock.
struct Shared<T> {
    current: AtomicU64,
    value: UnsafeCell<T>,
}

// Marker value to represent when the resource is locked.
const LOCKED: u64 = u64::MAX;

// Marker value to represent when the resource is poisoned.
const POISON: u64 = u64::MAX - 1;

impl<T> Sequex<T> {
    /// Create a new sequence that wrap an internval value.
    pub fn new(value: T, num_tickets: u64) -> Vec<Self> {
        let shared = Arc::new(Shared {
            current: AtomicU64::new(0),
            value: UnsafeCell::new(value),
        });
        (0..num_tickets)
            .map(|ticket| {
                let shared = shared.clone();
                Self {
                    ticket,
                    num_tickets,
                    shared,
                }
            })
            .collect()
    }

    /// Attempt to acquire the lock. Does not block the current thread if the lock could
    /// not be acquired. Returns [SequencePoisoned] if the lock was poisoned.
    pub fn try_lock(&self) -> Result<Option<Guard<'_, T>>, SequexPoisoned> {
        match self.shared.current.compare_exchange(
            self.ticket,
            LOCKED,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => Ok(Some(Guard { sequex: self })),
            Err(current) if current == POISON => Err(SequexPoisoned),
            Err(_) => Ok(None),
        }
    }

    /// Acquire a lock, blocking the current thread if it could not be acquired. Returns a
    /// [SequencePoisoned] if the lock was poisoned.
    pub fn lock(&self) -> Result<Guard<'_, T>, SequexPoisoned> {
        let mut backoff = 100;
        loop {
            if let Some(guard) = self.try_lock()? {
                return Ok(guard);
            }
            thread::park_timeout(Duration::from_micros(backoff));
            backoff *= 2;
        }
    }
}

impl<T> Drop for Sequex<T> {
    fn drop(&mut self) {
        self.shared.current.store(POISON, Ordering::SeqCst);
    }
}

impl<'a, T> Drop for Guard<'a, T> {
    fn drop(&mut self) {
        let next = (self.sequex.ticket + 1) % self.sequex.num_tickets;
        self.sequex
            .shared
            .current
            .compare_exchange(LOCKED, next, Ordering::SeqCst, Ordering::SeqCst)
            .ok();
    }
}

impl fmt::Display for SequexPoisoned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sequex poisoned")
    }
}

impl<'a, T> Deref for Guard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.sequex.shared.value.get() }
    }
}

impl<'a, T> DerefMut for Guard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.sequex.shared.value.get() }
    }
}
