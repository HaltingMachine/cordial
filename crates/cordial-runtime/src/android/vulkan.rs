//! Roblox's Vulkan path, interposed onto the host loader.
//!
//! Two separate problems, both visible in the engine's own log before this file
//! existed:
//!
//! ```text
//! [FLog::SurfaceController] Mode 6 failed: Unable to load Vulkan API
//! ```
//!
//! **1. The `dlopen` never reaches the host.** `libroblox.so` does not link
//! Vulkan — it is not in `DT_NEEDED` — it `dlopen`s `"libvulkan.so"` and, failing
//! that, `"libvulkan.so.1"` (see `docs/framework-api-inventory.md` §on Vulkan).
//! Both calls go through the *bionic* linker, which only knows Cordial's virtual
//! library set and has no reason to fall through to the host's `/usr/lib64`.
//! Nothing here loads a real ELF for either name: `get_instance_proc_addr_symbol`
//! is registered as a **virtual library** (see `symtab::build`), the same
//! mechanism `EMPTY_LIBRARIES` uses — bionic's `find_library` matches an
//! already-registered soname before it ever touches disk.
//!
//! **2. Even a working `dlopen` would not be enough.** The virtual library's
//! only export is `vkGetInstanceProcAddr`; every other Vulkan entry point,
//! including `vkCreateInstance` itself, is fetched *through* it — bionic's
//! `"Unable to load Vulkan API: vkCreateInstance is NULL"` message (present as a
//! second, more specific string in the binary) only fires once that dlsym-first
//! step already worked, which confirms this is how the engine bootstraps. So
//! `vk_get_instance_proc_addr` below is the entire interposition surface for
//! global-level Vulkan: every function it does not recognise by name is hostcode,
//! forwarded straight through the real loader.
//!
//! What it does recognise, and why:
//!
//! * `vkCreateAndroidSurfaceKHR` — the engine calls this and only this to get a
//!   surface. Desktop Mesa has never heard of it; on X11 it has
//!   `vkCreateXlibSurfaceKHR` instead, and on Wayland `vkCreateWaylandSurfaceKHR`.
//!   [`vk_create_android_surface_khr`] builds whichever real call from Cordial's
//!   own window — `android::window::current()` on X11,
//!   `android::wayland::current()` on Wayland, decided once by
//!   `android::backend()` — the same handles `egl_create_window_surface`
//!   substitutes for EGL in each backend's own module; see the comment there
//!   for why that translation lives with the window and not in a call-counting
//!   module. This file follows the same reasoning for Vulkan.
//! * `VK_KHR_android_surface` — the extension string that has to exist for the
//!   engine to ask for the function above at all. Mesa reports
//!   `VK_KHR_xlib_surface` or `VK_KHR_wayland_surface` under their own names,
//!   according to which platform is live;
//!   [`vk_enumerate_instance_extension_properties`] adds
//!   `VK_KHR_android_surface` to the host's real list whenever the real
//!   extension for the active backend is present, and [`vk_create_instance`]
//!   rewrites it back before the real `vkCreateInstance` ever sees it — the
//!   host loader must never be told to enable an extension it does not
//!   implement.
//!
//! Everything else — `vkCreateDevice`, every `vkCmd*`, the whole per-frame
//! surface. — is untouched: once a real `VkInstance` exists, forwarding
//! `vkGetInstanceProcAddr(instance, name)` to the host is correct for any name
//! this module does not special-case, because the host's implementation *is* the
//! implementation Cordial wants.

use std::ffi::{c_char, c_ulong, c_void, CStr};
use std::sync::OnceLock;

// ------------------------------------------------------------------ layout
//
// These four structs are laid out exactly as the Vulkan specification defines
// them. Unlike the bionic/glibc boundary elsewhere in this tree, there is no
// second layout to reconcile here — Vulkan's ABI is the same struct on Android
// and on desktop Linux, which is what makes interposing it (rather than
// reimplementing it) the right shape for this problem.

