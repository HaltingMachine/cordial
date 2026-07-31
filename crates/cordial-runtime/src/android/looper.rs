//! `ALooper_*` — the per-thread event loop the engine polls.
//!
//! `GameActivity`'s native side runs a loop that calls `ALooper_pollOnce` and
//! dispatches whatever comes back: input, lifecycle, and whatever file
//! descriptors the app registered. It is not optional and it cannot be faked —
//! a stub that returns immediately turns the engine's main loop into a busy spin,
//! and one that never returns hangs it.
//!
//! Android's implementation is epoll plus an eventfd for wakeups, which is
//! exactly what is available here.

use std::cell::RefCell;
use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

// Return values from android/looper.h.
pub const POLL_WAKE: c_int = -1;
pub const POLL_CALLBACK: c_int = -2;
pub const POLL_TIMEOUT: c_int = -3;
pub const POLL_ERROR: c_int = -4;

// Event bits.
const EVENT_INPUT: c_int = 1 << 0;
const EVENT_OUTPUT: c_int = 1 << 1;
const EVENT_ERROR: c_int = 1 << 2;
const EVENT_HANGUP: c_int = 1 << 3;

const EPOLLIN: u32 = 0x001;
const EPOLLOUT: u32 = 0x004;
const EPOLLERR: u32 = 0x008;
const EPOLLHUP: u32 = 0x010;
const EPOLL_CTL_ADD: c_int = 1;
const EPOLL_CTL_DEL: c_int = 2;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct EpollEvent {
    events: u32,
    data: u64,
}

extern "C" {
    fn epoll_create1(flags: c_int) -> c_int;
    fn epoll_ctl(epfd: c_int, op: c_int, fd: c_int, event: *mut EpollEvent) -> c_int;
    fn epoll_wait(epfd: c_int, events: *mut EpollEvent, maxevents: c_int, timeout: c_int)
        -> c_int;
    fn eventfd(initval: u32, flags: c_int) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    fn close(fd: c_int) -> c_int;
}

/// A registered descriptor. `ident` is what `pollOnce` reports for it; a
/// registration with a callback uses `POLL_CALLBACK` instead and Android runs
/// the callback itself.
struct Registration {
    fd: c_int,
    ident: c_int,
    callback: Option<extern "C" fn(c_int, c_int, *mut c_void) -> c_int>,
    data: *mut c_void,
}

pub struct Looper {
    epoll: c_int,
    /// Written to by `ALooper_wake`, so a blocked `pollOnce` returns promptly.
    wake: c_int,
    registrations: RefCell<Vec<Registration>>,
    refs: AtomicUsize,
}

thread_local! {
    /// Android's loopers are per-thread and `ALooper_forThread` returns the
    /// calling thread's, so the storage has to be thread-local too. Leaked on
    /// first use: the engine holds the pointer for the thread's lifetime and
    /// there is no point at which returning it would be safe.
    static LOOPER: RefCell<Option<&'static Looper>> = const { RefCell::new(None) };
}

impl Looper {
    fn new() -> Option<&'static Looper> {
        // SAFETY: plain syscall wrappers with no pointer arguments.
        let (epoll, wake) = unsafe { (epoll_create1(0), eventfd(0, 0)) };
        if epoll < 0 || wake < 0 {
            return None;
        }
        let looper = Box::leak(Box::new(Looper {
            epoll,
            wake,
            registrations: RefCell::new(Vec::new()),
            refs: AtomicUsize::new(1),
        }));

        let mut ev = EpollEvent {
            events: EPOLLIN,
            data: looper.wake as u64,
        };
        // SAFETY: `epoll` and `wake` are open descriptors; `ev` is live.
        unsafe { epoll_ctl(looper.epoll, EPOLL_CTL_ADD, looper.wake, &mut ev) };
        Some(looper)
    }

    fn for_thread() -> Option<&'static Looper> {
        LOOPER.with(|l| *l.borrow())
    }

    fn prepare() -> Option<&'static Looper> {
        LOOPER.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = Looper::new();
            }
            *slot
        })
    }
}

