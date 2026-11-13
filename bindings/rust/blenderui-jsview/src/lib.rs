//! The JavaScript view of a BUI window, shared by the JavaScript runtimes
//! (`blenderui-node`: Node.js / Bun through napi-rs; `blenderui-qjs`: the
//! embedded QuickJS runtime).
//!
//! JavaScript keeps a retained tree and sends what changed as ops (an `i32`
//! array plus a string table); [`JsView`] applies them to its mirror of the
//! tree and renders it. Events go back through a [`Sink`]. The functions at
//! the end run the window operations both runtimes expose (open, commit,
//! frame, close, screenshot, bounds, click) on the UI thread.
//!
//! SPDX-License-Identifier: GPL-2.0-or-later

use std::collections::HashMap;
use std::sync::Arc;

use blenderui::prelude::*;

/// An event from a window (`kind`: click, mousedown, mouseup, mousemove,
/// mouseenter, mouseleave, wheel, keydown, keyup, change, input, frame,
/// close).
#[derive(Clone, Debug, Default)]
pub struct Event {
    pub kind: String,
    pub window: u32,
    /// The node whose listener fired.
    pub target: u32,
    pub x: f64,
    pub y: f64,
    /// 0 left, 1 middle, 2 right (DOM numbering).
    pub button: u32,
    pub click_count: u32,
    pub delta: f64,
    pub key: Option<String>,
    pub key_char: Option<String>,
    pub value: Option<String>,
    pub checked: bool,
    pub number: f64,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Where a window's events go (called on the UI thread).
pub type Sink = Arc<dyn Fn(Event)>;

/// A window as JavaScript opens it.
#[derive(Clone, Debug, Default)]
pub struct WindowConfig {
    pub title: Option<String>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub resizable: Option<bool>,
    pub gpu: Option<bool>,
}

/// A layout box, logical pixels.
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Ops (`commit`): opcode, then its arguments; `s` = index into the strings.
pub mod op {
    pub const CREATE: i32 = 0; // id, s(tag)
    pub const TEXT: i32 = 1; // id, s(text)
    pub const SET_ATTR: i32 = 2; // id, s(name), s(value)
    pub const REMOVE_ATTR: i32 = 3; // id, s(name)
    pub const APPEND: i32 = 4; // parent, child
    pub const INSERT: i32 = 5; // parent, child, before
    pub const REMOVE: i32 = 6; // parent, child
    pub const SET_TEXT: i32 = 7; // id, s(text)
    pub const LISTEN: i32 = 8; // id, event bits (replaces)
    pub const FREE: i32 = 9; // id
}

/// Event bits (`LISTEN`), matching `bindings/node/index.js`.
pub mod ev {
    pub const CLICK: u32 = 1;
    pub const MOUSE_DOWN: u32 = 2;
    pub const MOUSE_UP: u32 = 4;
    pub const MOUSE_MOVE: u32 = 8;
    pub const HOVER: u32 = 16;
    pub const WHEEL: u32 = 32;
    pub const KEY_DOWN: u32 = 64;
    pub const KEY_UP: u32 = 128;
    pub const CHANGE: u32 = 256;
    pub const INPUT: u32 = 512;
}

// ==========================================================================
// The retained tree (UI thread)

struct JsNode {
    tag: &'static str,
    key: SharedString,
    text: Option<SharedString>,
    attrs: Vec<(SharedString, SharedString)>,
    /// The `style` attribute, parsed once when set (renders reuse it).
    style: Option<Box<[blenderui::css::Decl]>>,
    children: Vec<u32>,
    events: u32,
}

/// `style="..."` parsed into declarations whose values are shared.
fn parse_style(css: &str) -> Box<[blenderui::css::Decl]> {
    blenderui::css::parse_declarations(css).into_iter().map(|d| blenderui::css::Decl {
        value: blenderui::css::Value::Shared(std::sync::Arc::from(&*d.value)),
        ..d
    }).collect()
}

/// Hashes a node id as itself: ids are unique, increasing integers.
#[derive(Default, Clone, Copy)]
struct IdHasher(u64);

impl std::hash::Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 << 8) | u64::from(b);
        }
    }
    fn write_u32(&mut self, n: u32) {
        self.0 = u64::from(n);
    }
}

