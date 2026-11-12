//! Tooltips: an element's `title` shown in a small box once the pointer has
//! rested on it for a while.
//!
//! The window tracks the nearest element with a `title` under the pointer.
//! The tooltip comes up [`Tips::delay`] after the pointer came onto it
//! ([`DELAY`] unless the app sets one, `Window::set_tooltip_delay`); moving
//! onto another titled element starts the wait again. It goes at once when
//! the pointer leaves the element, and a click or a key hushes it until the
//! pointer leaves. The app can turn tooltips off (`Window::set_tooltips`),
//! say while one of its menus is open.
//!
//! The tooltip is a `<bui-tooltip>` element added at the end of the
//! document, so stylesheets style it like any element (the UA sheet gives it
//! Blender's look and `z-index: 1000`). It hangs below the pointer where it
//! came up, kept inside the window, and above the pointer when there's no
//! room below.

use crate::element::{Div, ParentElement};
use crate::layout::Key;

/// How long the pointer rests on an element before its tooltip shows, s.
pub const DELAY: f64 = 1.0;
/// The tooltip's key (its element's `.key`).
pub(crate) const KEY: &str = "bui-tooltip";
/// How far below the pointer the tooltip hangs (under the arrow), px.
const BELOW: f32 = 20.0;
/// How far above the pointer it goes when flipped, and its distance from
/// the window's edges, px.
const MARGIN: f32 = 4.0;

/// See the module docs.
pub(crate) struct Tips {
    /// The wait before a tooltip shows, s.
    pub(crate) delay: f64,
    /// Whether the app lets tooltips show.
    pub(crate) on: bool,
    /// The titled element under the pointer, and when the pointer came onto it
    /// (clock seconds).
    over: Option<(Key, f64)>,
    /// A click or key hid the tooltip of `over` until the pointer leaves it.
    hushed: bool,
    /// Where the shown tooltip hangs from (the pointer when it came up).
    pub(crate) at: Option<(f32, f32)>,
}

impl Default for Tips {
    fn default() -> Self {
        Self { delay: DELAY, on: true, over: None, hushed: false, at: None }
    }
}

impl Tips {
    /// The pointer is on titled element `key` (or none) at `now`: true when
    /// that hid a shown tooltip.
    pub(crate) fn set_over(&mut self, key: Option<Key>, now: f64) -> bool {
        if self.over.map(|(k, _)| k) == key {
            return false;
        }
        self.over = key.map(|k| (k, now));
        self.hushed = false;
        self.at.take().is_some()
    }

    /// A click or key: hide the tooltip until the pointer leaves its element.
    /// True when one was showing.
    pub(crate) fn hush(&mut self) -> bool {
        self.hushed = self.over.is_some();
        self.at.take().is_some()
    }

    /// When the waiting tooltip is due (clock seconds), if one waits.
    pub(crate) fn due(&self) -> Option<f64> {
        let (_, since) = self.over?;
        (self.on && !self.hushed && self.at.is_none()).then_some(since + self.delay)
    }

    /// Whether the tooltip shows at `now`; the pointer at `pointer` places
    /// it when it comes up.
    pub(crate) fn update(&mut self, now: f64, pointer: (f32, f32)) -> bool {
        let show = self.on && !self.hushed
            && self.over.is_some_and(|(_, since)| now + crate::window::TIMER_SLOP >= since + self.delay);
        self.at = if show { self.at.or(Some(pointer)) } else { None };
        show
    }

    /// The titled element the tooltip is for.
    pub(crate) fn key(&self) -> Option<Key> {
        self.over.map(|(k, _)| k)
    }
}

/// The tooltip's element for `text`.
pub(crate) fn element(text: &str) -> Div {
    crate::html::custom("bui-tooltip").key(KEY).child(text.to_owned())
}

/// Where a tooltip of `size` hanging from pointer `at` goes in a `viewport`:
/// below the pointer, above it when there's no room below, inside the window.
pub(crate) fn place(at: (f32, f32), size: (f32, f32), viewport: (f32, f32)) -> (f32, f32) {
    let (w, h) = size;
    let (vw, vh) = viewport;
    let x = at.0.min(vw - MARGIN - w).max(MARGIN);
    let below = at.1 + BELOW;
    let y = if below + h <= vh - MARGIN { below } else { (at.1 - MARGIN - h).max(MARGIN) };
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_the_delay_then_shows() {
        let mut t = Tips { delay: 2.0, ..Tips::default() };
        assert!(!t.set_over(Some(7), 10.0));
        assert_eq!(t.due(), Some(12.0));
        assert!(!t.update(11.9, (5.0, 5.0)));
        assert!(t.update(12.0, (5.0, 5.0)));
        assert_eq!(t.at, Some((5.0, 5.0)));
        assert_eq!(t.due(), None);
        // the pointer moving on the same element keeps it where it came up
        assert!(!t.set_over(Some(7), 12.5));
        assert!(t.update(12.5, (9.0, 9.0)));
        assert_eq!(t.at, Some((5.0, 5.0)));
    }

    #[test]
    fn another_element_restarts_the_wait() {
        let mut t = Tips { delay: 2.0, ..Tips::default() };
        t.set_over(Some(1), 0.0);
        assert!(t.update(2.0, (0.0, 0.0)));
        assert!(t.set_over(Some(2), 3.0), "leaving hides it at once");
        assert!(!t.update(4.9, (0.0, 0.0)));
        assert!(t.update(5.0, (0.0, 0.0)));
        assert!(t.set_over(None, 6.0));
        assert!(!t.update(9.0, (0.0, 0.0)));
        assert_eq!(t.due(), None);
    }

    #[test]
    fn a_click_or_key_hushes_it_until_the_pointer_leaves() {
        let mut t = Tips { delay: 2.0, ..Tips::default() };
        t.set_over(Some(1), 0.0);
        assert!(t.update(2.0, (0.0, 0.0)));
        assert!(t.hush());
        assert!(!t.update(10.0, (0.0, 0.0)));
        assert_eq!(t.due(), None);
        t.set_over(None, 10.0);
        t.set_over(Some(1), 11.0);
        assert!(t.update(13.0, (0.0, 0.0)));
        // hushed before it came up: it never does on this visit
        t.set_over(Some(2), 14.0);
        assert!(!t.hush());
        assert!(!t.update(20.0, (0.0, 0.0)));
    }

    #[test]
    fn off_hides_it_and_waits_nothing() {
        let mut t = Tips { delay: 2.0, ..Tips::default() };
        t.set_over(Some(1), 0.0);
        assert!(t.update(2.0, (0.0, 0.0)));
        t.on = false;
        assert!(!t.update(2.1, (0.0, 0.0)));
        assert_eq!(t.due(), None);
        t.on = true;
        assert!(t.update(2.2, (0.0, 0.0)));
    }

    #[test]
    fn placed_below_the_pointer_inside_the_window() {
        let vp = (200.0, 100.0);
        assert_eq!(place((50.0, 10.0), (60.0, 20.0), vp), (50.0, 30.0));
        // no room on the right: pulled in
        assert_eq!(place((190.0, 10.0), (60.0, 20.0), vp), (136.0, 30.0));
        // no room below: above the pointer
        assert_eq!(place((50.0, 90.0), (60.0, 20.0), vp), (50.0, 66.0));
        // wider than the window: from the left edge
        assert_eq!(place((50.0, 10.0), (300.0, 20.0), vp).0, 4.0);
    }
}
