/* Native window smoke test: opens a window with a node editor, renders,
 * runs timers and the loop, and quits. Exit 0 when frames were presented,
 * 77 (skipped) without a display.
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */

#include <stdio.h>

#include "BUI_kit.h"

static int g_ticks = 0;
static int g_focused_ok = 1; /* BK_window_focused only answers 1, 0 or -1 */

static void on_timer(void *data, BKWindow *win, int id)
{
  BKContext *ctx = (BKContext *)data;
  g_ticks++;
  /* some input through the same path the window uses */
  BK_motion(ctx, 100, 100, 0);
  BK_wheel(ctx, 200, 150, 1, 0);
  if (g_ticks == 1) {
    BK_window_activate(win, NULL); /* no token: Wayland asks nothing, the rest raise it */
  }
  const int focused = BK_window_focused(win);
  g_focused_ok &= focused >= -1 && focused <= 1;
  if (g_ticks < 5) {
    BK_window_timer(win, 50);
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
  BKNode *ed = BK_node_new("nodeeditor");
  for (int i = 0; i < 2; i++) {
    BKNode *n = BK_node_new("graphnode");
    BK_set_str(n, "label", i ? "Math" : "Value");
    BK_set_num(n, "nx", i * 220.0);
    const char *sockets[] = {"o|Value|float", "i|A|float|0.5"};
    BK_set_strs(n, "items", sockets, i ? 2 : 1);
    BK_node_add(ed, n, -1);
  }
  const char *links[] = {"0 0 1 0"};
  BK_set_strs(ed, "items", links, 1);
  BKContext *ctx = BK_context_new(ed);
  BKWindow *win = BK_window_new(ctx, "BUI X11 smoke", 480, 320);
  if (!win) {
    printf("window failed\n");
    return 1;
  }
  BKWindowCallbacks cb = {ctx, on_timer, NULL, NULL};
  BK_window_set_callbacks(win, &cb);
  BK_window_show(win);
  BK_window_timer(win, 300);
  BK_window_run();
  const int frames = BK_window_frames(win);
  int w, h;
  BK_window_get_size(win, &w, &h);
  printf("frames %d ticks %d size %dx%d gpu %d focused %d\n", frames, g_ticks, w, h,
         BK_window_gpu(win), BK_window_focused(win));
  BK_window_free(win);
  BK_context_free(ctx);
  BK_node_free(ed);
  return frames > 0 && g_ticks >= 5 && g_focused_ok ? 0 : 1;
}
