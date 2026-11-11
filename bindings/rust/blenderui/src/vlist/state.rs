//! [`ListState`]: the entity around a [`List`]. It owns the core list, the
//! element's id and last bounds, and the frame clock; its `Deref` makes the
//! core's API (`state.view`, `state.source`, `state.push(..)` for a chat)
//! read as its own.

use std::ops::{Deref, DerefMut};
use std::time::Instant;

use blenderui_vlist::{Budget, ItemPainter, ItemSource, List, Measurer};

use crate::geometry::Bounds;
use crate::sink::{measure, with_sink};
use crate::{Context, SharedString, Window};

/// What a list shows: TanStack Virtual's `range`, `count`, `scrollOffset` and `getTotalSize()`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewInfo {
    /// The items in the viewport (empty before the first layout).
    pub range: std::ops::Range<usize>,
    pub count: usize,
    /// Px from the top of the content (estimates for what is not measured).
    pub scroll_offset: f64,
    /// The content height, px.
    pub total_size: f64,
}

/// The list.
pub struct ListState<S: ItemSource + 'static, P: Measurer + ItemPainter + 'static> {
    list: List<S, P>,
    /// Where the list was last laid out (window coordinates).
    pub(crate) bounds: Bounds<f32>,
    pub(crate) id: SharedString,
    last_frame: Option<Instant>,
    /// The range and count last reported by [`take_view_change`](Self::take_view_change).
    reported: Option<(std::ops::Range<usize>, usize)>,
}

impl<S: ItemSource, P: Measurer + ItemPainter> Deref for ListState<S, P> {
    type Target = List<S, P>;
    fn deref(&self) -> &Self::Target {
        &self.list
    }
}

impl<S: ItemSource, P: Measurer + ItemPainter> DerefMut for ListState<S, P> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.list
    }
}

impl<S: ItemSource, P: Measurer + ItemPainter> ListState<S, P> {
    /// An entity state for `list` (call inside `cx.new`).
    pub fn new(list: List<S, P>, cx: &mut Context<Self>) -> Self {
        let id = SharedString::from(format!("bui-vlist-{}", cx.entity().entity_id()));
        Self { list, bounds: Bounds::default(), id, last_frame: None, reported: None }
    }

    /// Called while rendering with the list's last bounds: resize, advance
    /// the smooth scroll, lay the frame out and do idle measuring. Returns
    /// whether another frame is wanted (easing, or measuring left to do).
    pub(crate) fn sync(&mut self, bounds: Bounds<f32>, scale: f32, window: &mut Window) -> bool {
        self.bounds = bounds;
        let (w, h) = (bounds.size.width, bounds.size.height);
        if w < 1.0 || h < 1.0 {
            return false;
        }
        if (self.list.view.viewport().0 - w).abs() > f32::EPSILON || (self.list.view.viewport().1 - h).abs() > f32::EPSILON {
            self.list.resize(w, h);
        }
        let now = Instant::now();
        let dt = self.last_frame.replace(now).map_or(1.0 / 60.0, |t| now.duration_since(t).as_secs_f64().min(0.1));
        let mut more = self.list.view.tick(dt);
        let mut tw = |s: &str, z: f32, f| measure(s, z, f, scale);
        self.list.layout_fonts(&mut tw);
        if !self.list.view.flinging() {
            more |= self.list.prefetch_fonts(&mut tw, Budget::Time(std::time::Duration::from_micros(800)));
        }
        if more {
            window.request_animation_frame();
        } else {
            // idle: the next frame starts a fresh clock
            self.last_frame = None;
        }
        more
    }

    /// What the list shows now.
    #[must_use]
    pub fn view_info(&self) -> ViewInfo {
        let (scroll_offset, total_size) = self.list.view.scroll_extent();
        ViewInfo { range: self.list.view.visible_range().unwrap_or(0..0), count: self.list.source.len(), scroll_offset, total_size }
    }

    /// The view info when the visible range or the count changed since the
    /// last call (TanStack Virtual's `onChange`), else `None`.
    pub fn take_view_change(&mut self) -> Option<ViewInfo> {
        let info = self.view_info();
        let key = (info.range.clone(), info.count);
        (self.reported.as_ref() != Some(&key)).then(|| {
            self.reported = Some(key);
            info
        })
    }

    /// Draw the laid-out frame into `p` (logical px, `b` = the element's box).
    pub(crate) fn draw(&self, b: Bounds<f32>, p: &mut crate::Painter, scale: f32) {
        with_sink(p, b.origin, scale, |sink| self.list.paint(sink));
    }
}
