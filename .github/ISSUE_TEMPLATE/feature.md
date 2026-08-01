---
name: Feature or capability
about: Something Cordial should be able to do, or something a plugin should be able to ask for
labels: feature
---

## What should be possible

Describe it from the user's side first — what someone can do afterwards that
they cannot do now.

## Whose job is it

Cordial is deliberately small, and most features belong outside it:

- [ ] **A plugin** — it uses capabilities that already exist
- [ ] **A plugin, plus a new brokered capability** — Cordial has to expose an effect first
- [ ] **The client itself** — it is about loading, rendering, input or the shell

If you are unsure, say so. Getting this wrong is cheap to correct now and
expensive later.

## If it needs a new capability

Cordial brokers effects and never hands over the resource behind them
([ADR-007](../../docs/adr/ADR-007-host-resources-are-brokered.md)). `presence.set`
takes a presence payload; Cordial owns the Discord socket. Plugins get no socket,
no D-Bus connection, no file descriptor.

So: **what is the narrowest effect that does the job?** Not what access would let
a plugin arrange it itself.

A good broker is a payload type and an effect, and adding one is a small change.
If what you are describing needs a design document, that is usually a sign the
capability is too broad and wants splitting.

## Things that will be declined

Not to discourage the issue — to save you writing it:

- Script execution against Roblox, hooking, or memory access. Absent from the
  API rather than disabled, and permanently out of scope
  ([ADR-001](../../docs/adr/ADR-001-in-process-hooking.md)).
- A generic "open a socket" or "run a program" capability. That is the whole
  brokering decision undone.
- Anything drawing on top of the running game
  ([ADR-009](../../docs/adr/ADR-009-capture-yes-overlay-injection-no.md)).

Asset overlays **are** in scope and non-destructive
([ADR-010](../../docs/adr/ADR-010-plugin-asset-overlays.md)).

## Have you checked the ADRs

`docs/adr/` records what was decided and why, including several decisions that
were later reversed on evidence. If one of them rules this out and you think the
reasoning is wrong, **say that** — argue with the ADR. ADR-004 was reversed
exactly that way, by someone pointing out the reasoning did not hold.
