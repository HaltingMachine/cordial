// Android's `/system` tree, served from a directory Cordial owns.
//
// Roblox asks the platform for `/system/fonts/NotoSansCJK-Regular.ttc`. On
// Android that always exists. On a Linux host there is no `/system` at all, the
// lookup fails, and the engine turns the failure into an *empty* path and throws
// during app startup:
//
//     RBXCRASH: UnhandledException (St13runtime_error Path does not exist: "")
//
// Which is a genuinely hard failure to read from the outside — the exception
// names no path, because by then there isn't one. It was found by tracing the
// path-taking libc calls and noticing that the same thread stats the font, gets
// -1, and immediately stats "" three times.
//
// Serving `/system` is not a workaround for a Roblox bug. It is part of what an
// Android runtime owes the code it hosts, exactly like `AAssetManager` or
// `ALooper`. Cordial owns the symbol table, so the redirect belongs at the libc
// boundary rather than anywhere near the engine.
//
// Written in C++ because `open` is variadic, and forwarding a C variadic to the
// real `open` is the one thing C does better here — the same reason liblog.cpp
// is C++.

#include <cstdarg>
#include <cstddef>
#include <cstdio>
#include <cstring>
#include <cstdlib>

#include <dirent.h>
#include <sys/syscall.h>
#include <fcntl.h>
#include <limits.h>
#include <sys/stat.h>
#include <unistd.h>

namespace {

/// The host directory standing in for `/system`. Set once from Rust before the
/// engine runs; empty means "no redirect", which leaves every call untouched.
char g_root[PATH_MAX];
size_t g_root_len = 0;

/// `CORDIAL_TRACE_PATHS=1`. Every function here is fixed-arity except `open`,
/// which is forwarded properly, so this is safe to leave on — unlike
/// `CORDIAL_TRACE=1`, which wraps variadics with fixed-arity declarations and
/// makes the engine abort.
///
/// These wrappers are the only place the path calls are intercepted. An earlier
/// version had a second set in `trace.rs`; because both landed in the same
/// symbol map the tracing copy silently won and the redirect never ran, which
/// looked exactly like the redirect not working.
bool g_trace = false;

void trace(const char* call, const char* path, const char* result) {
    if (!g_trace) {
        return;
    }
    // The thread id is not decoration: Roblox spreads this work over twenty-odd
    // threads, and a single interleaved log invites reading two unrelated calls
    // as cause and effect.
    std::fprintf(stderr, "[paths] tid=%ld %s(\"%s\") = %s\n",
                 (long)::syscall(SYS_gettid), call, path ? path : "(null)", result);
}

void trace_i(const char* call, const char* path, long r) {
    if (!g_trace) {
        return;
    }
    char b[32];
    std::snprintf(b, sizeof b, "%ld", r);
    trace(call, path, b);
}

/// Rewrite `/system/<rest>` to `<root>/<rest>`.
///
/// Returns null when the path is not under `/system`, which is the overwhelming
/// majority of calls — the cost on that path is one `strncmp`.
const char* remap(const char* path, char* buf, size_t n) {
    if (!path || g_root_len == 0) {
        return nullptr;
    }
    if (std::strncmp(path, "/system/", 8) != 0) {
        return nullptr;
    }
    // `path + 7` keeps the separator, so the result is `<root>/fonts/...`.
    int w = std::snprintf(buf, n, "%s%s", g_root, path + 7);
    if (w < 0 || static_cast<size_t>(w) >= n) {
        return nullptr;
    }
    return buf;
}

#define REMAP(path)                          \
    char _buf[PATH_MAX];                     \
    const char* _p = remap(path, _buf, sizeof _buf); \
    const char* real = _p ? _p : (path)

int s_stat(const char* path, struct stat* out) {
    REMAP(path);
    int r = ::stat(real, out);
    trace_i("stat", real, r);
    return r;
}

int s_lstat(const char* path, struct stat* out) {
    REMAP(path);
    int r = ::lstat(real, out);
    trace_i("lstat", real, r);
    return r;
}

int s_access(const char* path, int mode) {
    REMAP(path);
    int r = ::access(real, mode);
    trace_i("access", real, r);
    return r;
}

DIR* s_opendir(const char* path) {
    REMAP(path);
    DIR* d = ::opendir(real);
    trace(d ? "opendir" : "opendir!", real, d ? "ok" : "null");
    return d;
}

char* s_realpath(const char* path, char* resolved) {
    REMAP(path);
    char* r = ::realpath(real, resolved);
    trace("realpath", real, r ? r : "null");
    return r;
}

ssize_t s_readlink(const char* path, char* buf, size_t n) {
    REMAP(path);
    ssize_t r = ::readlink(real, buf, n);
    trace_i("readlink", real, (long)r);
    return r;
}

FILE* s_fopen(const char* path, const char* mode) {
    REMAP(path);
    FILE* f = ::fopen(real, mode);
    trace("fopen", real, f ? "ok" : "null");
    return f;
}

/// `open` is variadic: the mode argument exists only for `O_CREAT`/`O_TMPFILE`.
/// Reading it unconditionally would walk the register save area for an argument
/// the caller never pushed, which is the mistake that makes `CORDIAL_TRACE=1`
/// abort the engine.
int s_open(const char* path, int flags, ...) {
    unsigned mode = 0;
    if (flags & (O_CREAT | O_TMPFILE)) {
        va_list ap;
        va_start(ap, flags);
        mode = va_arg(ap, unsigned);
        va_end(ap);
    }
    REMAP(path);
    int r = ::open(real, flags, mode);
    trace_i("open", real, r);
    return r;
}

#undef REMAP

} // namespace

extern "C" struct CordialSystemSymbol {
    const char* name;
    void* addr;
};

/// Point the redirect at a host directory. Passing null or "" disables it.
extern "C" void cordial_set_system_root(const char* root) {
    if (!root || !*root) {
        g_root_len = 0;
        g_root[0] = '\0';
        return;
    }
    std::snprintf(g_root, sizeof g_root, "%s", root);
    g_root_len = std::strlen(g_root);
    // A trailing slash would produce `<root>//fonts`, which works but reads
    // badly in a trace, and would break a later exact-prefix comparison.
    while (g_root_len > 1 && g_root[g_root_len - 1] == '/') {
        g_root[--g_root_len] = '\0';
    }
}

/// Turn on the path log. Separate from the root so tracing works even when the
/// redirect is disabled.
extern "C" void cordial_set_path_trace(int on) {
    g_trace = on != 0;
}

extern "C" const CordialSystemSymbol* cordial_system_symbols(size_t* count) {
    static const CordialSystemSymbol table[] = {
        {"stat", (void*)&s_stat},
        {"lstat", (void*)&s_lstat},
        {"access", (void*)&s_access},
        {"opendir", (void*)&s_opendir},
        {"realpath", (void*)&s_realpath},
        {"readlink", (void*)&s_readlink},
        {"fopen", (void*)&s_fopen},
        {"open", (void*)&s_open},
    };
    *count = sizeof(table) / sizeof(table[0]);
    return table;
}
