// Driving AGDK `GameActivity` bring-up.
//
// On Android the platform calls `GameActivity.initializeNativeCode` from Java
// with a real Activity behind it. Cordial has no Java, so it constructs the
// arguments through libjnivm and calls the exported JNI native directly:
//
//     jlong Java_com_google_androidgamesdk_GameActivity_initializeNativeCode(
//         JNIEnv*, jobject thiz,
//         jstring internalDataPath, jstring obbPath, jstring externalDataPath,
//         jobject assetMgr, jbyteArray savedState, jobject config)
//
// The descriptor came out of the shipping APK with tools/dex_method.py, not from
// AGDK's source — it changes between versions and Roblox ships whichever it
// built against.
//
// The returned handle is what every later callback carries:
// onSurfaceCreatedNative, onSurfaceChangedNative, onKeyDownNative and the rest.

#include <jnivm.h>

#include <cstdio>
#include <cstring>
#include <memory>
#include <mutex>
#include <string>

namespace cordial {
std::shared_ptr<jnivm::Object> make_display_metrics(jnivm::ENV* env);
std::shared_ptr<jnivm::Object> make_resources(jnivm::ENV* env);
void set_display_size(int width, int height);

/// Convert a C++ object into a `jobject` the way libjnivm expects.
///
/// A raw `cordial::to_jni(env, p)` looks right — libjnivm does
/// represent a `jobject` as its own `Object*` — but it skips the two things
/// `ToJNIType` does on the way:
///
///   * it sets `obj->clazz`, without which `GetObjectClass` returns null and
///     libjnivm falls back to `FindClass("Invalid")`. Every field and method
///     lookup on the object then resolves against the wrong class and yields
///     nothing.
///   * it parks the `shared_ptr` in the environment's local frame, which is what
///     keeps the object alive for the duration of the call.
///
/// The failure is silent: the call succeeds, the engine reads its parameters
/// through a classless receiver, gets nothing, and carries on into its failure
/// path.
template <class T>
static auto to_jni(jnivm::ENV* env, const std::shared_ptr<T>& p) {
    return jnivm::JNITypes<std::shared_ptr<T>>::ToJNIType(env, p);
}
jnivm::ENV* process_env();

using jnivm::Class;
using jnivm::ENV;
using jnivm::Object;
using jnivm::String;

namespace {
std::shared_ptr<String> jstr(const char* v) {
    return std::make_shared<String>(std::string(v ? v : ""));
}
} // namespace

/// `java.lang.ClassLoader`
///
/// Roblox resolves classes by name at runtime rather than only through
/// `FindClass`, so it asks the current `Class` for its loader and calls
/// `loadClass`. Both resolve to the same place: whatever libjnivm has registered.
class ClassLoader : public Object {
public:
    static std::shared_ptr<jnivm::Class> loadClass(ENV* env, Object*, std::shared_ptr<String> name) {
        if (!name) {
            return nullptr;
        }
        // Java uses dots, the JNI uses slashes, and callers are inconsistent
        // about which they pass here.
        std::string path = *name;
        for (auto& ch : path) {
            if (ch == '.') {
                ch = '/';
            }
        }
        return env->GetClass(path.c_str());
    }

    static void Register(ENV* env) {
        env->GetClass<ClassLoader>("java/lang/ClassLoader");
        auto c = env->GetClass("java/lang/ClassLoader");
        c->HookInstanceFunction(env, "loadClass", &ClassLoader::loadClass);
        c->HookInstanceFunction(env, "findClass", &ClassLoader::loadClass);
    }
};

/// `android.content.res.AssetManager`
///
/// Deliberately empty. The native side reaches assets through
/// `AAssetManager_fromJava`, which Cordial answers with its single process-wide
/// manager (see `android::asset`), so this object carries no state — it exists
/// to satisfy `initializeNativeCode`'s signature.
class AssetManager : public Object {
public:
    static void Register(ENV* env) {
        env->GetClass<AssetManager>("android/content/res/AssetManager");
    }
};

/// `android.content.res.Configuration`
///
/// Likewise empty: the native side reads configuration through
/// `AConfiguration_*` rather than this object's fields.
class Configuration : public Object {
public:
    static void Register(ENV* env) {
        env->GetClass<Configuration>("android/content/res/Configuration");
    }
};

/// `com.google.androidgamesdk.GameActivity`, as an object to pass as `thiz`.
class GameActivity : public Object {
public:

