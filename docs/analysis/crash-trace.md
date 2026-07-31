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
