# ADR-005: The flag service has two surfaces, because flags have two lifetimes

**Status:** accepted
**Supersedes:** nothing
**Related:** [ADR-003](ADR-003-plugin-isolation.md)

## Decision

Plugin-contributed FastFlags are exposed through **two** interfaces, not one:

1. A **launch-time resolver**. Plugins declare flags in
   `~/.local/share/cordial/plugins/<id>/flags.json`. Cordial reads every layer
   before the engine process starts, resolves them, and merges the result into
   the client-settings document the engine is given.
2. A **runtime API**, restricted to the `DFFlag`/`DFInt`/`DFString` family, which
   a plugin may call while the client is running.

A plugin loaded on demand — when it first appears in the ESM import tree — gets
the runtime API only. It cannot influence launch-time flags, and the API will
say so rather than silently accepting the call.

## Why

**Because the engine reads the two families at different times, and this is
measured rather than assumed.** `FFlag`, `FInt` and `FString` are consumed once,
during `nativeInitClientSettings`, roughly 100 ms into startup. Only the
`DFFlag`/`DFInt`/`DFString` family is re-read while the client runs. That is what
"dynamic" means in Roblox's own naming, and it is the whole constraint.

**A single unified API would be a lie for most flags.** The natural design — a
service plugin that manages flags, lazily loaded the first time something imports
it — cannot work for startup flags, because by the time any plugin host is up the
engine has already read them. An API that accepted `FFlagSomething` from a
lazily-loaded plugin would appear to succeed and do nothing. That is the exact
failure mode this project keeps finding elsewhere: a call that returns
successfully and has no effect, which is far more expensive to debug than a call
that fails.

**Splitting them makes the constraint visible at the point of use.** A plugin
author writing to `flags.json` can see it is a file read at launch. A plugin
author calling the runtime API gets an error for a non-`DF` flag, at the moment
they try, rather than a silent no-op discovered three days later.

**Layering already carries provenance, and the same rules apply here.** The
resolver is implemented in
[`crates/cordial-runtime/src/flags.rs`](../../crates/cordial-runtime/src/flags.rs):
the user's file always wins over any plugin, plugin-against-plugin conflicts are
reported rather than quietly resolved, and every effective value records which
layer set it. Extending that to a runtime API means the same guarantees, not new
ones — in particular, **a plugin must not be able to override a value the user
set explicitly**, whichever surface it uses.

## Consequences

**Accepted:** a plugin that needs a startup flag must be installed, not merely
imported. Its `flags.json` has to exist before launch. That is a real constraint
on plugin design and it should be documented where plugin authors will hit it.

**Accepted:** the runtime API needs a way to report "this flag exists but cannot
be changed while running", distinct from "this flag does not exist". Collapsing
those two into one error would leave authors unable to tell a typo from a
lifetime mismatch.

**Accepted:** changes made through the runtime API are not persisted by default.
A flag set at runtime is gone on restart unless the plugin also writes its
`flags.json`. Making runtime changes implicitly persistent would mean a plugin
could permanently alter a user's configuration from a call that reads like a
temporary adjustment.

**Open:** whether the runtime API writes through the engine's own dynamic-flag
refresh or requires Cordial to re-present the settings document. That is an
implementation question and it has not been established by experiment yet.
Whoever builds it should determine it by running something, not by reading the
binary — see [`docs/NEXT.md`](../NEXT.md) for why that rule exists here.

## What would change this

If a future Roblox build re-read the static families at runtime, the split would
become unnecessary and the two surfaces should collapse into one. That is
checkable: set an `FFlag` with an observable effect while the client is running
and see whether behaviour changes. It does not today.
