/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Internal mutex interface.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Environment, ThreadId};
use crate::libc::errno::{EBUSY, EDEADLK, EPERM};
use crate::parallel_jit::{
    decode_mutex_lock, encode_mutex_lock, set_fast_mutex_kind, FAST_MUTEXES, FAST_MUTEX_COUNT,
};

/// Stores and manages mutexes. Note that all the methods for locking and
/// unlocking mutexes are on [Environment] instead, because they interact with
/// threads.
#[derive(Default)]
pub struct MutexState {
    // TODO?: Maybe this should be a Vec instead? It would be bad if there were
    // many mutexes over the lifetime of an application, but it would perform
    // better. Maybe it could also be a fixed size allocator? (although that
    // seems a little overkill)
    mutexes: HashMap<MutexId, Mutex>,
    // Hopefully there will never be more than 2^64 mutexes in an application's
    // lifetime :P
    mutex_count: u64,
}

/// Unique identifier for mutexes, used for mutexes held by host objects and
/// guest pthread mutexes.
pub type MutexId = u64;

struct Mutex {
    type_: MutexType,
    waiting_count: u32,
    /// The current lock. Workers can change this for mutexes with low IDs, see
    /// [crate::parallel_jit].
    lock: &'static AtomicU64,
}

impl Mutex {
    /// The `NonZeroU32` is the number of locks on this thread (if it's a
    /// recursive mutex).
    fn locked(&self) -> Option<(ThreadId, NonZeroU32)> {
        decode_mutex_lock(self.lock.load(Ordering::Acquire))
            .map(|(thread, count)| (thread, NonZeroU32::new(count).unwrap()))
    }

    fn set_locked(&self, locked: Option<(ThreadId, NonZeroU32)>) {
        let word = locked.map_or(0, |(thread, count)| encode_mutex_lock(thread, count.get()));
        self.lock.store(word, Ordering::Release);
    }

    /// Lock an unlocked mutex. This can fail if a worker gets there first.
    fn try_lock(&self, thread: ThreadId) -> bool {
        let word = encode_mutex_lock(thread, 1);
        self.lock
            .compare_exchange(0, word, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }
}

#[repr(i32)]
#[derive(Debug, PartialEq, Copy, Clone)]
#[allow(non_camel_case_types)]
pub enum MutexType {
    PTHREAD_MUTEX_NORMAL = 0,
    PTHREAD_MUTEX_ERRORCHECK = 1,
    PTHREAD_MUTEX_RECURSIVE = 2,
}

impl TryFrom<i32> for MutexType {
    type Error = &'static str;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(MutexType::PTHREAD_MUTEX_NORMAL),
            1 => Ok(MutexType::PTHREAD_MUTEX_ERRORCHECK),
            2 => Ok(MutexType::PTHREAD_MUTEX_RECURSIVE),
            _ => Err("Value is not a valid mutex type!"),
        }
    }
}
pub const PTHREAD_MUTEX_DEFAULT: MutexType = MutexType::PTHREAD_MUTEX_NORMAL;

impl MutexState {
    /// Initializes a mutex and returns a handle to it. Similar to
    /// `pthread_mutex_init`, but for host code.
    pub fn init_mutex(&mut self, mutex_type: MutexType) -> MutexId {
        let mutex_id = self.mutex_count;
        self.mutex_count = self.mutex_count.checked_add(1).unwrap();
        self.mutexes.insert(
            mutex_id,
            Mutex {
                type_: mutex_type,
                waiting_count: 0,
                lock: match usize::try_from(mutex_id) {
                    Ok(index) if index < FAST_MUTEX_COUNT => &FAST_MUTEXES[index],
                    _ => Box::leak(Box::new(AtomicU64::new(0))),
                },
            },
        );
        set_fast_mutex_kind(
            mutex_id,
            true,
            mutex_type == MutexType::PTHREAD_MUTEX_RECURSIVE,
        );
        log_dbg!("Created mutex #{}, type {:?}", mutex_id, mutex_type);
        mutex_id
    }