type NodeMap = HashMap<u32, JsNode, std::hash::BuildHasherDefault<IdHasher>>;

/// One window's tree; node ids are per window, never reused (root = 0).
struct JsView {
    window: u32,
    nodes: NodeMap,
    sink: Sink,
    /// JS asked for an animation frame: tell it when this one renders.
    raf: bool,
    /// The next vsync frame is already scheduled (changes just wait for it).
    frame_scheduled: bool,
}

/// Tag names as the `&'static str` elements need: HTML tags from the
/// table, others interned once.
fn intern_tag(tag: &str, custom: &mut HashMap<String, &'static str>) -> &'static str {
    if let Some((t, _)) = blenderui::html::TAGS.iter().find(|(t, _)| *t == tag) {
        return t;
    }
    *custom.entry(tag.to_string()).or_insert_with(|| Box::leak(tag.to_string().into_boxed_str()))
}

impl JsView {
    fn node(&mut self, id: i32) -> Option<&mut JsNode> {
        self.nodes.get_mut(&u32::try_from(id).ok()?)
    }

    fn put(&mut self, id: i32, n: JsNode) {
        if let Ok(i) = u32::try_from(id) {
            self.nodes.insert(i, n);
        }
    }

    fn apply(&mut self, ops: &[i32], strings: &[String], tags: &mut HashMap<String, &'static str>) {
        let s = |k: i32| strings.get(k as usize).map(String::as_str).unwrap_or("");
        let mut i = 0;
        while i < ops.len() {
            let a = |k: usize| ops.get(i + k).copied().unwrap_or(-1);
            match ops[i] {
                op::CREATE => {
                    let tag = intern_tag(s(a(2)), tags);
                    self.put(a(1), JsNode { tag, key: format!("n{}", a(1)).into(), text: None, attrs: Vec::new(),
                                            style: None, children: Vec::new(), events: 0 });
                    i += 3;
                }
                op::TEXT => {
                    let text = s(a(2)).to_string();
                    self.put(a(1), JsNode { tag: "", key: SharedString::Borrowed(""), text: Some(text.into()),
                                            attrs: Vec::new(), style: None, children: Vec::new(), events: 0 });
                    i += 3;
                }
                op::SET_ATTR => {
                    let (name, value) = (s(a(2)).to_string(), s(a(3)).to_string());
                    if let Some(n) = self.node(a(1)) {
                        if name == "style" {
                            n.style = Some(parse_style(&value));
                        }
                        match n.attrs.iter_mut().find(|(k, _)| *k == name) {
                            Some((_, v)) => *v = value.into(),
                            None => n.attrs.push((name.into(), value.into())),
                        }
                    }
                    i += 4;
                }
                op::REMOVE_ATTR => {
                    let name = s(a(2));
                    if let Some(n) = self.node(a(1)) {
                        if name == "style" {
                            n.style = None;
                        }
                        n.attrs.retain(|(k, _)| k != name);
                    }
                    i += 3;
                }
                op::APPEND => {
                    let child = a(2) as u32;
                    if let Some(n) = self.node(a(1)) {
                        n.children.retain(|&c| c != child);
                        n.children.push(child);
                    }
                    i += 3;
                }
                op::INSERT => {
                    let (child, before) = (a(2) as u32, a(3) as u32);
                    if let Some(n) = self.node(a(1)) {
                        n.children.retain(|&c| c != child);
                        let at = n.children.iter().position(|&c| c == before).unwrap_or(n.children.len());
                        n.children.insert(at, child);
                    }
                    i += 4;
                }
                op::REMOVE => {
                    let child = a(2) as u32;
                    if let Some(n) = self.node(a(1)) {
                        n.children.retain(|&c| c != child);
                    }
                    i += 3;
                }
                op::SET_TEXT => {
                    let text = s(a(2)).to_string();
                    if let Some(n) = self.node(a(1)) {
                        n.text = Some(text.into());
                    }
                    i += 3;
                }
                op::LISTEN => {
                    let bits = a(2) as u32;
                    if let Some(n) = self.node(a(1)) {
                        n.events = bits;
                    }
                    i += 3;
                }
                op::FREE => {
                    if let Ok(k) = u32::try_from(a(1)) {
                        self.nodes.remove(&k);
                    }
                    i += 2;
                }
                _ => break, // malformed: stop rather than misread
            }
        }
    }

    fn build(&self, id: u32) -> AnyElement {
        let Some(n) = self.nodes.get(&id) else {
            return String::new().into_any_element();
        };
        if let Some(t) = &n.text {
            return t.clone().into_any_element();
        }
        let mut d = Div::new(n.tag).key(n.key.clone());
        for (k, v) in &n.attrs {
            d = match &**k {
                "class" | "className" => d.class(v.clone()),
                "style" => d, // parsed: below
                "id" => d.id(v.clone()),
                _ => d.attr(k.clone(), v.clone()),
            };
        }
        if let Some(s) = &n.style {
            d = d.style(&s[..]);
        }
        if n.events != 0 {
            d = self.listeners(d, id, n.events);
        }
        for &c in &n.children {
            d = d.child(self.build(c));
        }
        d.into_any_element()
    }

    fn listeners(&self, mut d: Div, id: u32, bits: u32) -> Div {
        let base = Event { window: self.window, target: id, ..Default::default() };
        let send = {
            let t = self.sink.clone();
            move |e: Event| t(e)
        };
        fn mods(e: &mut Event, m: &Modifiers) {
            (e.shift, e.ctrl, e.alt) = (m.shift, m.control || m.platform, m.alt);
        }
        fn button(b: &MouseButton) -> u32 {
            match b {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
                MouseButton::Back => 3,
                MouseButton::Forward => 4,
            }
        }
        // The deepest listening node gets the event, and native propagation
        // stops there: JavaScript bubbles it through its own tree, so
        // stopPropagation() works. Hover (mouseenter / mouseleave) doesn't bubble.
        macro_rules! on {
            ($bit:expr, $method:ident $(($arg:expr))?, |$e:ident, $ev:ident| $body:block) => {
                on!(@ true, $bit, $method $(($arg))?, |$e, $ev| $body)
            };
            (@ $stop:expr, $bit:expr, $method:ident $(($arg:expr))?, |$e:ident, $ev:ident| $body:block) => {
                if bits & $bit != 0 {
                    let (send, base) = (send.clone(), base.clone());
                    d = d.$method($($arg,)? move |$ev, _, app| {
                        let mut $e = base.clone();
                        $body
                        send($e);
                        if $stop {
                            app.stop_propagation();
                        }
                    });
                }
            };
        }
        on!(ev::CLICK, on_click, |e, c| {
            e.kind = "click".into();
            (e.x, e.y) = (c.up.position.x.0 as f64, c.up.position.y.0 as f64);
            e.button = button(&c.up.button);
            e.click_count = c.down.click_count as u32;
            mods(&mut e, &c.up.modifiers);
        });
        on!(ev::MOUSE_DOWN, on_any_mouse_down, |e, m| {
            e.kind = "mousedown".into();
            (e.x, e.y) = (m.position.x.0 as f64, m.position.y.0 as f64);
            e.button = button(&m.button);
            e.click_count = m.click_count as u32;
            mods(&mut e, &m.modifiers);
        });
        for b in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
            on!(ev::MOUSE_UP, on_mouse_up(b.clone()), |e, m| {
                e.kind = "mouseup".into();
                (e.x, e.y) = (m.position.x.0 as f64, m.position.y.0 as f64);
                e.button = button(&m.button);
                mods(&mut e, &m.modifiers);
            });
        }
        on!(ev::MOUSE_MOVE, on_mouse_move, |e, m| {
            e.kind = "mousemove".into();
            (e.x, e.y) = (m.position.x.0 as f64, m.position.y.0 as f64);
            mods(&mut e, &m.modifiers);
        });
        on!(@ false, ev::HOVER, on_hover, |e, h| {
            e.kind = if *h { "mouseenter" } else { "mouseleave" }.into();
        });
        on!(ev::WHEEL, on_scroll_wheel, |e, w| {
            e.kind = "wheel".into();
            (e.x, e.y) = (w.position.x.0 as f64, w.position.y.0 as f64);
            e.delta = w.delta as f64;
            mods(&mut e, &w.modifiers);
        });
        on!(ev::KEY_DOWN, on_key_down, |e, k| {
            e.kind = "keydown".into();
            e.key = Some(k.keystroke.key.clone());
            e.key_char = k.keystroke.key_char.clone();
            mods(&mut e, &k.keystroke.modifiers);
        });
        on!(ev::KEY_UP, on_key_up, |e, k| {
            e.kind = "keyup".into();
            e.key = Some(k.keystroke.key.clone());
            e.key_char = k.keystroke.key_char.clone();
            mods(&mut e, &k.keystroke.modifiers);
        });
        on!(ev::CHANGE, on_change, |e, c| {
            e.kind = "change".into();
            (e.value, e.checked, e.number) = (Some(c.value.clone()), c.checked, c.number);
        });
        on!(ev::INPUT, on_input, |e, c| {
            e.kind = "input".into();
            (e.value, e.checked, e.number) = (Some(c.value.clone()), c.checked, c.number);
        });
        d
    }
}