#[repr(C)]
#[derive(Clone, Copy)]
struct VkInstanceCreateInfo {
    s_type: i32,
    p_next: *const c_void,
    flags: u32,
    p_application_info: *const c_void,
    enabled_layer_count: u32,
    pp_enabled_layer_names: *const *const c_char,
    enabled_extension_count: u32,
    pp_enabled_extension_names: *const *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VkExtensionProperties {
    extension_name: [c_char; 256],
    spec_version: u32,
}

impl VkExtensionProperties {
    fn zeroed() -> Self {
        // SAFETY: an all-zero `VkExtensionProperties` (empty name, version 0) is
        // a valid bit pattern for every field.
        unsafe { std::mem::zeroed() }
    }

    fn named(name: &str, spec_version: u32) -> Self {
        let mut p = Self::zeroed();
        for (dst, &b) in p.extension_name.iter_mut().zip(name.as_bytes()) {
            *dst = b as c_char;
        }
        p.spec_version = spec_version;
        p
    }

    fn name_matches(&self, target: &[u8]) -> bool {
        // SAFETY: `extension_name` is always initialised (zeroed or `named`),
        // so reading it as bytes is sound regardless of where the NUL falls.
        let bytes: &[u8] =
            unsafe { std::slice::from_raw_parts(self.extension_name.as_ptr().cast(), 256) };
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(256);
        &bytes[..end] == target
    }
}

#[repr(C)]
struct VkAndroidSurfaceCreateInfoKHR {
    s_type: i32,
    p_next: *const c_void,
    flags: u32,
    window: *mut c_void,
}

#[repr(C)]
struct VkXlibSurfaceCreateInfoKHR {
    s_type: i32,
    p_next: *const c_void,
    flags: u32,
    dpy: *mut c_void,
    window: c_ulong,
}

/// `VK_STRUCTURE_TYPE_XLIB_SURFACE_CREATE_INFO_KHR`. Present in `vulkan_core.h`
/// unguarded (the `sType` enum is platform-independent even though the struct it
/// tags is declared behind `VK_USE_PLATFORM_XLIB_KHR`).
const VK_STRUCTURE_TYPE_XLIB_SURFACE_CREATE_INFO_KHR: i32 = 1000004000;

#[repr(C)]
struct VkWaylandSurfaceCreateInfoKHR {
    s_type: i32,
    p_next: *const c_void,
    flags: u32,
    display: *mut c_void,
    surface: *mut c_void,
}

/// `VK_STRUCTURE_TYPE_WAYLAND_SURFACE_CREATE_INFO_KHR`, same unguarded-`sType`
/// situation as the Xlib one above.
const VK_STRUCTURE_TYPE_WAYLAND_SURFACE_CREATE_INFO_KHR: i32 = 1000006000;

/// `VK_KHR_android_surface`'s spec version, per the Khronos extension registry.
/// Fixed at 6 since it was introduced; there is nothing to detect it against.
const ANDROID_SURFACE_SPEC_VERSION: u32 = 6;

/// The real surface extension `VK_KHR_android_surface` is substituted for,
/// according to whichever display backend [`crate::android::backend`]
/// selected. `backend()` is chosen once, from the environment, before any
/// window opens (see its own doc comment) — Vulkan bring-up always happens
/// after that choice is fixed, so there is no point in this call racing it.
fn real_surface_extension_name() -> &'static CStr {
    match crate::android::backend() {
        crate::android::Backend::Wayland => c"VK_KHR_wayland_surface",
        crate::android::Backend::X11 => c"VK_KHR_xlib_surface",
    }
}

const VK_SUCCESS: i32 = 0;
const VK_INCOMPLETE: i32 = 5;
const VK_ERROR_INITIALIZATION_FAILED: i32 = -3;
const VK_ERROR_EXTENSION_NOT_PRESENT: i32 = -7;

