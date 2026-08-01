# Security policy

## Reporting

Report security issues through GitHub's private vulnerability reporting on this
repository ("Security" → "Report a vulnerability"). Please do not open a public
issue for anything exploitable.

Include what you did, what happened, and what you expected. A reproduction that
someone else can run is worth more than a description, and this project's whole
method is that claims are verified by running them.

## What counts

Cordial runs a large proprietary binary inside a runtime it supplies, and hosts
plugins alongside it. In scope:

- **Sandbox escape from a plugin** — a plugin reaching Cordial's memory, the
  Roblox process, or the host beyond its granted capabilities. Plugins are
  isolated by process ([ADR-003](docs/adr/ADR-003-plugin-isolation.md)); a way
  around that is the most serious class of bug here.
- **Capability broker bypass** — obtaining an effect without the capability that
  should gate it.
- **Anything in the runtime that lets untrusted content reach the host** — path
  traversal out of the asset tree, the `/system` redirect
  (`native/system_paths.cpp`) resolving somewhere it should not, or the flag
  layering reading a file it should not.
- **Memory-safety bugs in Cordial's own `unsafe` code**, of which there is a
  great deal: the linker bindings, the bionic shims, and the libc interposers
  in `native/`.

## What does not

- **Crashes in Roblox's own code.** The engine is not ours and most crashes are
  Cordial handing it something malformed. Those are ordinary bugs — open a
  normal issue.
- **The ban risk from using a third-party client.** That is documented in the
  README, is inherent to what this is, and is not a vulnerability.
- **Requests for an exploit surface.** Cordial deliberately has no script
  execution, no hooking and no asset override
  ([ADR-001](docs/adr/ADR-001-in-process-hooking.md),
  [ADR-004](docs/adr/ADR-004-plugin-asset-overrides.md)). "Cordial cannot cheat"
  is the design, not a bug.

## Expectations

This is a hobby project with no funding and no on-call. There is no bounty and
no response-time guarantee. It is also two days old and largely written by an AI
with a human directing architecture — read the disclosure in the README — so
treat its security posture as unproven rather than assumed.
