//! bionic synchronisation primitives whose layout differs from glibc's.
//!
//! Most of bionic's pthread types happen to match glibc on x86-64 and can be
//! passed straight through:
//!
//! | type                  | bionic | glibc | |
//! |-----------------------|-------:|------:|-|
//! | `pthread_mutex_t`     |     40 |    40 | passthrough |
//! | `pthread_rwlock_t`    |     56 |    56 | passthrough |
//! | `pthread_attr_t`      |     56 |    56 | passthrough — layouts differ, but Roblox only ever hands the struct back to us |
//! | `pthread_once_t`      |      4 |     4 | passthrough |
//! | `pthread_key_t`       |      4 |     4 | passthrough |
//! | **`pthread_cond_t`**  | **32** |**48** | **wrapped** |
//! | **`sem_t`**           | **16** |**32** | **wrapped** |
//!
//! The two mismatches are not cosmetic. `pthread_cond_init` on a bionic-sized
//! condition variable writes 16 bytes past the end of the object, and Roblox
//! initialises condition variables during static construction — so the damage
//! lands early, in whatever happens to be adjacent, and surfaces much later as
//! an allocator aborting on a corrupted size. That is exactly the failure this
//! module exists to prevent.
//!
//! The technique is the usual one: treat the bionic-sized object as a handle,
//! and keep the real glibc object on the heap behind a pointer stored inside it.
//! Initialisation is lazy because bionic's `PTHREAD_COND_INITIALIZER` is all
//! zeroes, so a statically-initialised condition variable can reach `wait`
//! without `init` ever being called.

use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Marks a wrapper whose backing object exists. Arbitrary, but distinctive in a
/// memory dump and impossible to reach by zero-initialisation.
const READY: u64 = 0xC0D1A1_C0FFEE;

const UNINIT: u64 = 0;
const INITIALISING: u64 = 1;

/// bionic's `pthread_cond_t`: `int64_t __private[4]`.
#[repr(C)]
struct BionicCond {
    state: AtomicU64,
    real: AtomicUsize,
    _reserved: [u64; 2],
}

/// bionic's `sem_t`: `unsigned int count; int __reserved[3];`
#[repr(C)]
struct BionicSem {
    state: AtomicU64,
    real: AtomicUsize,
}

// glibc's implementations. These resolve to the host's libc at link time; our
// own wrappers are never exported, so there is no recursion.
extern "C" {
    fn pthread_cond_init(cond: *mut c_void, attr: *const c_void) -> c_int;
    fn pthread_cond_destroy(cond: *mut c_void) -> c_int;
    fn pthread_cond_wait(cond: *mut c_void, mutex: *mut c_void) -> c_int;
    fn pthread_cond_timedwait(cond: *mut c_void, mutex: *mut c_void, ts: *const c_void) -> c_int;
    fn pthread_cond_signal(cond: *mut c_void) -> c_int;
    fn pthread_cond_broadcast(cond: *mut c_void) -> c_int;

    fn sem_init(sem: *mut c_void, pshared: c_int, value: u32) -> c_int;
    fn sem_destroy(sem: *mut c_void) -> c_int;
    fn sem_post(sem: *mut c_void) -> c_int;
    fn sem_wait(sem: *mut c_void) -> c_int;
    fn sem_trywait(sem: *mut c_void) -> c_int;
}

/// Size of the heap allocation standing in for a glibc object. Generous on
/// purpose: it costs nothing and removes any chance of repeating the very bug
/// this module fixes if a libc grows its type.
const BACKING_SIZE: usize = 128;

fn alloc_backing() -> *mut c_void {
    let boxed: Box<[u8; BACKING_SIZE]> = Box::new([0u8; BACKING_SIZE]);
    Box::into_raw(boxed) as *mut c_void
}

/// SAFETY: `ptr` must have come from `alloc_backing` and not been freed.
unsafe fn free_backing(ptr: *mut c_void) {
    drop(Box::from_raw(ptr as *mut [u8; BACKING_SIZE]));
}

/// Resolve the real object behind a wrapper, creating it on first use.
///
/// `init` is called exactly once, with the freshly allocated backing store.
///
/// SAFETY: `state` and `real` must belong to the same live wrapper object.
unsafe fn resolve(
    state: &AtomicU64,
    real: &AtomicUsize,
    init: impl FnOnce(*mut c_void),
) -> *mut c_void {
    loop {
        match state.compare_exchange(UNINIT, INITIALISING, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {
                let backing = alloc_backing();
                init(backing);
                real.store(backing as usize, Ordering::Release);
                state.store(READY, Ordering::Release);
                return backing;
            }
            Err(READY) => return real.load(Ordering::Acquire) as *mut c_void,
            Err(INITIALISING) => {
                // Another thread is between allocation and publication. This
                // window is a handful of instructions.
                std::hint::spin_loop();
            }
            Err(_) => {
                // Not a value we wrote. The object was not zero-initialised and
                // is not one of ours — most likely a real bug in the caller, but
                // treating it as uninitialised would leak and corrupt. Refuse.
                return std::ptr::null_mut();
            }
        }
    }
}

