# Where to start

Cordial loads Roblox's engine, renders at about 27 fps on Vulkan, does HTTPS,
takes mouse and keyboard, and reaches the logged-out landing page. It is stable.
It is not yet usable, because you cannot sign in.

This file is the handover. It says what is blocking, how to work on it, and —
the part worth reading even if you are in a hurry — **what has already been
ruled out**.

## The one rule

**Grep the capture before disassembling anything.**

`docs/traces/waydroid-roblox-startup.log.gz` is a logcat capture of this exact
APK running on real Android. When a question comes up about what the engine
expects, that is a lookup rather than an investigation.

Over one long session, **nine consecutive conclusions drawn from reading the
stripped binary were wrong**, and every conclusion drawn from running something
held up. That record is why this rule is first.

## Read the engine's own log, always, before theorising

Roblox narrates itself to:

```
<files>/appData/logs/<version>_<timestamp>_Player_*.log
```

By default `~/.local/share/cordial/instances/default/data/files/appData/logs/`.
It names subsystems, stages, file paths and exceptions in Roblox's own words. It
is the best diagnostic in the project and it answers most questions. Two comments
in this repository once claimed FLog was unrouted; both were wrong, and nobody
had looked in `appData/`.

---

# What is blocking

## 1. Sign-in — the reason it is not usable

Without a session the client sits on the landing page. Avatar thumbnails fail
against user id 0 and there is nothing to do. `NativeUserJavaInterface` is
stubbed with an empty user.

`NativeSettingsInterface` exports `nativeGetCookiesForDomain`,
`nativeGetCookiesInNetscapeFormat` and `nativeSetMultipleCookies`, which is where
a session would be presented. Earlier analysis in this repository says the login
path involves a captcha and a WebView-hosted Activity — if that is right, an
embedded browser is a very large dependency and it changes the plan entirely.
Confirm before building anything.

## 2. The GLES fallback runs at about 1 fps

Vulkan is fine — 656 presents in 24 s across three runs, unchanged by injected
input, so it is a continuous loop and not render-on-demand. GLES is not: 20
`eglSwapBuffers` in 20 s, repeatedly. It matters because GLES is the fallback for
any host without Vulkan.

**Counting trap that has already produced one wrong conclusion:** a Vulkan
session leaves every GLES counter at zero, and a GLES session leaves
`vkQueuePresentKHR` at zero. `CORDIAL_COUNT_GL=1` reports both. Check which
renderer actually ran, in the engine's log, before reading a zero as "nothing
drew".

## 3. Plugins are designed and not built

[ADR-002](adr/ADR-002-core-shell-and-ui-handoff.md),
[ADR-003](adr/ADR-003-plugin-isolation.md) and
[ADR-005](adr/ADR-005-flag-service.md) describe the shape: out of process, behind
a capability broker, with flags contributed through layers that already exist in
`crates/cordial-runtime/src/flags.rs`. None of the host is written.

---

# Do not re-run these

Each was tested and each cost time. The evidence is the point.

**The futex that used to hang startup**
- Never an EGL/GBM surface handshake. It is the engine's ordinary wait primitive
  — offset +0x0C of a 64-byte-aligned object, the same class and call site all
  sixteen idle `RBX Worker` threads park on.
- Never an unserviced `ALooper`. Tested directly: the main thread pumped
  `epoll_wait` continuously on a dedicated thread while the worker still blocked
  at the identical futex.
- It was in `nativeAppBridgeStartLuaAppDM`, not `StartAppWithParams`.
- The block and the crash were **one bug, not two** — a completion handshake with
  a thread that segfaulted before it could signal.

**The frame rate**
- Window focus is not it. `onWindowFocusChangedNative(true)` and
  `onContentRectChangedNative` are both sent; the rate did not move.
- Frame-callback starvation is not it. `AChoreographer_*` is not imported at all.
- `FIntReactSchedulerMinFrameRate` set to 60 changed nothing.
- The render job binds its DataModel fine — `No DM yet` appears exactly twice,
  transiently, around 2.0 s.
- Graphics-quality FastFlags do nothing here. `DebugFRMQualityLevelOverride` and
  both MSAA overrides, at every prefix, left shader count, target size and frame
  rate identical. They govern 3D scene rendering and the landing page is a 2D
  interface. The hardware reports MSAA 16 support, so this is not a capability
  limit.

**The crash that used to kill a third of launches**
- Not a `pthread_create` override skipping per-thread setup — there is no such
  override, it is a plain passthrough.
- Not a `pthread_mutex_t`/`pthread_attr_t` ABI mismatch.
- Not a `malloc`/`free`/`operator new` mismatch directly — none of those are
  undefined symbols in `libroblox.so` at all.

**Flags**
- The flags verdict does not gate rendering. `onFlagsFailed` is a complaint, not
  a gate.
- `nativePreloadFlagOverrides` does nothing observable, despite the name. Merging
  into the client-settings document is the mechanism that works.
- **Do not test whether flags work using FLog channels.** Setting
  `FLogAndroidGLView=7` produces no output even when flags demonstrably work, so
  it is a broken instrument — it produced a confident, wrong "no flag reaches the
  engine" that survived several experiments. Use a flag with an observable
  behavioural effect, and run a control.

