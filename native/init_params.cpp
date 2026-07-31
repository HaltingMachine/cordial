// `MainGameActivity.nativeAppBridgeSetInitParams` — the engine's launch state.
//
// Modern Roblox renders its own app shell rather than using Java UI, so the
// engine draws nothing until it has been told where the service lives, what the
// device is, and what the viewport looks like. That is this object.
//
// Field names and types were read out of the shipping APK's dex, not guessed —
// `InitParams`, `DeviceParams` and `PlatformParams` are plain field-carrying
// classes and libjnivm binds field hooks by name and descriptor.
//
// `PlatformParams` is also where spec §4.2's "Roblox thinks you're mobile" is
// really answered: `isKeyboardDevice`, `isMouseDevice` and `isTouchDevice` decide
// which input scheme and which UI layout the engine picks.

#include <jnivm.h>

#include <cstdio>
#include <memory>
#include <mutex>
#include <string>

namespace cordial {

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

using jnivm::Class;
using jnivm::ENV;
using jnivm::Object;
using jnivm::String;

jnivm::ENV* process_env();
std::shared_ptr<String> jstr_shared(const char* v);

namespace {
std::shared_ptr<String> S(const char* v) {
    return std::make_shared<String>(std::string(v ? v : ""));
}
} // namespace

std::shared_ptr<String> S_pub(const char* v) { return S(v); }

/// `com.roblox.engine.jni.model.DeviceParams`
class DeviceParams : public Object {
public:
    std::shared_ptr<String> appBuildVariant, appVersion, country, deviceName, deviceSku;
    std::shared_ptr<String> displayResolution, manufacturer, networkType, osVersion;
    std::shared_ptr<String> socModel, testDeviceName;
    jboolean cpu64Bit = true;
    jboolean isChrome = false;
    jboolean isLowRamDevice = false;
    jint deviceTotalMemoryMB = 8192;
    jint displayPhysicalWidthPixels = 1280;
    jint displayPhysicalHeightPixels = 720;
    jint largeMemoryClass = 512;
    jint memoryClass = 256;
    jlong lowMemoryKillerBackgroundAppThreshold = 0;
    jlong lowMemoryKillerForegroundAppThreshold = 0;

    static std::shared_ptr<DeviceParams> Create(int width, int height) {
        auto p = std::make_shared<DeviceParams>();
        p->appBuildVariant = S("release");
        p->appVersion = S("");
        p->country = S("US");
        p->deviceName = S("Cordial");
        p->deviceSku = S("cordial");
        p->manufacturer = S("Cordial");
        p->socModel = S("cordial");
        p->osVersion = S("15");
        p->testDeviceName = S("");
        // Reported as "not on a metered mobile connection". The engine uses this
        // to decide how aggressively to stream assets.
        p->networkType = S("WIFI");
        char res[64];
        snprintf(res, sizeof(res), "%dx%d", width, height);
        p->displayResolution = S(res);
        p->displayPhysicalWidthPixels = width;
        p->displayPhysicalHeightPixels = height;
        return p;
    }

    static void Register(ENV* env) {
        env->GetClass<DeviceParams>("com/roblox/engine/jni/model/DeviceParams");
        auto c = env->GetClass("com/roblox/engine/jni/model/DeviceParams");
#define F(name) c->HookInstance(env, #name, &DeviceParams::name)
        F(appBuildVariant); F(appVersion); F(country); F(deviceName); F(deviceSku);
        F(displayResolution); F(manufacturer); F(networkType); F(osVersion);
        F(socModel); F(testDeviceName); F(cpu64Bit); F(isChrome); F(isLowRamDevice);
        F(deviceTotalMemoryMB); F(displayPhysicalWidthPixels); F(displayPhysicalHeightPixels);
        F(largeMemoryClass); F(memoryClass);
        F(lowMemoryKillerBackgroundAppThreshold); F(lowMemoryKillerForegroundAppThreshold);
#undef F
    }
};

/// `com.roblox.engine.jni.model.PlatformParams`
class PlatformParams : public Object {
public:
    std::shared_ptr<String> assetFolderPath;
    jfloat dpiScale = 1.0f;
    jboolean isKeyboardDevice = true;
    jboolean isMouseDevice = true;
    jboolean isTouchDevice = false;
    jint viewportWidthMm = 338;
    jint viewportHeightMm = 190;

