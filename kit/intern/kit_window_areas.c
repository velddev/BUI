/* A title bar drawn by the host (BK_window_wm "titlebar"): the areas it marks (drag,
 * minimise, maximise, close) and what a point is to the system. The lookups are pure, so
 * every platform builds and tests them; the Win32 and Cocoa hosts keep the areas, the
 * others (the compositor or window manager draws the title bar) take none.
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */

#include <stdlib.h>
#include <string.h>

#include "kit_intern.h"

int BK_areas_at(const BKWindowArea *areas, int count, float x, float y)
{
  for (int i = count - 1; i >= 0; i--) {
    const BKWindowArea *a = &areas[i];
    if (x >= a->x && y >= a->y && x < a->x + a->w && y < a->y + a->h) {
      return a->area;
    }
  }
  return BK_AREA_CLIENT;
}

int BK_frame_hit(int area, int y, int border, int maximized)
{
  if (!maximized && y >= 0 && y < border) {
    return BK_HIT_TOP;
  }
  switch (area) {
    case BK_AREA_DRAG:
      return BK_HIT_CAPTION;
    case BK_AREA_MAX:
      return BK_HIT_MAXBUTTON;
    default:
      return BK_HIT_CLIENT;
  }
}

void bk_areas_set(BKAreas *a, const BKWindowArea *areas, int count)
{
  if (count <= 0 || !areas) {
    bk_areas_free(a);
    return;
  }
  BKWindowArea *v = (BKWindowArea *)realloc(a->v, sizeof(BKWindowArea) * (size_t)count);
  if (!v) {
    return; /* keeps the last areas */
  }
  memcpy(v, areas, sizeof(BKWindowArea) * (size_t)count);
  a->v = v;
  a->n = count;
}

void bk_areas_free(BKAreas *a)
{
  free(a->v);
  a->v = NULL;
  a->n = 0;
}

#if !defined(_WIN32) && !defined(BUI_WINDOW_COCOA)
void BK_window_set_areas(BKWindow *win, const BKWindowArea *areas, int count)
{
  (void)win;
  (void)areas;
  (void)count;
}
#endif
