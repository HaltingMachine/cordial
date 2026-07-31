# Why `nativeInitializeNativeFlags` reports `onFlagsFailed` and segfaults

**Status:** investigation only; nothing modified except this file, per instructions.
Disassembly against the same `libroblox.so` (Roblox for Android 2.732.1043) used by
`findings.md`, [`app-bridge.md`](app-bridge.md) and [`render-gate.md`](render-gate.md),
using `objdump`/`nm`/`readelf` plus two small scratch scripts (a direct-`call`-site
scanner and a rip-relative-`lea`-site scanner, same technique `render-gate.md` §2
used for `eglCreateWindowSurface`). Cross-checked against `native/init_params.cpp`
(read, not modified) and reproduced once against the built `cordial-load` binary
without rebuilding it, since `native/*.cpp` is being edited concurrently.

**Bottom line up front:** `nativeInitializeNativeFlags` itself is not the function
that calls `gameActivity_onFlagsFailed`. Its own disassembled body — verified in
full — constructs the `NativeFlagsInitResult` exactly as documented, iterates the
input `String[]`, and returns normally, exception-safe, no matter what is in that
array. The **only** call to `gameActivity_onFlagsFailed` anywhere in the 116 MB
binary lives in a small, separate helper reached through a completely different,
**indirectly-invoked** function that has zero direct callers in `.text` — the same
shape `render-gate.md` §2 already found for `eglCreateWindowSurface`'s trigger, and
for the same reason: something calls it through a function pointer this pass cannot
resolve statically. What *is* pinned down precisely is the gating condition on that
call, and a live, currently-wrong piece of Cordial's own code that feeds
`nativeInitializeNativeFlags` a single "flag name" that is actually Roblox's entire
22,318-flag settings document.

---

## 1. `nativeInitializeNativeFlags` builds its result exactly as documented — verified

`Java_com_roblox_client_flags_FlagJniInterface_nativeInitializeNativeFlags` sits at
file offset `0x215a0b3` in `.dynsym`, 1625 bytes up to the next export
(`nativeRegisterFFlag` at `0x215a70c`). The dex confirms the descriptor:

```
com/roblox/client/flags/FlagJniInterface.nativeInitializeNativeFlags([Ljava/lang/String;)Lcom/roblox/client/flags/NativeFlagsInitResult;
```

Every `call *0xNN(%rax)` in this function was decoded against the exact
`JNINativeInterface` layout Cordial's own libjnivm ships
(`third_party/libjnivm/include/jni.h`), counted field-by-field rather than assumed
from memory — the header has an extra `ToReflectedField` slot that a memorized
table would miss, and getting one offset wrong cascades into every later one:

| Offset | Index | Function | Confirms |
|---|---|---|---|
| `0x30` | 6 | `FindClass` | class `com/roblox/client/flags/NativeFlagsInitResult` (read from `.rodata` at `0x34b965`) |
| `0x108` | 33 | `GetMethodID` | `"<init>"` `"(I)V"` (at `0x445690`/`0x46267b`), then `"addBoolean"` `"(Ljava/lang/String;ZZ)V"` (at `0x4628cc`/`0x507da6`) |
| `0x558` | 171 | `GetArrayLength` | called on the JNI argument itself — **it is a `jarray`, not a `jstring`** |
| `0x568` | 173 | `GetObjectArrayElement` | one call per loop iteration, index `i` |
| `0x720` | 228 | `ExceptionCheck` | after each `addBoolean` call |
| `0x88` | 17 | `ExceptionClear` | called, not `FatalError` — a pending exception here is **swallowed**, not fatal |
| `0xb8` | 23 | `DeleteLocalRef` | cleans up the array element every iteration |

Control flow, verified instruction-by-instruction:

1. `FindClass("com/roblox/client/flags/NativeFlagsInitResult")`.
2. `GetMethodID(class, "<init>", "(I)V")` and `GetMethodID(class, "addBoolean",
   "(Ljava/lang/String;ZZ)V")` — matching `native/init_params.cpp`'s own
   `NativeFlagsInitResult::ctor(ENV*, jint)` and
   `addBoolean(ENV*, shared_ptr<String>, jboolean, jboolean)` hooks exactly, both
   name and descriptor. **Cordial's Java-side stub for this class is shaped
   correctly.**
