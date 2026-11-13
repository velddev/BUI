//! Elements: one [`Div`] type for every HTML tag, built gpui-style.

use std::borrow::Cow;
use std::sync::Arc;

use crate::css::Decl;
pub use crate::styled::Styled;
use crate::events::*;
use crate::window::{App, Window};

/// Shared, cheap-to-clone text (gpui's `SharedString`).
pub type SharedString = Cow<'static, str>;

/// An element identifier (gpui's `ElementId`): keeps widget and scroll
/// state across renders.
pub type ElementId = SharedString;

/// An event listener, owned by its element (moved into the frame, never shared).
pub(crate) type Listener<E> = Box<dyn Fn(&E, &mut Window, &mut App)>;

/// Work an element postpones until the window can lend itself: it receives
/// the window and app and finishes the element (children, styles) in place.
pub(crate) type Deferred = Box<dyn FnOnce(&mut Window, &mut App, &mut Div)>;

thread_local! {
    /// Deferred elements built since the last [`resolve_deferred`].
    static DEFERRED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Finish every deferred element of `el` (parents first, so what a deferred
/// element adds is resolved too). Costs nothing when none were built.
pub(crate) fn resolve_deferred(el: &mut AnyElement, window: &mut Window, app: &mut App) {
    if DEFERRED.replace(0) == 0 {
        return;
    }
    fn walk(el: &mut AnyElement, window: &mut Window, app: &mut App) {
        if let AnyElement::Element(d) = el {
            if let Some(f) = d.deferred.take() {
                f(window, app, d);
            }
            for c in &mut d.children {
                walk(c, window, app);
            }
        }
    }
    walk(el, window, app);
}

/// Custom painting (gpui's `canvas()` element).
pub(crate) type PaintFn = Box<dyn Fn(crate::geometry::Bounds<f32>, &mut crate::paint::Painter, &App)>;

#[derive(Default)]
pub(crate) struct Listeners {
    pub click: Vec<Listener<ClickEvent>>,
    pub mouse_down: Vec<(Option<MouseButton>, Listener<MouseDownEvent>)>,
    pub mouse_up: Vec<(Option<MouseButton>, Listener<MouseUpEvent>)>,
    pub mouse_move: Vec<Listener<MouseMoveEvent>>,
    pub hover: Vec<Listener<bool>>,
    pub scroll: Vec<Listener<ScrollWheelEvent>>,
    pub key_down: Vec<Listener<KeyDownEvent>>,
    pub key_up: Vec<Listener<KeyUpEvent>>,
    pub change: Vec<Listener<ChangeEvent>>,
    pub input: Vec<Listener<ChangeEvent>>,
}

/// An element: a tag, its attributes, styles, children and listeners.
/// `div()`, `span()`, `button()` ... all build one of these.
pub struct Div {
    pub(crate) tag: &'static str,
    pub(crate) id: Option<ElementId>,
    pub(crate) attrs: Vec<(SharedString, SharedString)>,
    pub(crate) decls: Vec<Decl>,
    /// `:hover` / `:active` / `:focus` styles, allocated only when used.
    pub(crate) states: Option<Box<StateDecls>>,
    pub(crate) children: Vec<AnyElement>,
    /// Allocated only when the element has a listener.
    pub(crate) listeners: Option<Box<Listeners>>,
    pub(crate) paint: Option<PaintFn>,
    pub(crate) image: Option<Arc<crate::paint::Image>>,
    pub(crate) video: Option<std::rc::Rc<crate::video::Video>>,
    /// Runs before layout with the window and app (see [`Div::defer`]).
    pub(crate) deferred: Option<Deferred>,
}

/// Styles for interaction states (`.hover(|s| ...)` ...).
#[derive(Default)]
pub(crate) struct StateDecls {
    pub hover: Vec<Decl>,
    pub active: Vec<Decl>,
    pub focus: Vec<Decl>,
}

/// Anything that can be a child: an element or a text run. Elements are
/// stored inline (no box per element).
pub enum AnyElement {
    Element(Div),
    Text(SharedString),
}

/// Convert into an element (gpui's `IntoElement`).
pub trait IntoElement {
    fn into_any_element(self) -> AnyElement;
}

impl IntoElement for Div {
    fn into_any_element(self) -> AnyElement {
        AnyElement::Element(self)
    }
}
impl IntoElement for AnyElement {
    fn into_any_element(self) -> AnyElement {
        self
    }
}
impl IntoElement for &'static str {
    fn into_any_element(self) -> AnyElement {
        AnyElement::Text(Cow::Borrowed(self))
    }
}
impl IntoElement for String {
    fn into_any_element(self) -> AnyElement {
        AnyElement::Text(Cow::Owned(self))
    }
}
impl IntoElement for Cow<'static, str> {
    fn into_any_element(self) -> AnyElement {
        AnyElement::Text(self)
    }
}

