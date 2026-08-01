# Sign-in — what it would actually take

**Status: discovery only. Nothing here is implemented.** No credentials were
used, entered, or created while writing this. This document exists to turn
"there is no sign-in" from a known gap into a scoped, evidence-backed plan.

**Related:** `docs/design/instances-and-launch.md` (a related, independently
written and *not yet verified* theory about a `roblox://` ticket-launch path —
referenced in §4, not duplicated).

---

## How to read this document

Every claim below is tagged:

- **[verified: log]** — read directly out of `docs/traces/waydroid-roblox-startup.log.gz`,
  the capture of this exact APK running on real Android. This is the strongest
  evidence available in this project, per `docs/NEXT.md`'s standing rule.
- **[verified: dex]** — read directly out of the shipping APK's own dex
  declarations (class names, method names, signatures) using
  `dexsig.py`/`dexsig_cls.py`, or out of `libroblox.so`'s exported symbol table
  with `readelf`. This is the host app's own contract, not a guess about it.
- **[verified: cordial]** — read directly out of Cordial's current source.
- **[inferred]** — a plausible reading of the above that was not itself
  observed running. Flagged explicitly every time, with the reasoning shown so
  it can be checked rather than trusted.

The capture is of a **logged-out** cold start. Absence of some behaviour in it
(no captcha, no full login POST) is not evidence that behaviour doesn't exist
elsewhere in the app — only that this particular run didn't trigger it. Said
again inline wherever it matters.

---

## 1. What does the engine expect?

There are **two separate channels** the engine uses for identity, not one.
Cordial's docstring in `native/android_classes.cpp` only names the first.

### 1.1 `NativeUserJavaInterface` — queried on demand **[verified: cordial]**

`native/android_classes.cpp:225-265` already stubs this correctly for a
logged-out user: `getUserId` → 0, `getUsername`/`getDisplayName`/
`getAlternateName` → `""`, `getIsUnder13` → false, `getMembershipType` → 0,
`getHasRobloxSubscription` → false. The engine calls these when it needs to
know who's signed in (e.g. building a `rbxthumb://` avatar URL), and Cordial's
existing comment is correct that a fabricated non-zero id would be worse than
an honest zero.

### 1.2 `StartAppParams` — pushed once, at app-start time **[verified: cordial]**

`native/init_params.cpp:695-752` defines `com.roblox.engine.jni.autovalue.StartAppParams`,
the object handed to `nativeAppBridgeV2StartAppWithParams` when the engine's
app-shell actually starts. It **already has the fields**:

```cpp
std::shared_ptr<String> username;
jlong appUserId = 0;
jboolean isUnder13 = false;
jint membershipType = 0;
```

All four are currently hard-zeroed in `StartAppParams::Create()`
(`init_params.cpp:730-732`). This is a second, independent place identity has
to be threaded through — it is not automatically consistent with whatever
`NativeUserJavaInterface` answers, because it's a different call at a
different time. Any real implementation has to fill in both.

### 1.3 What would actually be different if these were real

**[verified: log]** The logged-out capture shows the exact symptom the task
description calls out:

```
1338: rbxthumb://type=AvatarHeadShot&id=&w=48&h=48&filters=circular  → "invalid format"
1375: rbxthumb://type=AvatarHeadShot&id=-1&w=48&h=48&filters=circular → "The requested Ids are invalid, of an invalid type or missing."
```

**This is an important correction to the premise in the task description.**
The exact string *"The requested Ids are invalid, of an invalid type or
missing"* is what the **real, logged-out Android client** produces too, for
`id=-1`. It is not a Cordial-specific defect — it is the correct, faithful
behaviour of a client with no signed-in user. Cordial's `getUserId() → 0`
already reproduces the *class* of failure the real client has in this state
(the real client's sentinel appears to be `-1`, not `0` — a one-line, low-risk
alignment worth making so failure messages match exactly, but not
behaviourally different).

**[verified: log]** Also from the capture, three lines up the stack from the
thumbnail failures:

```
1291: HTTP error url: https://apis.roblox.com/attribution/v1/events/post-authentication
1299: HttpResponse ... status:401 ... url: https://users.roblox.com/v1/users/authenticated/app-launch-info
1331: HTTP error body: {"errors":[{"code":9002,"subcode":0,"message":"Authentication token is missing"}]}
1337: HTTP error body: {"sessionId":"00000000-0000-0000-0000-000000000000","status":403,"message":"Unauthorized."}
```

