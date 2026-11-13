/* No native window host on this platform: BK_window_supported() returns 0
 * and hosts use Tk/Qt.
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */

#include "kit_intern.h"

#if !defined(_WIN32) && !defined(BUI_WINDOW_X11) && !defined(BUI_WINDOW_COCOA)


int BK_window_supported(void)
{
  return 0;
}
BKWindow *BK_window_new(BKContext *ctx, const char *title, int width, int height)
{
  return NULL;
}
void BK_window_free(BKWindow *win) {}
void BK_window_set_callbacks(BKWindow *win, const BKWindowCallbacks *cb) {}
void BK_window_show(BKWindow *win) {}
void BK_window_close(BKWindow *win) {}
int BK_window_is_open(BKWindow *win)
{
  return 0;
}
void BK_window_set_title(BKWindow *win, const char *title) {}
void BK_window_set_size(BKWindow *win, int width, int height) {}
void BK_window_get_size(BKWindow *win, int *w, int *h)
{
  *w = *h = 0;
}
BuiRaster *BK_window_raster(BKWindow *win)
{
  return NULL;
}
void BK_window_set_gpu(BKWindow *win, int on) {}
int BK_window_gpu(BKWindow *win)
{
  return 0;
}
int BK_window_frames(BKWindow *win)
{
  return 0;
}
void BK_window_render_now(BKWindow *win) {}
int BK_window_timer(BKWindow *win, int ms)
{
  return 0;
}
void BK_window_cancel_timer(BKWindow *win, int id) {}
void BK_window_request_frame(BKWindow *win, int vsync) {}
void *BK_window_handle(BKWindow *win)
{
  return NULL;
}
void BK_window_wm(BKWindow *win, const char *key, double a, double b) {}
int BK_window_focused(BKWindow *win)
{
  return -1;
}
void BK_window_activate(BKWindow *win, const char *token) {}
int BK_window_wm_get(BKWindow *win, const char *key, double *r_a, double *r_b)
{
  *r_a = *r_b = 0.0;
  return 0;
}
void BK_window_run(void) {}
int BK_window_pump(void)
{
  return 0;
}
void BK_window_quit(void) {}
void BK_window_set_stop_check(int (*stop)(void)) {}
int bk_window_wake_ready(void)
{
  return 0;
}
int BK_window_wake_init(void (*cb)(void))
{
  (void)cb;
  return 0;
}
void BK_window_wake(void) {}
void BK_window_set_keep_alive(int on)
{
  (void)on;
}

#endif
