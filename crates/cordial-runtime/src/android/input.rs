//! Display-server-independent input plumbing, shared by [`super::window`] (X11)
//! and [`super::wayland`].
//!
//! X11 delivers keysyms directly. Wayland delivers raw evdev keycodes plus an
//! xkb keymap the client has to interpret itself — see ADR-011. Everything
//! *below* that difference is identical: both backends end up with a keysym, a
//! button number, or committed text, and from there the two paths converge.
//! This module is that convergence point. It used to live inside `window.rs`,
//! written for the only backend that existed; the text-entry state machine in
//! particular (`TextField`, the caret arithmetic, the reseed-on-focus-change
//! logic) took real iteration to get right — see the tests below — and a
//! second display backend is exactly the situation duplicating it would have
//! caused a second, silently-diverging copy of the same bugs to be fixed twice.
//!
//! What stays behind in each backend is the part that is genuinely
//! display-specific: opening a connection, reading its events, and turning
//! them into the keysym/button/text vocabulary this module speaks.

use std::ffi::{c_ulong, c_void};
use std::sync::{Mutex, OnceLock};

// --------------------------------------------------------- Android vocabulary
//
// `android.view.MotionEvent`/`KeyEvent` constants both backends synthesise
// events against, via `deliver_touch`/`deliver_key` below.

pub const BUTTON_PRIMARY: i32 = 1;
pub const BUTTON_SECONDARY: i32 = 2;
pub const BUTTON_TERTIARY: i32 = 4;
pub const ACTION_DOWN: i32 = 0;
pub const ACTION_UP: i32 = 1;
pub const ACTION_MOVE: i32 = 2;
pub const ACTION_HOVER_MOVE: i32 = 7;
pub const ACTION_BUTTON_PRESS: i32 = 11;
pub const ACTION_BUTTON_RELEASE: i32 = 12;

// `android.view.KeyEvent.META_*`.
pub const META_SHIFT_ON: i32 = 1;
pub const META_ALT_ON: i32 = 2;
pub const META_CTRL_ON: i32 = 0x1000;
pub const META_CAPS_LOCK_ON: i32 = 0x100000;

/// A pragmatic subset of keysyms mapped to `android.view.KeyEvent.KEYCODE_*`.
///
/// The values are X11's `keysymdef.h` numbering, but that numbering is not an
/// X11 peculiarity — it is the shared keysym space `xkbcommon` also uses (the
/// "xkb" in the name is literally "X Keyboard extension"), which is what makes
/// this table usable from both backends rather than needing a second one keyed
/// on evdev codes. Covers what a desktop text field and basic UI navigation
/// need — letters, digits, common punctuation, arrows, and the usual control
/// keys. Anything outside this set is dropped rather than guessed at.
pub fn keysym_to_android(keysym: c_ulong) -> Option<i32> {
    let k = keysym as u32;
    Some(match k {
        0x30..=0x39 => 7 + (k - 0x30) as i32,  // 0..9 -> AKEYCODE_0..9
        0x61..=0x7a => 29 + (k - 0x61) as i32, // a..z -> AKEYCODE_A..Z
        0x41..=0x5a => 29 + (k - 0x41) as i32, // A..Z (shifted) -> the same keycodes
        0x0020 => 62,                          // space
        0xff0d | 0xff8d => 66,                 // Return, KP_Enter
        0xff08 => 67,                          // BackSpace
        0xff09 => 61,                          // Tab
        0xff1b => 111,                         // Escape
        0xff51 => 21,                          // Left
        0xff52 => 19,                          // Up
        0xff53 => 22,                          // Right
        0xff54 => 20,                          // Down
        0xffe1 => 59,                          // Shift_L
        0xffe2 => 60,                          // Shift_R
        0xffe3 => 113,                         // Control_L
        0xffe4 => 114,                         // Control_R
        0xffe9 => 57,                          // Alt_L
        0xffea => 58,                          // Alt_R
        0xffe5 => 115,                         // Caps_Lock
        0xffff => 112,                         // Delete (forward delete)
        0xff50 => 122,                         // Home
        0xff57 => 123,                         // End
        0xff55 => 92,                          // Page_Up
        0xff56 => 93,                          // Page_Down
        0xff63 => 124,                         // Insert
        0x002c => 55,                          // comma
        0x002e => 56,                          // period
        0x002f => 76,                          // slash
        0x003b => 74,                          // semicolon
        0x0027 => 75,                          // apostrophe
        0x0060 => 68,                          // grave
        0x002d => 69,                          // minus
        0x003d => 70,                          // equal
        0x005b => 71,                          // bracketleft
        0x005d => 72,                          // bracketright
        0x005c => 73,                          // backslash
        _ => return None,
    })
}

