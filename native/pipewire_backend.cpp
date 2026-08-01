// PipeWire, reached without ever appearing in `libcordial_liblog.a`'s
// `DT_NEEDED`.
//
// Two separate "optional" decisions are stacked here and it is worth being
// precise about which is which:
//
// 1. **Compile time.** `CMakeLists.txt` looks for `libpipewire-0.3` and
//    `spa-0.2` with `pkg_check_modules` and defines `CORDIAL_HAVE_PIPEWIRE`
//    only if it finds them. If it does not — a machine with no
//    `pipewire-devel` installed — this whole file compiles down to the
//    fallback at the bottom, and the rest of the tree builds exactly as it
//    did before this file existed. Everything above that `#else` uses the
//    real headers, because writing out `spa_pod`'s binary layout by hand
//    when the header that does it correctly is one `pkg-config` call away
//    is not a trade worth making.
//
// 2. **Link and run time.** Even when the headers were present at compile
//    time, `libpipewire-0.3.so` is never linked. Every function in it that
//    this file calls is `dlsym`'d through `load_library()` below, and a
//    missing library or a missing symbol degrades to "no audio" rather than
//    a failure to start. This is what makes a Cordial binary built on a
//    PipeWire machine still run — audio-less — on one that has neither the
//    daemon nor the library.
//
// A third category of PipeWire call needs neither: `pw_core_add_listener`,
// `pw_core_sync` and friends are `static inline` in `<pipewire/core.h>`,
// reading a method table embedded in the `pw_core` object itself (the same
// `spa_interface` pattern used throughout SPA). Those compile straight into
// this translation unit and never touch `dlsym` at all — only the functions
// that *construct* those objects (`pw_context_connect`, `pw_stream_new_simple`,
// ...) are real, exported, and therefore looked up by name.

#include "pipewire_backend.h"

#ifdef CORDIAL_HAVE_PIPEWIRE

#include <spa/param/audio/format-utils.h>
#include <spa/param/props.h>
#include <spa/utils/result.h>
#include <pipewire/pipewire.h>

#include <dlfcn.h>

#include <atomic>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <mutex>
#include <vector>

namespace cordial::audio {
namespace {

// ----------------------------------------------------------------- library

/// Every libpipewire-0.3 entry point this file calls, resolved by name at
/// run time. Grouped as plain fields rather than looked up per call: a
/// missing symbol is discovered once, at `load_library()`, rather than as a
/// null-pointer call partway through a stream's lifetime.
struct Library {
    void* handle = nullptr;

    void (*init)(int*, char***);
    void (*deinit)();

    pw_thread_loop* (*thread_loop_new)(const char*, const spa_dict*);
    void (*thread_loop_destroy)(pw_thread_loop*);
    int (*thread_loop_start)(pw_thread_loop*);
    void (*thread_loop_stop)(pw_thread_loop*);
    void (*thread_loop_lock)(pw_thread_loop*);
    void (*thread_loop_unlock)(pw_thread_loop*);
    int (*thread_loop_timed_wait)(pw_thread_loop*, int);
    void (*thread_loop_signal)(pw_thread_loop*, bool);
    pw_loop* (*thread_loop_get_loop)(pw_thread_loop*);

    pw_context* (*context_new)(pw_loop*, pw_properties*, size_t);
    void (*context_destroy)(pw_context*);
    pw_core* (*context_connect)(pw_context*, pw_properties*, size_t);
    int (*core_disconnect)(pw_core*);

    pw_properties* (*properties_new)(const char*, ...);