    static void Register(ENV* env) {
        env->GetClass<GameActivity>("com/google/androidgamesdk/GameActivity");
        auto c = env->GetClass("com/google/androidgamesdk/GameActivity");
    }
};

/// `android.view.MotionEvent`, synthesised from an X11 pointer event.
///
/// `onTouchEventNative`'s signature carries the event's scalar fields as
/// unpacked primitive arguments (see `cordial_game_activity_touch`, below) — that
/// unpacking is exactly what AGDK's own Java-side `processMotionEvent` does
/// before calling the native. What is *not* unpacked, and has to come from this
/// object when the native side (`GameActivityMotionEvent_fromJava` in AGDK's own
/// C++, statically linked into libroblox.so) asks for it, is the per-pointer
/// data: `getPointerId`, `getToolType`, `getRawX`/`getRawY` (SDK 29+), and
/// `getAxisValue` for whichever axes AGDK has enabled — X and Y by default,
/// which is all a single mouse pointer needs.
///
/// This mapping is not guessed: it is read directly out of AGDK's own
/// `GameActivityEvents.cpp` (Apache-2.0, google/agdk via the game-activity
/// prefab), which is the code that actually calls back onto whatever object
/// Roblox was handed here. With `historySize` always 0 in this implementation
/// (no motion coalescing), `getHistoricalEventTime`/`getHistoricalAxisValue` are
/// registered — because AGDK unconditionally resolves the method IDs at
/// `initializeNativeCode` time — but never actually invoked.
/// Whether to present input as a touchscreen rather than a mouse.
///
/// Read once. An input device that changed identity mid-session would be a
/// stranger thing than either choice.
bool input_is_touch() {
    static const bool v = [] {
        const char* e = getenv("CORDIAL_INPUT_TOUCH");
        return e && *e && *e != '0';
    }();
    return v;
}

class MotionEvent : public Object {
public:
    jint deviceId = 1;
    // InputDevice.SOURCE_MOUSE = SOURCE_CLASS_POINTER(0x2) | 0x2000, or
    // SOURCE_TOUCHSCREEN = SOURCE_CLASS_POINTER(0x2) | 0x1000 with
    // CORDIAL_INPUT_TOUCH=1. Roblox's Android UI may bind only touch handlers,
    // in which case a perfectly well-formed mouse event is consumed by the
    // input dispatcher and then ignored by the interface — which is exactly the
    // symptom: onTouchEventNative returns true and nothing moves.
    jint source = cordial::input_is_touch() ? 0x00001002 : 0x00002002;
    jint action = 0;
    jlong eventTime = 0, downTime = 0;
    jint flags = 0, metaState = 0, actionButton = 0, buttonState = 0;
    jfloat x = 0.0f, y = 0.0f;
    // TOOL_TYPE_MOUSE, or TOOL_TYPE_FINGER under CORDIAL_INPUT_TOUCH=1. Kept
    // consistent with `source` and with PlatformParams' isMouseDevice/
    // isTouchDevice, because claiming to be a mouse in one place and a finger in
    // another is the kind of inconsistency an input stack is entitled to reject.
    jint toolType = cordial::input_is_touch() ? 1 : 3;

    jint getPointerId(ENV*, jint) { return 0; }
    jint getToolType(ENV*, jint) { return toolType; }
    jfloat getRawX(ENV*, jint) { return x; }
    jfloat getRawY(ENV*, jint) { return y; }
    jfloat getXPrecision(ENV*) { return 1.0f; }
    jfloat getYPrecision(ENV*) { return 1.0f; }
    // AMOTION_EVENT_AXIS_X = 0, AMOTION_EVENT_AXIS_Y = 1 — the only two axes
    // AGDK enables by default (GameActivityEvents.cpp's `enabledAxes`).
    jfloat getAxisValue(ENV*, jint axis, jint) {
        if (axis == 0) return x;
        if (axis == 1) return y;
        return 0.0f;
    }
    jlong getHistoricalEventTime(ENV*, jint) { return eventTime; }
    jfloat getHistoricalAxisValue(ENV*, jint, jint, jint) { return 0.0f; }

    // Registered for completeness — any caller reading the object directly
    // rather than through the unpacked primitives gets real values instead of
    // an unresolved-symbol stub — but not on AGDK's own call path.
    jint getDeviceId(ENV*) { return deviceId; }
    jint getSource(ENV*) { return source; }
    jint getAction(ENV*) { return action; }
    jlong getEventTime(ENV*) { return eventTime; }
    jlong getDownTime(ENV*) { return downTime; }
    jint getFlags(ENV*) { return flags; }
    jint getMetaState(ENV*) { return metaState; }
    jint getActionButton(ENV*) { return actionButton; }
    jint getButtonState(ENV*) { return buttonState; }
    jint getClassification(ENV*) { return 0; }
    jint getEdgeFlags(ENV*) { return 0; }
    jint getHistorySize(ENV*) { return 0; }
    jint getPointerCount(ENV*) { return 1; }

    static std::shared_ptr<MotionEvent> Create(ENV* env, jfloat x, jfloat y, jint action,
                                               jint buttonState, jint actionButton,
                                               jlong eventTime, jlong downTime) {
        auto p = std::make_shared<MotionEvent>();
        p->x = x;
        p->y = y;
        p->action = action;
        p->buttonState = buttonState;
        p->actionButton = actionButton;
        p->eventTime = eventTime;
        p->downTime = downTime;
        to_jni(env, p);
        return p;
    }

