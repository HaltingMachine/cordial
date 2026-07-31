# Phase 0 — prior-art evaluation

**Question (§14 Phase 0):** should Cordial write an Android linker, bionic shim and JNI VM
from scratch, or port and adapt the minecraft-linux stack?

**Answer: port. Do not write.** Three of the four hardest Phase 1–2 components already
exist under usable licences, and all three **compile clean on this host today**.

**Date:** 2026-07-31. **Host:** Fedora (Linux 7.1.4), GCC 16.1.1, Clang (Homebrew),
CMake 4.4.0, x86-64.

---

## 1. Verdict per component

| Cordial component (§3/§4) | Base | Licence | Builds here | Recommendation |
|---|---|---|---|---|
| ELF loader | `mcpelauncher-linker` + `android_bionic` + `android_core` | MIT + Apache-2.0/BSD | **yes** (clang) | **Port.** Do not write. |
| bionic shim | `libc-shim` | **none — blocker, §5** | **yes** (clang) | **Port, once licensed.** |
| JNI boundary | `libjnivm` | MIT | **yes** (clang) | **Port.** |
| Windowing + input | `game-window` | MIT | partial — needs `linux-gamepad` | **Port**, likely with edits. |
| APK handling | `mcpelauncher-extract`, `axml-parser` | MIT | not tried | Port; small either way. |
| Play Store download | `Google-Play-API` | Apache-2.0 | not tried | Evaluate later. |
| Syscall translation (binder, ashmem) | — | — | — | **Write.** Nothing to port. |
| Graphics (GLES2 + EGL → Mesa) | — | — | — | **Write.** See §6. |
| Audio (OpenSL ES → PipeWire) | — | — | — | **Write.** |
| Framework layer (`android.*` classes) | `libjnivm` gives the *mechanism*, not the classes | — | — | **Write**, on libjnivm. |
| Launcher glue | `mcpelauncher-client` | **GPL-3.0 (umbrella)** | not tried | **Read, do not link.** §5. |

## 2. What was actually built

Not a paper assessment — these were compiled from source on this machine:

| Artefact | Size | Source |
|---|---|---|
| `liblinker.a` | 825 KB | AOSP bionic linker + libbase + liblog + libziparchive |
| `liblibc-shim.a` | 292 KB | `libc-shim` (+ `logger` headers) |
| `libjnivm.a` | 2.27 MB | `libjnivm` |
| `libfake-jni.a` | 308 KB | `libjnivm`'s fake-jni compatibility layer |

Zero errors in each. Full commands in §8.

### 2.1 Consequence: Cordial's native build must use Clang, not GCC

The bionic linker **fails to build with GCC 16** — 144 errors, all from bionic's use of
C11 `_Atomic` inside C++ headers (`bionic_elf_tls.h`, `bionic_fdsan.h`). That is a Clang
extension; GCC rejects it. AOSP is a Clang-only codebase and this is not going to change.

Under Clang the same tree builds with **zero errors**. So: Cordial's C/C++ components
require Clang. This is a real constraint on the build system, CI, and the Flatpak manifest,
and it is better known now than discovered in month three.

(Cordial's existing Meson tree builds `libbadcpu` with GCC. That stays fine — `libbadcpu`
is independent. The constraint applies to anything that includes bionic headers.)

## 3. `mcpelauncher-linker` — the biggest single win

**This is not a reimplementation of an Android linker. It is *the* Android linker.**

The repository's own code is 278 lines. Everything else is a CMake wrapper that compiles
real AOSP sources — `bionic/linker/linker.cpp`, `linker_relocate.cpp`, `linker_phdr.cpp`,
`linker_soinfo.cpp`, `linker_namespaces.cpp`, `linker_cfi.cpp`, `linker_tls*` — for the
host, behind a small `compat.h`. Real relocation handling, real TLS layout, real namespace
semantics, real `dlopen` behaviour, all of it maintained by Google and merely retargeted.

The §3 "ELF loader" row — "Load Android `.so` objects, resolve symbols, TLS layout" — is
done, and done better than a solo project would do it.

