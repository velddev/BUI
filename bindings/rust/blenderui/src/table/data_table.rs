//! [`DataTable`]: the element that shows a [`TableState`] (gpui-component's
//! `DataTable::new(&state)`).
//!
//! It is one painted leaf (a canvas drawn from the state, see
//! [`super::draw`]) inside a wrapper that takes pointer and keyboard input,
//! plus a few invisible handles over the header borders that give the
//! column-resize cursor. A frame therefore builds a constant number of
//! elements however many rows there are, and paints O(visible cells).

use std::rc::Rc;

use blenderui_table::{ColumnSort, TableEvent};
use super::delegate::TableDelegate;
use super::state::TableState;
use blenderui_table::Size;
use crate::element::{canvas_with, Div};
use crate::html::div;
use crate::css::Decl;
use crate::{px, AnyElement, App, Entity, IntoElement, MouseButton, ParentElement, Pixels, Styled, Window};

type Listener = Box<dyn Fn(&TableEvent, &mut Window, &mut App)>;

/// A table element.
///
/// ```ignore
/// let table = cx.new(|cx| TableState::new(MyDelegate::new(), window, cx));
/// // in render:
/// div().size_full().child(DataTable::new(&table).stripe(true).on_select(|row, _, _| println!("{row}")))
/// ```
pub struct DataTable<D: TableDelegate> {
    state: Entity<TableState<D>>,
    base: Div,
    stripe: Option<bool>,
    bordered: Option<bool>,
    size: Option<Size>,
    header_h: Option<f32>,
    fixed_border: Option<bool>,
    scrollbars: Option<(bool, bool)>,
    listeners: Vec<Listener>,
}

impl<D: TableDelegate> DataTable<D> {
    pub fn new(state: &Entity<TableState<D>>) -> Self {
        Self {
            state: *state,
            base: div().relative().overflow_hidden().size_full(),
            stripe: None,
            bordered: None,
            size: None,
            header_h: None,
            fixed_border: None,
            scrollbars: None,
            listeners: Vec::new(),
        }
    }

    /// Tint every second row (Blender's `draw_alternating_row_overlay`).
    pub fn stripe(mut self, on: bool) -> Self {
        self.stripe = Some(on);
        self
    }
    pub fn bordered(mut self, on: bool) -> Self {
        self.bordered = Some(on);
        self
    }
    pub fn size(mut self, size: Size) -> Self {
        self.size = Some(size);
        self
    }
    pub fn header_row_height(mut self, h: impl Into<Pixels>) -> Self {
        self.header_h = Some(h.into().0);
        self
    }
    pub fn fixed_cols_border(mut self, on: bool) -> Self {
        self.fixed_border = Some(on);
        self
    }
    pub fn scrollbar_visible(mut self, vertical: bool, horizontal: bool) -> Self {
        self.scrollbars = Some((vertical, horizontal));
        self
    }

    /// Every table event.
    pub fn on_event(mut self, f: impl Fn(&TableEvent, &mut Window, &mut App) + 'static) -> Self {
        self.listeners.push(Box::new(f));
        self
    }
    /// A row was selected (delegate row index).
    pub fn on_select(self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_event(move |e, w, cx| {
            if let TableEvent::SelectRow(r) = e {
                f(*r, w, cx);
            }
        })
    }
    /// A row was double clicked.
    pub fn on_row_double_click(self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_event(move |e, w, cx| {
            if let TableEvent::DoubleClickedRow(r) = e {
                f(*r, w, cx);
            }
        })
    }
    /// A header click sorted a column (delegate column index).
    pub fn on_sort(self, f: impl Fn(usize, ColumnSort, &mut Window, &mut App) + 'static) -> Self {
        self.on_event(move |e, w, cx| {
            if let TableEvent::Sorted(c, s) = e {
                f(*c, *s, w, cx);
            }
        })
    }
    /// A column was resized (all widths, view order).
    pub fn on_column_resize(self, f: impl Fn(&[Pixels], &mut Window, &mut App) + 'static) -> Self {
        self.on_event(move |e, w, cx| {
            if let TableEvent::ColumnWidthsChanged(v) = e {
                let widths: Vec<Pixels> = v.iter().map(|&w| Pixels(w)).collect();
                f(&widths, w, cx);
            }
        })
    }
    /// A right click on a row (`None`: on empty space).
    pub fn on_context_menu(self, f: impl Fn(Option<usize>, &mut Window, &mut App) + 'static) -> Self {
        self.on_event(move |e, w, cx| {
            if let TableEvent::RightClickedRow(r) = e {
                f(*r, w, cx);
            }
        })
    }
}