    pw_stream* (*stream_new_simple)(pw_loop*, const char*, pw_properties*,
                                     const pw_stream_events*, void*);
    void (*stream_destroy)(pw_stream*);
    int (*stream_connect)(pw_stream*, spa_direction, uint32_t, pw_stream_flags,
                           const spa_pod**, uint32_t);
    int (*stream_disconnect)(pw_stream*);
    pw_buffer* (*stream_dequeue_buffer)(pw_stream*);
    int (*stream_queue_buffer)(pw_stream*, pw_buffer*);
    int (*stream_set_active)(pw_stream*, bool);
    int (*stream_update_params)(pw_stream*, const spa_pod**, uint32_t);
};

Library g_lib{};

bool load_library() {
    // The SONAME every distribution that ships PipeWire installs; the bare
    // name is a fallback for the rare tree that only has the dev symlink.
    void* handle = dlopen("libpipewire-0.3.so.0", RTLD_NOW | RTLD_GLOBAL);
    if (!handle) handle = dlopen("libpipewire-0.3.so", RTLD_NOW | RTLD_GLOBAL);
    if (!handle) {
        std::fprintf(stderr,
            "E/Cordial-OpenSLES         libpipewire-0.3 not found (%s); Roblox's audio "
            "has nowhere to go. Install PipeWire and its client library to enable it.\n",
            dlerror());
        return false;
    }

    struct Entry { const char* name; void** slot; };
    const Entry entries[] = {
        {"pw_init", reinterpret_cast<void**>(&g_lib.init)},
        {"pw_deinit", reinterpret_cast<void**>(&g_lib.deinit)},
        {"pw_thread_loop_new", reinterpret_cast<void**>(&g_lib.thread_loop_new)},
        {"pw_thread_loop_destroy", reinterpret_cast<void**>(&g_lib.thread_loop_destroy)},
        {"pw_thread_loop_start", reinterpret_cast<void**>(&g_lib.thread_loop_start)},
        {"pw_thread_loop_stop", reinterpret_cast<void**>(&g_lib.thread_loop_stop)},
        {"pw_thread_loop_lock", reinterpret_cast<void**>(&g_lib.thread_loop_lock)},
        {"pw_thread_loop_unlock", reinterpret_cast<void**>(&g_lib.thread_loop_unlock)},
        {"pw_thread_loop_timed_wait", reinterpret_cast<void**>(&g_lib.thread_loop_timed_wait)},
        {"pw_thread_loop_signal", reinterpret_cast<void**>(&g_lib.thread_loop_signal)},
        {"pw_thread_loop_get_loop", reinterpret_cast<void**>(&g_lib.thread_loop_get_loop)},
        {"pw_context_new", reinterpret_cast<void**>(&g_lib.context_new)},
        {"pw_context_destroy", reinterpret_cast<void**>(&g_lib.context_destroy)},
        {"pw_context_connect", reinterpret_cast<void**>(&g_lib.context_connect)},
        {"pw_core_disconnect", reinterpret_cast<void**>(&g_lib.core_disconnect)},
        {"pw_properties_new", reinterpret_cast<void**>(&g_lib.properties_new)},
        {"pw_stream_new_simple", reinterpret_cast<void**>(&g_lib.stream_new_simple)},
        {"pw_stream_destroy", reinterpret_cast<void**>(&g_lib.stream_destroy)},
        {"pw_stream_connect", reinterpret_cast<void**>(&g_lib.stream_connect)},
        {"pw_stream_disconnect", reinterpret_cast<void**>(&g_lib.stream_disconnect)},
        {"pw_stream_dequeue_buffer", reinterpret_cast<void**>(&g_lib.stream_dequeue_buffer)},
        {"pw_stream_queue_buffer", reinterpret_cast<void**>(&g_lib.stream_queue_buffer)},
        {"pw_stream_set_active", reinterpret_cast<void**>(&g_lib.stream_set_active)},
        {"pw_stream_update_params", reinterpret_cast<void**>(&g_lib.stream_update_params)},
    };
    for (const Entry& e : entries) {
        *e.slot = dlsym(handle, e.name);
        if (!*e.slot) {
            std::fprintf(stderr,
                "E/Cordial-OpenSLES         libpipewire-0.3 is missing '%s'; treating the "
                "whole library as unusable rather than calling through a null pointer. "
                "No audio output.\n", e.name);
            dlclose(handle);
            return false;
        }
    }
    g_lib.handle = handle;
    return true;
}

// ------------------------------------------------------------------ session

/// One PipeWire connection for the whole process, matching how Roblox itself
/// only ever creates one OpenSL engine. Never torn down: it lives exactly as
/// long as `cordial_liblog`'s other process-lifetime state (bionic's TLS,
/// the linker's loaded-library table) does.
struct Session {
    pw_thread_loop* loop;
    pw_context* context;
    pw_core* core;
};

/// Connects and round-trips once. A `pw_context_connect` that returns a
/// non-null `pw_core` only proves a socket was opened — a stale
/// `PIPEWIRE_RUNTIME_DIR` pointing at a dead socket directory, or a daemon
/// that accepted the connection but is wedged, both get this far. The
/// `pw_core_sync`/`done` round trip is what actually proves someone is home;
/// see the "Degrade honestly" requirement this exists to satisfy.
Session* connect_session() {
    if (!load_library()) return nullptr;

    g_lib.init(nullptr, nullptr);

    pw_thread_loop* loop = g_lib.thread_loop_new("cordial-pipewire", nullptr);
    if (!loop) {
        std::fprintf(stderr,
            "E/Cordial-OpenSLES         pw_thread_loop_new failed; no audio output.\n");
        return nullptr;
    }
    if (g_lib.thread_loop_start(loop) < 0) {
        std::fprintf(stderr,
            "E/Cordial-OpenSLES         pw_thread_loop_start failed; no audio output.\n");
        g_lib.thread_loop_destroy(loop);
        return nullptr;
    }

    g_lib.thread_loop_lock(loop);

    pw_context* context = g_lib.context_new(g_lib.thread_loop_get_loop(loop), nullptr, 0);
    if (!context) {
        std::fprintf(stderr,
            "E/Cordial-OpenSLES         pw_context_new failed; no audio output.\n");
        g_lib.thread_loop_unlock(loop);
        g_lib.thread_loop_stop(loop);
        g_lib.thread_loop_destroy(loop);
        return nullptr;
    }

    pw_core* core = g_lib.context_connect(context, nullptr, 0);
    if (!core) {
        std::fprintf(stderr,
            "E/Cordial-OpenSLES         no PipeWire session reachable (is a PipeWire "
            "daemon running, and is PIPEWIRE_RUNTIME_DIR/XDG_RUNTIME_DIR set?); "
            "no audio output.\n");
        g_lib.thread_loop_unlock(loop);
        g_lib.context_destroy(context);
        g_lib.thread_loop_stop(loop);
        g_lib.thread_loop_destroy(loop);
        return nullptr;
    }

    struct SyncState {
        pw_thread_loop* loop;
        int pending_seq = -1;
        bool done = false;
        bool failed = false;
    } sync{loop};

    pw_core_events core_events{};
    core_events.version = PW_VERSION_CORE_EVENTS;
    core_events.done = [](void* data, uint32_t id, int seq) {
        auto* s = static_cast<SyncState*>(data);
        if (id == PW_ID_CORE && seq == s->pending_seq) {
            s->done = true;
            g_lib.thread_loop_signal(s->loop, false);
        }
    };
    core_events.error = [](void* data, uint32_t id, int seq, int res, const char* message) {
        auto* s = static_cast<SyncState*>(data);
        std::fprintf(stderr,
            "E/Cordial-OpenSLES         PipeWire core reported an error before the "
            "connection was confirmed (id=%u res=%d %s); no audio output.\n",
            id, res, message ? message : "");
        s->failed = true;
        g_lib.thread_loop_signal(s->loop, false);
    };

    spa_hook core_listener{};
    pw_core_add_listener(core, &core_listener, &core_events, &sync);
    sync.pending_seq = pw_core_sync(core, PW_ID_CORE, 0);

    // Three seconds is generous for a round trip over a local Unix socket;
    // a session that cannot answer that quickly is not one worth Roblox
    // waiting on either, so this fails the same way a missing library does.
    while (!sync.done && !sync.failed) {
        if (g_lib.thread_loop_timed_wait(loop, 3) != 0) break;
    }

    g_lib.thread_loop_unlock(loop);

    if (!sync.done) {
        if (!sync.failed) {
            std::fprintf(stderr,
                "E/Cordial-OpenSLES         PipeWire did not answer within 3s; treating "
                "the session as unreachable. No audio output.\n");
        }
        g_lib.thread_loop_lock(loop);
        g_lib.core_disconnect(core);
        g_lib.context_destroy(context);
        g_lib.thread_loop_unlock(loop);
        g_lib.thread_loop_stop(loop);
        g_lib.thread_loop_destroy(loop);
        return nullptr;
    }

    std::fprintf(stderr,
        "I/Cordial-OpenSLES         PipeWire session confirmed reachable; OpenSL ES "
        "audio players will play through it.\n");
    return new Session{loop, context, core};
}

Session* get_session() {
    static Session* session = connect_session();
    return session;
}

// -------------------------------------------------------------- PCM format

/// `SLDataFormat_PCM`'s `bitsPerSample`/`containerSize` pair maps onto SPA's
/// separate "packed width" formats rather than a single format plus a
/// padding flag, so this is a lookup rather than arithmetic. Combinations
/// this backend has not needed to describe (20- and 28-bit containers exist
/// in the OpenSL ES spec, but neither Android nor Roblox use them) fail
/// rather than being rounded to the nearest thing that compiles.
bool map_format(uint32_t bits_per_sample, uint32_t container_bits, bool big_endian,
                spa_audio_format& out) {
    if (bits_per_sample == 8 && container_bits == 8) {
        out = SPA_AUDIO_FORMAT_S8;
        return true;
    }
    if (bits_per_sample == 16 && container_bits == 16) {
        out = big_endian ? SPA_AUDIO_FORMAT_S16_BE : SPA_AUDIO_FORMAT_S16_LE;
        return true;
    }
    if (bits_per_sample == 24 && container_bits == 24) {
        out = big_endian ? SPA_AUDIO_FORMAT_S24_BE : SPA_AUDIO_FORMAT_S24_LE;
        return true;
    }
    if (bits_per_sample == 24 && container_bits == 32) {
        out = big_endian ? SPA_AUDIO_FORMAT_S24_32_BE : SPA_AUDIO_FORMAT_S24_32_LE;
        return true;
    }
    if (bits_per_sample == 32 && container_bits == 32) {
        out = big_endian ? SPA_AUDIO_FORMAT_S32_BE : SPA_AUDIO_FORMAT_S32_LE;
        return true;
    }
    return false;
}

} // namespace

// ------------------------------------------------------------------- Impl

using PendingBuffer = testing::PendingBuffer;

namespace testing {

uint32_t fill_pcm(std::deque<PendingBuffer>& pending, uint8_t* dst, uint32_t want,
                   std::vector<void*>& drained_contexts) {
    uint32_t filled = 0;
    while (filled < want && !pending.empty()) {
        PendingBuffer& front = pending.front();
        uint32_t avail = front.size - front.offset;
        uint32_t take = avail < (want - filled) ? avail : (want - filled);
        std::memcpy(dst + filled, front.data + front.offset, take);
        front.offset += take;
        filled += take;
        if (front.offset >= front.size) {
            drained_contexts.push_back(front.context);
            pending.pop_front();
        }
    }
    // Roblox has not kept the queue fed if this is nonzero. Silence, not
    // whatever this buffer held last cycle: pw_stream reuses its buffer
    // pool, so leaving the tail alone would replay stale audio on a loop —
    // which is exactly how a silent gap turns into an audible tone —
    // instead of producing the gap an underrun actually is.
    uint32_t padded = want - filled;
    if (padded != 0) {
        std::memset(dst + filled, 0, padded);
    }
    return padded;
}

} // namespace testing

namespace {

/// `CORDIAL_TRACE_AUDIO=1` prints a running tally every second: buffers
/// enqueued, buffers drained, and how many frames were silence-padded for
/// lack of a fed queue. Off by default — this is a diagnostic for chasing a
/// stalled or underrunning stream, not routine output.
bool trace_audio_enabled() {
    static const bool enabled = std::getenv("CORDIAL_TRACE_AUDIO") != nullptr;
    return enabled;
}

} // namespace

struct PlaybackStream::Impl {
    pw_stream* stream = nullptr;
    uint32_t bytes_per_frame = 0;
    uint32_t max_pending = 2;