### 3.1 The API is exactly the shape Cordial needs

```c++
namespace linker {
    void  init();
    void *load_library(const char *name,
                       const std::unordered_map<std::string, void*> &symbols);
    void  relocate(void *handle, const std::unordered_map<std::string, void*> &symbols);
    void *dlopen(const char *filename, int flags);
    void *dlsym(void *handle, const char *symbol);
    int   dl_iterate_phdr(...);
    size_t get_library_base(void *handle);
    void  get_library_code_region(void *handle, size_t &base, size_t &size);
}
```

That `symbols` map **is** the runtime layer's interface. Cordial builds a
`{name → function pointer}` table covering the 644 undefined symbols from
[`framework-api-inventory.md`](framework-api-inventory.md), hands it to `load_library`, and
the linker resolves Roblox's imports against Cordial's implementations. The inventory stops
being a document and becomes a literal table to fill in.

It also links `libziparchive` and ships `zip_archive_stream_entry.cc`, so it can map a `.so`
**straight out of the APK** without extracting it first. Cordial gets that for free.

## 4. `libc-shim` covers most of the bionic gap — measured, not guessed

Cross-referencing `libc-shim`'s 572 shimmed symbols against the 490 libc-class undefined
symbols in Roblox's native objects:

| Resolution | Count | Share |
|---|---:|---:|
| Provided directly by `libc-shim` | 354 | 72% |
| Resolvable from host `libm` (verified against `libm.so.6`) | 59 | 12% |
| Resolvable from host `libz` | 8 | 2% |
| Provided by the linker's `__loader_dl*` | 6 | 1% |
| Misclassified — actually `libmediandk` data symbols (`AMEDIAFORMAT_KEY_*`) | 10 | — |
| **Residual, genuinely unimplemented** | **53** | **11%** |

The residual, in full:

```
basename dirname environ execve execvpe getauxval getentropy getgrnam getopt_long
getpwnam_r ldiv memrchr mkdtemp mkstemp nftw optarg optind posix_fallocate pread64
pthread_exit pthread_sigmask ptrace recvmmsg sendmmsg sched_getcpu sched_getparam
sched_getscheduler sched_setscheduler setgid setsid setuid sigaltstack socketpair
sysinfo tcgetattr tcsetattr timerfd_create timerfd_settime
__cxa_thread_atexit_impl __fread_chk __fwrite_chk __poll_chk __readlink_chk __umask_chk
__gnu_strerror_r __gxx_personality_v0 __libc_init __gcov_dump __gcov_flush
ZSTD_trace_compress_begin ZSTD_trace_compress_end
ZSTD_trace_decompress_begin ZSTD_trace_decompress_end
```

Most of that is mechanical: glibc has direct equivalents for the POSIX names, the
`__*_chk` FORTIFY variants are one-line forwarders, `ZSTD_trace_*` and `__gcov_*` are
no-op weak hooks. Perhaps ten need actual thought — `__libc_init` (bionic's entry
protocol), the `environ`/`optarg`/`optind` data symbols, and `__gxx_personality_v0` (C++
unwinding across the bionic/host boundary, which is the one with teeth).

`libc-shim` also already implements **`__system_property_get`**, `__system_property_find`
and `__system_property_read_callback` — which is both the §3 bionic-shim row *and* the
mechanism behind §4.2's "Roblox thinks you're mobile".

> **Caveat that matters more than the percentage.** This is *symbol-name* coverage.
> A name existing in glibc does not make it ABI-compatible with bionic: `struct stat`,
> `pthread_mutex_t`, `DIR`, `FILE` and `sigset_t` all differ in layout. That is precisely
> why `libc-shim` has `stat.cpp`, `dirent.cpp`, `cstdio.cpp` and `pthreads.cpp` doing
> struct translation rather than forwarding. **89% of names resolved is not 89% of the
> work done** — but it does mean the shape of the problem is known and largely solved by
> someone else, which is what Phase 0 was asking.

## 5. Licensing — one blocker and one trap

