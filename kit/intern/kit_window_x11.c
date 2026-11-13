/* Native window host (X11, also Wayland sessions through XWayland; kit_window_wayland.c is the
 * native Wayland host).
 *
 * The same BK_window_* API as kit_window_win32.c, on Xlib: input events go
 * straight to BK_* input (button and key mapping after Blender's
 * GHOST_SystemX11), redraw requests coalesce into one render per loop turn,
 * and the damaged rectangles of the CPU raster are put with XPutImage (or,
 * with BUI_GPU / BK_window_set_gpu, the raster draws with OpenGL and presents
 * through GLX on the same connection).
 * Timers, the cross-thread wake (a pipe) and frame pacing run in the select()
 * loop.
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */

#include "kit_intern.h"

#ifdef BUI_WINDOW_X11

/* With Wayland support this host's API is bk_x11_window_* and kit_window_linux.c picks one. */
#  define BK_WINDOW_X11_HOST
#  include "kit_window_linux.h"

#  include <X11/Xatom.h>
#  include <X11/Xlib.h>
#  include <X11/Xresource.h>
#  include <X11/Xutil.h>
#  include <X11/cursorfont.h>
#  include <X11/keysym.h>

#  include <errno.h>
#  include <fcntl.h>
#  include <math.h>
#  include <stdio.h>
#  include <stdlib.h>
#  include <string.h>
#  include <sys/select.h>
#  include <time.h>
#  include <unistd.h>

#  include <dlfcn.h>

/* libX11 is loaded when the first window opens, not linked: a build with the
 * X11 headers (every Linux wheel) imports fine on servers and containers
 * without X11, and draws native windows wherever X11 (or XWayland) exists.
 * Every Xlib function below goes through a pointer filled by dlsym. */
#  define BK_X11_FUNCS(F)   F(XAllocSizeHints) \
  F(XChangeProperty) \
  F(XCheckTypedWindowEvent) \
  F(XConvertSelection) \
  F(XCreateBitmapFromData) \
  F(XCreateFontCursor) \
  F(XCreateGC) \
  F(XCreateImage) \
  F(XCreatePixmapCursor) \
  F(XCreateWindow) \
  F(XDefineCursor) \
  F(XDestroyWindow) \
  F(XEventsQueued) \
  F(XFlush) \
  F(XFree) \
  F(XFreeGC) \
  F(XFreePixmap) \
  F(XGetSelectionOwner) \
  F(XGetWindowAttributes) \
  F(XGetWindowProperty) \
  F(XGetWMNormalHints) \
  F(XInitThreads) \
  F(XInternAtom) \
  F(XKeysymToString) \
  F(XLookupString) \
  F(XLowerWindow) \
  F(XMapWindow) \
  F(XMoveWindow) \
  F(XNextEvent) \
  F(XOpenDisplay) \
  F(XPeekEvent) \
  F(XPending) \
  F(XPutImage) \
  F(XQueryPointer) \
  F(XRaiseWindow) \
  F(XResizeWindow) \
  F(XResourceManagerString) \
  F(XrmDestroyDatabase) \
  F(XrmGetResource) \
  F(XrmGetStringDatabase) \
  F(XrmInitialize) \
  F(XSendEvent) \
  F(XSetInputFocus) \
  F(XSetSelectionOwner) \
  F(XSetWMNormalHints) \
  F(XSetWMProtocols) \
  F(XStoreName) \
  F(XWarpPointer)

#  define BK_X11_DECL(f) static __typeof__(&f) p_##f;
BK_X11_FUNCS(BK_X11_DECL)

/* 1 when libX11 is loaded and every function resolved. */
static int x11_load(void)
{
  static int state = 0; /* 0 untried, 1 loaded, -1 unavailable */
  if (state) {
    return state > 0;
  }
  void *lib = dlopen("libX11.so.6", RTLD_NOW | RTLD_LOCAL);
  if (!lib) {
    lib = dlopen("libX11.so", RTLD_NOW | RTLD_LOCAL);
  }
  state = -1;
  if (!lib) {
    return 0;
  }
#  define BK_X11_LOAD(f)     if (!(p_##f = (__typeof__(&f))dlsym(lib, #f))) {       return 0;     }
  BK_X11_FUNCS(BK_X11_LOAD)
  state = 1;
  return 1;
}

