// OpenSL ES, enough of it to link.
//
// `libOpenSLES.so` was listed in `EMPTY_LIBRARIES` on the basis that Roblox
// consulted it only through `dlsym`, if at all. That was true of the build
// Cordial was first developed against. It is not true of current builds, which
// reference eight OpenSL symbols directly:
//
//     slCreateEngine
//     SL_IID_ENGINE  SL_IID_PLAY  SL_IID_RECORD  SL_IID_VOLUME
//     SL_IID_BUFFERQUEUE  SL_IID_ANDROIDSIMPLEBUFFERQUEUE
//     SL_IID_ANDROIDCONFIGURATION
//
// Seven of those are *data*, not functions — an `SLInterfaceID` is a pointer to
// a UUID struct. A missing data symbol fails the DT_NEEDED walk outright, so the
// whole client stopped loading with:
//
//     cannot locate symbol "SL_IID_ENGINE" referenced by "libroblox.so"
//
// This provides them so the library links. It does **not** implement OpenSL ES:
// `slCreateEngine` reports failure, which is the honest answer for a host with
// no OpenSL implementation behind it. Roblox's audio then does not come up
// through this path. That is a real gap and it is deliberately a visible one —
// returning success and handing back a non-functional engine object would move
// the failure somewhere far less legible.
//
// The IIDs are distinct, non-null pointers to zeroed structs. Their contents are
// never inspected here because nothing gets far enough to inspect them: they are
// arguments to an engine creation that fails. Distinctness matters only so that
// pointer comparisons between two different interface IDs do not alias.

#include <cstddef>
#include <cstdint>

namespace {

/// `SLInterfaceID_` from the OpenSL ES specification: a 128-bit UUID. Laid out
/// here so each exported ID is a distinct object of the right size, not so its
/// value carries meaning.
struct InterfaceID {
    uint32_t time_low;
    uint16_t time_mid;
    uint16_t time_hi_and_version;
    uint16_t clock_seq;
    uint8_t node[6];
};

InterfaceID g_engine{};
InterfaceID g_play{};
InterfaceID g_record{};
InterfaceID g_volume{};
InterfaceID g_buffer_queue{};
InterfaceID g_android_simple_buffer_queue{};
InterfaceID g_android_configuration{};

} // namespace

extern "C" {

InterfaceID* SL_IID_ENGINE = &g_engine;
InterfaceID* SL_IID_PLAY = &g_play;
InterfaceID* SL_IID_RECORD = &g_record;
InterfaceID* SL_IID_VOLUME = &g_volume;
InterfaceID* SL_IID_BUFFERQUEUE = &g_buffer_queue;
InterfaceID* SL_IID_ANDROIDSIMPLEBUFFERQUEUE = &g_android_simple_buffer_queue;
InterfaceID* SL_IID_ANDROIDCONFIGURATION = &g_android_configuration;

/// `SL_RESULT_FEATURE_UNSUPPORTED` (7). Not `SL_RESULT_SUCCESS`: there is no
/// engine behind this, and a caller that believes there is will fail later and
/// less clearly.
uint32_t slCreateEngine(void** engine, uint32_t, const void*, uint32_t, const void*,
                        const void*) {
    if (engine) *engine = nullptr;
    return 7;
}

struct CordialSymbol {
    const char* name;
    void* address;
};

static const CordialSymbol kSymbols[] = {
    {"slCreateEngine", reinterpret_cast<void*>(&slCreateEngine)},
    {"SL_IID_ENGINE", &SL_IID_ENGINE},
    {"SL_IID_PLAY", &SL_IID_PLAY},
    {"SL_IID_RECORD", &SL_IID_RECORD},
    {"SL_IID_VOLUME", &SL_IID_VOLUME},
    {"SL_IID_BUFFERQUEUE", &SL_IID_BUFFERQUEUE},
    {"SL_IID_ANDROIDSIMPLEBUFFERQUEUE", &SL_IID_ANDROIDSIMPLEBUFFERQUEUE},
    {"SL_IID_ANDROIDCONFIGURATION", &SL_IID_ANDROIDCONFIGURATION},
};

const CordialSymbol* cordial_opensles_symbols(size_t* count) {
    if (count) *count = sizeof(kSymbols) / sizeof(kSymbols[0]);
    return kSymbols;
}

} // extern "C"