| Component | Licence | Verdict |
|---|---|---|
| `mcpelauncher-linker` | MIT (© 2024 ChristopherHX and MCMrARM) | usable |
| `android_bionic` | AOSP — Apache-2.0 / BSD | usable, attribution required |
| `android_core` | AOSP — Apache-2.0 | usable, attribution required |
| `libjnivm` | MIT (© 2019 ChristopherHX) | usable |
| `game-window` | MIT (© 2018 MrARM) | usable |
| `mcpelauncher-extract` | MIT | usable |
| `logger`, `base64`, `properties-parser`, `arg-parser` | Unlicense | usable |
| `Google-Play-API` | Apache-2.0 | usable |
| **`libc-shim`** | **no LICENSE file at all** | **blocker — §5.1** |
| `fake-jni` | ambiguous; upstream stale since 2020-11 | avoid — superseded by `libjnivm` |
| `mcpelauncher-client`, `mcpelauncher-manifest`, `mcpelauncher-ui-qt` | **GPL-3.0** | **read only — §5.2** |

### 5.1 `libc-shim` has no licence

The repository contains no `LICENSE`, `COPYING`, or SPDX header. The only copyright
notices inside it belong to vendored third-party files (OpenBSD `strlcpy`, Berkeley
`setjmp`). Under default copyright the project's own ~6,600 lines are **all rights
reserved** and cannot be copied into Cordial.

**Action, and it is cheap:** open an issue asking the authors to add an explicit licence.
Every neighbouring repository by the same authors is MIT, so this reads as an oversight
rather than a decision. Until it is resolved:

- Do not vendor `libc-shim` source.
- It remains valuable as a **reference** — reading it to learn *which* bionic/glibc
  struct differences matter is fair and is most of its value anyway.
- If the licence never materialises, reimplement the residual against bionic's own
  headers (Apache-2.0/BSD) using `libc-shim` as a checklist of what to cover. That is
  meaningfully more work but it is bounded, and §4's measurement is what bounds it.

### 5.2 The GPL trap

`mcpelauncher-manifest` — the umbrella that assembles the whole launcher — is **GPL-3.0**,
as are `mcpelauncher-client` and the Qt UI. The *libraries* (linker, jnivm, game-window)
are MIT; the *glue* is GPL.

Cordial currently declares MIT. Linking GPL-3.0 code would relicense Cordial. So:

- **Link:** the MIT/Apache libraries.
- **Read, do not copy:** `mcpelauncher-client`. It is the best available worked example of
  how these pieces fit together, and reading it to understand the architecture is fine —
  copying it is not.
- Decide Cordial's own licence deliberately before Phase 1 lands. If GPL-3.0 is
  acceptable, the constraint disappears and `mcpelauncher-client` becomes directly
  reusable. That is a real option worth considering rather than defaulting to MIT out of
  habit.

## 6. What this does *not* solve

Phase 0's job is to stop Cordial rewriting solved problems, not to pretend the project is
nearly done.

**Graphics is entirely Cordial's problem.** Nothing in the stack maps GLES2/EGL onto Mesa
for this workload. minecraft-linux carries a patched ANGLE fork for Minecraft's
RenderDragon; Roblox's renderer is different and the 91 `gl*`/`egl*` symbols will need
their own work. Historically this is where Android-on-desktop projects lose the most time,
and Phase 0 has not reduced that risk at all.

**The framework layer is still Cordial's problem.** `libjnivm` provides the *mechanism* —
a JNI VM that lets C++ classes masquerade as Java ones. It does not provide
`android.app.Activity`, `android.view.SurfaceHolder`, `android.webkit.WebView`, or
`android.credentials.CredentialManager`. Those are Cordial's to write, against
`GameActivity`'s Apache-2.0 source. The inventory's 630 platform classes remain the
backlog.

**Minecraft is not Roblox.** This stack was shaped by one app's needs. Roblox uses AGDK
`GameActivity` (Minecraft historically used `NativeActivity`), targets SDK 35, and pulls
in `CredentialManager`, camera2, and a WebView. Expect the framework layer to diverge
substantially even where the runtime layer does not.

