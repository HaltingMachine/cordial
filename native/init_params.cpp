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
class Surface;

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

/// Screen size the Activity reports. AConfiguration, PlatformParams and
/// DisplayMetrics all have to agree about this.
static int g_width = 1280;
static int g_height = 720;

void set_display_size(int width, int height) {
    g_width = width;
    g_height = height;
}

class AndroidActivity;
std::shared_ptr<Object> make_display_metrics(ENV* env);

/// `android.util.DisplayMetrics`
///
/// The engine asks the Activity for these and reads `density` off the result.
/// Android's density is the scale factor against 160 dpi — a desktop display at
/// roughly 96 dpi is therefore *below* 1.0, not above it, and reporting a
/// phone's 2.5-3.0 here would make the client lay itself out for a screen held
/// at arm's length.
class DisplayMetrics : public Object {
public:
    jfloat density = 1.0f;
    jfloat scaledDensity = 1.0f;
    jfloat xdpi = 96.0f;
    jfloat ydpi = 96.0f;
    jint densityDpi = 160;
    jint widthPixels = 1280;
    jint heightPixels = 720;

    static std::shared_ptr<DisplayMetrics> Create(ENV* env, int width, int height) {
        auto p = std::make_shared<DisplayMetrics>();
        p->widthPixels = width;
        p->heightPixels = height;
        // 1.0 means "one density-independent pixel is one real pixel", which is
        // what a desktop window wants: no scaling, no phone-sized controls.
        p->density = 1.0f;
        p->scaledDensity = 1.0f;
        p->densityDpi = 160;
        to_jni(env, p);
        return p;
    }

    static void Register(ENV* env) {
        env->GetClass<DisplayMetrics>("android/util/DisplayMetrics");
        auto c = env->GetClass("android/util/DisplayMetrics");
#define F(name) c->HookInstance(env, #name, &DisplayMetrics::name)
        F(density); F(scaledDensity); F(xdpi); F(ydpi); F(densityDpi);
        F(widthPixels); F(heightPixels);
#undef F
    }
};

/// `android.view.Surface`
///
/// Typed rather than a bare Object because StartAppParams.surface is declared
/// `Landroid/view/Surface;` and libjnivm matches accessors on the descriptor it
/// derives from the C++ return type.
class AppSurface : public Object {
public:
    static std::shared_ptr<AppSurface> Create(ENV* env) {
        auto p = std::make_shared<AppSurface>();
        to_jni(env, p);
        return p;
    }
    static void Register(ENV* env) {
        env->GetClass<AppSurface>("android/view/Surface");
    }
};

/// `com.roblox.client.startup.NativeHelper`
///
/// The engine's own status channel back into the app. `onFlagsFailed` is the one
/// Cordial has been getting, and it arrives with no explanation — so these are
/// implemented mainly to make the engine's verdict visible rather than to do
/// anything with it.
class NativeHelper : public Object {
public:
    static void onFlagsFailed(ENV*, Object*) {
        fprintf(stderr, "[roblox] flags FAILED — the engine could not load its flag set\n");
    }
    static void onFlagsLoaded(ENV*, Object*, std::shared_ptr<Object>) {
        fprintf(stderr, "[roblox] flags loaded\n");
    }
    static void onAppReady(ENV*, Object*, std::shared_ptr<String> s) {
        fprintf(stderr, "[roblox] app ready: %s\n", s ? s->c_str() : "");
    }

    static void Register(ENV* env) {
        env->GetClass<NativeHelper>("com/roblox/client/startup/NativeHelper");
        auto c = env->GetClass("com/roblox/client/startup/NativeHelper");
        c->HookInstanceFunction(env, "gameActivity_onFlagsFailed", &NativeHelper::onFlagsFailed);
        c->HookInstanceFunction(env, "gameActivity_onFlagsLoaded", &NativeHelper::onFlagsLoaded);
        c->HookInstanceFunction(env, "gameActivity_onAppReady", &NativeHelper::onAppReady);
    }
};