    static void Register(ENV* env) {
        env->GetClass<MotionEvent>("android/view/MotionEvent");
        auto c = env->GetClass("android/view/MotionEvent");
        c->HookInstanceFunction(env, "getPointerId", &MotionEvent::getPointerId);
        c->HookInstanceFunction(env, "getToolType", &MotionEvent::getToolType);
        c->HookInstanceFunction(env, "getRawX", &MotionEvent::getRawX);
        c->HookInstanceFunction(env, "getRawY", &MotionEvent::getRawY);
        c->HookInstanceFunction(env, "getXPrecision", &MotionEvent::getXPrecision);
        c->HookInstanceFunction(env, "getYPrecision", &MotionEvent::getYPrecision);
        c->HookInstanceFunction(env, "getAxisValue", &MotionEvent::getAxisValue);
        c->HookInstanceFunction(env, "getHistoricalEventTime", &MotionEvent::getHistoricalEventTime);
        c->HookInstanceFunction(env, "getHistoricalAxisValue", &MotionEvent::getHistoricalAxisValue);
        c->HookInstanceFunction(env, "getDeviceId", &MotionEvent::getDeviceId);
        c->HookInstanceFunction(env, "getSource", &MotionEvent::getSource);
        c->HookInstanceFunction(env, "getAction", &MotionEvent::getAction);
        c->HookInstanceFunction(env, "getEventTime", &MotionEvent::getEventTime);
        c->HookInstanceFunction(env, "getDownTime", &MotionEvent::getDownTime);
        c->HookInstanceFunction(env, "getFlags", &MotionEvent::getFlags);
        c->HookInstanceFunction(env, "getMetaState", &MotionEvent::getMetaState);
        c->HookInstanceFunction(env, "getActionButton", &MotionEvent::getActionButton);
        c->HookInstanceFunction(env, "getButtonState", &MotionEvent::getButtonState);
        c->HookInstanceFunction(env, "getClassification", &MotionEvent::getClassification);
        c->HookInstanceFunction(env, "getEdgeFlags", &MotionEvent::getEdgeFlags);
        c->HookInstanceFunction(env, "getHistorySize", &MotionEvent::getHistorySize);
        c->HookInstanceFunction(env, "getPointerCount", &MotionEvent::getPointerCount);
    }
};

/// `android.view.KeyEvent`, synthesised from an X11 key event.
///
/// Unlike `MotionEvent`, `onKeyDownNative`/`onKeyUpNative` carry no unpacked
/// primitives at all — the whole event is this object, and AGDK's
/// `GameActivityKeyEvent_fromJava` (same source file as the MotionEvent mapping
/// above) calls every one of these accessors directly.
/// `com.google.androidgamesdk.gametextinput.State` — the whole editing state.
///
/// Android text fields do not receive keystrokes. They receive *state*: the
/// complete contents of the field, the selection, and any in-progress composing
/// region from an IME. Cordial had no implementation of this, which is why keys
/// reached `onKeyDownNative` and the login form's text boxes stayed empty — the
/// engine resolves these five fields and nothing was ever answering them.
///
/// Fields rather than getters, matching the real class, which libjnivm binds by
/// name and descriptor.
class TextInputState : public Object {
public:
    std::shared_ptr<String> text = std::make_shared<String>(std::string());
    jint selectionStart = 0;
    jint selectionEnd = 0;
    // -1 means "no composing region", which is what a physical keyboard
    // produces — composition is an IME concept and there is no IME here.
    jint composingRegionStart = -1;
    jint composingRegionEnd = -1;

    static void Register(ENV* env) {
        env->GetClass<TextInputState>("com/google/androidgamesdk/gametextinput/State");
        auto c = env->GetClass("com/google/androidgamesdk/gametextinput/State");
        c->HookInstance(env, "text", &TextInputState::text);
        c->HookInstance(env, "selectionStart", &TextInputState::selectionStart);
        c->HookInstance(env, "selectionEnd", &TextInputState::selectionEnd);
        c->HookInstance(env, "composingRegionStart", &TextInputState::composingRegionStart);
        c->HookInstance(env, "composingRegionEnd", &TextInputState::composingRegionEnd);
    }
};

class KeyEvent : public Object {
public:
    jint deviceId = 1;
    // InputDevice.SOURCE_KEYBOARD = SOURCE_CLASS_BUTTON(0x1) | 0x100.
    jint source = 0x00000101;
    jint action = 0;
    jlong eventTime = 0, downTime = 0;
    jint flags = 0, metaState = 0, modifiers = 0, repeatCount = 0;
    jint keyCode = 0, scanCode = 0, unicodeChar = 0;

    jint getDeviceId(ENV*) { return deviceId; }
    jint getSource(ENV*) { return source; }
    jint getAction(ENV*) { return action; }
    jlong getEventTime(ENV*) { return eventTime; }
    jlong getDownTime(ENV*) { return downTime; }
    jint getFlags(ENV*) { return flags; }
    jint getMetaState(ENV*) { return metaState; }
    jint getModifiers(ENV*) { return modifiers; }
    jint getRepeatCount(ENV*) { return repeatCount; }
    jint getKeyCode(ENV*) { return keyCode; }
    jint getScanCode(ENV*) { return scanCode; }
    jint getUnicodeChar(ENV*) { return unicodeChar; }

