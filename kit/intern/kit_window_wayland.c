/* Native window host (Wayland). The same BK_window_* API as kit_window_x11.c and
 * kit_window_win32.c, under the bk_wl_ prefix (kit_window_linux.c picks the host).
 *
 * Modelled on Blender's GHOST_SystemWayland / GHOST_WindowWayland: xdg-shell toplevels with
 * server-side decorations when the compositor offers them, a wl_seat with xkbcommon for keys
 * (key repeat is ours, the compositor only sends the rate and delay), the wl_pointer, wl_data_device
 * for the clipboard, wl_cursor for cursors, buffer scale from the output. libwayland-client and
 * libwayland-cursor come through Blender's wayland_dynload (extern/wayland_dynload, loaded when the
 * first window opens) and libxkbcommon is dlopen()ed here, so nothing is linked and a build runs
 * where Wayland is absent.
 *
 * Frames are drawn by the CPU raster into wl_shm buffers (two, alternating; only the damaged
 * rectangles are copied) and paced by wl_surface.frame callbacks. The GPU renderer is not
 * available here yet (see kit_window_linux.c).
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */

#define _GNU_SOURCE

#include "kit_window_linux.h"

#ifdef BUI_WINDOW_WAYLAND

/* Blender's dynload first: it turns every libwayland call into a call through a pointer. */
#  include "wayland_dynload_API.h"
#  include "wayland_dynload_client.h"
#  include "wayland_dynload_cursor.h"

#  include <wayland-client.h>

#  include "xdg-activation-v1-client-protocol.h"
#  include "xdg-decoration-unstable-v1-client-protocol.h"
#  include "xdg-shell-client-protocol.h"

#  include <dlfcn.h>
#  include <errno.h>
#  include <fcntl.h>
#  include <math.h>
#  include <poll.h>
#  include <stdio.h>
#  include <stdlib.h>
#  include <string.h>
#  include <sys/mman.h>
#  include <time.h>
#  include <unistd.h>

#  include <xkbcommon/xkbcommon.h>

#  define MAX_TIMERS 16
#  define MAX_RECTS 32
#  define MAX_OUTPUTS 8

/* -- libxkbcommon, loaded on first use -- */

#  define BK_XKB_FUNCS(F) \
    F(xkb_context_new) \
    F(xkb_context_unref) \
    F(xkb_keymap_new_from_buffer) \
    F(xkb_keymap_unref) \
    F(xkb_keymap_key_repeats) \
    F(xkb_state_new) \
    F(xkb_state_unref) \
    F(xkb_state_update_mask) \
    F(xkb_state_key_get_one_sym) \
    F(xkb_state_key_get_utf8) \
    F(xkb_state_mod_name_is_active) \
    F(xkb_keysym_get_name)

static struct {
#  define F(name) __typeof__(&name) name;
  BK_XKB_FUNCS(F)
#  undef F
} xkb;