    /// Destroys a mutex and returns an error on failure (as errno). Similar to
    /// `pthread_mutex_destroy`, but for host code. Note that the mutex is not
    /// destroyed on an Err return.
    pub fn destroy_mutex(&mut self, mutex_id: MutexId) -> Result<(), i32> {
        let mutex = self.mutexes.get_mut(&mutex_id).unwrap();
        if mutex.locked().is_some() {
            log_dbg!("Attempted to destroy currently locked mutex, returning EBUSY!");
            return Err(EBUSY);
        } else if mutex.waiting_count != 0 {
            log_dbg!("Attempted to destroy mutex with waiting locks, returning EBUSY!");
            return Err(EBUSY);
        }
        // TODO?: If we switch to a vec-based system, we should reuse destroyed
        // ids if they are at the top of the stack.
        self.mutexes.remove(&mutex_id);
        set_fast_mutex_kind(mutex_id, false, false);
        Ok(())
    }

    /// Whether any thread is blocked waiting to lock the mutex.
    pub fn mutex_has_waiters(&self, mutex_id: MutexId) -> bool {
        self.mutexes
            .get(&mutex_id)
            .is_some_and(|mutex| mutex.waiting_count > 0)
    }

    pub fn mutex_is_locked(&self, mutex_id: MutexId) -> bool {
        self.mutexes
            .get(&mutex_id)
            .is_some_and(|mutex| mutex.locked().is_some())
    }

    pub fn mutex_is_recursive(&self, mutex_id: MutexId) -> bool {
        self.mutexes
            .get(&mutex_id)
            .is_some_and(|mutex| mutex.type_ == MutexType::PTHREAD_MUTEX_RECURSIVE)
    }

    pub fn mutex_is_locked_by(&self, mutex_id: MutexId, thread: ThreadId) -> bool {
        self.mutexes.get(&mutex_id).is_some_and(|mutex| {
            if let Some((lock_thread, _)) = mutex.locked() {
                lock_thread == thread
            } else {
                false
            }
        })
    }
}

impl Environment {
    /// Relock mutex that was just unblocked. This should probably only be used
    /// by the thread scheduler. It returns [false] if a worker locked the mutex
    /// first, in which case the thread remains blocked.
    pub fn relock_unblocked_mutex_for_thread(
        &mut self,
        thread_id: ThreadId,
        mutex_id: MutexId,
    ) -> bool {
        let mutex: &mut _ = self.mutex_state.mutexes.get_mut(&mutex_id).unwrap();
        log_dbg!(
            "Relocking unblocked mutex {} for thread {}, waiting count {}",
            mutex_id,
            thread_id,
            mutex.waiting_count
        );
        if !mutex.try_lock(thread_id) {
            return false;
        }
        mutex.waiting_count = mutex.waiting_count.saturating_sub(1);
        true
    }

    /// Locks a mutex and returns the lock count or an error (as errno). Similar
    /// to `pthread_mutex_lock`, but for host code.
    pub fn lock_mutex(&mut self, mutex_id: MutexId) -> Result<u32, i32> {
        let current_thread = self.current_thread;
        let mutex: &mut _ = self.mutex_state.mutexes.get_mut(&mutex_id).unwrap();

        if mutex.try_lock(current_thread) {
            log_dbg!("Locked mutex #{} for thread {}.", mutex_id, current_thread);
            return Ok(1);
        }

        // A worker could unlock it right now, so this can't assume it's locked.
        let (locking_thread, lock_count) = mutex
            .locked()
            .unwrap_or((usize::MAX, NonZeroU32::new(1).unwrap()));

        if locking_thread == current_thread {
            match mutex.type_ {
                MutexType::PTHREAD_MUTEX_NORMAL => {
                    // This is a deadlock unless another thread unlocks the
                    // mutex, which is allowed on iPhone OS (see unlock_mutex)
                    // and which some apps rely on, using the mutex like a
                    // semaphore. So block like for any other locked mutex.
                    log_dbg!(
                        "Thread {} is locking non-error-checking mutex #{} it already locked, waiting for another thread to unlock it.",
                        current_thread,
                        mutex_id,
                    );
                }
                MutexType::PTHREAD_MUTEX_ERRORCHECK => {
                    log_dbg!("Attempted to lock error-checking mutex #{} for thread {}, already locked by same thread! Returning EDEADLK.", mutex_id, current_thread);
                    return Err(EDEADLK);
                }
                MutexType::PTHREAD_MUTEX_RECURSIVE => {
                    log_dbg!(
                        "Increasing lock level on recursive mutex #{}, currently locked by thread {}.",
                        mutex_id,
                        locking_thread,
                    );
                    mutex.set_locked(Some((locking_thread, lock_count.checked_add(1).unwrap())));
                    return Ok(lock_count.get() + 1);
                }
            }
        }

        // Add to the waiting count, so that the mutex isn't destroyed. This is
        // subtracted in relock_unblocked_mutex.
        mutex.waiting_count += 1;

        // Mutex is already locked, block thread until it isn't.
        self.block_on_mutex(mutex_id);
        // Lock count is always 1 after a thread-blocking lock.
        Ok(1)
    }