impl Div {
    /// An element with any tag name (custom elements: `Div::new("my-widget")`).
    pub fn new(tag: &'static str) -> Div {
        Div {
            tag,
            id: None,
            attrs: Vec::new(),
            decls: Vec::new(),
            states: None,
            children: Vec::new(),
            listeners: None,
            paint: None,
            image: None,
            video: None,
            deferred: None,
        }
    }

    /// Finish this element later, when the window is at hand: `f` runs
    /// once, after the view returned and before layout, and may add
    /// children and styles. Virtualized elements ([`uniform_list`], the data
    /// tables) use it to build only what their last-known size and scroll
    /// offset show.
    ///
    /// [`uniform_list`]: crate::uniform_list
    pub(crate) fn defer(mut self, f: impl FnOnce(&mut Window, &mut App, &mut Div) + 'static) -> Self {
        self.deferred = Some(Box::new(f));
        DEFERRED.set(DEFERRED.get() + 1);
        self
    }

    fn listeners_mut(&mut self) -> &mut Listeners {
        self.listeners.get_or_insert_with(Default::default)
    }

    fn states_mut(&mut self) -> &mut StateDecls {
        self.states.get_or_insert_with(Default::default)
    }

    /// The HTML tag name.
    pub fn tag(&self) -> &'static str {
        self.tag
    }

    /// Give the element an identity (keeps state such as scroll position and
    /// form values across renders).
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        let id = id.into();
        self.attrs.push(("id".into(), id.clone()));
        self.id = Some(id);
        self
    }

    /// Identity without an `id` attribute: keeps state (scroll position,
    /// form values) across renders, invisible to CSS (`#id` doesn't match).
    pub fn key(mut self, key: impl Into<ElementId>) -> Self {
        self.id = Some(key.into());
        self
    }

    /// Set an HTML attribute (`href`, `type`, `value`, `placeholder` ...).
    pub fn attr(mut self, name: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        let name = name.into();
        self.attrs.retain(|(n, _)| *n != name);
        self.attrs.push((name, value.into()));
        self
    }

    /// What this element is to an app-drawn title bar (gpui's `window_control_area`): the
    /// part that moves the window, or its minimise, maximise or close button. Only windows
    /// opened with [`crate::TitlebarOptions::appears_transparent`] use it, on Windows and
    /// macOS; see [`crate::chrome`].
    pub fn window_control_area(self, area: crate::chrome::WindowControlArea) -> Self {
        self.attr(crate::chrome::ATTR, area.name())
    }

    /// Give this element and everything inside it Blender's look (dark
    /// theme, UI font size, themed links, code, tables ...). Without it,
    /// elements get the browsers' defaults: black text on white, with
    /// browser-look (Chrome-style) form controls; inside `styled`, form
    /// controls are Blender's widgets. The Blender components
    /// (`NumberSlider()`, `Button()` ...) are Blender's widgets anywhere.
    /// `<div styled>` in `rsx!`.
    pub fn styled(self) -> Self {
        self.attr("styled", "")
    }

    /// Add CSS classes (matched by stylesheets).
    pub fn class(mut self, classes: impl Into<SharedString>) -> Self {
        let add = classes.into();
        match self.attrs.iter_mut().find(|(n, _)| n == "class") {
            Some((_, v)) => *v = format!("{v} {add}").into(),
            None => self.attrs.push(("class".into(), add)),
        }
        self
    }

    pub(crate) fn get_attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.as_ref())
    }

    // -- common HTML attributes, typed ---------------------------------------------
    pub fn href(self, url: impl Into<SharedString>) -> Self {
        self.attr("href", url)
    }
    pub fn value(self, v: impl Into<SharedString>) -> Self {
        self.attr("value", v)
    }
    pub fn placeholder(self, v: impl Into<SharedString>) -> Self {
        self.attr("placeholder", v)
    }
    pub fn input_type(self, t: impl Into<SharedString>) -> Self {
        self.attr("type", t)
    }
    pub fn name(self, n: impl Into<SharedString>) -> Self {
        self.attr("name", n)
    }
    pub fn checked(self, on: bool) -> Self {
        if on { self.attr("checked", "") } else { self.remove_attr("checked") }
    }
    pub fn disabled(self, on: bool) -> Self {
        if on { self.attr("disabled", "") } else { self.remove_attr("disabled") }
    }
    pub fn selected(self, on: bool) -> Self {
        if on { self.attr("selected", "") } else { self.remove_attr("selected") }
    }
    pub fn open(self, on: bool) -> Self {
        if on { self.attr("open", "") } else { self.remove_attr("open") }
    }
    pub fn min(self, v: f64) -> Self {
        self.attr("min", v.to_string())
    }
    pub fn max(self, v: f64) -> Self {
        self.attr("max", v.to_string())
    }
    pub fn step(self, v: f64) -> Self {
        self.attr("step", v.to_string())
    }
    pub fn src(self, path: impl Into<SharedString>) -> Self {
        self.attr("src", path)
    }
    pub fn alt(self, text: impl Into<SharedString>) -> Self {
        self.attr("alt", text)
    }
    pub fn title(self, text: impl Into<SharedString>) -> Self {
        self.attr("title", text)
    }
    pub fn colspan(self, n: u32) -> Self {
        self.attr("colspan", n.to_string())
    }

    fn remove_attr(mut self, name: &str) -> Self {
        self.attrs.retain(|(n, _)| n != name);
        self
    }

    /// Image pixels for `<img>`: straight RGBA, `width * height * 4` bytes
    /// (copied; to show the same pixels every frame, make an `Image` once and
    /// pass it to `image`).
    pub fn image_rgba(mut self, width: u32, height: u32, rgba: impl AsRef<[u8]>) -> Self {
        self.image = crate::paint::Image::from_rgba(width, height, rgba.as_ref()).map(Arc::new);
        self
    }

    /// Show shared image pixels (cheap to pass every frame).
    pub fn image(mut self, image: Arc<crate::paint::Image>) -> Self {
        self.image = Some(image);
        self
    }

    /// `<video>` with a player you keep (play, pause, seek, events): cheap
    /// to pass every frame. Without it, `<video src>` makes its own.
    pub fn video(mut self, video: std::rc::Rc<crate::video::Video>) -> Self {
        self.video = Some(video);
        self
    }

    // -- events ----------------------------------------------------------------------------
    /// Click (press and release on this element); bubbles to ancestors.
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.listeners_mut().click.push(Box::new(f));
        self
    }
    pub fn on_mouse_down(mut self, button: MouseButton,
                         f: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static) -> Self {
        self.listeners_mut().mouse_down.push((Some(button), Box::new(f)));
        self
    }
    pub fn on_any_mouse_down(mut self,
                             f: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static) -> Self {
        self.listeners_mut().mouse_down.push((None, Box::new(f)));
        self
    }
    pub fn on_mouse_up(mut self, button: MouseButton,
                       f: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static) -> Self {
        self.listeners_mut().mouse_up.push((Some(button), Box::new(f)));
        self
    }
    pub fn on_mouse_move(mut self,
                         f: impl Fn(&MouseMoveEvent, &mut Window, &mut App) + 'static) -> Self {
        self.listeners_mut().mouse_move.push(Box::new(f));
        self
    }
    /// Called with `true` when the pointer enters, `false` when it leaves.
    pub fn on_hover(mut self, f: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.listeners_mut().hover.push(Box::new(f));
        self
    }
    pub fn on_scroll_wheel(mut self,
                           f: impl Fn(&ScrollWheelEvent, &mut Window, &mut App) + 'static) -> Self {
        self.listeners_mut().scroll.push(Box::new(f));
        self
    }
    pub fn on_key_down(mut self, f: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static) -> Self {
        self.listeners_mut().key_down.push(Box::new(f));
        self
    }
    pub fn on_key_up(mut self, f: impl Fn(&KeyUpEvent, &mut Window, &mut App) + 'static) -> Self {
        self.listeners_mut().key_up.push(Box::new(f));
        self
    }
    /// Form controls: the committed value changed (`change` in HTML).
    pub fn on_change(mut self, f: impl Fn(&ChangeEvent, &mut Window, &mut App) + 'static) -> Self {
        self.listeners_mut().change.push(Box::new(f));
        self
    }
    /// Form controls: every edit (`input` in HTML).
    pub fn on_input(mut self, f: impl Fn(&ChangeEvent, &mut Window, &mut App) + 'static) -> Self {
        self.listeners_mut().input.push(Box::new(f));
        self
    }

    // -- state styles (gpui's .hover(|s| ...), CSS :hover / :active / :focus) ----------------
    pub fn hover(mut self, f: impl FnOnce(StyleRefinement) -> StyleRefinement) -> Self {
        self.states_mut().hover.extend(f(StyleRefinement::default()).decls);
        self
    }
    pub fn active(mut self, f: impl FnOnce(StyleRefinement) -> StyleRefinement) -> Self {
        self.states_mut().active.extend(f(StyleRefinement::default()).decls);
        self
    }
    pub fn focus(mut self, f: impl FnOnce(StyleRefinement) -> StyleRefinement) -> Self {
        self.states_mut().focus.extend(f(StyleRefinement::default()).decls);
        self
    }
}

