# The game-thread crash: what is established

**Status:** the fault is precisely located; its cause is not.

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
