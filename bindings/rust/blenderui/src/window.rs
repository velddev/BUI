//! Application, windows, entities and event dispatch, shaped like gpui:
//!
//! ```ignore
//! Application::new().run(|cx: &mut App| {
//!     cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| Counter { n: 0 })).unwrap();
//! });
//! ```

use std::any::{Any, TypeId};
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use crate::element::{AnyElement, IntoElement, Listener, Listeners, SharedString};
use crate::events::*;
use crate::geometry::{point, px, size, Bounds, Pixels, Size};
use crate::layout::{id_key, layout, Content, Frame, Key, LayoutCaches, UiState};
use crate::prof::{self, span, Phase};
use crate::paint::{paint_frame, Painter};
use crate::widgets::{WKind, Widgets};
pub use crate::widgets::TextCaret;
use blenderui_sys as sys;

/// How early a timer may fire and still count as due, s.
pub(crate) const TIMER_SLOP: f64 = 0.0005;

// ==========================================================================
// Entities

/// State owned by the app (gpui's `Entity<T>`): a `Copy` handle (slot index
/// and generation) into the app's entity slots. No reference counting.
pub struct Entity<T> {
    id: u32,
    generation: u32,
    _t: PhantomData<fn() -> T>,
}

impl<T> Clone for Entity<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Entity<T> {}

/// One entity's storage. The value is moved out while it is being updated.
struct Slot {
    generation: u32,
    value: Option<Box<dyn Any>>,
}

impl<T: 'static> Entity<T> {
    /// Read the value.
    pub fn read<'a>(&self, cx: &'a App) -> &'a T {
        let slot = &cx.entities[self.id as usize];
        assert!(slot.generation == self.generation, "entity was released");
        slot.value.as_ref().expect("entity is being built or updated")
            .downcast_ref::<T>().expect("entity type")
    }

    /// Mutate the value with a [`Context`] for it. The value is taken out of
    /// its slot meanwhile, so `cx` stays fully usable (except for updating
    /// this same entity again, which panics).
    pub fn update<R>(&self, cx: &mut App, f: impl FnOnce(&mut T, &mut Context<T>) -> R) -> R {
        let slot = &mut cx.entities[self.id as usize];
        assert!(slot.generation == self.generation, "entity was released");
        let mut value = slot.value.take().expect("entity is being built or already being updated");
        let r = {
            let v = value.downcast_mut::<T>().expect("entity type");
            let mut ctx = Context { app: cx, entity: *self };
            f(v, &mut ctx)
        };
        cx.entities[self.id as usize].value = Some(value);
        r
    }

    /// Same entity?
    pub fn entity_id(&self) -> usize {
        self.id as usize
    }

    pub fn downgrade(&self) -> WeakEntity<T> {
        WeakEntity { entity: *self }
    }
}

/// A handle that may outlive its entity (gpui's `WeakEntity<T>`).
pub struct WeakEntity<T> {
    entity: Entity<T>,
}

impl<T> Clone for WeakEntity<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for WeakEntity<T> {}

impl<T: 'static> WeakEntity<T> {
    /// The entity, if it hasn't been released.
    pub fn upgrade_in(&self, cx: &App) -> Option<Entity<T>> {
        let slot = cx.entities.get(self.entity.id as usize)?;
        (slot.generation == self.entity.generation).then_some(self.entity)
    }

    /// Update if still alive.
    pub fn update<R>(&self, cx: &mut App, f: impl FnOnce(&mut T, &mut Context<T>) -> R) -> Option<R> {
        self.upgrade_in(cx).map(|e| e.update(cx, f))
    }
}

/// The app, seen from inside an entity (gpui's `Context<T>`); derefs to [`App`].
pub struct Context<'a, T> {
    app: &'a mut App,
    entity: Entity<T>,
}

impl<T> Deref for Context<'_, T> {
    type Target = App;
    fn deref(&self) -> &App {
        self.app
    }
}
impl<T> DerefMut for Context<'_, T> {
    fn deref_mut(&mut self) -> &mut App {
        self.app
    }
}

impl<T: 'static> Context<'_, T> {
    /// This entity.
    pub fn entity(&self) -> Entity<T> {
        self.entity
    }

    pub fn weak_entity(&self) -> WeakEntity<T> {
        self.entity.downgrade()
    }

    /// Re-render the windows showing this entity.
    pub fn notify(&mut self) {
        self.app.notify_all();
    }

    /// Turn a method-like closure into an element listener (gpui's `cx.listener`).
    pub fn listener<E: ?Sized>(
        &self,
        f: impl Fn(&mut T, &E, &mut Window, &mut Context<T>) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut App) + 'static {
        let weak = self.entity.downgrade();
        move |e: &E, window: &mut Window, cx: &mut App| {
            weak.update(cx, |this, cx| f(this, e, window, cx));
        }
    }
}

/// A view: something that renders an element tree (gpui's `Render`).
pub trait Render: 'static + Sized {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement;
}

// ==========================================================================
// The application

/// Window identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowId(pub usize);

/// A typed window handle (gpui's `WindowHandle<V>`).
pub struct WindowHandle<V> {
    pub id: WindowId,
    entity: Entity<V>,
}

impl<V> Clone for WindowHandle<V> {
    fn clone(&self) -> Self {
        WindowHandle { id: self.id, entity: self.entity }
    }
}

impl<V: 'static> WindowHandle<V> {
    /// The root view of the window.
    pub fn root(&self) -> Entity<V> {
        self.entity
    }

    /// Update the root view with its window.
    pub fn update<R>(&self, cx: &mut App,
                     f: impl FnOnce(&mut V, &mut Window, &mut Context<V>) -> R) -> Option<R> {
        let mut w = cx.windows.get_mut(self.id.0)?.take()?;
        let notified = cx.notified;
        let r = self.entity.update(cx, |v, cx| f(v, &mut w, cx));
        if cx.notified != notified {
            w.refresh(); // `cx.notify()` ran while this window was out of the list
        }
        cx.windows[self.id.0] = Some(w);
        Some(r)
    }
}

/// How a window opens (gpui's `WindowBounds`).
#[derive(Clone, Copy, Debug)]
pub enum WindowBounds {
    Windowed(Bounds<Pixels>),
    Maximized(Bounds<Pixels>),
    Fullscreen(Bounds<Pixels>),
}

#[derive(Clone, Debug, Default)]
pub struct TitlebarOptions {
    pub title: Option<SharedString>,
}

/// Options for [`App::open_window`] (gpui's `WindowOptions`).
#[derive(Clone, Debug, Default)]
pub struct WindowOptions {
    pub window_bounds: Option<WindowBounds>,
    pub titlebar: Option<TitlebarOptions>,
    pub is_resizable: Option<bool>,
    pub window_min_size: Option<Size<Pixels>>,
    /// Draw this window on the GPU (Direct3D 11 / OpenGL 3.3) instead of the software
    /// raster; `None` follows [`Application::gpu`] / the `BUI_GPU` variable.
    pub gpu: Option<bool>,
}

/// The application entry point (gpui's `Application`).
pub struct Application {
    headless: bool,
    gpu: Option<bool>,
    slug: Option<bool>,
    keep_alive: bool,
    theme: Option<String>,
}

impl Default for Application {
    fn default() -> Self {
        Application::new()
    }
}

impl Application {
    /// Native windows.
    pub fn new() -> Application {
        Application { headless: false, gpu: None, slug: None, keep_alive: false, theme: None }
    }

    /// Draw every window on the GPU (Direct3D 11 on Windows): the same
    /// pixels as the software raster, with rasterization off the CPU. Falls
    /// back to software where no device exists. The `BUI_GPU` environment
    /// variable (1/0) overrides this.
    pub fn gpu(mut self, on: bool) -> Application {
        self.gpu = Some(on);
        self
    }

    /// How the GPU renderer draws text: `true` for Slug (glyph outlines in
    /// the pixel shader, sharp at any size), `false` for a per-glyph bitmap
    /// atlas like Blender's (less GPU work). Defaults to the `slug` feature;
    /// `BUI_SLUG=1`/`0` overrides. See also [`crate::set_slug`].
    pub fn slug(mut self, on: bool) -> Application {
        self.slug = Some(on);
        self
    }

    /// Blender's `"dark"` (default) or `"light"` theme for every window (see
    /// [`crate::set_theme`]); the `BUI_THEME` variable applies otherwise.
    pub fn theme(mut self, name: &str) -> Application {
        self.theme = Some(name.to_string());
        self
    }

