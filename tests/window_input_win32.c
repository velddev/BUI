/* SPDX-License-Identifier: GPL-2.0-or-later */

#include <stdio.h>
#include <string.h>

#include "BUI_kit.h"

#ifndef _WIN32
int main(void)
{
  printf("not Windows: skipped\n");
  return 77;
}
#else

#  define WIN32_LEAN_AND_MEAN
#  include <windows.h>

static int g_fail = 0;
#  define CHECK(c) \
    do { \
      if (!(c)) { \
        printf("FAIL %s:%d: %s\n", __FILE__, __LINE__, #c); \
        g_fail = 1; \
      } \
    } while (0)

static int g_frames = 0;
static int g_animate = 0;
static BKNode *g_canvas = NULL;
static int g_middle_presses = 0;
static int g_middle_releases = 0;

static void on_frame(void *data, BKWindow *win)
{
  (void)data;
  g_frames++;
  if (g_animate) {
    BK_window_request_frame(win, 0);
  }
}

static void emit(void *data, BKNode *node, int kind, void *ref, double a, double b)
{
  (void)data;
  (void)a;
  (void)b;
  if (node != g_canvas || !ref || (kind != BK_EV_PRESS && kind != BK_EV_RELEASE)) {
    return;
  }
  int mods = 0, button = 0;
  if (sscanf((const char *)ref, "%d %d", &mods, &button) == 2 && button == 2) {
    if (kind == BK_EV_PRESS) {
      g_middle_presses++;
    }
    else {
      g_middle_releases++;
    }
  }
}

static int render_posted(HWND h)
{
  MSG m;
  return PeekMessageW(&m, h, WM_APP + 1, WM_APP + 1, PM_NOREMOVE) != 0;
}

static void dispatch_for(DWORD ms, HWND h, int *posted_after_frame)
{
  const DWORD end = GetTickCount() + ms;
  while (GetTickCount() < end) {
    MSG m;
    if (!PeekMessageW(&m, NULL, 0, 0, PM_REMOVE)) {
      Sleep(1);
      continue;
    }
    const int was = g_frames;
    TranslateMessage(&m);
    DispatchMessageW(&m);
    if (posted_after_frame && g_frames != was && render_posted(h)) {
      (*posted_after_frame)++;
    }
  }
}

static void frames_in_a_row_leave_input_first_in_line(BKWindow *win, HWND h)
{
  g_animate = 1;
  BK_window_request_frame(win, 0);
  const int start = g_frames;
  int posted = 0;
  dispatch_for(500, h, &posted);
  g_animate = 0;
  dispatch_for(100, h, NULL);
  printf("%d frames in a row, %d left a posted render ahead of input\n", g_frames - start, posted);
  CHECK(g_frames - start > 10);
  CHECK(posted == 0);
}

static void the_middle_button_captures_the_pointer_until_released(HWND h)
{
  const LPARAM at = MAKELPARAM(40, 40);
  const int releases = g_middle_releases;
  SendMessageW(h, WM_MBUTTONDOWN, MK_MBUTTON, at);
  CHECK(GetCapture() == h);
  SendMessageW(h, WM_MOUSEMOVE, MK_MBUTTON, MAKELPARAM(40, 90));
  CHECK(GetCapture() == h);
  CHECK(g_middle_releases == releases);
  SendMessageW(h, WM_MBUTTONUP, 0, MAKELPARAM(40, 90));
  CHECK(GetCapture() == NULL);
  CHECK(g_middle_releases == releases + 1);
}

static void a_move_without_the_middle_button_releases_it(HWND h)
{
  const int releases = g_middle_releases;
  SendMessageW(h, WM_MBUTTONDOWN, MK_MBUTTON, MAKELPARAM(40, 40));
  SendMessageW(h, WM_MOUSEMOVE, 0, MAKELPARAM(40, 90));
  CHECK(GetCapture() == NULL);
  CHECK(g_middle_releases == releases + 1);
  SendMessageW(h, WM_MOUSEMOVE, 0, MAKELPARAM(40, 95));
  CHECK(g_middle_releases == releases + 1);
}

static void losing_the_capture_releases_the_middle_button(HWND h)
{
  const int releases = g_middle_releases;
  SendMessageW(h, WM_MBUTTONDOWN, MK_MBUTTON, MAKELPARAM(40, 40));
  CHECK(GetCapture() == h);
  ReleaseCapture();
  CHECK(g_middle_releases == releases + 1);
  SendMessageW(h, WM_MOUSEMOVE, 0, MAKELPARAM(40, 45));
  CHECK(g_middle_releases == releases + 1);
}

static void a_left_release_keeps_a_held_middle_capture(HWND h)
{
  SendMessageW(h, WM_MBUTTONDOWN, MK_MBUTTON, MAKELPARAM(40, 40));
  SendMessageW(h, WM_LBUTTONDOWN, MK_LBUTTON | MK_MBUTTON, MAKELPARAM(40, 40));
  SendMessageW(h, WM_LBUTTONUP, MK_MBUTTON, MAKELPARAM(40, 40));
  CHECK(GetCapture() == h);
  SendMessageW(h, WM_MBUTTONUP, 0, MAKELPARAM(40, 40));
  CHECK(GetCapture() == NULL);
}

int main(void)
{
  if (!BK_window_supported()) {
    printf("no display: skipped\n");
    return 77;
  }
  BKNode *root = BK_node_new("column");
  g_canvas = BK_node_new("canvas");
  BK_node_add(root, g_canvas, -1);
  BK_geom_set_manager(g_canvas, BK_GEOM_PLACE);
  BK_geom_set(g_canvas, "relwidth", 1.0);
  BK_geom_set(g_canvas, "relheight", 1.0);
  BK_set_num(g_canvas, "events", 1.0);
  BKContext *ctx = BK_context_new(root);
  BKHost host = {0};
  host.emit = emit;
  BK_context_set_host(ctx, &host);
  BKWindow *win = BK_window_new(ctx, "BUI input", 320, 200);
  if (!win) {
    printf("window failed\n");
    return 1;
  }
  BKWindowCallbacks cb = {0};
  cb.on_frame = on_frame;
  BK_window_set_callbacks(win, &cb);
  BK_window_show(win);
  HWND h = (HWND)BK_window_handle(win);
  dispatch_for(200, h, NULL);

  frames_in_a_row_leave_input_first_in_line(win, h);

  BK_capture_pointer(ctx, g_canvas);
  the_middle_button_captures_the_pointer_until_released(h);
  a_move_without_the_middle_button_releases_it(h);
  losing_the_capture_releases_the_middle_button(h);
  a_left_release_keeps_a_held_middle_capture(h);
  CHECK(g_middle_presses == 4);
  BK_capture_pointer(ctx, NULL);

  BK_window_free(win);
  BK_context_free(ctx);
  printf(g_fail ? "FAILED\n" : "ok\n");
  return g_fail;
}

#endif