/// `android.content.res.Resources`
///
/// Android's path to the screen is `activity.getResources().getDisplayMetrics()`,
/// not `activity.getDisplayMetrics()`. Hooking the Activity alone left
/// getResources returning null and the engine calling getDisplayMetrics on it.
class Resources : public Object {
public:
    std::shared_ptr<DisplayMetrics> getDisplayMetrics(ENV* env) {
        return DisplayMetrics::Create(env, g_width, g_height);
    }

    static std::shared_ptr<Resources> Create(ENV* env) {
        auto p = std::make_shared<Resources>();
        to_jni(env, p);
        return p;
    }

    static void Register(ENV* env) {
        env->GetClass<Resources>("android/content/res/Resources");
        auto c = env->GetClass("android/content/res/Resources");
        c->HookInstanceFunction(env, "getDisplayMetrics", &Resources::getDisplayMetrics);
    }
};

std::shared_ptr<Object> make_resources(ENV* env) { return Resources::Create(env); }

/// `android.app.Activity`
///
/// Both parameter objects carry one, typed `Landroid/app/Activity;`, and it is
/// the only Activity the engine gets from the app-bridge path. Left null it
/// asks the null for its display metrics and stops.
class AndroidActivity : public Object {
public:
    std::shared_ptr<DisplayMetrics> getDisplayMetrics(ENV* env) {
        return DisplayMetrics::Create(env, g_width, g_height);
    }
    std::shared_ptr<Resources> getResources(ENV* env) { return Resources::Create(env); }

    static std::shared_ptr<AndroidActivity> Create(ENV* env) {
        auto p = std::make_shared<AndroidActivity>();
        to_jni(env, p);
        return p;
    }