fn epoll_to_looper_events(events: u32) -> c_int {
    let mut out = 0;
    if events & EPOLLIN != 0 {
        out |= EVENT_INPUT;
    }
    if events & EPOLLOUT != 0 {
        out |= EVENT_OUTPUT;
    }
    if events & EPOLLERR != 0 {
        out |= EVENT_ERROR;
    }
    if events & EPOLLHUP != 0 {
        out |= EVENT_HANGUP;
    }
    out
}

// ------------------------------------------------------------------- the API

extern "C" fn looper_prepare(_opts: c_int) -> *mut c_void {
    Looper::prepare().map_or(std::ptr::null_mut(), |l| l as *const Looper as *mut c_void)
}

extern "C" fn looper_for_thread() -> *mut c_void {
    Looper::for_thread().map_or(std::ptr::null_mut(), |l| l as *const Looper as *mut c_void)
}

fn as_looper(p: *mut c_void) -> Option<&'static Looper> {
    // SAFETY: every pointer handed out came from a leaked Box that is never
    // freed, so a non-null one is always live.
    (!p.is_null()).then(|| unsafe { &*(p as *const Looper) })
}

extern "C" fn looper_acquire(looper: *mut c_void) {
    if let Some(l) = as_looper(looper) {
        l.refs.fetch_add(1, Ordering::Relaxed);
    }
}

extern "C" fn looper_release(looper: *mut c_void) {
    // The count is tracked but never acted on: the looper is thread-local and
    // leaked, so dropping to zero would mean freeing something the thread may
    // still poll. Android's own loopers outlive their refcount reaching zero in
    // the same way.
    if let Some(l) = as_looper(looper) {
        l.refs.fetch_sub(1, Ordering::Relaxed);
    }
}

extern "C" fn looper_add_fd(
    looper: *mut c_void,
    fd: c_int,
    ident: c_int,
    events: c_int,
    callback: Option<extern "C" fn(c_int, c_int, *mut c_void) -> c_int>,
    data: *mut c_void,
) -> c_int {
    let Some(l) = as_looper(looper) else {
        return -1;
    };

    let mut epoll_events = 0;
    if events & EVENT_INPUT != 0 {
        epoll_events |= EPOLLIN;
    }
    if events & EVENT_OUTPUT != 0 {
        epoll_events |= EPOLLOUT;
    }

    let mut ev = EpollEvent {
        events: epoll_events,
        data: fd as u64,
    };
    // SAFETY: `l.epoll` is open, `fd` is the caller's, `ev` is live.
    if unsafe { epoll_ctl(l.epoll, EPOLL_CTL_ADD, fd, &mut ev) } < 0 {
        return -1;
    }

    l.registrations.borrow_mut().push(Registration {
        fd,
        // With a callback Android reports POLL_CALLBACK rather than the ident.
        ident: if callback.is_some() { POLL_CALLBACK } else { ident },
        callback,
        data,
    });
    1
}

extern "C" fn looper_remove_fd(looper: *mut c_void, fd: c_int) -> c_int {
    let Some(l) = as_looper(looper) else {
        return -1;
    };
    // SAFETY: EPOLL_CTL_DEL ignores the event argument.
    unsafe { epoll_ctl(l.epoll, EPOLL_CTL_DEL, fd, std::ptr::null_mut()) };
    let mut regs = l.registrations.borrow_mut();
    let before = regs.len();
    regs.retain(|r| r.fd != fd);
    if regs.len() < before {
        1
    } else {
        0
    }
}

extern "C" fn looper_wake(looper: *mut c_void) {
    let Some(l) = as_looper(looper) else {
        return;
    };
    let one: u64 = 1;
    // SAFETY: writing the eight bytes an eventfd requires to our own descriptor.
    unsafe { write(l.wake, &one as *const u64 as *const c_void, 8) };
}