/* From here on, the Xlib names call through the loaded pointers. */
#  define XAllocSizeHints (*p_XAllocSizeHints)
#  define XChangeProperty (*p_XChangeProperty)
#  define XCheckTypedWindowEvent (*p_XCheckTypedWindowEvent)
#  define XConvertSelection (*p_XConvertSelection)
#  define XCreateBitmapFromData (*p_XCreateBitmapFromData)
#  define XCreateFontCursor (*p_XCreateFontCursor)
#  define XCreateGC (*p_XCreateGC)
#  define XCreateImage (*p_XCreateImage)
#  define XCreatePixmapCursor (*p_XCreatePixmapCursor)
#  define XCreateWindow (*p_XCreateWindow)
#  define XDefineCursor (*p_XDefineCursor)
#  define XDestroyWindow (*p_XDestroyWindow)
#  define XEventsQueued (*p_XEventsQueued)
#  define XFlush (*p_XFlush)
#  define XFree (*p_XFree)
#  define XFreeGC (*p_XFreeGC)
#  define XFreePixmap (*p_XFreePixmap)
#  define XGetSelectionOwner (*p_XGetSelectionOwner)
#  define XGetWindowAttributes (*p_XGetWindowAttributes)
#  define XGetWindowProperty (*p_XGetWindowProperty)
#  define XGetWMNormalHints (*p_XGetWMNormalHints)
#  define XInitThreads (*p_XInitThreads)
#  define XInternAtom (*p_XInternAtom)
#  define XKeysymToString (*p_XKeysymToString)
#  define XLookupString (*p_XLookupString)
#  define XLowerWindow (*p_XLowerWindow)
#  define XMapWindow (*p_XMapWindow)
#  define XMoveWindow (*p_XMoveWindow)
#  define XNextEvent (*p_XNextEvent)
#  define XOpenDisplay (*p_XOpenDisplay)
#  define XPeekEvent (*p_XPeekEvent)
#  define XPending (*p_XPending)
#  define XPutImage (*p_XPutImage)
#  define XQueryPointer (*p_XQueryPointer)
#  define XRaiseWindow (*p_XRaiseWindow)
#  define XResizeWindow (*p_XResizeWindow)
#  define XResourceManagerString (*p_XResourceManagerString)
#  define XrmDestroyDatabase (*p_XrmDestroyDatabase)
#  define XrmGetResource (*p_XrmGetResource)
#  define XrmGetStringDatabase (*p_XrmGetStringDatabase)
#  define XrmInitialize (*p_XrmInitialize)
#  define XSendEvent (*p_XSendEvent)
#  define XSetInputFocus (*p_XSetInputFocus)
#  define XSetSelectionOwner (*p_XSetSelectionOwner)
#  define XSetWMNormalHints (*p_XSetWMNormalHints)
#  define XSetWMProtocols (*p_XSetWMProtocols)
#  define XStoreName (*p_XStoreName)
#  define XWarpPointer (*p_XWarpPointer)

#  define MAX_TIMERS 64

typedef struct Timer {
  int id; /* > 0 user timer, -1 the tooltip timer, 0 free */
  double due;
} Timer;

struct BKWindow {
  Window xid;
  BKContext *ctx;
  BKHost prev;
  BuiRaster *raster;
  BKWindowCallbacks cb;
  XImage *image;
  GC gc;
  int render_pending, frames, cursor, tip_id, next_timer, full;
  int want_frame, frame_vsync;
  int focused; /* FocusIn / FocusOut */
  double frame_start;
  float scale;
  int w, h;
  Timer timers[MAX_TIMERS];
  Cursor cursors[12];
  struct BKWindow *next;
};

static Display *g_dpy = NULL;
static Atom a_delete, a_clipboard, a_utf8, a_targets, a_bui_sel;
static BKWindow *g_windows = NULL;
static int g_open = 0, g_quit = 0, g_keep_alive = 0;
static int (*g_stop)(void) = NULL;
static void (*g_wake_cb)(void) = NULL;
static int g_wake_pipe[2] = {-1, -1};
static char *g_clip_text = NULL; /* what we own on CLIPBOARD */

static double now_ms(void)
{
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return ts.tv_sec * 1000.0 + ts.tv_nsec / 1e6;
}

static int open_display(void)
{
  if (g_dpy) {
    return 1;
  }
  if (!x11_load()) {
    return 0; /* no libX11: headless, or the Tk host from Python */
  }
  XInitThreads();
  g_dpy = XOpenDisplay(NULL);
  if (!g_dpy) {
    return 0;
  }
  a_delete = XInternAtom(g_dpy, "WM_DELETE_WINDOW", False);
  a_clipboard = XInternAtom(g_dpy, "CLIPBOARD", False);
  a_utf8 = XInternAtom(g_dpy, "UTF8_STRING", False);
  a_targets = XInternAtom(g_dpy, "TARGETS", False);
  a_bui_sel = XInternAtom(g_dpy, "BUI_SELECTION", False);
  return 1;
}

int BK_window_supported(void)
{
  return open_display();
}

/* Xft.dpi, as GHOST_SystemX11 reads it, else 96. */
static float display_scale(void)
{
  const char *env = getenv("BUI_UI_SCALE");
  if (env) {
    return (float)atof(env);
  }
  float dpi = 96.0f;
  char *rms = XResourceManagerString(g_dpy);
  if (rms) {
    XrmInitialize();
    XrmDatabase db = XrmGetStringDatabase(rms);
    char *type = NULL;
    XrmValue value;
    if (db && XrmGetResource(db, "Xft.dpi", "Xft.Dpi", &type, &value) && value.addr) {
      dpi = (float)atof(value.addr);
    }
    if (db) {
      XrmDestroyDatabase(db);
    }
  }
  return BK_MAX(1.0f, dpi / 96.0f);
}