    static void Register(ENV* env) {
        env->GetClass<AndroidActivity>("android/app/Activity");
        auto c = env->GetClass("android/app/Activity");
        c->HookInstanceFunction(env, "getDisplayMetrics", &AndroidActivity::getDisplayMetrics);
        c->HookInstanceFunction(env, "getResources", &AndroidActivity::getResources);
    }
};

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

    static std::shared_ptr<DeviceParams> Create(ENV* env, int width, int height) {
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
        // Prime the class. Without this the object reaches Roblox with a null
        // clazz, GetObjectClass falls back to FindClass("Invalid"), and every
        // field read against it returns nothing.
        to_jni(env, p);
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

    static std::shared_ptr<PlatformParams> Create(ENV* env, const char* assets, int width, int height) {
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
        to_jni(env, p);
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
    std::shared_ptr<AndroidActivity> vrContext;
    jboolean isPotato = false;
    jboolean isTablet = false;
    jboolean isVrDevice = false;


    // AutoValue generates accessor methods, so the engine calls
    // `initParams.platformParams()` rather than reading a field. These are the
    // methods; the field hooks above stay for anything that does read directly.
    std::shared_ptr<DeviceParams> get_deviceParams(ENV*) { return deviceParams; }
    std::shared_ptr<PlatformParams> get_platformParams(ENV*) { return platformParams; }
    std::shared_ptr<String> get_baseURL(ENV*) { return baseURL; }
    std::shared_ptr<String> get_buildVariant(ENV*) { return buildVariant; }
    std::shared_ptr<String> get_userAgent(ENV*) { return userAgent; }
    jboolean get_isPotato(ENV*) { return isPotato; }
    jboolean get_isTablet(ENV*) { return isTablet; }
    jboolean get_isVrDevice(ENV*) { return isVrDevice; }
    std::shared_ptr<AndroidActivity> get_vrContext(ENV*) { return vrContext; }

    static std::shared_ptr<InitParams> Create(ENV* env, const char* assets, int width, int height) {
        auto p = std::make_shared<InitParams>();
        p->baseURL = S("https://www.roblox.com");
        p->buildVariant = S("release");
        // The engine sends this on every request. It is Roblox's own client
        // string, not Cordial's: the service routes and gates on it, and a
        // fabricated one would be both untrue and likely rejected.
        p->userAgent = S("Roblox/Android");
        p->deviceParams = DeviceParams::Create(env, width, height);
        p->platformParams = PlatformParams::Create(env, assets, width, height);
        // "Potato" is Roblox's own name for a device below the quality floor.
        p->isPotato = false;
        // Tablet rather than phone: a desktop window is a large screen, and this
        // agrees with the XLARGE reported through AConfiguration.
        p->isTablet = true;
        p->isVrDevice = false;
        p->vrContext = AndroidActivity::Create(env);
        to_jni(env, p);
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
#define G(name) c->HookInstanceFunction(env, #name, &InitParams::get_##name)
        G(baseURL); G(buildVariant); G(userAgent); G(deviceParams); G(platformParams);
        G(isPotato); G(isTablet); G(isVrDevice); G(vrContext);
#undef G
    }
};

/// `com.roblox.engine.jni.autovalue.StartAppParams`
///
/// What actually delivers the surface. `nativeAppBridgeV2StartAppWithParams`
/// takes one of these and its `surface` field is the window the engine renders
/// into — a completely separate path from AGDK's `onSurfaceCreatedNative`
/// lifecycle, which disassembly shows structurally cannot produce a frame here.
class StartAppParams : public Object {
public:
    std::shared_ptr<String> appStarterPlace, appStarterScript, selectedTheme, username;
    std::shared_ptr<PlatformParams> platformParams;
    std::shared_ptr<AppSurface> surface;
    std::shared_ptr<AndroidActivity> vrContext;
    jlong appUserId = 0;
    jboolean isUnder13 = false;
    jint membershipType = 0;


    std::shared_ptr<PlatformParams> get_platformParams(ENV*) { return platformParams; }
    std::shared_ptr<AppSurface> get_surface(ENV*) { return surface; }
    std::shared_ptr<AndroidActivity> get_vrContext(ENV*) { return vrContext; }
    std::shared_ptr<String> get_appStarterPlace(ENV*) { return appStarterPlace; }
    std::shared_ptr<String> get_appStarterScript(ENV*) { return appStarterScript; }
    std::shared_ptr<String> get_selectedTheme(ENV*) { return selectedTheme; }
    std::shared_ptr<String> get_username(ENV*) { return username; }
    jlong get_appUserId(ENV*) { return appUserId; }
    jboolean get_isUnder13(ENV*) { return isUnder13; }
    jint get_membershipType(ENV*) { return membershipType; }

    static std::shared_ptr<StartAppParams> Create(ENV* env, const char* assets, int width,
                                                  int height,
                                                  std::shared_ptr<AppSurface> surface) {
        auto p = std::make_shared<StartAppParams>();
        // Empty starter place and script mean "the default app shell" rather than
        // a specific experience. Naming one here would launch straight into a
        // game, which is not what a cold start does.
        p->appStarterPlace = S("");
        p->appStarterScript = S("");
        p->selectedTheme = S("Dark");
        p->username = S("");
        p->platformParams = PlatformParams::Create(env, assets, width, height);
        p->surface = std::move(surface);
        p->appUserId = 0;
        p->isUnder13 = false;
        p->membershipType = 0;
        p->vrContext = AndroidActivity::Create(env);
        to_jni(env, p);
        return p;
    }

    static void Register(ENV* env) {
        env->GetClass<StartAppParams>("com/roblox/engine/jni/autovalue/StartAppParams");
        auto c = env->GetClass("com/roblox/engine/jni/autovalue/StartAppParams");
#define F(name) c->HookInstance(env, #name, &StartAppParams::name)
        F(appStarterPlace); F(appStarterScript); F(selectedTheme); F(username);
        F(platformParams); F(surface); F(vrContext); F(appUserId); F(isUnder13);
        F(membershipType);
#undef F
#define G(name) c->HookInstanceFunction(env, #name, &StartAppParams::get_##name)
        G(appStarterPlace); G(appStarterScript); G(selectedTheme); G(username);
        G(platformParams); G(surface); G(vrContext); G(appUserId); G(isUnder13);
        G(membershipType);
#undef G
    }
};




std::shared_ptr<Object> make_display_metrics(ENV* env) {
    return DisplayMetrics::Create(env, g_width, g_height);
}

/// Hook `getResources` onto the Activity classes registered elsewhere.
///
/// Typed here rather than in game_activity.cpp because libjnivm binds by the JNI
/// descriptor it derives from the C++ signature: a `shared_ptr<Object>` return
/// becomes `Ljava/lang/Object;`, which never matches the
/// `()Landroid/content/res/Resources;` Roblox asks for. The hook registers
/// happily and is simply never called.
static std::shared_ptr<Resources> activity_get_resources(ENV* env, Object*) {
    return Resources::Create(env);
}

/// The engine reaches its own status channel through the Activity:
/// `activity.getNativeHelper().gameActivity_onFlagsFailed()`. A null helper here
/// means the failure report itself crashes, which is how the verdict stayed
/// invisible.
static std::shared_ptr<NativeHelper> activity_get_native_helper(ENV* env, Object*) {
    auto p = std::make_shared<NativeHelper>();
    to_jni(env, p);
    return p;
}

static void hook_activity_resources(ENV* env, const char* klass) {
    auto c = env->GetClass(klass);
    if (c) {
        c->HookInstanceFunction(env, "getResources", &activity_get_resources);
        c->HookInstanceFunction(env, "getNativeHelper", &activity_get_native_helper);
    }
}

void register_init_params_classes(ENV* env) {
    NativeHelper::Register(env);
    DisplayMetrics::Register(env);
    AppSurface::Register(env);
    Resources::Register(env);
    AndroidActivity::Register(env);
    // These classes are registered by register_game_activity_classes, which runs
    // first; only the descriptor-correct hook belongs here.
    for (const char* k : {"com/google/androidgamesdk/GameActivity",
                          "com/roblox/client/startup/MainGameActivity",
                          "android/app/Activity"}) {
        hook_activity_resources(env, k);
    }
    DeviceParams::Register(env);
    PlatformParams::Register(env);
    InitParams::Register(env);
    StartAppParams::Register(env);
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
        auto params = cordial::InitParams::Create(env, assets, width, height);
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
int cordial_init_flags(void* fn, const char* settings_json, char* err, size_t err_len) {
    using Call = jobject (*)(JNIEnv*, jclass, jobjectArray);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or nativeInitializeNativeFlags is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass("com/roblox/client/flags/FlagJniInterface");

        // Roblox's ClientSettings document, as the client itself fetches it:
        // {"applicationSettings": {"FFlagX": "True", ...}}. An empty array made
        // the engine report flagCount = 0 and then fail — it wants the real set.
        const bool have = settings_json && *settings_json;
        auto arr = std::make_shared<jnivm::Array<jnivm::String>>(have ? 1 : 0);
        if (have) {
            // The object-array specialisation exposes Set rather than a raw
            // element pointer; getArray() there is void*.
            arr->Set(0, cordial::S_pub(settings_json));
        }
        reinterpret_cast<Call>(fn)(
            env->GetJNIEnv(),
            (jclass)cordial::to_jni(env, cls),
            (jobjectArray)cordial::to_jni(env, arr));
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
        auto params = cordial::InitParams::Create(env, assets, width, height);
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

extern "C" {

/// `NativeGLInterface.nativeAppBridgeV2StartAppWithParams(StartAppParams)`
///
/// The call that hands the engine its window. Everything before it is setup.
int cordial_appbridge_start_app(void* fn, const char* assets, int width, int height, char* err,
                                size_t err_len) {
    using Call = void (*)(JNIEnv*, jobject, jobject);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or StartAppWithParams is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass("com/roblox/engine/jni/NativeGLInterface");
        // Reuses the android/view/Surface type registered in game_activity.cpp —
        // registering a second C++ class for the same Java name makes libjnivm throw.
        auto surface = cordial::AppSurface::Create(env);
        auto params = cordial::StartAppParams::Create(env, assets, width, height, surface);
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

} // extern "C"
