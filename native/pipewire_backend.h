// The host side of the OpenSL ES translation: a PCM playback stream backed by
// PipeWire, reached without linking against it.
//
// This header carries no PipeWire types, deliberately. `opensles.cpp` builds
// the OpenSL object model against this instead of `<pipewire/pipewire.h>`
// directly, so the object model compiles the same way whether or not this
// tree was configured with pipewire-devel present — see the top of
// `pipewire_backend.cpp` for how that split is enforced.

#include <cstddef>
#include <cstdint>

namespace cordial::audio {

/// True once a PipeWire session has been confirmed reachable: the library
/// loaded, `pw_init` ran, and a core connection completed a round trip. The
/// first call does the work and every later call returns the cached result,
/// so this is cheap to call from `slCreateEngine` on every attempt.
///
/// False covers three different failures on purpose — headers absent at
/// build time, `libpipewire-0.3.so.0` absent at run time, and a reachable
/// library with no session behind it (`PIPEWIRE_RUNTIME_DIR` unset, daemon
/// not running) — because the caller's response is the same for all three:
/// report failure rather than hand back an engine with no audio behind it.
bool pipewire_available();

/// One playback stream, backed by one `pw_stream`. `opensles.cpp` owns one of
/// these per realized `AudioPlayer` object.
///
/// The enqueue/drain contract mirrors `SLAndroidSimpleBufferQueueItf`
/// directly rather than adding a layer of translation: `enqueue` takes
/// ownership of nothing and copies nothing — the caller's buffer must stay
/// valid and unmodified until the drain callback fires for it, exactly as
/// the Android buffer queue documents. That symmetry is what makes the
/// threading tractable: PipeWire's realtime side drains our internal queue
/// under a single mutex and calls back out to the caller with the mutex
/// already released, so a callback that re-enters `enqueue` (the ordinary
/// OpenSL pattern — queue the next buffer as soon as the last one drains)
/// does not deadlock against itself.
class PlaybackStream {
public:
    PlaybackStream();
    ~PlaybackStream();

    PlaybackStream(const PlaybackStream&) = delete;
    PlaybackStream& operator=(const PlaybackStream&) = delete;

    /// Creates and connects the underlying `pw_stream`, negotiating exactly
    /// the format described (no resampling surprises: what Roblox put in
    /// `SLDataFormat_PCM` is what reaches PipeWire's converter). `rate` is in
    /// Hz — `opensles.cpp` divides the spec's milliHertz value by 1000 before
    /// calling this, which is where that conversion belongs, not here.
    /// `container_bits` and `big_endian` pick the exact `spa_audio_format`;
    /// combinations this backend does not know how to describe return false
    /// rather than guessing at a nearby one.
    bool open(uint32_t rate_hz, uint32_t channels, uint32_t bits_per_sample,
              uint32_t container_bits, bool big_endian, uint32_t max_pending_buffers);

    void close();

    /// Queues `size` bytes of interleaved PCM starting at `data`. Never
    /// blocks: it either appends to the pending list under a short-held lock
    /// or, if the list is already at the player's declared buffer count,
    /// returns false immediately for the caller to report as
    /// `SL_RESULT_BUFFER_INSUFFICIENT`.
    bool enqueue(const void* data, uint32_t size, void* buffer_context);

    /// Discards every pending buffer without invoking the drain callback for
    /// any of them — matching `SLAndroidSimpleBufferQueueItf::Clear`, which
    /// is a discard, not a fast-forward.
    void clear();

    /// Called from PipeWire's own thread the moment a previously enqueued
    /// buffer has been fully copied out (i.e. "consumed", in the buffer
    /// queue's vocabulary). Registering a new callback replaces the old one;
    /// there is exactly one, matching
    /// `SLAndroidSimpleBufferQueueItf::RegisterCallback`.
    using DrainCallback = void (*)(void* buffer_context, void* user);
    void set_drain_callback(DrainCallback cb, void* user);

    void set_active(bool active);
    void set_volume_linear(float linear);
    void set_mute(bool mute);

    struct QueueState {
        uint32_t count; ///< buffers currently pending (mirrors `SLAndroidSimpleBufferQueueState.count`)
        uint32_t index; ///< buffers enqueued since the last `open`/`clear` (mirrors `.index`)
    };
    QueueState state() const;

private:
    struct Impl;
    Impl* impl_;
};

} // namespace cordial::audio
