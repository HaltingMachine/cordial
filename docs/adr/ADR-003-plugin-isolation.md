# ADR-003: Plugins have no memory access to Cordial

**Status:** accepted
**Supersedes:** nothing
**Related:** [ADR-001](ADR-001-in-process-hooking.md), [ADR-002](ADR-002-core-shell-and-ui-handoff.md)

## Decision

Plugins run in their own address space. They cannot read or write Cordial's
memory, and there is no API by which they could ask to. The capability broker is
the entire surface between a plugin and the core.

This is not a default that a manifest key can turn off. There is no
`cap:core.memory.*`, in the same way and for the same reason that there is no
`cap:core.process.spawn` (ADR-002): a capability that hands over the machine is
not a capability, it is the absence of one.

## Why

**Otherwise the capability system is decorative.** Capabilities are worth
declaring only if declaring less means being able to do less. A plugin granted
nothing but `cap:core.window.title`, but able to reach into the core's memory,
can do everything the core can do — rewrite the broker's own allow-lists
included. Every other permission in the system becomes a suggestion, and the
manifest becomes documentation rather than enforcement.

**It would break the recoverability property ADR-002 depends on.** That ADR
splits the core shell from the UI so that a failing UI plugin can be restarted
without losing the session. That only holds if a plugin's failures are confined
to its own address space. With shared memory, any plugin bug is a core bug: a
stray write corrupts the core's heap, and the crash surfaces somewhere else
entirely, arbitrarily later. The debugging cost of that failure mode is paid by
Cordial, and the user experiences it as "Cordial is unstable", not as "that
plugin is broken".

**It is ADR-001's principle applied inward.** ADR-001 rejects in-process code
execution against the Roblox process — no hooking, no memory patching, no
injected script environment. Granting third-party plugins that same power over
Cordial would be inconsistent, and worse: plugin code is *ecosystem* code,
installed casually and in volume, from many authors of unknown intent. It
warrants more isolation than we grant ourselves, not less.

**Process isolation is the only kind that actually holds.** In-process
sandboxing of native plugins — a restricted API surface, a scripting runtime, a
"please don't" in the docs — is a boundary only as strong as the absence of bugs
in it. An address-space boundary is enforced by the MMU and does not depend on
Cordial being correct.

## Consequences

- Plugins are separate processes and communicate over IPC. The broker mediates
  every call; there is no fast path that bypasses it.
- Anything a plugin needs to observe about core state must be an explicit,
  named capability with an explicit payload. "Read this struct" is not available
  as a shortcut, so each such need becomes a deliberate API decision.
- Bulk data (frames, textures, large buffers) needs an explicit shared-memory
  transport if it is ever required — negotiated per use, scoped to a specific
  buffer, and granted by capability. That is a *narrow, named* sharing of one
  region, not access to Cordial's address space, and it does not weaken this
  decision. It is called out here so it is designed deliberately rather than
  arrived at by erosion.
- IPC costs latency and serialisation. This is accepted. The cold-start argument
  in §5 already assumes the plugin host is a separate thing being warmed in
  parallel, so the architecture is priced for it.

## What would change this

Nothing about performance. If an interaction is too slow across IPC, the answer
is a better-shaped capability — coarser calls, batched payloads, a negotiated
shared buffer for the specific data — not a hole in the boundary. A plugin API
that is fast because it is unsafe is not a plugin API; it is a patch loader with
a manifest file.
