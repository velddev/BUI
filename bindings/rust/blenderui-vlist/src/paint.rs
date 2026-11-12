//! Painting a list: only the placed items, straight into the shared
//! [`PaintSink`] (no element per item), clipped to the viewport, then the
//! scrollbar. What an item looks like is the [`ItemPainter`]'s business
//! (chat: [`crate::chat`]); what is painted is decided by the frame
//! ([`crate::ListView::layout`]). Wrapped lines come from the layout cache:
//! scrolling only moves them, so a steady frame is cache hits and sink calls.
//!
//! Damage regions (repainting only the strip a scroll exposed) need engine
//! support and are not done in the adapters yet (see the docs).

use blenderui_paint::{PaintSink, Palette, Rect};

use crate::measure::Measurer;
use crate::scrollbar::Thumb;
use crate::source::Record;
use crate::view::ListView;
use crate::wrap::Lines;

/// Draws one item.
pub trait ItemPainter {
    /// The theme colors (background and scrollbar are painted with them).
    fn palette(&self) -> &Palette;
    /// A measured item in `r` (viewport coordinates).
    fn item(&self, sink: &mut dyn PaintSink, r: Rect, rec: &Record<'_>, lines: &Lines);
    /// An item not measured or not loaded: a placeholder in `r`.
    fn skeleton(&self, sink: &mut dyn PaintSink, r: Rect, rec: Option<&Record<'_>>);
}

impl ListView {
    /// Paint the frame laid out by [`ListView::layout`].
    pub fn paint<S: Measurer + ItemPainter>(&self, style: &S, sink: &mut dyn PaintSink) {
        let pal = *style.palette();
        let (w, h) = (self.width, self.height);
        sink.fill(Rect::new(0.0, 0.0, w, h), pal.back);
        let thumb = {
            let (top, total) = self.scroll_extent();
            Thumb::new(top, total, f64::from(h), h)
        };
        let (placed, page, text) = (&self.placed, &self.page, &self.text);
        sink.clipped(Rect::new(0.0, 0.0, w, h), &mut |s: &mut dyn PaintSink| {
            for p in placed {
                let r = Rect::new(0.0, p.y, w, p.h);
                let rec = page.record(p.item);
                match rec {
                    Some(rec) if p.exact && rec.flags & crate::source::FLAG_PLACEHOLDER == 0 => {
                        // the frame cached these lines; a miss (evicted meanwhile) shows a skeleton
                        match text.peek(&rec, style, w) {
                            Some(lines) => style.item(s, r, &rec, lines),
                            None => style.skeleton(s, r, Some(&rec)),
                        }
                    }
                    other => style.skeleton(s, r, other.as_ref()),
                }
            }
            if let Some(t) = thumb {
                t.paint(s, &pal, w, h, self.bar);
            }
        });
    }
}
