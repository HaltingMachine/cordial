# ADR-006: Plugins may declare their own events, and some plugins ship with Cordial

**Status:** accepted
**Related:** [ADR-002](ADR-002-core-shell-and-ui-handoff.md), [ADR-003](ADR-003-plugin-isolation.md), [ADR-005](ADR-005-flag-service.md)

## Decision

Two things, which are separable but arrive together.

**1. A plugin may declare event types and broadcast on them**, behind two new
capabilities:

| | |
|---|---|
| `events.declare` | register event types under the plugin's own namespace |
| `events.publish` | broadcast on an event type the plugin declared |
| `events.subscribe` | receive events, including ones other plugins declared |

A plugin may only publish on types it declared itself. Namespacing is by plugin
id and is not optional: `flag-manager/profile-changed`, never `profile-changed`.

**2. Some plugins ship with Cordial as first-party**, installed by default and
loaded on demand when something depends on them. They are ordinary plugins —
same manifest, same capability grants, same isolation — and they are visible and
individually disableable in settings.

## Why plugin-declared events

**Because the alternative is a core that grows a case for every plugin.** A
multi-instance plugin wants to say "an instance started"; a launcher wants to
hear it. If every such pair needs a new core event type, the core's event
vocabulary becomes a list of whatever plugins happened to exist, and every new
integration is a core change. Letting plugins declare their own keeps the core's
vocabulary about *Cordial*, and lets the ecosystem grow sideways.

**Declaration is separate from publication on purpose.** A plugin that can
publish on any string can impersonate another plugin's events, and a subscriber
has no way to tell. Declaring first, under a namespace derived from the plugin
id rather than chosen by the plugin, makes the origin of an event a fact rather
than a claim.

**Subscription is deliberately broader than publication.** Hearing an event tells
you something happened; publishing tells everyone else something happened. Those
are different powers and it would be a mistake to grant them together — a plugin
that only reacts should not have to be trusted to speak.

## Why first-party plugins are still plugins

**Because "built in" and "a plugin" are not opposites, and pretending otherwise
costs the architecture.** The moment a behaviour is implemented in core because
it ships by default, it stops being subject to the capability model, stops being
inspectable the way a plugin is, and stops being removable. Cordial would then
have two kinds of behaviour with two sets of rules, and the interesting one would
be the one users cannot see.

Keeping them as plugins means the default feature set is *an example of the
plugin API being sufficient*. If a first-party plugin needs something the API
cannot express, that is a signal the API is incomplete — exactly the argument
[ADR-001](ADR-001-in-process-hooking.md) makes about the framework layer.

**Loaded on demand, not eagerly.** A first-party plugin that nothing depends on
should not be running. Resolution is by dependency: a plugin declares that it
needs `cordial/multi-instance`, and that plugin starts if it is not already up.
This is the same shape as an ESM import graph and it should behave like one —
resolved once, shared, not restarted per dependent.

**On by default is a defaults question, not an architecture question.** Some
first-party plugins will be on out of the box because the client is worse without
them. That is a setting, and settings are reversible. What must not happen is a
plugin that cannot be turned off being described as a plugin.

## Consequences

**Accepted:** the event registry is a real runtime object with ownership rules,
not a hashmap of strings. It has to record which plugin declared each type,
refuse a publish from anyone else, and survive a plugin restarting without
letting a different plugin claim its namespace in the gap.

**Accepted:** a first-party plugin ships in the repository and is covered by its
tests, so the plugin API is exercised by Cordial's own CI rather than only by
third parties.

**Accepted:** dependency resolution introduces a failure mode the current host
does not have — a plugin that depends on one that will not start. That must
surface as a named error to the dependent, not a silent absence, for the same
reason `denied` and `error` are distinct in the protocol.

**Rejected:** letting plugins publish on core event types. Core events describe
what Cordial did; a plugin claiming Cordial did something it did not is a
correctness problem for every subscriber. Plugins declare their own or say
nothing.

**Open:** whether a subscriber can filter by declaring plugin at subscribe time,
or must filter on receipt. Filtering at subscribe is better for both privacy and
cost, but needs the registry to answer "who declared this" before delivery.

## What would change this

If the event registry turns out to be a way for plugins to fingerprint each
other — learning what is installed by watching what is declared — subscription
would need to be scoped to declared dependencies rather than open. That is worth
checking before the first plugin that handles anything account-shaped ships.