So a real session changes things at the **network layer**, not primarily at
the `NativeUserJavaInterface`/`StartAppParams` layer. The engine calls
`/v1/users/authenticated/app-launch-info` and other `authenticated/*`
endpoints itself, over its own HTTP client, and those calls **succeed or fail
based on the cookie the engine's HTTP client is carrying** — not based on what
`getUserId()` returns. `getUserId()`/`StartAppParams.appUserId` look like they
mirror an already-established server-side session rather than establish one.
**[inferred]** This means the cookie (§2) is the actual source of truth, and
`NativeUserJavaInterface`/`StartAppParams` are secondary, presentation-layer
mirrors of it — filling them in without a real cookie would produce a client
that *displays* a fake identity while still getting 401s from every
`authenticated/*` endpoint, which is worse than the current honest state, not
better.

**[verified: log]** Two more facts from the same run that constrain the
picture:

- `ActivityNativeMain.onResume(): IsLoggedIn = false` and
  `navigateToMainScreen: IsLoggedIn = false` (lines 941, 961) — the real app
  has an explicit `IsLoggedIn` boolean it branches the UI on, checked before
  any of the native calls above run.
- `initializeLuaAppWithLoggedInUser` (line 1031) and `userDidLogin` (line
  1191, an `[FLog::SingleSurfaceApp]` event) both fire **unconditionally**,
  even in this logged-out run. **These names are misleading if read as proof
  of authentication** — they are generic app-shell lifecycle stages the client
  reaches regardless of session state, not evidence that a real login
  happened. Do not treat either as a signal to test against.

---

## 2. Where does the session actually live?

### 2.1 The exported contract **[verified: dex + readelf]**

`com.roblox.engine.jni.NativeSettingsInterface` (all confirmed as real
`Java_com_roblox_engine_jni_NativeSettingsInterface_*` exports in
`libroblox.so`, with signatures read from the dex):

```
nativeGetCookiesForDomain(String) → String
nativeGetCookiesInNetscapeFormat(String) → String
nativeSetMultipleCookies(String, String) → void
nativeSetUserId(String) → void            (not previously documented anywhere in this repo)
nativeIsLuaLoginEnabled() → boolean
```

These are natives **the engine implements and Java calls** — the same
direction as the `nativeSetFilesDirectory`/`nativeSetDeviceInfo` family
Cordial already drives from `native/init_params.cpp` and
`crates/cordial-runtime/src/bin/load.rs`. Nothing here is currently called by
Cordial; grep confirms zero references to any of these five names anywhere in
`native/` or `crates/` before this document.

### 2.2 The reverse direction — the engine pushes cookies out, too

**[verified: dex + readelf]** `com.roblox.universalapp.cookie.JNICookieProtocol.updateOnSetCookieHandler(OnSetCookieHandler)`
is also a real exported native (`Java_..._JNICookieProtocol_updateOnSetCookieHandler`).
The real app's implementation of that handler,
`CookieProtocol$OnSetCookieHandlerImpl.onSetCookie(String[], String)`, is a
**plain Java class** (not a native) — meaning the engine, on receiving a
`Set-Cookie` from its own HTTP client, calls back **into Java** through
whatever handler object was registered.

**[verified: log]** This is directly observed firing, twice, during the
logged-out run:

```
1222: Flushed WebViewCookieHandler with Cookies from URL https://apis.roblox.com/browser-tracker-api/device/initialize
1223: OnSetCookieHandlerImpl.b(): Updated WebViewCookieHandler with Cookies from URL https://apis.roblox.com/browser-tracker-api/device/initialize
1355: Flushed WebViewCookieHandler with Cookies from URL https://apis.roblox.com/account-switcher/v1/getLoggedInUsersMetadata
1356: OnSetCookieHandlerImpl.b(): Updated WebViewCookieHandler with Cookies from URL https://apis.roblox.com/account-switcher/v1/getLoggedInUsersMetadata
```

So the contract has **three** legs, not one, and all three are exercised even
with no user signed in (this traffic is device/tracking cookies, not auth
cookies — but the plumbing is identical for both):

