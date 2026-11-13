/* Native window resize test: the first frame at a new size is announced to the host
 * (`on_frame`) before it renders, with the new size already in BK_window_get_size, so a
 * host that lays out by the window's size never presents its last layout stretched to the
 * new one (centred text jumped back and forth while a window was dragged wider). Exit 0 when
 * every size the window rendered at was announced first, 77 (skipped) without a display.
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */

#include <stdio.h>

#include "BUI_kit.h"
#include "GPU_raster.h"

enum { SIZES = 3 };

static const int g_sizes[SIZES][2] = {{480, 320}, {481, 320}, {497, 333}};
static int g_step = 0;
/* sizes the host was told of before a render at them: (window size, raster size then) */
static int g_told[64][4];
static int g_ntold = 0;
/* sizes frames were rendered at (the raster's size after each turn) */
static int g_drawn[64][2];
static int g_ndrawn = 0;

static void on_frame(void *data, BKWindow *win)
{
  (void)data;
  int w, h;
  BK_window_get_size(win, &w, &h);
  BuiRaster *r = BK_window_raster(win);
  if (g_ntold < 64) {
    g_told[g_ntold][0] = w;
    g_told[g_ntold][1] = h;
    g_told[g_ntold][2] = BUI_raster_width(r);
    g_told[g_ntold][3] = BUI_raster_height(r);
    g_ntold++;
  }
}

static void on_timer(void *data, BKWindow *win, int id)
{
  (void)data;
  (void)id;
  BuiRaster *r = BK_window_raster(win);
  if (g_ndrawn < 64) {
    g_drawn[g_ndrawn][0] = BUI_raster_width(r);
    g_drawn[g_ndrawn][1] = BUI_raster_height(r);
    g_ndrawn++;
  }
  if (g_step < SIZES) {
    BK_window_set_size(win, g_sizes[g_step][0], g_sizes[g_step][1]);
    g_step++;
    BK_window_timer(win, 150);
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
  int ok = g_ndrawn > SIZES;
  /* every size a frame was rendered at was announced while the raster had another */
  for (int i = 0; i < g_ndrawn; i++) {
    int told = 0;
    for (int j = 0; j < g_ntold; j++) {
      told |= g_told[j][0] == g_drawn[i][0] && g_told[j][1] == g_drawn[i][1] &&
              (g_told[j][2] != g_drawn[i][0] || g_told[j][3] != g_drawn[i][1]);
    }
    if (!told) {
      printf("rendered at %dx%d before the host was told of the size\n", g_drawn[i][0],
             g_drawn[i][1]);
      ok = 0;
    }
  }
  printf("frames %d, told %d sizes, rendered at %d sizes\n", BK_window_frames(win), g_ntold,
         g_ndrawn);
  BK_window_free(win);
  BK_context_free(ctx);
  BK_node_free(root);
  return ok ? 0 : 1;
}