impl Render for JsView {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.frame_scheduled = false;
        if self.raf {
            // requestAnimationFrame: JS runs its callbacks while this frame
            // presents, and the next vsync frame is scheduled now (as a
            // browser does), so their changes make it into that frame
            self.raf = false;
            self.frame_scheduled = true;
            window.request_animation_frame();
            (self.sink)(Event { kind: "frame".into(), window: self.window, ..Default::default() });
        }
        self.build(0)
    }
}

/// UI-thread state: JS window ids -> windows.
#[derive(Default)]
pub struct UiState {
    windows: HashMap<u32, WindowHandle<JsView>>,
    by_native: HashMap<usize, u32>,
    tags: HashMap<String, &'static str>,
}

impl JsView {
    /// A view of window `window` (JavaScript's id) with an empty `<body>` as node 0.
    pub fn new(window: u32, sink: Sink) -> JsView {
        let mut v = JsView { window, nodes: NodeMap::default(), sink, raf: false, frame_scheduled: false };
        v.put(0, JsNode { tag: "body", key: "n0".into(), text: None, attrs: Vec::new(),
                          style: None, children: Vec::new(), events: 0 });
        v
    }
}

// ==========================================================================
// Window operations (UI thread)

/// Set up the UI-thread state; `sink` also gets `close` events.
pub fn init(cx: &mut App, sink: Sink) {
    cx.set_global(UiState::default());
    cx.set_global(CloseSink(sink));
    cx.on_window_closed(|id, cx| {
        let Some(w) = cx.global::<UiState>().by_native.get(&id.0).copied() else { return };
        let st = cx.global_mut::<UiState>();
        st.windows.remove(&w);
        st.by_native.remove(&id.0);
        let sink = cx.global::<CloseSink>().0.clone();
        sink(Event { kind: "close".into(), window: w, ..Default::default() });
    });
}