// -------------------------------------------------------------- host loading

/// The real entry points, resolved from the host's Vulkan loader once.
struct HostVulkan {
    get_instance_proc_addr: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    create_instance:
        unsafe extern "C" fn(*const VkInstanceCreateInfo, *const c_void, *mut *mut c_void) -> i32,
    enumerate_instance_extension_properties:
        unsafe extern "C" fn(*const c_char, *mut u32, *mut VkExtensionProperties) -> i32,
}

// Only ever read after `OnceLock` initialisation; the fields are plain function
// pointers into a library that, like every other host library this runtime
// opens, is never closed.
unsafe impl Send for HostVulkan {}
unsafe impl Sync for HostVulkan {}

static HOST: OnceLock<Option<HostVulkan>> = OnceLock::new();

fn host() -> Option<&'static HostVulkan> {
    HOST.get_or_init(load_host).as_ref()
}

extern "C" {
    // The *host* loader, not the bionic one — same reasoning as `window.rs`'s
    // X11 loading and `symtab.rs`'s `host_dlopen`: this file must reach real
    // `/usr/lib64/libvulkan.so.1`, and the bionic `dlopen` Roblox itself calls is
    // the one this module exists to answer, not to recurse through.
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}
use std::ffi::c_int;
const RTLD_NOW: c_int = 2;

fn load_host() -> Option<HostVulkan> {
    // `CORDIAL_NO_VULKAN=1` makes the host look exactly like a machine with no
    // Vulkan loader at all: `host()` returns `None`, `symtab::build` leaves both
    // virtual `libvulkan.so`/`libvulkan.so.1` sonames unregistered, and Roblox's
    // own `dlopen` fails the same way it did before this module existed — a
    // clean, deliberate fall-through to GLES. Useful on its own (forcing the
    // fallback path to test it) independent of whatever bug prompted adding it.
    if std::env::var_os("CORDIAL_NO_VULKAN").is_some() {
        return None;
    }

    let mut handle = std::ptr::null_mut();
    // The Linux soname first, then the Android one Roblox actually asks for —
    // either is fine to load from, since what matters is which real library
    // answers, not which name found it.
    for name in [c"libvulkan.so.1", c"libvulkan.so"] {
        // SAFETY: literal, NUL-terminated sonames; the handle is never closed.
        handle = unsafe { dlopen(name.as_ptr(), RTLD_NOW) };
        if !handle.is_null() {
            break;
        }
    }
    if handle.is_null() {
        return None;
    }

    // SAFETY: `handle` is open; the name is the Vulkan loader's own
    // documented export.
    let gipa = unsafe { dlsym(handle, c"vkGetInstanceProcAddr".as_ptr()) };
    if gipa.is_null() {
        return None;
    }
    // SAFETY: resolved from the host loader for exactly this name.
    let gipa: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void =
        unsafe { std::mem::transmute(gipa) };

    // `vkCreateInstance` and `vkEnumerateInstanceExtensionProperties` are the
    // two "global" commands Roblox needs before any `VkInstance` exists. Per
    // spec they are only guaranteed reachable through `vkGetInstanceProcAddr(
    // NULL, name)` — the Linux loader also exports them directly, but going
    // through the documented bootstrap costs nothing extra and is the same path
    // being interposed for everything else.
    // SAFETY: `gipa` came from the host loader and `instance = NULL` is its
    // documented way to ask for global commands.
    let create_instance = unsafe { gipa(std::ptr::null_mut(), c"vkCreateInstance".as_ptr()) };
    let enum_ext = unsafe {
        gipa(
            std::ptr::null_mut(),
            c"vkEnumerateInstanceExtensionProperties".as_ptr(),
        )
    };
    if create_instance.is_null() || enum_ext.is_null() {
        return None;
    }

    Some(HostVulkan {
        get_instance_proc_addr: gipa,
        // SAFETY: resolved from the host loader for exactly these names.
        create_instance: unsafe { std::mem::transmute(create_instance) },
        enumerate_instance_extension_properties: unsafe { std::mem::transmute(enum_ext) },
    })
}

