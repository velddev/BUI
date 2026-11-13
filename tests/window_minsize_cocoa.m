/* The Cocoa host's "minsize": logical px in, the content's minimum in points
 * out (x UI scale / backing scale), scaled once. Opening a 900x560 minimum
 * must not ask for more points than that on a Retina display (task 170: the
 * Rust binding scaled it too, 1800x1120 points, wider than a 14" MacBook).
 * Exit 77 (skipped) without a display.
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */

#import <Cocoa/Cocoa.h>
#include <math.h>
#include <stdio.h>

#include "BUI_kit.h"

int main(void)
{
  if (!BK_window_supported()) {
    printf("no display: skipped\n");
    return 77;
  }
  BKContext *ctx = BK_context_new(BK_node_new("frame"));
  BKWindow *win = BK_window_new(ctx, "BUI minsize", 1000, 700);
  if (!win) {
    printf("window failed\n");
    return 1;
  }
  BK_window_wm(win, "minsize", 900, 560);
  NSWindow *nw = (__bridge NSWindow *)BK_window_handle(win);
  double scale = 1.0, unused = 0.0;
  BK_window_wm_get(win, "scale", &scale, &unused);
  const double px = [nw backingScaleFactor];
  const NSSize mn = [nw contentMinSize];
  const double want_w = 900 * scale / px, want_h = 560 * scale / px;
  printf("scale %.2f backing %.2f min %.0fx%.0f points, want %.0fx%.0f\n", scale, px,
         mn.width, mn.height, want_w, want_h);
  const int ok = fabs(mn.width - want_w) < 1.0 && fabs(mn.height - want_h) < 1.0;
  BK_window_free(win);
  BK_context_free(ctx);
  return ok ? 0 : 1;
}
