# Multi-architecture strategy

**Task B (§16.3). Status: decided.**

## Decision

**Execute natively when the host ABI matches an ABI the APK ships. Do not build a
translation layer.**

Task A established that Roblox ships a complete x86-64 Android build (see
[`findings.md`](findings.md) §1). Cordial therefore never translates machine code. This
is a build-flag and runtime-dispatch concern, not an architectural one.

| Host | APK ships a matching ABI | Strategy | Phase |
|---|---|---|---|
| x86-64 | `lib/x86_64/` — yes | Native execution + CPU *feature* emulation (`libbadcpu`) | 1 |
| ARM64 | `lib/arm64-v8a/` — yes | Native execution of `arm64-v8a`; no feature emulator needed | later |
| ARM64 | absent | Translation required | **not supported** |
| anything else | — | — | not supported |

## Consequences

**x86-64 is the only supported target for Phases 1–2.** Everything else is deferred until
the runtime actually launches Roblox on the primary target.

**ARM64 hosts are cheap in principle and expensive in practice.** The ABI is present in
the APK, so the loader, bionic shim, syscall translation and framework layer are all
architecture-agnostic in design — but each of them contains architecture-specific code
(TLS layout, relocation types, syscall numbers, signal frame layout), and the graphics
path differs. Treat ARM64 as a real port, not a compile flag, and do not attempt it until
x86-64 works. `libbadcpu` is not built on ARM64: it is an x86-64 instruction emulator and
has no meaning there.

**No translation layer will be designed.** If a future host has no matching ABI, the
answer is "unsupported", not "write a JIT". Reopening this requires reopening Task A.

**No Quest/VR target.** The Quest build ships no x86 code, so supporting it would mandate
exactly the translation path this decision exists to avoid. Linux desktop only.

## Implementation notes

- Host ABI is resolved once, at instance launch, and selects the `lib/<abi>/` directory to
  load from. If the APK ships no matching ABI, fail early with a clear message rather than
  falling back to anything.
- Prefer the APK variant that carries the host ABI. If only a universal APK is available,
  load from the matching subdirectory and ignore the others.
- `libbadcpu` is gated on `host_machine.cpu_family() == 'x86_64'` in the build, and its
  `SIGILL` handler is installed only in the Roblox child process.
- x86-64-v2 is the effective CPU floor. Below SSE4.1 the emulator cannot help enough and
  the correct behaviour is a clear hardware-too-old message, not a crash.