| Direction | Mechanism | Verified how |
|---|---|---|
| Java **pulls** cookies out of the engine | `nativeGetCookiesForDomain`/`nativeGetCookiesInNetscapeFormat` | dex + readelf signatures; `InitHelper: updateCookiesFromEngine` log line (§2.3) |
| Engine **pushes** cookies out to Java | `JNICookieProtocol.updateOnSetCookieHandler` registers a callback; engine `CallMethod`s it | dex + readelf; log lines 1222-1225, 1355-1356 |
| Java **injects** cookies into the engine | `nativeSetMultipleCookies(domain, cookies)` | dex + readelf signature only — not observed firing in this (logged-out, no WebView-login) capture |

**[inferred]** The purpose of this three-way sync: the engine's own HTTP
client and Android's `WebView`/`CookieManager` are two separate cookie jars on
real Android, and the app keeps them consistent so that (a) a cookie the
engine's network layer receives (e.g. after login) is visible to any WebView
the app opens next (communities, captcha, help), and (b) a cookie a WebView
obtains (e.g. the user just typed a password into a login page) is fed back
into the engine so its own HTTP client starts authenticating too. This
explains why `nativeSetMultipleCookies` exists at all — it is the write path
for exactly that second case, and per the task's framing, is the one Cordial
would need to call to hand the engine a real session.

### 2.3 Timing — cookie sync starts before the app shell even exists

**[verified: log]** The earliest cookie-related line in the entire capture is
at `ActivitySplash`, before `ActivityNativeMain` is even created:

```
300: E InitHelper: [l0.S()-160]: updateCookiesFromEngine: Invalid cookie format: []
```

This fires right after crashpad initialization, well before
`nativeAppBridgeSetInitParams`. **[inferred]** The obfuscated caller
(`l0.S()`, logged under the `InitHelper` tag) is almost certainly calling one
of `nativeGetCookiesForDomain`/`nativeGetCookiesInNetscapeFormat` this early
and getting an empty result back (`[]`), which it logs as a format error
rather than treating as fatal — i.e. **calling these natives before the
engine has anything to say is expected and handled gracefully**, not a
precondition Cordial has to satisfy before calling them.

### 2.4 A loose end, flagged rather than resolved

**[verified: readelf, contradicted by dex]** `libroblox.so` also exports
`Java_com_roblox_universalapp_cookie_JNICookieManager_{getCookie,setCookie,setCookiesFromDisk,convertCookiesToNetscape}`
— a *different* class from `JNICookieProtocol`. Searching all three dex files'
raw strings for `JNICookieManager` (not just via `dexsig`, but plain `strings`)
returns **zero matches** — the class does not exist anywhere in this APK's
dex. **[inferred]** This reads as vestigial: an older/renamed native symbol
table entry with no Java-side caller left in this build (native libraries
aren't proguard-stripped the way dex is, so dead exported symbols are
unsurprising). Treat `JNICookieManager` as dead code, not a second live
contract — the three `NativeSettingsInterface` methods plus
`JNICookieProtocol` are the live path.

---

## 3. Is a WebView required?

**Short answer: partially yes, and the part that's required is bigger than a
single captcha screen — but the core username/password submission may not be
one of the WebView parts.** Both halves of that are evidenced below; this
refines rather than simply confirms `docs/findings.md` §5(d)/§3.6.

### 3.1 WebView is loaded unconditionally at process start

**[verified: log]** Line 235, well before `ActivityNativeMain` or any login
UI: `WebViewFactory: Loading com.google.android.webview version 150.0.7871.181`.
Android's system WebView is instantiated during ordinary app startup, not
lazily when a login button is pressed. This alone doesn't prove login *needs*
it, but it means "just don't implement WebView" is not free even before
touching sign-in — communities and captcha (below) already depend on it per
the existing framework inventory.

### 3.2 The captcha flow is a URL loaded in a WebView-hosted Activity

**[verified: dex]** `com.roblox.client.captcha.ActivityFunCaptcha` is a
declared Activity (confirmed previously in `framework-api-inventory.md` §3.5).
Its configuration classes resolve to a URL:

```
LoginCaptchaConfig.getUrl() → String
SignUpCaptchaConfig.getUrl() → String
CaptchaConfig.getUrl() → String            (the common base type)
```

