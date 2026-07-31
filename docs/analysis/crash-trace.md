# The game-thread crash: what is established

**Status:** the fault is precisely located; its cause is not.

## The faulting object is `SingleSurfaceAppImpl`

Identified by reading its vtable's RTTI at the crash, not by inference:

```
(lldb) # at the fault, %rbx is the object
vtable  = *(void**)rbx            = 0x7ffff691aab8
typeinfo = *(void**)(vtable - 8)  = 0x7ffff691ab30
name     = *(char**)(typeinfo + 8) -> "20SingleSurfaceAppImpl"
```

So the engine's single-surface application object holds a null `JNIEnv*` at
`+0x400` and faults calling `FindClass` through it.

Related types in the binary: `16SingleSurfaceApp`, `17ISingleSurfaceApp`,
`20SingleSurfaceAppImpl`, and a Lua-side `App.SingleSurfaceAppLayer`. The class
has its own logging channel, `FLog::SingleSurfaceApp`, with messages including
`applyLocale`, `destroyLuaApp`, `replaceCurrentDataModel` and `destroy
controllers` — which is a good map of what it is responsible for.

**This is the single most useful fact for anyone continuing.** The question is no
longer "what is that struct" but "what initialises `SingleSurfaceAppImpl`'s JNI
environment, and when".

An attempt to turn on `FLog::SingleSurfaceApp` by passing `FLogSingleSurfaceApp`
and friends to `nativePreloadFlagOverrides` produced no extra output — either the
override format is different or FLog is not routed to `__android_log_print` in
this build. Worth another attempt; the engine narrating itself would be worth
more than any amount of further disassembly.

## Verified

The engine faults on its game thread (`thread #4`, named `Main`) at
`libroblox+0x2ccd937`:

```
mov  0x400(%rbx),%rdi
mov  (%rdi),%rax        <- faults, %rdi is null
call *0x30(%rax)
```

`0x30` into a `JNIEnv`'s function table is `FindClass`. So the engine holds a
struct whose field at `+0x400` should be a `JNIEnv*`, finds null there, and
dereferences it.

Established by breakpoint, not inference:

- The instruction that writes a pointer into some struct's `+0x400`
  (`libroblox+0x24eca2d`) **never executes**. A breakpoint on it is never hit
  before the crash.
- That write lives inside the initialiser of a function-local static — the
  standard `__cxa_guard_acquire` / `if (!(guard & 1)) construct()` shape — whose
  getter is at `libroblox+0x24ec960`.
- That getter has **155 call sites**, so it is a widely used engine service, not
  something reached only down one path.

## The watchpoint result — what actually happens to the field

A hardware watchpoint on the faulting field (`0x7fffb17c70a0`, set from a
breakpoint in Cordial's own `cordial_appbridge_init`, ASLR disabled so the
address reproduces) answers it:

- The field **is** written — twice, both times with **zero**.
- The write is at `libroblox+0x2186803`, which is
  `nativeAppBridgeV2InitWithParams+0x1543`, and it is a constructor zeroing a
  block: `movaps %xmm0,0x3c0(%rbx)` through `0x440(%rbx)` with `%xmm0` zero.
- Scanning the whole of `nativeAppBridgeV2InitWithParams` finds **no non-zeroing
  store to `+0x400` at all**.

So the object is created and zeroed by the app-bridge init on Cordial's calling
thread, and nothing ever puts a `JNIEnv` in it. The game thread then reads it and
faults. This is not a missing Java class or a wrong argument — it is a field the
engine expects some *other* path to populate, and that path is not running.

The obvious candidate is thread attachment: the engine may expect the thread that
will run the game to attach to the JavaVM and cache its environment here. Cordial
drives the whole bring-up from one thread, and AGDK's game thread was created
before the bridge object existed.

## Not established — and a correction

The write at `+0x24eca2d` was connected to the crash's `+0x400` **only because
the offsets matched**. That is a coincidence, not evidence: many structs have a
field at `0x400`, and a getter with 155 callers being constructed exactly zero
times is hard to reconcile with the rest of the engine running.

So the identity of the faulting struct is open, and the hour spent on that write
was spent on an unproven link. Recording it so the next attempt does not repeat
it.

## What would actually settle it

A watchpoint on `%rbx + 0x400` for the specific object, set once its address is
known and before the game thread touches it. ASLR is disabled in the lldb runs,
so addresses are reproducible between runs — the object was at
`0x7fffb17c6ca0`, putting the field at `0x7fffb17c70a0`. The obstacle is timing:
the watchpoint has to be set after the allocation and before the read, and
nothing convenient breaks in between.

