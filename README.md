# Cordial

**A Linux client for Roblox — plugins and all, all yours.**

> **Warning — read this before you use Cordial with an account you care about.**
>
> Roblox does not support third-party clients and operates automated systems that
> ban accounts for using them, up to permanent termination. Those systems have
> produced false positives against innocent players. Sober has survived this for
> years; Cordial is a different project with no track record whatsoever.
>
> Cordial does not modify the Roblox client — it runs the official Android build
> unmodified — but it necessarily presents a synthesised Android environment, and
> a heuristic detector does not owe you that distinction. Alternate accounts are
> not a shield: Roblox's Terms reserve the right to terminate those too.
>
> **If your account matters to you, do not use it here.** If you use Cordial and
> get banned, that is on you, and the maintainers cannot get it reversed.

A Linux runtime for Roblox with a first-class plugin ecosystem.

Roblox ships no Linux client, and Hyperion blocks the Windows client under Wine. Cordial
runs the official x86-64 Android build on Linux through a purpose-built runtime, fixes the
Android-on-desktop gaps at the framework layer rather than by patching the client, and
exposes a sandboxed, capability-scoped plugin surface on top.

**Status: Phase 1 done, Phase 2 underway. It renders — slowly.**

`cordial-load` maps Roblox's 105.6 MB `libroblox.so` with the AOSP bionic linker,
resolves every relocation, runs all of its static constructors, completes
`JNI_OnLoad`, brings the app bridge up, hands the engine a surface and **draws
frames**. Both renderers work: Vulkan through a `VK_KHR_android_surface` →
`VK_KHR_xlib_surface` shim, and GLES2/EGL as the fallback when the host has no
Vulkan. The client narrates itself through Cordial's `liblog` and writes its own
FastLog to `<files>/appData/logs/`.

The app shell comes up: the engine reaches `APP_READY (Landing)`, talks to
Roblox over HTTPS, writes its flag cache to disk, and reports no flag failures.

What is still wrong:

- **It crashes on roughly a third of launches** — always the same signature, an `HttpClient` thread indexing a table off a null base. Newly reached rather than newly introduced.
- **It runs at about 1 fps.** Not compute-bound — 13% CPU over thirty seconds,
  with every engine thread parked in a futex and waking once a second. The app
  shell registers a *rendering frequency* and renders on demand; that is still
  the best theory and it is unproven. Window focus and frame-callback starvation
  have both been tested and ruled out.
- **Not signed in.** Without a session the landing page has nothing to show, and
  avatar thumbnails fail against user id 0.

Input is still unimplemented. See [`docs/NEXT.md`](docs/NEXT.md).

## What is here

| Path | |
|---|---|
| [`docs/findings.md`](docs/findings.md) | Bootstrap analysis: the architecture verdict, what is unknown, what is blocked |
| [`docs/multiarch.md`](docs/multiarch.md) | Multi-architecture decision |
| [`docs/framework-api-inventory.md`](docs/framework-api-inventory.md) | The Phase 2 backlog, enumerated from the shipping APK |
| [`docs/analysis/`](docs/analysis) | Raw enumeration output: linked libraries, undefined symbols, JNI natives, framework classes |
| [`docs/base-evaluation.md`](docs/base-evaluation.md) | Phase 0: port-vs-write assessment of the minecraft-linux stack |
| [`docs/adr/ADR-001-in-process-hooking.md`](docs/adr/ADR-001-in-process-hooking.md) | Why Cordial has no in-process hooking, ever |
| [`docs/design/path-to-a-frame.md`](docs/design/path-to-a-frame.md) | The remaining Phase 2 core: GameActivity, assets, surface |
| [`docs/design/instances-and-launch.md`](docs/design/instances-and-launch.md) | Multi-instance, multi-account, and `roblox://` handling |
| [`crates/cordial-linker-sys/`](crates/cordial-linker-sys) | Rust bindings to the AOSP bionic linker |
| [`crates/cordial-runtime/`](crates/cordial-runtime) | Symbol table, bionic shims, `cordial-load` |
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

## How this was built

Cordial was written almost entirely by **Claude (Anthropic)** — model Opus 5 —
working from a human's direction, review and hardware. That is not a disclaimer
bolted on afterwards; it is why the repository looks the way it does. The commit
messages are long because each one records what was measured and what was
disproved, and the `docs/` tree exists because an agent that forgets everything
between sessions has to write down how it knows what it knows.

Read it with that in mind. The engineering is real and the findings were all
verified by running the thing rather than by reasoning about it — several
sections of `docs/NEXT.md` are explicitly lists of confident conclusions that
turned out to be wrong. But nobody should adopt this on the assumption that a
careful human reviewed every line.

## Licence

GPL-3.0-or-later. See [`LICENSE`](LICENSE).

The vendored and submoduled dependencies are MIT and keep their own notices:
[`third_party/libbadcpu`](third_party/libbadcpu) (from Sober OSS),
`mcpelauncher-linker` and `libjnivm` (ChristopherHX / MCMrARM). MIT is
compatible with GPL-3.0 in this direction.

**Roblox itself is not included and never will be.** Cordial ships no Roblox
code, no APK and no assets. It loads the official Android client that you supply
from your own installation. Roblox is a trademark of Roblox Corporation, which
has nothing to do with this project and does not endorse it.

## Build

Rust, with a C++ subtree for the bionic linker. **Clang is required** — AOSP bionic uses
C11 `_Atomic` inside C++ headers, which GCC rejects (see
[`docs/base-evaluation.md`](docs/base-evaluation.md) §2.1). x86-64 Linux only.

```bash
git submodule update --init --recursive
cargo build --release
```

To load Roblox's engine, point it at the `lib/x86_64/` objects from an installed APK:

```bash
cargo run --release --bin cordial-load -- --lib-dir /path/to/lib/x86_64 --host-libc
```

## Flatpak

Flatpak is the primary distribution target (spec §11).

```bash
packaging/build-flatpak.sh --install
```

The manifest deliberately has no `--filesystem=host` and no
`--talk-name=org.freedesktop.Flatpak` — the latter is arbitrary host command
execution and would hand every plugin the sandbox escape the capability model
exists to prevent, below where any broker could see it
([ADR-002](docs/adr/ADR-002-core-shell-and-ui-handoff.md) §2).

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
