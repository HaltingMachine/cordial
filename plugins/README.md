# Plugins

A plugin is a directory containing `plugin.json` and its entry module. Cordial
discovers them under `~/.local/share/cordial/plugins/`.

```json
{
  "id": "flag-inspector",
  "name": "Flag Inspector",
  "entry": "main.ts",
  "capabilities": ["flags.read", "log"]
}
```

`capabilities` is what the plugin **requests**. What it actually gets is what you
approved in `~/.config/cordial/plugin-grants.json`:

```json
{ "flag-inspector": ["flags.read", "log"] }
```

Default deny — a plugin missing from that file gets nothing, and a capability it
asked for but you did not grant is refused at the point of use, by name, so the
author can tell "not allowed" from "broken".

## What a plugin cannot do

It runs as a separate Deno process started with **no permissions**: no file,
network, environment or subprocess access. That is a second, independent layer
under Cordial's own capability broker, so a plugin cannot reach the machine even
if the broker had a hole in it.

There is no script execution against Roblox and no memory access. Those are
absent from the API rather than disabled — see
[ADR-001](../docs/adr/ADR-001-in-process-hooking.md) and
[ADR-003](../docs/adr/ADR-003-plugin-isolation.md).

A plugin granted `assets.override` **may** provide files that resolve in place
of Roblox's own for the same name — a non-destructive overlay, never a write
into the APK or anything extracted from it. See
[ADR-010](../docs/adr/ADR-010-plugin-asset-overlays.md) for what that does and
does not permit: it covers cosmetic substitution, and it explicitly does not
protect you from a plugin that overlays a gameplay-affecting asset such as a
collision mesh. That is the same trust decision you already make in approving
the capability at all.

## Host resources are brokered, never handed over

A plugin never receives a socket, a D-Bus connection or a file descriptor, and
installing one can never widen Cordial's Flatpak permissions. Where a capability
needs a host resource, Cordial holds the permission and performs the effect.

Discord Rich Presence is the example: `presence.set` takes a presence payload.
Cordial owns the connection to Discord's IPC socket — your plugin never learns
where it is, cannot read Discord's state, and cannot send anything else down it.

The reasoning is in [ADR-007](../docs/adr/ADR-007-host-resources-are-brokered.md),
and the short version is that a Flatpak permission is app-wide and permanent
while a capability is per-plugin and revocable. If installing a plugin could add
a permission, uninstalling it would not take the permission away.

This also means a resource Cordial does not already broker needs a change to
Cordial rather than to your manifest. Slower on purpose: the manifest is the one
place anyone can read the whole sandbox, and it should stay true.

To keep that rare, the common effects are already brokered — `presence.set`,
`notify.send` and `url.open` (`http`/`https` only). If you need one that is not
here, open an issue: a broker is a payload type and an effect, so adding one is a
small change rather than a redesign. If a proposed broker *cannot* be small, that
usually means the capability is too broad and wants splitting.

## Flags have two lifetimes

`flags.write` contributes flags that take effect at the **next launch**.
`flags.write.dynamic` changes one **while the client runs**, and only works for
the `DFFlag`/`DFInt`/`DFString` families — the static families are read once at
startup and cannot be changed live at all. They are separate capabilities so that
an API call cannot silently do nothing. See
[ADR-005](../docs/adr/ADR-005-flag-service.md).

## Example

[`flag-inspector/`](flag-inspector) reports which flag overrides are in effect
and where each came from, then deliberately attempts a write it was not granted
so the refusal is visible.