/// Deliver one AGDK touch event, the same `MotionEvent` synthesis both
/// backends drive their pointer input through.
pub fn deliver_touch(
    handle: i64,
    action: i32,
    x: f32,
    y: f32,
    button_state: i32,
    action_button: i32,
    event_time_ms: i64,
    down_time_ms: i64,
) {
    match cordial_linker_sys::game_activity::touch(
        handle,
        action,
        x,
        y,
        button_state,
        action_button,
        event_time_ms,
        down_time_ms,
    ) {
        Ok(Some(consumed)) => {
            super::trace(format_args!("onTouchEventNative(action={action}) -> {consumed}"))
        }
        // Not registered yet — a normal race against initializeNativeCode
        // early in startup.
        Ok(None) => {}
        Err(e) => super::trace(format_args!("onTouchEventNative(action={action}) failed: {e}")),
    }
}

/// Deliver one AGDK key event, the `KeyEvent` synthesis both backends drive.
pub fn deliver_key(
    handle: i64,
    down: bool,
    key_code: i32,
    scan_code: i32,
    meta_state: i32,
    repeat_count: i32,
    unicode_char: i32,
    event_time_ms: i64,
    down_time_ms: i64,
) {
    match cordial_linker_sys::game_activity::key(
        handle,
        down,
        key_code,
        scan_code,
        meta_state,
        repeat_count,
        unicode_char,
        event_time_ms,
        down_time_ms,
    ) {
        Ok(Some(consumed)) => {
            super::trace(format_args!("onKey{}Native(code={key_code}) -> {consumed}",
                if down { "Down" } else { "Up" }))
        }
        Ok(None) => {}
        Err(e) => super::trace(format_args!(
            "onKey{}Native(code={key_code}) failed: {e}",
            if down { "Down" } else { "Up" }
        )),
    }
}

pub fn deliver_surface_redraw(handle: i64) {
    match cordial_linker_sys::game_activity::surface_redraw_needed(handle) {
        Ok(Some(())) => super::trace(format_args!("onSurfaceRedrawNeededNative")),
        Ok(None) => {}
        Err(e) => super::trace(format_args!("onSurfaceRedrawNeededNative failed: {e}")),
    }
}

// ------------------------------------------------------------ native passthrough
//
// The two `NativeInputInterface` natives Roblox's interface actually reads.
//
// Resolved once by the loader and stored here, because the input drain runs on
// the looper thread and has no access to the loaded library. Null until set, in
// which case only the AGDK path is driven — which is what shipped before, and
// which the interface ignores.
static PASS_MOUSE_MOVE: std::sync::atomic::AtomicPtr<c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
static PASS_MOUSE_BUTTON: std::sync::atomic::AtomicPtr<c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
static PASS_KEY_EVENT: std::sync::atomic::AtomicPtr<c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
static PASS_TEXT: std::sync::atomic::AtomicPtr<c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
/// `syncTextboxTextAndCursorPosition2`. Separate from `PASS_TEXT` because it is
/// a different call at a different moment, not an alternative spelling of one.
static SYNC_TEXTBOX: std::sync::atomic::AtomicPtr<c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
/// `updateKeyboardSize`, the acknowledgement that an editor is up.
static UPDATE_KEYBOARD_SIZE: std::sync::atomic::AtomicPtr<c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
/// Focus generation the keyboard state was last reported for.
static KEYBOARD_REPORTED: Mutex<Option<u32>> = Mutex::new(None);