A config object that is just a URL, handed to an Activity whose only declared
purpose is captcha, is consistent with loading a challenge page in a WebView
— nothing here contradicts the existing finding that captcha is WebView-based.
This document did not find the internal fragment's layout (obfuscated names
like `H0(LayoutInflater, ViewGroup, Bundle) → View` don't reveal what view
they inflate without decompiling, which is off-limits per this project's
rules) but the URL-based config plus the WebView's unconditional early load
(§3.1) is corroborating rather than new doubt.

### 3.3 But there's a flag for a non-WebView login path, unexplained in prior docs

**[verified: dex + readelf]** `NativeSettingsInterface.nativeIsLuaLoginEnabled() → boolean`
is a real native. **[inferred]** Roblox's app shell (the "Landing", "Startup"
etc. stages visible throughout the capture) is rendered by the Lua-based
`SingleSurfaceApp`/`UniversalApp`, the same layer that renders the rest of the
UI without a WebView. A flag named `IsLuaLoginEnabled` reads most naturally as
"is the login *form* (username/password fields) rendered by the Lua app shell
rather than a WebView", talking to `auth.roblox.com` (DNS for which is
pre-warmed at startup — line 103: `Pre-warmed DNS for auth.roblox.com`)
directly over HTTPS, which Cordial already supports end-to-end.

If that reading is right, **basic username/password login may not require a
WebView at all** — only the captcha challenge and any federated (Google/Apple)
sign-in option would. This is not verified by running anything (the captured
run never reached a login screen), so treat it as the single most valuable
thing to confirm early if this work is picked up: instrument
`nativeIsLuaLoginEnabled`'s return value and see whether the login form that
appears is native/Lua or a WebView.

### 3.4 The honest expectation regardless

**[inferred]** Roblox's captcha is risk-based and server-decided, not
something the client can predict or skip. A Linux host presenting as a
brand-new, unrecognized Android x86_64 device is close to the profile that
triggers step-up challenges most often. So even if password submission itself
turns out to be WebView-free, **a real user attempting to actually sign in
through Cordial should be expected to hit `ActivityFunCaptcha` on a first
attempt**, and a WebView implementation should be budgeted for, not treated as
optional-in-practice. This matches — and does not overturn —
`framework-api-inventory.md`'s conclusion that WebView "is also on the login
path... not optional."

---

## 4. Is there a path that avoids it?

Three non-web-form surfaces are declared in the APK. None of them turned out
to be a low-effort shortcut on inspection — stated plainly, since that's the
honest answer to the question asked.

### 4.1 Passkeys / `CredentialManager` — already scoped, still large

Covered in depth by `framework-api-inventory.md` §3.6 already: the platform
`android.credentials.CredentialManager` path is real and avoids GMS, but
still means implementing the platform credential-provider contract and
bridging to `libfido2`/`xdg-desktop-portal` — not smaller than WebView, just a
different large piece of work. Nothing new found here; not re-litigated.

### 4.2 "Magic Login" — local-network device pairing, genuinely non-web, unverified in operation

**[verified: dex strings]** Distinct from `JNIAccountProtocol`'s
`getMagicLoginActionKey`/`getMagicLoginMethodName` (small JS-bridge-style
constant getters, same pattern as `FlagJniInterface`), the dex contains a
cluster of unrelated-looking class names:

```
MagicLoginManager
MagicLoginNsdHelperClient / MagicLoginNsdHelperServer
MagicLoginSocketHelperClient / MagicLoginSocketHelperServer
MagicLoginSocketTimeout
Roblox-Magic-Login          (an HTTP header name)
```