impl<D: TableDelegate> Styled for DataTable<D> {
    fn decls_mut(&mut self) -> &mut Vec<Decl> {
        self.base.decls_mut()
    }
}

/// Hand the state's pending events to the listeners.
fn dispatch<D: TableDelegate>(state: Entity<TableState<D>>, ls: &[Listener], window: &mut Window, cx: &mut App) {
    let events = state.update(cx, |s, _| s.take_events());
    for e in &events {
        for l in ls {
            l(e, window, cx);
        }
    }
}

impl<D: TableDelegate> IntoElement for DataTable<D> {
    fn into_any_element(self) -> AnyElement {
        let DataTable { state, base, stripe, bordered, size, header_h, fixed_border, scrollbars, listeners } = self;
        let ls: Rc<[Listener]> = listeners.into();
        let el = base.defer(move |window, cx, el| {
            let (id, mut o) = {
                let s = state.read(cx);
                (s.id.clone(), s.opts)
            };
            if let Some(v) = stripe { o.stripe = v }
            if let Some(v) = bordered { o.bordered = v }
            if let Some(v) = size { o.size = v }
            if let Some(v) = fixed_border { o.fixed_cols_border = v }
            if let Some(v) = scrollbars { o.scrollbars = v }
            o.header_row_height = header_h.or(o.header_row_height);
            // drawn for its size: laid out elsewhere this frame, the frame is laid out again
            let bounds = window.own_bounds(&id).unwrap_or_default();
            let (unit, scale) = (window.ui_unit(), window.scale_factor());
            let handles = state.update(cx, |s, c| {
                s.opts = o;
                let changed = (s.unit() - unit).abs() > f32::EPSILON;
                s.sync(bounds, unit, scale, window, c);
                if changed {
                    window.refresh(); // another UI unit: draw once more with it
                }
                s.header_handles()
            });
            let (down, mv, up, wh, key) = (ls.clone(), ls.clone(), ls.clone(), ls.clone(), ls);
            let taken = std::mem::replace(el, Div::new("div"));
            let mut d = taken.id(id)
                .on_any_mouse_down(move |e, w, cx| {
                    let (pos, b, m, n) = (crate::point(e.position.x.0, e.position.y.0), e.button, e.modifiers, e.click_count);
                    state.update(cx, |s, c| s.mouse_down(pos, b, m, n, w, c));
                    dispatch(state, &down, w, cx);
                })
                .on_mouse_move(move |e, w, cx| {
                    let pos = crate::point(e.position.x.0, e.position.y.0);
                    state.update(cx, |s, c| {
                        if e.pressed_button.is_none() && s.dragging() {
                            s.pointer_up(w, c);
                        } else {
                            s.mouse_move(pos, w, c);
                        }
                    });
                    dispatch(state, &mv, w, cx);
                })
                .on_mouse_up(MouseButton::Left, move |_, w, cx| {
                    state.update(cx, |s, c| s.pointer_up(w, c));
                    dispatch(state, &up, w, cx);
                })
                .on_scroll_wheel(move |e, w, cx| {
                    state.update(cx, |s, _| s.wheel(e.delta, e.modifiers.shift, w));
                    dispatch(state, &wh, w, cx);
                })
                .on_key_down(move |e, w, cx| {
                    state.update(cx, |s, c| s.key_down(e, w, c));
                    dispatch(state, &key, w, cx);
                })
                .child(
                    // the canvas alone in its layer: a percentage-sized canvas next to other
                    // children is not laid out by the engine
                    div().absolute().left(px(0.0)).top(px(0.0)).w_full().h_full()
                        .child(canvas_with(move |b, p, cx| state.read(cx).draw(b, p)).size_full()),
                );
            for (x, y, h) in handles {
                d = d.child(div_handle(x, y, h));
            }
            *el = d;
        });
        AnyElement::Element(el)
    }
}

/// An invisible strip over a header border: only there for its cursor.
fn div_handle(x: f32, y: f32, h: f32) -> Div {
    div().absolute().left(px(x - 3.0)).top(px(y)).w(px(7.0)).h(px(h)).cursor_col_resize()
}
