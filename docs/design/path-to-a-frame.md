# The path to a frame

**Status:** mapped, not built. This is the remaining core of Phase 2.

Everything below is read from the shipping APK with `tools/dex_method.py`, not
inferred from AGDK's source — the signatures change between AGDK versions and
Roblox ships whichever it built against.

---

## 1. The contract

Roblox's game surface is AGDK `GameActivity`. Its native entry point is:

```java
long initializeNativeCode(
    String internalDataPath,
    String obbPath,
    String externalDataPath,
    AssetManager assetMgr,
    byte[] savedState,
    Configuration config)
```

The returned `long` is the native handle every subsequent callback carries. The
window lifecycle is then four calls:

```java
void onSurfaceCreatedNative(long handle, Surface surface)
void onSurfaceChangedNative(long handle, Surface surface, int format, int width, int height)
void onSurfaceRedrawNeededNative(long handle, Surface surface)
void onSurfaceDestroyedNative(long handle)
```

and input is:

```java
boolean onTouchEventNative(long handle, MotionEvent e, int, int, int, int, int,
                           long, long, int, int, int, int, int, int, float, float)
boolean onKeyDownNative(long handle, KeyEvent e)
boolean onKeyUpNative(long handle, KeyEvent e)
void    setInputConnectionNative(long handle, InputConnection ic)
```

That is the entire window and input surface Cordial has to satisfy. It is small,
and it is now known exactly rather than approximately.

## 2. What has to exist before the first call

`initializeNativeCode` takes four things Cordial does not have yet.

| Argument | What it needs | Where |
|---|---|---|
| `internalDataPath` | Already have it — `SessionReporterJavaInterface::files_dir()` creates a per-instance directory | done |
| `obbPath`, `externalDataPath` | Can be the same directory or empty; Android's expansion-file layout has no analogue here | trivial |
| `AssetManager` | **The real work.** Roblox reads its assets out of the APK, so this has to be backed by the zip | §3 |
| `Configuration` | Screen metrics, density, orientation, locale. This is the other half of §4.2's desktop identification — `DeviceStaticParams` covers the engine's view, `Configuration` covers the framework's | small |
| `savedState` | `null` on a cold start | trivial |

## 3. `AssetManager` is the gate

`Java_com_roblox_client_JNIAAssetManagerSetup_initNative` is exported, and the
native side reaches assets through `AAssetManager_*` from `libandroid.so` — 31
symbols, all currently stubbed.

The APK is a zip and the linker already links `libziparchive` for loading `.so`
objects straight out of one, so the reading machinery is present. What is missing
is the `AAsset*` API over it.

Nothing renders before this works: the engine cannot load a shader, a font, or a
texture without it.

## 4. The surface

`onSurfaceCreatedNative` takes a Java `Surface`, and the native side turns it into
an `ANativeWindow` to render through. So Cordial needs:

1. A host window — X11 or Wayland.
2. An EGL surface on it, via Mesa. GLES2 + EGL is the mandatory path;
   `libroblox.so` links both and only `dlopen`s Vulkan
   ([`findings.md`](../findings.md) §5(a)).
3. `ANativeWindow_*` implemented over that EGL surface, so the 17 `egl*` and 74
   `gl*` symbols — already resolving to host Mesa — have something to draw into.
4. A `Surface` object on the Java side whose native peer is that window.

`game-window` from the minecraft-linux stack (MIT) does X11/Wayland/SDL3 window
creation and is the obvious starting point
([`base-evaluation.md`](../base-evaluation.md) §1); it needs `linux-gamepad` to
build, which was not investigated.

## 5. Order

1. `ClassLoader.loadClass` / `findClass` and `Class.getClassLoader` — Roblox
   resolves classes by name at runtime and is asking for these now.
2. `AAssetManager` over the APK zip (§3). Nothing renders before this.
3. `Configuration` with desktop metrics.
4. Host window + EGL surface (§4).
5. `ANativeWindow_*` over it.
6. Call `initializeNativeCode`, then `onSurfaceCreatedNative` and
   `onSurfaceChangedNative`.
7. Input: `onKeyDownNative` / `onKeyUpNative` from host events. Touch can wait —
   `onTouchEventNative`'s seventeen parameters are worth doing once, properly,
   after there is something on screen to aim at.

## 6. Honest scale

Steps 2 and 4 are each larger than everything the framework layer has needed so
far. The asset manager is a real implementation of a documented API over a zip;
the window and EGL work is where Android-on-desktop projects historically lose
the most time, and Cordial has not started it.

What has changed is that none of it is unknown any more. Every signature above
came out of the APK, and the discovery loop
([`observed-java-surface.md`](../analysis/observed-java-surface.md)) means each
next requirement announces itself rather than having to be guessed.