---

# How to work on this

## Debugging facts that cost real time

- **lldb breakpoints inside `libroblox.so` do not work and fail silently.**
  Cordial `mmap`s it outside the system linker, so lldb never lists the image and
  every breakpoint stays unresolved with hit count 0. Use `memory write` of
  `0xCC`, then rewind `$pc` and restore the byte on trap. Crash-stop backtraces
  and breakpoints in Cordial's own code work normally.
- **Read syscall arguments from `/proc/<pid>/task/<tid>/syscall`** while lldb has
  the process stopped, not from registers. Number plus all six arguments, no
  guesswork about the libc wrapper's register shuffling. That is how the futex
  was identified without disassembling anything.
- **There are three threads named `Main`.** Use `thread backtrace all`.
- **`CORDIAL_TRACE_PATHS=1` is safe** and logs every path-taking libc call with a
  thread id. **`CORDIAL_TRACE=1` is not** — it wraps variadic functions with
  fixed-arity declarations and aborts the engine.
- **`CORDIAL_SKIP_AGDK=1` skips the flag and app-bridge calls entirely.** Several
  historical results were measured on a path that never ran the code under test.
- With ASLR disabled `libroblox` loads at `0x7fffefec0000`.
- lldb is at `/home/linuxbrew/.linuxbrew/bin/lldb`. No gdb, no strace.
- **Never inject input with `XTestFake*`** — it takes over the real cursor on the
  machine you are using. Target the window with `XSendEvent`, found by `WM_CLASS`
  `cordial`.

## Measuring anything

- Use a **control**. The flag mechanism was only established by showing a log
  line vanishes with the flag set and is present without it, in the same session.
- **Repeat.** One bug here reproduced on roughly one launch in three and its rate
  moved with machine load, so before/after samples taken under different load are
  meaningless.
- Label anything you could not test **`INFERRED`**.

## A limit on the capture, stated honestly

Cordial runs natively on the host; the capture came from Waydroid. It is
trustworthy for **call order, names and contract** — which is what it was taken
for — and **not** for timing or render behaviour. Roblox under Waydroid burns CPU
with little GPU utilisation. Do not read its rendering path as a model of a
healthy one.

## On observing other binaries, including Sober

**Decompilation reconstructs expression.** You end up reading a reconstruction of
someone's source and writing code from it, which is where derivative-work risk
lives. That is why decompiled material is off limits (§16.1,
[ADR-001](adr/ADR-001-in-process-hooking.md)).

**A debugger on a running process yields behaviour** — which libraries load, which
natives are called, in what order, with what arguments. Facts and interfaces, not
expression.

So the line is not the tool, it is **what you take away**:

- Fine: call sequence, load order, argument shapes, resolved symbols, timing,
  syscalls.
- Not fine: stepping into its routines to read how it implements something and
  transcribing that logic. At that point the debugger is a slower decompiler.

**One rule, applied to any binary including Roblox: observe freely, do not
transcribe.** Sober was built by observing Roblox and nobody treats it as tainted
for it.

---

# Solved, for reference

Kept because the *shape* recurs — most of these were ABI or contract mismatches
that presented as something else entirely.

| Symptom | Actual cause |
|---|---|
| Startup hung in a futex, then died | Asset folder passed as the unpack root. The engine wants `<root>/content` and resolves siblings against the parent, so `canonical` threw and `SingleSurfaceApp` aborted before instantiating its controllers |
| Every hostname failed to resolve | `struct addrinfo` has `ai_canonname`/`ai_addr` transposed between bionic and glibc, and the `AI_*` constants differ — bionic's `AI_DEFAULT` sets a bit glibc rejects with `EAI_BADFLAGS` |
| Every HTTPS request failed | Engine builds `./exe/cacert.pem` from a root it was never given; it now has its own run directory with the APK's CA bundle linked in |
| SIGSEGV on `HttpClient`, one launch in three | `realpath(path, NULL)` allocates with the host allocator; Roblox statically links mimalloc and freed a pointer its arena table never registered |
| `eglCreateWindowSurface` returned `EGL_BAD_ALLOC` | The engine was handed Cordial's `ANativeWindow*`; host EGL on X11 wants an XID |
| Vulkan refused to initialise | Roblox needs `VK_KHR_android_surface`, which desktop Mesa never exposes. Translated to `VK_KHR_xlib_surface` behind one interposed `vkGetInstanceProcAddr` |
| Engine reported "Android API 15" and refused Vulkan | `DeviceParams.osVersion` is read as an API *level*. Neither the system property nor `android_get_device_api_level()` fed it |
| Paths resolved against the working directory | `NativeSettingsInterface`'s directory setters were never called |
| Interface looked like a low-end phone | Surface hardcoded to 720p and `dpiScale` to 1.0 — Roblox lays out in dp and picks asset resolutions from exactly those |

## Branches

`archive/gameactivity-per-callback` holds per-callback GameActivity dispatch that
was never merged. **Read it, do not merge it** — it is built on the disproved
ALooper theory and restructures the App Bridge onto a worker thread to fix a hang
whose real cause was the asset path. Merging it produced code that did not
compile against current main. One good idea in it: sharing one GameActivity
`thiz` and one Surface across calls, the way Android does.
