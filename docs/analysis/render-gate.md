# What gates the engine's first EGL surface

**Status:** investigation only, nothing modified. Disassembly against
`libroblox.so` from Roblox for Android 2.732.1043 (same build as
[`findings.md`](../findings.md) and [`app-bridge.md`](app-bridge.md)), using
`objdump`/`readelf`/`nm` plus two small scratch scripts (a raw call-site
scanner) written for this pass. AGDK `GameActivity.cpp`/`.h` sources were read
from a mirror (`ejoy/ant`, Apache-2.0, matches the upstream
`platform/frameworks/opt/gamesdk` tree) to know what the disassembled AGDK code
is supposed to do. `decompiled/` was not read, per instructions.

**Bottom line up front:** the raw AGDK `GameActivity` surface lifecycle that
Cordial currently drives (`initializeNativeCode` → `onStartNative` →
`onResumeNative` → `onSurfaceCreatedNative` → `onSurfaceChangedNative`, all
against `MainGameActivity`) is a dead end for rendering. Disassembly shows
Roblox's own `onNativeWindowCreated` handler for that path does not call any
EGL function — it hands the window to a worker thread that, by the time any
window exists, has already run to completion and exited. Separately, the
*only* code in the entire 116 MB binary that calls `eglCreateWindowSurface` is
reached through a completely different object-construction chain that
[`app-bridge.md`](app-bridge.md) already identified, from the Java side, as
belonging to `ActivityNativeMain`'s App Bridge (`vi/e`), not
`MainGameActivity`. The two documents corroborate each other from opposite
directions — one from dex bytecode, one from native disassembly — and the
combined conclusion is the actionable part of this report (§3).

---

## 1. What `onSurfaceCreatedNative` actually does with the window

**Verified by disassembly.**

`Java_com_google_androidgamesdk_GameActivity_initializeNativeCode` (the export
at file offset `0x24fbd40`) is a small forwarding thunk: it calls a helper at
`0x24fad90` (`GameActivity_register`, which does the `RegisterNatives` for the
whole AGDK method table — `onStartNative`, `onSurfaceCreatedNative`,
`onSurfaceChangedNative`, etc., all `(J...)V`-shaped as in upstream
`GameActivity.cpp`), then tail-jumps into `0x24fbd90`, the real
`initializeNativeCode_native` body. That body matches upstream's function
statement for statement: allocate and zero a `NativeCode`-sized object,
`ALooper_forThread`/`acquire`, `pipe()`, `fcntl` non-blocking x2,
`ALooper_addFd` (AGDK's own `mainWorkRead` pipe — the one whose callback is
`mainWorkCallback`, already observed firing), `GetJavaVM`, `NewGlobalRef`,
three `GetStringUTFChars`/`ReleaseStringUTFChars` pairs for the three data
paths, `AAssetManager_fromJava`, then a `GetByteArrayElements`/`GetArrayLength`
pair for `savedState`, and finally a call at `0x24fc25c` with arguments
`(code, rawSavedState, rawSavedSize)` — exactly `GameActivity_onCreate`'s
signature. That call target, `0x24fee50`, is Roblox's own implementation of
the `GameActivity_onCreate` symbol AGDK declares `extern` and expects the app
to define (this is the same contract as `ANativeActivity_onCreate` — not a
JNI-registered method, so it has no `Java_*` export name and does not appear
in `nm -D`; it was located purely by tracing this one call site).

### 1.1 Roblox populates the entire `GameActivityCallbacks` table

