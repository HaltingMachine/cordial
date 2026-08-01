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

## 1. Text entry — the last step before you can sign in

**The login form works.** Clicking Sign In on the landing page opens Roblox's
Lua-rendered login form — username, password with a reveal toggle, Quick Sign-in,
Forgot Password. Clicking a field focuses it and shows a caret. All of that is
verified by screenshot.

**What does not work is typing into it.** Characters do not appear. Everything
else about sign-in is now reachable, so this is the single remaining step.

The cause is almost certainly the same shape as the input bug it came out of.
Roblox reads text through its own on-screen-keyboard contract, and Cordial
implements none of the reverse half:

```text
nativeGetTextBoxInfo()                     -> NativeTextBoxInfo
syncTextboxTextAndCursorPosition2(String, I)
updateKeyboardSize(...)
nativeReturnPressedFromOnScreenKeyboard()
GameActivity.setSoftKeyboardActive(Z, I)   <- engine calls INTO Java
```

`nativePassText` and `nativePassKeyEvent` are already wired and are not enough on
their own. The engine calls *out* to Java to raise a soft keyboard and attach an
input connection, and nothing answers — so the field is focused with no text
source attached. Implementing that reverse contract is the job.

**Already ruled out, do not redo:** delivering keys through AGDK's
`onKeyDownNative` (accepted, ignored), delivering editing state through AGDK's
`onTextInputEventNative` with a populated `gametextinput/State` (accepted,
ignored), and re-sending window focus after the Lua app is up (no effect).

### What has since been established

Most of the reverse contract above is now wired, and the picture is much
narrower than "nothing answers".

**`showKeyboard`'s first argument is the handle of the box being edited.** It was
being discarded and text was then sent with handle 0, which the engine drops in
silence. Captured now, along with the box's current contents. `nativePassText`,
`syncTextboxTextAndCursorPosition2` and `updateKeyboardSize` are all driven and
all return without error, and `CORDIAL_TRACE_TEXT=1` shows focus detected and
text accumulating correctly. **It still does not appear in the field.**

**`updateKeyboardSize(visible=true)` destroys focus. This is the important one.**
The trace order is not ambiguous:

```text
textbox focused handle=139759059370112
updateKeyboardSize(visible=true)
textbox blurred
updateKeyboardSize(visible=false)
textbox focused handle=139759059370112
```

Focus bounces continuously while that call is driven. With
`CORDIAL_NO_KEYBOARD_REPORT=1` it is stable — one `focused`, no blur, confirmed
by control in the same session.

That also explains the field appearing to clear on every keystroke:
`edit_text_buffer` reseeds from the engine whenever the focus generation changes,
so a bouncing focus resets the buffer to empty between characters. The clearing
was self-inflicted, not the engine rejecting anything.

**Do not conclude `updateKeyboardSize` is useless and delete it.** The engine
asks for a keyboard, so something is expected to acknowledge one. What is wrong
is a bare `visible=true` with a zero-height rectangle at the window's bottom edge
— plausibly the engine re-lays-out around the reported keyboard and drops the
capture in the process. The call needs different arguments, or a different
moment, not removal.

**Ruled out as the cause of the bounce:** duplicate pointer delivery. Both AGDK's
`onTouchEventNative` and `NativeInputInterface` receive every click, so one press
does arrive twice — but disabling AGDK's copy (`CORDIAL_NO_AGDK_TOUCH=1`) leaves
the bounce exactly as it was.

### Where this is going

Synthesising an input method by hand is the wrong shape and is being abandoned.
Cordial becomes a **bridge**: the platform's own input method on one side,
Android's contract on the other. On Wayland that is `zwp_text_input_v3`, which
the compositor routes to whatever the user actually runs — ibus, fcitx,
squeekboard on a phone — so composition, dead keys and CJK candidate windows stop
being Cordial's to reimplement badly.

The Android half does not go away: the engine only speaks `showKeyboard` and
friends, because it is the Android build. What goes away is Cordial inventing the
editing state in the middle. See
[ADR-011](adr/ADR-011-wayland-and-libadwaita.md).

## 2. Sign-in itself

Without a session the client sits on the landing page. Avatar thumbnails fail
against user id 0 and there is nothing to do. `NativeUserJavaInterface` is
stubbed with an empty user.

**[`docs/design/sign-in.md`](design/sign-in.md) is the investigation.** Read it
before starting; it is careful about what is verified and what is inferred.