    static std::shared_ptr<KeyEvent> Create(ENV* env, jboolean down, jint keyCode, jint scanCode,
                                            jint metaState, jint repeatCount, jint unicodeChar,
                                            jlong eventTime, jlong downTime) {
        auto p = std::make_shared<KeyEvent>();
        // ACTION_DOWN = 0, ACTION_UP = 1.
        p->action = down ? 0 : 1;
        p->keyCode = keyCode;
        p->scanCode = scanCode;
        p->metaState = metaState;
        p->modifiers = metaState;
        p->repeatCount = repeatCount;
        p->unicodeChar = unicodeChar;
        p->eventTime = eventTime;
        p->downTime = downTime;
        to_jni(env, p);
        return p;
    }

    static void Register(ENV* env) {
        env->GetClass<KeyEvent>("android/view/KeyEvent");
        auto c = env->GetClass("android/view/KeyEvent");
        c->HookInstanceFunction(env, "getDeviceId", &KeyEvent::getDeviceId);
        c->HookInstanceFunction(env, "getSource", &KeyEvent::getSource);
        c->HookInstanceFunction(env, "getAction", &KeyEvent::getAction);
        c->HookInstanceFunction(env, "getEventTime", &KeyEvent::getEventTime);
        c->HookInstanceFunction(env, "getDownTime", &KeyEvent::getDownTime);
        c->HookInstanceFunction(env, "getFlags", &KeyEvent::getFlags);
        c->HookInstanceFunction(env, "getMetaState", &KeyEvent::getMetaState);
        c->HookInstanceFunction(env, "getModifiers", &KeyEvent::getModifiers);
        c->HookInstanceFunction(env, "getRepeatCount", &KeyEvent::getRepeatCount);
        c->HookInstanceFunction(env, "getKeyCode", &KeyEvent::getKeyCode);
        c->HookInstanceFunction(env, "getScanCode", &KeyEvent::getScanCode);
        c->HookInstanceFunction(env, "getUnicodeChar", &KeyEvent::getUnicodeChar);
    }
};

namespace {
/// The single `GameActivity` `thiz` shared by every native call in this
/// process, the way Android hands the real Activity to all of them. A fresh
/// object per call (the previous behaviour here) is a needless difference
/// from what the engine actually receives — `GameActivity` carries no
/// per-call state in this file — and there is exactly one Activity to model.
std::shared_ptr<GameActivity>& shared_activity(ENV* env) {
    static std::shared_ptr<GameActivity> activity;
    if (!activity) {
        activity = std::make_shared<GameActivity>();
        to_jni(env, activity);
    }
    return activity;
}

/// The `Surface` object threaded through `onSurfaceCreatedNative`,
/// `onSurfaceChangedNative` and `onSurfaceRedrawNeededNative` — the three
/// calls Android makes against one surface's lifetime, which
/// `onSurfaceDestroyedNative` ends. A fresh object per call would let engine
/// code that compares `Surface` identity (e.g. "is this a resize of the
/// surface I already have, or a new one?") see every call as an unrelated new
/// surface. `make_new` starts a new lifetime; only `onSurfaceCreatedNative`
/// passes true.
std::shared_ptr<jnivm::Object>& shared_surface(ENV* env, bool make_new) {
    static std::shared_ptr<jnivm::Object> surface;
    if (make_new || !surface) {
        surface = std::make_shared<jnivm::Object>();
        to_jni(env, surface);
    }
    return surface;
}
} // namespace

void register_game_activity_classes(ENV* env) {
    ClassLoader::Register(env);
    AssetManager::Register(env);
    Configuration::Register(env);
    GameActivity::Register(env);
    MotionEvent::Register(env);
    KeyEvent::Register(env);
    TextInputState::Register(env);
}

} // namespace cordial

extern "C" {

/// Call `initializeNativeCode` and return its handle, or 0.
///
/// `err` receives a message on failure. Exceptions are contained here for the
/// same reason as in jni_shim.cpp: one crossing the Rust boundary is a core dump
/// with no explanation.
long cordial_game_activity_init(void* fn, const char* internal_path, const char* obb_path,
                                const char* external_path, char* err, size_t err_len) {
    using Init = jlong (*)(JNIEnv*, jobject, jstring, jstring, jstring, jobject, jbyteArray,
                           jobject);
    // Taken from the VM here rather than passed in. A `JNIEnv*` and a
    // `jnivm::ENV*` are unrelated types that both arrive as `void*`, and
    // confusing them does not fail at the boundary — it fails much later, as a
    // call through a null slot in what was assumed to be the function table.
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM or no initializeNativeCode");
        return 0;
    }

    try {
        auto activity = std::make_shared<cordial::GameActivity>();
        auto assets = std::make_shared<cordial::AssetManager>();
        auto config = std::make_shared<cordial::Configuration>();

        auto internal = cordial::jstr(internal_path);
        auto obb = cordial::jstr(obb_path);
        auto external = cordial::jstr(external_path);

        // libjnivm represents a `jobject` as its own `Object*`, so the shared_ptrs
        // above convert by taking their raw pointer. They stay in scope for the
        // duration of the call, which is what keeps the objects alive — the
        // engine must not retain them past this without its own reference.
        auto j = [env](const auto& p) { return cordial::to_jni(env, p); };

        return reinterpret_cast<Init>(fn)(
            env->GetJNIEnv(),
            j(activity),
            cordial::to_jni(env, internal),
            cordial::to_jni(env, obb),
            cordial::to_jni(env, external),
            j(assets),
            // savedState is null on a cold start, which this always is.
            nullptr,
            j(config));
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return 0;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return 0;
    }
}

} // extern "C"

