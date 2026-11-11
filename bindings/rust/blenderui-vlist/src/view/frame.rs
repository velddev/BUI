//! One frame: from the position to the list of items to paint.
//!
//! 1. resolve the anchor (a goal being chased, the end, or the stored anchor);
//! 2. walk the index from it, placing items until the viewport is full;
//! 3. measure the placed items nobody has measured, within the budget, and
//!    write the results back in one batch ([`HeightIndex::set_heights`]);
//!    when a height changed, place again (the anchor item did not move, so
//!    only what is below it can shift);
//! 4. at the end of the list with room left, move up so the list ends at the
//!    bottom edge (or pin to it when sticking);
//! 5. ask the source to load what is on screen, unless the user is flinging.
//!
//! A placed item that could not be measured in budget keeps its estimated
//! height and is flagged inexact: the painter draws a skeleton for it.
//! Everything here reuses the view's buffers: a steady frame allocates nothing.

use std::ops::Range;
use std::time::{Duration, Instant};

use super::{ListView, Pos};
use crate::fx::units_to_px;
use crate::measure::{FontWidth, Measurer, TextWidth};
use crate::source::{ItemSource, FLAG_PLACEHOLDER};

/// How much measuring a frame may do.
#[derive(Copy, Clone, Debug)]
pub enum Budget {
    Unlimited,
    /// At most this many items.
    Items(usize),
    /// Until this much time has passed.
    Time(Duration),
}

/// A running budget.
struct Clock {
    budget: Budget,
    start: Instant,
    done: usize,
}

impl Clock {
    fn new(budget: Budget) -> Self {
        Self { budget, start: Instant::now(), done: 0 }
    }

    fn exhausted(&self) -> bool {
        match self.budget {
            Budget::Unlimited => false,
            Budget::Items(n) => self.done >= n,
            // at least one item per frame, so a slow measurer still progresses
            Budget::Time(t) => self.done > 0 && self.start.elapsed() >= t,
        }
    }
}

/// An item placed in the viewport.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Placed {
    pub item: usize,
    /// Top edge relative to the viewport top (negative: partly above).
    pub y: f32,
    pub h: f32,
    /// Measured (false: an estimate, painted as a skeleton).
    pub exact: bool,
}

impl ListView {
    fn budget(&self) -> Budget {
        if self.flinging() { self.opts.fling_budget } else { Budget::Unlimited }
    }

    /// The anchor for this pass: the goal, the end, or the stored one, clamped to the list.
    fn resolve(&mut self) -> (usize, u64) {
        if let Some((item, align)) = self.goal {
            self.pos = self.goal_pos(item, align);
        }
        if self.pos == Pos::Bottom
            && let Pos::Anchor { item, off } = self.bottom_anchor()
        {
            return (item, off);
        }
        let Pos::Anchor { item, off } = self.pos else { return (0, 0) };
        let item = item.min(self.index.len().saturating_sub(1));
        let h = self.index.height_of(item).map_or(0, |(h, _)| u64::from(h.units()));
        let off = off.min(h);
        self.pos = Pos::Anchor { item, off };
        (item, off)
    }

    /// Fill `placed` from the anchor.
    fn place(&mut self, item: usize, off: u64) {
        self.placed.clear();
        let view = f64::from(self.height);
        let mut y = 0.0 - units_to_px(off);
        let placed = &mut self.placed;
        self.index.walk_forward(item, |i, h, exact| {
            placed.push(super::Placed { item: i, y: y as f32, h: h.px() as f32, exact });
            y += h.px();
            y < view
        });
    }

    /// Whether the last item is placed and its bottom is above the viewport bottom (with more content than the viewport).
    fn ends_short(&self) -> bool {
        let view = f64::from(self.height);
        self.placed.last().is_some_and(|p| p.item + 1 == self.index.len() && f64::from(p.y + p.h) < view - 0.01) && self.scroll_extent().1 > view
    }

