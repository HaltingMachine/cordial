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

/// `android.view.Surface`
///
/// The handle passed to `onSurfaceCreatedNative`. `ANativeWindow_fromSurface`
/// ignores it and returns Cordial's single window, so like the two above it is a
/// type rather than a carrier of state.
class Surface : public Object {
public:
    static void Register(ENV* env) {
        env->GetClass<Surface>("android/view/Surface");
    }
};

/// `com.google.androidgamesdk.GameActivity`, as an object to pass as `thiz`.
class GameActivity : public Object {
public:
    /// The engine asks the Activity for the screen it is drawing on and reads
    /// `density` off the answer. A null here stops it before it renders.
    std::shared_ptr<Object> getDisplayMetrics(ENV* env) {
        return make_display_metrics(env);
    }

    static void Register(ENV* env) {
        env->GetClass<GameActivity>("com/google/androidgamesdk/GameActivity");
        auto c = env->GetClass("com/google/androidgamesdk/GameActivity");
        c->HookInstanceFunction(env, "getDisplayMetrics", &GameActivity::getDisplayMetrics);
    }
};

void register_game_activity_classes(ENV* env) {
    ClassLoader::Register(env);
    AssetManager::Register(env);
    Configuration::Register(env);
    Surface::Register(env);
    GameActivity::Register(env);
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

        auto surface = std::make_shared<cordial::Surface>();
        auto jsurface = cordial::to_jni(env, surface);
        auto activity = std::make_shared<cordial::GameActivity>();
        auto jactivity = cordial::to_jni(env, activity);

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