pub fn set_input_natives(
    mouse_move: *mut c_void,
    mouse_button: *mut c_void,
    key_event: *mut c_void,
    pass_text: *mut c_void,
    sync_textbox: *mut c_void,
    update_keyboard_size: *mut c_void,
) {
    PASS_MOUSE_MOVE.store(mouse_move, std::sync::atomic::Ordering::Relaxed);
    PASS_MOUSE_BUTTON.store(mouse_button, std::sync::atomic::Ordering::Relaxed);
    PASS_KEY_EVENT.store(key_event, std::sync::atomic::Ordering::Relaxed);
    PASS_TEXT.store(pass_text, std::sync::atomic::Ordering::Relaxed);
    SYNC_TEXTBOX.store(sync_textbox, std::sync::atomic::Ordering::Relaxed);
    UPDATE_KEYBOARD_SIZE.store(update_keyboard_size, std::sync::atomic::Ordering::Relaxed);
}

/// Tell the engine whether an editor is up, when that has changed.
///
/// This closes the handshake `showKeyboard` opens. It runs from the input pump
/// rather than from inside `showKeyboard` itself because on Android the reply
/// comes from the UI thread after the IME has actually appeared, not
/// synchronously from within the request — and calling back into the engine
/// from inside its own call is a re-entry this has no reason to risk.
pub fn report_keyboard_state(current_geometry: (i32, i32)) {
    let f = UPDATE_KEYBOARD_SIZE.load(std::sync::atomic::Ordering::Relaxed);
    if f.is_null() {
        return;
    }
    let generation = cordial_linker_sys::game_activity::textbox_generation();
    {
        let mut seen = KEYBOARD_REPORTED.lock().unwrap_or_else(|e| e.into_inner());
        if *seen == Some(generation) {
            return;
        }
        *seen = Some(generation);
    }
    let visible = cordial_linker_sys::game_activity::focused_textbox().is_some();
    let (w, h) = current_geometry;
    // Zero height: no soft keyboard occupies the screen here, and a real height
    // would make the engine shift its layout up to avoid nothing.
    let r = cordial_linker_sys::game_activity::update_keyboard_size(f, visible, 0, h, w, 0);
    if trace_text() {
        eprintln!("[cordial] updateKeyboardSize(visible={visible}, w={w}, h=0) -> {r:?}");
    }
}

pub fn pass_key_event(down: bool, key_code: i32, modifiers: i32) {
    let f = PASS_KEY_EVENT.load(std::sync::atomic::Ordering::Relaxed);
    if !f.is_null() {
        let _ = cordial_linker_sys::game_activity::pass_key_event(f, down, key_code, modifiers, false);
    }
}

pub fn pass_text(which: i64, text: &str, cursor: i32) {
    // The per-keystroke sync first: this is the call that actually fills the
    // field. `nativePassText` is driven alongside it for the same reason both
    // mouse paths are — the interface declares both and the cost of driving
    // one that turns out to be a no-op is nothing.
    let sync = SYNC_TEXTBOX.load(std::sync::atomic::Ordering::Relaxed);
    if !sync.is_null() {
        if let Err(e) = cordial_linker_sys::game_activity::sync_textbox(sync, text, cursor) {
            if trace_text() {
                eprintln!("[cordial] syncTextbox failed: {e}");
            }
        }
    }
    let f = PASS_TEXT.load(std::sync::atomic::Ordering::Relaxed);
    if !f.is_null() {
        if let Err(e) = cordial_linker_sys::game_activity::pass_text(f, which, text, cursor) {
            if trace_text() {
                eprintln!("[cordial] passText failed: {e}");
            }
        }
    }
    if trace_text() {
        eprintln!(
            "[cordial] text -> {text:?} caret={cursor} sync={} passText={}",
            !sync.is_null(), !f.is_null()
        );
    }
}

