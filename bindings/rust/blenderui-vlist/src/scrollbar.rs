//! The scrollbar: a thumb sized and placed from the *estimated* content
//! height, with a minimum size so it stays grabbable at 10M items, and the
//! drag mapping back to a fraction of the content. Dragging far picks an
//! anchor from the estimate ([`crate::ListView::scroll_to_fraction`]); as
//! the items around it are measured the estimate improves but the anchor
//! stays, so the content under the pointer does not fight the user (only
//! the thumb adjusts). The look is an overlay scrollbar's: a thin rounded
//! pill floating over the content's right edge, no track, as opaque as the
//! host's [`Overlay`] says (it shows it while the user scrolls and fades it
//! out after).

use blenderui_paint::{PaintSink, Palette, Rect};

/// The scrollbar's lane at the right edge, in px: where a press grabs the
/// thumb (the thumb itself is thinner, see [`THUMB_W`]).
pub const WIDTH: f32 = 10.0;
/// The smallest thumb.
pub const MIN_THUMB: f32 = 24.0;
/// The thumb's width at rest, px.
pub const THUMB_W: f32 = 5.0;
/// The thumb's width while the pointer is over the lane or drags it, px.
pub const HOT_THUMB_W: f32 = 8.0;
/// The thumb's gap to the edge (and to the viewport's top and bottom), px.
pub const INSET: f32 = 3.0;
/// The thumb's opacity, the text colour's, at rest.
pub const ALPHA: f32 = 0.25;
/// The thumb's opacity while hot.
pub const HOT_ALPHA: f32 = 0.40;

/// How an overlay scrollbar shows now: its opacity (0: hidden; it scales
/// [`ALPHA`]) and whether it is hot (the pointer over the lane, or a drag).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Overlay {
    pub alpha: f32,
    pub hot: bool,
}

impl Default for Overlay {
    /// Shown: a host that doesn't drive it gets a thumb always there.
    fn default() -> Self {
        Self { alpha: 1.0, hot: false }
    }
}

/// The overlay thumb's rect for a thumb at `y`, `h` tall in a track along
/// `x0..x1` (the scroll area's right edge at `x1`) that is `len` long.
#[must_use]
pub fn overlay_rect(x1: f32, y: f32, h: f32, len: f32, hot: bool) -> Rect {
    let w = if hot { HOT_THUMB_W } else { THUMB_W };
    let y0 = (y + INSET).min(len - INSET);
    let y1 = (y + h - INSET).max(y0 + 1.0).min(len - INSET).max(y0 + 1.0);
    Rect::new(x1 - INSET - w, y0, w, y1 - y0)
}

/// The thumb inside a track.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Thumb {
    pub y: f32,
    pub h: f32,
}

impl Thumb {
    /// The thumb for scroll position `top` of `total` px of content with a
    /// `view` px viewport in a `track` px track; `None` when everything fits.
    #[must_use]
    pub fn new(top: f64, total: f64, view: f64, track: f32) -> Option<Self> {
        if !(total > view && view > 0.0 && track > 0.0) {
            return None;
        }
        let h = ((f64::from(track) * view / total) as f32).clamp(MIN_THUMB.min(track), track);
        let free = f64::from(track - h);
        let y = (free * (top / (total - view)).clamp(0.0, 1.0)) as f32;
        Some(Self { y, h })
    }

    /// The content fraction for a thumb whose top is at `y` in the track.
    #[must_use]
    pub fn fraction(&self, y: f32, track: f32) -> f64 {
        let free = f64::from(track - self.h);
        if free <= 0.0 { 0.0 } else { (f64::from(y) / free).clamp(0.0, 1.0) }
    }

    /// Paint the thumb at the right edge of a `width` x `height` viewport,
    /// floating over the content (no track), looking as `look` says.
    pub fn paint(&self, sink: &mut dyn PaintSink, pal: &Palette, width: f32, height: f32, look: Overlay) {
        if look.alpha <= 0.0 {
            return;
        }
        let r = overlay_rect(width, self.y, self.h, height, look.hot);
        let a = if look.hot { HOT_ALPHA } else { ALPHA } * look.alpha.min(1.0);
        sink.round(r, r.w * 0.5, pal.text.opacity(a));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_is_proportional_and_bounded() {
        let t = Thumb::new(0.0, 1000.0, 100.0, 200.0).unwrap();
        assert_eq!((t.y, t.h), (0.0, 24.0));
        let end = Thumb::new(900.0, 1000.0, 100.0, 200.0).unwrap();
        assert!((end.y + end.h - 200.0).abs() < 0.01);
        // 10M items: the thumb stays grabbable
        let tiny = Thumb::new(0.0, 6e8, 600.0, 600.0).unwrap();
        assert_eq!(tiny.h, MIN_THUMB);
        assert!(Thumb::new(0.0, 50.0, 100.0, 200.0).is_none());
    }

    #[test]
    fn fraction_inverts_position() {
        let t = Thumb::new(450.0, 1000.0, 100.0, 200.0).unwrap();
        assert!((t.fraction(t.y, 200.0) - 0.5).abs() < 0.01);
        assert_eq!(t.fraction(-5.0, 200.0), 0.0);
        assert_eq!(t.fraction(1e9, 200.0), 1.0);
    }

    #[test]
    fn overlay_thumb_floats_inside_the_edge() {
        let t = Thumb::new(0.0, 1000.0, 100.0, 200.0).unwrap();
        let r = overlay_rect(300.0, t.y, t.h, 200.0, false);
        assert_eq!((r.x, r.y, r.w), (300.0 - INSET - THUMB_W, INSET, THUMB_W));
        let hot = overlay_rect(300.0, t.y, t.h, 200.0, true);
        assert_eq!(hot.x + hot.w, r.x + r.w, "it widens inwards");
        assert_eq!(hot.w, HOT_THUMB_W);
        let end = Thumb::new(900.0, 1000.0, 100.0, 200.0).unwrap();
        let r = overlay_rect(300.0, end.y, end.h, 200.0, false);
        assert!((r.y + r.h - (200.0 - INSET)).abs() < 0.01);
    }
}