3. Calls a local helper at `0x215a308` with a *global* pointer (`0x77d9298`, an
   internal engine singleton, unrelated to the JNI argument) to obtain an `int` —
   logged as `"nativeInitializeNativeFlags: flagCount = %d"` (string at `0x34b993`,
   tag `"rbx.JNIRobloxSettings"` at `0x4d0b6b`). This value becomes the constructor's
   sole `int` argument.
4. `NewObject(class, ctor, thatInt)` via a direct-call wrapper at `0x20b7658` — the
   `int` passed here is a **provider ID**, not a count of the array (see §3).
5. `GetArrayLength(env, arg3)` — **confirms the third JNI parameter is read as an
   array**, matching the dex descriptor.
6. Loop `i = 0 .. length-1`:
   - `GetObjectArrayElement(arg3, i)` → one array element (a `jstring`, per the
     descriptor).
   - A local helper (`0x20b6351`) extracts it into a small string wrapper (SSO
     inline buffer or heap-allocated, decided by a size check).
   - A local helper (`0x215a5ce`) looks that name up in an internal table (via a
     shared hash-table `find` at `0x215a48a` — the same low-level `find` also used,
     unrelated to flags, by another function this pass ran into at `0x68fab46`;
     `objdump`'s nearest-export label for it, `...FMOD_OutputAAudioHeadphonesChanged`,
     is nearest-symbol noise per `app-bridge.md` §4.1's caveat, not a real
     relationship).
   - Logs either `"nativeInitializeNativeFlags: ... %d: %s = %s"` (found, `0x3d6727`)
     or `"nativeInitializeNativeFlags: ... %d: %s not found."` (not found,
     `0x40eed1`) — **this is the log line the reproduction run below actually
     produced.**
   - `addBoolean(result, name, found, wasOverridden)`.
   - `ExceptionCheck` → if true, `ExceptionClear` (swallowed, execution continues).
   - `DeleteLocalRef` on the element, next iteration.
7. Stack-canary check, return the constructed `NativeFlagsInitResult`.

**No path through this function — including all three of its own internal, non-exported
helpers, which were disassembled in full — references the strings `"NativeHelper"` or
`"gameActivity_onFlagsFailed"`, and none of them can throw past the per-element
`ExceptionClear`.** Whatever ends up in the array, this function returns a validly
constructed result object. It is not, itself, capable of calling
`gameActivity_onFlagsFailed`, and nothing in it looks capable of segfaulting on its
own account either (SSO/heap string handling is bounds-checked; the hash lookup is a
generic `find`; the JNI calls are all standard, guarded ones).

---

## 2. The live bug: Cordial is passing the entire ClientSettings document as a single flag *name*

`native/init_params.cpp`'s `cordial_init_flags` (the C++ side of `cordial_init_flags`,
called from `crates/cordial-runtime/src/bin/load.rs`) contains this, **as of this
session, unedited by this investigation**:

```cpp
// The array is a list of flag *names to cache*, not a settings document.
//
// This was wrong for several iterations: passing Roblox's ClientSettings
// JSON here made the engine call addBoolean with the entire document as a
// single flag name, which is exactly what the trace showed. ...
// An empty list is therefore correct: cache nothing up front. ...
const bool have = settings_json && *settings_json;
auto arr = std::make_shared<jnivm::Array<jnivm::String>>(have ? 1 : 0);
if (have) {
    arr->Set(0, cordial::S_pub(settings_json));
}
```

The comment states the array should always be empty. **The code directly below it
does not do that** — when `--client-settings` points at a real file (as the
reproduction instructions in this task do), `have` is true and the entire file
contents become element 0 of a one-element `String[]`. This is a live discrepancy
between documented intent and actual behaviour, not a hypothesis: the reproduction
run below shows it happening.

### Reproduced

Ran the exact command given, against the already-built binary (did **not** run
`cargo build --release` first, to avoid racing the concurrent edits to
`native/*.cpp`):