    /// Keep the event loop running with no windows open, until
    /// [`App::quit`]: for apps driven from another thread ([`App::ui_sender`]).
    pub fn keep_alive(mut self, on: bool) -> Application {
        self.keep_alive = on;
        self
    }

    /// Off-screen windows (tests, screenshots, CI): nothing is shown, the
    /// `simulate_*` and `draw` methods drive it.
    pub fn headless() -> Application {
        Application { headless: true, gpu: None, slug: None, keep_alive: false, theme: None }
    }

    /// Run `f`, then the event loop until every window is closed.
    pub fn run(self, f: impl FnOnce(&mut App)) {
        // the C kit keeps global state: one application at a time per process
        static KIT: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = KIT.lock().unwrap_or_else(|e| e.into_inner());
        let headless = self.headless || unsafe { sys::BK_window_supported() } == 0;
        if let Some(on) = self.gpu {
            unsafe { sys::BUI_gpu_set_default(on as c_int) };
        }
        if let Some(on) = self.slug {
            crate::set_slug(on);
        }
        if let Some(t) = &self.theme {
            crate::set_theme(t);
        }
        if headless {
            // off-screen windows have no monitor DPI: BUI_UI_SCALE (e.g. 2 for
            // retina-sharp screenshots) sets the resolution scale
            if let Some(s) = std::env::var("BUI_UI_SCALE").ok().and_then(|v| v.parse::<f32>().ok()) {
                unsafe { sys::BK_set_scale(s) };
            }
        }
        let mut app = App::create(headless);
        f(&mut app);
        app.flush();
        if headless {
            app.shutdown();
            return;
        }
        // the app stays on this stack frame; kit callbacks reach it through APP
        APP.with(|a| a.set(&mut app as *mut App));
        unsafe { sys::BK_window_set_keep_alive(self.keep_alive as c_int) };
        // SAFETY: plain message loop; `app` isn't touched here until it returns.
        unsafe { sys::BK_window_run() };
        APP.with(|a| a.set(std::ptr::null_mut()));
        app.shutdown();
    }
}

thread_local! {
    /// The app while the native loop runs (it lives on `Application::run`'s stack).
    static APP: Cell<*mut App> = const { Cell::new(std::ptr::null_mut()) };
    /// Set while a callback holds the app (a re-entrant callback must not).
    static BUSY: Cell<bool> = const { Cell::new(false) };
    /// Kit events, handled in order once the app is free.
    static QUEUE: Cell<VecDeque<RawEvent>> = const { Cell::new(VecDeque::new()) };
}

fn queue_push(ev: RawEvent) {
    QUEUE.with(|q| {
        let mut v = q.take();
        v.push_back(ev);
        q.set(v);
    });
}

fn queue_pop() -> Option<RawEvent> {
    QUEUE.with(|q| {
        let mut v = q.take();
        let ev = v.pop_front();
        q.set(v);
        ev
    })
}

struct RawEvent {
    window: usize,
    node: *mut sys::BKNode,
    kind: c_int,
    text: String,
    a: f64,
    b: f64,
}

/// The application state (gpui's `App`).
pub struct App {
    windows: Vec<Option<Box<Window>>>,
    entities: Vec<Slot>,
    headless: bool,
    globals: HashMap<TypeId, Box<dyn Any>>,
    propagate: bool,
    /// Work sent from other threads ([`UiSender`]).
    remote: Option<(std::sync::mpsc::Sender<RemoteJob>, std::sync::mpsc::Receiver<RemoteJob>)>,
    /// Called when the user closes a window.
    close_hooks: Vec<Box<dyn FnMut(WindowId, &mut App)>>,
    /// Counts `notify_all` calls (see `WindowHandle::update`).
    notified: u64,
}

type RemoteJob = Box<dyn FnOnce(&mut App) + Send>;

/// Runs closures on the UI thread from any thread ([`App::ui_sender`]):
/// the UI loop wakes, runs them in order, then re-renders what they changed.
#[derive(Clone)]
pub struct UiSender {
    tx: std::sync::mpsc::Sender<RemoteJob>,
}

impl UiSender {
    /// Queue `f` for the UI thread. False when the app has ended.
    pub fn send(&self, f: impl FnOnce(&mut App) + Send + 'static) -> bool {
        let ok = self.tx.send(Box::new(f)).is_ok();
        // SAFETY: posts a message; callable from any thread.
        unsafe { sys::BK_window_wake() };
        ok
    }
}

unsafe extern "C" fn on_wake() {
    let ran = with_running_app(|app| {
        app.drain();
        app.flush();
    });
    if !ran {
        // a callback further up holds the app: try again on the next turn
        unsafe { sys::BK_window_wake() };
    }
}

impl App {
    fn create(headless: bool) -> App {
        App { windows: Vec::new(), entities: Vec::new(), headless, globals: HashMap::new(), propagate: true,
              remote: None, close_hooks: Vec::new(), notified: 0 }
    }