static int xkb_load(void)
{
  static int state = 0; /* 1 loaded, -1 failed */
  if (state) {
    return state > 0;
  }
  void *lib = dlopen("libxkbcommon.so.0", RTLD_NOW);
  state = -1;
  if (lib) {
    state = 1;
#  define F(name) \
    if (!(*(void **)&xkb.name = dlsym(lib, #name))) { \
      state = -1; \
    }
    BK_XKB_FUNCS(F)
#  undef F
  }
  return state > 0;
}

/* -- state -- */

typedef struct {
  int id; /* > 0 user timer, -1 the tooltip timer, -2 custom nodes' frames, 0 free */
  double due;
} Timer;

typedef struct {
  struct wl_buffer *buffer;
  uint32_t *data;
  size_t size;
  int w, h;
  bool busy, valid; /* held by the compositor; holds the frame before the last */
} Buf;

struct BKWindow {
  BKContext *ctx;
  BKHost prev;
  BuiRaster *raster;
  BKWindowCallbacks cb;
  struct wl_surface *surface;
  struct xdg_surface *xdg_surface;
  struct xdg_toplevel *toplevel;
  struct zxdg_toplevel_decoration_v1 *decoration;
  struct wl_callback *frame_cb;
  Buf bufs[2];
  int open, configured;
  int render_pending, frames, cursor, tip_id, next_timer, full;
  int want_frame, frame_vsync;
  double frame_start, hold_until;
  int bscale;           /* wl_surface buffer scale (the output's) */
  float ui_scale;       /* BK_set_scale: BUI_UI_SCALE, else the buffer scale */
  int lw, lh;           /* the window in compositor (logical) units */
  int w, h;             /* the raster in pixels: logical * bscale */
  int last_n;           /* damage of the last presented frame, to refresh the other buffer */
  int last_rects[MAX_RECTS][4];
  bool last_full;
  Timer timers[MAX_TIMERS];
  struct BKWindow *next;
};

typedef struct {
  struct wl_output *output;
  int scale, w, h;
} Output;

static struct {
  struct wl_display *display;
  struct wl_registry *registry;
  struct wl_compositor *compositor;
  uint32_t compositor_version;
  struct wl_shm *shm;
  struct xdg_wm_base *wm_base;
  struct zxdg_decoration_manager_v1 *decoration_manager;
  struct xdg_activation_v1 *activation; /* NULL when the compositor has no xdg-activation */
  struct wl_seat *seat;
  struct wl_pointer *pointer;
  struct wl_keyboard *keyboard;
  struct wl_data_device_manager *data_manager;
  struct wl_data_device *data_device;
  struct wl_data_offer *offer; /* the clipboard, with a text mime type */
  struct wl_data_offer *pending_offer;
  const char *offer_mime;
  struct wl_data_source *source;
  Output outputs[MAX_OUTPUTS];
  int noutputs;
  struct wl_cursor_theme *cursor_theme;
  struct wl_surface *cursor_surface;
  int cursor_scale;
  uint32_t serial; /* the last input serial (clipboard ownership needs one) */
  uint32_t pointer_serial;
  BKWindow *pointer_win, *keyboard_win;
  double px, py;  /* pointer position in raster pixels */
  int buttons;    /* BK_BUTTON1 while the left button is down */
  struct xkb_context *xkb_context;
  struct xkb_keymap *keymap;
  struct xkb_state *xkb_state;
  int repeat_rate, repeat_delay;
  uint32_t repeat_key; /* evdev code, 0 none */
  double repeat_due;
} g;

static BKWindow *g_windows = NULL;
static int g_open = 0, g_quit = 0, g_keep_alive = 0;
static int (*g_stop)(void) = NULL;
static void (*g_wake_cb)(void) = NULL;
static int g_wake_pipe[2] = {-1, -1};
static char *g_clip_text = NULL; /* what we own on the clipboard */

static double now_ms(void)
{
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return ts.tv_sec * 1000.0 + ts.tv_nsec / 1e6;
}

static double refresh_ms(void)
{
  return 1000.0 / 60.0;
}

static BKWindow *find_window(struct wl_surface *surface)
{
  for (BKWindow *w = g_windows; w; w = w->next) {
    if (w->surface == surface) {
      return w;
    }
  }
  return NULL;
}

/* -- shm buffers -- */

static void buffer_release(void *data, struct wl_buffer *buffer)
{
  BKWindow *win = (BKWindow *)data;
  for (int i = 0; i < 2; i++) {
    if (win->bufs[i].buffer == buffer) {
      win->bufs[i].busy = false;
    }
  }
  win->render_pending = 1; /* a render that waited for a free buffer */
}

static struct wl_buffer_listener buffer_listener = {.release = buffer_release};

static void buf_destroy(Buf *b)
{
  if (b->buffer) {
    wl_buffer_destroy(b->buffer);
  }
  if (b->data) {
    munmap(b->data, b->size);
  }
  memset(b, 0, sizeof(*b));
}

static bool buf_create(BKWindow *win, Buf *b, int w, int h)
{
  const int stride = w * 4;
  const size_t size = (size_t)stride * (size_t)h;
  const int fd = memfd_create("bui-shm", MFD_CLOEXEC);
  if (fd < 0 || ftruncate(fd, (off_t)size) < 0) {
    if (fd >= 0) {
      close(fd);
    }
    return false;
  }
  void *data = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  if (data == MAP_FAILED) {
    close(fd);
    return false;
  }
  struct wl_shm_pool *pool = wl_shm_create_pool(g.shm, fd, (int32_t)size);
  b->buffer = wl_shm_pool_create_buffer(pool, 0, w, h, stride, WL_SHM_FORMAT_XRGB8888);
  wl_shm_pool_destroy(pool);
  close(fd);
  wl_buffer_add_listener(b->buffer, &buffer_listener, win);
  b->data = (uint32_t *)data;
  b->size = size;
  b->w = w;
  b->h = h;
  b->busy = false;
  b->valid = false;
  return true;
}

/* A buffer of the raster's size that the compositor is not holding (NULL: both are). */
static Buf *buf_acquire(BKWindow *win, int w, int h)
{
  for (int i = 0; i < 2; i++) { /* a new size drops the old buffers */
    Buf *b = &win->bufs[i];
    if (b->buffer && (b->w != w || b->h != h)) {
      buf_destroy(b);
    }
  }
  for (int i = 0; i < 2; i++) {
    Buf *b = &win->bufs[i];
    if (!b->buffer && !buf_create(win, b, w, h)) {
      continue;
    }
    if (!b->busy) {
      return b;
    }
  }
  return NULL;
}

static void copy_rect(Buf *b, const uint32_t *src, int stride_px, const int r[4])
{
  for (int y = r[1]; y < r[1] + r[3]; y++) {
    memcpy(b->data + (size_t)y * (size_t)b->w + r[0], src + (size_t)y * (size_t)stride_px + r[0],
           (size_t)r[2] * 4);
  }
}

static void frame_done(void *data, struct wl_callback *cb, uint32_t time)
{
  BKWindow *win = (BKWindow *)data;
  wl_callback_destroy(cb);
  if (win->frame_cb == cb) {
    win->frame_cb = NULL;
  }
  win->render_pending = BK_animating(win->ctx) || win->render_pending;
  (void)time;
}

static struct wl_callback_listener frame_listener = {.done = frame_done};

/* Copies what changed into a free buffer and commits it; false: no buffer was free. */
static void present(BKWindow *win, Buf *b, int all)
{
  const int w = BUI_raster_width(win->raster), h = BUI_raster_height(win->raster);
  const uint32_t *px = BUI_raster_pixels(win->raster);
  const int(*rects)[4] = NULL;
  const int n = all ? 0 : BUI_raster_damage(win->raster, &rects);
  /* this buffer holds the frame before the last: it needs that frame's damage too */
  const bool whole = all || !b->valid || win->last_full || n > MAX_RECTS;
  if (whole) {
    memcpy(b->data, px, (size_t)w * (size_t)h * 4);
    wl_surface_damage_buffer(win->surface, 0, 0, w, h);
  }
  else {
    for (int i = 0; i < win->last_n; i++) {
      copy_rect(b, px, w, win->last_rects[i]);
    }
    for (int i = 0; i < n; i++) {
      copy_rect(b, px, w, rects[i]);
      wl_surface_damage_buffer(win->surface, rects[i][0], rects[i][1], rects[i][2], rects[i][3]);
    }
  }
  win->last_full = whole;
  win->last_n = whole ? 0 : n;
  for (int i = 0; i < win->last_n; i++) {
    memcpy(win->last_rects[i], rects[i], sizeof(int) * 4);
  }
#  ifdef BUI_WL_CHECK /* debug builds: the damage-limited copy must equal the raster */
  if (memcmp(b->data, px, (size_t)w * (size_t)h * 4) != 0) {
    fprintf(stderr, "kit_window_wayland: presented buffer differs from the raster\n");
    abort();
  }
#  endif
  b->valid = true;
  b->busy = true;
  win->frame_cb = wl_surface_frame(win->surface);
  wl_callback_add_listener(win->frame_cb, &frame_listener, win);
  wl_surface_set_buffer_scale(win->surface, win->bscale);
  wl_surface_attach(win->surface, b->buffer, 0, 0);
  wl_surface_commit(win->surface);
}

static void do_render(BKWindow *win)
{
  const int w = BK_MAX(1, win->w), h = BK_MAX(1, win->h);
  Buf *b = buf_acquire(win, w, h);
  if (!b) {
    return; /* both buffers are with the compositor: the release asks for the render again */
  }
  const int resized = BUI_raster_width(win->raster) != w || BUI_raster_height(win->raster) != h;
  const int full = win->full || resized;
  win->full = 0;
  const int regions = BK_render(win->ctx, win->raster, w, h, full);
  if (regions || full) {
    present(win, b, full);
    win->frames++;
  }
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
    if (!win->open) { /* the frame callback can close the window */
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
    win->render_pending = 1; /* paced by the frame callback */
  }
  if (win->render_pending && !win->frame_cb) {
    win->hold_until = now_ms() + refresh_ms(); /* nothing was presented: do not spin */
  }
}

/* -- scale -- */

static void apply_scale(BKWindow *win, int bscale)
{
  const char *env = getenv("BUI_UI_SCALE");
  win->bscale = BK_MAX(1, bscale);
  win->ui_scale = env ? (float)atof(env) : (float)win->bscale;
  BK_set_scale(win->ui_scale);
  win->w = win->lw * win->bscale;
  win->h = win->lh * win->bscale;
  win->full = 1;
  win->render_pending = 1;
}

/* -- input mapping -- */

static int mods_now(void)
{
  int m = g.buttons;
  if (g.xkb_state) {
    if (xkb.xkb_state_mod_name_is_active(g.xkb_state, XKB_MOD_NAME_SHIFT, XKB_STATE_MODS_EFFECTIVE) > 0) {
      m |= BK_SHIFT;
    }
    if (xkb.xkb_state_mod_name_is_active(g.xkb_state, XKB_MOD_NAME_CTRL, XKB_STATE_MODS_EFFECTIVE) > 0) {
      m |= BK_CTRL;
    }
    if (xkb.xkb_state_mod_name_is_active(g.xkb_state, XKB_MOD_NAME_ALT, XKB_STATE_MODS_EFFECTIVE) > 0) {
      m |= BK_ALT;
    }
  }
  return m;
}

static void send_key(BKWindow *win, uint32_t evdev, bool press)
{
  if (!g.xkb_state) {
    return;
  }
  const xkb_keycode_t code = evdev + 8;
  const xkb_keysym_t sym = xkb.xkb_state_key_get_one_sym(g.xkb_state, code);
  char name[64] = "", text[32] = "";
  xkb.xkb_keysym_get_name(sym, name, sizeof(name)); /* the X keysym names BUI uses */
  const int len = xkb.xkb_state_key_get_utf8(g.xkb_state, code, text, sizeof(text));
  const int mods = mods_now();
  if (!press) {
    BK_key_release(win->ctx, name, mods);
    return;
  }
  /* control characters are not text */
  const bool printable = len > 0 && (unsigned char)text[0] >= 0x20 && text[0] != 0x7f &&
                         !(mods & BK_CTRL);
  BK_key(win->ctx, printable ? text : "", name, mods);
}

/* -- seat: pointer -- */

/* The cursors are xcursor names, as GHOST_SystemWayland's cursor table (X11's shapes). */
static void apply_cursor(BKWindow *win)
{
  if (!g.pointer || g.pointer_win != win) {
    return;
  }
  static const char *names[12] = {"left_ptr",   "sb_h_double_arrow", "sb_v_double_arrow",
                                  "xterm",      "crosshair",         "hand2",
                                  "fleur",      "left_ptr",          "bottom_left_corner",
                                  "bottom_right_corner", "X_cursor", "watch"};
  const int c = BK_CLAMP(win->cursor, 0, 11);
  if (c == BK_CURSOR_NONE) {
    wl_pointer_set_cursor(g.pointer, g.pointer_serial, NULL, 0, 0);
    return;
  }
  if (!g.cursor_theme || !g.cursor_surface) {
    return;
  }
  struct wl_cursor *cursor = wl_cursor_theme_get_cursor(g.cursor_theme, names[c]);
  if (!cursor || cursor->image_count == 0) {
    return;
  }
  struct wl_cursor_image *image = cursor->images[0];
  wl_surface_set_buffer_scale(g.cursor_surface, g.cursor_scale);
  wl_surface_attach(g.cursor_surface, wl_cursor_image_get_buffer(image), 0, 0);
  wl_surface_damage_buffer(g.cursor_surface, 0, 0, (int)image->width, (int)image->height);
  wl_surface_commit(g.cursor_surface);
  wl_pointer_set_cursor(g.pointer, g.pointer_serial, g.cursor_surface,
                        (int)image->hotspot_x / g.cursor_scale,
                        (int)image->hotspot_y / g.cursor_scale);
}

static void pointer_enter(void *d, struct wl_pointer *p, uint32_t serial, struct wl_surface *s,
                          wl_fixed_t x, wl_fixed_t y)
{
  BKWindow *win = find_window(s);
  g.pointer_win = win;
  g.pointer_serial = serial;
  if (win) {
    g.px = wl_fixed_to_double(x) * win->bscale;
    g.py = wl_fixed_to_double(y) * win->bscale;
    apply_cursor(win);
    BK_motion(win->ctx, (int)g.px, (int)g.py, mods_now());
  }
}

static void pointer_leave(void *d, struct wl_pointer *p, uint32_t serial, struct wl_surface *s)
{
  BKWindow *win = find_window(s);
  if (win && !g.buttons) {
    BK_motion(win->ctx, -100000, -100000, 0);
  }
  if (g.pointer_win == win) {
    g.pointer_win = NULL;
  }
}

static void pointer_motion(void *d, struct wl_pointer *p, uint32_t time, wl_fixed_t x,
                           wl_fixed_t y)
{
  BKWindow *win = g.pointer_win;
  if (win) {
    g.px = wl_fixed_to_double(x) * win->bscale;
    g.py = wl_fixed_to_double(y) * win->bscale;
    BK_motion(win->ctx, (int)g.px, (int)g.py, mods_now());
  }
}

enum {
  EVDEV_BTN_LEFT = 0x110,
  EVDEV_BTN_RIGHT = 0x111,
  EVDEV_BTN_MIDDLE = 0x112,
  EVDEV_BTN_SIDE = 0x113, /* back on most mice */
  EVDEV_BTN_EXTRA = 0x114, /* forward on most mice */
  EVDEV_BTN_FORWARD = 0x115,
  EVDEV_BTN_BACK = 0x116,
};

static void pointer_button(void *d, struct wl_pointer *p, uint32_t serial, uint32_t time,
                           uint32_t button, uint32_t state)
{
  BKWindow *win = g.pointer_win;
  g.serial = serial;
  if (!win) {
    return;
  }
  const bool down = state == WL_POINTER_BUTTON_STATE_PRESSED;
  const int x = (int)g.px, y = (int)g.py;
  if (button == EVDEV_BTN_LEFT) {
    if (down) {
      g.buttons = BK_BUTTON1;
      BK_press(win->ctx, x, y, mods_now());
    }
    else {
      g.buttons = 0;
      BK_release(win->ctx, x, y, mods_now());
    }
  }
  else if (button == EVDEV_BTN_RIGHT && down) {
    BK_context_menu(win->ctx, x, y, mods_now());
  }
  else if (button == EVDEV_BTN_RIGHT || button == EVDEV_BTN_MIDDLE) { /* as the X11 host */
    BK_button(win->ctx, x, y, button == EVDEV_BTN_MIDDLE ? 2 : 3, down ? 0 : 1,
              mods_now() & ~BK_BUTTON1);
  }
  else if (button == EVDEV_BTN_SIDE || button == EVDEV_BTN_BACK) {
    BK_button(win->ctx, x, y, BK_BUTTON_BACK, down ? 0 : 1, mods_now() & ~BK_BUTTON1);
  }
  else if (button == EVDEV_BTN_EXTRA || button == EVDEV_BTN_FORWARD) {
    BK_button(win->ctx, x, y, BK_BUTTON_FORWARD, down ? 0 : 1, mods_now() & ~BK_BUTTON1);
  }
}

static void pointer_axis(void *d, struct wl_pointer *p, uint32_t time, uint32_t axis,
                         wl_fixed_t value)
{
  BKWindow *win = g.pointer_win;
  if (!win) {
    return;
  }
  /* a wheel notch is 10 units; touchpads send smaller steps (GHOST scales the same way) */
  const float v = (float)wl_fixed_to_double(value) / 10.0f;
  if (axis == WL_POINTER_AXIS_VERTICAL_SCROLL) {
    BK_scroll(win->ctx, (int)g.px, (int)g.py, 0.0f, -v, mods_now() & ~BK_BUTTON1);
  }
  else {
    BK_scroll(win->ctx, (int)g.px, (int)g.py, v, 0.0f, mods_now() & ~BK_BUTTON1);
  }
}

static struct wl_pointer_listener pointer_listener = {
    .enter = pointer_enter, .leave = pointer_leave, .motion = pointer_motion,
    .button = pointer_button, .axis = pointer_axis};

/* -- seat: keyboard -- */

static void keyboard_keymap(void *d, struct wl_keyboard *kb, uint32_t format, int32_t fd,
                            uint32_t size)
{
  if (format == WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1 && xkb_load()) {
    void *map = mmap(NULL, size, PROT_READ, MAP_PRIVATE, fd, 0);
    if (map != MAP_FAILED) {
      if (!g.xkb_context) {
        g.xkb_context = xkb.xkb_context_new(XKB_CONTEXT_NO_FLAGS);
      }
      struct xkb_keymap *keymap = xkb.xkb_keymap_new_from_buffer(
          g.xkb_context, (const char *)map, size > 0 ? size - 1 : 0, XKB_KEYMAP_FORMAT_TEXT_V1,
          XKB_KEYMAP_COMPILE_NO_FLAGS);
      munmap(map, size);
      if (keymap) {
        if (g.xkb_state) {
          xkb.xkb_state_unref(g.xkb_state);
        }
        if (g.keymap) {
          xkb.xkb_keymap_unref(g.keymap);
        }
        g.keymap = keymap;
        g.xkb_state = xkb.xkb_state_new(keymap);
      }
    }
  }
  close(fd);
}

/* BK_window_focused changed: render a frame so the host's on_frame can read it. */
static void focus_changed(BKWindow *win)
{
  if (win && win->open) {
    bk_wl_window_request_frame(win, 0);
  }
}

static void keyboard_enter(void *d, struct wl_keyboard *kb, uint32_t serial, struct wl_surface *s,
                           struct wl_array *keys)
{
  BKWindow *prev = g.keyboard_win;
  g.keyboard_win = find_window(s);
  g.serial = serial;
  if (prev != g.keyboard_win) {
    focus_changed(prev);
    focus_changed(g.keyboard_win);
  }
}

static void keyboard_leave(void *d, struct wl_keyboard *kb, uint32_t serial, struct wl_surface *s)
{
  BKWindow *win = find_window(s);
  if (g.keyboard_win == win) {
    g.keyboard_win = NULL;
    focus_changed(win);
  }
  g.repeat_key = 0;
}

static void keyboard_key(void *d, struct wl_keyboard *kb, uint32_t serial, uint32_t time,
                         uint32_t key, uint32_t state)
{
  g.serial = serial;
  BKWindow *win = g.keyboard_win;
  if (!win || !win->open) {
    return;
  }
  const bool press = state == WL_KEYBOARD_KEY_STATE_PRESSED;
  send_key(win, key, press);
  if (press && g.keymap && xkb.xkb_keymap_key_repeats(g.keymap, key + 8) && g.repeat_rate > 0) {
    g.repeat_key = key;
    g.repeat_due = now_ms() + g.repeat_delay;
  }
  else if (!press && g.repeat_key == key) {
    g.repeat_key = 0;
  }
}

static void keyboard_modifiers(void *d, struct wl_keyboard *kb, uint32_t serial, uint32_t depressed,
                               uint32_t latched, uint32_t locked, uint32_t group)
{
  if (g.xkb_state) {
    xkb.xkb_state_update_mask(g.xkb_state, depressed, latched, locked, 0, 0, group);
  }
}

static void keyboard_repeat_info(void *d, struct wl_keyboard *kb, int32_t rate, int32_t delay)
{
  g.repeat_rate = rate;
  g.repeat_delay = delay;
}

static struct wl_keyboard_listener keyboard_listener = {
    .keymap = keyboard_keymap, .enter = keyboard_enter, .leave = keyboard_leave,
    .key = keyboard_key, .modifiers = keyboard_modifiers, .repeat_info = keyboard_repeat_info};

/* Calls for the held key until it is released, at the compositor's rate. */
static void run_repeat(void)
{
  if (g.repeat_key && g.keyboard_win && g.repeat_rate > 0) {
    const double t = now_ms();
    while (g.repeat_key && t >= g.repeat_due) {
      send_key(g.keyboard_win, g.repeat_key, true);
      g.repeat_due += 1000.0 / g.repeat_rate;
    }
  }
}

static void seat_capabilities(void *d, struct wl_seat *seat, uint32_t caps)
{
  if ((caps & WL_SEAT_CAPABILITY_POINTER) && !g.pointer) {
    g.pointer = wl_seat_get_pointer(seat);
    wl_pointer_add_listener(g.pointer, &pointer_listener, NULL);
  }
  if ((caps & WL_SEAT_CAPABILITY_KEYBOARD) && !g.keyboard) {
    g.keyboard = wl_seat_get_keyboard(seat);
    wl_keyboard_add_listener(g.keyboard, &keyboard_listener, NULL);
  }
}

static struct wl_seat_listener seat_listener = {.capabilities = seat_capabilities};

/* -- clipboard (GHOST_SystemWayland's data device) -- */

static void offer_mime(void *d, struct wl_data_offer *offer, const char *mime)
{
  if (offer == g.pending_offer && !g.offer_mime) {
    if (!strcmp(mime, "text/plain;charset=utf-8")) {
      g.offer_mime = "text/plain;charset=utf-8";
    }
    else if (!strcmp(mime, "UTF8_STRING")) {
      g.offer_mime = "UTF8_STRING";
    }
    else if (!strcmp(mime, "text/plain")) {
      g.offer_mime = "text/plain";
    }
  }
}

static struct wl_data_offer_listener offer_listener = {.offer = offer_mime};

static void device_data_offer(void *d, struct wl_data_device *dev, struct wl_data_offer *offer)
{
  if (g.pending_offer && g.pending_offer != g.offer) {
    wl_data_offer_destroy(g.pending_offer);
  }
  g.pending_offer = offer;
  g.offer_mime = NULL;
  wl_data_offer_add_listener(offer, &offer_listener, NULL);
}

static void device_selection(void *d, struct wl_data_device *dev, struct wl_data_offer *offer)
{
  if (g.offer && g.offer != offer) {
    wl_data_offer_destroy(g.offer);
  }
  g.offer = offer;
  if (!offer) {
    g.pending_offer = NULL;
    g.offer_mime = NULL;
  }
}

static struct wl_data_device_listener device_listener = {
    .data_offer = device_data_offer, .selection = device_selection};

static void source_send(void *d, struct wl_data_source *src, const char *mime, int32_t fd)
{
  if (g_clip_text) {
    const char *p = g_clip_text;
    size_t left = strlen(p);
    while (left > 0) {
      const ssize_t n = write(fd, p, left);
      if (n <= 0) {
        break;
      }
      p += n;
      left -= (size_t)n;
    }
  }
  close(fd);
}

static void source_cancelled(void *d, struct wl_data_source *src)
{
  if (g.source == src) {
    g.source = NULL;
  }
  wl_data_source_destroy(src);
}

static struct wl_data_source_listener source_listener = {.send = source_send,
                                                               .cancelled = source_cancelled};

static char *host_clipboard_get(void *data)
{
  if (g.source && g_clip_text) {
    return strdup(g_clip_text); /* ours */
  }
  if (!g.data_manager || !g.data_device) {
    /* no clipboard to share: the text copied here */
    return g_clip_text ? strdup(g_clip_text) : NULL;
  }
  if (!g.offer || !g.offer_mime) {
    return NULL;
  }
  int fds[2];
  if (pipe2(fds, O_CLOEXEC) < 0) {
    return NULL;
  }
  wl_data_offer_receive(g.offer, g.offer_mime, fds[1]);
  close(fds[1]);
  wl_display_flush(g.display);
  char *out = NULL;
  size_t len = 0;
  const double until = now_ms() + 300.0;
  for (;;) {
    const double left = until - now_ms();
    struct pollfd pfd = {fds[0], POLLIN, 0};
    if (left <= 0 || poll(&pfd, 1, (int)left) <= 0) {
      break;
    }
    char buf[4096];
    const ssize_t n = read(fds[0], buf, sizeof(buf));
    if (n <= 0) {
      break;
    }
    out = (char *)realloc(out, len + (size_t)n + 1);
    memcpy(out + len, buf, (size_t)n);
    len += (size_t)n;
  }
  close(fds[0]);
  if (out) {
    out[len] = '\0';
  }
  return out;
}

static void host_clipboard_set(void *data, const char *text)
{
  free(g_clip_text);
  g_clip_text = strdup(text ? text : "");
  if (!g.data_manager || !g.data_device) {
    return;
  }
  struct wl_data_source *src = wl_data_device_manager_create_data_source(g.data_manager);
  wl_data_source_add_listener(src, &source_listener, NULL);
  wl_data_source_offer(src, "text/plain;charset=utf-8");
  wl_data_source_offer(src, "UTF8_STRING");
  wl_data_source_offer(src, "text/plain");
  wl_data_device_set_selection(g.data_device, src, g.serial);
  if (g.source) {
    wl_data_source_destroy(g.source);
  }
  g.source = src;
  wl_display_flush(g.display);
}

/* -- outputs and the registry -- */

static void output_geometry(void *d, struct wl_output *o, int32_t x, int32_t y, int32_t pw,
                            int32_t ph, int32_t sub, const char *make, const char *model,
                            int32_t transform)
{
}

static void output_mode(void *d, struct wl_output *o, uint32_t flags, int32_t w, int32_t h,
                        int32_t refresh)
{
  Output *out = (Output *)d;
  if (flags & WL_OUTPUT_MODE_CURRENT) {
    out->w = w;
    out->h = h;
  }
}

static void output_scale(void *d, struct wl_output *o, int32_t factor)
{
  ((Output *)d)->scale = factor;
}

static struct wl_output_listener output_listener = {
    .geometry = output_geometry, .mode = output_mode, .scale = output_scale};

static void wm_base_ping(void *d, struct xdg_wm_base *wm, uint32_t serial)
{
  xdg_wm_base_pong(wm, serial);
}

static struct xdg_wm_base_listener wm_base_listener = {.ping = wm_base_ping};

static void registry_global(void *d, struct wl_registry *reg, uint32_t name, const char *iface,
                            uint32_t version)
{
  if (!strcmp(iface, wl_compositor_interface.name)) {
    g.compositor_version = BK_MIN(version, 6u);
    g.compositor = wl_registry_bind(reg, name, &wl_compositor_interface, g.compositor_version);
  }
  else if (!strcmp(iface, wl_shm_interface.name)) {
    g.shm = wl_registry_bind(reg, name, &wl_shm_interface, 1);
  }
  else if (!strcmp(iface, xdg_wm_base_interface.name)) {
    g.wm_base = wl_registry_bind(reg, name, &xdg_wm_base_interface, 1);
    xdg_wm_base_add_listener(g.wm_base, &wm_base_listener, NULL);
  }
  else if (!strcmp(iface, zxdg_decoration_manager_v1_interface.name)) {
    g.decoration_manager = wl_registry_bind(reg, name, &zxdg_decoration_manager_v1_interface, 1);
  }
  else if (!strcmp(iface, xdg_activation_v1_interface.name)) {
    g.activation = wl_registry_bind(reg, name, &xdg_activation_v1_interface, 1);
  }
  else if (!strcmp(iface, wl_seat_interface.name) && !g.seat) {
    g.seat = wl_registry_bind(reg, name, &wl_seat_interface, BK_MIN(version, 5u));
    wl_seat_add_listener(g.seat, &seat_listener, NULL);
  }
  else if (!strcmp(iface, wl_data_device_manager_interface.name)) {
    g.data_manager = wl_registry_bind(reg, name, &wl_data_device_manager_interface, 1);
  }
  else if (!strcmp(iface, wl_output_interface.name) && g.noutputs < MAX_OUTPUTS) {
    Output *out = &g.outputs[g.noutputs++];
    out->scale = 1;
    out->output = wl_registry_bind(reg, name, &wl_output_interface, BK_MIN(version, 2u));
    wl_output_add_listener(out->output, &output_listener, out);
  }
}

static struct wl_registry_listener registry_listener = {.global = registry_global};

/* libwayland aborts when an event has no listener function ("listener function for opcode N is
 * NULL"), so every slot we do not use gets this. */
static void noop(void) {}

static void fill_noop(void *listener, size_t size)
{
  void (**slot)(void) = (void (**)(void))listener;
  for (size_t i = 0; i < size / sizeof(*slot); i++) {
    if (!slot[i]) {
      slot[i] = noop;
    }
  }
}

/* defined further down: tentative definitions so fill_listeners can reach them */
static struct xdg_surface_listener xdg_surface_listener;
static struct xdg_toplevel_listener toplevel_listener;
static struct wl_surface_listener surface_listener;

static void fill_listeners(void)
{
  fill_noop(&buffer_listener, sizeof(buffer_listener));
  fill_noop(&frame_listener, sizeof(frame_listener));
  fill_noop(&pointer_listener, sizeof(pointer_listener));
  fill_noop(&keyboard_listener, sizeof(keyboard_listener));
  fill_noop(&seat_listener, sizeof(seat_listener));
  fill_noop(&offer_listener, sizeof(offer_listener));
  fill_noop(&device_listener, sizeof(device_listener));
  fill_noop(&source_listener, sizeof(source_listener));
  fill_noop(&output_listener, sizeof(output_listener));
  fill_noop(&wm_base_listener, sizeof(wm_base_listener));
  fill_noop(&registry_listener, sizeof(registry_listener));
  fill_noop(&xdg_surface_listener, sizeof(xdg_surface_listener));
  fill_noop(&toplevel_listener, sizeof(toplevel_listener));
  fill_noop(&surface_listener, sizeof(surface_listener));
}

/* Connects (once): the libraries, the display, the globals a window needs. */
static int open_display(void)
{
  static int state = 0; /* 1 connected, -1 failed */
  if (state) {
    return state > 0;
  }
  state = -1;
  if (!wayland_dynload_client_init(false) || !xkb_load()) {
    return 0;
  }
  fill_listeners();
  g.display = wl_display_connect(NULL);
  if (!g.display) {
    return 0;
  }
  g.registry = wl_display_get_registry(g.display);
  wl_registry_add_listener(g.registry, &registry_listener, NULL);
  wl_display_roundtrip(g.display); /* the globals */
  wl_display_roundtrip(g.display); /* their first events: outputs, seat capabilities */
  if (!g.compositor || !g.shm || !g.wm_base) {
    return 0;
  }
  if (g.data_manager && g.seat) {
    g.data_device = wl_data_device_manager_get_data_device(g.data_manager, g.seat);
    wl_data_device_add_listener(g.data_device, &device_listener, NULL);
  }
  g.cursor_scale = 1;
  for (int i = 0; i < g.noutputs; i++) {
    g.cursor_scale = BK_MAX(g.cursor_scale, g.outputs[i].scale);
  }
  if (wayland_dynload_cursor_init(false)) {
    const char *size = getenv("XCURSOR_SIZE");
    g.cursor_theme = wl_cursor_theme_load(getenv("XCURSOR_THEME"),
                                          (size ? atoi(size) : 24) * g.cursor_scale, g.shm);
    g.cursor_surface = wl_compositor_create_surface(g.compositor);
  }
  g.repeat_rate = 25;
  g.repeat_delay = 600;
  state = 1;
  return 1;
}

int bk_wl_window_supported(void)
{
  return open_display();
}

/* -- host callbacks -- */

static void host_request(void *data)
{
  BKWindow *win = (BKWindow *)data;
  if (win->open) {
    win->render_pending = 1;
  }
}

static void host_set_cursor(void *data, int c)
{
  BKWindow *win = (BKWindow *)data;
  win->cursor = c;
  apply_cursor(win);
}

static void host_warp(void *data, int x, int y)
{
  /* Wayland has no pointer warping for ordinary clients (GHOST needs pointer constraints) */
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

/* -- xdg-shell -- */

static void close_window(BKWindow *win)
{
  if (!win->open) {
    return;
  }
  win->open = 0;
  if (g.pointer_win == win) {
    g.pointer_win = NULL;
  }
  if (g.keyboard_win == win) {
    g.keyboard_win = NULL;
    g.repeat_key = 0;
  }
  if (win->frame_cb) {
    wl_callback_destroy(win->frame_cb);
    win->frame_cb = NULL;
  }
  if (win->decoration) {
    zxdg_toplevel_decoration_v1_destroy(win->decoration);
  }
  xdg_toplevel_destroy(win->toplevel);
  xdg_surface_destroy(win->xdg_surface);
  wl_surface_destroy(win->surface);
  buf_destroy(&win->bufs[0]);
  buf_destroy(&win->bufs[1]);
  win->decoration = NULL;
  win->toplevel = NULL;
  win->xdg_surface = NULL;
  win->surface = NULL;
  g_open--;
  wl_display_flush(g.display);
}

static void xdg_surface_configure(void *d, struct xdg_surface *s, uint32_t serial)
{
  BKWindow *win = (BKWindow *)d;
  xdg_surface_ack_configure(s, serial);
  win->configured = 1;
  win->render_pending = 1;
}

static struct xdg_surface_listener xdg_surface_listener = {.configure =
                                                                     xdg_surface_configure};

static void toplevel_configure(void *d, struct xdg_toplevel *tl, int32_t w, int32_t h,
                               struct wl_array *states)
{
  BKWindow *win = (BKWindow *)d;
  if (w > 0 && h > 0 && (w != win->lw || h != win->lh)) { /* 0: the client picks */
    win->lw = w;
    win->lh = h;
    win->w = w * win->bscale;
    win->h = h * win->bscale;
    win->render_pending = 1;
  }
}

static void toplevel_close(void *d, struct xdg_toplevel *tl)
{
  BKWindow *win = (BKWindow *)d;
  if (!win->cb.on_close || win->cb.on_close(win->cb.data, win)) {
    close_window(win);
  }
}

static struct xdg_toplevel_listener toplevel_listener = {.configure = toplevel_configure,
                                                               .close = toplevel_close};

static void surface_enter(void *d, struct wl_surface *s, struct wl_output *o)
{
  BKWindow *win = (BKWindow *)d;
  if (g.compositor_version >= 6) {
    return; /* preferred_buffer_scale says it */
  }
  for (int i = 0; i < g.noutputs; i++) {
    if (g.outputs[i].output == o && g.outputs[i].scale != win->bscale) {
      apply_scale(win, g.outputs[i].scale);
    }
  }
}

static void surface_leave(void *d, struct wl_surface *s, struct wl_output *o) {}

#  ifdef WL_SURFACE_PREFERRED_BUFFER_SCALE_SINCE_VERSION
static void surface_preferred_scale(void *d, struct wl_surface *s, int32_t factor)
{
  BKWindow *win = (BKWindow *)d;
  if (factor != win->bscale) {
    apply_scale(win, factor);
  }
}
#  endif

static struct wl_surface_listener surface_listener = {
    .enter = surface_enter,
    .leave = surface_leave,
#  ifdef WL_SURFACE_PREFERRED_BUFFER_SCALE_SINCE_VERSION
    .preferred_buffer_scale = surface_preferred_scale,
#  endif
};

/* -- windows -- */

BKWindow *bk_wl_window_new(BKContext *ctx, const char *title, int width, int height)
{
  if (!open_display()) {
    return NULL;
  }
  BKWindow *win = (BKWindow *)calloc(1, sizeof(BKWindow));
  win->ctx = ctx;
  win->prev = ctx->host;
  win->raster = BUI_raster_create(1, 1);
  win->next_timer = 1;
  win->lw = width;
  win->lh = height;
  win->surface = wl_compositor_create_surface(g.compositor);
  wl_surface_add_listener(win->surface, &surface_listener, win);
  win->xdg_surface = xdg_wm_base_get_xdg_surface(g.wm_base, win->surface);
  xdg_surface_add_listener(win->xdg_surface, &xdg_surface_listener, win);
  win->toplevel = xdg_surface_get_toplevel(win->xdg_surface);
  xdg_toplevel_add_listener(win->toplevel, &toplevel_listener, win);
  const char *app_id = getenv("BUI_APP_ID");
  xdg_toplevel_set_app_id(win->toplevel, app_id ? app_id : "bui");
  if (g.decoration_manager) { /* title bar and borders from the compositor, as GHOST asks */
    win->decoration = zxdg_decoration_manager_v1_get_toplevel_decoration(g.decoration_manager,
                                                                         win->toplevel);
    zxdg_toplevel_decoration_v1_set_mode(win->decoration,
                                         ZXDG_TOPLEVEL_DECORATION_V1_MODE_SERVER_SIDE);
  }
  win->open = 1;
  apply_scale(win, 1);
  /* BUI_UI_SCALE sizes the window in design pixels, as the X11 host does */
  win->lw = BK_MAX(1, (int)(width * win->ui_scale / win->bscale));
  win->lh = BK_MAX(1, (int)(height * win->ui_scale / win->bscale));
  win->w = win->lw * win->bscale;
  win->h = win->lh * win->bscale;
  win->next = g_windows;
  g_windows = win;
  g_open++;
  bk_wl_window_set_title(win, title);
  BKHost host = {win,           host_request,       host_set_cursor, host_warp,
                 host_clipboard_get, host_clipboard_set, host_schedule, host_emit};
  BK_context_set_host(ctx, &host);
  wl_surface_commit(win->surface); /* the first commit asks for the configure */
  wl_display_flush(g.display);
  return win;
}

void bk_wl_window_free(BKWindow *win)
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
  if (win->ctx) {
    BK_context_set_host(win->ctx, &win->prev);
  }
  BUI_raster_free(win->raster);
  free(win);
}

void bk_wl_window_set_callbacks(BKWindow *win, const BKWindowCallbacks *cb)
{
  win->cb = *cb;
}

void bk_wl_window_show(BKWindow *win)
{
  if (win->open) { /* a Wayland surface shows with its first buffer, after the configure */
    win->full = 1;
    win->render_pending = 1;
  }
}

void bk_wl_window_close(BKWindow *win)
{
  close_window(win);
}

int bk_wl_window_is_open(BKWindow *win)
{
  return win && win->open;
}

void bk_wl_window_set_title(BKWindow *win, const char *title)
{
  if (win->open) {
    xdg_toplevel_set_title(win->toplevel, title ? title : "");
    wl_display_flush(g.display);
  }
}

void bk_wl_window_set_size(BKWindow *win, int width, int height)
{
  if (win->open) {
    /* the toplevel's size is the compositor's to give; take it now like the other hosts and
     * keep it as what the next configure with no size means */
    win->lw = BK_MAX(1, (int)(width * win->ui_scale / win->bscale));
    win->lh = BK_MAX(1, (int)(height * win->ui_scale / win->bscale));
    win->w = win->lw * win->bscale;
    win->h = win->lh * win->bscale;
    xdg_surface_set_window_geometry(win->xdg_surface, 0, 0, win->lw, win->lh);
    win->full = 1;
    win->render_pending = 1;
  }
}

void bk_wl_window_get_size(BKWindow *win, int *w, int *h)
{
  *w = win->open ? win->w : 0;
  *h = win->open ? win->h : 0;
}

BuiRaster *bk_wl_window_raster(BKWindow *win)
{
  return win->raster;
}

void bk_wl_window_set_gpu(BKWindow *win, int on)
{
  (void)win;
  (void)on; /* the CPU raster only, see kit_window_linux.c */
}

int bk_wl_window_gpu(BKWindow *win)
{
  (void)win;
  return 0;
}

int bk_wl_window_frames(BKWindow *win)
{
  return win->frames;
}

void bk_wl_window_render_now(BKWindow *win)
{
  if (win->open && win->configured) {
    win->full = 1;
    render_window(win);
  }
}

int bk_wl_window_timer(BKWindow *win, int ms)
{
  const int id = win->next_timer++;
  Timer *t = timer_slot(win, id);
  if (t) {
    t->id = id;
    t->due = now_ms() + BK_MAX(0, ms);
  }
  return id;
}

void bk_wl_window_cancel_timer(BKWindow *win, int id)
{
  for (int i = 0; i < MAX_TIMERS; i++) {
    if (win->timers[i].id == id) {
      win->timers[i].id = 0;
    }
  }
}

void bk_wl_window_request_frame(BKWindow *win, int vsync)
{
  win->want_frame = 1;
  win->frame_vsync = vsync;
  win->render_pending = 1;
}

int bk_wl_window_focused(BKWindow *win)
{
  return win->open && g.keyboard_win == win;
}

/* xdg_activation_v1: a compositor only raises a window for a token it handed out (here, from a
 * notification server's ActivationToken). Without the global or a token there is nothing to ask. */
void bk_wl_window_activate(BKWindow *win, const char *token)
{
  if (!win->open || !g.activation || !token || !token[0]) {
    return;
  }
  xdg_activation_v1_activate(g.activation, token, win->surface);
  wl_display_flush(g.display);
}

void *bk_wl_window_handle(BKWindow *win)
{
  return win->surface;
}

/* GHOST_WindowWayland: min / max size, maximize and minimize; the rest is the compositor's */
void bk_wl_window_wm(BKWindow *win, const char *key, double a, double b)
{
  if (!win->open) {
    return;
  }
  /* logical px, like BK_window_set_size: x ui_scale is pixels, / bscale surface units */
  if (!strcmp(key, "minsize")) {
    xdg_toplevel_set_min_size(win->toplevel, (int)(a * win->ui_scale / win->bscale),
                              (int)(b * win->ui_scale / win->bscale));
  }
  else if (!strcmp(key, "maxsize")) {
    xdg_toplevel_set_max_size(win->toplevel, (int)(a * win->ui_scale / win->bscale),
                              (int)(b * win->ui_scale / win->bscale));
  }
  else if (!strcmp(key, "state")) {
    if (a == 1) {
      xdg_toplevel_set_minimized(win->toplevel);
    }
    else if (a == 3) {
      xdg_toplevel_set_maximized(win->toplevel);
    }
    else if (a == 0) {
      xdg_toplevel_unset_maximized(win->toplevel);
    }
  }
  wl_display_flush(g.display);
}

int bk_wl_window_wm_get(BKWindow *win, const char *key, double *r_a, double *r_b)
{
  if (!strcmp(key, "screen")) {
    *r_a = g.noutputs ? g.outputs[0].w : 0;
    *r_b = g.noutputs ? g.outputs[0].h : 0;
    return g.noutputs > 0;
  }
  if (!strcmp(key, "scale")) {
    *r_a = *r_b = win ? win->ui_scale : 1.0;
    return 1;
  }
  if (!strcmp(key, "pointer")) { /* Wayland gives no global position: the window's own */
    *r_a = g.px;
    *r_b = g.py;
    return g.pointer_win != NULL;
  }
  return 0; /* "pos", "state": not available to a Wayland client */
}

/* -- the loop -- */

static void run_timers(void)
{
  const double t = now_ms();
  for (BKWindow *win = g_windows; win; win = win->next) {
    for (int i = 0; i < MAX_TIMERS && win->open; i++) {
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

/* Milliseconds until the next timer, frame or key repeat (-1: nothing to wait for). */
static double next_wait(void)
{
  double wait = -1.0;
  const double t = now_ms();
  for (BKWindow *win = g_windows; win; win = win->next) {
    if (!win->open) {
      continue;
    }
    if (win->render_pending && win->configured && !win->frame_cb) {
      const double hold = win->hold_until - t;
      if (hold <= 0.0) {
        return 0.0;
      }
      wait = wait < 0 ? hold : BK_MIN(wait, hold);
    }
    for (int i = 0; i < MAX_TIMERS; i++) {
      if (win->timers[i].id != 0) {
        const double d = BK_MAX(0.0, win->timers[i].due - t);
        wait = wait < 0 ? d : BK_MIN(wait, d);
      }
    }
  }
  if (g.repeat_key) {
    const double d = BK_MAX(0.0, g.repeat_due - t);
    wait = wait < 0 ? d : BK_MIN(wait, d);
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

/* One turn: events, timers, renders; blocks up to `block_ms` (-1: forever, 0: not at all). */
static void turn(double block_ms)
{
  struct wl_display *d = g.display;
  while (wl_display_prepare_read(d) != 0) {
    wl_display_dispatch_pending(d);
  }
  wl_display_flush(d);
  struct pollfd fds[2] = {{wl_display_get_fd(d), POLLIN, 0}, {g_wake_pipe[0], POLLIN, 0}};
  const int nfds = g_wake_pipe[0] >= 0 ? 2 : 1;
  int timeout = block_ms < 0 ? -1 : (int)ceil(block_ms);
  if (block_ms != 0.0) {
    const double rw = g.repeat_key ? BK_MAX(0.0, g.repeat_due - now_ms()) : -1.0;
    if (rw >= 0.0 && (timeout < 0 || rw < timeout)) {
      timeout = (int)ceil(rw);
    }
  }
  const int ready = poll(fds, (nfds_t)nfds, timeout);
  if (ready > 0 && (fds[0].revents & POLLIN)) {
    wl_display_read_events(d);
  }
  else {
    wl_display_cancel_read(d);
  }
  if (ready > 0 && nfds == 2 && (fds[1].revents & POLLIN)) {
    drain_wake();
  }
  wl_display_dispatch_pending(d);
  run_repeat();
  run_timers();
  for (BKWindow *win = g_windows; win; win = win->next) {
    if (win->open && win->configured && win->render_pending && !win->frame_cb &&
        win->hold_until <= now_ms())
    {
      render_window(win);
    }
  }
  wl_display_flush(d);
}

void bk_wl_window_set_stop_check(int (*stop)(void))
{
  g_stop = stop;
}

void bk_wl_window_quit(void)
{
  g_quit = 1;
  bk_wl_window_wake();
}

void bk_wl_window_run(void)
{
  if (!g.display) {
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

int bk_wl_window_pump(void)
{
  if (g.display) {
    turn(0.0);
  }
  return g_open;
}

int bk_wl_window_wake_ready(void)
{
  if (g_wake_pipe[0] < 0 && pipe(g_wake_pipe) == 0) {
    fcntl(g_wake_pipe[0], F_SETFL, O_NONBLOCK);
    fcntl(g_wake_pipe[1], F_SETFL, O_NONBLOCK);
  }
  return g_wake_pipe[0] >= 0;
}

int bk_wl_window_wake_init(void (*cb)(void))
{
  g_wake_cb = cb;
  return bk_wl_window_wake_ready();
}

/* Thread-safe: wakes the loop, which calls the wake callback. */
void bk_wl_window_wake(void)
{
  if (g_wake_pipe[1] >= 0) {
    const char c = 1;
    (void)!write(g_wake_pipe[1], &c, 1);
  }
}

void bk_wl_window_set_keep_alive(int on)
{
  g_keep_alive = on;
}

#endif /* BUI_WINDOW_WAYLAND */