```
CORDIAL_STUB_QUIET=1 timeout 100 ./target/release/cordial-load --lib-dir "$LIBDIR" \
  --apk "$APK" --client-settings /tmp/clientsettings.json --host-libc --game-activity --run 8
```

Result: **exit 139 (SIGSEGV)**, `timeout: the monitored command dumped core`. The
captured output includes:

```
[roblox] flags FAILED — the engine could not load its flag set
I/rbx.JNIRobloxSettings    nativeInitializeNativeFlags: ... 0: {"applicationSettings":{"DFFlagConsumePlatformNameOverAlternateName":"False", ... [4123 bytes, cuts off mid-JSON, no closing brace] ...
```

That second line is exactly step 6's "found"/"not found" log (§1), firing for array
index 0, whose "name" is the raw JSON text — confirming, at runtime, that the array
Cordial builds today is the one-element, whole-document array the comment says is
wrong. The line is truncated at roughly 4 KB, consistent with `__android_log_print`'s
own bounded formatting buffer (not a crash by itself — the line ends cleanly with a
newline).

**Caveat on ordering:** the `onFlagsFailed` print (via `fprintf(stderr, …)`, which
Cordial's `NativeHelper::onFlagsFailed` hook uses unbuffered) appears *before* the
`__android_log_print`-based line in the captured file, even though §1's disassembly
shows the per-element log is produced from inside `nativeInitializeNativeFlags`,
which — per §1 — cannot itself call `onFlagsFailed`. The most likely explanation is
buffering: the two log paths go through different sinks with different flush
timing, so file order does not necessarily reflect wall-clock order. Treat the
printed order as inconclusive; both facts (the call happened, the truncated-JSON
log line happened) are independently confirmed, their relative order is not.

**No usable backtrace could be obtained in this sandbox.** There is no `gdb`
installed, and `coredumpctl` (`core_pattern` is piped to `systemd-coredump`) lists
no core for this crash — its storage is not retaining dumps here (other processes'
crashes show the same `COREFILE missing`). This is worth recording so a future
session does not repeat the attempt: getting a real backtrace requires either
installing a debugger, or Cordial installing its own `SIGSEGV` handler (the
terminate-handler precedent in `native/jni_shim.cpp`, per `findings.md` §8.2, is the
right shape for this).

---

## 3. Where `gameActivity_onFlagsFailed` is actually called from — verified, with one open edge

A whole-binary scan (every rip-relative `lea` in `.text`, ~80.6 MB, checked against
its computed target — the same method `render-gate.md` §2 used for
`eglCreateWindowSurface`) for the string `"gameActivity_onFlagsFailed"` (the only
occurrence of that string in the file, at `0x40f096`) found **exactly one** site
referencing it: `0x29c931e`.

That address is inside a small, non-exported helper starting at `0x29c92eb`. Its
entire body:

```
GetMethodID(cachedClass, "gameActivity_onFlagsFailed", "()V")   ; class ref cached at 0x6eba788
CallVoidMethod(cachedInstance, thatMethodID)                     ; instance cached at 0x6eba780
```

— i.e. this is precisely the JNI call that reaches Cordial's
`NativeHelper::onFlagsFailed` hook (`native/init_params.cpp:279`). A sibling copy
seven bytes later (`0x29c937d`) is structurally identical but calls
`"gameActivity_onEngineInitialized"` instead (`0x462ada`) — both share the
`FLog::JNINativeHelper] FATAL: Java exception occurred in JNI call.` diagnostic
string (`0x40f0d3`), consistent with a generic "call this no-arg `NativeHelper`
callback, log FATAL if it throws" template instantiated per callback name.

**This `onFlagsFailed` helper has exactly one direct caller in the whole
binary**: `0x29c553c`. That call site is gated by:

```
this = <some object>                     ; entered with this in %rbx
if (this->[0x10] == null) goto skip;      // 0x29c54f4
handle = getter(this->[0x10]);            // 0x2932f40 — looks like weak_ptr::lock()
if (handle == null) goto skip;            // 0x29c550d
target = this->[0x8];
if (target != null) {
    target->[0x10] = 11;                    // marks a state/result field FAILED
    obj = target->[0x40]->[0x10]->[0x10];   // three more pointer dereferences
    report_onFlagsFailed(obj);              // 0x29c92eb — the call at 0x29c553c
}
// unconditionally: release `handle` (refcount decrement, virtual dtor if it hits 0)
```

(`skip` reaches the same release/cleanup path with no report — i.e. those two
null-checks are safety guards around an otherwise-unconditional report, not a
separate success/failure branch; whatever decided "this is the failure case"
happened earlier, outside what this pass could see.)

**The containing function (starts at `0x29c52cc`) has zero direct callers anywhere
in `.text`.** Exactly like `render-gate.md` §2's GPU-tier device-cache function, it
is reached only through an indirect call (a function pointer, `std::function`, or
virtual dispatch) that a direct-`call`-site scan cannot follow. This is the
honest edge of what this pass could establish: **what invokes this check, and
under what real-world condition `this->[0x8]` ends up non-null with state
worth marking `11`, is not determined here.**

What can be said with confidence: `this`, `this->[0x10]`, and `this->[0x8]` are
**not** Java objects and are **not** anything Cordial's JNI layer
(`NativeFlagsInitResult`, `NativeHelper`, etc.) constructs or touches — they are
private, internal C++ engine state, invisible to JNI entirely. Nothing in
`native/init_params.cpp` initialises or influences this object graph one way or
the other. If the crash is in the three-deep dereference chain
(`target->[0x40]->[0x10]->[0x10]`) immediately preceding the report call, or in
whatever indirectly invokes `0x29c52cc` at all, it would be because that internal
state was never populated the way the real Android app populates it before this
code path runs — consistent with, but not proven by, the reproduction in §2. This
is inference, not a verified fact; nailing it down needs a live debugger (§2's
caveat) or an instrumented breakpoint/wrapper at `0x29c52cc`'s entry, similar in
spirit to the libc-call wrapper technique `findings.md` §8.1 already uses.

---

## 4. `nativeRegisterJavaFlagProvider` — verified unreachable from Java in this build

`Java_com_roblox_client_flags_FlagJniInterface_nativeRegisterJavaFlagProvider` is
exported at `0x29aba7a` (57 bytes, ending well before the next export,
`MemStorage.setItem`). Checked against **all three** shipping dex files with
`tools/dex_method.py`, both restricted to the `FlagJniInterface` class and with an
unrestricted name search across every class:

```
$ python3 tools/dex_method.py apk/dex/ --class com/roblox/client/flags/FlagJniInterface
# lists 7 methods: nativeGetFFlag, nativeGetFInt, nativeGetFString,
# nativeInitializeNativeFlags, nativeRegisterFFlag, nativeRegisterFInt,
# nativeRegisterFString — no nativeRegisterJavaFlagProvider

$ python3 tools/dex_method.py apk/dex/ nativeRegisterJavaFlagProvider
no match
```

**No Java class in this shipping build declares this native method at all.** It
cannot be reached through the normal JNI static-linkage path (the
`Java_com_roblox_..._methodName` symbol convention only matters if some
`class.native(...)` declaration in the dex causes the JVM to look it up), and
nothing else in the dex calls it either. Its own body confirms it does not expect
to be called through the normal path: it takes the standard `(JNIEnv*, jclass, ...)`
JNI convention but **ignores every incoming argument** and simply:

```
GetOrRegisterProviderId(&globalProviderRegistry)   ; call 0x215a308 — the SAME
                                                     ; helper nativeInitializeNativeFlags's
                                                     ; own preamble calls (§1 step 3)
__android_log_print(INFO, "rbx.JNIRobloxSettings",
    "nativeRegisterJavaFlagProvider: Registered external flag provider from Java with ID: %d",
    result)
return result
```

The log format string (`0x4b41e6`) confirms the semantics: the shared counter at
`0x215a308` is a **provider-ID generator**, and this native is just an alternate
(unused, in this build) entry point into the same registration the constructor
step in §1 already performs inline. **Answer to the task's question: no, the flag
pipeline does not require calling it first — the real shipping app does not call
it either, and `nativeInitializeNativeFlags` already performs the equivalent
registration itself.** This rules it out as the missing piece.

---

## 5. What's verified vs inferred

**Verified** (disassembly, cross-checked against Cordial's own `jni.h` and
`native/init_params.cpp`, and against a live reproduction):
- `nativeInitializeNativeFlags`'s full body, all three of its private helpers,
  builds `NativeFlagsInitResult` exactly as Cordial's Java-side stub expects
  (§1), takes a `String[]` of flag *names* (not a document), and cannot itself
  call `onFlagsFailed` or crash on the input it's given.
- `native/init_params.cpp`'s `cordial_init_flags` currently packs the entire
  settings document as element 0 of that array when `--client-settings` is
  supplied, contradicting its own comment (§2) — reproduced live.
- The sole reference to `"gameActivity_onFlagsFailed"` in the binary, its sole
  caller, and that caller's gating logic (§3).
- `nativeRegisterJavaFlagProvider` is unreachable from Java in this dex set and
  is not a prerequisite (§4).
- The reproduction: exit 139/SIGSEGV, `onFlagsFailed` fires, the truncated
  whole-document-as-flag-name log line fires (§2).

**Inferred, not verified:**
- That the segfault is specifically the `target->[0x40]->[0x10]->[0x10]` chain in
  §3, or specifically triggered by whatever indirectly invokes `0x29c52cc`. No
  backtrace was obtainable in this sandbox (§2) to confirm the exact faulting
  instruction.
- Why `this->[0x8]` (§3) ends up non-null/marked-11 at all in Cordial's run — this
  requires knowing what real Android's flags subsystem normally does to populate
  it, which is internal engine state with no JNI-visible surface.

**Not established:**
- Whether fixing §2 (passing an empty array, as the code's own comment says it
  should) changes the outcome. Given §3's trigger looks independent of the
  array's contents — it's gated on unrelated internal engine state, not on
  anything `nativeInitializeNativeFlags` touches — there is no evidence either
  way that this fixes `onFlagsFailed`, only that it removes a confirmed,
  currently-live bug (a multi-megabyte string being hashed, logged, and searched
  for as if it were a flag name).

---

## 6. Recommendation

1. **Fix the discrepancy in `native/init_params.cpp`'s `cordial_init_flags`
   (§2) to match its own comment** — always construct a zero-length array,
   regardless of whether `settings_json` is non-empty. This is confirmed-wrong
   today and reproducibly wastes a multi-KB log line and a hash lookup on a
   string that can never be a real flag name. Low risk, because §1 shows an
   empty array makes the loop in step 6 simply not execute — the function still
   returns a validly constructed (if empty) `NativeFlagsInitResult`.
2. **Do not add a call to `nativeRegisterJavaFlagProvider`** — §4 shows it is
   unreachable in the real app and redundant with what
   `nativeInitializeNativeFlags` already does internally.
3. **Fix #1 is very unlikely to be sufficient on its own.** §3's trigger for
   `onFlagsFailed` is gated on internal engine object state
   (`this->[0x10]`, `this->[0x8]`) that has nothing to do with the JNI array
   argument — it is reached through an indirect call this pass could not
   resolve. Expect `onFlagsFailed` (and possibly the segfault) to persist after
   fix #1, unless it happens to be timing-sensitive in a way that an empty,
   fast-returning array changes.
4. **The highest-leverage next step is a working debugger or core-dump
   pipeline in this environment** — there is no `gdb` here and
   `systemd-coredump` is not retaining dumps (§2). Without a backtrace, closing
   §3's open edge (what calls `0x29c52cc`, and what exactly is null/dangling at
   the crash) requires either an external debugger, or an instrumented
   breakpoint/wrapper at `0x29c52cc`'s entry — printing `this`, `this->[0x10]`,
   and `this->[0x8]` at runtime, the same wrapper-based instrumentation
   technique `findings.md` §8.1 already used successfully for libc calls.