    /// Create an entity (gpui's `cx.new`).
    pub fn new<T: 'static>(&mut self, build: impl FnOnce(&mut Context<T>) -> T) -> Entity<T> {
        let id = self.entities.len() as u32;
        self.entities.push(Slot { generation: 0, value: None });
        let entity = Entity { id, generation: 0, _t: PhantomData };
        let v = build(&mut Context { app: self, entity });
        self.entities[id as usize].value = Some(Box::new(v));
        entity
    }

    /// Drop an entity's value. Its handles become dead: `WeakEntity::update`
    /// skips them, `Entity::read` / `update` panic.
    pub fn release<T: 'static>(&mut self, entity: Entity<T>) {
        if let Some(slot) = self.entities.get_mut(entity.id as usize).filter(|s| s.generation == entity.generation) {
            slot.value = None;
            slot.generation += 1;
        }
    }

    /// Open a window whose root view is built by `build`.
    pub fn open_window<V: Render>(
        &mut self,
        options: WindowOptions,
        build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> Result<WindowHandle<V>, String> {
        let bounds = match options.window_bounds {
            Some(WindowBounds::Windowed(b) | WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b)) => b,
            None => Bounds { origin: point(px(0.0), px(0.0)), size: size(px(1024.0), px(700.0)) },
        };
        let title = options.titlebar.as_ref().and_then(|t| t.title.clone())
            .map(|t| t.to_string()).unwrap_or_else(|| "BUI".into());
        let id = self.windows.len();
        let mut w = Window::create(id, &title, bounds.size.width.0, bounds.size.height.0, self.headless)?;
        if let Some(n) = &w.native {
            unsafe {
                if let Some(WindowBounds::Maximized(_)) = options.window_bounds {
                    sys::BK_window_wm(*n, c"state".as_ptr(), 1.0, 0.0);
                }
                if options.is_resizable == Some(false) {
                    sys::BK_window_wm(*n, c"resizable".as_ptr(), 0.0, 0.0);
                }
                if let Some(m) = options.window_min_size {
                    sys::BK_window_wm(*n, c"minsize".as_ptr(), (m.width.0 * w.scale) as f64,
                                      (m.height.0 * w.scale) as f64);
                }
                if options.gpu == Some(true) {
                    sys::BK_window_set_gpu(*n, 1);
                }
            }
        }
        let entity = build(&mut w, self);
        let view = entity;
        w.view = Some(Box::new(move |window: &mut Window, cx: &mut App| {
            view.update(cx, |v, cx| v.render(window, cx).into_any_element())
        }));
        w.dirty = true;
        self.windows.push(Some(Box::new(w)));
        if !self.headless {
            self.with_window(id, |w, app| w.rebuild(app));
            if let Some(n) = self.window(WindowId(id)).and_then(|w| w.native) {
                unsafe { sys::BK_window_show(n) };
            }
        }
        Ok(WindowHandle { id: WindowId(id), entity })
    }

    /// Mark every window for re-render.
    pub fn notify_all(&mut self) {
        self.notified = self.notified.wrapping_add(1);
        for w in self.windows.iter_mut().flatten() {
            w.refresh();
        }
    }

    /// A handle other threads use to run code on this (the UI) thread.
    pub fn ui_sender(&mut self) -> UiSender {
        if self.remote.is_none() {
            self.remote = Some(std::sync::mpsc::channel());
            if !self.headless {
                unsafe { sys::BK_window_wake_init(Some(on_wake)) };
            }
        }
        UiSender { tx: self.remote.as_ref().map(|(tx, _)| tx.clone()).unwrap() }
    }

    /// Run work queued by [`UiSender`]s and re-render what changed (for a
    /// loop that isn't the native one, e.g. headless apps driven by
    /// another thread).
    pub fn run_pending(&mut self) {
        self.drain();
        self.flush();
    }

    /// Call `f` whenever the user closes a window.
    pub fn on_window_closed(&mut self, f: impl FnMut(WindowId, &mut App) + 'static) {
        self.close_hooks.push(Box::new(f));
    }

    /// Close a window (as if the user had).
    pub fn close_window(&mut self, id: WindowId) {
        if let Some(w) = self.windows.get_mut(id.0).and_then(|w| w.as_deref_mut()) {
            w.closed = true;
            if let Some(n) = w.native {
                unsafe { sys::BK_window_close(n) };
            }
        }
    }

    /// Stop the event loop.
    pub fn quit(&mut self) {
        unsafe { sys::BK_window_quit() };
    }

    /// Stop the current event from reaching ancestors.
    pub fn stop_propagation(&mut self) {
        self.propagate = false;
    }

    /// Activate the app (no-op: windows come to the front when shown).
    pub fn activate(&mut self, _ignoring_other_apps: bool) {}

    pub fn set_global<G: 'static>(&mut self, g: G) {
        self.globals.insert(TypeId::of::<G>(), Box::new(g));
    }
    pub fn global<G: 'static>(&self) -> &G {
        self.try_global().expect("global not set")
    }
    pub fn try_global<G: 'static>(&self) -> Option<&G> {
        self.globals.get(&TypeId::of::<G>()).and_then(|g| g.downcast_ref())
    }
    pub fn global_mut<G: 'static>(&mut self) -> &mut G {
        self.globals.get_mut(&TypeId::of::<G>()).and_then(|g| g.downcast_mut()).expect("global not set")
    }

    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.get(id.0)?.as_deref()
    }

    fn with_window<R>(&mut self, id: usize, f: impl FnOnce(&mut Window, &mut App) -> R) -> Option<R> {
        let mut w = self.windows.get_mut(id)?.take()?;
        let r = f(&mut w, self);
        self.windows[id] = Some(w);
        Some(r)
    }

    /// Handle queued kit events, then re-render what changed.
    fn drain(&mut self) {
        while let Some(ev) = queue_pop() {
            self.with_window(ev.window, |w, app| w.dispatch(ev, app));
        }
        // work from other threads (UiSender)
        loop {
            let job = match &self.remote {
                Some((_, rx)) => rx.try_recv().ok(),
                None => None,
            };
            match job {
                Some(f) => f(self),
                None => break,
            }
        }
    }

    /// Re-render dirty windows now (headless) or soon (native: next loop turn).
    fn flush(&mut self) {
        self.drain();
        // windows whose signals were written (hooks)
        for id in crate::hooks::take_dirty() {
            if id == usize::MAX {
                self.notify_all();
            } else if let Some(w) = self.windows.get_mut(id).and_then(|w| w.as_deref_mut()) {
                w.refresh();
            }
        }
        for id in 0..self.windows.len() {
            let Some(w) = self.windows[id].as_deref_mut() else { continue };
            if !w.dirty {
                continue;
            }
            match w.native {
                // animation: rebuild right before the next (vsync-paced) render
                Some(n) if w.animation_frame => {
                    w.animation_frame = false;
                    w.frame_pending = true;
                    unsafe { sys::BK_window_request_frame(n, c_int::from(w.vsync)) };
                }
                // the requested frame rebuilds the window at its paced time
                Some(n) if !w.timer_pending && !w.frame_pending => {
                    w.timer_pending = true;
                    unsafe { sys::BK_window_timer(n, 0) };
                }
                Some(_) => {}
                None => {
                    self.with_window(id, |w, app| w.rebuild(app));
                }
            }
        }
    }

    fn shutdown(&mut self) {
        for w in self.windows.iter_mut().flatten() {
            w.destroy();
        }
        self.windows.clear();
    }

    // -- automation (tests, screenshots) --------------------------------------------------

    /// Render a window off-screen: (width, height, 0x00RRGGBB pixels) in device pixels.
    /// Images it shows are decoded first (the frame waits for the kit's
    /// image workers), so the pixels are deterministic.
    pub fn draw(&mut self, id: WindowId) -> (usize, usize, Vec<u32>) {
        self.flush();
        let Some(w) = self.windows.get_mut(id.0).and_then(|w| w.as_deref_mut()) else {
            return (0, 0, Vec::new());
        };
        let (pw, ph) = w.phys;
        // SAFETY: the raster is sized to the window; pixels are copied out.
        let px = unsafe {
            if w.raster.is_null() {
                w.raster = sys::BUI_raster_create(pw, ph);
            }
            let _s = span(Phase::Widgets);
            sys::BK_render(w.ctx, w.raster, pw, ph, 1);
            // decodes this frame asked for: wait, then draw what arrived
            let mut st = sys::BKImageStats::default();
            sys::BK_image_stats(&raw mut st);
            for _ in 0..10_000 {
                if st.pending == 0 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_micros(200));
                sys::BK_image_stats(&raw mut st);
            }
            if sys::BK_image_poll() > 0 {
                sys::BK_render(w.ctx, w.raster, pw, ph, 0);
            }
            let p = sys::BUI_raster_pixels(w.raster);
            std::slice::from_raw_parts(p, (pw * ph) as usize).to_vec()
        };
        self.flush(); // configure events from the render
        (pw as usize, ph as usize, px)
    }

    /// Move every window's CSS animation clock forward by `d`. Headless
    /// windows' clocks stand still otherwise, so tests step animations and
    /// transitions deterministically; native windows add `d` to real time.
    pub fn advance_clock(&mut self, d: std::time::Duration) {
        for w in self.windows.iter_mut().flatten() {
            w.clock.advance(d);
            if w.caches.anim.any() {
                w.dirty = true;
            }
            // a scrollbar's hold or a tooltip's wait ended (headless windows have no timers)
            w.timer_fired();
        }
        self.flush();
    }

    /// Bounds (logical pixels) of the element with `.id(id)`.
    pub fn element_bounds(&mut self, window: WindowId, id: &str) -> Option<Bounds<f32>> {
        self.flush();
        let w = self.window(window)?;
        let f = w.frame.as_ref()?;
        f.by_key().get(&id_key(id)).map(|&i| f.nodes[i].bounds)
    }

    fn input(&mut self, window: WindowId, f: impl FnOnce(*mut sys::BKContext, f32)) {
        self.flush();
        if let Some(w) = self.window(window) {
            if w.frame.is_none() {
                return;
            }
            f(w.ctx, w.scale);
        }
        self.flush();
    }

    /// Move the pointer to (x, y) in logical pixels.
    pub fn simulate_mouse_move(&mut self, window: WindowId, x: f32, y: f32) {
        self.input(window, |ctx, s| unsafe {
            sys::BK_motion(ctx, (x * s) as c_int, (y * s) as c_int, 0);
        });
    }

    /// Press and release the left button at (x, y).
    pub fn simulate_click(&mut self, window: WindowId, x: f32, y: f32) {
        self.simulate_mouse_move(window, x, y);
        self.input(window, |ctx, s| unsafe {
            sys::BK_press(ctx, (x * s) as c_int, (y * s) as c_int, 0);
        });
        self.input(window, |ctx, s| unsafe {
            sys::BK_release(ctx, (x * s) as c_int, (y * s) as c_int, sys::BK_BUTTON1);
        });
    }

    /// Click at (x, y) with modifier keys held (`ctrl`, `shift`).
    pub fn simulate_click_with(&mut self, window: WindowId, x: f32, y: f32, ctrl: bool, shift: bool) {
        let mods = c_int::from(shift) | (c_int::from(ctrl) << 1);
        self.simulate_mouse_move(window, x, y);
        self.input(window, |ctx, s| unsafe {
            sys::BK_press(ctx, (x * s) as c_int, (y * s) as c_int, mods);
        });
        self.input(window, |ctx, s| unsafe {
            sys::BK_release(ctx, (x * s) as c_int, (y * s) as c_int, sys::BK_BUTTON1 | mods);
        });
    }

    /// Press at `from`, drag to `to` and release there.
    pub fn simulate_drag(&mut self, window: WindowId, from: (f32, f32), to: (f32, f32)) {
        self.simulate_mouse_move(window, from.0, from.1);
        self.input(window, |ctx, s| unsafe {
            sys::BK_press(ctx, (from.0 * s) as c_int, (from.1 * s) as c_int, 0);
        });
        for k in 1..=4 {
            let t = k as f32 / 4.0;
            let (x, y) = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
            self.input(window, |ctx, s| unsafe {
                sys::BK_motion(ctx, (x * s) as c_int, (y * s) as c_int, sys::BK_BUTTON1);
            });
        }
        self.input(window, |ctx, s| unsafe {
            sys::BK_release(ctx, (to.0 * s) as c_int, (to.1 * s) as c_int, sys::BK_BUTTON1);
        });
    }

    /// A key with modifier keys held.
    pub fn simulate_key_with(&mut self, window: WindowId, text: &str, keysym: &str, ctrl: bool, shift: bool) {
        let mods = c_int::from(shift) | (c_int::from(ctrl) << 1);
        let (t, k) = (CString::new(text).unwrap_or_default(), CString::new(keysym).unwrap_or_default());
        self.input(window, |ctx, _| unsafe {
            sys::BK_key(ctx, t.as_ptr(), k.as_ptr(), mods);
        });
    }

    /// Scroll the wheel at (x, y): positive `lines` scroll the content up.
    pub fn simulate_scroll(&mut self, window: WindowId, x: f32, y: f32, lines: i32) {
        self.simulate_mouse_move(window, x, y);
        self.input(window, |ctx, s| unsafe {
            sys::BK_wheel(ctx, (x * s) as c_int, (y * s) as c_int, lines, 0);
        });
    }

    /// Type text / press a key (`keysym` as in Tk: "Return", "BackSpace", "a" ...).
    pub fn simulate_key(&mut self, window: WindowId, text: &str, keysym: &str) {
        let (t, k) = (CString::new(text).unwrap_or_default(), CString::new(keysym).unwrap_or_default());
        self.input(window, |ctx, _| unsafe {
            sys::BK_key(ctx, t.as_ptr(), k.as_ptr(), 0);
        });
    }
}