static BKWindow *find_window(Window xid)
{
  for (BKWindow *w = g_windows; w; w = w->next) {
    if (w->xid == xid) {
      return w;
    }
  }
  return NULL;
}

/* -- host callbacks -- */

static void host_request(void *data)
{
  BKWindow *win = (BKWindow *)data;
  if (win->xid) {
    win->render_pending = 1;
  }
}

/* GHOST_WindowX11::setWindowCursorShape */
static void host_set_cursor(void *data, int c)
{
  BKWindow *win = (BKWindow *)data;
  if (!win->xid) {
    return;
  }
  c = BK_CLAMP(c, 0, 11);
  win->cursor = c;
  if (!win->cursors[c]) {
    if (c == BK_CURSOR_NONE) { /* an empty 1x1 bitmap */
      static char zero = 0;
      Pixmap p = XCreateBitmapFromData(g_dpy, win->xid, &zero, 1, 1);
      XColor black = {0};
      win->cursors[c] = XCreatePixmapCursor(g_dpy, p, p, &black, &black, 0, 0);
      XFreePixmap(g_dpy, p);
    }
    else {
      static const unsigned shapes[12] = {XC_left_ptr,     XC_sb_h_double_arrow,
                                          XC_sb_v_double_arrow, XC_xterm,
                                          XC_crosshair,    XC_hand2,
                                          XC_fleur,        XC_left_ptr,
                                          XC_bottom_left_corner, XC_bottom_right_corner,
                                          XC_X_cursor,     XC_watch};
      win->cursors[c] = XCreateFontCursor(g_dpy, shapes[c]);
    }
  }
  XDefineCursor(g_dpy, win->xid, win->cursors[c]);
  XFlush(g_dpy);
}

static void host_warp(void *data, int x, int y)
{
  BKWindow *win = (BKWindow *)data;
  if (win->xid) {
    XWarpPointer(g_dpy, None, win->xid, 0, 0, 0, 0, x, y);
    XFlush(g_dpy);
  }
}

static void handle_event(XEvent *ev);

/* GHOST_SystemX11::getClipboard: ask the owner, wait for its answer. */
static char *host_clipboard_get(void *data)
{
  BKWindow *win = (BKWindow *)data;
  if (XGetSelectionOwner(g_dpy, a_clipboard) == win->xid && g_clip_text) {
    return strdup(g_clip_text);
  }
  XConvertSelection(g_dpy, a_clipboard, a_utf8, a_bui_sel, win->xid, CurrentTime);
  XFlush(g_dpy);
  const double until = now_ms() + 300.0;
  while (now_ms() < until) {
    XEvent ev;
    if (XCheckTypedWindowEvent(g_dpy, win->xid, SelectionNotify, &ev)) {
      if (ev.xselection.property == None) {
        return NULL;
      }
      Atom type;
      int format;
      unsigned long n, after;
      unsigned char *prop = NULL;
      XGetWindowProperty(g_dpy, win->xid, a_bui_sel, 0, 1 << 24, True, AnyPropertyType, &type,
                         &format, &n, &after, &prop);
      char *out = prop ? strndup((const char *)prop, n) : NULL;
      if (prop) {
        XFree(prop);
      }
      return out;
    }
    usleep(2000);
  }
  return NULL;
}

static void host_clipboard_set(void *data, const char *text)
{
  BKWindow *win = (BKWindow *)data;
  free(g_clip_text);
  g_clip_text = strdup(text ? text : "");
  XSetSelectionOwner(g_dpy, a_clipboard, win->xid, CurrentTime);
  XFlush(g_dpy);
}

static Timer *timer_slot(BKWindow *win, int id)
{
  for (int i = 0; i < MAX_TIMERS; i++) {
    if (win->timers[i].id == id) {
      return &win->timers[i];
    }
  }
  for (int i = 0; i < MAX_TIMERS; i++) {
    if (win->timers[i].id == 0) {
      return &win->timers[i];
    }
  }
  return NULL;
}

static void host_schedule(void *data, int ms, int timer)
{
  BKWindow *win = (BKWindow *)data;
  /* slot -1: the tooltip; -2: custom nodes' frames (BK_TIMER_CUSTOM) */
  const int slot = timer == BK_TIMER_CUSTOM ? -2 : -1;
  if (slot == -1) {
    win->tip_id = timer;
  }
  Timer *t = timer_slot(win, slot);
  if (t) {
    t->id = slot;
    t->due = now_ms() + ms;
  }
}

static void host_emit(void *data, BKNode *node, int kind, void *ref, double a, double b)
{
  BKWindow *win = (BKWindow *)data;
  if (win->prev.emit) {
    win->prev.emit(win->prev.data, node, kind, ref, a, b);
  }
}

/* -- rendering -- */

/* Moves the window's raster to the GPU (OpenGL through GLX on our Display);
 * BUI_raster_gpu_poll switches over at the next render, and failures leave
 * it on the software raster. */
static void gpu_attach(BKWindow *win)
{
  if (win->xid) {
    BuiGpuX11Window target = {g_dpy, (unsigned long)win->xid};
    BUI_raster_gpu_attach(win->raster, &target, 0);
  }
}

