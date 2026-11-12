//! [`VariableList`]: the element that shows a [`ListState`].
//!
//! One painted leaf (a canvas drawn straight from the state) inside a
//! wrapper that takes pointer, wheel and keyboard input: a frame builds a
//! constant number of elements however many items there are. Layout (which
//! measures and mutates the state) runs while rendering, in the deferred
//! hook; painting only reads.

use std::rc::Rc;

use blenderui_vlist::{ChatStyle, Hit, ItemPainter, ItemSource, List, Measurer, VecSource};

use super::state::{ListState, ViewInfo};
use crate::css::Decl;
use crate::element::{canvas_with, Div};
use crate::html::div;
use crate::{px, AnyElement, App, Entity, IntoElement, MouseButton, ParentElement, Styled, Window};

type Listener<S, P> = Rc<dyn Fn(usize, &mut List<S, P>)>;

/// A variable-height list over `state`.
///
/// ```ignore
/// let list = cx.new(|cx| ListState::new(List::new(source, style, 600.0, 400.0, Options::default()).unwrap(), cx));
/// // in render:
/// div().size_full().child(variable_list(&list).on_item_click(|i, list| println!("{i}")))
/// ```
pub struct VariableList<S: ItemSource + 'static, P: Measurer + ItemPainter + 'static> {
    state: Entity<ListState<S, P>>,
    base: Div,
    on_item_click: Option<Listener<S, P>>,
    on_view_change: Option<Rc<dyn Fn(&ViewInfo)>>,
}

/// The element for a list state.
pub fn variable_list<S: ItemSource, P: Measurer + ItemPainter>(state: &Entity<ListState<S, P>>) -> VariableList<S, P> {
    VariableList { state: *state, base: div().relative().overflow_hidden().size_full(), on_item_click: None, on_view_change: None }
}

/// A chat: a [`VariableList`] over a [`ChatList`] where a click selects a
/// whole message (click it again to deselect).
pub fn chat_list(state: &Entity<ListState<VecSource, ChatStyle>>) -> VariableList<VecSource, ChatStyle> {
    variable_list(state).on_item_click(|i, list| list.toggle_selected(i))
}

impl<S: ItemSource, P: Measurer + ItemPainter> VariableList<S, P> {
    /// The visible range or the count changed (TanStack Virtual's `onChange`):
    /// called after the frame that did it, with the list not borrowed.
    #[must_use]
    pub fn on_view_change(mut self, f: impl Fn(&ViewInfo) + 'static) -> Self {
        self.on_view_change = Some(Rc::new(f));
        self
    }

    /// An item was clicked (item index). The closure may change the list.
    #[must_use]
    pub fn on_item_click(mut self, f: impl Fn(usize, &mut List<S, P>) + 'static) -> Self {
        self.on_item_click = Some(Rc::new(f));
        self
    }
}

impl<S: ItemSource, P: Measurer + ItemPainter> Styled for VariableList<S, P> {
    fn decls_mut(&mut self) -> &mut Vec<Decl> {
        self.base.decls_mut()
    }
}

impl<S: ItemSource, P: Measurer + ItemPainter> IntoElement for VariableList<S, P> {
    fn into_any_element(self) -> AnyElement {
        let VariableList { state, base, on_item_click, on_view_change } = self;
        let el = base.defer(move |window, cx, el| {
            let id = state.read(cx).id.clone();
            let known = window.bounds_of(&id);
            let bounds = known.unwrap_or_default();
            let scale = window.scale_factor();
            let (changed, view) = state.update(cx, |s, _| {
                let moved = known.is_none() || s.bounds != bounds;
                s.sync(bounds, scale, window);
                (moved, s.take_view_change())
            });
            if changed {
                window.refresh(); // placed for the first time or resized: lay out once more with the size
            }
            if let (Some(f), Some(info)) = (&on_view_change, view) {
                f(&info);
            }
            let (down, mv, up, wh, key) = (state, state, state, state, state);
            let click = on_item_click;
            let taken = std::mem::replace(el, Div::new("div"));
            *el = taken
                .id(id)
                .on_any_mouse_down(move |e, w: &mut Window, cx: &mut App| {
                    if e.button != MouseButton::Left {
                        return;
                    }
                    let local = |s: &ListState<S, P>| (e.position.x.0 - s.bounds.origin.x, e.position.y.0 - s.bounds.origin.y);
                    if state_pointer_down(down, click.as_ref(), local, cx) == Hit::Scrollbar {
                        w.poke_scrollbar(down.read(cx).bar_key());
                    }
                    w.refresh();
                })
                .on_mouse_move(move |e, w, cx| {
                    let moved = mv.update(cx, |s, _| {
                        let y = e.position.y.0 - s.bounds.origin.y;
                        if e.pressed_button.is_none() {
                            s.pointer_up();
                            false
                        } else {
                            s.pointer_move(y)
                        }
                    });
                    if moved {
                        w.refresh();
                    }
                })
                .on_mouse_up(MouseButton::Left, move |_, _, cx| up.update(cx, |s, _| s.pointer_up()))
                .on_scroll_wheel(move |e, w, cx| {
                    let key = wh.update(cx, |s, _| {
                        // Blender's wheel scrolls at once, a notch or a touchpad fraction alike: no easing
                        let lines = f64::from(e.delta);
                        s.view.wheel(-lines * WHEEL_NOTCH_PX, true);
                        s.bar_key()
                    });
                    w.poke_scrollbar(key);
                    w.request_animation_frame();
                })
                .on_key_down(move |e, w, cx| {
                    let (handled, k) = key.update(cx, |s, _| (s.key(&e.keystroke.key, LINE_PX), s.bar_key()));
                    if handled {
                        w.poke_scrollbar(k);
                        w.request_animation_frame();
                    }
                })
                .child(
                    // the canvas alone in its layer, as the data table does
                    div().absolute().left(px(0.0)).top(px(0.0)).w_full().h_full().child(
                        canvas_with(move |b, p, cx| state.read(cx).draw(b, p, scale)).size_full(),
                    ),
                );
        });
        AnyElement::Element(el)
    }
}

/// A line of text in px, for keyboard steps.
const LINE_PX: f64 = 19.0;

/// One wheel notch, in px: several messages per notch.
const WHEEL_NOTCH_PX: f64 = 57.0;

fn state_pointer_down<S: ItemSource, P: Measurer + ItemPainter>(
    state: Entity<ListState<S, P>>,
    click: Option<&Listener<S, P>>,
    local: impl Fn(&ListState<S, P>) -> (f32, f32),
    cx: &mut App,
) -> Hit {
    state.update(cx, |s, _| {
        let (x, y) = local(s);
        let hit = s.pointer_down(x, y);
        if let Hit::Item(i) = hit {
            if let Some(f) = click {
                f(i, s);
            }
        }
        hit
    })
}