**Audio, syscall translation (binder/ashmem/ion), and multi-instance namespacing** have no
prior art here and are Cordial's to write.

### 6.1 One unexpected gift: `libjnivm` can generate the backlog for you

`libjnivm` exposes `VM::GenerateClassDump(const char *path)` and a `JNIVM_ENABLE_TRACE`
build option. It records which Java classes and methods the native code *actually* calls
at runtime and emits C++ stubs for them.

That converts [`framework-api-inventory.md`](framework-api-inventory.md) from a static
enumeration — which, as `findings.md` §6 admits, tells you *which* APIs are referenced but
not *what behaviour each needs* — into a runtime-driven work queue: run Roblox, see what it
actually calls, implement that, repeat. It is the single most useful tool in the stack for
Phase 2 and it should be turned on from the first launch attempt.

## 7. Recommended Phase 1 order

1. **Resolve the `libc-shim` licence question** (§5.1). It gates the approach, an issue
   costs five minutes, and the answer changes the plan.
2. **Switch Cordial's native build to Clang** and add the three libraries as subprojects.
   `libbadcpu` stays on Meson/GCC; the bionic-derived tree needs Clang and CMake, so
   expect a two-build-system repository or a Meson wrapper around CMake.
3. **Get `linker::load_library` to load `libroblox.so` out of the APK** with a symbol map
   that is entirely stubs — every one of the 644 symbols pointing at a function that logs
   its own name and aborts. This does not run Roblox, but it proves the loader, the
   relocations and the TLS layout work against the real 116 MB object, and it turns the
   inventory into a prioritised crash log. **This is the first milestone worth aiming at.**
4. Fill in `libc-shim` (or its replacement) + the 53 residual symbols until the stubs stop
   being hit before `JNI_OnLoad`.
5. Stand up `libjnivm` with tracing on and start answering "what does Roblox actually
   call".
6. Graphics. Budget generously.

## 8. Reproduction

```bash
mkdir minecraft-linux-reference && cd minecraft-linux-reference
git clone --depth 1 https://github.com/minecraft-linux/mcpelauncher-linker
git clone --depth 1 https://github.com/minecraft-linux/libc-shim
git clone --depth 1 https://github.com/minecraft-linux/logger
git clone --depth 1 https://github.com/ChristopherHX/libjnivm
git clone --depth 1 https://github.com/minecraft-linux/game-window

# The linker pulls AOSP bionic (63 MB) and core (191 MB) as submodules
cd mcpelauncher-linker && git submodule update --init --depth 1 --recursive

# Clang is required — GCC fails on bionic's C11 _Atomic in C++ headers
cmake -S . -B build -DCMAKE_C_COMPILER=clang -DCMAKE_CXX_COMPILER=clang++ \
      -DCMAKE_BUILD_TYPE=Release
cmake --build build -j$(nproc)          # -> build/liblinker.a

cd ../libc-shim
cmake -S . -B build -DCMAKE_C_COMPILER=clang -DCMAKE_CXX_COMPILER=clang++ \
      -DCMAKE_CXX_FLAGS="-I../logger/include" -DCMAKE_C_FLAGS="-I../logger/include"
cmake --build build -j$(nproc)          # -> build/liblibc-shim.a

cd ../libjnivm
cmake -S . -B build -DCMAKE_C_COMPILER=clang -DCMAKE_CXX_COMPILER=clang++
cmake --build build -j$(nproc)          # -> build/libjnivm.a
```

`game-window` additionally needs `minecraft-linux/linux-gamepad`; it fails on
`gamepad/gamepad_ids.h` without it. Not investigated further — it is a Phase 1 windowing
concern, not a Phase 0 blocker.

The symbol-coverage measurement in §4 cross-references
[`analysis/undefined-symbols.tsv`](analysis/undefined-symbols.tsv) against symbol names
extracted from `libc-shim/src/*.cpp` with `grep -oE '\{"[A-Za-z0-9_]+",'`.