// ==========================================================================
// Kit callbacks

unsafe extern "C" fn on_emit(data: *mut c_void, node: *mut sys::BKNode, kind: c_int, ref_: *mut c_void,
                             a: c_double, b: c_double) {
    let text = if ref_.is_null() || kind < sys::BK_EV_PRESS {
        String::new()
    } else {
        // SAFETY: raw events carry a C string in `ref`.
        unsafe { CStr::from_ptr(ref_ as *const c_char) }.to_string_lossy().into_owned()
    };
    queue_push(RawEvent { window: data as usize, node, kind, text, a, b });
    with_running_app(|app| app.flush());
}

unsafe extern "C" fn on_timer(data: *mut c_void, _win: *mut sys::BKWindow, _id: c_int) {
    let id = data as usize;
    let ran = with_running_app(|app| {
        app.drain();
        app.with_window(id, |w, app| {
            w.timer_pending = false;
            w.timer_fired();
            if w.dirty {
                w.rebuild(app);
            }
        });
        app.flush();
    });
    if !ran {
        // the app is busy (re-entrant callback): try again next turn
        unsafe { sys::BK_window_timer(_win, 1) };
    }
}

unsafe extern "C" fn on_frame(data: *mut c_void, _win: *mut sys::BKWindow) {
    // BUI_PROFILE: wall time between frames, i.e. rebuild + kit render + present
    thread_local! { static LAST: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) }; }
    if prof::on() {
        let now = std::time::Instant::now();
        if let Some(t) = LAST.with(|l| l.replace(Some(now))) {
            prof::add(Phase::FrameInterval, (now - t).as_secs_f64() * 1000.0);
        }
    }
    let id = data as usize;
    with_running_app(|app| {
        app.drain();
        app.with_window(id, |w, app| {
            w.frame_pending = false;
            if w.dirty {
                w.rebuild(app);
            }
        });
        app.flush();
    });
}

unsafe extern "C" fn on_close(data: *mut c_void, _win: *mut sys::BKWindow) -> c_int {
    let id = data as usize;
    with_running_app(|app| {
        if let Some(w) = app.windows.get_mut(id).and_then(|w| w.as_deref_mut()) {
            w.closed = true;
        }
        let mut hooks = std::mem::take(&mut app.close_hooks);
        for h in hooks.iter_mut() {
            h(WindowId(id), app);
        }
        hooks.append(&mut app.close_hooks);
        app.close_hooks = hooks;
    });
    1
}

/// Run `f` with the app, unless there is none (before the loop, headless)
/// or a callback further up the stack already holds it.
fn with_running_app(f: impl FnOnce(&mut App)) -> bool {
    let app = APP.with(Cell::get);
    if app.is_null() || BUSY.with(Cell::get) {
        return false;
    }
    BUSY.with(|b| b.set(true));
    // SAFETY: `app` points at the live App on `Application::run`'s stack,
    // which isn't touched while the loop runs; BUSY keeps this `&mut` unique.
    f(unsafe { &mut *app });
    BUSY.with(|b| b.set(false));
    true
}

unsafe extern "C" fn noop(_: *mut c_void) {}

// ==========================================================================
// Windows

type ViewFn = Box<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// A window (gpui's `Window`): its kit context, layout and interaction state.
pub struct Window {
    id: usize,
    ctx: *mut sys::BKContext,
    canvas: *mut sys::BKNode,
    root: *mut sys::BKNode,
    opaque: bool,
    native: Option<*mut sys::BKWindow>,
    raster: *mut sys::BuiRaster,
    phys: (i32, i32),
    scale: f32,
    view: Option<ViewFn>,
    frame: Option<Frame>,
    /// Styles, text widths, images, the author sheet and the taffy tree:
    /// kept across frames.
    caches: LayoutCaches,
    /// The display list, reused every frame.
    painter: Painter,
    state: UiState,
    widgets: Widgets,
    dirty: bool,
    timer_pending: bool,
    frame_pending: bool,
    animation_frame: bool,
    /// CSS animation time (virtual in headless windows).
    clock: crate::anim::Clock,
    vsync: bool,
    closed: bool,
    title: String,
    cursor: i32,
    pointer: (f32, f32),
    pressed: Vec<Key>,
    press_button: MouseButton,
    /// The overlay scrollbars' state (`crate::scrollbar`).
    bars: crate::scrollbar::Bars,
    /// When the next timer fires (clock seconds): a scrollbar's hold ends,
    /// a tooltip's wait.
    due: Option<f64>,
    /// The tooltip (`crate::tooltip`).
    tips: crate::tooltip::Tips,
}

impl Window {
    fn create(id: usize, title: &str, w: f32, h: f32, headless: bool) -> Result<Window, String> {
        // SAFETY: building a fresh kit tree; pointers stay owned by this window.
        unsafe {
            let root = sys::BK_node_new(c"frame".as_ptr());
            let canvas = sys::BK_node_new(c"canvas".as_ptr());
            if root.is_null() || canvas.is_null() {
                return Err("kit nodes unavailable".into());
            }
            sys::BK_node_add(root, canvas, -1);
            sys::BK_geom_set_manager(canvas, sys::BK_GEOM_PLACE);
            sys::BK_geom_set(canvas, c"relwidth".as_ptr(), 1.0);
            sys::BK_geom_set(canvas, c"relheight".as_ptr(), 1.0);
            sys::BK_set_num(canvas, c"events".as_ptr(), 1.0);
            let ctx = sys::BK_context_new(root);
            let host = sys::BKHost {
                data: id as *mut c_void,
                request: Some(noop),
                set_cursor: None,
                warp: None,
                clipboard_get: None,
                clipboard_set: None,
                schedule: None,
                emit: Some(on_emit),
            };
            sys::BK_context_set_host(ctx, &host);
            let mut win = Window {
                id,
                ctx,
                canvas,
                root,
                opaque: false,
                native: None,
                raster: std::ptr::null_mut(),
                phys: (w.round() as i32, h.round() as i32),
                scale: 1.0,
                view: None,
                frame: None,
                caches: LayoutCaches::default(),
                painter: Painter::new(),
                state: UiState::default(),
                widgets: Widgets::new(),
                dirty: true,
                timer_pending: false,
                frame_pending: false,
                animation_frame: false,
                clock: crate::anim::Clock::new(!headless),
                vsync: true,
                closed: false,
                title: title.to_owned(),
                cursor: 0,
                pointer: (-1.0, -1.0),
                pressed: Vec::new(),
                press_button: MouseButton::Left,
                bars: crate::scrollbar::Bars::default(),
                due: None,
                tips: crate::tooltip::Tips::default(),
            };
            if headless {
                win.scale = sys::BK_scale().max(1.0);
                win.phys = ((w * win.scale).round() as i32, (h * win.scale).round() as i32);
            } else {
                let t = CString::new(title).unwrap_or_default();
                let n = sys::BK_window_new(ctx, t.as_ptr(), w.round() as c_int, h.round() as c_int);
                if n.is_null() {
                    return Err("could not create a native window".into());
                }
                let cb = sys::BKWindowCallbacks { data: id as *mut c_void, on_timer: Some(on_timer),
                                                  on_close: Some(on_close), on_frame: Some(on_frame) };
                sys::BK_window_set_callbacks(n, &cb);
                win.scale = sys::BK_scale().max(0.5);
                win.phys = ((w * win.scale).round() as i32, (h * win.scale).round() as i32);
                win.native = Some(n);
            }
            win.caches.text.set_scale(win.scale);
            Ok(win)
        }
    }