pub fn pass_mouse_move(x: f32, y: f32) {
    let f = PASS_MOUSE_MOVE.load(std::sync::atomic::Ordering::Relaxed);
    if !f.is_null() {
        let _ = cordial_linker_sys::game_activity::pass_mouse_move(f, x, y, 0.0, 0.0);
    }
}

pub fn pass_mouse_button(x: f32, y: f32, down: bool) {
    let f = PASS_MOUSE_BUTTON.load(std::sync::atomic::Ordering::Relaxed);
    if !f.is_null() {
        // Button 0 is the primary button in this interface's numbering.
        let _ = cordial_linker_sys::game_activity::pass_mouse_button(f, x, y, down, 0);
    }
}

/// `CORDIAL_TRACE_TEXT=1`. Text entry is the one path where the interesting
/// question is what the host *saw*, not what the engine did, so it gets its own
/// switch rather than riding on the general trace — which is documented as
/// ABI-unsafe and aborts the engine.
pub fn trace_text() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CORDIAL_TRACE_TEXT").is_some())
}

// ------------------------------------------------------------------ text entry

static TEXT_BUFFER: Mutex<TextField> = Mutex::new(TextField::new());

/// The editing state Cordial keeps on behalf of the engine.
///
/// Android delegates text editing to the IME, and with a hardware keyboard the
/// IME is still in the loop — it receives the key events and commits finished
/// text through the InputConnection. Cordial is that IME here, so it owns the
/// caret as well as the contents. Sending the whole string with the caret
/// pinned to the end is what made typing feel broken: every keystroke dragged
/// the caret back, so arrows and clicking into the middle of a field could not
/// work by construction.
///
/// The caret is counted in `char`s, not bytes, because that is what the engine
/// is told and what a person means by "third character".
///
/// This state is display-server independent by construction: it is driven by
/// committed text and caret movements (`Edit`), which is exactly the vocabulary
/// `zwp_text_input_v3` hands over on Wayland and `XLookupString` approximates
/// on X11. Neither backend needs its own copy.
struct TextField {
    text: String,
    caret: usize,
}

impl TextField {
    const fn new() -> Self {
        TextField { text: String::new(), caret: 0 }
    }