**[inferred]** `Nsd` is Android's Network Service Discovery (mDNS) API. This
shape — an NSD advertise/discover pair plus a raw socket helper plus an HTTP
header — reads as a **"sign in using another device that's already logged
in"** flow: an already-authenticated device (e.g. the user's phone) advertises
on the LAN, a new device discovers it and exchanges a session over the local
socket, likely with a server-mediated confirmation (the HTTP header suggests
a request is also sent to Roblox's servers as part of the handshake). This is
the shape of the "sign in with your phone" flow real Roblox ships on
console/TV-class clients.

**This would let Cordial obtain a real session without ever presenting a
password field or a WebView itself** — the credential entry happens on the
peer device, which the user already trusts. It is the most promising
non-WebView lead this investigation found.

**Caveat, stated as strongly as the evidence deserves:** this was found by
`strings`-grepping obfuscated class names in a dex, not by observing it run.
The logged-out capture never triggers this flow (there is no reason it would,
on a cold start), so nothing here confirms it actually works, what the wire
protocol is, or whether it's gated behind a flag/feature that's off in this
build. It needs its own investigation — likely by triggering it on a real
device and capturing the traffic — before it can be relied on for a design.

### 4.3 The `roblox://` launch-ticket path — a different document's open question, not resolved here

`docs/design/instances-and-launch.md` §2 already describes (and explicitly
flags as **unverified**) a theory that pressing "Play" on roblox.com in an
external browser emits a `roblox://`-scheme URI carrying a short-lived
authentication ticket, and that `ActivityProtocolLaunch` consuming it is what
lets multiple Cordial instances each be signed in as a different account
without Cordial ever touching credentials.

**[verified: dex strings]** What this document found narrows that: the
`roblox://` URIs actually declared in the dex are all **game-join** shaped —
`roblox://placeId=%d&reservedServerAccessCode=%s&callId=%s`,
`roblox://experiences/start`, `roblox://placeId=%d&gameInstanceId=%s&callId=%s`
— not a bare authentication URI. `ActivityProtocolLaunch`'s own methods
(`i2(String) → boolean`, `j2() → void`) give no further signature detail
without decompiling.

**[inferred]** This is more consistent with the classic Roblox desktop-client
"join ticket" — scoped to launching into one specific place/server, minted by
a browser session that is *already* fully authenticated — than with a
general-purpose account sign-in mechanism. It may still be genuinely useful
(a join ticket plausibly carries enough for the client to also treat itself as
signed in as that account for the session, which is `instances-and-launch.md`'s
working theory), but this document did not find evidence settling whether a
join ticket alone produces a durable, reusable `.ROBLOSECURITY`-equivalent
cookie, or only an ephemeral per-join credential. **This is the same open
question `instances-and-launch.md` §8 already lists** ("verify the `roblox://`
URI format and whether tickets are genuinely single-use and session-scoped")
— this investigation corroborates that it's worth answering, without
answering it.

### 4.4 Verdict on §4

No path was found that is both (a) confirmed to work and (b) avoids either a
WebView or a second physical/logical device. If forced to rank by plausible
effort: §4.2 (magic login) is the cheapest **if** it turns out to work as
inferred, §4.3 (join ticket) is worth resolving because another design
already depends on it, and §4.1 (passkeys) is real but not small. None of
these should be assumed as the plan without running something first — every
one of them is currently a string in a dex, not an observed behaviour.

---

## 5. What is the minimum viable step, if a real session existed?

This section assumes a real `.ROBLOSECURITY`-style session cookie has been
obtained by some means outside Cordial (e.g. a real browser login performed
by the user, independent of Cordial itself — consistent with `instances-and-launch.md`
§7's "Cordial stores no credentials" stance) and asks what the smallest change
to Cordial would be to make the engine actually use it. It does **not**
propose building any login UI.

### 5.1 The existing call order, and where the new calls slot in

**[verified: cordial]** `crates/cordial-runtime/src/bin/load.rs` already
drives this exact sequence (line numbers as of this session):

```
NativeSettingsInterface.nativeSetFilesDirectory / nativeSetCacheDirectory /
  nativeSetExternalDirectory / nativeSetBaseDataDirectories        (~652-669)
MainGameActivity.nativeSetAssetPath                                 (~676)
NativeSettingsInterface.nativeSetRobloxVersion / nativeSetRobloxChannel (~681,690)
NativeSettingsInterface.nativeSetDeviceInfo                         (~699)
MainGameActivity.nativeAppBridgeSetInitParams                       (~766)
  ...
MainGameActivity.nativePreloadFlagOverrides                         (~957)
MainGameActivity.nativeRetryInit                                    (~1001)
NativeGLInterface.nativeAppBridgeV2InitWithParams                   (~1071)
NativeGLInterface.nativeAppBridgeStartLuaAppDM                      (~1082)
NativeGLInterface.nativeAppBridgeV2StartAppWithParams                (~1116)
```

### 5.2 The concrete proposal

1. **Call `NativeSettingsInterface.nativeSetMultipleCookies(domain, cookies)`
   once the real cookie is available**, before `nativeAppBridgeSetInitParams`
   (~766) — so the engine's own HTTP client is carrying the session before
   the app-bridge sequence that will immediately start hitting
   `authenticated/*` endpoints (§1.3) begins. `nativeSetUserId(String)`
   (§2.1, previously undocumented) should be called in the same neighborhood
   — its exact required timing relative to the cookie call is **not verified**
   and would need to be checked against behaviour, not assumed.
2. **Register a real `OnSetCookieHandler`** via
   `JNICookieProtocol.updateOnSetCookieHandler` (§2.2) so cookies the engine's
   own HTTP client subsequently receives (e.g. a session refresh) are
   observed rather than silently dropped — this is the callback direction
   Cordial has no equivalent of yet, structurally the same shape as the
   existing `NativeHelper::gameActivity_onFlagsFailed` callback pattern
   already used in `native/android_classes.cpp`.
3. **Fill in `StartAppParams`** (`init_params.cpp:730-732`) — `appUserId`,
   `username`, `isUnder13`, `membershipType` — from the same real account,
   right before `nativeAppBridgeV2StartAppWithParams` (~1116).
4. **Fill in `NativeUserJavaInterface`** (`android_classes.cpp:227-250`) —
   `getUserId`, `getUsername`, `getDisplayName`, `getMembershipType`,
   `getHasRobloxSubscription`, `getIsUnder13` — consistently with the same
   account, since the engine can call these at any later point and a mismatch
   with what `StartAppParams` said would be a self-contradicting client.
5. **Verify against the log, not by assumption**: rerun with
   `CORDIAL_MONITOR=1`, capture the engine's own FastLog
   (`$CORDIAL_FILES_DIR/files/appData/logs/*.log` per the project's standing
   diagnostic), and check that `authenticated/*` calls now return 200 instead
   of the 401/403 pattern in §1.3, and that the two `rbxthumb://` failures in
   §1.3 are replaced by a real avatar id.

Nothing in steps 1-4 requires new JNI plumbing beyond what already exists in
`native/init_params.cpp`'s `cordial_call_static_strings`/`cordial_call_bare`
helper family (§2.1's natives all fit the "static native, up to three string
args" shape those helpers already handle) plus one new callback registration
(step 2) shaped like the existing `NativeHelper` pattern. This is a
small, mechanical change **conditioned entirely on having a real cookie to
put in it** — which is the actual blocker, per §4.

---

## 6. Summary

| Question | Answer | Confidence |
|---|---|---|
| What does the engine expect differently when logged in? | Two identity mirrors (`NativeUserJavaInterface`, `StartAppParams`) plus, more importantly, a real cookie flowing through the engine's own HTTP client — the mirrors alone don't unblock `authenticated/*` calls. | Verified (log) for the HTTP-layer part; the mirrors' role is inferred but low-risk to fill in regardless. |
| Where does the session live? | Three-way cookie sync between the engine's HTTP client and Android's WebView `CookieManager`, via `NativeSettingsInterface`'s three cookie natives plus `JNICookieProtocol`'s callback. | Verified (dex, readelf, log) for the contract shape; `nativeSetMultipleCookies` itself was not observed firing. |
| Is a WebView required? | For captcha: yes, and expect it to fire in practice. For plain username/password: possibly not (`nativeIsLuaLoginEnabled`) — unresolved, worth confirming first. | Captcha: verified-adjacent. Password path: inferred, unconfirmed. |
| Is there a path that avoids it? | No confirmed one. Local-device "magic login" is the most promising unverified lead; a join-ticket path already assumed by another design doc remains unresolved. | Inferred, both. |
| What's the minimum viable step? | Five mechanical calls into existing/adjacent JNI plumbing, entirely gated on obtaining a real cookie by some means outside Cordial. | Verified for the mechanics; the precondition (§4) is the real blocker. |

**The one thing this document changes about the project's understanding of
its own blocker:** it is not "Cordial has no auth." It is "Cordial has no way
to *obtain* a session, and every way of obtaining one that was found requires
either a WebView, a second device, or an unresolved ticket flow." The stub
code in `NativeUserJavaInterface` is not the blocker — it is honestly
reporting a true fact (nobody is signed in) and would need real data fed to
it regardless of which acquisition path gets built.
