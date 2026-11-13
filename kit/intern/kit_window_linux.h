/* The two Linux window hosts, X11 (kit_window_x11.c) and Wayland (kit_window_wayland.c).
 *
 * Each implements the whole BK_window_* API under its own prefix, bk_x11_ and bk_wl_, and
 * kit_window_linux.c defines BK_window_* on top: it picks the host once, on the first call, like
 * GHOST_ISystem::createSystem (Wayland when the session has it, else X11). Built without
 * Wayland, kit_window_x11.c keeps the public names and none of this is used.
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */

#pragma once

#ifdef BUI_WINDOW_WAYLAND

#  include "kit_intern.h"

/* Every function of the window API: (return type, name, parameters, arguments). The first list
 * is answered by the chosen host; the second is process-wide state both hosts keep (the loop's
 * wake-up, keep-alive, the stop check), which can be set before any window exists, so both
 * hosts get it and the one that runs uses it. */
#  define BK_WINDOW_GLOBAL_FUNCS(X) \
    X(void, quit, (void), ()) \
    X(void, set_stop_check, (int (*stop)(void)), (stop)) \
    X(int, wake_init, (void (*cb)(void)), (cb)) \
    X(void, wake, (void), ()) \
    X(void, set_keep_alive, (int on), (on))

#  define BK_WINDOW_FUNCS(X) \
    X(BKWindow *, new, (BKContext * ctx, const char *title, int width, int height), \
      (ctx, title, width, height)) \
    X(void, free, (BKWindow * win), (win)) \
    X(void, set_callbacks, (BKWindow * win, const BKWindowCallbacks *cb), (win, cb)) \
    X(void, show, (BKWindow * win), (win)) \
    X(void, close, (BKWindow * win), (win)) \
    X(int, is_open, (BKWindow * win), (win)) \
    X(void, set_title, (BKWindow * win, const char *title), (win, title)) \
    X(void, set_size, (BKWindow * win, int width, int height), (win, width, height)) \
    X(void, get_size, (BKWindow * win, int *r_width, int *r_height), (win, r_width, r_height)) \
    X(BuiRaster *, raster, (BKWindow * win), (win)) \
    X(void, set_gpu, (BKWindow * win, int on), (win, on)) \
    X(int, gpu, (BKWindow * win), (win)) \
    X(int, frames, (BKWindow * win), (win)) \
    X(void, render_now, (BKWindow * win), (win)) \
    X(int, timer, (BKWindow * win, int ms), (win, ms)) \
    X(void, cancel_timer, (BKWindow * win, int id), (win, id)) \
    X(void, request_frame, (BKWindow * win, int vsync), (win, vsync)) \
    X(void *, handle, (BKWindow * win), (win)) \
    X(void, wm, (BKWindow * win, const char *key, double a, double b), (win, key, a, b)) \
    X(int, wm_get, (BKWindow * win, const char *key, double *r_a, double *r_b), \
      (win, key, r_a, r_b)) \
    X(void, run, (void), ()) \
    X(int, pump, (void), ()) \
    X(int, focused, (BKWindow * win), (win)) \
    X(void, activate, (BKWindow * win, const char *token), (win, token))

/* The per-backend prototypes (and bk_window_wake_ready). */
#  define BK_WINDOW_DECLARE(ret, name, params, args) \
    ret bk_x11_window_##name params; \
    ret bk_wl_window_##name params;
BK_WINDOW_FUNCS(BK_WINDOW_DECLARE)
BK_WINDOW_GLOBAL_FUNCS(BK_WINDOW_DECLARE)
int bk_x11_window_supported(void);
int bk_wl_window_supported(void);
#  undef BK_WINDOW_DECLARE
int bk_x11_window_wake_ready(void);
int bk_wl_window_wake_ready(void);

/* kit_window_x11.c: its public names become bk_x11_*. */
#  ifdef BK_WINDOW_X11_HOST
#    define BK_window_supported bk_x11_window_supported
#    define BK_window_new bk_x11_window_new
#    define BK_window_free bk_x11_window_free
#    define BK_window_set_callbacks bk_x11_window_set_callbacks
#    define BK_window_show bk_x11_window_show
#    define BK_window_close bk_x11_window_close
#    define BK_window_is_open bk_x11_window_is_open
#    define BK_window_set_title bk_x11_window_set_title
#    define BK_window_set_size bk_x11_window_set_size
#    define BK_window_get_size bk_x11_window_get_size
#    define BK_window_raster bk_x11_window_raster
#    define BK_window_set_gpu bk_x11_window_set_gpu
#    define BK_window_gpu bk_x11_window_gpu
#    define BK_window_frames bk_x11_window_frames
#    define BK_window_render_now bk_x11_window_render_now
#    define BK_window_timer bk_x11_window_timer
#    define BK_window_cancel_timer bk_x11_window_cancel_timer
#    define BK_window_request_frame bk_x11_window_request_frame
#    define BK_window_handle bk_x11_window_handle
#    define BK_window_wm bk_x11_window_wm
#    define BK_window_wm_get bk_x11_window_wm_get
#    define BK_window_focused bk_x11_window_focused
#    define BK_window_activate bk_x11_window_activate
#    define BK_window_run bk_x11_window_run
#    define BK_window_pump bk_x11_window_pump
#    define BK_window_quit bk_x11_window_quit
#    define BK_window_set_stop_check bk_x11_window_set_stop_check
#    define BK_window_wake_init bk_x11_window_wake_init
#    define BK_window_wake bk_x11_window_wake
#    define BK_window_set_keep_alive bk_x11_window_set_keep_alive
#    define bk_window_wake_ready bk_x11_window_wake_ready
#  endif

#endif /* BUI_WINDOW_WAYLAND */