    /// Byte offset of the caret, for slicing.
    fn byte_offset(&self) -> usize {
        self.text
            .char_indices()
            .nth(self.caret)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len())
    }

    fn len_chars(&self) -> usize {
        self.text.chars().count()
    }

    fn seed(&mut self, text: String) {
        self.caret = text.chars().count();
        self.text = text;
    }

    fn insert(&mut self, s: &str) {
        let at = self.byte_offset();
        self.text.insert_str(at, s);
        self.caret += s.chars().count();
    }

    /// Delete the character before the caret. False when there is nothing to
    /// delete, so the caller can avoid sending an unchanged state.
    fn backspace(&mut self) -> bool {
        if self.caret == 0 {
            return false;
        }
        self.caret -= 1;
        let at = self.byte_offset();
        self.text.remove(at);
        true
    }

    /// Delete the character at the caret — the `Delete` key, as distinct from
    /// backspace. Without it, correcting a typo means deleting everything after
    /// it too.
    fn delete(&mut self) -> bool {
        if self.caret >= self.len_chars() {
            return false;
        }
        let at = self.byte_offset();
        self.text.remove(at);
        true
    }

    /// Move the caret. Returns whether it moved, so a Left at position zero
    /// does not resend identical state.
    fn move_caret(&mut self, to: Caret) -> bool {
        let before = self.caret;
        self.caret = match to {
            Caret::Left => self.caret.saturating_sub(1),
            Caret::Right => (self.caret + 1).min(self.len_chars()),
            Caret::Home => 0,
            Caret::End => self.len_chars(),
        };
        self.caret != before
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Caret {
    Left,
    Right,
    Home,
    End,
}

/// The focus generation `TEXT_BUFFER` was last seeded for. `showKeyboard`
/// bumps the engine-side counter on every focus change; when this falls behind
/// it, the buffer belongs to a box that no longer has focus and is reseeded
/// from whatever the engine says the newly focused box contains.
///
/// Without this, moving from the username field to the password field carries
/// the username into it, and the first keystroke in a pre-filled field appends
/// rather than continues.
static TEXT_GENERATION: Mutex<Option<u32>> = Mutex::new(None);

/// What a key press means to the focused field.
pub enum Edit<'a> {
    Insert(&'a str),
    Backspace,
    Delete,
    Move(Caret),
}

/// Apply one edit to the focused field.
///
/// Returns the contents and caret to send, or `None` when nothing changed —
/// resending identical state on every arrow key at the end of a field makes the
/// engine redraw for no reason.
pub fn edit_text_buffer(edit: Edit<'_>) -> Option<(String, i32)> {
    let mut buf = TEXT_BUFFER.lock().unwrap_or_else(|e| e.into_inner());

    // Reseed when focus has moved since this buffer was filled.
    let generation = cordial_linker_sys::game_activity::textbox_generation();
    {
        let mut seen = TEXT_GENERATION.lock().unwrap_or_else(|e| e.into_inner());
        if *seen != Some(generation) {
            buf.seed(cordial_linker_sys::game_activity::textbox_text());
            *seen = Some(generation);
        }
    }

    let changed = match edit {
        Edit::Insert(s) => {
            // Control characters are not text. A field receives what a person
            // typed, not every key they pressed.
            if s.is_empty() || s.chars().any(|c| c.is_control()) {
                false
            } else {
                buf.insert(s);
                true
            }
        }
        Edit::Backspace => buf.backspace(),
        Edit::Delete => buf.delete(),
        Edit::Move(to) => buf.move_caret(to),
    };

    changed.then(|| (buf.text.clone(), buf.caret as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caret_edits_where_it_is_not_at_the_end() {
        // Every keystroke used to send the whole string with the caret pinned to
        // the end, which meant arrows and clicking into the middle of a field
        // could not work however the engine behaved. This is the regression that
        // made typing feel broken rather than absent.
        let mut f = TextField::new();
        f.seed("hello".into());
        assert_eq!(f.caret, 5);
        assert!(f.move_caret(Caret::Home));
        assert_eq!(f.caret, 0);
        f.insert("say ");
        assert_eq!(f.text, "say hello");
        assert_eq!(f.caret, 4);
    }

    #[test]
    fn backspace_and_delete_are_not_the_same_key() {
        // Backspace removes before the caret, Delete at it. Treating Delete as
        // backspace loses the character on the wrong side of the cursor, which
        // is the sort of bug people describe as "it eats my text".
        let mut f = TextField::new();
        f.seed("abc".into());
        f.move_caret(Caret::Home);
        assert!(!f.backspace()); // nothing before the caret
        assert!(f.delete());
        assert_eq!(f.text, "bc");
        assert_eq!(f.caret, 0);
        f.move_caret(Caret::End);
        assert!(f.backspace());
        assert_eq!(f.text, "b");
    }

    #[test]
    fn the_caret_is_counted_in_characters_not_bytes() {
        // The engine is told a character offset. Counting bytes puts the caret
        // mid-codepoint for any non-ASCII input and slices a String there, which
        // panics rather than misbehaving quietly.
        let mut f = TextField::new();
        f.seed("héllo".into());
        assert_eq!(f.caret, 5);
        f.move_caret(Caret::Home);
        f.move_caret(Caret::Right);
        f.move_caret(Caret::Right);
        assert_eq!(f.caret, 2);
        f.insert("X");
        assert_eq!(f.text, "héXllo");
    }

    #[test]
    fn a_caret_move_that_goes_nowhere_reports_no_change() {
        // Left at position zero must not resend identical state; the engine
        // would redraw the field on every held arrow key for nothing.
        let mut f = TextField::new();
        f.seed("ab".into());
        f.move_caret(Caret::Home);
        assert!(!f.move_caret(Caret::Left));
        assert!(f.move_caret(Caret::Right));
        f.move_caret(Caret::End);
        assert!(!f.move_caret(Caret::Right));
    }
}
