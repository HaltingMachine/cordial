# libbadcpu (vendored)

x86-64 CPU **feature** emulator. Installs a `SIGILL` handler, decodes the faulting
instruction, emulates it in software, and advances `RIP` past it. This lets Roblox's
x86-64-v2 baseline code run on CPUs that predate some of those instructions.

It is **not** an architecture translator and has no meaning outside x86-64 hosts. See
[`docs/multiarch.md`](../../docs/multiarch.md).

## Provenance

Vendored from [`Z3ki/sober-oss`](https://github.com/Z3ki/sober-oss) at commit
`e48a905efdffa1ad49a3ebb873895bcff73aa935`, licensed MIT. Upstream licence text is
preserved verbatim in [`LICENSE.upstream`](LICENSE.upstream).

Upstream describes this as a **clean-room reimplementation** written from documented
behaviour, with no decompiled pseudocode copied into it. Cordial vendors only
`src/libbadcpu/`, `include/badcpu.h` and the test — none of that repository's decompiled
material.

Copyright (c) 2026 Sober OSS Contributors.

Changes on vendoring: directory layout flattened (`include/`, `src/`, `test/`) and
`meson.build` rewritten for that layout. Source files are otherwise unmodified.

## Scope

Eight emulated instructions: `POPCNT`, `MOVBE`, `LZCNT`, `TZCNT`, `ANDN`, `BLSI`,
`BLSMSK`, `BLSR`. Hard floor at SSE4.1 — below that, emulation cannot close the gap and
the correct behaviour is a clear "hardware too old" message.

## Build

From the repository root:

```bash
meson setup build && ninja -C build && meson test -C build
```

Produces `build/third_party/libbadcpu/libbadcpu.so`.

## Known gap — emulation is untested

The test suite has two real assertions (handler installs, handler removes). The decode
section prints its results without asserting them, and **no test checks emulation
correctness** — nothing executes a faulting instruction and compares the emulated result
against hardware.

Upstream's most recent commit is `fix: correct REX/VEX decoding, popcnt /r check, 64-bit
movbe, r8-r15 support`, which is precisely the untested surface. Treat correctness as
unverified until a differential test exists: for each instruction, run it natively on a
capable CPU, run it through the emulator, compare the full register file and flags.
