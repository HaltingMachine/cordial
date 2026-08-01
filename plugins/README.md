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

There is no script execution against Roblox, no memory access, and no way to
replace Roblox's assets. Those are absent from the API rather than disabled — see
[ADR-001](../docs/adr/ADR-001-in-process-hooking.md),
[ADR-003](../docs/adr/ADR-003-plugin-isolation.md) and
[ADR-004](../docs/adr/ADR-004-plugin-asset-overrides.md).

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
