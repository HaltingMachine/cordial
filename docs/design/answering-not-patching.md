# You are the platform, not a patch

[ADR-001](../adr/ADR-001-in-process-hooking.md) says what Cordial will not do:
no hooking, no memory patching, no injected script environment. That is the
prohibition. This is the reason it costs nothing.

Every capability people reach for hooking to get is already a question the client
asks out loud, through an interface it links against or calls across JNI. Cordial
implements the interface. The answer is authoritative because Cordial *is* the
thing being asked — not because it got there first and overwrote something.

| The complaint | What the client actually does | What Cordial does |
|---|---|---|
| "Roblox thinks it's on a phone." | Calls `__system_property_get("ro.product.model")`, reads `Build.MODEL`, `Build.MANUFACTURER`, display density, input source flags. | Implements `__system_property_get` in the bionic shim and the `Build.*` fields in the framework layer. Returns desktop values. |
| "Passkeys don't work." | Calls `CredentialManager.getCredential()` across JNI. | Receives the call, parses the WebAuthn parameters, dispatches to libfido2 or the xdg-desktop-portal, marshals the assertion back. |
| "FastFlags are wrong." | Calls `FlagJniInterface` methods over JNI to read flags. | Implements those methods. The flag system is under Cordial's control. |
| "Graphics don't initialise." | Links against EGL and GLES2. | Provides those libraries. From the client's perspective Cordial is the GPU driver: surface creation, context setup, buffer swaps. |
| "Input doesn't work." | Uses `GameActivity` — Apache-2.0, documented. | Implements its input contract, translating X11/Wayland events into the Android input pipeline the client already expects. |
| "Audio doesn't work." | Calls OpenSL ES. | Implements OpenSL ES, routed to PipeWire. |

In none of these does Cordial read or write the client's memory. It never needs
to. **You didn't patch a string in memory. You are the API it called.**

## Why this is a technical position and not just an ethical one

A hook is a bet that the thing you patched stays where it was. It breaks on the
next build, silently, and in a way that looks like the client's fault. An
implemented interface breaks only when the interface changes — which is a
versioned, documented, greppable event, and which the client's own developers
have to care about too.

It is also the difference between a project that can be distributed and one that
cannot. Implementing a published interface is ordinary compatibility work.
Rewriting another program's memory is not, and no amount of "but it works"
changes what it is.

## The direction of the call matters

The table above is all calls *outward*: the client asks, Cordial answers. There
is a second category that reads the same way but is not the same shape — natives
the client **exports for its host application to call inward**.

Client settings are the live example.
`NativeGLInterface.nativeInitClientSettings(String, String, String)` is a native.
The engine is not asking Cordial for settings; it is waiting for its host app to
deliver them, the way the Roblox app on Android fetches them and calls this. The
same is true of `nativePostClientSettingsLoadedInitialization3(List)` and
`readLocalFlags()`, which hands the host the engine's own bundled defaults.

So the principle generalises with one added clause. Cordial is not only the
platform underneath the client — **it is also the application around it**, and
some of the client's expectations are of its host, not of its OS. When something
never happens, the question is not only "which API did it call that I have not
implemented" but also "which call was its app supposed to make that I am not
making".

Calling an exported native is no more a patch than answering a JNI call is. It
is the interface the engine publishes for exactly this purpose. What would cross
the line is manufacturing something the interface is meant to authenticate — a
forged signature for `nativeInitClientSettingsSigned`, say — or standing up a
service impersonating Roblox's own to feed it. Use the unsigned, cached, or
local-flags paths, or let the real fetch succeed. Be the app; do not counterfeit
the server.