// --------------------------------------------------------------- registration

/// The sonames Roblox tries, in the order it tries them (per
/// `docs/framework-api-inventory.md`). Both are registered identically —
/// whichever one bionic's `dlopen` is asked for finds the same virtual library.
pub const LIBRARY_NAMES: [&str; 2] = ["libvulkan.so", "libvulkan.so.1"];

/// The one export the virtual `libvulkan.so`/`libvulkan.so.1` libraries need.
/// `None` if the host has no Vulkan at all, in which case `symtab::build` leaves
/// both sonames unregistered and Roblox's `dlopen` fails exactly as it does
/// today — a clean fall-through to GLES, not a half-working Vulkan.
pub fn get_instance_proc_addr_symbol() -> Option<*mut c_void> {
    host().map(|_| vk_get_instance_proc_addr as *const () as *mut c_void)
}

// -------------------------------------------------------------- interposition

extern "C" fn vk_get_instance_proc_addr(instance: *mut c_void, name: *const c_char) -> *mut c_void {
    let Some(h) = host() else {
        return std::ptr::null_mut();
    };
    if name.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: bionic's `vkGetInstanceProcAddr` contract is a NUL-terminated name.
    let bytes = unsafe { CStr::from_ptr(name) }.to_bytes();
    match bytes {
        // A `vkGetInstanceProcAddr(instance, "vkGetInstanceProcAddr")` query
        // must return itself — the spec requires it, and code that
        // self-verifies its loader this way exists.
        b"vkGetInstanceProcAddr" => vk_get_instance_proc_addr as *const () as *mut c_void,
        b"vkCreateInstance" => vk_create_instance as *const () as *mut c_void,
        b"vkEnumerateInstanceExtensionProperties" => {
            vk_enumerate_instance_extension_properties as *const () as *mut c_void
        }
        b"vkCreateAndroidSurfaceKHR" => vk_create_android_surface_khr as *const () as *mut c_void,
        // Counted, not altered. A Vulkan session leaves every GLES counter at
        // zero, so without this the graphics report cannot tell "Vulkan is
        // presenting frames" from "nothing is drawing at all".
        // Device-level entry points are normally fetched through
        // `vkGetDeviceProcAddr`, not through this function — that is the whole
        // point of the device dispatch. Intercepting only the instance getter
        // meant the present counter never incremented and a perfectly healthy
        // Vulkan session read as "nothing is drawing".
        b"vkGetDeviceProcAddr" => {
            HOST_GET_DEVICE_PROC_ADDR.store(
                unsafe { (h.get_instance_proc_addr)(instance, name) } as usize,
                std::sync::atomic::Ordering::Relaxed,
            );
            vk_get_device_proc_addr as *const () as *mut c_void
        }
        b"vkQueuePresentKHR" => {
            HOST_QUEUE_PRESENT.store(
                unsafe { (h.get_instance_proc_addr)(instance, name) } as usize,
                std::sync::atomic::Ordering::Relaxed,
            );
            vk_queue_present_khr as *const () as *mut c_void
        }
        // Every other name — `vkCreateDevice`, every `vkCmd*`, everything a real
        // `VkInstance` answers for once one exists — is exactly what the host
        // loader would give a native Linux Vulkan app. Forwarding unconditionally
        // is correct because `instance`, once created, is a real host
        // `VkInstance`: see `vk_create_instance`.
        _ => unsafe { (h.get_instance_proc_addr)(instance, name) },
    }
}

/// The host's `vkGetDeviceProcAddr`, so device-level lookups can be forwarded
/// after the counted ones are peeled off.
static HOST_GET_DEVICE_PROC_ADDR: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