`gdb` is not installed here; lldb can do it but needs a stopping point in the
middle of engine startup, which is the part still missing.

## Why the setter scan missed: an embedded sub-object

The vtable stored at the object's `+0` is loaded from exactly two places in the
binary, one of them inside `nativeAppBridgeV2InitWithParams` — so that native
constructs `SingleSurfaceAppImpl` directly, consistent with the zeroing seen at
`+0x21867c0`.

That zeroing covers `+0x3c0` through `+0x440` as one block, which is the shape of
an **embedded member**, not a scatter of independent fields. If the `JNIEnv` lives
inside that member, its setter writes at a small offset from a pointer to the
member — e.g. `mov %rax,0x40(%rbx)` with `%rbx == object + 0x3c0` — and a scan
for `0x400(%reg)` cannot see it.

That resolves the contradiction between "59 stores to `+0x400`, none executed" and
"the field must be written somewhere": it is written, but not at that literal
displacement. The watchpoint remains the authority — and it says nothing writes a
non-zero value to this object's field during the run.

## The setter is not findable by direct scan

Every `mov %reg,0x400(%reg)` in the binary — 59 of them — was breakpointed in a
single run. **None is hit** before the crash.

Combined with the watchpoint result (the field is written only by the
constructor's `movaps` zeroing), that means the value is not placed there by a
plain register store at all. Remaining possibilities, none yet tested:

- an immediate store (`movq $imm,0x400(%rbx)`)
- a `memcpy`/struct assignment that copies a whole block in
- a store through a base register that is not the object pointer, with the
  offset folded in by earlier arithmetic
- the object being expected to arrive already populated — i.e. copied or moved
  from another instance rather than constructed empty

The last is the most interesting: it would mean the bridge object is supposed to
be initialised *from* something, and Cordial is producing a fresh empty one.

## Inside `nativeAppBridgeStartLuaAppDM`

The crash happens under this native. Its first act is to load a global from
`.bss` at `0x6eb9e48` and branch on it:

```
mov  0x4ce7e0a(%rip),%rdi     # 0x6eb9e48, in .bss
test $0xfc00,%edi
jne  <alternate path>
```

**A watchpoint on it shows it is written, to `1030` (`0x406`).** So the "it is
still zero" reading was wrong — and it would not have mattered either way:
`0x406 & 0xfc00 == 0`, so the branch falls through exactly as it would at zero.
That test is not a gate on initialisation at all; `1030` looks like a count or
capacity. This line of reasoning is a dead end and is recorded so it is not
retried.

## Ruled out

- **Missing Java.** Unresolved JNI lookups are at zero; every class, method and
  field the engine reaches for resolves against a real implementation.
- **`nativeRegisterJavaFlagProvider`.** Absent from all three dex files; not a
  prerequisite (see `flag-init.md`).
- **The flag argument.** `nativeInitializeNativeFlags` takes flag *names*, not a
  settings document; passing ClientSettings there was wrong and is fixed.
- **Networking.** Every network primitive resolves to the host. The engine never
  calls `socket()`, so it fails before reaching the network.
- **Activity lifecycle.** All nine `JNIActivityLifecycleCallbacks` stages fire in
  Android's order. It changed nothing.
- **AGDK interference.** `CORDIAL_SKIP_AGDK=1` drives the app bridge with no
  `initializeNativeCode` and no GameActivity at all. Same crash, same
  instruction, on a thread Roblox creates itself — so it is not an artefact of
  running two bring-ups together.

---

# Resolved: libjnivm handed unknown threads a null ENV

The blocker was in Cordial, not in Roblox.

`jnivm::VM::GetEnv()` is, in full:

```cpp
const std::shared_ptr<ENV>& VM::GetEnv() {
    return jnienvs[pthread_self()];   // EnableJNIVMGC is on
}
```

`jnienvs` is an `unordered_map<pthread_t, shared_ptr<ENV>>`, so `operator[]` on
a thread the VM has never seen **default-constructs a null `shared_ptr`, inserts
it, and returns a reference to it**. The caller gets `nullptr`, with no error
raised anywhere.

`cordial::process_env()` was exactly that call, and all thirteen JNI hooks in
this build go through it. The engine invokes those hooks from threads it creates
itself, so on every one of those threads the hooks ran with a null `ENV` — and
the engine stored the null where it expected a `JNIEnv`. The field was never
"not written": it was written, with the null we supplied.

`AttachCurrentThread` is the entry point that *does* create an env for an
unknown thread (`if (!nenv) nenv = nvm.CreateEnv();`) and finds the existing one
when there is one, so `process_env()` now goes through it and the question of
which thread it is called on stops mattering.

The general form of this bug is worth remembering: **an `operator[]` on a map of
per-thread state silently manufactures a null entry for any thread that was not
registered.** It cannot fail loudly, and the damage surfaces arbitrarily far
away, on whichever thread happened to be unlucky.

## What it unblocked

`nativeAppBridgeV2InitWithParams` now completes. The engine proceeds into real
startup: it resolves the WebRTC audio classes and `NativeTextBoxInfo`, and makes
two full passes over `NativeUserJavaInterface` and `NativeLocaleJavaInterface`.

## Correction: the faulting call is not `FindClass`

Earlier notes here read `callq *0x30(%rax)` as JNI `FindClass`, because `0x30`
is `FindClass`'s slot in `JNINativeInterface`. That is very probably wrong. The
full sequence is:

```
movq (%rdi),%rax          ; rdi = *(SingleSurfaceAppImpl + 0x400), NULL
leaq -0x2c0(%rbp),%rsi    ; arg2 is a STACK ADDRESS
movq %r15,%rdx
callq *0x30(%rax)
```

`FindClass` takes a `const char*`; a pointer into the caller's own frame is the
shape of a `std::string` or a struct being passed by address. So this is an
ordinary **C++ virtual call, vtable slot 6**, on a null member — and `+0x400`
holds a *subsystem pointer*, not a `JNIEnv`. Reading a raw offset as a JNI slot
because the number matches is the same class of mistake as the `+0x400`/write
coincidence recorded above.

## The call chain, resolved

```
frame 0  libroblox+0x2ccd937                    (the virtual call above)
frame 1  nativeAppBridgeStartLuaAppDM +0x17f
frame 2  nativeGameGlobalInit +0xe64
frame 3  nativeGameGlobalInit +0xd3c
frame 4  nativeGameGlobalInit +0xbbb
frame 5  libc start_thread
```

The thread is named `Main` and **the engine spawns it itself** from inside
`nativeGameGlobalInit`; it then calls `nativeAppBridgeStartLuaAppDM` — the Lua
app DataModel, which is what this platform actually renders — without waiting
for us. Cordial calls `nativeAppBridgeV2StartAppWithParams` and hands over the
surface only *after* its own `StartLuaAppDM` call, so the ordering is a live
suspect: the engine's thread may be arriving at the app before the surface it
depends on exists.

The invoke interface is being used correctly by the engine, so this is not an
attach problem: `CORDIAL_JNI_TRACE=1` records 30 `GetEnv` and 2
`AttachCurrentThread` calls, all against our JavaVM.

---

# Client settings never load — the current best explanation

Two independent investigations converged on this from opposite ends: an audit of
the render path, and the flags failure recorded above.

**`nativeInitClientSettings`, `nativeInitClientSettingsSigned`,
`nativeInitClientSettingsCached[Compressed]` and
`nativePostClientSettingsLoadedInitialization3` are never called, and no path in
Cordial can call them.** They are wired only to an async network-response
callback (dex `vi/e$f`, methods `a`/`b`), never to the synchronous
`ActivityNativeMain.N2()` / `vi/e.j` chain Cordial replicates. Cordial never
constructs that callback, never drives it, and never times it out.

`InitParams::Create` hands the engine `baseURL = "https://www.roblox.com"`, so
the engine issues the real request. At the moment of the crash there are two
`HttpClient` threads: one blocked in `poll()`, one idle in `pthread_cond_wait`.
The engine is waiting for a client-settings response that never arrives.

The faulting code branches on two booleans and *then* reads the member that is
null — the shape of "have client settings arrived yet?" taking the not-yet path.
A sibling branch reads the same `[rbx+0x400]` and calls through it successfully,
so the member is legitimately populated on some other path. This is a state
problem, not a missing implementation.

Roblox's FastFlags *are* client settings, so this is very likely the same root
cause as `onFlagsFailed`, not a second bug.

## Correction: networking is no longer ruled out

The "Ruled out" list above says the engine never calls `socket()`. That was true
when written and is now false. It was true *because* the engine died earlier;
with the null-`ENV` bug fixed it gets far enough to actually attempt the network.

This is worth naming as a pattern, because it has now happened twice in this
file: **a fact established about a crashing program expires when the crash moves.**
Anything ruled out by "the engine never gets there" has to be re-checked after
any fix that gets it further.

## Correction: the ordering change is not the improvement it appeared to be

Calling `nativeAppBridgeV2StartAppWithParams` before
`nativeAppBridgeStartLuaAppDM` was reported to move the crash later, to
`libroblox+0x240462b`. On current `main` it does not reproduce: across four
consecutive runs the process dies while our thread is still inside
`nativeAppBridgeV2InitWithParams`, and the crash is the original
`libroblox+0x2ccd937`.

The engine's `Main` thread crashes on its own schedule, asynchronously, so which
site is reached depends on timing rather than on our call order — and the
accompanying claim that the thread is "blocked, not racing" does not hold. The
reorder is kept because it is harmless and matches the engine's own order, but
it is not a fix and should not be counted as progress.

**How to judge a fix from here:** not by how far Cordial's own log gets, which
is a race. By the crash address moving, and by the flags verdict line changing.

---

# The surface is fine — a disconfirmed fix

An RTTI/breakpoint trace of the second crash site (`libroblox+0x240462b`, fault
address `0x140`) established the object graph precisely:

```
SingleSurfaceApp        (static facade, RTTI "16SingleSurfaceApp")
  +0x20 -> SingleSurfaceAppImpl   (RTTI "20SingleSurfaceAppImpl", same object as before)
             +0x400 = 0          (the earlier crash's field)
             +0x410 = .bss addr  (POPULATED)
             +0x430 = 0          (this crash's field: null `this`)
```

The useful part is `+0x410`. A *neighbouring* slot in the same
`+0x3c0..+0x440` block — the block the constructor zeroes as one unit — is
populated. So that block is a **table of independent delegate pointers**, and
only some get filled during a run. This is selective failure, not an
uninitialised lump, which means each null slot has its own cause and none of
them is evidence about the others.

## The proposed fix was wrong

The accompanying inference was that `AppSurface` (`native/init_params.cpp`) is a
bare typed placeholder for `android.view.Surface` with no native window behind
it, so the engine cannot resolve `StartAppParams.surface` into a real window and
leaves the delegate null. The recommendation was to give it a real native peer.

**Disconfirmed by running it.** With `CORDIAL_ANDROID_TRACE=1`:

```
[android] ANativeWindow_fromSurface -> 0x55fd52fb4e18
```

The engine does call `ANativeWindow_fromSurface` on that Surface, and Cordial
returns a real, non-null `ANativeWindow` backed by the open X11 window. The
surface handoff works. Backing `AppSurface` with "a real native peer" would have
been effort spent on a component that is already correct.

This is the seventh confident diagnosis on this binary to fail on contact, and
it failed the same way as the others: the reasoning was sound, the code it
described really is a placeholder, and the conclusion still did not hold —
because the placeholder is sufficient. `ANativeWindow_fromSurface` is Cordial's
own implementation, so the Java `Surface` never needed state; the window is
resolved on our side regardless of what the object contains.

The trace now prints the returned pointer, not just the call, so this specific
question can never again be answered by inference.

## What it leaves

The engine takes the window and then does nothing with it — no
`setBuffersGeometry`, no `getWidth`/`getHeight`, no EGL. It has a valid window
and stops anyway, which puts the remaining suspicion back on engine *state*
rather than on the surface: most likely the client settings it is still waiting
for.

## Both bring-up paths crash in the same function

The full AGDK path segfaults too — `libroblox+0x2ccd912`, versus `+0x2ccd937` on
the `CORDIAL_SKIP_AGDK` path. Same call chain in both:

```
nativeGameGlobalInit -> nativeAppBridgeStartLuaAppDM -> here
```

on an engine-spawned thread named `Main`, fault address `0x0` in both.

`0x2ccd912` and `0x2ccd937` are 0x25 apart, inside the branch region already
disassembled here:

```
2ccd8bc: cmp  byte [flag1], 0
2ccd8c3: mov  al, [flag2]
   ...   branches on (flag1, flag2)
2ccd924: mov  rdi, [rbx+0x400]
2ccd937: mov  rax, [rdi]        <- skip-AGDK path faults here
```

So the two paths take *different branches of the same two-flag test* and both
reach a null pointer. That is consistent with the block at `+0x3c0..+0x440`
being a table of delegate pointers of which only some are populated: whichever
branch is taken, the slot it wants is empty.

It also means AGDK-vs-app-bridge is not the axis that matters. Both bring-ups
arrive at the same unpopulated state, so the missing initialisation is upstream
of the choice between them.