    fn destroy(&mut self) {
        self.widgets.free_all();
        unsafe {
            if let Some(n) = self.native.take() {
                sys::BK_window_free(n);
            }
            if !self.raster.is_null() {
                sys::BUI_raster_free(self.raster);
                self.raster = std::ptr::null_mut();
            }
        }
    }

    /// Window id.
    pub fn window_handle(&self) -> WindowId {
        WindowId(self.id)
    }

    /// Re-render on the next turn.
    /// Render again on the next display refresh (gpui's
    /// `request_animation_frame`): call it from `render` to animate.
    pub fn request_animation_frame(&mut self) {
        self.animation_frame = true;
        self.dirty = true;
    }

    /// Pace animation frames to the display (default `true`). With `false`
    /// frames render as fast as the CPU allows (benchmarks).
    pub fn set_vsync(&mut self, on: bool) {
        self.vsync = on;
    }

    /// Whether CSS animations or transitions were running at the last
    /// render (the window keeps requesting frames while they are).
    pub fn is_animating(&self) -> bool {
        self.caches.anim.running()
    }

    /// Whether the window will render again (something changed or is
    /// animating). False for an idle window.
    pub fn needs_render(&self) -> bool {
        self.dirty
    }

    /// Frames presented to the native window so far.
    pub fn presented_frames(&self) -> u64 {
        self.native.map_or(0, |n| unsafe { sys::BK_window_frames(n) } as u64)
    }

    pub fn refresh(&mut self) {
        self.dirty = true;
    }

    /// Content size in logical pixels.
    pub fn viewport_size(&self) -> Size<Pixels> {
        size(px(self.phys.0 as f32 / self.scale), px(self.phys.1 as f32 / self.scale))
    }

    pub fn bounds(&self) -> Bounds<Pixels> {
        Bounds { origin: point(px(0.0), px(0.0)), size: self.viewport_size() }
    }

    /// Device pixels per logical pixel.
    pub fn scale_factor(&self) -> f32 {
        self.scale
    }

    /// Whether frames are drawn on the GPU right now (it turns on a few
    /// frames after start, once the device is ready).
    pub fn is_gpu(&self) -> bool {
        // SAFETY: `native` is a live kit window owned by this Window.
        self.native.map(|n| unsafe { sys::BK_window_gpu(n) } != 0).unwrap_or(false)
    }

    pub fn set_window_title(&mut self, title: &str) {
        if self.title != title {
            self.title = title.to_owned();
            if let Some(n) = self.native {
                let t = CString::new(title).unwrap_or_default();
                unsafe { sys::BK_window_set_title(n, t.as_ptr()) };
            }
        }
    }

    /// Pointer position in logical pixels.
    pub fn mouse_position(&self) -> crate::geometry::Point<Pixels> {
        point(px(self.pointer.0), px(self.pointer.1))
    }

    /// The `.id()` of the focused element, if any.
    pub fn focused_id(&self) -> Option<&str> {
        self.state.focused_id.as_deref()
    }

    /// Focus the element with `.id(id)` (keyboard events go to it). A
    /// widget (`<textarea>`, `<input>` ...) also gets the kit's focus, so it
    /// takes typed text again after it gave the focus up (Escape).
    pub fn focus_id(&mut self, id: &str) {
        let key = id_key(id);
        self.state.focused = Some(key);
        self.state.focus_path = [key].into_iter().collect();
        self.state.focused_id = Some(id.to_owned());
        if let Some(w) = self.widgets.nodes.get(&key) {
            // SAFETY: a live widget node of this window's kit context.
            unsafe { sys::BK_focus_node(self.ctx, w.node) };
        }
        self.dirty = true;
    }

    /// The caret and selection of the `<textarea>` with `.id(id)`, as of
    /// now (key events reach `on_key_down` after the textarea handled them).
    pub fn textarea_caret(&self, id: &str) -> Option<TextCaret> {
        let n = self.widgets.textarea(id_key(id))?;
        let (mut a, mut b) = (0, 0);
        // SAFETY: a live textarea node of this window; out pointers valid for the call.
        unsafe {
            let caret = sys::BK_textarea_caret(n);
            let anchor = match sys::BK_textarea_selection(n, &mut a, &mut b) {
                0 => caret,
                _ if a == caret => b,
                _ => a,
            };
            Some(TextCaret { anchor: anchor as usize, caret: caret as usize })
        }
    }

    /// Move the caret of the `<textarea>` with `.id(id)` and select from
    /// `to.anchor` (or nothing), scrolling the caret into view. Offsets are
    /// clamped to the text and moved back to a character boundary; Up and
    /// Down go on from the new column. False when there is no such textarea.
    pub fn set_textarea_caret(&mut self, id: &str, to: TextCaret) -> bool {
        let Some(n) = self.widgets.textarea(id_key(id)) else { return false };
        // SAFETY: a live textarea node; the text is read before the kit changes anything.
        unsafe {
            let text = CStr::from_ptr(sys::BK_get_str(n, c"value".as_ptr())).to_bytes();
            let fit = |mut at: usize| {
                at = at.min(text.len());
                while at > 0 && at < text.len() && (text[at] & 0xc0) == 0x80 {
                    at -= 1;
                }
                at as c_int
            };
            let (anchor, caret) = (fit(to.anchor), fit(to.caret));
            sys::BK_textarea_set_selection(n, if anchor == caret { -1 } else { anchor }, caret);
            sys::BK_textarea_see(n, caret);
        }
        true
    }

    /// Is the element with `.id(id)` hovered?
    pub fn is_hovered(&self, id: &str) -> bool {
        self.state.hovered.contains(&id_key(id))
    }

    /// Scroll offset of a scroll container with `.id(id)`.
    /// How long the pointer rests on an element with a `title` before its
    /// tooltip shows ([`crate::tooltip::DELAY`] unless set).
    pub fn set_tooltip_delay(&mut self, delay: std::time::Duration) {
        self.tips.delay = delay.as_secs_f64();
    }

    /// Let tooltips show (the default), or hide them and wait for none,
    /// say while a menu is open. Call it while rendering: it applies to the
    /// frame being built.
    pub fn set_tooltips(&mut self, on: bool) {
        self.tips.on = on;
    }

    /// Hide the tooltip until the pointer leaves its element, as a click or
    /// a key does.
    pub fn hush_tooltip(&mut self) {
        if self.tips.hush() {
            self.dirty = true;
        }
    }

    /// The text of the tooltip showing, if one is.
    pub fn tooltip(&self) -> Option<&str> {
        self.tips.at?;
        let f = self.frame.as_ref()?;
        f.attr(*f.by_key().get(&self.tips.key()?)?, "title")
    }

    pub fn scroll_offset(&self, id: &str) -> (f32, f32) {
        self.state.scroll.get(&id_key(id)).copied().unwrap_or((0.0, 0.0))
    }

    pub fn set_scroll_offset(&mut self, id: &str, x: f32, y: f32) {
        self.state.scroll.insert(id_key(id), (x, y));
        self.dirty = true;
    }

    /// Bounds (logical pixels) of the element with `.id(id)` as of the last
    /// frame. Works while rendering (views read where they were last
    /// placed); inside an event listener the frame is lent out, so a view
    /// keeps what it read while rendering.
    pub fn bounds_of(&self, id: &str) -> Option<Bounds<f32>> {
        let f = self.frame.as_ref()?;
        f.by_key().get(&id_key(id)).map(|&i| f.nodes[i].bounds)
    }