extern "C" fn vk_get_device_proc_addr(device: *mut c_void, name: *const c_char) -> *mut c_void {
    let f = HOST_GET_DEVICE_PROC_ADDR.load(std::sync::atomic::Ordering::Relaxed);
    if f == 0 || name.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: resolved from the host loader for exactly this name.
    let host: extern "C" fn(*mut c_void, *const c_char) -> *mut c_void =
        unsafe { std::mem::transmute(f) };
    // SAFETY: Vulkan's contract is a NUL-terminated name.
    if unsafe { CStr::from_ptr(name) }.to_bytes() == b"vkQueuePresentKHR" {
        HOST_QUEUE_PRESENT.store(host(device, name) as usize, std::sync::atomic::Ordering::Relaxed);
        return vk_queue_present_khr as *const () as *mut c_void;
    }
    host(device, name)
}

/// The real `vkQueuePresentKHR`, resolved on first request and then called
/// through unchanged.
static HOST_QUEUE_PRESENT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

extern "C" fn vk_queue_present_khr(queue: *mut c_void, info: *const c_void) -> i32 {
    crate::android::glcount::QUEUE_PRESENT
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let f = HOST_QUEUE_PRESENT.load(std::sync::atomic::Ordering::Relaxed);
    if f == 0 {
        return 0;
    }
    // SAFETY: resolved from the host loader for exactly this name.
    let f: extern "C" fn(*mut c_void, *const c_void) -> i32 = unsafe { std::mem::transmute(f) };
    f(queue, info)
}

extern "C" fn vk_create_instance(
    create_info: *const VkInstanceCreateInfo,
    allocator: *const c_void,
    instance_out: *mut *mut c_void,
) -> i32 {
    let Some(h) = host() else {
        return VK_ERROR_INITIALIZATION_FAILED;
    };
    let Some(info) = (unsafe { create_info.as_ref() }) else {
        // SAFETY: `create_info` is caller-supplied; a null pointer here is the
        // caller's bug, not this shim's — hand it to the host unchanged and let
        // it report `VK_ERROR_INITIALIZATION_FAILED` on its own terms.
        return unsafe { (h.create_instance)(create_info, allocator, instance_out) };
    };

    let count = info.enabled_extension_count as usize;
    // SAFETY: `count` and `pp_enabled_extension_names` are the caller's own
    // paired length and pointer, per the Vulkan struct contract.
    let names: &[*const c_char] = if count == 0 || info.pp_enabled_extension_names.is_null() {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(info.pp_enabled_extension_names, count) }
    };

    // The host must never be asked to enable an extension it does not
    // implement. `VK_KHR_android_surface` only exists in what Roblox sees —
    // `vk_enumerate_instance_extension_properties` invented it — so it is
    // rewritten back to the real extension for whichever backend is live
    // before this ever reaches Mesa. Everything else passes through
    // untouched, including extensions this shim knows nothing about; the
    // host rejecting one it truly lacks is the correct failure, not
    // something to mask here.
    let real_name = real_surface_extension_name();
    let mut swapped = false;
    let rewritten: Vec<*const c_char> = names
        .iter()
        .map(|&p| {
            if !p.is_null() && unsafe { CStr::from_ptr(p) }.to_bytes() == b"VK_KHR_android_surface"
            {
                swapped = true;
                real_name.as_ptr()
            } else {
                p
            }
        })
        .collect();

    crate::android::trace(format_args!(
        "vkCreateInstance: {count} extension(s) requested, VK_KHR_android_surface -> {}: {swapped}",
        real_name.to_string_lossy()
    ));

    let patched = VkInstanceCreateInfo {
        pp_enabled_extension_names: if rewritten.is_empty() {
            info.pp_enabled_extension_names
        } else {
            rewritten.as_ptr()
        },
        ..*info
    };
    // SAFETY: `patched` matches the host's `VkInstanceCreateInfo` layout exactly
    // (see the module doc); `rewritten` outlives this call.
    unsafe { (h.create_instance)(&patched, allocator, instance_out) }
}

