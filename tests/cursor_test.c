/* The text cursor steps over a character as it is drawn: an emoji sequence
 * (zero width joiners, skin tones, keycaps, flags) is one step, as its glyph
 * is one (gpu_raster.c's glyph walk).
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */

#include <stdbool.h>
#include <stdio.h>
#include <string.h>

/* core/intern/string_cursor_utf8.c (as kit_textarea.c declares them) */
bool BLI_str_cursor_step_next_utf8(const char *str, int str_maxlen, int *pos);
bool BLI_str_cursor_step_prev_utf8(const char *str, int str_maxlen, int *pos);

static int g_fail = 0;
#define CHECK(c) \
  do { \
    if (!(c)) { \
      printf("FAIL %s:%d: %s\n", __FILE__, __LINE__, #c); \
      g_fail = 1; \
    } \
  } while (0)

/* The byte offsets the cursor stops at stepping forward through `s`, then
 * back, as one list each; whether both are `stops` (`n` of them, 0 first). */
static int steps_are(const char *s, const int *stops, int n)
{
  const int len = (int)strlen(s);
  int pos = 0, i = 1, ok = 1;
  while (BLI_str_cursor_step_next_utf8(s, len, &pos)) {
    ok &= i < n && pos == stops[i];
    i++;
  }
  ok &= i == n;
  pos = len;
  i = n - 2;
  while (BLI_str_cursor_step_prev_utf8(s, len, &pos)) {
    ok &= i >= 0 && pos == stops[i];
    i--;
  }
  ok &= i == -1;
  if (!ok) {
    printf("stops in %s differ\n", s);
  }
  return ok;
}

int main(void)
{
  /* a, bald man (U+1F468 U+200D U+1F9B2), b */
  CHECK(steps_are("a\xF0\x9F\x91\xA8\xE2\x80\x8D\xF0\x9F\xA6\xB2" "b", (int[]){0, 1, 12, 13}, 4));
  /* thumbs up with a skin tone (U+1F44D U+1F3FB) */
  CHECK(steps_are("\xF0\x9F\x91\x8D\xF0\x9F\x8F\xBB", (int[]){0, 8}, 2));
  /* keycap one (1 U+FE0F U+20E3), then x */
  CHECK(steps_are("1\xEF\xB8\x8F\xE2\x83\xA3x", (int[]){0, 7, 8}, 3));
  /* two flags (US, FR): a pair of regional indicators each */
  CHECK(steps_are("\xF0\x9F\x87\xBA\xF0\x9F\x87\xB8\xF0\x9F\x87\xAB\xF0\x9F\x87\xB7",
                  (int[]){0, 8, 16}, 3));
  /* family: man ZWJ woman ZWJ girl */
  CHECK(steps_are("\xF0\x9F\x91\xA8\xE2\x80\x8D\xF0\x9F\x91\xA9\xE2\x80\x8D\xF0\x9F\x91\xA7",
                  (int[]){0, 18}, 2));
  /* plain text and plain emoji step one at a time */
  CHECK(steps_are("ab\xF0\x9F\xA4\x94" "c", (int[]){0, 1, 2, 6, 7}, 5));
  if (!g_fail) {
    printf("cursor_test: ok\n");
  }
  return g_fail;
}
