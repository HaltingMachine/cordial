# Cordial

A Linux runtime for Roblox with a first-class plugin ecosystem.

Roblox ships no Linux client, and Hyperion blocks the Windows client under Wine. Cordial
runs the official x86-64 Android build on Linux through a purpose-built runtime, fixes the
Android-on-desktop gaps at the framework layer rather than by patching the client, and
exposes a sandboxed, capability-scoped plugin surface on top.

**Status: bootstrap analysis complete. The runtime does not exist yet.**

Nothing here launches Roblox. The only code in the tree is a vendored CPU-feature
emulator — roughly 1% of Phase 1. See [`docs/findings.md`](docs/findings.md) for what has
actually been established.

## What is here

| Path | |
|---|---|
| [`docs/findings.md`](docs/findings.md) | Bootstrap analysis: the architecture verdict, what is unknown, what is blocked |
| [`docs/multiarch.md`](docs/multiarch.md) | Multi-architecture decision |
| [`docs/framework-api-inventory.md`](docs/framework-api-inventory.md) | The Phase 2 backlog, enumerated from the shipping APK |
| [`docs/analysis/`](docs/analysis) | Raw enumeration output: linked libraries, undefined symbols, JNI natives, framework classes |
| [`docs/base-evaluation.md`](docs/base-evaluation.md) | Phase 0: port-vs-write assessment of the minecraft-linux stack |
| [`docs/adr/ADR-001-in-process-hooking.md`](docs/adr/ADR-001-in-process-hooking.md) | Why Cordial has no in-process hooking, ever |
| [`third_party/libbadcpu/`](third_party/libbadcpu) | Vendored x86-64 CPU feature emulator (MIT) |

## Headline findings

**Roblox ships a complete x86-64 Android build.** Verified against Roblox 2.732.1043:
`split_config.x86_64.apk` carries `lib/x86_64/libroblox.so`, 116 MB of x86-64 machine code
built by NDK r28c. Cordial executes it natively and needs **no CPU architecture
translation** — only CPU *feature* emulation. That is the difference between a tractable
systems project and one an order of magnitude larger.

**The runtime surface is bounded:** 13 Android libraries linked, 644 undefined symbols,
GLES2 + EGL mandatory with Vulkan `dlopen`ed as an optional upgrade.

**Roblox's game surface is AGDK `GameActivity`** — which is Apache-2.0 open source, so the
activity, surface, input and IME contract Phase 2 must satisfy can be read rather than
inferred.

**Phase 2 is bigger than the spec implies:** the communities window is a WebView-hosted
Activity, so fixing it needs an embedded browser as well as window management — and the
captcha flow puts that on the login path.

Details in [`docs/findings.md`](docs/findings.md) and
[`docs/framework-api-inventory.md`](docs/framework-api-inventory.md).

## Not in scope, permanently

No in-process code execution against the Roblox process: no hooking, no memory patching,
no injected script environment. Not "disabled by default" — absent from the API
vocabulary. The protection is that no injection primitive exists in the binary to extract.
Reasoning in [ADR-001](docs/adr/ADR-001-in-process-hooking.md).

Also out: client-side integrity flags or watermarks (no root of trust on a machine the
user owns), and obfuscation-as-security.

## Build

```bash
meson setup build && ninja -C build && meson test -C build
```

x86-64 Linux, C++20. Builds `libbadcpu.so` and runs its test. That is all it builds today.

## Build order

0. **Evaluate prior art** — done. Port, don't write: the AOSP bionic linker, a bionic libc
   shim and a JNI VM all exist under usable licences and all build here.
   [`docs/base-evaluation.md`](docs/base-evaluation.md).
1. **Runtime** — loader, bionic shim, syscall translation, GLES2 + EGL, OpenSL ES → PipeWire,
   input. Roblox launches and renders. Nothing else matters until this works.
2. **Framework layer** — JNI stubs against the `GameActivity` contract. Desktop
   identification, login path, FastFlags, passkeys, WebView. This is where Cordial
   differentiates.
3. **Core** — bootstrap shell, instance manager, auth, event bus, capability broker,
   plugin host.
4. **Plugin API** — five events, three capabilities, one real plugin.
5. **Ecosystem** — registry, services, first-party library tier.

Phases 1–2 are the majority of the work. The architecture is designed so partial
completion still ships something useful: a runtime with a good framework layer and no
plugin system is already the best Roblox client on Linux.

## Estimation note

Sober is a ~7.1 MB runtime built by a small team since 2022, still described by its
authors as experimental and liable to be discontinued. Bloxstrap and Fishstrap are Windows
launchers wrapping an already-working client — they contain no runtime. This is a solo
project. Report progress honestly: if a component is stubbed, it is stubbed; if the
runtime does not launch Roblox, it is not working.

## Related

Cordial is independent and unaffiliated with Roblox Corporation or VinegarHQ.
Naming lineage: Wine → Vinegar → Sober → Cordial.
