# Ground-truth traces

`waydroid-roblox-startup.log.gz` — Roblox for Android (the same APK Cordial
loads) running under Waydroid on this machine, captured with `adb logcat`.

## Why this exists

Cordial spent a long time trying to *deduce* the engine's startup contract from
a stripped 116 MB binary. That approach has a poor record here — nine confident
diagnoses were disconfirmed by running the thing. This trace replaces inference
with observation: the same engine, on the platform it was built for, saying what
it does.

It is our own capture of an app's behaviour on its own platform. Nothing is
copied from anyone, and there is no provenance problem — unlike decompiled
proprietary binaries, which remain off-limits (see §16.1 and ADR-001).

## Reproducing it

```
waydroid show-full-ui                      # thaw the container
adb connect 192.168.240.112:5555
adb install-multiple base.apk split_config.x86_64.apk
adb logcat -G 16M && adb logcat -c
adb shell am start -n com.roblox.client/.startup.ActivitySplash
adb logcat -d > startup.log
```

Note the launcher activity is `com.roblox.client/.startup.ActivitySplash` — in
the `.startup` subpackage. `com.roblox.client/.ActivitySplash` does not exist,
and `monkey` silently fails to launch it.

## What it already answers

The real client's flag sequence, which Cordial has never reproduced:

```
nativeLogDurationEvent: [flag_prefetch_begin]   32
nativeLogDurationEvent: [fetch_flag_begin]      38
nativeLogDurationEvent: [fetch_flag_end]       127
nativeLogDurationEvent: [fastflag_load_success] 72
LoggingProtocol: Adding event to pending list: fastflag_load_success
```

So `fastflag_load_success` is a real, observable state, reached via a
prefetch/fetch pair — against Cordial's `onFlagsFailed`. `JNILoggingProtocol`
is also an app-to-engine channel Cordial does not implement.

Other immediately useful facts visible in the capture: `setBaseUrl() null =>
www.roblox.com`, DNS pre-warming for `apis.`/`friends.`/`locale.`/`users.`
roblox.com, and a `ShellConfigurationContentProvider` the app queries at
startup — a ContentProvider Cordial has no equivalent for.

## How to use it

When a question arises about what the engine expects, grep this first. It is
cheaper and far more reliable than another disassembly pass, and it is the
reason the remaining work should be mechanical rather than speculative.

## `native-flag-names.txt` — the 139 flags the real client registers

The real client's own log, tag `rbx.JNIRobloxSettings`:

```
nativeInitializeNativeFlags: Registered Flag Provider ID from Java: 0
nativeInitializeNativeFlags: flagCount = 139.
nativeInitializeNativeFlags: ... 0: EnableAndroidBinaryChannelDownloadTiming not found.
nativeInitializeNativeFlags: ... 5: FixAndroidWebDialogPaymentSessionId = true
```

So the argument really is a list of **flag names** (confirming the earlier
disassembly), the real count is **139**, and the engine reports each one as
either `not found` or `= <value>`. `docs/traces/native-flag-names.txt` is that
exact list, in order, extracted from the trace — the thing Cordial should pass
instead of an empty array.

Note also `Registered Flag Provider ID from Java: 0`. The provider ID is
supplied *by Java*, i.e. by the host app, which is a piece of the contract
Cordial has not been fulfilling.

## `flog-channels.txt` — FLog is routed to the Android log

Thirty `FLog::`/`DFLog::` channels appear in the capture, including
`FLog::AndroidGLView`, `DFLog::FlagCache`, `DFLog::HttpTraceError` and
`DFLog::RbxTransportIoLibContext`.

This settles a question that blocked progress for a long time: **the engine's own
logging does reach the Android log**, so the earlier conclusion that "FLog is not
routed in this build" was about Cordial's environment, not the engine. Getting
these channels to emit under Cordial would give the engine a voice, and the
trace shows which channels are worth enabling.

## Other contract details visible in the capture

- User-Agent: `... ROBLOX Android App 2.732.1043 Tablet Hybrid GooglePlayStore RobloxApp/2.732.1043`
  — note **Tablet Hybrid**, matching Cordial's `isTablet`/XLARGE choices.
- `BaseUrl = www.roblox.com/` — with a trailing slash.
- `Arch = X86_64`, `OS Ver. = 13, Lvl = 33`, `Build = googleProdRelease`.
- `ShellConfigurationContentProvider` is queried twice at startup via
  `content://com.roblox.client.ShellConfigurationProvider/config.get`.