    /// Unlocks a mutex and returns the lock count or an error (as errno).
    /// Similar to `pthread_mutex_unlock`, but for host code.
    pub fn unlock_mutex(&mut self, mutex_id: MutexId) -> Result<u32, i32> {
        let current_thread = self.current_thread;
        let mutex: &mut _ = self.mutex_state.mutexes.get_mut(&mutex_id).unwrap();

        let Some((locking_thread, lock_count)) = mutex.locked() else {
            match mutex.type_ {
                MutexType::PTHREAD_MUTEX_NORMAL => {
                    // This case is undefined, we may as well panic.
                    panic!(
                        "Attempted to unlock non-error-checking mutex #{mutex_id} for thread {current_thread}, already unlocked!",
                    );
                }
                MutexType::PTHREAD_MUTEX_ERRORCHECK | MutexType::PTHREAD_MUTEX_RECURSIVE => {
                    log_dbg!(
                        "Attempted to unlock error-checking or recursive mutex #{} for thread {}, already unlocked! Returning EPERM.",
                        mutex_id, current_thread,
                    );
                    return Err(EPERM);
                }
            }
        };

        if locking_thread != current_thread {
            match mutex.type_ {
                MutexType::PTHREAD_MUTEX_NORMAL => {
                    // This case is undefined,
                    // but tests on macOS/iOS shows it is allowed!
                    // Some apps (e.g. Monster Hunter Portable 2nd G) do this
                    // every frame, so only warn once.
                    log_once!(
                        "Warning: Allowing to unlock a non-error-checking mutex locked by a different thread!"
                    );
                    log_dbg!(
                        "Unlocking non-error-checking mutex #{mutex_id} for thread {current_thread}, locked by different thread {locking_thread}.",
                    );
                }
                MutexType::PTHREAD_MUTEX_ERRORCHECK | MutexType::PTHREAD_MUTEX_RECURSIVE => {
                    log_dbg!(
                        "Attempted to unlock error-checking or recursive mutex #{} for thread {}, locked by different thread {}! Returning EPERM.",
                        mutex_id, current_thread, locking_thread,
                    );
                    return Err(EPERM);
                }
            }
        }

        if lock_count.get() == 1 {
            log_dbg!(
                "Unlocked mutex #{} for thread {}.",
                mutex_id,
                current_thread
            );
            mutex.set_locked(None);
            Ok(0)
        } else {
            assert!(mutex.type_ == MutexType::PTHREAD_MUTEX_RECURSIVE);
            log_dbg!(
                "Decreasing lock level on recursive mutex #{} (count {}), currently locked by thread {}.",
                mutex_id,
                lock_count,
                locking_thread
            );
            mutex.set_locked(Some((
                locking_thread,
                NonZeroU32::new(lock_count.get() - 1).unwrap(),
            )));
            Ok(lock_count.get() - 1)
        }
    }
}
