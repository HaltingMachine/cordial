// extern "C" surface over libjnivm, so Rust can stand up a JavaVM.
//
// As with shim.cpp: translation only, no policy.

#include <jnivm.h>

#include <cstddef>
#include <cstdio>
#include <exception>
#include <execinfo.h>
#include <unistd.h>
#include <stdexcept>
#include <memory>

namespace {
// The VM owns every class, object and environment it hands out, so it outlives
// anything derived from it. One per process.
std::unique_ptr<jnivm::VM> g_vm;

/// Report an uncaught C++ exception with its stack instead of dying mute.
///
/// Roblox spawns worker threads during `JNI_OnLoad` and they call straight back
/// into JNI. libjnivm reports misuse by throwing, and an exception escaping a
/// thread Cordial did not start cannot be caught anywhere — the default is
/// `std::terminate` and a core dump carrying no information about which thread,
/// which call, or why.
[[noreturn]] void report_terminate() {
    fprintf(stderr, "\n*** uncaught C++ exception on thread %ld ***\n", (long)gettid());
    if (auto e = std::current_exception()) {
        try {
            std::rethrow_exception(e);
        } catch (const std::exception& ex) {
            fprintf(stderr, "    what(): %s\n", ex.what());
        } catch (...) {
            fprintf(stderr, "    non-standard exception\n");
        }
    }
    void* frames[32];
    int n = backtrace(frames, 32);
    fprintf(stderr, "    %d frames:\n", n);
    backtrace_symbols_fd(frames, n, 2);
    _exit(70);
}
} // namespace

extern "C" {

/// Create the process's JavaVM. Returns the `JavaVM*` Roblox expects in
/// `JNI_OnLoad`, or null if one already exists.
void* cordial_jni_create_vm() {
    if (g_vm) {
        return nullptr;
    }
    std::set_terminate(report_terminate);
    g_vm = std::make_unique<jnivm::VM>();
    JavaVM* vm = g_vm->GetJavaVM();
    // libjnivm recovers its VM from JavaVM::functions->reserved0. If that is not
    // set, every callback Roblox makes throws before it does anything.
    fprintf(stderr, "[jni] JavaVM=%p functions=%p reserved0=%p (expect %p)\n",
            (void*)vm, (void*)(vm ? vm->functions : nullptr),
            vm && vm->functions ? vm->functions->reserved0 : nullptr,
            (void*)g_vm.get());
    return vm;
}

/// The current thread's `JNIEnv*`.
void* cordial_jni_env() {
    return g_vm ? g_vm->GetJNIEnv() : nullptr;
}

/// Write C++ stubs for every Java class and method the native code has reached
/// for so far. This is the Phase 2 backlog, observed rather than guessed.
int cordial_jni_dump_classes(const char* path) {
#ifdef JNI_DEBUG
    if (!g_vm) {
        return -1;
    }
    g_vm->GenerateClassDump(path);
    return 0;
#else
    (void)path;
    return -2;  // built without JNI_DEBUG
#endif
}

/// Call `JNI_OnLoad` with the process JavaVM, containing any C++ exception.
///
/// libjnivm reports misuse by throwing. Those exceptions originate inside
/// Roblox's call stack and would otherwise cross the Rust FFI boundary, where
/// the only outcome is `std::terminate` and a core dump — which says nothing
/// about what went wrong. Catching here turns that into a message.
///
/// Returns the JNI version on success, or one of the negative codes below.
int cordial_jni_call_onload(void* fn, char* err, size_t err_len) {
    using OnLoad = jint (*)(JavaVM*, void*);
    if (!fn || !g_vm) {
        return -1;
    }
    try {
        return reinterpret_cast<OnLoad>(fn)(g_vm->GetJavaVM(), nullptr);
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -2;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -3;
    }
}

} // extern "C"