// ---------------------------------------------------------------- condition vars

pub extern "C" fn cond_init(cond: *mut c_void, attr: *const c_void) -> c_int {
    if cond.is_null() {
        return libc_einval();
    }
    // SAFETY: bionic's contract is a pointer to a 32-byte pthread_cond_t.
    let c = unsafe { &mut *(cond as *mut BionicCond) };
    // An explicit init on an object we already wrapped replaces it, matching
    // glibc's "undefined behaviour, but do something sane" posture.
    unsafe { destroy_backing(&c.state, &c.real, pthread_cond_destroy) };
    c.state.store(UNINIT, Ordering::Release);
    let backing = unsafe {
        resolve(&c.state, &c.real, |p| {
            pthread_cond_init(p, attr);
        })
    };
    if backing.is_null() {
        libc_einval()
    } else {
        0
    }
}

pub extern "C" fn cond_destroy(cond: *mut c_void) -> c_int {
    if cond.is_null() {
        return libc_einval();
    }
    // SAFETY: as above.
    let c = unsafe { &mut *(cond as *mut BionicCond) };
    unsafe { destroy_backing(&c.state, &c.real, pthread_cond_destroy) };
    0
}

/// Tear down and free a wrapper's backing object, if it has one.
///
/// SAFETY: `state`/`real` must belong to the same live wrapper.
unsafe fn destroy_backing(
    state: &AtomicU64,
    real: &AtomicUsize,
    destroy: unsafe extern "C" fn(*mut c_void) -> c_int,
) {
    if state.swap(UNINIT, Ordering::AcqRel) == READY {
        let p = real.swap(0, Ordering::AcqRel) as *mut c_void;
        if !p.is_null() {
            destroy(p);
            free_backing(p);
        }
    }
}

macro_rules! cond_op {
    ($name:ident, $glibc:ident) => {
        pub extern "C" fn $name(cond: *mut c_void) -> c_int {
            if cond.is_null() {
                return libc_einval();
            }
            // SAFETY: bionic's contract is a pointer to a 32-byte pthread_cond_t.
            let c = unsafe { &mut *(cond as *mut BionicCond) };
            let backing = unsafe {
                resolve(&c.state, &c.real, |p| {
                    pthread_cond_init(p, std::ptr::null());
                })
            };
            if backing.is_null() {
                return libc_einval();
            }
            unsafe { $glibc(backing) }
        }
    };
}

cond_op!(cond_signal, pthread_cond_signal);
cond_op!(cond_broadcast, pthread_cond_broadcast);

pub extern "C" fn cond_wait(cond: *mut c_void, mutex: *mut c_void) -> c_int {
    let Some(backing) = cond_backing(cond) else {
        return libc_einval();
    };
    // SAFETY: `mutex` is a bionic pthread_mutex_t, which is layout-identical to
    // glibc's on x86-64 (both 40 bytes) and so passes straight through.
    unsafe { pthread_cond_wait(backing, mutex) }
}

pub extern "C" fn cond_timedwait(
    cond: *mut c_void,
    mutex: *mut c_void,
    abstime: *const c_void,
) -> c_int {
    let Some(backing) = cond_backing(cond) else {
        return libc_einval();
    };
    // SAFETY: as above; `struct timespec` is identical between the two libcs.
    unsafe { pthread_cond_timedwait(backing, mutex, abstime) }
}

fn cond_backing(cond: *mut c_void) -> Option<*mut c_void> {
    if cond.is_null() {
        return None;
    }
    // SAFETY: bionic's contract is a pointer to a 32-byte pthread_cond_t.
    let c = unsafe { &mut *(cond as *mut BionicCond) };
    let backing = unsafe {
        resolve(&c.state, &c.real, |p| {
            pthread_cond_init(p, std::ptr::null());
        })
    };
    (!backing.is_null()).then_some(backing)
}

// ------------------------------------------------------------------ semaphores