    /// Make `self.page` cover `range` (refetching when the source changed).
    fn ensure_page<S: ItemSource>(&mut self, src: &S, range: &Range<usize>) {
        let have = self.page.range();
        if self.page_version == src.version() && have.start <= range.start && have.end >= range.end {
            return;
        }
        let margin = range.len().max(16);
        let start = range.start.saturating_sub(margin / 2);
        let end = range.end.saturating_add(margin).min(src.len());
        src.fetch(start..end, &mut self.page);
        self.page_version = src.version();
    }

    /// Measure the placed items that are not exact, within `clock`. Returns
    /// whether a height was written (the placement must be redone).
    fn measure_placed<S: ItemSource, M: Measurer>(&mut self, src: &S, m: &M, tw: FontWidth<'_>, clock: &mut Clock) -> bool {
        let (Some(a), Some(b)) = (self.placed.first(), self.placed.last()) else { return false };
        let range = a.item..b.item + 1;
        self.ensure_page(src, &range);
        self.updates.clear();
        self.samples.clear();
        for p in &self.placed {
            if p.exact {
                continue;
            }
            if clock.exhausted() {
                self.more_to_measure = true;
                break;
            }
            let Some(rec) = self.page.record(p.item) else { continue };
            if rec.flags & FLAG_PLACEHOLDER != 0 {
                continue;
            }
            let h = self.text.height_fonts(&rec, m, self.width, tw);
            self.updates.push((p.item, h));
            self.samples.push((rec.text.len() as u64, h.px()));
            clock.done += 1;
        }
        self.commit_measured()
    }

    /// Write `updates` and learn from `samples`. Returns whether there were any.
    pub(crate) fn commit_measured(&mut self) -> bool {
        if self.updates.is_empty() {
            return false;
        }
        self.index.set_heights(&mut self.updates);
        for &(bytes, px) in &self.samples {
            self.index.observe(bytes, px);
        }
        true
    }

    /// Lay out the frame and return the items to paint (viewport-relative).
    /// `tw` measures text widths (the host's shaper).
    pub fn layout<S: ItemSource, M: Measurer>(&mut self, src: &S, m: &M, tw: TextWidth<'_>) -> &[Placed] {
        self.layout_fonts(src, m, &mut |s, size, _| tw(s, size))
    }

    /// [`ListView::layout`] measuring each face of the text in its font
    /// ([`Measurer::fonts`]).
    pub fn layout_fonts<S: ItemSource, M: Measurer>(&mut self, src: &S, m: &M, tw: FontWidth<'_>) -> &[Placed] {
        self.text.next_generation();
        self.text.ensure_capacity(self.placed.len() * 4);
        if src.len() != self.index.len() {
            self.resync(src, m);
        }
        self.more_to_measure = false;
        let mut clock = Clock::new(self.budget());
        for pass in 0..6 {
            let (item, off) = self.resolve();
            self.place(item, off);
            let changed = self.measure_placed(src, m, tw, &mut clock);
            if changed {
                if !clock.exhausted() && pass < 5 {
                    continue;
                }
                // out of budget or passes: place once more with what was learned
                let (item, off) = self.resolve();
                self.place(item, off);
            }
            if self.ends_short() && matches!(self.pos, Pos::Anchor { .. }) && pass < 5 {
                // the list ended above the bottom edge: slide down to it
                self.pos = if self.opts.stick_to_bottom { Pos::Bottom } else { self.bottom_anchor() };
                continue;
            }
            break;
        }
        if let (Some(a), Some(b)) = (self.placed.first(), self.placed.last()) {
            let range = a.item..b.item + 1;
            self.ensure_page(src, &range);
            self.request_loads(src, &range);
        }
        if let Some((item, _)) = self.goal
            && (self.placed.iter().any(|p| p.item == item && p.exact) || self.index.height_of(item).is_some_and(|(_, e)| e))
        {
            self.goal = None;
        }
        // cache the wrapped lines of what is shown: painting is read-only and only looks them up
        for p in &self.placed {
            if let Some(rec) = self.page.record(p.item).filter(|r| p.exact && r.flags & FLAG_PLACEHOLDER == 0) {
                self.text.lines_fonts(&rec, m, self.width, tw);
            }
        }
        self.fling *= 0.7;
        self.frames = self.frames.wrapping_add(1);
        if self.frames.is_multiple_of(32) {
            let center = self.placed.first().map_or(0, |p| p.item);
            self.index.trim(center, self.opts.max_measured);
        }
        &self.placed
    }

