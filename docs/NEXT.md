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