pub extern "C" fn semaphore_init(sem: *mut c_void, pshared: c_int, value: u32) -> c_int {
    if sem.is_null() {
        return libc_einval();
    }
    // SAFETY: bionic's contract is a pointer to a 16-byte sem_t.
    let s = unsafe { &mut *(sem as *mut BionicSem) };
    unsafe { destroy_backing(&s.state, &s.real, sem_destroy) };
    s.state.store(UNINIT, Ordering::Release);
    let backing = unsafe {
        resolve(&s.state, &s.real, |p| {
            sem_init(p, pshared, value);
        })
    };
    if backing.is_null() {
        libc_einval()
    } else {
        0
    }
}

pub extern "C" fn semaphore_destroy(sem: *mut c_void) -> c_int {
    if sem.is_null() {
        return libc_einval();
    }
    // SAFETY: as above.
    let s = unsafe { &mut *(sem as *mut BionicSem) };
    unsafe { destroy_backing(&s.state, &s.real, sem_destroy) };
    0
}

macro_rules! sem_op {
    ($name:ident, $glibc:ident) => {
        pub extern "C" fn $name(sem: *mut c_void) -> c_int {
            if sem.is_null() {
                return libc_einval();
            }
            // SAFETY: bionic's contract is a pointer to a 16-byte sem_t.
            let s = unsafe { &mut *(sem as *mut BionicSem) };
            // Unlike condition variables a semaphore has no static initialiser,
            // so reaching here uninitialised means sem_init was skipped. Create
            // a zero-count semaphore rather than crashing.
            let backing = unsafe {
                resolve(&s.state, &s.real, |p| {
                    sem_init(p, 0, 0);
                })
            };
            if backing.is_null() {
                return libc_einval();
            }
            unsafe { $glibc(backing) }
        }
    };
}

sem_op!(semaphore_post, sem_post);
sem_op!(semaphore_wait, sem_wait);
sem_op!(semaphore_trywait, sem_trywait);

fn libc_einval() -> c_int {
    22 // EINVAL, identical in both libcs
}

/// Everything this module replaces.
pub fn overrides() -> Vec<(&'static str, *mut c_void)> {
    macro_rules! f {
        ($name:literal, $fn:expr) => {
            ($name, $fn as *const () as *mut c_void)
        };
    }
    vec![
        f!("pthread_cond_init", cond_init),
        f!("pthread_cond_destroy", cond_destroy),
        f!("pthread_cond_wait", cond_wait),
        f!("pthread_cond_timedwait", cond_timedwait),
        f!("pthread_cond_signal", cond_signal),
        f!("pthread_cond_broadcast", cond_broadcast),
        f!("sem_init", semaphore_init),
        f!("sem_destroy", semaphore_destroy),
        f!("sem_post", semaphore_post),
        f!("sem_wait", semaphore_wait),
        f!("sem_trywait", semaphore_trywait),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_sizes_match_bionic() {
        // The whole point: these must be the *bionic* sizes, not glibc's.
        assert_eq!(std::mem::size_of::<BionicCond>(), 32);
        assert_eq!(std::mem::size_of::<BionicSem>(), 16);
    }

    #[test]
    fn statically_initialised_cond_works() {
        // bionic's PTHREAD_COND_INITIALIZER is all zeroes; signalling one that
        // was never explicitly initialised must still work.
        // u64, not u8: the wrapper's first field is an AtomicU64, so byte
        // storage is under-aligned and the cast is UB. Real conds come from the
        // engine's own allocations and are always aligned.
        let mut storage = [0u64; 4];
        let cond = storage.as_mut_ptr() as *mut c_void;
        assert_eq!(cond_signal(cond), 0);
        assert_eq!(cond_broadcast(cond), 0);
        assert_eq!(cond_destroy(cond), 0);
    }

    #[test]
    fn init_destroy_roundtrip_does_not_leak_state() {
        let mut storage = [0u64; 4];
        let cond = storage.as_mut_ptr() as *mut c_void;
        assert_eq!(cond_init(cond, std::ptr::null()), 0);
        assert_eq!(cond_destroy(cond), 0);
        // Destroyed wrappers return to the zero state, so they can be reused.
        assert_eq!(cond_init(cond, std::ptr::null()), 0);
        assert_eq!(cond_destroy(cond), 0);
    }

    #[test]
    fn semaphore_counts() {
        let mut storage = [0u64; 2];
        let sem = storage.as_mut_ptr() as *mut c_void;
        assert_eq!(semaphore_init(sem, 0, 1), 0);
        assert_eq!(semaphore_wait(sem), 0); // consumes the one permit
        assert_ne!(semaphore_trywait(sem), 0); // none left
        assert_eq!(semaphore_post(sem), 0);
        assert_eq!(semaphore_trywait(sem), 0);
        assert_eq!(semaphore_destroy(sem), 0);
    }
}
