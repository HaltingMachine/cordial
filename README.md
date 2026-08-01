<p align="center">
  <img src="https://raw.githubusercontent.com/luohoa97/cordial/main/packaging/icons/org.cordial.Cordial.svg" alt="Cordial" width="220">
</p>

# Run Roblox on Linux natively — Plugins, all yours.

Cordial loads Roblox's official Android x86-64 engine directly on Linux through a
purpose-built runtime: the AOSP bionic linker, a bionic/glibc shim, a JNI VM in
place of Android's, and a framework layer that answers the client's calls. No
emulator, no container, no virtual machine. It talks to your GPU through Vulkan
or GLES2 the way any native application does.

### Disclosure

**This is NOT an official Roblox client. This project is in no way endorsed or
sponsored by Roblox Corporation.** Roblox is a trademark of Roblox Corporation.

**It was built in two days by [Claude Code](https://claude.com/claude-code)** —
Anthropic's Claude, model Opus 5 — with the architecture directed by a human
working alongside it. That is not a footnote. It is why the commit messages are
long, why `docs/` records what was disproved as carefully as what worked, and why
nobody should assume a human reviewed every line. The engineering is real and
every finding was verified by running the thing rather than reasoning about it.
It has still only existed for two days.

> ### ⚠️ Read this before using an account you care about
>
> Roblox does not support third-party clients and operates automated systems that
> ban accounts for using them, up to permanent termination. Those systems have
> produced false positives against innocent players.
>
> Cordial does not modify the Roblox client — it runs the official Android build
> unmodified — but it necessarily presents a synthesised Android environment, and
> a heuristic detector does not owe you that distinction. Alternate accounts are
> not a shield; Roblox's Terms reserve the right to terminate those too.
>
> **If your account matters to you, do not use it here.** If you use Cordial and
> get banned, that is on you, and the maintainers cannot get it reversed.

## Status: early. It runs, it draws, it is not yet usable.

| | |
|---|---|
| Loads `libroblox.so` natively | ✅ |
| App shell reaches `APP_READY (Landing)` | ✅ |
| Renders — Vulkan, with GLES2 fallback | ✅ |
| Networking / HTTPS | ✅ |
| Mouse and keyboard reach the engine | ✅ |
| Stable | ❌ crashes on roughly 1 launch in 3 |
| Playable frame rate | ❌ about 1 fps |
| Signed in | ❌ not implemented |
| Plugins | ❌ designed, not built |

The two blockers are a `SIGSEGV` on an `HttpClient` thread and a render loop that
ticks once a second. Both are characterised in [`docs/NEXT.md`](docs/NEXT.md),
along with the explanations that were tested and ruled out.

**Do not install this expecting to play Roblox.** Install it if you want to work
on it.

## Install

Flatpak, built from source. There is no hosted remote yet.

```bash
git clone --recursive https://github.com/luohoa97/cordial
cd cordial
packaging/build-flatpak.sh --install
```

Cordial ships **no Roblox code, APK or assets** and never will. You supply the
official Android client from your own installation — Cordial needs the
`lib/x86_64/` objects and the base APK.

For development, skip Flatpak and run the loader directly. **Clang is required**
— AOSP bionic uses C11 `_Atomic` inside C++ headers, which GCC rejects — and
x86-64 Linux only:

```bash
cargo build --release

CORDIAL_MONITOR=1 CORDIAL_FULLSCREEN=1 \
cargo run --release --bin cordial-load -- \
  --lib-dir /path/to/lib/x86_64 --apk /path/to/base.apk \
  --host-libc --game-activity --run 30
```

`CORDIAL_MONITOR=<n>` puts the window on the nth monitor, `CORDIAL_FULLSCREEN=1`
covers it, `CORDIAL_WINDOW_POS=<x>,<y>` overrides with explicit coordinates, and
`CORDIAL_ANDROID_TRACE=1` logs the Android API calls — which is how to tell
whether input is reaching the engine. `cordial-load --help` lists the rest.

The engine also writes its own log to `<files>/appData/logs/`. It is the single
best diagnostic in the project; read it before forming any theory.

## Documentation

| | |
|---|---|
| [`docs/NEXT.md`](docs/NEXT.md) | Where to start, what is blocking, and what has already been ruled out |
| [`docs/findings.md`](docs/findings.md) | Bootstrap analysis: the architecture verdict and what is unknown |
| [`docs/framework-api-inventory.md`](docs/framework-api-inventory.md) | The framework backlog, enumerated from the shipping APK |
| [`docs/traces/`](docs/traces) | A capture of the same APK on real Android — the ground truth this project checks itself against |
| [`docs/adr/ADR-001-in-process-hooking.md`](docs/adr/ADR-001-in-process-hooking.md) | Why Cordial has no in-process hooking, ever |
| [`docs/design/path-to-a-frame.md`](docs/design/path-to-a-frame.md) | GameActivity, assets, surface |
| [`docs/design/instances-and-launch.md`](docs/design/instances-and-launch.md) | Multi-instance, multi-account, `roblox://` handling |
| [`docs/base-evaluation.md`](docs/base-evaluation.md) | Port-vs-write assessment of the prior art |
| [`docs/multiarch.md`](docs/multiarch.md) | Multi-architecture decision |

## Headline findings

**Roblox ships a complete x86-64 Android build.** `split_config.x86_64.apk`
carries `lib/x86_64/libroblox.so` — 116 MB of x86-64 machine code built by NDK
r28c. Cordial executes it natively and needs **no CPU architecture translation**,
only CPU *feature* emulation. That is the difference between a tractable systems
project and one an order of magnitude larger.

**The runtime surface is bounded:** 13 Android libraries linked, 644 undefined
symbols, GLES2 + EGL mandatory with Vulkan `dlopen`ed as an optional upgrade.

**Roblox's game surface is AGDK `GameActivity`**, which is Apache-2.0 open
source — so the activity, surface, input and IME contract can be read rather than
inferred.

## Not in scope, permanently

No in-process code execution against the Roblox process: no hooking, no memory
patching, no injected script environment. Not "disabled by default" — absent from
the API vocabulary, so there is no injection primitive in the binary to extract.
Reasoning in [ADR-001](docs/adr/ADR-001-in-process-hooking.md).

Also out: client-side integrity flags or watermarks, and obfuscation-as-security.

## Licence

GPL-3.0-or-later. See [`LICENSE`](LICENSE).

Vendored and submoduled dependencies are MIT and keep their own notices:
[`third_party/libbadcpu`](third_party/libbadcpu) (from Sober OSS),
`mcpelauncher-linker` and `libjnivm` (ChristopherHX / MCMrARM).