    /// Put `text` on the clipboard.
    pub fn write_clipboard(&mut self, text: &str) {
        let c = CString::new(text.replace('\0', "")).unwrap_or_default();
        // SAFETY: a live context and a NUL-terminated string the kit copies.
        unsafe { sys::BK_clipboard_set(self.ctx, c.as_ptr()) };
    }

    /// One UI unit (Blender's `UI_UNIT_Y`, a row of widgets) in logical pixels.
    pub fn ui_unit(&self) -> f32 {
        // SAFETY: a constant query.
        unsafe { sys::BK_unit() as f32 / self.scale }
    }

    fn viewport(&self) -> (f32, f32) {
        (self.phys.0 as f32 / self.scale, self.phys.1 as f32 / self.scale)
    }

    // -- render -----------------------------------------------------------------------------

    fn rebuild(&mut self, app: &mut App) {
        // the view is moved out while it renders (it gets `&mut Window`)
        let Some(view) = self.view.take() else { return };
        self.dirty = false;
        self.bars.lists.clear(); // the lists rendered now register again
        let root = {
            let _s = span(Phase::Render);
            crate::hooks::begin(self.id);
            let mut root = view(self, app);
            crate::element::resolve_deferred(&mut root, self, app);
            root
        };
        if self.view.is_none() {
            self.view = Some(view);
        }
        let vp = self.viewport();
        let (canvas, scale) = (self.canvas, self.scale);
        let widgets = &mut self.widgets;
        let recycle = self.frame.take().unwrap_or_default();
        let tooltip = self.tips.update(self.clock.now(), self.pointer).then(|| {
            let f = recycle.by_key().get(&self.tips.key()?).copied()?;
            recycle.attr(f, "title").map(crate::tooltip::element)
        }).flatten();
        self.caches.anim.begin(self.clock.now());
        let mut frame = layout(root, tooltip, vp, &self.state, &mut self.caches, recycle,
                               &mut |key, spec| widgets.ensure(canvas, key, spec, scale));
        self.place_tooltip(&mut frame, vp);
        // CSS animations / transitions still moving: render again next frame
        // (nothing running: no frames, so an idle window costs no CPU)
        let animating = self.caches.anim.end();
        // `@container`: sizes changed under a query: render once more
        let container_changed = self.caches.after_layout(&frame);
        let p = &mut self.painter;
        p.reset();
        let (now, reduced) = (self.clock.now(), crate::reduced_motion());
        {
            let _s = span(Phase::Paint);
            crate::scrollbar::with_looks(&self.bars, now, reduced, || {
                paint_frame(&frame, &self.caches.styles, &mut self.caches.text, p, vp, self.scale, app);
            });
        }
        {
            let _s = span(Phase::Flush);
            p.flush(canvas, scale);
        }
        let _rest = span(Phase::Widgets);
        let mut at = HashMap::new(); // empty (no allocation) without form widgets
        for n in &frame.nodes {
            if matches!(n.content, Content::Widget(_)) {
                at.insert(n.key, (n.bounds, n.clip, n.visible));
            }
        }
        self.widgets.place(&at, scale);
        // an opaque <html> background covers the window: the kit can skip its clear
        let styles = &self.caches.styles;
        let opaque = frame.nodes.first()
            .is_some_and(|h| styles[h.style].background.a >= 1.0 && styles[h.style].opacity >= 1.0);
        if opaque != self.opaque {
            self.opaque = opaque;
            unsafe { sys::BK_set_num(self.root, c"opaque".as_ptr(), opaque as i32 as f64) };
        }
        if let Some(t) = frame.title.clone() {
            self.set_window_title(&t);
        }
        self.frame = Some(frame);
        self.update_hover(app, false);
        drop(_rest);
        prof::frame_done();
        // state of components that didn't render is freed; effects run now
        crate::hooks::end();
        if container_changed {
            self.dirty = true;
        }
        if animating {
            self.request_animation_frame();
        }
        self.wake_for_bars(now, reduced);
        if let Some(due) = self.tips.due() {
            self.wake_at(due);
        }
    }

    // -- timers ------------------------------------------------------------------------------

    /// Draw again at clock time `due` (a timer; headless windows wait for
    /// `App::advance_clock`). An earlier timer on its way is kept: the frame
    /// it draws asks again.
    fn wake_at(&mut self, due: f64) {
        if self.due.is_some_and(|d| d <= due + 0.001) {
            return;
        }
        self.due = Some(due);
        if let Some(n) = self.native {
            let ms = ((due - self.clock.now()) * 1000.0).ceil().max(0.0) as c_int + 1;
            // SAFETY: a live native window; the timer comes back through on_timer.
            unsafe { sys::BK_window_timer(n, ms) };
        }
    }

    /// A timer fired (or the clock moved): draw if one was due.
    fn timer_fired(&mut self) {
        if self.due.is_some_and(|due| due <= self.clock.now() + TIMER_SLOP) {
            self.due = None;
            self.dirty = true;
        }
    }

    // -- tooltips ----------------------------------------------------------------------------

    /// Put the tooltip laid out in `frame` (at the window's corner) where it
    /// hangs from the pointer.
    fn place_tooltip(&self, frame: &mut Frame, vp: (f32, f32)) {
        let Some(at) = self.tips.at else { return };
        let Some(&i) = frame.by_key().get(&crate::layout::id_key(crate::tooltip::KEY)) else { return };
        let b = frame.nodes[i].bounds;
        let (x, y) = crate::tooltip::place(at, (b.size.width, b.size.height), vp);
        frame.shift(i, x - b.origin.x, y - b.origin.y);
    }

    /// The pointer moved, or the frame changed under it: follow the titled
    /// element it is on.
    fn hover_tooltip(&mut self, path: &[Key]) {
        let Some(f) = &self.frame else { return };
        let titled = path.iter().rev().copied()
            .find(|k| f.by_key().get(k).is_some_and(|&i| f.attr(i, "title").is_some_and(|t| !t.is_empty())));
        if self.tips.set_over(titled, self.clock.now()) {
            self.dirty = true;
        }
        if let Some(due) = self.tips.due() {
            self.wake_at(due);
        }
    }

    // -- overlay scrollbars ------------------------------------------------------------------

    /// The user scrolled the area `key`: show its overlay scrollbar.
    pub(crate) fn poke_scrollbar(&mut self, key: Key) {
        self.bars.poke(key, self.clock.now());
        self.dirty = true;
    }

    /// A virtual list's scrollbar this frame (`key`: its element's; whether
    /// its content overflows; whether its thumb is dragged), registered so
    /// the pointer near its edge shows it.
    pub(crate) fn list_scrollbar(&mut self, key: Key, overflows: bool, dragged: bool) -> crate::scrollbar::Overlay {
        self.bars.lists.insert(key, overflows);
        if !overflows {
            return crate::scrollbar::Overlay { alpha: 0.0, hot: false };
        }
        let now = self.clock.now();
        if dragged {
            self.bars.poke(key, now);
            return crate::scrollbar::Overlay { alpha: 1.0, hot: true };
        }
        self.bars.look(key, now, crate::reduced_motion())
    }

    /// Draw again when the scrollbars need it: the next frame while one
    /// fades, a timer at the end of a hold, nothing while idle.
    fn wake_for_bars(&mut self, now: f64, reduced: bool) {
        match self.bars.wake(now, reduced) {
            crate::scrollbar::Wake::Idle => {}
            crate::scrollbar::Wake::Frame => self.request_animation_frame(),
            crate::scrollbar::Wake::After(s) => self.wake_at(now + s),
        }
    }

    /// The scroll area whose right edge the pointer at (`x`, `y`) is near,
    /// innermost first, and whether it is over the thumb's lane.
    fn bar_under(&self, x: f32, y: f32) -> Option<(Key, bool)> {
        let f = self.frame.as_ref()?;
        for key in self.path_at(x, y).iter().rev() {
            let Some(&i) = f.by_key().get(key) else { continue };
            let n = &f.nodes[i];
            if n.scroll_range.1 <= 0.0 && self.bars.lists.get(key) != Some(&true) {
                continue;
            }
            let right = n.bounds.origin.x + n.bounds.size.width;
            if x >= right - crate::scrollbar::NEAR && x < right {
                return Some((*key, x >= right - crate::scrollbar::LANE));
            }
        }
        None
    }