    /// Ask the source for what is on screen (and a little ahead), unless the
    /// user is flinging; cancel what we asked for before and no longer need.
    fn request_loads<S: ItemSource>(&mut self, src: &S, shown: &Range<usize>) {
        let page = self.page.range();
        let wants = shown.clone().any(|i| self.page.record(i).is_some_and(|r| r.flags & FLAG_PLACEHOLDER != 0));
        if !wants || self.flinging() {
            return;
        }
        let ahead = shown.len().max(self.opts.overscan);
        let want = if self.last_dir >= 0 { shown.start..(shown.end + ahead).min(page.end) } else { shown.start.saturating_sub(ahead).max(page.start)..shown.end };
        if self.requested.as_ref() != Some(&want) {
            if let Some(old) = self.requested.replace(want.clone()) {
                src.cancel(old);
            }
            src.ensure_loaded(want);
        }
    }

    /// The source's items changed without the view being told: rebuild the
    /// index (measured heights are lost), keeping the anchor item.
    pub(crate) fn resync<S: ItemSource, M: Measurer>(&mut self, src: &S, m: &M) {
        let est = super::estimator_for(m, &self.opts, self.width);
        if let Ok(index) = crate::index::HeightIndex::estimated(src.len(), est, |r| src.bytes(r)) {
            self.index = index;
        }
        self.page_version = u64::MAX;
        if let Pos::Anchor { item, .. } = &mut self.pos {
            *item = (*item).min(src.len().saturating_sub(1));
        }
    }

    /// Idle work: measure the items beyond the viewport in the scroll
    /// direction, so a flick finds them measured. Returns whether more
    /// remains (ask for another frame while true).
    pub fn prefetch<S: ItemSource, M: Measurer>(&mut self, src: &S, m: &M, tw: TextWidth<'_>, budget: Budget) -> bool {
        self.prefetch_fonts(src, m, &mut |s, size, _| tw(s, size), budget)
    }

    /// [`ListView::prefetch`] measuring each face of the text in its font.
    pub fn prefetch_fonts<S: ItemSource, M: Measurer>(&mut self, src: &S, m: &M, tw: FontWidth<'_>, budget: Budget) -> bool {
        let (Some(a), Some(b)) = (self.placed.first(), self.placed.last()) else { return false };
        let n = (self.placed.len() * 3).max(48);
        let (first, last) = (a.item, b.item);
        let range = if self.last_dir >= 0 { last + 1..(last + 1 + n).min(self.index.len()) } else { first.saturating_sub(n)..first };
        if range.is_empty() {
            return false;
        }
        let mut todo: Vec<usize> = Vec::new();
        self.index.walk_forward(range.start, |i, _, exact| {
            if i >= range.end {
                return false;
            }
            if !exact {
                todo.push(i);
            }
            true
        });
        if todo.is_empty() {
            return false;
        }
        src.fetch(range, &mut self.ahead);
        let mut clock = Clock::new(budget);
        self.updates.clear();
        self.samples.clear();
        let mut left = false;
        for &i in &todo {
            if clock.exhausted() {
                left = true;
                break;
            }
            let Some(rec) = self.ahead.record(i) else { continue };
            if rec.flags & FLAG_PLACEHOLDER != 0 {
                continue;
            }
            let h = self.text.height_fonts(&rec, m, self.width, tw);
            self.updates.push((i, h));
            self.samples.push((rec.text.len() as u64, h.px()));
            clock.done += 1;
        }
        self.commit_measured();
        self.more_to_measure = left;
        left
    }

    /// The anchor item and its offset in px, for tests and adapters.
    #[must_use]
    pub fn anchor_px(&self) -> Option<(usize, f64)> {
        match self.pos {
            Pos::Anchor { item, off } => Some((item, units_to_px(off))),
            Pos::Bottom => None,
        }
    }

}