    mutable std::mutex mutex;
    std::deque<PendingBuffer> pending;
    PlaybackStream::DrainCallback drain_cb = nullptr;
    void* drain_user = nullptr;
    uint32_t enqueued_index = 0;
    uint64_t underrun_frames = 0; // diagnostic counter, not surfaced through the OpenSL API

    // CORDIAL_TRACE_AUDIO=1 bookkeeping only; untouched otherwise.
    uint64_t trace_process_cycles = 0;
    uint64_t trace_drains = 0;
    std::chrono::steady_clock::time_point trace_last_report{};

    static void on_process(void* data) { static_cast<Impl*>(data)->process(); }

    static void on_state_changed(void* data, pw_stream_state old_state, pw_stream_state state,
                                  const char* error) {
        (void)data;
        (void)old_state;
        // Only the failure transition is worth a line: PAUSED/STREAMING churn
        // as the graph reconfigures (e.g. the default sink changing) and is
        // not, by itself, evidence of anything wrong.
        if (state == PW_STREAM_STATE_ERROR) {
            std::fprintf(stderr,
                "E/Cordial-OpenSLES         PipeWire stream entered the error state (%s).\n",
                error ? error : "no reason given");
        }
    }

    void process() {
        pw_buffer* b = g_lib.stream_dequeue_buffer(stream);
        if (!b) return; // no buffer available this cycle; PipeWire will ask again
        spa_buffer* buf = b->buffer;
        auto* dst = static_cast<uint8_t*>(buf->datas[0].data);
        if (!dst) {
            g_lib.stream_queue_buffer(stream, b);
            return;
        }

        uint32_t capacity = buf->datas[0].maxsize;
        uint32_t want = capacity;
        if (b->requested != 0 && bytes_per_frame != 0) {
            uint64_t requested_bytes = b->requested * static_cast<uint64_t>(bytes_per_frame);
            if (requested_bytes < capacity) want = static_cast<uint32_t>(requested_bytes);
        }

        DrainCallback cb;
        void* user;
        // Contexts of buffers this cycle fully drained. The callback for each
        // fires after `mutex` is released — see the header comment on why:
        // the ordinary OpenSL pattern is to re-enqueue the next buffer from
        // inside this callback, which would deadlock against its own lock.
        std::vector<void*> drained;
        uint32_t padded;
        {
            std::lock_guard<std::mutex> lock(mutex);
            cb = drain_cb;
            user = drain_user;
            padded = testing::fill_pcm(pending, dst, want, drained);
        }
        if (padded != 0) {
            underrun_frames += padded / (bytes_per_frame ? bytes_per_frame : 1);
        }

        buf->datas[0].chunk->offset = 0;
        buf->datas[0].chunk->stride = bytes_per_frame;
        buf->datas[0].chunk->size = want;

        g_lib.stream_queue_buffer(stream, b);

        if (cb) {
            for (void* ctx : drained) cb(ctx, user);
        }

        if (trace_audio_enabled()) {
            ++trace_process_cycles;
            trace_drains += drained.size();
            auto now = std::chrono::steady_clock::now();
            if (now - trace_last_report >= std::chrono::seconds(1)) {
                trace_last_report = now;
                std::fprintf(stderr,
                    "D/Cordial-OpenSLES         audio trace: %llu buffers enqueued, %llu process "
                    "cycles, %llu drained, %llu underrun frames padded with silence\n",
                    static_cast<unsigned long long>(enqueued_index),
                    static_cast<unsigned long long>(trace_process_cycles),
                    static_cast<unsigned long long>(trace_drains),
                    static_cast<unsigned long long>(underrun_frames));
            }
        }
    }