    /// The pointer moved: show the scrollbar it is near.
    fn hover_bars(&mut self) {
        let (x, y) = self.pointer;
        let under = if x < 0.0 { None } else { self.bar_under(x, y) };
        if self.bars.set_hover(under, self.clock.now()) {
            self.dirty = true;
        }
    }

    /// A press on a scroll container's thumb grabs it (true: the press is
    /// the scrollbar's).
    fn grab_bar(&mut self, y: f32) -> bool {
        let Some((key, true)) = self.bars.hover else { return false };
        let Some(f) = &self.frame else { return false };
        let Some(&i) = f.by_key().get(&key) else { return false };
        let n = &f.nodes[i];
        let Some((ty, th)) = crate::scrollbar::thumb(n.bounds.size.height, n.scroll_range.1, n.scroll_pos.1) else {
            return false; // a virtual list's: it grabs its own
        };
        let top = n.bounds.origin.y + ty;
        if y < top || y >= top + th {
            return false;
        }
        self.bars.drag = Some((key, y - top));
        self.poke_scrollbar(key);
        true
    }

    /// The dragged thumb follows the pointer.
    fn drag_bar(&mut self, y: f32) {
        let (Some((key, grab)), Some(f)) = (self.bars.drag, &self.frame) else { return };
        let Some(&i) = f.by_key().get(&key) else { return };
        let n = &f.nodes[i];
        let (view, range) = (n.bounds.size.height, n.scroll_range.1);
        let Some((_, th)) = crate::scrollbar::thumb(view, range, n.scroll_pos.1) else { return };
        let free = (view - th).max(1.0);
        let sy = ((y - grab - n.bounds.origin.y) / free).clamp(0.0, 1.0) * range;
        if (sy - n.scroll_pos.1).abs() > f32::EPSILON {
            self.state.scroll.insert(key, (n.scroll_pos.0, sy));
        }
        self.poke_scrollbar(key);
    }

    // -- events ------------------------------------------------------------------------------

    /// Element keys from the root to the deepest element under the pointer.
    fn path_at(&self, x: f32, y: f32) -> Vec<Key> {
        let Some(f) = &self.frame else { return Vec::new() };
        f.hit_path(x, y, &self.caches.styles).into_iter().map(|i| f.nodes[i].key).collect()
    }

    /// Call the listeners `pick` selects, from element `key` up to the root.
    /// The frame is moved out of the window meanwhile, so listeners can take
    /// `&mut Window` while the frame lends them out: no copies, no `Rc`.
    fn emit<E: 'static>(&mut self, app: &mut App, key: Key, e: &E,
                        pick: impl Fn(&Listeners) -> Vec<&Listener<E>>) {
        let Some(frame) = self.frame.take() else { return };
        app.propagate = true;
        let mut cur = frame.by_key().get(&key).copied();
        'up: while let Some(i) = cur {
            if let Some(ls) = frame.el(i).and_then(|e| e.listeners.as_deref()) {
                for l in pick(ls) {
                    l(e, self, app);
                    if !app.propagate {
                        break 'up;
                    }
                }
            }
            cur = frame.nodes[i].parent;
        }
        app.propagate = true;
        if self.frame.is_none() {
            self.frame = Some(frame);
        }
    }

    fn update_hover(&mut self, app: &mut App, fire: bool) {
        let path = self.path_at(self.pointer.0, self.pointer.1);
        self.hover_tooltip(&path);
        let now: std::collections::HashSet<Key> = path.iter().copied().collect();
        if now == self.state.hovered {
            return;
        }
        let entered: Vec<Key> = now.difference(&self.state.hovered).copied().collect();
        let left: Vec<Key> = self.state.hovered.difference(&now).copied().collect();
        let restyle = self.frame.as_ref().is_some_and(|f| {
            entered.iter().chain(&left).any(|k| f.by_key().get(k).is_some())
        });
        self.state.hovered = now;
        // cursor of the deepest element that sets one
        if let Some(f) = &self.frame {
            let cursor = path.iter().rev()
                .find_map(|k| f.by_key().get(k).and_then(|&i| self.caches.styles[f.nodes[i].style].cursor)).unwrap_or(0);
            if cursor != self.cursor {
                self.cursor = cursor;
                unsafe { sys::BK_set_num(self.canvas, c"cursor".as_ptr(), cursor as f64) };
            }
        }
        if fire {
            // hover listeners don't bubble: each element hears its own enter / leave
            for (keys, on) in [(left, false), (entered, true)] {
                for k in keys {
                    let Some(frame) = self.frame.take() else { return };
                    let el = frame.by_key().get(&k).and_then(|&i| frame.el(i));
                    if let Some(ls) = el.and_then(|e| e.listeners.as_deref()) {
                        for l in &ls.hover {
                            l(&on, self, app);
                        }
                    }
                    if self.frame.is_none() {
                        self.frame = Some(frame);
                    }
                }
            }
        }
        if restyle {
            self.dirty = true;
        }
    }

    fn pointer_event(&self, text: &str) -> (Modifiers, MouseButton) {
        let mut it = text.split_whitespace().map(|v| v.parse::<i32>().unwrap_or(0));
        let mods = it.next().unwrap_or(0);
        let button = match it.next().unwrap_or(1) {
            2 => MouseButton::Middle,
            3 => MouseButton::Right,
            _ => MouseButton::Left,
        };
        (Modifiers::from_kit(mods), button)
    }

    /// Focus the element `key` (path: its ancestors, for `:focus-within`).
    fn set_focus(&mut self, path: &[Key]) {
        self.state.focused = path.last().copied();
        self.state.focus_path = path.iter().copied().collect();
        self.state.focused_id = self.frame.as_ref().and_then(|f| {
            let i = *f.by_key().get(path.last()?)?;
            f.el(i)?.id.as_ref().map(|s| s.to_string())
        });
    }

    fn dispatch(&mut self, ev: RawEvent, app: &mut App) {
        if ev.node != self.canvas {
            self.widget_event(ev, app);
            return;
        }
        let (x, y) = (ev.a as f32 / self.scale, ev.b as f32 / self.scale);
        match ev.kind {
            sys::BK_EV_CONFIGURE => {
                let phys = (ev.a as i32, ev.b as i32);
                if phys != self.phys && phys.0 > 0 && phys.1 > 0 {
                    self.phys = phys;
                    self.dirty = true;
                }
            }
            sys::BK_EV_MOTION | sys::BK_EV_ENTER => {
                self.pointer = (x, y);
                if self.bars.drag.is_some() {
                    self.drag_bar(y);
                    return;
                }
                self.hover_bars();
                self.update_hover(app, true);
                let (modifiers, _) = self.pointer_event(&ev.text);
                let pressed = (!self.pressed.is_empty()).then_some(self.press_button);
                let e = MouseMoveEvent { position: point(px(x), px(y)), pressed_button: pressed, modifiers };
                if let Some(deepest) = self.state_deepest() {
                    self.emit(app, deepest, &e, |l| l.mouse_move.iter().collect());
                }
            }
            sys::BK_EV_LEAVE => {
                self.pointer = (-1.0, -1.0);
                if self.bars.drag.is_none() {
                    self.hover_bars();
                }
                self.update_hover(app, true);
            }
            sys::BK_EV_PRESS | sys::BK_EV_DOUBLE => {
                self.pointer = (x, y);
                let (modifiers, button) = self.pointer_event(&ev.text);
                if button == MouseButton::Left && self.grab_bar(y) {
                    return;
                }
                self.hush_tooltip();
                let path = self.path_at(x, y);
                self.press_button = button;
                self.state.active = path.iter().copied().collect();
                self.set_focus(&path);
                self.pressed = path.clone();
                self.dirty = true;
                let e = MouseDownEvent { button, position: point(px(x), px(y)), modifiers,
                                         click_count: if ev.kind == sys::BK_EV_DOUBLE { 2 } else { 1 } };
                if let Some(&deepest) = path.last() {
                    self.emit(app, deepest, &e, |l| l.mouse_down.iter()
                        .filter(|(b, _)| b.is_none_or(|b| b == button)).map(|(_, f)| f).collect());
                }
            }
            sys::BK_EV_RELEASE => {
                let (modifiers, button) = self.pointer_event(&ev.text);
                if let Some((key, _)) = self.bars.drag.take() {
                    self.poke_scrollbar(key);
                    self.pointer = (x, y);
                    self.hover_bars();
                    return;
                }
                let path = self.path_at(x, y);
                let up = MouseUpEvent { button, position: point(px(x), px(y)), modifiers };
                if let Some(&deepest) = path.last() {
                    self.emit(app, deepest, &up, |l| l.mouse_up.iter()
                        .filter(|(b, _)| b.is_none_or(|b| b == button)).map(|(_, f)| f).collect());
                }
                // click: the deepest element under both press and release
                let common = self.pressed.iter().zip(&path).take_while(|(a, b)| a == b)
                    .map(|(a, _)| *a).last();
                self.pressed.clear();
                self.state.active.clear();
                self.dirty = true;
                if let (Some(target), MouseButton::Left) = (common, button) {
                    self.toggle_details(target);
                    let down = MouseDownEvent { button, position: point(px(x), px(y)), modifiers, click_count: 1 };
                    let e = ClickEvent { down, up };
                    self.emit(app, target, &e, |l| l.click.iter().collect());
                }
            }
            sys::BK_EV_WHEEL => {
                let (modifiers, _) = self.pointer_event(&ev.text);
                let (px_, py_) = self.pointer;
                let e = ScrollWheelEvent { position: point(px(px_), px(py_)), delta: ev.a as f32, modifiers };
                let path = self.path_at(px_, py_);
                if let Some(&deepest) = path.last() {
                    self.emit(app, deepest, &e, |l| l.scroll.iter().collect());
                }
                self.scroll_by(&path, -(ev.a as f32) * 40.0, modifiers.shift);
            }
            sys::BK_EV_KEY | sys::BK_EV_KEYRELEASE => {
                let mut parts = ev.text.splitn(2, '\t');
                let ch = parts.next().unwrap_or("").to_owned();
                let key = parts.next().unwrap_or("").to_owned();
                let ks = Keystroke { key, key_char: (!ch.is_empty()).then_some(ch),
                                     modifiers: Modifiers::from_kit(ev.a as i32) };
                let target = self.state.focused.or_else(|| self.state_deepest())
                    .unwrap_or(crate::layout::ROOT_KEY);
                if ev.kind == sys::BK_EV_KEY {
                    self.hush_tooltip();
                    self.emit(app, target, &KeyDownEvent { keystroke: ks }, |l| l.key_down.iter().collect());
                } else {
                    self.emit(app, target, &KeyUpEvent { keystroke: ks }, |l| l.key_up.iter().collect());
                }
            }
            _ => {}
        }
    }

    fn state_deepest(&self) -> Option<Key> {
        self.path_at(self.pointer.0, self.pointer.1).last().copied()
    }

    /// `<summary>` clicks open and close their `<details>`.
    fn toggle_details(&mut self, target: Key) {
        let Some(f) = &self.frame else { return };
        let mut cur = f.by_key().get(&target).copied();
        while let Some(i) = cur {
            if f.el(i).is_some_and(|e| e.tag == "summary") {
                let details = f.nodes[i].parent.filter(|&p| f.el(p).is_some_and(|e| e.tag == "details"));
                if let Some(p) = details {
                    let key = f.nodes[p].key;
                    let open = self.state.details_open.get(&key).copied()
                        .unwrap_or(f.attr(p, "open").is_some());
                    self.state.details_open.insert(key, !open);
                    self.dirty = true;
                }
                return;
            }
            cur = f.nodes[i].parent;
        }
    }

    /// Scroll the innermost scroll container along `path` that can move.
    fn scroll_by(&mut self, path: &[Key], dy: f32, horizontal: bool) {
        let Some(f) = &self.frame else { return };
        for key in path.iter().rev() {
            let Some(&i) = f.by_key().get(key) else { continue };
            let n = &f.nodes[i];
            let (rx, ry) = n.scroll_range;
            let (sx, sy) = n.scroll_pos;
            let (nx, ny) = if horizontal || ry <= 0.0 {
                ((sx + dy).clamp(0.0, rx), sy)
            } else {
                (sx, (sy + dy).clamp(0.0, ry))
            };
            let (nx, ny) = snap(f, &self.caches.styles, i, (sx, sy), (nx, ny));
            if (nx, ny) != (sx, sy) {
                self.state.scroll.insert(*key, (nx, ny));
                self.dirty = true;
                let key = *key;
                self.poke_scrollbar(key);
                return;
            }
        }
    }

    fn widget_event(&mut self, ev: RawEvent, app: &mut App) {
        if self.widgets.syncing {
            return;
        }
        let Some(key) = self.widgets.key_of(ev.node) else { return };
        match ev.kind {
            sys::BK_EV_CHANGE => {
                let e = self.widgets.read(key);
                let kind = self.widgets.nodes.get(&key).map(|w| w.spec.kind);
                match kind {
                    Some(WKind::Checkbox) => {
                        self.state.checked.insert(key, e.checked);
                    }
                    Some(WKind::Radio) if e.checked => {
                        self.state.checked.insert(key, true);
                        for other in self.widgets.radio_group(key) {
                            self.state.checked.insert(other, false);
                            self.widgets.set_value(other, 0.0);
                        }
                    }
                    _ => {}
                }
                self.emit(app, key, &e, |l| l.input.iter().collect());
                self.emit(app, key, &e, |l| l.change.iter().collect());
                self.dirty = true;
            }
            sys::BK_EV_CLICK => {
                let (x, y) = self.pointer;
                let down = MouseDownEvent { button: MouseButton::Left, position: point(px(x), px(y)),
                                            modifiers: Modifiers::default(), click_count: 1 };
                let up = MouseUpEvent { button: MouseButton::Left, position: point(px(x), px(y)),
                                        modifiers: Modifiers::default() };
                self.emit(app, key, &ClickEvent { down, up }, |l| l.click.iter().collect());
                self.dirty = true;
            }
            _ => {}
        }
    }
}