The short version: the blocker is **obtaining a session cookie**, not the stub
code. The engine's own HTTP client takes 401/403 from authenticated endpoints
regardless of what the Java-side user stubs return, so filling those in changes
nothing on its own.

**Good news, and it changes the plan: plain login does not need a WebView.**
Lua-rendered login is the *shipped default* in this build, established three
ways — the dex bytecode for what the native gates, the flag that controls it, and
the shipped content itself, which carries a full `Authentication.Login.*` string
table and a `LoginNative` screen name while `LoginWeb`/`LoginWebView` return zero
matches.

Reproduced here with a control:

```text
default                          nativeIsLuaLoginEnabled() -> true
FIntLuaAppLoginMethod=0          nativeIsLuaLoginEnabled() -> false
```

`CORDIAL_SIGNIN_PROBE=1` asks the engine directly.

**Captcha is still narrowed rather than settled** — there is a `CaptchaNative`
screen name, but also `Turnstile` and `CaptchaV2` strings suggesting
server-selected backends, so budget for a WebView on that path even though the
login form itself does not need one.

One correction that came out of it: the `The requested Ids are invalid`
thumbnail failure is what the **real, logged-out Android client** also produces.
It is not a Cordial defect and it is not evidence of anything.

## 3. Plugins: running, but with three methods

`crates/cordial-plugins` has capabilities, a broker, manifests, user grants and a
Deno host. `crates/cordial-runtime/src/plugin_host.rs` joins it to the client:
plugins are discovered, started after bring-up, and served from the real flag
resolver. Verified in a live launch — the example plugin in
[`plugins/flag-inspector`](../plugins/flag-inspector) reads a flag the user
actually set and is refused a capability it did not request.

**Update, still true where it matters:** `presence.set`/`presence.clear`,
`notify.send`, `url.open`, and the three `events.*` methods (ADR-006) are now
real, effect-performing brokers — Discord IPC framing, the freedesktop
notification and OpenURI portals, and an event registry with ownership rules
— all implemented and tested in `crates/cordial-plugins` (see `presence.rs`,
`notify.rs`, `urlopen.rs`, `events.rs`, and `host.rs`'s `Session`, which is the
single dispatcher that authorises and performs all of them). `lifecycle.read`
now has something to push, too: `Session::push_lifecycle` delivers `launch`,
`ready` and `shutdown` to any plugin holding the capability. A first-party
plugin, [`plugins/discord-presence`](../plugins/discord-presence), exercises
the whole path — verified against a local Discord IPC test double, not a real
Discord client (none is available where this was built); see the plugin's own
source comment.

**What is still missing is the join, not the brokers.** `serve()`'s `dispatch`
in `crates/cordial-runtime/src/plugin_host.rs` is a separate, older function
that predates `Session` and still only answers `flags.list`, `flags.get` and
`log.write`, falling through to `error: not implemented yet` for everything
else — including the four methods above, which now have real implementations
sitting unused one crate away. `plugin_host.rs` was out of scope for the work
that added them (other agents were active there), so the live client still
cannot broker Discord presence, notifications, URL-opening or plugin events
until `dispatch` is replaced with (or delegates to) `cordial_plugins::host::
Session::handle`, and `push_lifecycle` is called from wherever the client's
own launch/ready/shutdown transitions are detected. That wiring, plus
`flags.write` and `flags.write.dynamic`, is what remains.

See [ADR-003](adr/ADR-003-plugin-isolation.md) for why isolation is by process,
[ADR-004](adr/ADR-004-plugin-asset-overrides.md) for why plugins cannot replace
Roblox's assets, and [ADR-005](adr/ADR-005-flag-service.md) for why flag writes
are two capabilities.

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
| GLES ran at about 1 fps while Vulkan was fine | Every `eglSwapBuffers` blocked ~1.00 s inside Mesa. The engine asks for vsync and Xwayland owns no CRTC, so DRI3's vblank query fell back to a one-second wait. The interval Mesa receives is now forced to 0; 20 → 652 swaps in 20 s |
| Interface looked like a low-end phone | Surface hardcoded to 720p and `dpiScale` to 1.0 — Roblox lays out in dp and picks asset resolutions from exactly those |

## Branches

`archive/gameactivity-per-callback` holds per-callback GameActivity dispatch that
was never merged. **Read it, do not merge it** — it is built on the disproved
ALooper theory and restructures the App Bridge onto a worker thread to fix a hang
whose real cause was the asset path. Merging it produced code that did not
compile against current main. One good idea in it: sharing one GameActivity
`thiz` and one Surface across calls, the way Android does.
