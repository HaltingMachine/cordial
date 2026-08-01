# Where to start

Roblox does not render under Cordial. It **does** run under Waydroid on this
machine, and that capture is committed — see `docs/traces/`.

## The one rule that matters

**Grep the trace before disassembling anything.** Over one long session, every
conclusion drawn by reading the stripped binary was wrong — nine in a row — and
every conclusion drawn by running something held up. The trace exists so that
"what does the engine expect here?" is a lookup, not an investigation.

The rule held again this session. The futex was identified without disassembling
a single instruction, and the root cause underneath it was found by reading a log
file the engine had been writing all along.

## Read this first: the engine has a voice, and it always did

**Roblox writes its own FastLog to `appData/logs/<version>_<timestamp>_Player_*.log`,
relative to the working directory.** Every run produces one. It is far and away
the best diagnostic in the project — it names subsystems, stages, file paths and
exceptions in the engine's own words.

Two comments in this repo claimed the opposite ("`FLog` is not routed anywhere
visible in this build"). Both were wrong and are now corrected. Nobody had
looked in `appData/`.

So: **before anything else, read the newest file in `appData/logs/`.**

```bash
cat "appData/logs/$(ls -t appData/logs | head -1)"
```

Enabling extra channels via client settings (adding `FLog<Channel>` keys to the
cached settings document) was tried and produced no additional output; the
channels that matter are on by default anyway. Not worth a second attempt.

## The futex: answered, and it was not what we thought

The previous handoff said the driving thread parks in a futex that is "most
likely an EGL/GBM surface handshake that never completes". **That was wrong.**
There is no graphics primitive involved.

What it actually is, established by observation:

- The futex word lives in an anonymous heap arena, at **offset +0x0C of a
  64-byte-aligned engine object**. Every one of the 16 idle `RBX Worker` threads
  parks on the *same class of object at the same offset through the same call
  site*. It is the engine's ordinary internal wait primitive, nothing special.
- The wait is `FUTEX_WAIT_BITSET|FUTEX_PRIVATE`, expected value 2, **timeout
  NULL** — indefinite.
- It is a **completion handshake**: the JNI thread dispatches work to an engine
  thread and waits for that thread to signal. The engine thread segfaults before
  signalling, so the wait can never end.

**The block and the crash were one bug, not two.** The previous handoff treated
them as independent ("it dies because a *different* engine thread segfaults").
They are cause and effect.

Proven causally rather than argued: at the SIGSEGV, stepping the faulting thread
over its null dereferences and continuing made the futex resolve immediately,
`StartAppWithParams` return, `ANativeWindow_*` calls follow, and the process run
its full 12 seconds and exit 0.

### Correction: it is `StartLuaAppDM`, not `StartAppWithParams`

The blocking call is `nativeAppBridgeStartLuaAppDM`
(`load.rs:821`), not `nativeAppBridgeV2StartAppWithParams`. The previous handoff
named the wrong one. This matches the capture, where `StartLuaAppDM` is exactly
the call that hands work to the `SingleSurfaceApp` thread and waits.

### How to read a futex under Cordial

`lldb` cannot symbolise `libroblox`, but it stops the process fine, and while it
is stopped `/proc/<pid>/task/<tid>/syscall` gives the syscall number and all six
arguments directly — no register-shuffle guesswork about glibc's `syscall()`
wrapper. Combine with `/proc/<pid>/maps` to place the address. That is how the
above was established, and it is the technique to reuse.

## Root cause found and fixed: the asset folder was one level too high

The engine's log named it:

```text
[FLog::Output] setAssetFolder ~/.cache/cordial/assets
[FLog::CreatorError] Error: boost::filesystem::canonical:
    No such file or directory: ".../.cache/cordial/android"
```

The capture says what the path should be:

```text
[FLog::Output] setAssetFolder      /data/user/0/com.roblox.client/app_assets/content
[FLog::Output] setExtraAssetFolder /data/user/0/com.roblox.client/app_assets/ExtraContent
```

The engine is handed the **`content` subdirectory** and resolves its siblings —
`android/`, `ssl/`, `fonts/`, `ExtraContent/` — relative to the *parent*. Cordial
handed it the unpack root, so every sibling lookup landed a level too high, the
`canonical` call threw, and `SingleSurfaceApp` initialisation aborted **before**
`setStage: (stage:Native)` and before it instantiated its controllers. The later
`initializeLuaAppWithLoggedInUser` then ran at `(stage:None)` and made a virtual
call through a controller that had never been constructed — the null dereference
at `libroblox+0x2ccd912`.

Fixed in `asset_folder()` in `load.rs`. The same string also feeds
`PlatformParams.assetFolderPath` via `nativeAppBridgeSetInitParams`, which was
still being handed the raw `.apk` file path; that is fixed too.

### What the fix bought

The engine now gets all the way through the sequence it was failing at. Against
`docs/traces/render-bringup-sequence.log`, Cordial now reproduces:

```text
initializeWithAppStarter / initializeSingleton
setAssetFolder + setExtraAssetFolder          (correct paths)
registerForForceOTAUpdateAvailableConnection   <- new
setStage: (stage:Native)                       <- new
instantiate controllers                        <- new
SurfaceController[_:1]::SurfaceController      <- new
instantiate experience coordinator             <- new
initializeLuaAppWithLoggedInUser: (stage:Native).   (was (stage:None))
applyLocale
DataModelPatchConfigurer ... deserializeAndVerifyPatch with blake3
[FLog::Output] Hello world, CLI-208683! ...    <- Lua is running
```

**Lua executes.** That is a long way past where this was stuck.

## Fixed since: DNS, HTTPS, and the working directory

`struct addrinfo` is **not** the same in bionic and glibc — the last two
pointers are transposed, and the `AI_*` constants disagree outright (bionic's
`AI_DEFAULT` sets a bit glibc rejects with `EAI_BADFLAGS`). That is why every
request failed with `Could not resolve host`. Translated in
`native/netdb_compat.cpp`; put `addrinfo` on the list next to `stat`,
`pthread_mutex_t`, `DIR`, `FILE` and `sigset_t`.

Then curl failed on `error adding trust anchors from file: ./exe/cacert.pem`.
The engine builds several paths from a root it was never given and resolves them
against the working directory — `./exe/cacert.pem`, `http/`, `sounds/`, `cache/`,
`ContentProvider_<pid>`. Cordial now gives the process its own run directory
with the APK's CA bundle linked into `exe/`, which also stops the engine
littering whatever directory you launched from.

With those two in, the engine reaches `APP_READY (Landing)` and **`flags FAILED`
drops to zero** — the static-flag problem below resolved itself once HTTPS
worked, so it was a symptom, not a cause. Left recorded because the measurement
technique is the reusable part.

## Fixed: the crash on a third of runs

`realpath(path, NULL)` is a GNU extension where **glibc allocates the result and
the caller frees it**. Roblox statically links its own allocator — `malloc`,
`free`, `operator new` and `operator delete` are not undefined symbols in
`libroblox.so` at all — so when it released a buffer that came from the host
allocator, the free ran inside mimalloc, whose arena lookup is keyed on the
pointer's own address. A host pointer was never registered there, the first
level came back null, and the next dereference was unconditional:
`movq (%rax,%rcx), %rdi` with `rax=0`.

Only reachable once HTTPS completes a request, because that is when the cURL
layer re-resolves the CA bundle path per connection — which is why it appeared
the moment networking started working.

Cordial's `s_realpath` no longer forwards the allocating form. It returns
`NULL`/`ENOTSUP`, the documented POSIX failure, and the caller falls back to the
path string it already had. There is no buffer Cordial *could* hand back safely,
because Roblox's allocator is not reachable from outside it.

Measured 5/16 before and 16/16 after by the agent that found it, then 10/10
independently on a checkout carrying every other change.

**Disproved on the way there:** a `pthread_create` override skipping per-thread
setup (there is no such override — it is a plain passthrough), a
`pthread_mutex_t`/`pthread_attr_t` ABI mismatch, and the same cross-allocator
theory applied to `malloc`/`free` directly.

## Previously: it crashed on roughly a third of runs

Deterministic signature, same every time:

```text
thread 'HttpClient', SIGSEGV at libroblox+0x1cb7cc6, fault address 0xe000
    movq (%rax,%rcx), %rdi      rax = 0   rcx = 0xe000
```

A table indexed 0xe000 bytes off a **null base**. It only started appearing once
HTTPS began working, so it is newly *reached* rather than newly introduced —
verified by A/B against the previous commit, which fails at the same rate.

`rax` being null on an `HttpClient` thread, for a large fixed-offset table,
smells like per-thread state that was never set up on a thread the engine
created through Cordial's `pthread_create` override. Check the TLS block before
anything else. Do **not** assume it is the HTTP code just because the thread is
named HttpClient.

Note this invalidates any earlier claim in the history that the client "stays up
for twelve seconds" — that was measured on a run of successes and the failure
rate was not sampled.

## Corrected: the frame rate was a measurement error on Vulkan, and a real
## problem on GLES

Vulkan renders at **about 27 fps, steady** — 656, 656 and 655 presents over 24 s,
unchanged by injected input, so it is a continuous loop and not render-on-demand.

The "1 fps" that this file previously called the blocker was measured with
`eglSwapBuffers`. Two things were wrong with that:

- Once Vulkan landed it became the default renderer, and a Vulkan session leaves
  every GLES counter at zero. Reading zero as "nothing is drawing" is exactly the
  mistake the counter was added to prevent, and it was made anyway.
- The `vkQueuePresentKHR` counter added to replace it *also* read zero, because
  device-level entry points are resolved through `vkGetDeviceProcAddr`, not
  `vkGetInstanceProcAddr`. The shim only intercepted the instance getter. Fixed.

**What survives as a real problem:** the GLES path genuinely was about 1 fps
(20 swaps in 20 s, repeatedly). That matters for any host without Vulkan, since
GLES is the fallback. The investigation below was not wasted — it was aimed at
the right symptom on the wrong renderer.

## The GLES fallback is about 1 fps

Every engine thread sits in `futex_do_wait` and wakes once a second; 13% CPU
over thirty seconds; exactly 20 swaps in 20s and 30 in 30s. It is waiting, not
working.

**Disconfirmed:** that the engine was throttling for lack of window focus.
`onWindowFocusChangedNative(true)` and `onContentRectChangedNative` are now sent
after the surface handover — both are part of the AGDK contract and Android does
send them, so they are kept — and the frame rate did not move at all.

**Also disconfirmed:** frame-callback starvation. `AChoreographer_*` is not
imported by `libroblox`.

**Also disconfirmed:** `FIntReactSchedulerMinFrameRate`. The client-settings
document carries `FIntReactSchedulerMinFrameRate_IXP = 1`, and the app shell UI
runs on a React-style deferred scheduler, so a minimum frame rate of 1 looked
like an exact match for the symptom. Setting the plain
`FIntReactSchedulerMinFrameRate` to 60 changed nothing — still exactly 20 swaps
in 20 s. Either the engine only honours the IXP-delivered form (and we get no
experiment assignment without a session) or it is the wrong knob.

**Also ruled out:** that the render job never binds a DataModel.
`RenderJob::stepDataModelJob: No DM yet` and `scheduleRender: No data model`
appear exactly twice, at ~2.0 s, and are transient — by ~3.1 s the log shows
`onGameLoaded`, then `APP_READY` for `PlatformAccountRouter`, `Startup` and
`Landing` in sequence. The DataModel binds fine.

**Live possibility, and it needs a session to test:** the client is sitting on
the logged-out landing screen, and Roblox's app shell may legitimately idle
there. Nothing yet distinguishes "Cordial fails to drive the render loop" from
"the landing screen has nothing to animate". The cheapest discriminator is
input: if a click produces a burst of swaps, the loop is fine and the idle is
the app's own choice.

The app shell logs `Register rendering frequency during startup` and later
`Restoring rendering frequency to normal`, and renders on demand. Still the best
theory, still unproven.

**The discriminator now exists but needs a display.** Input landed, so a click
should produce a burst of frames if the render loop is healthy and the idle is
the app's own choice. Measure `vkQueuePresentKHR` (Vulkan) or `eglSwapBuffers`
(GLES) with and without input — note that a Vulkan session leaves every GLES
counter at zero, so use the right one.

Do **not** measure this with `XTestFake*` on a desktop someone is using: it
injects into the real session and takes over their cursor. Use a nested server
(`Xephyr`, `Xvfb`) or a dedicated seat.

Every engine thread sits in `futex_do_wait` and wakes once a second; 13% CPU
over thirty seconds; exactly 30 swaps in 30s. It is waiting, not working.

The app shell logs `Register rendering frequency during startup` and later
`Restoring rendering frequency to normal`, and renders on demand rather than
continuously. **Working theory, not proven:** nothing in Cordial delivers a
frame or input signal to drive that, so it falls back to a one-second heartbeat.
Note `AChoreographer_*` is *not* imported by `libroblox`, so it is not simple
frame-callback starvation — that was checked.

## Previously the blocker: the static flag path

Measured against the capture, same 139 names, same call:

| | resolved | not found |
|---|---|---|
| Real client | **74** | 67 |
| Cordial | **0** | 68 |

The *not found* sets agree — those flags are genuinely absent from the engine's
registry on both. The 74 that should resolve return nothing here. That is what
`onFlagsFailed` is reporting, and the real client never calls it once.

It is **not** that client settings are ignored. That was tested with a control:
setting `DFFlagRbxTransportUseRtcioRna=False` removes
`Initialized RtcIoRna with 1 event loop threads` from the engine's log, and the
control run with the document unmodified has it. Dynamic flags apply. So the
defect is specific to the **static** (`FFlag`) path that
`nativeInitializeNativeFlags` looks up — 64 of the 139 names are present in the
client-settings document as `FFlag<name>` and still report not found.

Do not use `FLog` channels to test whether flags apply. Setting `FLogAndroidGLView=7`
through client settings *or* `nativePreloadFlagOverrides` produces no output even
though flags demonstrably work, so it is a broken instrument — it produced a
confident wrong conclusion ("no FastFlag reaches the engine") that survived
several experiments. Use a flag with an observable behavioural effect, and run
the control.

This most likely gates rendering: the surface handler returns early with
`nativeActivity_onSurfaceChanged: ... Flags-Not-Received. Return.`, and the
client draws at about 1 fps at 8% CPU — waiting, not working.

## Second: the engine cannot resolve a hostname

Every request fails with `Could not resolve host: apis.roblox.com`, so no remote
content arrives and `glTexImage2D` stays at zero. This is Cordial's, not the
environment's: `getent hosts apis.roblox.com` and `curl` both succeed from a
shell on this machine, and `getaddrinfo`/`gethostbyname` resolve to the host's
libc (confirmed with `--verbose`). Suspect the resolver thread rather than the
lookup — curl's default backend spawns a thread per lookup, and thread creation
goes through Cordial's pthread overrides.

## The previous blocker, for reference

```text
[LOGCHANNELS + 1] RBXCRASH: UnhandledException (St13runtime_error Path does not exist: "")
```

Thrown ~0.2 s after `deserializeAndVerifyPatch with blake3`, on the DataModel/Lua
thread — the same thread that prints the `Hello world` lines. The path is
**empty**, not merely missing.

Facts about it, all from running:

- It happens with `CORDIAL_SKIP_LUA_DM=1` too, so it is **not** driven by
  `StartLuaAppDM`. The patch configurer is started during `V2Init`.
- **No JNI upcall precedes it.** Cordial's `[JNIVM]` log stops at
  `PlatformParams.assetFolderPath`. So the empty path was supplied earlier or is
  computed internally — the engine is not asking the host for it.
- The subsequent SIGSEGV is *secondary*: it lands in `_IO_fflush` with a null
  `FILE*`, i.e. inside Roblox's own crash reporter. Do not chase that address; it
  is the handler tripping over glibc/bionic `FILE` layout, not the defect.
- The content is not missing: all 1839 asset files extract, including
  `ExtraContent/models/UniversalApp/UniversalApp.rbxm` and
  `ExtraContent/places/Mobile.rbxl` (which is what the real client loads next).

### Leads, in the order worth trying

1. **The engine ignores the storage directory Cordial gives it.**
   `initStorageManagerNativeV3` is passed
   `$XDG_DATA_HOME/cordial/instances/default/data` — *twice*, the same string for
   both arguments — and that directory **did not even exist**. Creating it changed
   nothing, and the engine keeps writing to a CWD-relative `appData/`, which is
   its unconfigured fallback. So the storage root is very likely still unset
   inside the engine, and an unset root is a plausible source of an empty path.
   Find out what the two arguments actually are before guessing again — the dex
   in the APK names them.
2. `InitParams.baseURL` is `https://www.roblox.com` and `userAgent` is
   `Roblox/Android`. The capture says `www.roblox.com/` (**trailing slash**) and
   the long real UA (`... ROBLOX Android App 2.732.1043 Tablet Hybrid
   GooglePlayStore RobloxApp/2.732.1043`). Cheap to align, and `setBaseUrl()` is
   visible in the capture.
3. `DeviceParams.appVersion` is `""`. If any path is built as
   `<root>/<version>/...` that is a candidate.

## Still true, still not worth redoing

- The **flags verdict does not gate rendering**. `onFlagsFailed` is a complaint,
  not a gate. Confirmed again this session: the verdict is still `FAILED` while
  the engine happily instantiates controllers and runs Lua.
- It is **not** an unserviced ALooper.
- The 139 flag names and the corrected bring-up order are already in the tree.

## On observing Sober

**Decompilation reconstructs expression** — you end up reading a reconstruction
of their source and writing code from it, which is where derivative-work risk
lives. That is why `decompiled/` stays off-limits (§16.1, ADR-001).

**A debugger on a running process yields behaviour** — which libraries it loads,
which natives it calls, in what order, with what arguments. Those are facts and
interfaces, not expression, and black-box observation for interoperability is the
ordinary basis for this kind of work.

So the line is not the tool, it is **what you take away**:

- Fine: the call sequence, the load order, argument shapes, which symbols get
  resolved, timing, syscalls.
- Not fine: stepping into its routines to read how it implements something and
  transcribing that logic. At that point the debugger is just a slower decompiler.

**One rule, applied to any binary including Roblox: observe freely, do not
transcribe.** Sober was built by observing Roblox, and nobody treats Sober as
tainted for it.

Sober remains the better reference for the render path specifically — it runs the
same APK natively against the host GPU, which is Cordial's shape. It was not
needed this session; the engine's own log was enough.

## A limit on the trace, stated honestly

Cordial runs **natively on the host** (X11/Mesa), not inside the container. The
Waydroid capture is trustworthy for **call order, names and contract** — which is
what it was taken for — but **not** for timing or render behaviour.

## Debugging facts that cost time to learn

- **Read `appData/logs/` first.** See the top of this file.
- **lldb breakpoints inside `libroblox.so` do not work.** Cordial `mmap`s it
  outside the system linker, so lldb never lists the image and every breakpoint
  stays unresolved with hit count 0 — silently. Use `memory write` of `0xCC`,
  then rewind `$pc` and restore the byte on trap. Crash-stop backtraces and
  breakpoints in Cordial's own code are unaffected.
- **Read syscall arguments from `/proc/<pid>/task/<tid>/syscall`** while lldb has
  the process stopped, not from registers.
- **Stepping a thread over its faults is a legitimate causality experiment.** On
  SIGSEGV, advance `$pc` by the instruction length, zero the destination
  register, and continue. Five skips were enough to prove the futex was
  downstream of the crash. Debugger-only; nothing shipped.
- **There are three threads named `Main`.** Use `thread backtrace all`.
- **`CORDIAL_SKIP_AGDK=1` skips the flag and app-bridge calls entirely.**
- The whole run lives and dies in ~120 ms. Sampling `/proc` from a shell loop is
  too slow to catch it; drive it under lldb.
- Roblox's launcher activity is `com.roblox.client/.startup.ActivitySplash`.

## Branches

`agent/wt-agdk` has the per-callback GameActivity work and the worker-thread
restructure; it is not merged. `agent/ordering` and `agent/flags` are merged.
