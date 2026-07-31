// extern "C" surface over mcpelauncher-linker's C++ API.
//
// The linker's own interface takes std::unordered_map<std::string, void*>, which
// Rust cannot construct. Everything here is a translation of that, and nothing
// else — no policy, no state. Policy lives in Rust.

#include <mcpelauncher/linker.h>

#include <cstddef>
#include <string>
#include <unordered_map>

extern "C" {

void cordial_linker_init() {
    linker::init();
}

// names/addrs are parallel arrays of length n. Returns a library handle, or null.
void* cordial_linker_load_library(const char* name, const char* const* names,
                                  void* const* addrs, size_t n) {
    std::unordered_map<std::string, void*> symbols;
    symbols.reserve(n);
    for (size_t i = 0; i < n; i++) {
        symbols.emplace(names[i], addrs[i]);
    }
    return linker::load_library(name, symbols);
}

void cordial_linker_update_ld_library_path(const char* path) {
    linker::update_LD_LIBRARY_PATH(path);
}

void* cordial_linker_dlopen(const char* filename, int flags) {
    return linker::dlopen(filename, flags);
}

void* cordial_linker_dlsym(void* handle, const char* symbol) {
    return linker::dlsym(handle, symbol);
}

const char* cordial_linker_dlerror() {
    return linker::dlerror();
}

size_t cordial_linker_get_library_base(void* handle) {
    return linker::get_library_base(handle);
}

void cordial_linker_get_library_code_region(void* handle, size_t* base, size_t* size) {
    size_t b = 0, s = 0;
    linker::get_library_code_region(handle, b, s);
    *base = b;
    *size = s;
}

} // extern "C"
