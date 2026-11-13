//! [`uniform_list`]: gpui's virtualized list of equal-height items.
//!
//! Only the items in view (plus a little overscan) become elements: the
//! visible range comes from the same math Blender's spreadsheet uses
//! ([`visible_rows`], `get_visible_rows` in `spreadsheet_draw.cc`). The
//! container scrolls like any `overflow: scroll` element, its offset kept
//! per element id, and a spacer as tall as every item together gives the
//! scrollbar the full extent. The cost of a frame is O(visible items).
//!
//! Heights are `f32` layout pixels: past about 16 million pixels of content
//! (a million 24 px rows) the scroll offset loses sub-pixel precision. The
//! [`DataTable`](crate::table::DataTable) keeps its own `f64` offset and has
//! no such limit.

use std::ops::Range;

use crate::css::Decl;
use crate::element::{AnyElement, Div, ElementId, IntoElement, ParentElement};
use crate::html::div;
use blenderui_table::{clamp_scroll, visible_rows};
use crate::{px, App, Styled, Window};

type ItemFn = Box<dyn Fn(Range<usize>, &mut Window, &mut App) -> Vec<AnyElement>>;

/// Items drawn beyond the edges, so a fast scroll doesn't show gaps.
const OVERSCAN: usize = 2;

/// A virtualized list (gpui's `uniform_list(id, item_count, |range, window, cx| ...)`).
///
/// ```ignore
/// uniform_list("rows", 1_000_000, |range, _window, _cx| {
///     range.map(|i| div().child(format!("Row {i}"))).collect::<Vec<_>>()
/// })
/// .h(px(300.0))
/// ```
pub fn uniform_list<R: IntoElement>(
    id: impl Into<ElementId>,
    item_count: usize,
    f: impl Fn(Range<usize>, &mut Window, &mut App) -> Vec<R> + 'static,
) -> UniformList {
    let id = id.into();
    UniformList {
        base: div().id(id.clone()).overflow_y_scroll(),
        id,
        count: item_count,
        item_h: None,
        f: Box::new(move |r, w, cx| f(r, w, cx).into_iter().map(IntoElement::into_any_element).collect()),
    }
}

/// See [`uniform_list`].
pub struct UniformList {
    base: Div,
    id: ElementId,
    count: usize,
    item_h: Option<f32>,
    f: ItemFn,
}

impl UniformList {
    /// Every item's height. Defaults to Blender's `UI_UNIT_Y` (one widget
    /// row at the current UI scale).
    pub fn item_height(mut self, h: impl Into<crate::Pixels>) -> Self {
        self.item_h = Some(h.into().0);
        self
    }
}

impl Styled for UniformList {
    fn decls_mut(&mut self) -> &mut Vec<Decl> {
        self.base.decls_mut()
    }
}

impl IntoElement for UniformList {
    fn into_any_element(self) -> AnyElement {
        let UniformList { base, id, count, item_h, f } = self;
        AnyElement::Element(base.defer(move |window, cx, el| {
            let item_h = item_h.unwrap_or_else(|| window.ui_unit()).max(1.0);
            // rows for its height: laid out elsewhere this frame, the frame is laid out again
            let bounds = window.own_bounds(&id);
            let view_h = bounds.map_or_else(|| window.viewport_size().height.0, |b| b.size.height);
            let scroll = clamp_scroll(
                f64::from(window.scroll_offset(&id).1), count, f64::from(item_h), f64::from(view_h));
            let r = visible_rows(scroll, f64::from(item_h), f64::from(view_h), count);
            let range = r.start.saturating_sub(OVERSCAN)..(r.end + OVERSCAN).min(count);
            let items = if range.is_empty() { Vec::new() } else { f(range.clone(), window, cx) };
            let mut at = div().absolute().left(px(0.0)).right(px(0.0)).top(px(range.start as f32 * item_h));
            for it in items {
                at = at.child(div().h(px(item_h)).overflow_hidden().child(it));
            }
            let spacer = div().relative().w_full().flex_none().h(px(count as f32 * item_h)).child(at);
            el.children.push(AnyElement::Element(spacer));
        }))
    }
}