static void present(BKWindow *win, int all)
{
  if (BUI_raster_is_gpu(win->raster)) {
    BUI_raster_present_native(win->raster, NULL, all);
    if (!BUI_raster_is_gpu(win->raster)) {
      win->full = 1; /* device lost: the software raster redraws everything */
      win->render_pending = 1;
    }
    return;
  }
  const int w = BUI_raster_width(win->raster), h = BUI_raster_height(win->raster);
  if (!win->image || win->image->width != w || win->image->height != h) {
    if (win->image) {
      win->image->data = NULL; /* the raster owns the pixels */
      XDestroyImage(win->image);
    }
    Visual *vis = DefaultVisual(g_dpy, DefaultScreen(g_dpy));
    win->image = XCreateImage(g_dpy, vis, 24, ZPixmap, 0, NULL, (unsigned)w, (unsigned)h, 32,
                              w * 4);
    all = 1;
  }
  /* 0xAARRGGBB words are B, G, R, A bytes: the 24-bit TrueColor layout */
  win->image->data = (char *)BUI_raster_pixels(win->raster);
  if (all) {
    XPutImage(g_dpy, win->xid, win->gc, win->image, 0, 0, 0, 0, (unsigned)w, (unsigned)h);
  }
  else {
    const int(*rects)[4];
    const int n = BUI_raster_damage(win->raster, &rects);
    for (int i = 0; i < n; i++) {
      XPutImage(g_dpy, win->xid, win->gc, win->image, rects[i][0], rects[i][1], rects[i][0],
                rects[i][1], (unsigned)rects[i][2], (unsigned)rects[i][3]);
    }
  }
  XFlush(g_dpy);
}

static void do_render(BKWindow *win)
{
  const int w = BK_MAX(1, win->w), h = BK_MAX(1, win->h);
  const int resized = BUI_raster_width(win->raster) != w || BUI_raster_height(win->raster) != h;
  if (BUI_raster_gpu_poll(win->raster)) {
    win->full = 1; /* the renderer changed: its frame starts empty */
  }
  const int full = win->full || resized;
  win->full = 0;
  const int regions = BK_render(win->ctx, win->raster, w, h, full);
  if (regions || full) {
    present(win, full);
    win->frames++;
  }
}

static double refresh_ms(void)
{
  return 1000.0 / 60.0;
}

static void render_window(BKWindow *win)
{
  /* a new size: the host lays out for it before this frame, so no frame shows the last
   * layout at the new size (centred text would jump back and forth while the window is
   * dragged wider or narrower) */
  if (win->cb.on_frame && (BUI_raster_width(win->raster) != BK_MAX(1, win->w) ||
                           BUI_raster_height(win->raster) != BK_MAX(1, win->h)))
  {
    win->want_frame = 1;
  }
  if (win->want_frame && win->cb.on_frame) {
    win->want_frame = 0;
    win->frame_start = now_ms();
    win->cb.on_frame(win->cb.data, win);
    if (!win->xid) { /* the frame callback can close the window */
      return;
    }
  }
  win->render_pending = 0;
  do_render(win);
  if (win->want_frame) {
    bk_frame_ms = now_ms() - win->frame_start;
    bk_frame_budget_ms = win->frame_vsync ? refresh_ms() : 0.0;
    win->render_pending = 1;
  }
  else {
    bk_frame_ms = 0.0;
  }
  if (BK_animating(win->ctx)) {
    win->render_pending = 1; /* paced by the loop's frame wait */
  }
}

/* -- input mapping (GHOST_SystemX11::processEvent) -- */

static int mods_of(unsigned state)
{
  int m = 0;
  if (state & ShiftMask) {
    m |= BK_SHIFT;
  }
  if (state & ControlMask) {
    m |= BK_CTRL;
  }
  if (state & Mod1Mask) {
    m |= BK_ALT;
  }
  if (state & Button1Mask) {
    m |= BK_BUTTON1;
  }
  return m;
}

static void handle_key(BKWindow *win, XKeyEvent *ke, int press)
{
  char text[32] = {0};
  KeySym sym = 0;
  const int len = XLookupString(ke, text, sizeof(text) - 1, &sym, NULL);
  text[BK_MAX(0, len)] = 0;
  const char *name = XKeysymToString(sym); /* the X keysym names Tk and BUI use */
  if (!name) {
    name = "";
  }
  const int mods = mods_of(ke->state);
  if (!press) {
    BK_key_release(win->ctx, name, mods);
    return;
  }
  /* control characters are not text */
  const int printable = len > 0 && (unsigned char)text[0] >= 0x20 && text[0] != 0x7f &&
                        !(mods & BK_CTRL);
  BK_key(win->ctx, printable ? text : "", name, mods);
}

static void close_window(BKWindow *win)
{
  if (!win->xid) {
    return;
  }
  BUI_raster_gpu_release(win->raster); /* the GL context uses the window */
  XDestroyWindow(g_dpy, win->xid);
  XFlush(g_dpy);
  win->xid = 0;
  g_open--;
}