    static const pw_stream_events& events() {
        static const pw_stream_events e = [] {
            pw_stream_events ev{};
            ev.version = PW_VERSION_STREAM_EVENTS;
            ev.state_changed = &Impl::on_state_changed;
            ev.process = &Impl::on_process;
            return ev;
        }();
        return e;
    }
};

// --------------------------------------------------------------- interface

bool pipewire_available() { return get_session() != nullptr; }

PlaybackStream::PlaybackStream() : impl_(new Impl()) {}

PlaybackStream::~PlaybackStream() {
    close();
    delete impl_;
}

bool PlaybackStream::open(uint32_t rate_hz, uint32_t channels, uint32_t bits_per_sample,
                           uint32_t container_bits, bool big_endian, uint32_t max_pending_buffers) {
    Session* session = get_session();
    if (!session) return false; // slCreateEngine already refused if this is reachable; defensive only

    spa_audio_format format;
    if (!map_format(bits_per_sample, container_bits, big_endian, format)) {
        std::fprintf(stderr,
            "E/Cordial-OpenSLES         unsupported PCM layout: %u-bit samples in a "
            "%u-bit container; no audio for this player.\n", bits_per_sample, container_bits);
        return false;
    }
    if (channels == 0 || channels > SPA_AUDIO_MAX_CHANNELS) {
        std::fprintf(stderr,
            "E/Cordial-OpenSLES         unsupported channel count %u; no audio for this "
            "player.\n", channels);
        return false;
    }

    impl_->bytes_per_frame = channels * (container_bits / 8);
    impl_->max_pending = max_pending_buffers > 0 ? max_pending_buffers : 2;

    static std::atomic<uint32_t> next_id{0};
    char name[64];
    std::snprintf(name, sizeof name, "cordial-audioplayer-%u", next_id.fetch_add(1));

    g_lib.thread_loop_lock(session->loop);

    pw_properties* props = g_lib.properties_new(
        PW_KEY_MEDIA_TYPE, "Audio",
        PW_KEY_MEDIA_CATEGORY, "Playback",
        PW_KEY_MEDIA_ROLE, "Game",
        PW_KEY_APP_NAME, "Cordial",
        PW_KEY_NODE_NAME, name,
        PW_KEY_NODE_DESCRIPTION, "Cordial (Roblox via OpenSL ES)",
        nullptr);

    impl_->stream = g_lib.stream_new_simple(g_lib.thread_loop_get_loop(session->loop), name, props,
                                             &Impl::events(), impl_);
    if (!impl_->stream) {
        std::fprintf(stderr,
            "E/Cordial-OpenSLES         pw_stream_new_simple failed; no audio for this "
            "player.\n");
        g_lib.thread_loop_unlock(session->loop);
        return false;
    }

    uint8_t pod_buffer[1024];
    spa_pod_builder builder = SPA_POD_BUILDER_INIT(pod_buffer, sizeof pod_buffer);
    spa_audio_info_raw info{};
    info.format = format;
    info.channels = channels;
    info.rate = rate_hz;
    if (channels == 1) {
        info.position[0] = SPA_AUDIO_CHANNEL_MONO;
    } else if (channels == 2) {
        info.position[0] = SPA_AUDIO_CHANNEL_FL;
        info.position[1] = SPA_AUDIO_CHANNEL_FR;
    } else {
        info.flags |= SPA_AUDIO_FLAG_UNPOSITIONED;
    }

    const spa_pod* params[1];
    params[0] = spa_format_audio_raw_build(&builder, SPA_PARAM_EnumFormat, &info);

    // Connected inactive: Realize brings the resource into existence, but
    // playback only starts once SLPlayItf::SetPlayState asks for
    // SL_PLAYSTATE_PLAYING, same as the object model's own state machine.
    int rc = g_lib.stream_connect(
        impl_->stream, SPA_DIRECTION_OUTPUT, PW_ID_ANY,
        static_cast<pw_stream_flags>(PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS |
                                      PW_STREAM_FLAG_INACTIVE),
        params, 1);

    g_lib.thread_loop_unlock(session->loop);

    if (rc < 0) {
        std::fprintf(stderr,
            "E/Cordial-OpenSLES         pw_stream_connect failed (%s); no audio for this "
            "player.\n", spa_strerror(rc));
        g_lib.thread_loop_lock(session->loop);
        g_lib.stream_destroy(impl_->stream);
        g_lib.thread_loop_unlock(session->loop);
        impl_->stream = nullptr;
        return false;
    }
    return true;
}

void PlaybackStream::close() {
    if (!impl_->stream) return;
    Session* session = get_session();
    if (session) {
        g_lib.thread_loop_lock(session->loop);
        g_lib.stream_destroy(impl_->stream);
        g_lib.thread_loop_unlock(session->loop);
    }
    impl_->stream = nullptr;
}

bool PlaybackStream::enqueue(const void* data, uint32_t size, void* buffer_context) {
    if (size == 0) return true;
    std::lock_guard<std::mutex> lock(impl_->mutex);
    if (impl_->pending.size() >= impl_->max_pending) return false;
    impl_->pending.push_back({static_cast<const uint8_t*>(data), size, 0, buffer_context});
    ++impl_->enqueued_index;
    return true;
}

void PlaybackStream::clear() {
    std::lock_guard<std::mutex> lock(impl_->mutex);
    impl_->pending.clear();
}

void PlaybackStream::set_drain_callback(DrainCallback cb, void* user) {
    std::lock_guard<std::mutex> lock(impl_->mutex);
    impl_->drain_cb = cb;
    impl_->drain_user = user;
}

void PlaybackStream::set_active(bool active) {
    if (!impl_->stream) return;
    Session* session = get_session();
    if (!session) return;
    g_lib.thread_loop_lock(session->loop);
    g_lib.stream_set_active(impl_->stream, active);
    g_lib.thread_loop_unlock(session->loop);
}

void PlaybackStream::set_volume_linear(float linear) {
    if (!impl_->stream) return;
    Session* session = get_session();
    if (!session) return;

    uint8_t pod_buffer[128];
    spa_pod_builder builder = SPA_POD_BUILDER_INIT(pod_buffer, sizeof pod_buffer);
    const spa_pod* params[1];
    params[0] = static_cast<const spa_pod*>(spa_pod_builder_add_object(
        &builder, SPA_TYPE_OBJECT_Props, SPA_PARAM_Props,
        SPA_PROP_volume, SPA_POD_Float(linear)));

    g_lib.thread_loop_lock(session->loop);
    g_lib.stream_update_params(impl_->stream, params, 1);
    g_lib.thread_loop_unlock(session->loop);
}

void PlaybackStream::set_mute(bool mute) {
    if (!impl_->stream) return;
    Session* session = get_session();
    if (!session) return;

    uint8_t pod_buffer[128];
    spa_pod_builder builder = SPA_POD_BUILDER_INIT(pod_buffer, sizeof pod_buffer);
    const spa_pod* params[1];
    params[0] = static_cast<const spa_pod*>(spa_pod_builder_add_object(
        &builder, SPA_TYPE_OBJECT_Props, SPA_PARAM_Props,
        SPA_PROP_mute, SPA_POD_Bool(mute)));

    g_lib.thread_loop_lock(session->loop);
    g_lib.stream_update_params(impl_->stream, params, 1);
    g_lib.thread_loop_unlock(session->loop);
}

PlaybackStream::QueueState PlaybackStream::state() const {
    std::lock_guard<std::mutex> lock(impl_->mutex);
    return {static_cast<uint32_t>(impl_->pending.size()), impl_->enqueued_index};
}

} // namespace cordial::audio

