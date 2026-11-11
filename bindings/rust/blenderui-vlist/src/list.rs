//! [`List`]: the three things an adapter needs in one place: the items, the
//! style (how they measure and paint) and the [`ListView`]; plus the
//! pointer protocol for the scrollbar. An adapter owns one `List`, forwards
//! its events and implements `PaintSink`; it holds no list logic.

use blenderui_paint::PaintSink;

use crate::measure::{FontWidth, Measurer, TextWidth};
use crate::paint::ItemPainter;
use crate::scrollbar::{Thumb, WIDTH as SCROLL_W};
use crate::source::ItemSource;
use crate::view::{Budget, ListView, Nav, Options, Placed};
use crate::IndexError;

/// What a press landed on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Hit {
    /// The scrollbar (a drag may have started).
    Scrollbar,
    /// An item.
    Item(usize),
    /// Nothing.
    None,
}

impl Nav {
    /// The navigation a key name stands for (`"pageup"`, `"End"` ...; case-insensitive).
    #[must_use]
    pub fn from_key(name: &str) -> Option<Self> {
        const KEYS: [(&str, Nav); 10] = [
            ("up", Nav::LineUp),
            ("arrowup", Nav::LineUp),
            ("down", Nav::LineDown),
            ("arrowdown", Nav::LineDown),
            ("pageup", Nav::PageUp),
            ("prior", Nav::PageUp),
            ("pagedown", Nav::PageDown),
            ("next", Nav::PageDown),
            ("home", Nav::Home),
            ("end", Nav::End),
        ];
        KEYS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|&(_, k)| k)
    }
}

/// See the module docs.
pub struct List<S: ItemSource, P: Measurer + ItemPainter> {
    pub view: ListView,
    pub source: S,
    pub style: P,
    /// Distance from the thumb's top to where it was grabbed.
    drag: Option<f32>,
}

impl<S: ItemSource, P: Measurer + ItemPainter> List<S, P> {
    /// A list of `source` shown through `style` in a `width` x `height` viewport.
    pub fn new(source: S, style: P, width: f32, height: f32, opts: Options) -> Result<Self, IndexError> {
        let view = ListView::new(&source, &style, width, height, opts)?;
        Ok(Self { view, source, style, drag: None })
    }

    /// Lay out the frame (`tw`: the host's text width).
    pub fn layout(&mut self, tw: TextWidth<'_>) -> &[Placed] {
        self.view.layout(&self.source, &self.style, tw)
    }

    /// Lay out the frame measuring each face in its font (`tw`: the host's
    /// text width in a face).
    pub fn layout_fonts(&mut self, tw: FontWidth<'_>) -> &[Placed] {
        self.view.layout_fonts(&self.source, &self.style, tw)
    }

    /// Paint the laid-out frame.
    pub fn paint(&self, sink: &mut dyn PaintSink) {
        self.view.paint(&self.style, sink);
    }

    /// Resize the viewport (a new width re-wraps everything lazily).
    pub fn resize(&mut self, width: f32, height: f32) {
        self.view.resize(&self.style, width, height);
    }

    /// A theme or font change: new style, everything re-measured lazily.
    pub fn restyle(&mut self, style: P) {
        self.style = style;
        self.view.restyle(&self.style);
    }

    /// Idle measuring ahead of the scroll direction; true while more remains.
    pub fn prefetch(&mut self, tw: TextWidth<'_>, budget: Budget) -> bool {
        self.view.prefetch(&self.source, &self.style, tw, budget)
    }

    /// [`List::prefetch`] measuring each face in its font.
    pub fn prefetch_fonts(&mut self, tw: FontWidth<'_>, budget: Budget) -> bool {
        self.view.prefetch_fonts(&self.source, &self.style, tw, budget)
    }

    /// The scrollbar thumb, if the content is taller than the viewport.
    #[must_use]
    pub fn thumb(&self) -> Option<Thumb> {
        let (top, total) = self.view.scroll_extent();
        let (_, h) = self.view.viewport();
        Thumb::new(top, total, f64::from(h), h)
    }

    /// A key: navigation for a recognised name. Returns whether it was handled.
    pub fn key(&mut self, name: &str, line_px: f64) -> bool {
        Nav::from_key(name).map(|n| self.view.navigate(n, line_px)).is_some()
    }

    /// The primary button pressed at viewport position (`x`, `y`): grabs the
    /// thumb, pages towards a press on the track, or reports the item.
    pub fn pointer_down(&mut self, x: f32, y: f32) -> Hit {
        let (w, h) = self.view.viewport();
        if x >= w - SCROLL_W {
            let Some(t) = self.thumb() else { return Hit::None };
            if y >= t.y && y < t.y + t.h {
                self.drag = Some(y - t.y);
            } else {
                self.view.scroll_by(if y < t.y { -f64::from(h) } else { f64::from(h) });
            }
            return Hit::Scrollbar;
        }
        self.view.item_at(y).map_or(Hit::None, Hit::Item)
    }

    /// The pointer moved to `y`; returns whether the thumb is being dragged.
    pub fn pointer_move(&mut self, y: f32) -> bool {
        let (Some(grab), Some(t)) = (self.drag, self.thumb()) else { return false };
        let (_, h) = self.view.viewport();
        self.view.scroll_to_fraction(t.fraction(y - grab, h));
        true
    }

    /// The button was released.
    pub const fn pointer_up(&mut self) {
        self.drag = None;
    }

    /// Whether the thumb is being dragged.
    #[must_use]
    pub const fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}