extern "C" fn looper_poll_once(
    timeout_millis: c_int,
    out_fd: *mut c_int,
    out_events: *mut c_int,
    out_data: *mut *mut c_void,
) -> c_int {
    let Some(l) = Looper::for_thread() else {
        return POLL_ERROR;
    };

    let mut events = [EpollEvent { events: 0, data: 0 }; 16];
    // SAFETY: `events` is a live array of the length passed.
    let n = unsafe { epoll_wait(l.epoll, events.as_mut_ptr(), events.len() as c_int, timeout_millis) };
    if n < 0 {
        return POLL_ERROR;
    }
    if n == 0 {
        return POLL_TIMEOUT;
    }

    for ev in events.iter().take(n as usize) {
        let fd = ev.data as c_int;

        if fd == l.wake {
            let mut sink = 0u64;
            // SAFETY: draining the eight bytes written by looper_wake.
            unsafe { read(l.wake, &mut sink as *mut u64 as *mut c_void, 8) };
            return POLL_WAKE;
        }

        let (ident, callback, data) = {
            let regs = l.registrations.borrow();
            match regs.iter().find(|r| r.fd == fd) {
                Some(r) => (r.ident, r.callback, r.data),
                None => continue,
            }
        };
        let looper_events = epoll_to_looper_events(ev.events);

        if let Some(cb) = callback {
            // The registration is not borrowed across this call: a callback is
            // entitled to add or remove descriptors, and holding the borrow would
            // panic when it did.
            if cb(fd, looper_events, data) == 0 {
                looper_remove_fd(l as *const Looper as *mut c_void, fd);
            }
            return POLL_CALLBACK;
        }

        if !out_fd.is_null() {
            // SAFETY: caller-provided out-parameters, checked for null.
            unsafe { *out_fd = fd };
        }
        if !out_events.is_null() {
            // SAFETY: as above.
            unsafe { *out_events = looper_events };
        }
        if !out_data.is_null() {
            // SAFETY: as above.
            unsafe { *out_data = data };
        }
        return ident;
    }

    POLL_TIMEOUT
}

pub fn overrides() -> Vec<(&'static str, *mut c_void)> {
    macro_rules! f {
        ($name:literal, $fn:expr) => {
            ($name, $fn as *const () as *mut c_void)
        };
    }
    vec![
        f!("ALooper_prepare", looper_prepare),
        f!("ALooper_forThread", looper_for_thread),
        f!("ALooper_acquire", looper_acquire),
        f!("ALooper_release", looper_release),
        f!("ALooper_addFd", looper_add_fd),
        f!("ALooper_removeFd", looper_remove_fd),
        f!("ALooper_pollOnce", looper_poll_once),
        f!("ALooper_wake", looper_wake),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_times_out_rather_than_spinning() {
        let looper = looper_prepare(0);
        assert!(!looper.is_null());
        let start = std::time::Instant::now();
        assert_eq!(looper_poll_once(50, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()), POLL_TIMEOUT);
        // A stub returning immediately would turn the engine's main loop into a
        // busy spin, which is the failure this implementation exists to avoid.
        assert!(start.elapsed().as_millis() >= 40, "pollOnce returned without waiting");
    }

    #[test]
    fn wake_interrupts_a_blocked_poll() {
        let looper = looper_prepare(0);
        looper_wake(looper);
        assert_eq!(
            looper_poll_once(1000, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()),
            POLL_WAKE
        );
    }

    #[test]
    fn a_readable_fd_is_reported_with_its_ident() {
        let looper = looper_prepare(0);
        // SAFETY: creating and writing to our own eventfd.
        let fd = unsafe { eventfd(1, 0) };
        assert!(fd >= 0);

        assert_eq!(looper_add_fd(looper, fd, 42, EVENT_INPUT, None, std::ptr::null_mut()), 1);

        let (mut out_fd, mut out_events) = (0, 0);
        let mut out_data = std::ptr::null_mut();
        let rc = looper_poll_once(500, &mut out_fd, &mut out_events, &mut out_data);
        assert_eq!(rc, 42, "pollOnce must report the ident the fd was registered with");
        assert_eq!(out_fd, fd);
        assert_ne!(out_events & EVENT_INPUT, 0);

        assert_eq!(looper_remove_fd(looper, fd), 1);
        // SAFETY: closing the descriptor this test opened.
        unsafe { close(fd) };
    }
}