extern "C" fn vk_enumerate_instance_extension_properties(
    layer_name: *const c_char,
    property_count: *mut u32,
    properties: *mut VkExtensionProperties,
) -> i32 {
    let Some(h) = host() else {
        return VK_ERROR_INITIALIZATION_FAILED;
    };
    if property_count.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }

    // The two-call idiom, run once against the host to build the real list.
    let mut host_count: u32 = 0;
    // SAFETY: `layer_name` is the caller's own pointer, forwarded unchanged;
    // `host_count` is a valid local out-parameter.
    let rc = unsafe {
        (h.enumerate_instance_extension_properties)(layer_name, &mut host_count, std::ptr::null_mut())
    };
    if rc != VK_SUCCESS {
        return rc;
    }
    let mut combined = vec![VkExtensionProperties::zeroed(); host_count as usize];
    if host_count > 0 {
        // SAFETY: `combined` has exactly `host_count` elements, matching the
        // count this same call just reported.
        let rc = unsafe {
            (h.enumerate_instance_extension_properties)(
                layer_name,
                &mut host_count,
                combined.as_mut_ptr(),
            )
        };
        if rc != VK_SUCCESS {
            return rc;
        }
        combined.truncate(host_count as usize);
    }

    // Mesa reports its own name for this capability — `VK_KHR_xlib_surface`
    // on X11, `VK_KHR_wayland_surface` on Wayland, per whichever backend
    // `real_surface_extension_name` says is live. Roblox will only ever ask a
    // Vulkan loader for `VK_KHR_android_surface`, because that is the only
    // surface extension Android ever had. Advertise it whenever the host has
    // the capability it stands in for — layer-provided extension lists
    // (`layer_name` non-null) are left as the layer reported them, since this
    // is an ICD-level substitution, not a layer's.
    let real_name = real_surface_extension_name();
    let has_real = combined.iter().any(|p| p.name_matches(real_name.to_bytes()));
    let has_android = combined
        .iter()
        .any(|p| p.name_matches(b"VK_KHR_android_surface"));
    if layer_name.is_null() && has_real && !has_android {
        combined.push(VkExtensionProperties::named(
            "VK_KHR_android_surface",
            ANDROID_SURFACE_SPEC_VERSION,
        ));
        crate::android::trace(format_args!(
            "vkEnumerateInstanceExtensionProperties: advertising VK_KHR_android_surface (backing {})",
            real_name.to_string_lossy()
        ));
    }

    let combined_count = combined.len() as u32;
    if properties.is_null() {
        // SAFETY: caller-supplied out-parameter, per the Vulkan two-call idiom.
        unsafe { *property_count = combined_count };
        return VK_SUCCESS;
    }

    // SAFETY: the caller sets `*property_count` to the capacity of `properties`
    // before this call, per the Vulkan two-call idiom.
    let requested = unsafe { *property_count };
    let to_copy = requested.min(combined_count);
    // SAFETY: `properties` has room for at least `requested` entries per the
    // caller's own contract; `to_copy` is bounded by both that and `combined`'s
    // real length.
    unsafe {
        std::ptr::copy_nonoverlapping(combined.as_ptr(), properties, to_copy as usize);
        *property_count = to_copy;
    }
    if to_copy < combined_count {
        VK_INCOMPLETE
    } else {
        VK_SUCCESS
    }
}