struct CloseSink(Sink);

/// Open window `id` (chosen by JavaScript); its events go to `sink`.
pub fn open_window(cx: &mut App, id: u32, config: WindowConfig, sink: Sink) {
    let opts = blenderui::WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(config.width.unwrap_or(800.0) as f32), px(config.height.unwrap_or(600.0) as f32)),
        })),
        titlebar: config.title.map(|t| TitlebarOptions { title: Some(t.into()), ..Default::default() }),
        is_resizable: config.resizable,
        gpu: config.gpu,
        ..Default::default()
    };
    let handle = cx.open_window(opts, |_, cx| cx.new(|_| JsView::new(id, sink.clone())));
    match handle {
        Ok(handle) => {
            let st = cx.global_mut::<UiState>();
            st.by_native.insert(handle.id.0, id);
            st.windows.insert(id, handle);
        }
        Err(e) => {
            // Say why, then close the window for JavaScript: with quitOnLastWindowClosed the app
            // ends instead of running with nothing to see.
            eprintln!("blenderui: could not open window {id}: {e}. Is there a display? Check \
                       DISPLAY (X11) or WAYLAND_DISPLAY and XDG_RUNTIME_DIR (Wayland).");
            sink(Event { kind: "close".into(), window: id, ..Default::default() });
        }
    }
}