    static std::shared_ptr<PlatformParams> Create(const char* assets, int width, int height) {
        auto p = std::make_shared<PlatformParams>();
        p->assetFolderPath = S(assets);
        // This is the desktop answer, and it is the point: keyboard and mouse
        // present, touch absent. It decides the input scheme and the UI layout
        // the engine chooses, which is what §4.2 is actually about.
        p->isKeyboardDevice = true;
        p->isMouseDevice = true;
        p->isTouchDevice = false;
        p->dpiScale = 1.0f;
        // Physical size at roughly 96 DPI, which is what a desktop display is.
        // A phone's 400+ DPI here would make the engine scale its UI for a
        // screen held at arm's length.
        p->viewportWidthMm = static_cast<jint>(width * 25.4 / 96.0);
        p->viewportHeightMm = static_cast<jint>(height * 25.4 / 96.0);
        return p;
    }

    static void Register(ENV* env) {
        env->GetClass<PlatformParams>("com/roblox/engine/jni/model/PlatformParams");
        auto c = env->GetClass("com/roblox/engine/jni/model/PlatformParams");
#define F(name) c->HookInstance(env, #name, &PlatformParams::name)
        F(assetFolderPath); F(dpiScale); F(isKeyboardDevice); F(isMouseDevice);
        F(isTouchDevice); F(viewportWidthMm); F(viewportHeightMm);
#undef F
    }
};

/// `com.roblox.engine.jni.autovalue.InitParams`
class InitParams : public Object {
public:
    std::shared_ptr<String> baseURL, buildVariant, userAgent;
    std::shared_ptr<DeviceParams> deviceParams;
    std::shared_ptr<PlatformParams> platformParams;
    std::shared_ptr<Object> vrContext;
    jboolean isPotato = false;
    jboolean isTablet = false;
    jboolean isVrDevice = false;

    static std::shared_ptr<InitParams> Create(const char* assets, int width, int height) {
        auto p = std::make_shared<InitParams>();
        p->baseURL = S("https://www.roblox.com");
        p->buildVariant = S("release");
        // The engine sends this on every request. It is Roblox's own client
        // string, not Cordial's: the service routes and gates on it, and a
        // fabricated one would be both untrue and likely rejected.
        p->userAgent = S("Roblox/Android");
        p->deviceParams = DeviceParams::Create(width, height);
        p->platformParams = PlatformParams::Create(assets, width, height);
        // "Potato" is Roblox's own name for a device below the quality floor.
        p->isPotato = false;
        // Tablet rather than phone: a desktop window is a large screen, and this
        // agrees with the XLARGE reported through AConfiguration.
        p->isTablet = true;
        p->isVrDevice = false;
        return p;
    }

    static void Register(ENV* env) {
        env->GetClass<InitParams>("com/roblox/engine/jni/autovalue/InitParams");
        auto c = env->GetClass("com/roblox/engine/jni/autovalue/InitParams");
#define F(name) c->HookInstance(env, #name, &InitParams::name)
        F(baseURL); F(buildVariant); F(userAgent); F(deviceParams); F(platformParams);
        F(vrContext); F(isPotato); F(isTablet); F(isVrDevice);
#undef F
        // AutoValue exposes the fields through accessors as well, and the engine
        // uses whichever the generated class provided.
#define G(name) c->HookInstanceGetterFunction(env, #name, &InitParams::name)
        G(baseURL); G(buildVariant); G(userAgent); G(deviceParams); G(platformParams);
        G(vrContext); G(isPotato); G(isTablet); G(isVrDevice);
#undef G
    }
};

class InitParams;

void register_init_params_classes(ENV* env) {
    DeviceParams::Register(env);
    PlatformParams::Register(env);
    InitParams::Register(env);
}

} // namespace cordial

