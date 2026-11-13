/* A title bar drawn by the host: which area a point is in (the last rectangle holding it, in
 * paint order) and what that point is to Windows (BK_frame_hit), on every platform.
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */

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

int main(void)
{
  /* a 1000 x 48 bar that drags, a tab on it, its buttons, and a menu's backdrop over all */
  const BKWindowArea bar[] = {
      {0, 0, 1000, 48, BK_AREA_DRAG},
      {100, 10, 80, 28, BK_AREA_CLIENT}, /* a tab */
      {862, 0, 46, 48, BK_AREA_MIN},
      {908, 0, 46, 48, BK_AREA_MAX},
      {954, 0, 46, 48, BK_AREA_CLOSE},
  };
  const int n = (int)(sizeof(bar) / sizeof(bar[0]));
  CHECK(BK_areas_at(bar, n, 50, 20) == BK_AREA_DRAG);
  CHECK(BK_areas_at(bar, n, 120, 20) == BK_AREA_CLIENT);
  CHECK(BK_areas_at(bar, n, 120, 5) == BK_AREA_DRAG); /* above the tab */
  CHECK(BK_areas_at(bar, n, 870, 20) == BK_AREA_MIN);
  CHECK(BK_areas_at(bar, n, 908, 0) == BK_AREA_MAX);   /* edges: the left and top are in */
  CHECK(BK_areas_at(bar, n, 954, 47) == BK_AREA_CLOSE); /* ... the right and bottom out */
  CHECK(BK_areas_at(bar, n, 999, 47) == BK_AREA_CLOSE);
  CHECK(BK_areas_at(bar, n, 500, 48) == BK_AREA_CLIENT); /* below the bar */
  CHECK(BK_areas_at(bar, n, -1, 10) == BK_AREA_CLIENT);
  CHECK(BK_areas_at(NULL, 0, 10, 10) == BK_AREA_CLIENT);
  const BKWindowArea covered[] = {
      {0, 0, 1000, 48, BK_AREA_DRAG}, {908, 0, 46, 48, BK_AREA_MAX}, {0, 0, 1000, 48, 0}};
  CHECK(BK_areas_at(covered, 3, 50, 20) == BK_AREA_CLIENT);
  CHECK(BK_areas_at(covered, 3, 920, 20) == BK_AREA_CLIENT);

  /* the top border resizes unless maximised, over buttons too */
  CHECK(BK_frame_hit(BK_AREA_DRAG, 0, 8, 0) == BK_HIT_TOP);
  CHECK(BK_frame_hit(BK_AREA_MAX, 7, 8, 0) == BK_HIT_TOP);
  CHECK(BK_frame_hit(BK_AREA_CLIENT, 3, 8, 0) == BK_HIT_TOP);
  CHECK(BK_frame_hit(BK_AREA_DRAG, 8, 8, 0) == BK_HIT_CAPTION);
  CHECK(BK_frame_hit(BK_AREA_DRAG, 0, 8, 1) == BK_HIT_CAPTION);
  CHECK(BK_frame_hit(BK_AREA_MAX, 20, 8, 0) == BK_HIT_MAXBUTTON);
  CHECK(BK_frame_hit(BK_AREA_MAX, 0, 8, 1) == BK_HIT_MAXBUTTON);
  /* minimise and close stay the host's own buttons, as does the rest */
  CHECK(BK_frame_hit(BK_AREA_MIN, 20, 8, 0) == BK_HIT_CLIENT);
  CHECK(BK_frame_hit(BK_AREA_CLOSE, 20, 8, 0) == BK_HIT_CLIENT);
  CHECK(BK_frame_hit(BK_AREA_CLIENT, 300, 8, 0) == BK_HIT_CLIENT);
  if (!g_fail) {
    printf("window areas: ok\n");
  }
  return g_fail;
}
