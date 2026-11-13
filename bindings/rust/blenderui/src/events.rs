//! Event types passed to listeners (named as in gpui).

use crate::geometry::{Pixels, Point};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    /// The mouse's back (side) button. Only listeners for this button hear it, never
    /// `on_any_mouse_down`: it is a window's navigation, not a press on what is under it.
    Back,
    /// The mouse's forward (side) button, heard as `Back` is.
    Forward,
}

/// Modifier keys held during an event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    /// The Control key (on macOS only the Control key itself, never Command).
    pub control: bool,
    pub alt: bool,
    /// The platform key, as in gpui: Command on macOS; never set elsewhere.
    pub platform: bool,
}

impl Modifiers {
    pub(crate) fn from_kit(mods: i32) -> Modifiers {
        use blenderui_sys::{BK_ALT, BK_CMD, BK_CONTROL_KEY, BK_CTRL, BK_SHIFT};
        // Cocoa sets BK_CTRL for Command too; BK_CMD / BK_CONTROL_KEY say which key it was.
        let mac = mods & (BK_CMD | BK_CONTROL_KEY) != 0;
        Modifiers {
            shift: mods & BK_SHIFT != 0,
            control: if mac { mods & BK_CONTROL_KEY != 0 } else { mods & BK_CTRL != 0 },
            alt: mods & BK_ALT != 0,
            platform: mods & BK_CMD != 0,
        }
    }

    /// The kit's bits for these keys, as the host would report them.
    pub(crate) fn to_kit(self) -> i32 {
        use blenderui_sys::{BK_ALT, BK_CMD, BK_CONTROL_KEY, BK_CTRL, BK_SHIFT};
        let mut m = 0;
        if self.shift {
            m |= BK_SHIFT;
        }
        if self.alt {
            m |= BK_ALT;
        }
        if self.platform {
            m |= BK_CTRL | BK_CMD;
        }
        if self.control {
            m |= BK_CTRL;
            if self.platform || cfg!(target_os = "macos") {
                m |= BK_CONTROL_KEY;
            }
        }
        m
    }
}

#[cfg(test)]
mod modifier_tests {
    use super::Modifiers;

    #[test]
    fn modifiers_survive_the_kits_bits() {
        for bits in 0..16u8 {
            let m = Modifiers { shift: bits & 1 != 0, control: bits & 2 != 0, alt: bits & 4 != 0,
                                platform: bits & 8 != 0 };
            assert_eq!(Modifiers::from_kit(m.to_kit()), m);
        }
    }

    #[test]
    fn plain_ctrl_is_control_and_never_the_platform_key() {
        let m = Modifiers::from_kit(blenderui_sys::BK_CTRL);
        assert!(m.control && !m.platform);
    }

    #[test]
    fn cocoas_command_is_the_platform_key_not_control() {
        let m = Modifiers::from_kit(blenderui_sys::BK_CTRL | blenderui_sys::BK_CMD);
        assert!(m.platform && !m.control);
        let m = Modifiers::from_kit(blenderui_sys::BK_CTRL | blenderui_sys::BK_CONTROL_KEY);
        assert!(m.control && !m.platform);
    }
}

#[derive(Clone, Debug)]
pub struct MouseDownEvent {
    pub button: MouseButton,
    pub position: Point<Pixels>,
    pub modifiers: Modifiers,
    pub click_count: usize,
}

#[derive(Clone, Debug)]
pub struct MouseUpEvent {
    pub button: MouseButton,
    pub position: Point<Pixels>,
    pub modifiers: Modifiers,
}

#[derive(Clone, Debug)]
pub struct MouseMoveEvent {
    pub position: Point<Pixels>,
    pub pressed_button: Option<MouseButton>,
    pub modifiers: Modifiers,
}

/// A click: the press and the release.
#[derive(Clone, Debug)]
pub struct ClickEvent {
    pub down: MouseDownEvent,
    pub up: MouseUpEvent,
}

impl ClickEvent {
    pub fn click_count(&self) -> usize {
        self.down.click_count
    }
}

#[derive(Clone, Debug)]
pub struct ScrollWheelEvent {
    pub position: Point<Pixels>,
    /// Lines: positive scrolls the content up (wheel away from the user).
    pub delta: f32,
    pub modifiers: Modifiers,
}

/// A key: `key` is the keysym (`"a"`, `"Return"`, `"Left"` ...).
#[derive(Clone, Debug)]
pub struct Keystroke {
    pub key: String,
    pub key_char: Option<String>,
    pub modifiers: Modifiers,
}

#[derive(Clone, Debug)]
pub struct KeyDownEvent {
    pub keystroke: Keystroke,
}

#[derive(Clone, Debug)]
pub struct KeyUpEvent {
    pub keystroke: Keystroke,
}

/// A form control's value (`input`, `select`, `textarea` ...).
#[derive(Clone, Debug, Default)]
pub struct ChangeEvent {
    /// The value as text (what `.value` holds in the DOM).
    pub value: String,
    /// Checkboxes and radio buttons.
    pub checked: bool,
    /// Numbers, ranges and selects (the selected index).
    pub number: f64,
}
