/* Native window resize test: every frame is laid out at the newest size. Before the first
 * frame at a new size the host is called (`on_frame`) with that size already in
 * BK_window_get_size, so a host that lays out by the window's size never presents its last
 * layout stretched to the new one (centred text jumped back and forth while a window was
 * dragged wider), and when several sizes come between two frames the frame is at the last one,
 * never an earlier one. Exit 0 when every frame rendered at the size the host was last told
 * of, 77 (skipped) without a display.
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */

#include <stdio.h>

#include "BUI_kit.h"
#include "GPU_raster.h"

enum { STEPS = 4, MAX_SIZES = 3 };

/* each step asks for these sizes back to back, with no frame between them */
static const int g_steps[STEPS][MAX_SIZES][2] = {
    {{480, 320}},
    {{481, 320}},
    {{530, 340}, {505, 351}, {497, 333}},
    {{470, 310}, {612, 300}},
};
static int g_step = 0;
/* the size the host was last told of (on_frame) */
static int g_told[2] = {0, 0};
static int g_ntold = 0;
static int g_failures = 0;
static int g_checked = 0;

static int sizes_in(int step)
{
  int n = 0;
  while (n < MAX_SIZES && g_steps[step][n][0] > 0) {
    n++;
  }
  return n;
}

static void on_frame(void *data, BKWindow *win)
{
  (void)data;
  BK_window_get_size(win, &g_told[0], &g_told[1]);
  g_ntold++;
}

/* the frame drawn last (the raster) is at the size the host was last told of */
static void check_drawn(BKWindow *win, const char *when)
{
  BuiRaster *r = BK_window_raster(win);
  const int rw = BUI_raster_width(r), rh = BUI_raster_height(r);
  int w, h;
  BK_window_get_size(win, &w, &h);
  g_checked++;
  if (rw != g_told[0] || rh != g_told[1]) {
    printf("%s: a frame rendered at %dx%d, but the host was last told %dx%d\n", when, rw, rh,
           g_told[0], g_told[1]);
    g_failures++;
  }
  if (rw != w || rh != h) {
    printf("%s: the last frame is %dx%d, the window %dx%d\n", when, rw, rh, w, h);
    g_failures++;
  }
}

static void on_timer(void *data, BKWindow *win, int id)
{
  (void)data;
  (void)id;
  char when[64];
  snprintf(when, sizeof(when), "after step %d", g_step);
  check_drawn(win, when);
  if (g_step < STEPS) {
    const int n = sizes_in(g_step);
    for (int i = 0; i < n; i++) {
      BK_window_set_size(win, g_steps[g_step][i][0], g_steps[g_step][i][1]);
    }
    g_step++;
    BK_window_timer(win, 200);
  }
  else {
    BK_window_quit();
  }
}

int main(void)
{
  if (!BK_window_supported()) {
    printf("no display: skipped\n");
    return 77;
  }
  BKNode *root = BK_node_new("frame");
  BKNode *label = BK_node_new("label");
  BK_set_str(label, "text", "resize");
  BK_node_add(root, label, -1);
  BKContext *ctx = BK_context_new(root);
  BKWindow *win = BK_window_new(ctx, "BUI resize test", 460, 300);
  if (!win) {
    printf("window failed\n");
    return 1;
  }
  BKWindowCallbacks cb = {ctx, on_timer, NULL, on_frame};
  BK_window_set_callbacks(win, &cb);
  BK_window_show(win);
  BK_window_timer(win, 300);
  BK_window_run();
  const int ok = g_failures == 0 && g_checked == STEPS + 1 && g_ntold > 1;
  printf("frames %d, told %d times, checked %d frames, %d failures\n", BK_window_frames(win),
         g_ntold, g_checked, g_failures);
  BK_window_free(win);
  BK_context_free(ctx);
  BK_node_free(root);
  return ok ? 0 : 1;
}
