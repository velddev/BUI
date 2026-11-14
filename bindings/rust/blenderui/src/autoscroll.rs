//! Browser-style middle-button autoscroll.
//!
//! A middle press over something that scrolls (a scroll container, or an element that hears the
//! wheel, as a virtual list does) anchors there, unless something under it listens for the
//! middle button's press itself (`on_mouse_down(MouseButton::Middle, ..)`: an image viewer's
//! pan), which then hears it as any press. While the button is held, the pointer's distance
//! from the anchor scrolls it, faster the further it is, every frame; the release stops it. A
//! press and release without leaving the anchor's dead zone makes it sticky: moving the pointer
//! scrolls until any click, Escape or the wheel. The window captures the pointer meanwhile
//! (`BK_capture_pointer`: over a text box too) and shows the all-scroll cursor.
//!
//! Frames come only while it moves: inside the dead zone, or with nothing left to scroll (the top
//! or the bottom), none are asked for until the pointer moves again.

use crate::layout::Key;

/// Pixels around the anchor that scroll nothing.
pub const DEAD_ZONE: f32 = 12.0;

/// The fastest scroll, px per second.
pub const MAX_SPEED: f32 = 6000.0;

/// The longest step a frame takes, in seconds (a stalled frame doesn't jump).
pub const MAX_STEP: f64 = 0.1;

/// Scroll speed (px per second, positive: down the content) for the pointer `dy` px below the
/// anchor: none in the dead zone, then about 7 px/s per px, a little faster further out.
#[must_use]
pub fn speed(dy: f32) -> f32 {
    let past = dy.abs() - DEAD_ZONE;
    if past <= 0.0 {
        return 0.0;
    }
    (past * 7.0 + past * past * 0.04).min(MAX_SPEED) * dy.signum()
}

/// An autoscroll in progress.
#[derive(Clone, Debug)]
pub(crate) struct Autoscroll {
    /// Where the middle button went down (window logical px).
    pub origin: (f32, f32),
    /// The elements under it then, outermost first: what scrolls.
    pub path: Vec<Key>,
    /// Released without leaving the dead zone: it goes on until a click, Escape or the wheel.
    pub sticky: bool,
    /// The pointer has left the dead zone since the press.
    pub moved: bool,
    /// The clock time of the last step, while moving (`None`: still).
    pub last: Option<f64>,
}

impl Autoscroll {
    pub fn new(origin: (f32, f32), path: Vec<Key>) -> Self {
        Autoscroll { origin, path, sticky: false, moved: false, last: None }
    }

    /// The pointer is at `p`: whether it is out of the dead zone (it notes it).
    pub fn pointer(&mut self, p: (f32, f32)) -> bool {
        let out = (p.0 - self.origin.0).hypot(p.1 - self.origin.1) > DEAD_ZONE;
        self.moved |= out;
        out
    }

    /// How far to scroll at clock time `now` with the pointer at `p` (px, positive: down), or
    /// `None` when still (no frame wanted). The first step after stillness only starts the
    /// clock: `Some(0.0)`.
    pub fn step(&mut self, now: f64, p: (f32, f32)) -> Option<f32> {
        let v = speed(p.1 - self.origin.1);
        if v == 0.0 {
            self.last = None;
            return None;
        }
        let Some(last) = self.last.replace(now) else { return Some(0.0) };
        let dt = (now - last).clamp(0.0, MAX_STEP);
        Some(v * dt as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dead_zone_scrolls_nothing_then_faster_further_out() {
        assert_eq!(speed(0.0), 0.0);
        assert_eq!(speed(DEAD_ZONE), 0.0);
        assert_eq!(speed(-DEAD_ZONE), 0.0);
        assert!(speed(40.0) > 0.0 && speed(-40.0) < 0.0);
        assert_eq!(speed(-40.0), -speed(40.0));
        assert!(speed(200.0) > 2.0 * speed(100.0) - 1.0);
        assert_eq!(speed(10_000.0), MAX_SPEED);
    }

    #[test]
    fn steps_follow_the_clock_and_stop_when_still() {
        let mut a = Autoscroll::new((100.0, 100.0), Vec::new());
        assert!(!a.pointer((105.0, 105.0)));
        assert_eq!(a.step(1.0, (105.0, 105.0)), None);
        assert!(!a.moved);
        assert!(a.pointer((100.0, 200.0)));
        assert!(a.moved);
        // the first step starts the clock, the next moves by speed x time
        assert_eq!(a.step(2.0, (100.0, 200.0)), Some(0.0));
        let d = a.step(2.05, (100.0, 200.0)).unwrap();
        assert!((d - speed(100.0) * 0.05).abs() < 0.01, "{d}");
        // a long stall moves at most MAX_STEP's worth
        let d = a.step(10.0, (100.0, 200.0)).unwrap();
        assert!((d - speed(100.0) * MAX_STEP as f32).abs() < 0.01, "{d}");
        // back in the dead zone: still
        assert_eq!(a.step(10.1, (100.0, 101.0)), None);
        assert_eq!(a.last, None);
        // up is negative
        assert_eq!(a.step(11.0, (100.0, 0.0)), Some(0.0));
        assert!(a.step(11.1, (100.0, 0.0)).unwrap() < 0.0);
    }
}