extern "C" {

/// Call `MainGameActivity.nativeAppBridgeSetInitParams(InitParams)`.
int cordial_set_init_params(void* fn, const char* assets, int width, int height, char* err,
                            size_t err_len) {
    using Call = void (*)(JNIEnv*, jobject, jobject);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or nativeAppBridgeSetInitParams is not exported");
        return -1;
    }
    try {
        auto params = cordial::InitParams::Create(assets, width, height);
        auto activity = std::make_shared<jnivm::Object>();
        reinterpret_cast<Call>(fn)(env->GetJNIEnv(),
                                   cordial::to_jni(env, activity),
                                   cordial::to_jni(env, params));
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

/// `JNIAAssetManagerSetup.initNative(AssetManager)` — a *static* native, so the
/// second argument is the class rather than an instance.
///
/// This is how the engine gets its asset manager. Without it the engine has no
/// way to read its own content, which is why nothing downstream ever starts:
/// no assets, no app shell, no reason to open a socket or draw a frame.
int cordial_asset_manager_init(void* fn, char* err, size_t err_len) {
    using Call = void (*)(JNIEnv*, jclass, jobject);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or initNative is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass("com/roblox/client/JNIAAssetManagerSetup");
        auto assets = std::make_shared<jnivm::Object>();
        reinterpret_cast<Call>(fn)(env->GetJNIEnv(),
                                   cordial::to_jni(env, cls),
                                   cordial::to_jni(env, assets));
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

/// `LocalStorageManager.initStorageManagerNativeV3(AssetManager, String, String)`
int cordial_storage_init(void* fn, const char* a, const char* b, char* err, size_t err_len) {
    using Call = void (*)(JNIEnv*, jclass, jobject, jstring, jstring);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or the storage native is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass("com/roblox/client/LocalStorageManager");
        auto assets = std::make_shared<jnivm::Object>();
        auto s1 = cordial::S_pub(a);
        auto s2 = cordial::S_pub(b);
        reinterpret_cast<Call>(fn)(env->GetJNIEnv(),
                                   cordial::to_jni(env, cls),
                                   cordial::to_jni(env, assets),
                                   cordial::to_jni(env, s1),
                                   cordial::to_jni(env, s2));
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

/// A native taking only `(JNIEnv*, jobject)` — `nativeRetryInit`.
int cordial_call_bare(void* fn, char* err, size_t err_len) {
    using Call = void (*)(JNIEnv*, jobject);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or the native is not exported");
        return -1;
    }
    try {
        auto obj = std::make_shared<jnivm::Object>();
        reinterpret_cast<Call>(fn)(env->GetJNIEnv(), cordial::to_jni(env, obj));
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

/// `FlagJniInterface.nativeInitializeNativeFlags(String[])`
///
/// This is what `bootstrapTheApp()` exists to reach. On Android the Kotlin
/// bootstrap fetches the flag set and passes it here; the engine then reports
/// back through `NativeHelper.gameActivity_onFlagsLoaded` or, failing that,
/// `gameActivity_onFlagsFailed` — and the second is what Cordial has been
/// getting, because nothing ever called this.
///
/// An empty array means "no overrides": the engine falls back to the defaults
/// compiled into it. That is the honest starting point — inventing flag values
/// would change engine behaviour in ways nothing here could account for.
int cordial_init_flags(void* fn, char* err, size_t err_len) {
    using Call = jobject (*)(JNIEnv*, jclass, jobjectArray);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or nativeInitializeNativeFlags is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass("com/roblox/client/flags/FlagJniInterface");
        auto empty = std::make_shared<jnivm::Array<jnivm::String>>(0);
        reinterpret_cast<Call>(fn)(
            env->GetJNIEnv(),
            (jclass)cordial::to_jni(env, cls),
            (jobjectArray)cordial::to_jni(env, empty));
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

/// `NativeGLInterface.nativeAppBridgeV2InitWithParams(InitParams)`
///
/// The real entry to Roblox's app bridge. The launcher Activity is
/// `ActivitySplash`, whose default target is `ActivityNativeMain` — not the AGDK
/// `MainGameActivity`, which the manifest marks `exported=false`. The chain that
/// actually brings the client up runs through here, not through
/// `MainGameActivity.nativeAppBridgeSetInitParams`.
int cordial_appbridge_init(void* fn, const char* assets, int width, int height, char* err,
                           size_t err_len) {
    using Call = void (*)(JNIEnv*, jobject, jobject);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or the app-bridge native is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass("com/roblox/engine/jni/NativeGLInterface");
        auto params = cordial::InitParams::Create(assets, width, height);
        reinterpret_cast<Call>(fn)(env->GetJNIEnv(),
                                   (jobject)cordial::to_jni(env, cls),
                                   (jobject)cordial::to_jni(env, params));
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

/// A `NativeGLInterface` native taking no arguments — `nativeAppBridgeStartLuaAppDM`.
///
/// "Start Lua App DataModel": the Lua app shell is what Roblox actually renders
/// on this platform, so this is the call that turns a live engine into a drawing
/// one.
int cordial_appbridge_call_bare(void* fn, char* err, size_t err_len) {
    using Call = void (*)(JNIEnv*, jobject);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or the native is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass("com/roblox/engine/jni/NativeGLInterface");
        reinterpret_cast<Call>(fn)(env->GetJNIEnv(), (jobject)cordial::to_jni(env, cls));
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