Inside `GameActivity_onCreate` (`0x24fee50`), the very first thing it does is
`rax = *(activity->callbacks_ptr)` (dereferencing the `GameActivityCallbacks*`
AGDK set up at `initializeNativeCode_native`'s `code->GameActivity::callbacks =
&code->callbacks`), then writes **21 function pointers** into it, one per
`lea reg,[rip+#target]; mov [rax+OFS],reg`:

| Offset | Field (from AGDK's public `GameActivity.h`) | Target |
|---|---|---|
| `0x00` | `onStart` | `0x24ff280` |
| `0x08` | `onResume` | `0x24ff290` |
| `0x10` | `onSaveInstanceState` | `0x24ff2a0` |
| `0x18` | `onPause` | `0x24ff3b0` |
| `0x20` | `onStop` | `0x24ff3c0` |
| `0x28` | `onDestroy` | `0x24ff0d0` |
| `0x30` | `onWindowFocusChanged` | `0x24ff7f0` |
| **`0x38`** | **`onNativeWindowCreated`** | **`0x24ff870`** |
| `0x40` | `onNativeWindowResized` | `0x24ffb20` |
| `0x48` | `onNativeWindowRedrawNeeded` | `0x24ffaa0` |
| `0x50` | `onNativeWindowDestroyed` | `0x24ff9c0` |
| `0x58` | `onConfigurationChanged` | `0x24ff6f0` |
| `0x60` | `onTrimMemory` | `0x24ff770` |
| `0x68` | `onTouchEvent` | `0x24ff3d0` |
| `0x70` | `onKeyDown` | `0x24ff530` |
| `0x78` | `onKeyUp` | `0x24ff530` (same fn as `onKeyDown`) |
| `0x80` | `onTextInputEvent` | `0x24ff6b0` |
| `0x88` | `onWindowInsetsChanged` | `0x24ffba0` |
| `0x90`, `0x98`, `0xa0` | *(beyond the header compared against)* | `0x24ffc20`, `0x24ffcd0`, `0x24ffd70` |

Every one of the 18 offsets that exist in the upstream header lines up
**exactly**, in order, with the field it's supposed to be — this is about as
strong as static confirmation gets that this is genuinely AGDK's
`GameActivityCallbacks` struct, and that Roblox populates every callback
unconditionally, synchronously, during `initializeNativeCode`, well before any
surface exists. (The three extra offsets past `0x88` mean Roblox's build links
a slightly newer/extended AGDK than the header used for comparison; harmless
to this analysis.)

**This settles half of task 3 directly: yes, Roblox populates
`onNativeWindowCreated`, unconditionally, from `GameActivity_onCreate`.** AGDK's
own gate (`callbacks.onNativeWindowCreated != NULL`) is satisfied by the time
`onSurfaceCreatedNative` is ever called.

### 1.2 `onNativeWindowCreated` does not touch EGL — it blocks on a handoff

`0x24ff870` (the function just wired into offset `0x38`) does:

1. `r12 = activity->instance` (the `GameActivity` struct's app-opaque pointer,
   at offset `0x38` of the *activity*, not to be confused with offset `0x38`
   of the *callbacks* table above — same numeric offset, different struct).
   This is a Roblox-owned object, `0x180` bytes, allocated inside
   `GameActivity_onCreate` (§1.3).
2. `pthread_mutex_lock(r12+0xc8)`.
3. If `r12+0x158` ("pending window") was `NULL`, write one byte (`0x02`) to a
   private pipe at fd `r12+0x124`.
4. Unconditionally store the new `ANativeWindow*` into `r12+0x158`.
5. If the new window is non-`NULL`, write one more byte (`0x01`) to the same
   pipe.
6. Loop `pthread_cond_wait` on `r12+0xf0`/`r12+0xc8` **until `r12+0x38` (a
   "current window" field) equals `r12+0x158`** (the pending window just
   stored).
7. Unlock, return.

There is no `eglGetDisplay`, `eglCreateWindowSurface`, `eglMakeCurrent`, or any
`gl*`/`egl*` call anywhere in this function. It is a pure producer/consumer
handoff: **it hands the `ANativeWindow*` to whatever is on the other end of
that pipe/condvar, and blocks the calling thread until that other party
advances `r12+0x38`.** Nothing in `onSurfaceChanged_native`'s equivalent
handling (`onNativeWindowCreated`/`onNativeWindowResized`, called from AGDK's
own `onSurfaceChanged_native` per upstream source, which is why
`ANativeWindow_fromSurface` is reached twice) differs in this respect.

### 1.3 The thread meant to service that handoff is short-lived and exits during startup

`GameActivity_onCreate` allocates the `0x180`-byte object above, initializes
its mutex/condvar, then calls `pthread_create` (`0x24ff068`, detached:
`pthread_attr_setdetachstate(..., PTHREAD_CREATE_DETACHED)`) with entry point
`0x24fff60`, and **immediately blocks the calling thread** on a second
mutex/condvar pair waiting for the new thread to set `r12+0x148` ("started")
— this is why `initializeNativeCode` can return at all (this barrier already
passes in Cordial today, since `MainGameActivity`'s lifecycle calls are
observed firing).

The spawned thread (`0x24fff60`), read start to end, does exactly this,
unconditionally, with no branch that skips any of it:

1. `AConfiguration_new` / `_fromAssetManager` / `_getLanguage` / `_getCountry`.
2. Allocate four buffers, set up an `android_poll_source`-shaped struct
   (`{ident, self, process_fn}` at `r12+0x130..0x140` — a copy of the pattern
   `android_native_app_glue` uses, not AGDK's own).
3. `ALooper_prepare(1)`, `ALooper_addFd(looper, fd=r12+0x120 /* pipe read
   end */, ident=1, events=INPUT, callback=NULL, data=&r12+0x130)`.
4. Lock mutex, **set `r12+0x148 = 1`, broadcast, unlock — the startup
   barrier release** that lets `GameActivity_onCreate` (and hence
   `initializeNativeCode`) return.
5. Call one function (`0x29bbb52`, a bounded, non-looping routine — it
   allocates a `0x320`-byte scratch object, constructs it, uses it for one
   call, destroys it, and returns via its own stack-canary-checked epilogue)
   that reads like a one-shot global/settings bootstrap, not a render loop.
6. Lock mutex, free the saved-state copy if present, unlock.
7. Lock mutex, `AConfiguration_delete`, **set `r12+0x150 = 1`, broadcast,
   unlock.**
8. `ret` — **the thread function returns and the (detached) thread exits.**

`r12+0x150` is exactly the flag the `onDestroy` handler (`0x24ff0d0`) waits to
become non-zero before it proceeds to tear the whole object down — i.e. it is
this same object's "worker thread finished" signal. Put together: **the one
thread Roblox spawns to own this handoff finishes its entire job and exits
during app startup, before any `Surface` can plausibly exist.** By the time
`onSurfaceCreatedNative` is ever called and reaches `onNativeWindowCreated`
(§1.2), the thread that was supposed to read the pipe and advance
`r12+0x38` is gone. Cordial's own `ALooper` implementation
(`crates/cordial-runtime/src/android/looper.rs`) is genuinely per-thread
(`thread_local!`), matching real Android — so this is not a Cordial bug to fix
in the looper; the *real* Android would have exactly the same problem if
nothing else served that fd. On real Android something else evidently does
not need to, because — per §2 and §3 — this handoff mechanism is very likely
never the thing that produces the frame in the first place.

**Confidence:** the disassembly in §1.1–§1.3 is verified line-by-line against
upstream AGDK source where AGDK code is involved, and independently checked
for internal consistency (offsets referenced by one function match offsets
written by another) where Roblox's own code is involved. What is *inferred*
rather than directly proven is the semantic label "one-shot bootstrap thread"
for `0x24fff60`/`0x29bbb52` — that label rests on the absence of any loop or
branch back to the `ALooper_addFd`'d fd in the disassembled control flow, not
on a positively-identified purpose for that thread.

---

## 2. What gates `eglCreateWindowSurface`

**Verified by disassembly**, using a raw call-site scanner over the full
`.text` section (a Python script scanning every `E8` opcode for a relative
displacement landing on the PLT stub — full linear disassembly of an 80 MB
`.text` was avoided for time; this method finds only *direct* `call`s, not
calls through function pointers/vtables, which matters below).

**There is exactly one direct call site to `eglCreateWindowSurface` in the
entire binary**, at `0x23371db`. Working backwards:

1. **`0x23371b4`** — a tiny helper: if `*(this+0x10) != 0`, calls
   `eglCreatePbufferSurface`; otherwise calls `eglCreateWindowSurface(dpy =
   *(this+0x20), config = <passed through from caller>, window =
   *(this+0x18), attrib_list = NULL)`. Two call sites reach it
   (`0x23360d5` and `0x233a480`; only the first was examined in depth).
2. **`0x2335f56`** (wrapped by a small allocating trampoline, `0x2335f02`) —
   this is Roblox's "bring up EGL" routine: `eglGetDisplay` → `eglInitialize`
   → `eglCreateContext` → the helper above (window or pbuffer surface) →
   `eglMakeCurrent` → two `eglQuerySurface` calls (width, height). A parameter
   (`r15`, the wrapper's 2nd/3rd caller-supplied value) gates whether the
   `eglGetDisplay`/`eglInitialize`/`eglCreateContext` prefix runs at all —
   when non-null it skips straight to surface-creation + `eglMakeCurrent`,
   i.e. this same routine is designed to run once for first bring-up and
   again for reuse.
3. **`0x2335f02`** has exactly two direct callers, `0x6031788` and
   `0x6035917`, both inside one large object **constructor** (`0x60314c4`,
   object size `0x348` bytes — confirmed by a `pthread_self()` call stashed
   at `object+0x10` and a `~0x2b0`-byte field-zeroing prologue typical of a
   C++ constructor). The call at `0x6031788` passes the `ANativeWindow*` that
   the constructor's own caller supplied and is immediately preceded by
   `ANativeWindow_acquire`/`ANativeWindow_getWidth`/`getHeight` calls on that
   same window — i.e. **`eglCreateWindowSurface` fires as a side effect of
   constructing this object**, gated by a runtime flag check
   (`cmp byte[global],0; jne <skip-full-init>` at `0x6031768`) that decides
   whether this is first-time EGL bring-up or a reuse of an existing
   `EGLDisplay`.
4. That constructor has exactly **one** direct caller, `0x2291426`, inside a
   small factory function (`0x22913fa`) shaped like
   `CreateGraphicsDevice(int backendType, InitParams* params)`: `backendType
   == 0` allocates the `0x348`-byte object above and constructs it via
   `0x60314c4` (the GLES/EGL path); `backendType == 4` allocates a different,
   larger (`0x880`-byte) object via a different constructor (`0x603a618`, not
   examined — plausibly a second/alternate backend); any other value takes a
   fatal-error/log-and-throw path.
5. That factory has exactly **one** direct (`E8`) caller anywhere in `.text`:
   `0x33d6ca2`, inside a larger function (starting ~`0x33d6b00`) shaped like
   "get-or-create the graphics device for this GPU tier" (it indexes an
   8-entry static table by a tier id and calls the factory with the matching
   backend-type). **This outer function itself has zero direct callers
   anywhere in `.text`** — it is reached only through an indirect
   (virtual/function-pointer) call, so the static call-site method used here
   cannot identify what triggers it. This is the actual edge of what pure
   native disassembly can settle in a reasonable pass.

Corroborating evidence that the whole render/present path is virtualized this
way: `eglSwapBuffers`, too, has **zero** direct call sites in `.text` — it is
only ever reached indirectly, consistent with a "present" step dispatched
through the same object that gates surface creation, rather than called from
a fixed, staticaly-visible location.

**So the conditions for `eglCreateWindowSurface`, from the top down, are:**
some caller must invoke the "get-or-create graphics device for GPU tier X"
object (reached only by indirect call — not yet located), which must resolve
`backendType == 0`, which constructs a `0x348`-byte GLES device object with a
real, non-null `ANativeWindow*` already in hand. **Nothing in this chain is
fed by the AGDK `onNativeWindowCreated` callback from §1** — that callback's
window never reaches this code path in anything disassembled here; its
`ANativeWindow*` is consumed entirely by the handoff-and-block mechanism in
§1.2/§1.3, which (as established) has nobody left to service it.

---

## 3. Where the real window comes from, and the concrete recommendation

This is the part that closes the loop, and it does so by combining this
session's native-code tracing with [`app-bridge.md`](app-bridge.md)'s
independent, dex-bytecode-based investigation from earlier in the project —
**two unrelated methods (binary call-graph tracing here; Java bytecode xref
there) converging on the same conclusion is why this recommendation is held
with high confidence, not moderate.**

`app-bridge.md` already established, from the Android manifest and dex
bytecode (not touched in this session), that:

- The only exported, `LAUNCHER`-tagged Activity is `ActivitySplash`, and its
  hardcoded default `Intent` target is **`ActivityNativeMain`** — a plain
  `Activity` subclass, *not* `GameActivity`, and *not* `MainGameActivity`.
- `ActivityNativeMain.N2()` calls, in order: `vi/e.E(Context)` →
  `nativeGameGlobalInit`/`nativeUpdateAdapterInit`; `vi/e.j(...)` →
  **`nativeAppBridgeV2InitWithParams`**; and finally
  **`nativeAppBridgeStartLuaAppDM`** — the only call site for that native in
  the whole dex set.
- Separately, `vi/a` (a `SurfaceHolder.Callback` owned by `ActivityNativeMain`)
  calls `vi/e.F(Surface)` → **`nativeAppBridgeV2StartAppWithParams(StartAppParams)`**
  once a real `Surface` exists, where `StartAppParams` carries **its own
  `surface` field** — a plain `android.view.Surface` from a regular
  `SurfaceView`/`SurfaceHolder`, wired up through Java's ordinary
  `surfaceCreated`/`surfaceChanged` callbacks, structurally unrelated to
  AGDK's four-call window lifecycle.

This session's independent finding — that the *only* `eglCreateWindowSurface`
call site in the binary is reached through a `GraphicsDevice`-construction
chain that AGDK's own `onNativeWindowCreated` never feeds (§2) — is exactly
what you would expect if that chain is instead fed by the `Surface` inside
`StartAppParams`/`PlatformParams`, delivered through
`nativeAppBridgeV2StartAppWithParams` / `nativeAppBridgeV2UpdateSurfaceAppWithPlatformParams`,
not through `MainGameActivity`/`GameActivity` at all. The two documents were
produced by different methods against the same binary and dex, and they land
on the same Activity (`ActivityNativeMain`, not `MainGameActivity`) and the
same conclusion (the App Bridge V2 API, not the raw AGDK surface lifecycle, is
what actually starts rendering).

**This session did not trace the exact bytes from `StartAppParams.surface`
through JNI marshaling into the `ANativeWindow*` argument the `GraphicsDevice`
constructor consumes** — that is the one missing link, and it is exactly the
"indirect caller of the get-or-create-graphics-device function" gap flagged
at the end of §2. `app-bridge.md` §4.1 already flags two candidate `JNIEnv`
vtable calls inside `nativeAppBridgeV2StartAppWithParams` (offsets `0xf8` and
`0x710` — plausibly `GetObjectClass` and a `CallObjectMethod`-family call) as
the likely place the `Surface` gets pulled out of the params object; that is
consistent with, but not proof of, this being the same surface that reaches
`ANativeWindow_acquire` at `0x6031788`.

### Recommendation

**Stop trying to make `MainGameActivity`'s raw `GameActivity` surface
lifecycle produce a frame — the disassembly in §1 shows concretely that it
cannot, on its own, no matter how faithfully Cordial drives it.** Roblox's own
`onNativeWindowCreated` for that path hands the window to a thread that has
already exited by the time any window would arrive; nothing downstream of
that ever reaches EGL.

Instead, drive the path `app-bridge.md` identified as the one the shipping app
actually launches into by default:

1. Instantiate `com.roblox.client.ActivityNativeMain` (or replicate its
   `onCreate`/`N2()` sequence directly at the native/JNI level), not
   `MainGameActivity`.
2. Call, in the order recovered from `ActivityNativeMain.N2()`'s bytecode:
   `nativeGameGlobalInit` → `nativeUpdateAdapterInit` →
   `nativeAppBridgeV2InitWithParams(InitParams)` → `nativeAppBridgeStartLuaAppDM()`.
3. Provide a plain `android.view.Surface` (backed by whatever Cordial's
   `SurfaceView`/`SurfaceHolder` equivalent is — this does not need AGDK's
   `Surface`/`ANativeWindow` ceremony, just a JNI object with the right shape)
   through `nativeAppBridgeV2StartAppWithParams(StartAppParams{surface=...})`
   and, on resize, `nativeAppBridgeV2UpdateSurfaceAppWithPlatformParams`.
4. Verify with a wrapper/trace on `eglCreateWindowSurface`, `eglMakeCurrent`,
   and `glClear` (the same technique `findings.md` §8.1 already uses for libc
   calls) that this path — and not `MainGameActivity`'s — is what fires them.
   That single check would also resolve the one gap left open above (whether
   `StartAppParams.surface` really is what reaches the `GraphicsDevice`
   constructor at `0x60314c4`), cheaply and conclusively, without needing to
   resolve the indirect call this static pass could not follow.

This does not necessarily mean `MainGameActivity` is dead work — `app-bridge.md`
§6 notes it may be what a *joined game* uses once the app shell is already up
(the `StartGameParams`/`vi/h0` "Game" half). But for "get one frame of Roblox
on screen," the evidence in both documents now points at the same, different
target.

---

## 4. Confidence summary

| Claim | Confidence | Basis |
|---|---|---|
| `initializeNativeCode`/AGDK code matches upstream `GameActivity.cpp` structurally | Verified | Line-by-line disassembly vs. fetched AGDK source |
| Roblox populates the full `GameActivityCallbacks` table incl. `onNativeWindowCreated` | Verified | Exact offset match, 18/18 fields against upstream header |
| `onNativeWindowCreated` never calls EGL; it blocks on a pipe/condvar handoff | Verified | Full disassembly of `0x24ff870` |
| The thread meant to service that handoff exits during startup, before a window can exist | Verified (straight-line disasm, no branch skips it) | Full disassembly of `0x24fff60` and its one non-looping callee `0x29bbb52` |
| Exactly one direct call site to `eglCreateWindowSurface` in the binary, and its full backward call chain to a GPU-tier device cache | Verified (direct-call scan of all of `.text`) | Raw `E8` opcode scan, cross-checked at each hop |
| That device-cache function's own caller is indirect and not identified | Verified absence (zero direct callers) / open question for its trigger | Same scan; cannot see vtable/fn-ptr calls |
| The App Bridge V2 path (`ActivityNativeMain`/`vi/e`), not `MainGameActivity`, is what feeds a real `Surface` into rendering | High confidence, not fully proven | This session's native-side chain (§2) is consistent with, but not directly wired to, `app-bridge.md`'s independent dex-bytecode chain; the missing link is named explicitly above |