static void handle_event(XEvent *ev)
{
  if (ev->type == SelectionRequest) { /* someone pastes what we copied */
    XSelectionRequestEvent *rq = &ev->xselectionrequest;
    XSelectionEvent sn = {SelectionNotify, 0, True, rq->display, rq->requestor, rq->selection,
                          rq->target, None, rq->time};
    if (rq->target == a_targets) {
      Atom targets[] = {a_targets, a_utf8, XA_STRING};
      XChangeProperty(g_dpy, rq->requestor, rq->property, XA_ATOM, 32, PropModeReplace,
                      (unsigned char *)targets, 3);
      sn.property = rq->property;
    }
    else if ((rq->target == a_utf8 || rq->target == XA_STRING) && g_clip_text) {
      XChangeProperty(g_dpy, rq->requestor, rq->property, rq->target, 8, PropModeReplace,
                      (unsigned char *)g_clip_text, (int)strlen(g_clip_text));
      sn.property = rq->property;
    }
    XSendEvent(g_dpy, rq->requestor, False, 0, (XEvent *)&sn);
    return;
  }
  BKWindow *win = find_window(ev->xany.window);
  if (!win || !win->xid) {
    return;
  }
  BKContext *ctx = win->ctx;
  switch (ev->type) {
    case Expose:
      if (ev->xexpose.count == 0) {
        win->full = 1;
        win->render_pending = 1;
      }
      break;
    case ConfigureNotify:
      if (ev->xconfigure.width != win->w || ev->xconfigure.height != win->h) {
        win->w = ev->xconfigure.width;
        win->h = ev->xconfigure.height;
        win->render_pending = 1;
      }
      break;
    case MotionNotify:
      BK_motion(ctx, ev->xmotion.x, ev->xmotion.y, mods_of(ev->xmotion.state));
      break;
    case LeaveNotify:
      if (!(ev->xcrossing.state & (Button1Mask | Button2Mask | Button3Mask))) {
        BK_motion(ctx, -100000, -100000, 0);
      }
      break;
    case ButtonPress: {
      const XButtonEvent *b = &ev->xbutton;
      const int mods = mods_of(b->state);
      switch (b->button) {
        case Button1:
          XSetInputFocus(g_dpy, win->xid, RevertToParent, CurrentTime);
          BK_press(ctx, b->x, b->y, mods | BK_BUTTON1);
          break;
        case Button2:
          BK_button(ctx, b->x, b->y, 2, 0, mods);
          break;
        case Button3:
          BK_context_menu(ctx, b->x, b->y, mods);
          break;
        case Button4: /* wheel up */
        case Button5:
          BK_scroll(ctx, b->x, b->y, 0.0f, b->button == Button4 ? 1.0f : -1.0f,
                    mods & ~BK_BUTTON1);
          break;
        case 6: /* horizontal wheel: left, right (widgets: Shift+wheel, as GHOST maps it) */
        case 7:
          BK_scroll(ctx, b->x, b->y, b->button == 6 ? -1.0f : 1.0f, 0.0f, mods & ~BK_BUTTON1);
          break;
      }
      break;
    }
    case ButtonRelease: {
      const XButtonEvent *b = &ev->xbutton;
      const int mods = mods_of(b->state) & ~BK_BUTTON1;
      if (b->button == Button1) {
        BK_release(ctx, b->x, b->y, mods);
      }
      else if (b->button == Button2 || b->button == Button3) {
        BK_button(ctx, b->x, b->y, (int)b->button == Button2 ? 2 : 3, 1, mods);
      }
      break;
    }
    case KeyPress:
    case KeyRelease:
      if (ev->type == KeyRelease && XEventsQueued(g_dpy, QueuedAfterReading)) {
        /* autorepeat: a release followed by a press at the same time */
        XEvent next;
        XPeekEvent(g_dpy, &next);
        if (next.type == KeyPress && next.xkey.time == ev->xkey.time &&
            next.xkey.keycode == ev->xkey.keycode)
        {
          break;
        }
      }
      handle_key(win, &ev->xkey, ev->type == KeyPress);
      break;
    case FocusIn:
    case FocusOut:
      /* NotifyPointer is the pointer's window getting focus events it doesn't hold */
      if (ev->xfocus.detail != NotifyPointer) {
        const int focused = ev->type == FocusIn;
        if (focused != win->focused) {
          win->focused = focused;
          BK_window_request_frame(win, 0); /* the host's on_frame reads BK_window_focused */
        }
      }
      break;
    case ClientMessage:
      if ((Atom)ev->xclient.data.l[0] == a_delete) {
        if (!win->cb.on_close || win->cb.on_close(win->cb.data, win)) {
          close_window(win);
        }
      }
      break;
  }
}

/* -- windows -- */

