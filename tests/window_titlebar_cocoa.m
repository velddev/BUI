/* The Cocoa host's "titlebar": the content runs under a transparent title bar and keeps its
 * size, a press anywhere on the bar but the traffic lights reaches the content view (so the
 * host's tabs and buttons there take clicks, and its drag areas move the window), and
 * "traffic_lights" centres the buttons where asked, also after a resize.
 * Exit 77 (skipped) without a display.
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */

#import <Cocoa/Cocoa.h>
#include <math.h>
#include <stdio.h>

#include "BUI_kit.h"

static int g_fail = 0;
#define CHECK(c) \
  do { \
    if (!(c)) { \
      printf("FAIL %s:%d: %s\n", __FILE__, __LINE__, #c); \
      g_fail = 1; \
    } \
  } while (0)

/* The close button's left edge and vertical centre from the window's top-left (points). */
static NSPoint close_at(NSWindow *nw)
{
  NSButton *close = [nw standardWindowButton:NSWindowCloseButton];
  const NSRect r = [close convertRect:[close bounds] toView:nil];
  return NSMakePoint(NSMinX(r), [nw frame].size.height - NSMidY(r));
}

int main(void)
{
  if (!BK_window_supported()) {
    printf("no display: skipped\n");
    return 77;
  }
  BKContext *ctx = BK_context_new(BK_node_new("frame"));
  BKWindow *win = BK_window_new(ctx, "BUI titlebar", 1000, 700);
  if (!win) {
    printf("window failed\n");
    return 1;
  }
  NSWindow *nw = (__bridge NSWindow *)BK_window_handle(win);
  double scale = 1.0, unused = 0.0;
  BK_window_wm_get(win, "scale", &scale, &unused);
  const double k = scale / [nw backingScaleFactor]; /* logical px -> points */
  const NSSize before = [[nw contentView] frame].size;
  BK_window_wm(win, "titlebar", 1, 0);
  BK_window_wm(win, "traffic_lights", 20, 24);
  BK_window_show(win);

  CHECK([nw styleMask] & NSWindowStyleMaskFullSizeContentView);
  CHECK([nw titlebarAppearsTransparent]);
  const NSSize after = [[nw contentView] frame].size;
  printf("content %.0fx%.0f -> %.0fx%.0f points, window %.0f high\n", before.width,
         before.height, after.width, after.height, [nw frame].size.height);
  CHECK(fabs(after.width - before.width) < 1.0 && fabs(after.height - before.height) < 1.0);
  CHECK(fabs(after.height - [nw frame].size.height) < 1.0); /* under the title bar */

  /* presses on the bar, away from the lights, reach the content view */
  NSView *theme = [[nw contentView] superview];
  const CGFloat top = [theme frame].size.height;
  for (int i = 0; i < 4; i++) {
    const NSPoint p = NSMakePoint(200 + i * 200, top - (4 + i * 8));
    NSView *hit = [theme hitTest:p];
    printf("hit at %.0f,%.0f from the top: %s\n", p.x, top - p.y, [[hit className] UTF8String]);
    CHECK(hit == [nw contentView]);
  }

  NSPoint c = close_at(nw);
  printf("close button at x %.1f, centre %.1f from the top; want %.1f, %.1f\n", c.x, c.y,
         20 * k, 24 * k);
  CHECK(fabs(c.x - 20 * k) < 1.0 && fabs(c.y - 24 * k) < 1.0);
  [nw setContentSize:NSMakeSize(after.width + 50, after.height + 40)];
  c = close_at(nw);
  printf("after a resize: x %.1f, centre %.1f\n", c.x, c.y);
  CHECK(fabs(c.x - 20 * k) < 1.0 && fabs(c.y - 24 * k) < 1.0);

  BK_window_free(win);
  BK_context_free(ctx);
  return g_fail;
}