/// `vkCreateAndroidSurfaceKHR`, answered with `vkCreateXlibSurfaceKHR` on X11
/// or `vkCreateWaylandSurfaceKHR` on Wayland, according to
/// [`crate::android::backend`].
///
/// The `ANativeWindow*` inside `pCreateInfo` is Cordial's own — there is
/// exactly one window — so it is not read; the real handles come from
/// whichever backend's window singleton is live, the same pair
/// `egl_create_window_surface` already substitutes for EGL in each backend's
/// own module (`window.rs`/`wayland.rs`).
extern "C" fn vk_create_android_surface_khr(
    instance: *mut c_void,
    create_info: *const VkAndroidSurfaceCreateInfoKHR,
    allocator: *const c_void,
    surface_out: *mut *mut c_void,
) -> i32 {
    let _ = create_info;
    let Some(h) = host() else {
        return VK_ERROR_INITIALIZATION_FAILED;
    };

    match crate::android::backend() {
        crate::android::Backend::Wayland => {
            let Some(win) = crate::android::wayland::current() else {
                return VK_ERROR_INITIALIZATION_FAILED;
            };
            // SAFETY: `instance` is a real host `VkInstance` by the time the
            // engine can reach this call, and the name is Mesa's own
            // documented export.
            let f = unsafe {
                (h.get_instance_proc_addr)(instance, c"vkCreateWaylandSurfaceKHR".as_ptr())
            };
            if f.is_null() {
                return VK_ERROR_EXTENSION_NOT_PRESENT;
            }
            type Fn_ = unsafe extern "C" fn(
                *mut c_void,
                *const VkWaylandSurfaceCreateInfoKHR,
                *const c_void,
                *mut *mut c_void,
            ) -> i32;
            // SAFETY: resolved from the host for exactly this name.
            let f: Fn_ = unsafe { std::mem::transmute(f) };
            let wayland_info = VkWaylandSurfaceCreateInfoKHR {
                s_type: VK_STRUCTURE_TYPE_WAYLAND_SURFACE_CREATE_INFO_KHR,
                p_next: std::ptr::null(),
                flags: 0,
                display: win.wl_display(),
                surface: win.wl_surface(),
            };
            crate::android::trace(format_args!(
                "vkCreateAndroidSurfaceKHR -> vkCreateWaylandSurfaceKHR"
            ));
            // SAFETY: `wayland_info` matches Mesa's
            // `VkWaylandSurfaceCreateInfoKHR` layout exactly (see the module
            // doc); `instance`, `allocator` and `surface_out` are the
            // caller's own arguments, forwarded unchanged.
            unsafe { f(instance, &wayland_info, allocator, surface_out) }
        }
        crate::android::Backend::X11 => {
            let Some(win) = crate::android::window::current() else {
                return VK_ERROR_INITIALIZATION_FAILED;
            };
            // SAFETY: as above.
            let f =
                unsafe { (h.get_instance_proc_addr)(instance, c"vkCreateXlibSurfaceKHR".as_ptr()) };
            if f.is_null() {
                return VK_ERROR_EXTENSION_NOT_PRESENT;
            }
            type Fn_ = unsafe extern "C" fn(
                *mut c_void,
                *const VkXlibSurfaceCreateInfoKHR,
                *const c_void,
                *mut *mut c_void,
            ) -> i32;
            // SAFETY: resolved from the host for exactly this name.
            let f: Fn_ = unsafe { std::mem::transmute(f) };
            let xlib_info = VkXlibSurfaceCreateInfoKHR {
                s_type: VK_STRUCTURE_TYPE_XLIB_SURFACE_CREATE_INFO_KHR,
                p_next: std::ptr::null(),
                flags: 0,
                dpy: win.egl_native_display(),
                window: win.egl_native_window(),
            };
            crate::android::trace(format_args!("vkCreateAndroidSurfaceKHR -> vkCreateXlibSurfaceKHR"));
            // SAFETY: `xlib_info` matches Mesa's `VkXlibSurfaceCreateInfoKHR`
            // layout exactly (see the module doc); `instance`, `allocator`
            // and `surface_out` are the caller's own arguments, forwarded
            // unchanged.
            unsafe { f(instance, &xlib_info, allocator, surface_out) }
        }
    }
}