BKWindow *BK_window_new(BKContext *ctx, const char *title, int width, int height)
{
  if (!open_display()) {
    return NULL;
  }
  BKWindow *win = (BKWindow *)calloc(1, sizeof(BKWindow));
  win->ctx = ctx;
  win->prev = ctx->host;
  win->raster = BUI_raster_create(1, 1);
  win->next_timer = 1;
  win->scale = display_scale();
  BK_set_scale(win->scale);
  win->w = (int)(width * win->scale);
  win->h = (int)(height * win->scale);
  const int screen = DefaultScreen(g_dpy);
  XSetWindowAttributes attr;
  memset(&attr, 0, sizeof(attr));
  attr.background_pixel = BlackPixel(g_dpy, screen);
  attr.event_mask = ExposureMask | StructureNotifyMask | PointerMotionMask | ButtonPressMask |
                    ButtonReleaseMask | KeyPressMask | KeyReleaseMask | LeaveWindowMask |
                    FocusChangeMask;
  win->xid = XCreateWindow(g_dpy, RootWindow(g_dpy, screen), 0, 0, (unsigned)win->w,
                           (unsigned)win->h, 0, CopyFromParent, InputOutput, CopyFromParent,
                           CWBackPixel | CWEventMask, &attr);
  if (!win->xid) {
    BUI_raster_free(win->raster);
    free(win);
    return NULL;
  }
  XSetWMProtocols(g_dpy, win->xid, &a_delete, 1);
  win->gc = XCreateGC(g_dpy, win->xid, 0, NULL);
  BK_window_set_title(win, title);
  win->next = g_windows;
  g_windows = win;
  g_open++;
  if (BUI_gpu_default()) {
    gpu_attach(win);
  }
  BKHost host = {win,           host_request,       host_set_cursor, host_warp,
                 host_clipboard_get, host_clipboard_set, host_schedule, host_emit};
  BK_context_set_host(ctx, &host);
  return win;
}

void BK_window_free(BKWindow *win)
{
  if (!win) {
    return;
  }
  close_window(win);
  for (BKWindow **p = &g_windows; *p; p = &(*p)->next) {
    if (*p == win) {
      *p = win->next;
      break;
    }
  }
  if (win->image) {
    win->image->data = NULL;
    XDestroyImage(win->image);
  }
  if (win->gc) {
    XFreeGC(g_dpy, win->gc);
  }
  if (win->ctx) {
    BK_context_set_host(win->ctx, &win->prev);
  }
  BUI_raster_free(win->raster);
  free(win);
}

void BK_window_set_callbacks(BKWindow *win, const BKWindowCallbacks *cb)
{
  win->cb = *cb;
}

void BK_window_show(BKWindow *win)
{
  if (win->xid) {
    XMapWindow(g_dpy, win->xid);
    XFlush(g_dpy);
    win->render_pending = 1;
  }
}

void BK_window_close(BKWindow *win)
{
  close_window(win);
}

int BK_window_is_open(BKWindow *win)
{
  return win && win->xid != 0;
}

void BK_window_set_title(BKWindow *win, const char *title)
{
  if (!win->xid) {
    return;
  }
  const char *t = title ? title : "";
  XChangeProperty(g_dpy, win->xid, XInternAtom(g_dpy, "_NET_WM_NAME", False), a_utf8, 8,
                  PropModeReplace, (const unsigned char *)t, (int)strlen(t));
  XStoreName(g_dpy, win->xid, t);
  XFlush(g_dpy);
}

void BK_window_set_size(BKWindow *win, int width, int height)
{
  if (win->xid) {
    /* X resizes asynchronously; take the size now, as Win32's SetWindowPos
     * does (a ConfigureNotify from the window manager corrects it later) */
    win->w = BK_MAX(1, (int)(width * win->scale));
    win->h = BK_MAX(1, (int)(height * win->scale));
    XResizeWindow(g_dpy, win->xid, (unsigned)win->w, (unsigned)win->h);
    XFlush(g_dpy);
    win->render_pending = 1;
  }
}

void BK_window_get_size(BKWindow *win, int *w, int *h)
{
  *w = win->xid ? win->w : 0;
  *h = win->xid ? win->h : 0;
}

BuiRaster *BK_window_raster(BKWindow *win)
{
  return win->raster;
}

void BK_window_set_gpu(BKWindow *win, int on)
{
  if (win && on) {
    gpu_attach(win);
    host_request(win);
  }
}

int BK_window_gpu(BKWindow *win)
{
  return win && BUI_raster_is_gpu(win->raster);
}

int BK_window_frames(BKWindow *win)
{
  return win->frames;
}

void BK_window_render_now(BKWindow *win)
{
  if (win->xid) {
    win->full = 1;
    render_window(win);
  }
}

int BK_window_timer(BKWindow *win, int ms)
{
  const int id = win->next_timer++;
  Timer *t = timer_slot(win, id);
  if (t) {
    t->id = id;
    t->due = now_ms() + BK_MAX(0, ms);
  }
  return id;
}

void BK_window_cancel_timer(BKWindow *win, int id)
{
  for (int i = 0; i < MAX_TIMERS; i++) {
    if (win->timers[i].id == id) {
      win->timers[i].id = 0;
    }
  }
}

void BK_window_request_frame(BKWindow *win, int vsync)
{
  win->want_frame = 1;
  win->frame_vsync = vsync;
  win->render_pending = 1;
}

