//! Overlay scrollbars: a thin rounded thumb floating over a scroll area's
//! edge, no track and no layout width, shown only while it is wanted.
//!
//! A thumb shows while the user scrolls (the wheel, keys, a drag), and while
//! the pointer is near the area's right edge; [`HOLD`] after the last of
//! those it fades out over [`FADE`] (with reduced motion it just hides). It
//! is hidden while the content fits. Over the lane at the edge, or while
//! dragged, it is hot: wider and more opaque (the look is
//! [`blenderui_vlist::scrollbar`]'s, which a virtual list paints too).
//!
//! A window keeps one [`Bars`]: scroll containers and virtual lists poke it,
//! the painters read each area's [`Overlay`] from it, and after a frame it
//! says when the window must draw again: one timer at the end of a hold and
//! frames only while a thumb fades, so an idle window draws nothing.

use std::cell::RefCell;
use std::collections::HashMap;

pub use blenderui_vlist::scrollbar::{Overlay, ALPHA, HOT_ALPHA, HOT_THUMB_W, INSET, THUMB_W, WIDTH as LANE};

use crate::layout::Key;

/// How long a thumb stays after the last scroll, s.
pub const HOLD: f64 = 0.8;
/// How long it takes to fade out, s.
pub const FADE: f64 = 0.2;
/// How near the right edge the pointer shows the thumb, px.
pub const NEAR: f32 = 16.0;

/// When the window must draw again for its thumbs.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum Wake {
    /// Nothing is showing (or the pointer holds it): never.
    Idle,
    /// A thumb is fading: the next frame.
    Frame,
    /// After this many seconds (a hold ends).
    After(f64),
}

/// See the module docs.
#[derive(Default)]
pub(crate) struct Bars {
    /// Each area's last scroll (clock seconds).
    shown: HashMap<Key, f64>,
    /// The area whose edge the pointer is near, and whether it's over the lane.
    pub(crate) hover: Option<(Key, bool)>,
    /// The scroll container whose thumb is dragged, and where it was grabbed
    /// (px from the thumb's top).
    pub(crate) drag: Option<(Key, f32)>,
    /// The virtual lists laid out last frame, and whether each overflows.
    pub(crate) lists: HashMap<Key, bool>,
}

impl Bars {
    /// The user scrolled area `key` at `now`: show its thumb.
    pub(crate) fn poke(&mut self, key: Key, now: f64) {
        self.shown.insert(key, now);
    }

    /// The pointer is near the edge of `hover` (or none): returns whether
    /// that changed. The area it leaves keeps its thumb a hold longer.
    pub(crate) fn set_hover(&mut self, hover: Option<(Key, bool)>, now: f64) -> bool {
        if hover == self.hover {
            return false;
        }
        if let Some((old, _)) = self.hover {
            self.poke(old, now);
        }
        self.hover = hover;
        true
    }

    /// How area `key`'s thumb shows at `now`.
    pub(crate) fn look(&self, key: Key, now: f64, reduced: bool) -> Overlay {
        let dragged = self.drag.is_some_and(|(k, _)| k == key);
        let hover = self.hover.filter(|&(k, _)| k == key);
        if dragged || hover.is_some() {
            return Overlay { alpha: 1.0, hot: dragged || hover.is_some_and(|(_, lane)| lane) };
        }
        let alpha = self.shown.get(&key).map_or(0.0, |&at| fade(now - at, reduced));
        Overlay { alpha, hot: false }
    }

    /// When the window must draw again; forgets the thumbs that are gone.
    pub(crate) fn wake(&mut self, now: f64, reduced: bool) -> Wake {
        let (hover, drag) = (self.hover.map(|h| h.0), self.drag.map(|d| d.0));
        self.shown.retain(|&k, &mut at| Some(k) == hover || Some(k) == drag || fade(now - at, reduced) > 0.0);
        let mut wake = Wake::Idle;
        for (&k, &at) in &self.shown {
            if Some(k) == hover || Some(k) == drag {
                continue;
            }
            let t = now - at;
            if t < HOLD {
                wake = match wake {
                    Wake::Frame => Wake::Frame,
                    Wake::After(s) => Wake::After(s.min(HOLD - t)),
                    Wake::Idle => Wake::After(HOLD - t),
                };
            } else {
                wake = Wake::Frame;
            }
        }
        wake
    }
}

