<p align="center">
  <img src="https://raw.githubusercontent.com/luohoa97/cordial/main/packaging/icons/org.cordial.Cordial.svg" alt="Cordial" width="460">
</p>

# Run Roblox on Linux natively — Plugins, all yours.

Cordial loads Roblox's official Android x86-64 engine directly on Linux through a
purpose-built runtime: the AOSP bionic linker, a bionic/glibc shim, a JNI VM in
place of Android's, and a framework layer that answers the client's calls. No
emulator, no container, no virtual machine. It talks to your GPU through Vulkan
or GLES2 the way any native application does.

## Get started

- [Read the documentation 📖](docs)
- [Start here — what works and what is blocking 🧭](docs/NEXT.md)
- [Install it 🔽](#install)
- [How it actually works 🔬](docs/findings.md)
- [Why there is no script execution, ever 🔒](docs/adr/ADR-001-in-process-hooking.md)
- [Report a bug 🐛](https://github.com/luohoa97/cordial/issues)

**New here?** Read the warning below first, then
[`docs/NEXT.md`](docs/NEXT.md) — it is written for someone picking the project
up cold and says plainly what is broken and what has already been ruled out.

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

## Status: early. It runs and draws; you cannot sign in yet.

| | |
|---|---|
| Loads `libroblox.so` natively | ✅ |
| App shell reaches `APP_READY (Landing)` | ✅ |
| Renders — Vulkan, with GLES2 fallback | ✅ |
| Networking / HTTPS | ✅ |
| Mouse and keyboard reach the engine | ✅ |
| Stable | ✅ 26 consecutive clean launches |
| Frame rate | ✅ ~27 fps steady on Vulkan (GLES fallback is much slower) |
| Signed in | ❌ not implemented |
| Plugins | ❌ designed, not built |

Measured with `vkQueuePresentKHR`: 656, 656 and 655 presents over 24 s across
three runs, unchanged by injected input — so it renders continuously rather than
on demand. The GLES fallback path is far slower (about 1 fps) and is a separate
open problem for hosts without Vulkan; see [`docs/NEXT.md`](docs/NEXT.md).

The blocker now is sign-in. Without a session the client sits on the logged-out
landing page, so there is nothing much to do with it.

**Do not install this expecting to play Roblox.** Install it if you want to work
on it.

## Install

> Cordial is **not** ready to play Roblox on. Install it if you want to work on
> it, or to watch it come up. You cannot sign in yet.

### 1. What you need

- x86-64 Linux
- **Clang** — AOSP bionic uses C11 `_Atomic` inside C++ headers and GCC rejects it
- An X11 session (Wayland works through XWayland)
- Roblox's official Android client, which **you supply** — Cordial ships no
  Roblox code, APK or assets and never will

From an installed APK you need the `lib/x86_64/` objects and the base APK.

### 2. Build it

Flatpak is the intended way. There is no hosted remote yet, so it builds from
source:

```bash
git clone --recursive https://github.com/luohoa97/cordial
cd cordial
packaging/build-flatpak.sh --install
```

For development, skip Flatpak and build the loader directly:

```bash
git clone --recursive https://github.com/luohoa97/cordial
cd cordial
cargo build --release
```

### 3. Run it

```bash
cargo run --release --bin cordial-load -- \
  --lib-dir /path/to/lib/x86_64 --apk /path/to/base.apk \
  --host-libc --game-activity --run 30
```

A window opens, the engine comes up, and it renders Roblox's logged-out landing
page at about 27 fps. `--run` is how many seconds to stay up.

### 4. Useful knobs

| | |
|---|---|
| `CORDIAL_MONITOR=<n>` | open on the nth monitor instead of the primary one |
| `CORDIAL_FULLSCREEN=1` | cover that monitor |
| `CORDIAL_WINDOW_POS=<x>,<y>` | explicit position, overrides the above |
| `CORDIAL_RESOLUTION=<w>x<h>` | render resolution, default 1280x720 |
| `CORDIAL_DPI_SCALE=<f>` | UI density Roblox lays out against; 1.0 is a low-density phone |
| `CORDIAL_ANDROID_TRACE=1` | log Android API calls |
| `CORDIAL_COUNT_GL=1` | report graphics calls on exit |

```bash
CORDIAL_MONITOR=1 CORDIAL_FULLSCREEN=1 cargo run --release --bin cordial-load -- \
  --lib-dir /path/to/lib/x86_64 --apk /path/to/base.apk \
  --host-libc --game-activity --run 30
```

`cordial-load --help` lists the rest.

**If the interface looks coarse**, it is being laid out for a low-density phone.
Raise both — the render resolution is 720p by default and `dpiScale` is 1.0,
which is what Roblox treats as a cheap handset:

```bash
CORDIAL_MONITOR=1 CORDIAL_RESOLUTION=1920x1200 CORDIAL_DPI_SCALE=1.75 \
cargo run --release --bin cordial-load -- \
  --lib-dir /path/to/lib/x86_64 --apk /path/to/base.apk \
  --host-libc --game-activity --run 30
```

Roblox's graphics-quality FastFlags (`DebugFRMQualityLevelOverride` and the MSAA
overrides) were tested and change nothing here, because they govern 3D scene
rendering and the logged-out landing page is a 2D interface. Resolution and
density are the levers that apply to it.

### 5. When something goes wrong

**Read the engine's own log first.** Roblox writes it to
`<files>/appData/logs/*.log` and it names subsystems, stages, paths and
exceptions in its own words. It is the best diagnostic in the project and most
questions are answered by the newest file in that directory.

To check whether input is reaching the engine, run with
`CORDIAL_ANDROID_TRACE=1` and look for `onTouchEventNative(...) -> true`.

## Documentation

Start with [`docs/NEXT.md`](docs/NEXT.md). The rest is reference.

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

Third-party components keep their own licences and notices, reproduced in
[`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md) and installed alongside the
binary by the Flatpak:

- [`third_party/libbadcpu`](third_party/libbadcpu) — MIT, vendored from
  [Sober OSS](https://github.com/Z3ki/sober-oss)
- `mcpelauncher-linker` — MIT, ChristopherHX and MCMrARM
- AOSP bionic, carried within it — Apache-2.0 and BSD
- `libjnivm` — MIT, ChristopherHX

MIT and Apache-2.0 are satisfied while the combined work is offered under the
GPL, provided those notices travel with it. That is a condition, not a
courtesy.