/// Apply a batch of ops to window `window` and re-render it: right away, or
/// (`animated`, during requestAnimationFrame) on the next vsync.
pub fn commit(cx: &mut App, window: u32, ops: &[i32], strings: &[String], animated: bool) {
    let Some(handle) = cx.global::<UiState>().windows.get(&window).cloned() else { return };
    let mut tags = std::mem::take(&mut cx.global_mut::<UiState>().tags);
    handle.update(cx, |v, w, _| {
        v.apply(ops, strings, &mut tags);
        if !animated {
            w.refresh(); // this window only
        } else if !v.frame_scheduled {
            v.frame_scheduled = true;
            w.request_animation_frame();
        }
    });
    cx.global_mut::<UiState>().tags = tags;
}

/// Send a `frame` event when window `window` next renders (vsync).
pub fn request_frame(cx: &mut App, window: u32) {
    if let Some(handle) = cx.global::<UiState>().windows.get(&window).cloned() {
        handle.update(cx, |v, w, _| {
            v.raf = true;
            if !v.frame_scheduled {
                v.frame_scheduled = true;
                w.request_animation_frame();
            }
        });
    }
}

pub fn close_window(cx: &mut App, window: u32) {
    let st = cx.global_mut::<UiState>();
    if let Some(h) = st.windows.remove(&window) {
        st.by_native.remove(&h.id.0);
        cx.close_window(h.id);
    }
}

/// How many windows are open.
pub fn window_count(cx: &App) -> usize {
    cx.try_global::<UiState>().map_or(0, |s| s.windows.len())
}

/// Close every window.
pub fn close_all(cx: &mut App) {
    let ids: Vec<WindowId> = cx.global::<UiState>().windows.values().map(|h| h.id).collect();
    for id in ids {
        cx.close_window(id);
    }
    let st = cx.global_mut::<UiState>();
    st.windows.clear();
    st.by_native.clear();
}

/// Render window `window`: `(width, height, RGBA bytes)` in device pixels.
pub fn screenshot(cx: &mut App, window: u32) -> Option<(u32, u32, Vec<u8>)> {
    let handle = cx.global::<UiState>().windows.get(&window).cloned()?;
    let (w, h, px) = cx.draw(handle.id);
    let mut rgba = Vec::with_capacity(px.len() * 4);
    for p in px {
        rgba.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, 255]);
    }
    Some((w as u32, h as u32, rgba))
}

/// Bounds of node `node` of window `window`, after rendering it.
pub fn bounds(cx: &mut App, window: u32, node: u32) -> Option<Rect> {
    let h = cx.global::<UiState>().windows.get(&window).cloned()?;
    cx.draw(h.id);
    cx.element_bounds(h.id, &format!("n{node}")).map(|b| Rect {
        x: b.origin.x as f64,
        y: b.origin.y as f64,
        width: b.size.width as f64,
        height: b.size.height as f64,
    })
}

/// Click at (x, y) in window `window` (tests; logical pixels).
pub fn simulate_click(cx: &mut App, window: u32, x: f64, y: f64) {
    if let Some(h) = cx.global::<UiState>().windows.get(&window).cloned() {
        cx.simulate_click(h.id, x as f32, y as f32);
    }
}

// ==========================================================================
// Measuring (benchmarks): the same counters as bench/common/measure.rs.

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> isize;
    fn QueryProcessCycleTime(p: isize, cycles: *mut u64) -> i32;
}

/// CPU cycles used by every thread of this process (Windows; otherwise None).
pub fn process_cpu_cycles() -> Option<f64> {
    #[cfg(windows)]
    {
        let mut c = 0u64;
        // SAFETY: the current-process pseudo handle and a valid out pointer.
        unsafe { QueryProcessCycleTime(GetCurrentProcess(), &mut c) };
        Some(c as f64)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// The time-stamp counter (x86-64; otherwise None).
pub fn timestamp_counter() -> Option<f64> {
    #[cfg(target_arch = "x86_64")]
    {
        // SAFETY: rdtsc has no preconditions on x86-64.
        Some(unsafe { core::arch::x86_64::_rdtsc() } as f64)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        None
    }
}