void *BK_window_handle(BKWindow *win)
{
  return (void *)(uintptr_t)win->xid;
}

int BK_window_focused(BKWindow *win)
{
  return win->xid && win->focused;
}

/* EWMH activation: _NET_ACTIVE_WINDOW from a pager (source 2), which window managers honour
 * without focus-stealing prevention, plus the "lift" and "focus" requests for those without
 * EWMH. The token goes to _NET_STARTUP_ID first, as startup notification hands it over. */
void BK_window_activate(BKWindow *win, const char *token)
{
  if (!win->xid) {
    return;
  }
  if (token && token[0]) {
    XChangeProperty(g_dpy, win->xid, XInternAtom(g_dpy, "_NET_STARTUP_ID", False), a_utf8, 8,
                    PropModeReplace, (const unsigned char *)token, (int)strlen(token));
  }
  XMapWindow(g_dpy, win->xid);
  XRaiseWindow(g_dpy, win->xid);
  XEvent ev;
  memset(&ev, 0, sizeof(ev));
  ev.xclient.type = ClientMessage;
  ev.xclient.window = win->xid;
  ev.xclient.message_type = XInternAtom(g_dpy, "_NET_ACTIVE_WINDOW", False);
  ev.xclient.format = 32;
  ev.xclient.data.l[0] = 2; /* source indication: pager */
  ev.xclient.data.l[1] = CurrentTime;
  XSendEvent(g_dpy, RootWindow(g_dpy, DefaultScreen(g_dpy)), False,
             SubstructureRedirectMask | SubstructureNotifyMask, &ev);
  XWindowAttributes attrs; /* focusing a window that isn't viewable yet is a BadMatch */
  if (XGetWindowAttributes(g_dpy, win->xid, &attrs) && attrs.map_state == IsViewable) {
    XSetInputFocus(g_dpy, win->xid, RevertToParent, CurrentTime);
  }
  XFlush(g_dpy);
}

/* GHOST_WindowX11: size hints for minsize / maxsize / resizable, moves for pos */
void BK_window_wm(BKWindow *win, const char *key, double a, double b)
{
  if (!win->xid) {
    return;
  }
  if (!strcmp(key, "pos")) {
    XMoveWindow(g_dpy, win->xid, (int)a, (int)b);
  }
  else if (!strcmp(key, "minsize") || !strcmp(key, "maxsize") || !strcmp(key, "resizable")) {
    XSizeHints *hints = XAllocSizeHints();
    long supplied;
    XGetWMNormalHints(g_dpy, win->xid, hints, &supplied);
    if (!strcmp(key, "minsize")) {
      hints->flags |= PMinSize;
      hints->min_width = (int)(a * win->scale);
      hints->min_height = (int)(b * win->scale);
    }
    else if (!strcmp(key, "maxsize")) {
      hints->flags |= PMaxSize;
      hints->max_width = (int)(a * win->scale);
      hints->max_height = (int)(b * win->scale);
    }
    else if (a == 0 || b == 0) { /* not resizable along an axis: pin it */
      hints->flags |= PMinSize | PMaxSize;
      hints->min_width = hints->max_width = a == 0 ? win->w : 1;
      hints->min_height = hints->max_height = b == 0 ? win->h : 1;
      if (a != 0) {
        hints->max_width = 1 << 15;
      }
      if (b != 0) {
        hints->max_height = 1 << 15;
      }
    }
    XSetWMNormalHints(g_dpy, win->xid, hints);
    XFree(hints);
  }
  else if (!strcmp(key, "lift")) {
    XRaiseWindow(g_dpy, win->xid);
  }
  else if (!strcmp(key, "lower")) {
    XLowerWindow(g_dpy, win->xid);
  }
  else if (!strcmp(key, "focus")) {
    XSetInputFocus(g_dpy, win->xid, RevertToParent, CurrentTime);
  }
  XFlush(g_dpy);
}

int BK_window_wm_get(BKWindow *win, const char *key, double *r_a, double *r_b)
{
  *r_a = *r_b = 0.0;
  if (!g_dpy) {
    return 0;
  }
  const int screen = DefaultScreen(g_dpy);
  if (!strcmp(key, "screen")) {
    *r_a = DisplayWidth(g_dpy, screen);
    *r_b = DisplayHeight(g_dpy, screen);
    return 1;
  }
  if (!strcmp(key, "scale")) {
    *r_a = *r_b = win ? win->scale : display_scale();
    return 1;
  }
  if (!win || !win->xid) {
    return 0;
  }
  if (!strcmp(key, "pointer") || !strcmp(key, "pos")) {
    Window root, child;
    int rx, ry, wx, wy;
    unsigned mask;
    XQueryPointer(g_dpy, win->xid, &root, &child, &rx, &ry, &wx, &wy, &mask);
    if (!strcmp(key, "pointer")) {
      *r_a = rx;
      *r_b = ry;
    }
    else {
      *r_a = rx - wx;
      *r_b = ry - wy;
    }
    return 1;
  }
  return 0;
}

/* -- the loop -- */