extern "C" {

/// Drive the Activity lifecycle and hand the engine its surface.
///
/// Android's order is onCreate, onStart, onResume, then the surface callbacks as
/// the window becomes available. AGDK's natives are registered on the
/// `GameActivity` class rather than exported, so they are reached through the
/// JNI method table rather than `dlsym`.
///
/// Every call carries the handle `initializeNativeCode` returned.
int cordial_game_activity_start(long handle, int width, int height, int format,
                                char* err, size_t err_len) {
    auto* env = cordial::process_env();
    if (!env || handle == 0) {
        snprintf(err, err_len, "no JavaVM, or initializeNativeCode gave no handle");
        return -1;
    }

    try {
        JNIEnv* jni = env->GetJNIEnv();
        auto cls = env->GetClass("com/google/androidgamesdk/GameActivity");
        if (!cls) {
            snprintf(err, err_len, "GameActivity class is not registered");
            return -1;
        }

        // The natives are called directly rather than through GetMethodID and
        // CallVoidMethod, because libjnivm's method lookup deliberately excludes
        // them: `AllowNative == (bool)namesp->native` is false for an ordinary
        // GetMethodID, and CallVoidMethod dispatches on `nativehandle`, which
        // RegisterNatives never sets. Going through JNI silently finds nothing
        // and does nothing — which is exactly what it did.
        //
        // RegisterNatives does keep the raw function pointers, so this looks them
        // up there and calls them with the signatures AGDK registered.
        auto native = [&](const char* name) -> void* {
            std::lock_guard<std::mutex> lock(cls->mtx);
            auto it = cls->natives.find(name);
            return it == cls->natives.end() ? nullptr : it->second;
        };

        // The shared thiz and the surface starting a new lifetime — see
        // `shared_activity`/`shared_surface`'s own doc comments.
        auto jactivity = cordial::to_jni(env, cordial::shared_activity(env));
        auto jsurface = cordial::to_jni(env, cordial::shared_surface(env, /*make_new=*/true));

        using HandleOnly = void (*)(JNIEnv*, jobject, jlong);
        using SurfaceFn = void (*)(JNIEnv*, jobject, jlong, jobject);
        using SurfaceChangedFn = void (*)(JNIEnv*, jobject, jlong, jobject, jint, jint, jint);

        // Lifecycle first: the engine builds its renderer on resume and ignores a
        // surface that arrives before it is ready for one.
        if (auto f = native("onStartNative")) {
            reinterpret_cast<HandleOnly>(f)(jni, jactivity, (jlong)handle);
        }
        if (auto f = native("onResumeNative")) {
            reinterpret_cast<HandleOnly>(f)(jni, jactivity, (jlong)handle);
        }

        auto created = native("onSurfaceCreatedNative");
        if (!created) {
            snprintf(err, err_len, "onSurfaceCreatedNative was never registered");
            return -1;
        }
        reinterpret_cast<SurfaceFn>(created)(jni, jactivity, (jlong)handle, jsurface);

        // Size and format come after creation; this is what tells the engine how
        // big its framebuffers have to be.
        if (auto f = native("onSurfaceChangedNative")) {
            reinterpret_cast<SurfaceChangedFn>(f)(jni, jactivity, (jlong)handle, jsurface,
                                                  (jint)format, (jint)width, (jint)height);
        }

        // The content rectangle, which on Android arrives from the view layout
        // pass. Nothing here performs one, so the engine would never be told
        // where inside the window it is allowed to draw.
        using RectFn = void (*)(JNIEnv*, jobject, jlong, jint, jint, jint, jint);
        if (auto f = native("onContentRectChangedNative")) {
            reinterpret_cast<RectFn>(f)(jni, jactivity, (jlong)handle, 0, 0, (jint)width,
                                        (jint)height);
        }

        // Focus, last, and it matters more than it looks. An Android game that
        // has never been told it has the window renders as if it were in the
        // background — which is what about one frame per second is. Cordial
        // drove the lifecycle up to onResume and then never sent this.
        using FocusFn = void (*)(JNIEnv*, jobject, jlong, jboolean);
        if (auto f = native("onWindowFocusChangedNative")) {
            reinterpret_cast<FocusFn>(f)(jni, jactivity, (jlong)handle, JNI_TRUE);
        }
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

} // extern "C"

extern "C" {

/// A `GameActivity` native shaped `(J)V` — every lifecycle stage that carries
/// no argument beyond the handle: `onPauseNative`, `onStopNative`,
/// `onSurfaceDestroyedNative`, and `terminateNativeCode` itself.
///
/// `terminateNativeCode` is not exported the way `initializeNativeCode` is —
/// `nm -D` on the shipping `libroblox.so` shows only
/// `Java_com_google_androidgamesdk_GameActivity_initializeNativeCode` among
/// its defined symbols. It is one of the 24 natives AGDK registers
/// dynamically through `RegisterNatives` during `initializeNativeCode`,
/// exactly like `onPauseNative` and the rest, so it is looked up here the
/// same way — by name, in `cls->natives` — rather than through `dlsym`.
///
/// Returns 0 on success, -1 on error (`err` populated), or -2 if
/// `native_name` was never registered.
int cordial_game_activity_lifecycle(long handle, const char* native_name, char* err,
                                    size_t err_len) {
    auto* env = cordial::process_env();
    if (!env || handle == 0) {
        snprintf(err, err_len, "no JavaVM, or no native handle");
        return -1;
    }
    try {
        JNIEnv* jni = env->GetJNIEnv();
        auto cls = env->GetClass("com/google/androidgamesdk/GameActivity");
        if (!cls) {
            snprintf(err, err_len, "GameActivity class is not registered");
            return -1;
        }
        void* fn;
        {
            std::lock_guard<std::mutex> lock(cls->mtx);
            auto it = cls->natives.find(native_name);
            fn = it == cls->natives.end() ? nullptr : it->second;
        }
        if (!fn) {
            return -2;
        }
        using HandleOnly = void (*)(JNIEnv*, jobject, jlong);
        auto jactivity = cordial::to_jni(env, cordial::shared_activity(env));
        reinterpret_cast<HandleOnly>(fn)(jni, jactivity, (jlong)handle);
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

/// `onWindowFocusChangedNative(J Z)V`, callable in both directions: `true` at
/// bring-up (see `cordial_game_activity_start`, which still drives that call
/// inline) and `false` at teardown — Android sends this immediately before
/// `onPauseNative` when a run ends, the same way it sends the `true` case
/// immediately after `onResumeNative` when one starts.
int cordial_game_activity_window_focus(long handle, int focused, char* err, size_t err_len) {
    auto* env = cordial::process_env();
    if (!env || handle == 0) {
        snprintf(err, err_len, "no JavaVM, or no native handle");
        return -1;
    }
    try {
        JNIEnv* jni = env->GetJNIEnv();
        auto cls = env->GetClass("com/google/androidgamesdk/GameActivity");
        if (!cls) {
            snprintf(err, err_len, "GameActivity class is not registered");
            return -1;
        }
        void* fn;
        {
            std::lock_guard<std::mutex> lock(cls->mtx);
            auto it = cls->natives.find("onWindowFocusChangedNative");
            fn = it == cls->natives.end() ? nullptr : it->second;
        }
        if (!fn) {
            return -2;
        }
        using FocusFn = void (*)(JNIEnv*, jobject, jlong, jboolean);
        auto jactivity = cordial::to_jni(env, cordial::shared_activity(env));
        reinterpret_cast<FocusFn>(fn)(jni, jactivity, (jlong)handle,
                                      (jboolean)(focused ? JNI_TRUE : JNI_FALSE));
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

/// `onSurfaceRedrawNeededNative(J Landroid/view/Surface;)V` — Android's "the
/// compositor needs a frame before the next normal draw slot" nudge. Driven
/// here from X11 `Expose`: a damaged window (uncovered, restored, redirected
/// through a compositor) would otherwise sit un-repainted until whatever the
/// engine's own next frame happens to be, which on a stalled or backgrounded
/// engine may not come for a while, if at all.
///
/// Uses the *existing* surface (`make_new=false`) — this does not start a new
/// surface lifetime, it re-announces the current one, exactly as Android does
/// when asking an already-created surface to be redrawn.
int cordial_game_activity_surface_redraw_needed(long handle, char* err, size_t err_len) {
    auto* env = cordial::process_env();
    if (!env || handle == 0) {
        snprintf(err, err_len, "no JavaVM, or no native handle");
        return -1;
    }
    try {
        JNIEnv* jni = env->GetJNIEnv();
        auto cls = env->GetClass("com/google/androidgamesdk/GameActivity");
        if (!cls) {
            snprintf(err, err_len, "GameActivity class is not registered");
            return -1;
        }
        void* fn;
        {
            std::lock_guard<std::mutex> lock(cls->mtx);
            auto it = cls->natives.find("onSurfaceRedrawNeededNative");
            fn = it == cls->natives.end() ? nullptr : it->second;
        }
        if (!fn) {
            return -2;
        }
        using SurfaceFn = void (*)(JNIEnv*, jobject, jlong, jobject);
        auto jactivity = cordial::to_jni(env, cordial::shared_activity(env));
        auto jsurface = cordial::to_jni(env, cordial::shared_surface(env, /*make_new=*/false));
        reinterpret_cast<SurfaceFn>(fn)(jni, jactivity, (jlong)handle, jsurface);
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

} // extern "C"

extern "C" {

/// Deliver a synthesised mouse pointer event through `onTouchEventNative`.
///
/// `action` is a caller-supplied Android `MotionEvent.ACTION_*` constant; this
/// function does not interpret X11 semantics itself, only the AGDK call
/// contract — the X11-to-Android policy (which button maps to which action,
/// hover vs. drag) lives on the Rust side, in `android::window`.
///
/// Returns 0 on success with `*consumed` set to the engine's boolean result,
/// -1 on error (`err` populated), or -2 if `onTouchEventNative` has not been
/// registered yet — a normal race against `initializeNativeCode` early in
/// startup, not a failure worth reporting as one.
///
/// Wrapped in `PushLocalFrame`/`PopLocalFrame`: unlike the once-per-launch
/// calls elsewhere in this file, this runs once per input event, and
/// `cordial::to_jni` parks every object it touches in the current local frame
/// (see its own doc comment) — without popping, a long session would grow that
/// frame without bound.
/// `NativeInputInterface.nativePassMouseMove(F,F,F,F)` and
/// `nativePassMouseButton(F,F,Z,I)`.
///
/// This is the input path Roblox's *interface* actually reads. AGDK's
/// `onTouchEventNative` is a different pipe: it accepts events and returns true,
/// and the engine buffers them, but the Lua app shell never hit-tests anything
/// delivered that way. Feeding only AGDK produced a client where every click was
/// accepted and nothing on screen ever moved — pixel-identical before and after,
/// including hover.
///
/// Signatures read from the shipping APK's dex, not guessed.
/// `GameActivity.onTextInputEventNative(J, State)`.
///
/// Delivers the field's entire contents, not a keystroke. The caller owns the
/// buffer and sends the whole thing each time it changes, which is what the real
/// Android implementation does when an IME edits the text.
int cordial_game_activity_text_input(long handle, const char* text, int sel_start, int sel_end,
                                     char* err, size_t err_len) {
    auto* env = cordial::process_env();
    if (!env || handle == 0) {
        snprintf(err, err_len, "no JavaVM, or no native handle");
        return -1;
    }
    try {
        JNIEnv* jni = env->GetJNIEnv();
        auto cls = env->GetClass("com/google/androidgamesdk/GameActivity");
        if (!cls) {
            snprintf(err, err_len, "GameActivity class is not registered");
            return -1;
        }
        auto it = cls->natives.find("onTextInputEventNative");
        if (it == cls->natives.end()) {
            snprintf(err, err_len, "onTextInputEventNative was never registered");
            return -1;
        }
        auto state = std::make_shared<cordial::TextInputState>();
        state->text = std::make_shared<cordial::String>(std::string(text ? text : ""));
        state->selectionStart = sel_start;
        state->selectionEnd = sel_end;

        using Call = void (*)(JNIEnv*, jobject, jlong, jobject);
        reinterpret_cast<Call>(it->second)(jni, (jobject)cordial::to_jni(env, cordial::shared_activity(env)), (jlong)handle,
                                           (jobject)cordial::to_jni(env, state));
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

/// `NativeGLInterface.nativePassKeyEvent(Z down, I keyCode, I modifiers, Z isRepeat)`.
///
/// Roblox's own keyboard path, the counterpart to `nativePassMouseButton`. AGDK's
/// `onKeyDownNative` is accepted and ignored by the interface in exactly the way
/// `onTouchEventNative` was.
int cordial_input_key_event(void* fn, int down, int key_code, int modifiers, int is_repeat,
                            char* err, size_t err_len) {
    using Call = void (*)(JNIEnv*, jobject, jboolean, jint, jint, jboolean);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or nativePassKeyEvent is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass("com/roblox/engine/jni/NativeGLInterface");
        reinterpret_cast<Call>(fn)(env->GetJNIEnv(), (jobject)cordial::to_jni(env, cls),
                                   down ? JNI_TRUE : JNI_FALSE, key_code, modifiers,
                                   is_repeat ? JNI_TRUE : JNI_FALSE);
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

/// `NativeGLInterface.nativePassText(J, String, Z, I)` — text entered into a
/// focused text box, which is a different thing from a key being pressed.
int cordial_input_pass_text(void* fn, long long which, const char* text, int flag, int cursor,
                            char* err, size_t err_len) {
    using Call = void (*)(JNIEnv*, jobject, jlong, jstring, jboolean, jint);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or nativePassText is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass("com/roblox/engine/jni/NativeGLInterface");
        auto str = std::make_shared<cordial::String>(std::string(text ? text : ""));
        reinterpret_cast<Call>(fn)(env->GetJNIEnv(), (jobject)cordial::to_jni(env, cls),
                                   (jlong)which, (jstring)cordial::to_jni(env, str),
                                   flag ? JNI_TRUE : JNI_FALSE, cursor);
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

int cordial_input_mouse_move(void* fn, float x, float y, float dx, float dy, char* err,
                             size_t err_len) {
    using Call = void (*)(JNIEnv*, jobject, jfloat, jfloat, jfloat, jfloat);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or nativePassMouseMove is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass("com/roblox/engine/jni/NativeInputInterface");
        reinterpret_cast<Call>(fn)(env->GetJNIEnv(), (jobject)cordial::to_jni(env, cls), x, y, dx,
                                   dy);
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

int cordial_input_mouse_button(void* fn, float x, float y, int down, int button, char* err,
                               size_t err_len) {
    using Call = void (*)(JNIEnv*, jobject, jfloat, jfloat, jboolean, jint);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or nativePassMouseButton is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass("com/roblox/engine/jni/NativeInputInterface");
        reinterpret_cast<Call>(fn)(env->GetJNIEnv(), (jobject)cordial::to_jni(env, cls), x, y,
                                   down ? JNI_TRUE : JNI_FALSE, button);
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

int cordial_game_activity_touch(long handle, int action, float x, float y, int button_state,
                                int action_button, long long event_time_ms,
                                long long down_time_ms, int* consumed, char* err,
                                size_t err_len) {
    auto* env = cordial::process_env();
    if (!env || handle == 0) {
        snprintf(err, err_len, "no JavaVM, or no native handle");
        return -1;
    }
    try {
        JNIEnv* jni = env->GetJNIEnv();
        auto cls = env->GetClass("com/google/androidgamesdk/GameActivity");
        if (!cls) {
            snprintf(err, err_len, "GameActivity class is not registered");
            return -1;
        }
        void* fn;
        {
            std::lock_guard<std::mutex> lock(cls->mtx);
            auto it = cls->natives.find("onTouchEventNative");
            fn = it == cls->natives.end() ? nullptr : it->second;
        }
        if (!fn) {
            return -2;
        }

        jni->PushLocalFrame(8);

        auto jactivity = cordial::to_jni(env, cordial::shared_activity(env));
        auto event = cordial::MotionEvent::Create(env, x, y, action, button_state, action_button,
                                                  (jlong)event_time_ms, (jlong)down_time_ms);
        auto jevent = cordial::to_jni(env, event);

        using TouchFn = jboolean (*)(JNIEnv*, jobject, jlong, jobject, jint, jint, jint, jint,
                                     jint, jlong, jlong, jint, jint, jint, jint, jint, jint,
                                     jfloat, jfloat);
        jboolean r = reinterpret_cast<TouchFn>(fn)(
            jni, jactivity, (jlong)handle, jevent,
            /*pointerCount=*/1, /*historySize=*/0, /*deviceId=*/event->deviceId,
            /*source=*/event->source, /*action=*/(jint)action,
            /*eventTime=*/(jlong)event_time_ms, /*downTime=*/(jlong)down_time_ms,
            /*flags=*/0, /*metaState=*/0, /*actionButton=*/(jint)action_button,
            /*buttonState=*/(jint)button_state, /*classification=*/0, /*edgeFlags=*/0,
            /*precisionX=*/1.0f, /*precisionY=*/1.0f);

        jni->PopLocalFrame(nullptr);
        if (consumed) {
            *consumed = r ? 1 : 0;
        }
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

/// Deliver a synthesised key event through `onKeyDownNative`/`onKeyUpNative`.
///
/// `down` selects which of the two natives is called; both share the single
/// `(J, KeyEvent) -> Z` signature. See `cordial_game_activity_touch`'s doc
/// comment for the return-code convention and the local-frame wrapping.
int cordial_game_activity_key(long handle, int down, int key_code, int scan_code, int meta_state,
                              int repeat_count, int unicode_char, long long event_time_ms,
                              long long down_time_ms, int* consumed, char* err, size_t err_len) {
    auto* env = cordial::process_env();
    if (!env || handle == 0) {
        snprintf(err, err_len, "no JavaVM, or no native handle");
        return -1;
    }
    try {
        JNIEnv* jni = env->GetJNIEnv();
        auto cls = env->GetClass("com/google/androidgamesdk/GameActivity");
        if (!cls) {
            snprintf(err, err_len, "GameActivity class is not registered");
            return -1;
        }
        const char* name = down ? "onKeyDownNative" : "onKeyUpNative";
        void* fn;
        {
            std::lock_guard<std::mutex> lock(cls->mtx);
            auto it = cls->natives.find(name);
            fn = it == cls->natives.end() ? nullptr : it->second;
        }
        if (!fn) {
            return -2;
        }

        jni->PushLocalFrame(8);

        auto jactivity = cordial::to_jni(env, cordial::shared_activity(env));
        auto event = cordial::KeyEvent::Create(env, down ? JNI_TRUE : JNI_FALSE, (jint)key_code,
                                               (jint)scan_code, (jint)meta_state,
                                               (jint)repeat_count, (jint)unicode_char,
                                               (jlong)event_time_ms, (jlong)down_time_ms);
        auto jevent = cordial::to_jni(env, event);

        using KeyFn = jboolean (*)(JNIEnv*, jobject, jlong, jobject);
        jboolean r = reinterpret_cast<KeyFn>(fn)(jni, jactivity, (jlong)handle, jevent);

        jni->PopLocalFrame(nullptr);
        if (consumed) {
            *consumed = r ? 1 : 0;
        }
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

} // extern "C"