/// Marker so `Window` is not `Send` (it holds raw kit pointers).
#[allow(dead_code)]
struct NotSend(PhantomData<*const ()>);

/// `scroll-snap-type`: the scroll position `to` (moving from `from`) of
/// container `i` moved to its children's snap positions: the next one in
/// the direction of travel (`mandatory`), or the nearest when it's within a
/// third of the view (`proximity`).
fn snap(f: &crate::layout::Frame, styles: &crate::layout::StyleStore, i: usize, from: (f32, f32), to: (f32, f32))
        -> (f32, f32) {
    let n = &f.nodes[i];
    let c = &styles[n.style];
    if c.snap_axes == 0 {
        return to;
    }
    let (rx, ry) = n.scroll_range;
    let view = n.bounds;
    let mut out = to;
    for axis in 0..2 {
        if c.snap_axes & (1 << axis) == 0 {
            continue;
        }
        let (cur, target, range, view_len) = if axis == 0 {
            (from.0, to.0, rx, view.size.width)
        } else {
            (from.1, to.1, ry, view.size.height)
        };
        if target == cur {
            continue;
        }
        // snap positions of the children, in scroll coordinates
        let mut points: Vec<f32> = Vec::new();
        for k in f.children_of(i) {
            let kc = &styles[f.nodes[k].style];
            let align = if axis == 0 { kc.snap_align.0 } else { kc.snap_align.1 };
            if align == 0 {
                continue;
            }
            let b = f.nodes[k].bounds;
            let (start, len, vstart) = if axis == 0 {
                (b.origin.x + cur, b.size.width, view.origin.x)
            } else {
                (b.origin.y + cur, b.size.height, view.origin.y)
            };
            let rel = start - vstart;
            let p = match align {
                1 => rel,
                2 => rel + len / 2.0 - view_len / 2.0,
                _ => rel + len - view_len,
            };
            points.push(p.clamp(0.0, range));
        }
        if points.is_empty() {
            continue;
        }
        let forward = target > cur;
        let pick = if c.snap_mandatory {
            if forward {
                points.iter().copied().filter(|p| *p > cur + 0.5).fold(f32::INFINITY, f32::min)
            } else {
                points.iter().copied().filter(|p| *p < cur - 0.5).fold(f32::NEG_INFINITY, f32::max)
            }
        } else {
            let near = points.iter().copied().min_by(|a, b| (a - target).abs().total_cmp(&(b - target).abs()));
            near.filter(|p| (p - target).abs() <= view_len / 3.0).unwrap_or(target)
        };
        let v = if pick.is_finite() { pick } else { target };
        if axis == 0 {
            out.0 = v;
        } else {
            out.1 = v;
        }
    }
    out
}