static void run_timers(void)
{
  const double t = now_ms();
  for (BKWindow *win = g_windows; win; win = win->next) {
    for (int i = 0; i < MAX_TIMERS && win->xid; i++) {
      Timer *tm = &win->timers[i];
      if (tm->id != 0 && tm->due <= t) {
        const int id = tm->id;
        tm->id = 0;
        if (id == -1) {
          BK_timer(win->ctx, win->tip_id);
        }
        else if (id == -2) {
          BK_timer(win->ctx, BK_TIMER_CUSTOM);
        }
        else if (win->cb.on_timer) {
          win->cb.on_timer(win->cb.data, win, id);
        }
      }
    }
  }
}

/* Milliseconds until the next timer or frame (-1: nothing to wait for). */
static double next_wait(void)
{
  double wait = -1.0;
  const double t = now_ms();
  for (BKWindow *win = g_windows; win; win = win->next) {
    if (!win->xid) {
      continue;
    }
    if (win->render_pending) {
      return 0.0;
    }
    if (BUI_raster_gpu_pending(win->raster)) { /* check the device once per refresh */
      wait = wait < 0 ? refresh_ms() : BK_MIN(wait, refresh_ms());
    }
    for (int i = 0; i < MAX_TIMERS; i++) {
      if (win->timers[i].id != 0) {
        const double d = BK_MAX(0.0, win->timers[i].due - t);
        wait = wait < 0 ? d : BK_MIN(wait, d);
      }
    }
  }
  return wait;
}

static void drain_wake(void)
{
  char buf[64];
  while (g_wake_pipe[0] >= 0 && read(g_wake_pipe[0], buf, sizeof(buf)) > 0) {
  }
  BK_image_poll(); /* decoded images: damage what shows them */
  if (g_wake_cb) {
    g_wake_cb();
  }
}

/* One turn: events, timers, renders; blocks up to `block_ms` (-1: forever). */
static void turn(double block_ms)
{
  const int fd = ConnectionNumber(g_dpy);
  if (!XPending(g_dpy) && block_ms != 0.0) {
    fd_set set;
    FD_ZERO(&set);
    FD_SET(fd, &set);
    int maxfd = fd;
    if (g_wake_pipe[0] >= 0) {
      FD_SET(g_wake_pipe[0], &set);
      maxfd = BK_MAX(maxfd, g_wake_pipe[0]);
    }
    struct timeval tv, *ptv = NULL;
    if (block_ms > 0.0) {
      tv.tv_sec = (long)(block_ms / 1000.0);
      tv.tv_usec = (long)(fmod(block_ms, 1000.0) * 1000.0);
      ptv = &tv;
    }
    if (select(maxfd + 1, &set, NULL, NULL, ptv) > 0 && g_wake_pipe[0] >= 0 &&
        FD_ISSET(g_wake_pipe[0], &set))
    {
      drain_wake();
    }
  }
  while (XPending(g_dpy)) {
    XEvent ev;
    XNextEvent(g_dpy, &ev);
    handle_event(&ev);
  }
  run_timers();
  for (BKWindow *win = g_windows; win; win = win->next) {
    /* a GPU device that is ready switches an idle window over too */
    if (win->xid && BUI_raster_gpu_pending(win->raster) && BUI_raster_gpu_poll(win->raster)) {
      win->full = 1;
      win->render_pending = 1;
    }
  }
  int rendered = 0;
  for (BKWindow *win = g_windows; win; win = win->next) {
    if (win->xid && win->render_pending) {
      render_window(win);
      rendered |= win->render_pending; /* animating: another frame wanted */
    }
  }
  if (rendered) {
    usleep((useconds_t)(refresh_ms() * 1000.0)); /* pace animation frames */
  }
}

void BK_window_set_stop_check(int (*stop)(void))
{
  g_stop = stop;
}

void BK_window_quit(void)
{
  g_quit = 1;
  BK_window_wake();
}

void BK_window_run(void)
{
  if (!g_dpy) {
    return;
  }
  g_quit = 0;
  while ((g_open > 0 || g_keep_alive) && !g_quit) {
    turn(next_wait());
    if (g_stop && g_stop()) {
      break;
    }
  }
}

int BK_window_pump(void)
{
  if (g_dpy) {
    turn(0.0);
  }
  return g_open;
}

int bk_window_wake_ready(void)
{
  if (g_wake_pipe[0] < 0 && pipe(g_wake_pipe) == 0) {
    fcntl(g_wake_pipe[0], F_SETFL, O_NONBLOCK);
    fcntl(g_wake_pipe[1], F_SETFL, O_NONBLOCK);
  }
  return g_wake_pipe[0] >= 0;
}

int BK_window_wake_init(void (*cb)(void))
{
  g_wake_cb = cb;
  return bk_window_wake_ready();
}

/* Thread-safe: wakes the loop, which calls the wake callback. */
void BK_window_wake(void)
{
  if (g_wake_pipe[1] >= 0) {
    const char c = 1;
    (void)!write(g_wake_pipe[1], &c, 1);
  }
}

void BK_window_set_keep_alive(int on)
{
  g_keep_alive = on;
}

#endif /* BUI_WINDOW_X11 */
