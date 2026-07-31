# Where to start

Roblox does not render under Cordial. It **does** run under Waydroid on this
machine, and that capture is committed — see `docs/traces/`.

## The one rule that matters

**Grep the trace before disassembling anything.** Over one long session, every
conclusion drawn by reading the stripped binary was wrong — nine in a row — and
every conclusion drawn by running something held up. The trace exists so that
"what does the engine expect here?" is a lookup, not an investigation.

Two of the nine were caught only because they were tested before being built on:
a fix for a `Surface` that turned out to already work, and a client-settings
theory that survived three careful arguments and died to one experiment.

## Start here: the futex

The driving thread parks in a futex during render bring-up. A wait like that is
almost always on a sync primitive **nobody is going to signal**, and the prime
candidate is an **EGL/GBM surface handshake that never completes**. That fits the
one confirmed fact nothing else explained: the engine takes a real, non-null
`ANativeWindow` and then never touches it again — no `setBuffersGeometry`, no
`getWidth`, no EGL.

Identify what that futex word belongs to before anything else. If it is a
graphics-side handshake, the wall may not be Cordial's client code at all.

## A limit on the trace, stated honestly

Cordial runs **natively on the host** (X11/Mesa), not inside the container. The
Waydroid capture is therefore trustworthy for **call order, names and contract**
— which is what it was taken for — but **not** for timing or render behaviour.
Roblox under Waydroid is reported to burn CPU with very little GPU utilisation,
with missing explicit sync suspected, and on NVIDIA the container's Android is
built against bionic while `nvidia-utils` is not. Do not read the capture's
rendering path as a model of a healthy one.

## Do not re-derive

The 139 flag names are already extracted and built in
(`crates/cordial-runtime/src/native-flag-names.txt`). The bring-up order is
already corrected. Both came from the capture; do not spend a session
rediscovering them.

## On observing Sober

An earlier version of this file said attaching a debugger to Sober raises "the
same provenance question" as reading its decompilation. That was wrong, and the
distinction matters enough to state properly.

**Decompilation reconstructs expression.** You end up reading a reconstruction of
their source and writing code from it, which is where derivative-work risk lives.
That is why `decompiled/` stays off-limits (§16.1, ADR-001).

**A debugger on a running process yields behaviour.** Which libraries it loads,
which natives it calls, in what order, with what arguments, what it maps where.
Those are facts and interfaces, not expression, and black-box observation for
interoperability is the ordinary basis for this kind of work rather than an edge
case.

So the line is not the tool, it is **what you take away**:

- Fine: the call sequence, the load order, argument shapes, which symbols get
  resolved, timing, syscalls. Anything you could in principle have discovered by
  watching the outside of the process.
- Not fine: stepping into Sober's own routines to read how it implements
  something and transcribing that logic. At that point the debugger is just a
  slower decompiler.

**And Sober is the better reference for this specific problem.** Waydroid runs a
full Android stack in a container, which is exactly why its render path is not a
model to copy (see the caveat above). Sober runs the same APK natively on the
host against the host GPU — the shape Cordial is aiming at. Its *sequencing* is
therefore more relevant ground truth than the Waydroid capture, particularly for
the EGL/surface handshake the futex is likely waiting on.

Worth doing, and worth recording in the commit which of the two kinds of
observation produced any given fact.

## The blocker, as precisely as it is known

Confirmed by runtime evidence:

- The **flags verdict does not gate rendering**. `onFlagsFailed` is a complaint,
  not a gate. The crash address moves between paths while the verdict stays
  constant. Do not spend time on it.
- **No EGL or GL call ever happens.** All counters read zero at the crash.
  `ANativeWindow_fromSurface` returns a real non-null window and nothing follows.
- At the moment of death the **driving thread is not crashed — it is blocked**,
  in a futex, inside Roblox's own code, underneath
  `nativeAppBridgeV2StartAppWithParams`. It never returns, so it never reaches
  the code that would create the EGL surface.
- It dies because a **different** engine thread segfaults first
  (`libroblox+0x2ccd912`), and any thread's SIGSEGV kills the process.
- It is **not** an unserviced ALooper. That was tested: the main thread pumps
  `epoll_wait` continuously on a dedicated thread while the worker still blocks
  at the identical futex.

So the question is **what that futex is waiting on**. That is the whole problem.

## Cheap things not yet tried

- Enable the engine's own logging. Thirty `FLog::`/`DFLog::` channels are live in
  the Waydroid capture (`docs/traces/flog-channels.txt`), including
  `FLog::AndroidGLView`. An earlier note here claiming FLog is unrouted was wrong
  — it is routed on real Android, so the channels work; Cordial just has not
  turned them on. Getting the engine to narrate itself is worth more than any
  amount of further static analysis.
- Diff Cordial's JNI call sequence against the trace directly. Cordial's own
  `[JNIVM]` log and the logcat capture are both ordered lists of the same
  bring-up; the first divergence is the thing to fix.
- Drive the remaining GameActivity callbacks. Only 4 of ~23 were being called;
  adding `onWindowFocusChangedNative` alone provoked three new engine call-outs
  (`setImeEditorInfoFields`, `setWindowFlags`, `getWindowInsets`) and that
  isolated path exits cleanly. `agent/wt-agdk` has that work.
- Implement `ShellConfigurationContentProvider`
  (`content://com.roblox.client.ShellConfigurationProvider/config.get`), which the
  real client queries twice at startup and Cordial has no equivalent for.

## Debugging facts that cost time to learn

- **lldb breakpoints inside `libroblox.so` do not work.** Cordial `mmap`s it
  outside the system linker, so lldb never lists the image and every breakpoint
  stays unresolved with hit count 0 — silently. The only working technique is
  `memory write` of `0xCC`, then rewinding `$pc` and restoring the byte on trap.
  Crash-stop backtraces and breakpoints in Cordial's own code are unaffected.
- **There are three threads named `Main`**: Cordial's driving thread, the AGDK
  looper-service thread, and one the engine spawns. Use `thread backtrace all`;
  any note in this repo saying "the engine's Main thread" is ambiguous.
- **`CORDIAL_SKIP_AGDK=1` skips the flag and app-bridge calls entirely.** Several
  results were measured on a path that never ran the code under test.
- Roblox's launcher activity is `com.roblox.client/.startup.ActivitySplash`, in
  the `.startup` subpackage. `monkey` fails silently on the wrong name.

## Branches

`agent/wt-agdk` has the per-callback GameActivity work and the worker-thread
restructure; it is not merged. `agent/ordering` and `agent/flags` are merged.
