# Observed Java surface — what Roblox actually asks for

Recorded by `libjnivm`'s `VM::GenerateClassDump` during a real `JNI_OnLoad`,
not inferred from the dex. This is the difference between *which* Android APIs
are referenced (framework-api-inventory.md) and *which ones Roblox reaches for
on the startup path* — the second is the work queue.

Regenerate:

```bash
cordial-load --lib-dir <apk lib/x86_64> --host-libc --dump-classes out.cpp
```

## 22 classes

- `com/roblox/audio/AppRtcDeviceWrapper`
- `com/roblox/engine/jni/NativeGLJavaInterface`
- `com/roblox/engine/jni/locale/NativeLocaleJavaInterface`
- `com/roblox/engine/jni/memstorage/Connection`
- `com/roblox/engine/jni/model/DeviceStaticParams`
- `com/roblox/engine/jni/model/NativeTextBoxInfo`
- `com/roblox/engine/jni/reporter/SessionReporterJavaInterface`
- `com/roblox/engine/jni/user/NativeUserJavaInterface`
- `com/roblox/engine/jni/util/NetworkUtils`
- `com/roblox/engine/jni/video/MediaCodecInfoUtils`
- `com/roblox/engine/jni/video/VideoCodecCapability`
- `com/roblox/universalapp/logging/LoggingProtocol`
- `com/roblox/universalapp/messagebus/Connection`
- `com/snapchat/djinni/NativeObjectManager`
- `java/lang/ClassLoader`
- `org/fmod/AudioDevice`
- `org/fmod/FMOD`
- `org/fmod/MediaCodec`
- `org/webrtc/voiceengine/BuildInfo`
- `org/webrtc/voiceengine/WebRtcAudioManager`
- `org/webrtc/voiceengine/WebRtcAudioRecord`
- `org/webrtc/voiceengine/WebRtcAudioTrack`

## Implemented so far

`native/android_classes.cpp`. Roblox stops at the first null, so each of these
had to be answered before the next became visible at all.

| Class | State |
|---|---|
| `model/DeviceStaticParams` | 8 fields, desktop values |
| `model/NativeTextBoxInfo` | type only — exists so `showKeyboard`'s descriptor matches |
| `NativeGLJavaInterface` | device params, keyboard, leave/exit, overlay, orientation |
| `locale/NativeLocaleJavaInterface` | system locale, from `LANG`/`LC_*` |
| `user/NativeUserJavaInterface` | signed-out values |
| `reporter/SessionReporterJavaInterface` | files dir real; reporting inert |
| `video/MediaCodecInfoUtils` | no codecs — `libmediandk` is stubbed |
| `universalapp/logging/LoggingProtocol` | process uptime |

Outstanding on the startup path: `java/lang/ClassLoader` (`loadClass`, `findClass`)
and `Class.getClassLoader`, which Roblox uses to resolve classes by name at
runtime; and the `promptNativePurchase` overloads, which are not on any path that
matters yet.

### A note on descriptors

libjnivm matches a hook by the JNI descriptor it derives from the C++ signature.
`std::shared_ptr<Object>` becomes `Ljava/lang/Object;`, which will not match a
parameter typed as anything more specific — and the mismatch is silent: the hook
registers and is simply never called. `showKeyboard` needed
`Array<jbyte>` and the real `NativeTextBoxInfo` before it bound. When a hook
appears to do nothing, compare descriptors first.

## Raw dump

The generated C++ stubs are not committed — they are 537 lines of
machine-written scaffolding that goes stale with every Roblox release.
Regenerate them with the command above when implementing a class.