/// A thumb's opacity `t` seconds after the last scroll.
fn fade(t: f64, reduced: bool) -> f32 {
    if t < HOLD {
        1.0
    } else if reduced || t >= HOLD + FADE {
        0.0
    } else {
        // ease-out, as the stylesheet's transitions
        let x = ((t - HOLD) / FADE) as f32;
        1.0 - x * (2.0 - x)
    }
}

thread_local! {
    /// The looks of the frame being painted ([`with_looks`]).
    static LOOKS: RefCell<HashMap<Key, Overlay>> = RefCell::new(HashMap::new());
}

/// Paint with each area's look at hand ([`look_of`]).
pub(crate) fn with_looks<R>(bars: &Bars, now: f64, reduced: bool, f: impl FnOnce() -> R) -> R {
    LOOKS.with(|l| {
        let mut l = l.borrow_mut();
        l.clear();
        let keys = bars.shown.keys().copied().chain(bars.hover.map(|h| h.0)).chain(bars.drag.map(|d| d.0));
        for k in keys {
            l.insert(k, bars.look(k, now, reduced));
        }
    });
    f()
}

/// Area `key`'s look in the frame being painted (hidden if it has none).
pub(crate) fn look_of(key: Key) -> Overlay {
    LOOKS.with(|l| l.borrow().get(&key).copied()).unwrap_or(Overlay { alpha: 0.0, hot: false })
}

/// The vertical thumb of a scroll container showing `view` px of
/// `view + range`, scrolled `pos`: its top and height along the track.
pub(crate) fn thumb(view: f32, range: f32, pos: f32) -> Option<(f32, f32)> {
    if range <= 0.0 || view <= 0.0 {
        return None;
    }
    let h = (view * view / (view + range)).max(blenderui_vlist::scrollbar::MIN_THUMB.min(view));
    Some(((view - h) * (pos / range).clamp(0.0, 1.0), h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thumb_holds_then_fades() {
        let mut b = Bars::default();
        assert_eq!(b.wake(0.0, false), Wake::Idle, "nothing shown: no frames");
        b.poke(7, 1.0);
        assert_eq!(b.look(7, 1.5, false).alpha, 1.0);
        assert_eq!(b.wake(1.5, false), Wake::After(HOLD - 0.5));
        let mid = b.look(7, 1.0 + HOLD + FADE / 2.0, false).alpha;
        assert!(mid > 0.0 && mid < 1.0, "{mid}");
        assert_eq!(b.wake(1.0 + HOLD + 0.01, false), Wake::Frame);
        assert_eq!(b.look(7, 1.0 + HOLD + 0.01, true).alpha, 0.0, "reduced motion: no fade");
        assert_eq!(b.wake(1.0 + HOLD + FADE, false), Wake::Idle);
        assert_eq!(b.look(7, 1.0 + HOLD + FADE, false).alpha, 0.0);
    }

    #[test]
    fn the_pointer_holds_it_and_the_lane_makes_it_hot() {
        let mut b = Bars::default();
        assert!(b.set_hover(Some((3, false)), 0.0));
        assert!(!b.set_hover(Some((3, false)), 0.5));
        assert_eq!(b.look(3, 10.0, false), Overlay { alpha: 1.0, hot: false });
        assert_eq!(b.wake(10.0, false), Wake::Idle, "a resting pointer costs no frames");
        b.set_hover(Some((3, true)), 10.0);
        assert!(b.look(3, 10.0, false).hot);
        b.set_hover(None, 11.0);
        assert_eq!(b.look(3, 11.5, false).alpha, 1.0, "it stays a hold after the pointer leaves");
        assert_eq!(b.look(3, 11.0 + HOLD + FADE, false).alpha, 0.0);
    }

    #[test]
    fn thumbs_are_proportional() {
        assert_eq!(thumb(100.0, 0.0, 0.0), None);
        let (y, h) = thumb(100.0, 100.0, 100.0).unwrap();
        assert_eq!(h, 50.0);
        assert_eq!(y + h, 100.0);
    }
}
