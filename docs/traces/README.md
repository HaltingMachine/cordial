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