/// A set of style declarations (the argument of `.hover(|s| ...)`).
#[derive(Clone, Default)]
pub struct StyleRefinement {
    pub(crate) decls: Vec<Decl>,
}

impl Styled for Div {
    fn decls_mut(&mut self) -> &mut Vec<Decl> {
        // one allocation for a typical element's handful of style calls
        if self.decls.capacity() == 0 {
            self.decls.reserve(8);
        }
        &mut self.decls
    }
}
impl Styled for StyleRefinement {
    fn decls_mut(&mut self) -> &mut Vec<Decl> {
        &mut self.decls
    }
}

/// Children (gpui's `ParentElement`).
pub trait ParentElement: Sized {
    fn children_mut(&mut self) -> &mut Vec<AnyElement>;

    fn child(mut self, child: impl IntoElement) -> Self {
        self.children_mut().push(child.into_any_element());
        self
    }

    fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.children_mut().extend(children.into_iter().map(IntoElement::into_any_element));
        self
    }
}

impl ParentElement for Div {
    fn children_mut(&mut self) -> &mut Vec<AnyElement> {
        &mut self.children
    }
}

/// Conditional building (gpui's `FluentBuilder`).
pub trait FluentBuilder: Sized {
    fn when(self, condition: bool, then: impl FnOnce(Self) -> Self) -> Self {
        if condition { then(self) } else { self }
    }
    fn when_some<T>(self, option: Option<T>, then: impl FnOnce(Self, T) -> Self) -> Self {
        match option {
            Some(v) => then(self, v),
            None => self,
        }
    }
    fn when_none<T>(self, option: &Option<T>, then: impl FnOnce(Self) -> Self) -> Self {
        if option.is_none() { then(self) } else { self }
    }
    fn map<U>(self, f: impl FnOnce(Self) -> U) -> U {
        f(self)
    }
}

impl FluentBuilder for Div {}
impl FluentBuilder for StyleRefinement {}

/// gpui's `canvas(prepaint, paint)`: an element that paints itself.
pub fn canvas(paint: impl Fn(crate::geometry::Bounds<f32>, &mut crate::paint::Painter) + 'static)
    -> Div {
    let mut d = Div::new("canvas");
    d.paint = Some(Box::new(move |b, p, _| paint(b, p)));
    d
}

/// Like [`canvas`], and the paint callback also reads the app: a view over
/// an entity paints straight from it (`entity.read(cx)`) with nothing copied
/// into the element tree. The data tables draw this way.
pub fn canvas_with(paint: impl Fn(crate::geometry::Bounds<f32>, &mut crate::paint::Painter, &App) + 'static)
    -> Div {
    let mut d = Div::new("canvas");
    d.paint = Some(Box::new(paint));
    d
}
