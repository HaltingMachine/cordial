# ADR-011: Wayland is the display backend, and the window is libadwaita

**Status:** accepted
**Supersedes:** the X11-first choice recorded in [`android/window.rs`](../../crates/cordial-runtime/src/android/window.rs)
**Related:** [ADR-002](ADR-002-core-shell-and-ui-handoff.md), [ADR-009](ADR-009-capture-yes-overlay-injection-no.md)

## Decision

Cordial targets **Wayland natively**. The window is a GTK4 + libadwaita window,
and the engine renders into a Wayland surface belonging to it.

X11 is not developed further.

## Why this reverses the earlier choice

`window.rs` opens by explaining that X11 came first because
`eglCreateWindowSurface` takes an X `Window` directly, whereas Wayland needs a
`wl_egl_window` and a surface role — "more moving parts for the same first
frame". That was a correct judgement about reaching a first frame fastest. It is
the wrong judgement about the client people actually use, and two symptoms made
that concrete.

**Resize.** Dragging Cordial's window flashes flat colour for about a tenth of a
second before the engine catches up. Sober, which is native Wayland, resizes
cleanly. On Xwayland the compositor cannot know when the client's new-size
content is ready, so it shows *something* in the meantime; on Wayland the
`xdg_toplevel` configure/ack/commit sequence exists precisely so that a resize is
atomic and the surface never appears in a half-updated state. The X11 path can be
patched to flash less. It cannot be made correct, because the protocol has no way
to express what is needed.

**Text entry.** The remaining sign-in blocker is that Cordial has to act as the
IME. On Wayland there is a protocol for exactly this — `zwp_text_input_v3` —
which is how squeekboard, Maliit, ibus and fcitx all deliver text. Bridging to it
gets Linux phones and CJK input for the same work, rather than hand-rolling an
input method against raw X keysyms. That was already the plan; Wayland is where
the plan is expressible.

**The background behind the canvas.** While the engine catches up, whatever is
behind its canvas is visible. It should be the desktop's own background colour,
following light and dark mode, rather than a flash of white. That is a themed
window's job, and libadwaita answers it directly through `AdwStyleManager`.
Suppressing the repaint instead — the first attempt here — is a workaround for
not having a themed window, and was reverted.

## Why libadwaita specifically

[ADR-002](ADR-002-core-shell-and-ui-handoff.md) already specifies a native
GTK/libadwaita core shell that paints the chooser before the plugin host is up,
and a UI plugin that takes over afterwards. That shell and this window are the
same window. Building the engine's host window as a bare Wayland surface would
mean building the shell twice, and the second one would have to inherit the
theme anyway.

It also settles the light/dark question without a portal round-trip of Cordial's
own: `AdwStyleManager` already tracks `org.freedesktop.appearance color-scheme`
and updates live when the user switches.

## Consequences

**Accepted:** a native Wayland backend is substantially more code than the X11
one — registry binding, `xdg_shell`, `wl_seat` input with xkbcommon keymaps,
`wl_egl_window` for EGL, and `VK_KHR_wayland_surface` in place of
`VK_KHR_xlib_surface` in the Vulkan interposition. The X11 backend reached a
first frame sooner and that was worth having; it is not worth keeping two.

**Accepted:** the existing X11 code stays in the tree for now rather than being
deleted in the same change that adds its replacement, so that a regression in the
new backend is diagnosable against a working one. It is not a supported
configuration and it is not to be extended. It should be removed once the Wayland
path has run the sign-in flow end to end.

**Accepted:** input has to be rebuilt, not ported. X11 delivers keysyms; Wayland
delivers keycodes plus an xkb keymap the client is expected to interpret. The
`TextField` editing logic is unaffected — it takes committed text and a caret,
which is what `zwp_text_input_v3` hands over — but everything below it changes.

**Accepted:** `WM_CLASS` becomes the `xdg_toplevel` app_id, which must keep
matching the desktop entry for the reasons in
[ADR-009](ADR-009-capture-yes-overlay-injection-no.md). The test that pins them
together needs to follow.

**Rejected: Wayland with an X11 fallback maintained in parallel.** Two backends
means every input and surface bug is asked "on which one?", and the fallback
rots because nobody runs it. Xwayland exists for hosts without a Wayland
compositor and Cordial does not need to reimplement it.

## What would change this

Evidence that a Wayland compositor in common use cannot do something the engine
requires — an EGL configuration, a presentation timing guarantee — would make
this a compatibility question rather than a quality one. Nothing observed so far
suggests that.