#else // !CORDIAL_HAVE_PIPEWIRE

// No PipeWire headers at configure time (see CMakeLists.txt). Every entry
// point degrades to "no audio", the same answer this backend gives for an
// absent library or an absent session at run time — from `opensles.cpp`'s
// side these three cases are indistinguishable and deliberately so.

#include <cstdio>

namespace cordial::audio {

bool pipewire_available() {
    static const bool warned = [] {
        std::fprintf(stderr,
            "E/Cordial-OpenSLES         built without pipewire-devel present at configure "
            "time (see native/CMakeLists.txt); OpenSL ES audio is unavailable. Install "
            "pipewire-devel and reconfigure to enable it.\n");
        return true;
    }();
    (void)warned;
    return false;
}

struct PlaybackStream::Impl {};

PlaybackStream::PlaybackStream() : impl_(nullptr) {}
PlaybackStream::~PlaybackStream() {}

bool PlaybackStream::open(uint32_t, uint32_t, uint32_t, uint32_t, bool, uint32_t) { return false; }
void PlaybackStream::close() {}
bool PlaybackStream::enqueue(const void*, uint32_t, void*) { return false; }
void PlaybackStream::clear() {}
void PlaybackStream::set_drain_callback(DrainCallback, void*) {}
void PlaybackStream::set_active(bool) {}
void PlaybackStream::set_volume_linear(float) {}
void PlaybackStream::set_mute(bool) {}
PlaybackStream::QueueState PlaybackStream::state() const { return {0, 0}; }

} // namespace cordial::audio

#endif // CORDIAL_HAVE_PIPEWIRE
