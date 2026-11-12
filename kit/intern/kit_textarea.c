/* Multi-line text area (Tk Text, Qt QTextEdit/QPlainTextEdit).
 *
 * UTF-8 buffer with caret/selection, colored spans (tags), vertical
 * scrolling; editing keys follow the single-line field (Blender's word
 * jumps via BLI_str_cursor_step_utf8). Drawn on Blender's text-field
 * widget (wcol_text).
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */

#include <math.h>
#include <stdlib.h>
#include <string.h>

#include "kit_intern.h"

typedef enum { STRCUR_JUMP_NONE, STRCUR_JUMP_DELIM, STRCUR_JUMP_ALL } eStrCursorJumpType;
typedef enum { STRCUR_DIR_PREV, STRCUR_DIR_NEXT } eStrCursorJumpDirection;
void BLI_str_cursor_step_utf8(const char *str, int str_maxlen, int *pos,
                              eStrCursorJumpDirection direction, eStrCursorJumpType jump,
                              bool use_init_step);
void BLI_str_cursor_step_bounds_utf8(const char *str, int str_maxlen, int pos, int *r_start,
                                     int *r_end);

typedef BKTextSpan Span;

typedef struct TAData {
  char *text;
  int len, cap;
  int caret, anchor; /* anchor < 0: no selection */
  float want_x;      /* column memory for Up/Down */
  int scroll;        /* first visible line */
  int *lines;        /* line start offsets */
  int nlines, caplines;
  Span *spans;
  int nspans;
  bool rows_set;  /* "rows" was given (the browser look defaults to 2) */
  bool edge_jump; /* "edge_jump": Up past the first line goes to the start, Down past the last to
                   * the end (as browsers do) */
  BKImeComp ime; /* the IME composition at the caret */
} TAData;

#define TA(n) ((TAData *)(n)->ext)

static float caret_x(const TAData *t, int off);

static int line_h(void)
{
  return bk_iround(0.8f * bk_unit());
}

/* Browser look (Chrome's <textarea>): 1px border + 2px padding, 15px lines of
 * 13.33px text. Blender look: the text-field margins and 0.8 unit lines, taller
 * for larger text ("font_scale"). */
static int ta_line_h(const BKNode *n)
{
  return BK_HTML(n) ? bk_iround(bk_hpx(15.0f)) :
                      bk_iround(0.8f * bk_unit() * BK_MAX(1.0f, n->font_scale));
}
static int ta_pad_x(const BKNode *n)
{
  return BK_HTML(n) ? bk_iround(bk_hpx(3.0f)) : bk_pad();
}
static int ta_pad_y(const BKNode *n)
{
  return BK_HTML(n) ? bk_iround(bk_hpx(3.0f)) : bk_pad() / 2;
}

/* Events measure text at the size it is drawn with. */
static float ta_font_begin(const BKNode *n)
{
  const float saved = bk_text_scale;
  bk_text_scale = n->font_scale > 0.0f ? n->font_scale : 1.0f;
  if (BK_HTML(n)) {
    bk_text_scale *= BK_HTML_FONT;
    bk_html_text_scale = bk_text_scale;
  }
  return saved;
}

static void ta_reindex(TAData *t)
{
  t->nlines = 0;
  for (int i = 0; i <= t->len; i++) {
    if (i == 0 || t->text[i - 1] == '\n') {
      if (t->nlines == t->caplines) {
        t->caplines = t->caplines ? t->caplines * 2 : 64;
        t->lines = (int *)realloc(t->lines, sizeof(int) * (size_t)t->caplines);
      }
      t->lines[t->nlines++] = i;
    }
  }
}

static int line_end(const TAData *t, int line)
{
  return line + 1 < t->nlines ? t->lines[line + 1] - 1 : t->len;
}

static int line_of(const TAData *t, int off)
{
  int lo = 0, hi = t->nlines - 1;
  while (lo < hi) {
    const int mid = (lo + hi + 1) / 2;
    if (t->lines[mid] <= off) {
      lo = mid;
    }
    else {
      hi = mid - 1;
    }
  }
  return lo;
}

static void ta_init(BKNode *n)
{
  TAData *t = (TAData *)calloc(1, sizeof(TAData));
  t->cap = 64;
  t->text = (char *)calloc(1, (size_t)t->cap);
  t->anchor = -1;
  ta_reindex(t);
  n->ext = t;
  n->rows = 8;
}

static void ta_free(BKNode *n)
{
  TAData *t = TA(n);
  bk_ime_comp_free(&t->ime);
  free(t->text);
  free(t->lines);
  free(t->spans);
  free(t);
}

BKImeComp *bk_textarea_ime(BKNode *n)
{
  return &TA(n)->ime;
}

static void ta_changed(BKNode *n)
{
  ta_reindex(TA(n));
  bk_invalidate(n);
  bk_emit(n, BK_EV_CHANGE, NULL, TA(n)->caret, 0);
}

static void ta_insert_raw(TAData *t, int off, const char *s)
{
  const int n = (int)strlen(s);
  if (t->len + n + 1 > t->cap) {
    t->cap = t->len + n + 64;
    t->text = (char *)realloc(t->text, (size_t)t->cap);
  }
  off = BK_CLAMP(off, 0, t->len);
  memmove(t->text + off + n, t->text + off, (size_t)(t->len - off + 1));
  memcpy(t->text + off, s, (size_t)n);
  t->len += n;
  /* keep spans attached to their text */
  for (int i = 0; i < t->nspans; i++) {
    if (t->spans[i].start >= off) {
      t->spans[i].start += n;
    }
    if (t->spans[i].end > off) {
      t->spans[i].end += n;
    }
  }
}

static void ta_delete_raw(TAData *t, int a, int b)
{
  a = BK_CLAMP(a, 0, t->len);
  b = BK_CLAMP(b, a, t->len);
  memmove(t->text + a, t->text + b, (size_t)(t->len - b + 1));
  t->len -= b - a;
  for (int i = 0; i < t->nspans; i++) {
    Span *s = &t->spans[i];
    s->start = s->start >= b ? s->start - (b - a) : BK_MIN(s->start, a);
    s->end = s->end >= b ? s->end - (b - a) : BK_MIN(s->end, a);
  }
}

void BK_textarea_insert(BKNode *n, int off, const char *text)
{
  TAData *t = TA(n);
  ta_insert_raw(t, off, text ? text : "");
  if (t->caret >= off) {
    t->caret += (int)strlen(text ? text : "");
  }
  t->anchor = -1;
  ta_reindex(t);
  bk_invalidate(n);
}

void BK_textarea_delete(BKNode *n, int a, int b)
{
  TAData *t = TA(n);
  ta_delete_raw(t, a, b);
  if (t->caret > a) {
    t->caret = BK_MAX(a, t->caret - (b - a));
  }
  t->anchor = -1;
  ta_reindex(t);
  bk_invalidate(n);
}

int BK_textarea_caret(BKNode *n)
{
  return TA(n)->caret;
}

void BK_textarea_set_caret(BKNode *n, int off)
{
  BK_textarea_set_selection(n, -1, off);
}

int BK_textarea_selection(BKNode *n, int *a, int *b)
{
  TAData *t = TA(n);
  if (t->anchor < 0 || t->anchor == t->caret) {
    return 0;
  }
  *a = BK_MIN(t->anchor, t->caret);
  *b = BK_MAX(t->anchor, t->caret);
  return 1;
}

void BK_textarea_set_selection(BKNode *n, int a, int b)
{
  TAData *t = TA(n);
  t->anchor = a < 0 ? -1 : BK_CLAMP(a, 0, t->len);
  t->caret = BK_CLAMP(b, 0, t->len);
  const float saved = ta_font_begin(n); /* Up/Down go on from the new column */
  t->want_x = caret_x(t, t->caret);
  bk_text_scale = saved;
  bk_invalidate(n);
}

void BK_textarea_set_spans(BKNode *n, const BKTextSpan *spans, int count)
{
  TAData *t = TA(n);
  free(t->spans);
  t->spans = (Span *)calloc((size_t)BK_MAX(1, count), sizeof(Span));
  memcpy(t->spans, spans, sizeof(Span) * (size_t)count);
  t->nspans = count;
  bk_invalidate(n);
}

static int visible_lines(const BKNode *n)
{
  return BK_MAX(1, (n->h - 2 * ta_pad_y(n)) / ta_line_h(n));
}

static void ta_view(BKNode *n, int axis, int *first, int *vis, int *total)
{
  if (axis == 0) { /* lines */
    *first = TA(n)->scroll;
    *vis = visible_lines(n);
    *total = TA(n)->nlines;
  }
}

static void ta_set_view(BKNode *n, int axis, int first)
{
  if (axis == 0) {
    TA(n)->scroll = first;
  }
}

void BK_textarea_see(BKNode *n, int off)
{
  TAData *t = TA(n);
  const int line = line_of(t, BK_CLAMP(off, 0, t->len)), vis = visible_lines(n);
  if (line < t->scroll) {
    t->scroll = line;
  }
  else if (line >= t->scroll + vis) {
    t->scroll = line - vis + 1;
  }
  bk_invalidate(n);
}

/* Set/get the whole text through the generic "value" property. */
int bk_textarea_set_text(BKNode *n, const char *s)
{
  TAData *t = TA(n);
  ta_delete_raw(t, 0, t->len);
  t->nspans = 0;
  ta_insert_raw(t, 0, s ? s : "");
  t->caret = BK_MIN(t->caret, t->len);
  t->anchor = -1;
  ta_reindex(t);
  bk_invalidate(n);
  return 1;
}

const char *bk_textarea_text(BKNode *n)
{
  return TA(n)->text;
}

/* -- input -- */

static int col_at(const TAData *t, int line, float px)
{
  const int a = t->lines[line], b = line_end(t, line);
  float prev = 0.0f;
  int i = a;
  while (i < b) {
    int nx = i;
    BLI_str_cursor_step_utf8(t->text, t->len, &nx, STRCUR_DIR_NEXT, STRCUR_JUMP_NONE, true);
    const float w = bk_text_width(t->text + a, nx - a, false);
    if (px < (prev + w) * 0.5f) {
      return i;
    }
    prev = w;
    i = nx;
  }
  return b;
}

static int offset_at(BKNode *n, int x, int y)
{
  TAData *t = TA(n);
  const int line = BK_CLAMP((y - n->y - ta_pad_y(n)) / ta_line_h(n) + t->scroll, 0,
                            t->nlines - 1);
  return col_at(t, line, (float)(x - n->x - ta_pad_x(n)));
}

int BK_textarea_index_at(BKNode *n, int x, int y)
{
  const float saved = ta_font_begin(n);
  const int off = offset_at(n, x, y);
  bk_text_scale = saved;
  return off;
}

static float caret_x(const TAData *t, int off)
{
  const int line = line_of(t, off);
  return bk_text_width(t->text + t->lines[line], off - t->lines[line], false);
}

static void move(TAData *t, int to, bool extend)
{
  if (extend) {
    if (t->anchor < 0) {
      t->anchor = t->caret;
    }
  }
  else {
    t->anchor = -1;
  }
  t->caret = BK_CLAMP(to, 0, t->len);
}

static bool delete_selection(BKNode *n)
{
  int a, b;
  if (!BK_textarea_selection(n, &a, &b)) {
    return false;
  }
  bk_undo_record(n, 1);
  ta_delete_raw(TA(n), a, b);
  TA(n)->caret = a;
  TA(n)->anchor = -1;
  return true;
}

bool bk_textarea_delete_selection(BKNode *n)
{
  const bool deleted = delete_selection(n);
  if (deleted) {
    ta_reindex(TA(n));
  }
  return deleted;
}

static int ta_press(BKNode *n, BKEvent *e)
{
  BKContext *ctx = bk_ctx(n);
  if (ctx && ctx->focus != n) {
    bk_ime_commit(ctx); /* the composition stays in the field being left */
    if (ctx->focus && ctx->focus->cls->finish_edit) {
      ctx->focus->cls->finish_edit(ctx->focus, true);
    }
    ctx->focus = n;
  }
  move(TA(n), offset_at(n, e->x, e->y), EV_SHIFT(e));
  TA(n)->want_x = caret_x(TA(n), TA(n)->caret);
  bk_invalidate(n);
  return 1;
}

static int ta_dbl(BKNode *n, BKEvent *e)
{
  TAData *t = TA(n);
  int a, b;
  BLI_str_cursor_step_bounds_utf8(t->text, t->len, offset_at(n, e->x, e->y), &a, &b);
  t->anchor = a;
  t->caret = b;
  bk_invalidate(n);
  return 1;
}

static int ta_drag(BKNode *n, BKEvent *e)
{
  TAData *t = TA(n);
  if (t->anchor < 0) {
    t->anchor = t->caret;
  }
  t->caret = offset_at(n, e->x, e->y);
  BK_textarea_see(n, t->caret);
  return 1;
}

static int ta_wheel(BKNode *n, BKEvent *e)
{
  TAData *t = TA(n);
  const int ns = BK_CLAMP(t->scroll - 3 * e->delta, 0, BK_MAX(0, t->nlines - visible_lines(n)));
  if (ns != t->scroll) {
    t->scroll = ns;
    bk_invalidate(n);
    return 1;
  }
  return 0;
}

static int step(TAData *t, int pos, bool next, bool word)
{
  BLI_str_cursor_step_utf8(t->text, t->len, &pos, next ? STRCUR_DIR_NEXT : STRCUR_DIR_PREV,
                           word ? STRCUR_JUMP_DELIM : STRCUR_JUMP_NONE, true);
  return pos;
}

static int ta_key(BKNode *n, BKEvent *e)
{
  BKContext *ctx = bk_ctx(n);
  if (!ctx || ctx->focus != n || !NODE_ENABLED(n)) {
    return 0;
  }
  TAData *t = TA(n);
  const char *ks = e->keysym;
  bool ctrl = EV_CTRL(e);
  const bool shift = EV_SHIFT(e);
  if (BK_MAC_CONTROL(e->mods)) {
    /* macOS: Control alone is never a shortcut; its letters are Cocoa's Emacs keys */
    ctrl = false;
    if (bk_keysym_is(ks, "k")) {
      /* kill to the line's end; at its end, the newline */
      const int line_e = line_end(t, line_of(t, t->caret));
      const int end = line_e > t->caret ? line_e : BK_MIN(t->caret + 1, t->len);
      if (end > t->caret) {
        bk_undo_record(n, 1);
        ta_delete_raw(t, t->caret, end);
      }
      t->anchor = -1;
      ta_reindex(t);
      t->want_x = caret_x(t, t->caret);
      ta_changed(n);
      BK_textarea_see(n, t->caret);
      return 1;
    }
    static const char *const emacs[][2] = {{"a", "Home"},  {"e", "End"},  {"b", "Left"},
                                           {"f", "Right"}, {"p", "Up"},   {"n", "Down"},
                                           {"d", "Delete"}, {"h", "BackSpace"}};
    const char *mapped = NULL;
    for (size_t i = 0; i < sizeof(emacs) / sizeof(emacs[0]); i++) {
      if (bk_keysym_is(ks, emacs[i][0])) {
        mapped = emacs[i][1];
      }
    }
    if (!mapped) {
      return 0;
    }
    ks = mapped;
  }
  int a, b;
  const bool sel = BK_textarea_selection(n, &a, &b);
  bool edited = false, keep_x = false;
  if (bk_keysym_is(ks, "Escape")) {
    ctx->focus = NULL;
    t->anchor = -1;
    bk_invalidate(n);
    return 1;
  }
  if (ctrl && bk_keysym_is(ks, "a")) {
    t->anchor = 0;
    t->caret = t->len;
  }
  else if (ctrl && (bk_keysym_is(ks, "c") || bk_keysym_is(ks, "x"))) {
    if (sel) {
      char *s = (char *)malloc((size_t)(b - a + 1));
      memcpy(s, t->text + a, (size_t)(b - a));
      s[b - a] = 0;
      bk_clipboard_set(ctx, s);
      free(s);
      if (bk_keysym_is(ks, "x")) {
        edited = delete_selection(n);
      }
    }
  }
  else if (ctrl && bk_keysym_is(ks, "v")) {
    char *clip = bk_clipboard_get(ctx);
    if (clip) {
      bk_undo_record(n, 1);
      delete_selection(n);
      ta_insert_raw(t, t->caret, clip);
      t->caret += (int)strlen(clip);
      free(clip);
      edited = true;
    }
  }
  else if (bk_keysym_is(ks, "Left") || bk_keysym_is(ks, "Right")) {
    const bool right = bk_keysym_is(ks, "Right");
    if (sel && !shift) {
      move(t, right ? b : a, false);
    }
    else {
      move(t, step(t, t->caret, right, ctrl), shift);
    }
  }
  else if (bk_keysym_is(ks, "Up") || bk_keysym_is(ks, "Down") || bk_keysym_is(ks, "Prior") ||
           bk_keysym_is(ks, "Next"))
  {
    const int line = line_of(t, t->caret);
    int d = bk_keysym_is(ks, "Up") ? -1 : (bk_keysym_is(ks, "Down") ? 1 : 0);
    if (!d) {
      d = (bk_keysym_is(ks, "Prior") ? -1 : 1) * visible_lines(n);
    }
    if (t->edge_jump && (line + d < 0 || line + d >= t->nlines)) {
      move(t, line + d < 0 ? 0 : t->len, shift);
    }
    else {
      move(t, col_at(t, BK_CLAMP(line + d, 0, t->nlines - 1), t->want_x), shift);
      keep_x = true;
    }
  }
  else if (bk_keysym_is(ks, "Home")) {
    move(t, ctrl ? 0 : t->lines[line_of(t, t->caret)], shift);
  }
  else if (bk_keysym_is(ks, "End")) {
    move(t, ctrl ? t->len : line_end(t, line_of(t, t->caret)), shift);
  }
  else if (bk_keysym_is(ks, "BackSpace")) {
    if (!(edited = delete_selection(n)) && t->caret > 0) {
      bk_undo_record(n, 1);
      const int start = step(t, t->caret, false, ctrl);
      ta_delete_raw(t, start, t->caret);
      t->caret = start;
      edited = true;
    }
  }
  else if (bk_keysym_is(ks, "Delete")) {
    if (!(edited = delete_selection(n)) && t->caret < t->len) {
      bk_undo_record(n, 1);
      ta_delete_raw(t, t->caret, step(t, t->caret, true, ctrl));
      edited = true;
    }
  }
  else if (bk_keysym_is(ks, "Return") || bk_keysym_is(ks, "KP_Enter") ||
           bk_keysym_is(ks, "Tab") || (e->text && (unsigned char)e->text[0] >= 0x20 && !ctrl))
  {
    const char *ins = bk_keysym_is(ks, "Tab") ? "    " :
                      (bk_keysym_is(ks, "Return") || bk_keysym_is(ks, "KP_Enter")) ? "\n" :
                                                                                     e->text;
    bk_undo_record(n, 1);
    delete_selection(n);
    ta_insert_raw(t, t->caret, ins);
    t->caret += (int)strlen(ins);
    t->anchor = -1;
    edited = true;
  }
  else {
    return 0;
  }
  if (!keep_x) {
    ta_reindex(t);
    t->want_x = caret_x(t, t->caret);
  }
  if (edited) {
    ta_changed(n);
  }
  BK_textarea_see(n, t->caret);
  return 1;
}

static int ta_cursor(BKNode *n, int x, int y)
{
  return BK_CURSOR_TEXT;
}

static void ta_finish(BKNode *n, bool confirm)
{
  TA(n)->anchor = -1;
  bk_invalidate(n);
}

static void ta_measure(BKNode *n, int *rw, int *rh)
{
  *rw = 12 * bk_unit();
  *rh = n->rows * line_h() + bk_pad();
}

static void draw_line_spans(TAData *t, int a, int b, float x, const BKRect *line,
                            const uchar def[4]);
static void draw_caret_line(BKNode *n, int a, int b, float x, const BKRect *line,
                            const uchar def[4]);

/* Handlers that measure text, at the size it is drawn with. */
#define TA_SCALED(name, impl) \
  static int name(BKNode *n, BKEvent *e) \
  { \
    const float saved = ta_font_begin(n); \
    const int r = impl(n, e); \
    bk_text_scale = saved; \
    return r; \
  }
TA_SCALED(ta_press_s, ta_press)
TA_SCALED(ta_dbl_s, ta_dbl)
TA_SCALED(ta_drag_s, ta_drag)
TA_SCALED(ta_key_s, ta_key)

static int ta_set_num(BKNode *n, const char *key, double v)
{
  if (strcmp(key, "rows") == 0) {
    n->rows = (int)v;
    TA(n)->rows_set = true;
    bk_relayout(n);
    return 1;
  }
  if (strcmp(key, "edge_jump") == 0) {
    TA(n)->edge_jump = v != 0.0;
    return 1;
  }
  return 0;
}

bool bk_textarea_html_measure(BKNode *n, int *rw, int *rh)
{
  /* Chrome: cols=20 -> 182px; rows (default 2) x 15px + 2px padding + 1px border */
  const int rows = TA(n)->rows_set ? BK_MAX(1, n->rows) : 2;
  *rw = bk_iround(bk_hpx(182.0f));
  *rh = rows * ta_line_h(n) + 2 * ta_pad_y(n);
  return true;
}

void bk_textarea_html_draw(BKNode *n)
{
  TAData *t = TA(n);
  BKContext *ctx = bk_ctx(n);
  const bool focused = ctx && ctx->focus == n;
  const bool enabled = NODE_ENABLED(n), hover = enabled && bk_hovered(n);
  const float rad = bk_hpx(2.0f), bw = BK_MAX(1.0f, floorf(bk_hpx(1.0f)));
  float fill[4], border[4];
  if (!enabled) { /* lighten layer, translucent white, faded border */
    bk_argb(0x33A9A9A9u, fill);
    bk_html_box(n->x, n->y, n->w, n->h, rad, fill, NULL, 0);
  }
  bk_argb(enabled ? 0xFFFFFFFFu : 0x99FFFFFFu, fill);
  bk_argb(!enabled ? 0x4D767676u : (hover ? 0xFF4F4F4Fu : 0xFF767676u), border);
  bk_html_box(n->x, n->y, n->w, n->h, rad, fill, border, bw);
  if (focused) { /* the 2px focus ring, 1px outside */
    const float o = bk_hpx(1.0f);
    float ring[4];
    bk_argb(0xFF101010u, ring);
    bk_html_box(n->x - o, n->y - o, n->w + 2 * o, n->h + 2 * o, rad + o, NULL, ring,
                bk_hpx(2.0f));
  }
  static const uchar black[4] = {0, 0, 0, 255}, gray[4] = {0x54, 0x54, 0x54, 255},
                     white[4] = {255, 255, 255, 255};
  const uchar *tc = enabled ? black : gray;
  const int px = ta_pad_x(n), py = ta_pad_y(n), lh = ta_line_h(n);
  const int b = (int)bw;
  bk_clip_push(n->x + b, n->y + b, n->w - 2 * b, n->h - 2 * b);
  t->scroll = BK_CLAMP(t->scroll, 0, BK_MAX(0, t->nlines - visible_lines(n)));
  int sa = -1, sb = -1;
  const bool sel = BK_textarea_selection(n, &sa, &sb);
  const float x0 = (float)(n->x + px);
  for (int line = t->scroll; line < t->nlines; line++) {
    const int y = n->y + py + (line - t->scroll) * lh;
    if (y > n->y + n->h) {
      break;
    }
    const int la = t->lines[line], lb = line_end(t, line);
    const BKRect lr = bk_rect(n->x, y, n->w, lh);
    if (focused && t->ime.len && t->caret >= la && t->caret <= lb) {
      draw_caret_line(n, la, lb, x0, &lr, tc);
    }
    else {
      draw_line_spans(t, la, lb, x0, &lr, tc);
    }
    if (sel && sa < lb + 1 && sb > la) { /* #3390ff behind white text */
      const int s0 = BK_MAX(sa, la), s1 = BK_MIN(sb, lb);
      const float sx = x0 + bk_text_width(t->text + la, s0 - la, false);
      float sw = bk_text_width(t->text + s0, s1 - s0, false);
      if (sb > lb) {
        sw += bk_font_size() * 0.4f; /* the newline */
      }
      float sc[4];
      bk_argb(0xFF3390FFu, sc);
      bk_fill_f(sx, (float)y, sw, (float)lh, sc);
      bk_clip_push((int)floorf(sx), y, (int)ceilf(sw) + 1, lh);
      bk_text(x0, bk_text_baseline(&lr), t->text + la, lb - la, white, false);
      bk_clip_pop();
    }
    if (focused && !sel && t->caret >= la && t->caret <= lb) {
      const float cx = x0 + bk_text_width(t->text + la, t->caret - la, false) +
                       bk_ime_caret_offset(&t->ime, false);
      bk_fill(floorf(cx), (float)y, bw, (float)lh, tc);
    }
    if (focused && t->caret >= la && t->caret <= lb) {
      bk_ime_set_candidate(&t->ime, n->x, y + lh + 1);
    }
  }
  bk_clip_pop();
}

static void draw_line_spans(TAData *t, int a, int b, float x, const BKRect *line,
                            const uchar def[4])
{
  for (int i = 0; i < t->nspans; i++) { /* backgrounds first */
    const Span *s = &t->spans[i];
    const int s0 = BK_MAX(s->start, a), s1 = BK_MIN(s->end, b);
    if (s->has_bg && s1 > s0) {
      const float sx = x + bk_text_width(t->text + a, s0 - a, false);
      bk_fill_f(sx, (float)line->ymin, bk_text_width(t->text + s0, s1 - s0, false),
                (float)bk_rh(line), s->bg);
    }
  }
  const float base = bk_text_baseline(line);
  /* split [a, b) at span boundaries; later spans win */
  int pos = a;
  while (pos < b) {
    int next = b;
    const float *col = NULL;
    for (int i = 0; i < t->nspans; i++) {
      const Span *s = &t->spans[i];
      if (s->start <= pos && s->end > pos) {
        if (s->has_fg) {
          col = s->fg;
        }
        next = BK_MIN(next, s->end);
      }
      else if (s->start > pos) {
        next = BK_MIN(next, s->start);
      }
    }
    const float sx = x + bk_text_width(t->text + a, pos - a, false);
    if (col) {
      bk_text_f(sx, base, t->text + pos, next - pos, col, false);
    }
    else {
      bk_text(sx, base, t->text + pos, next - pos, def, false);
    }
    pos = next;
  }
}

/* The caret's line with an IME composition: the text before the caret, the composite underlined,
 * then the rest (spans keep colouring the text on either side). */
static void draw_caret_line(BKNode *n, int a, int b, float x, const BKRect *line,
                            const uchar def[4])
{
  TAData *t = TA(n);
  const BKImeComp *c = &t->ime;
  const float cx = x + bk_text_width(t->text + a, t->caret - a, false);
  draw_line_spans(t, a, t->caret, x, line, def);
  bk_text(cx, bk_text_baseline(line), c->text, c->len, def, false);
  draw_line_spans(t, t->caret, b, cx + bk_text_width(c->text, c->len, false), line, def);
  bk_ime_draw_underline(c, cx, (float)line->ymax, (float)bk_rw(line), def, false);
}

static void ta_draw(BKNode *n)
{
  TAData *t = TA(n);
  BKContext *ctx = bk_ctx(n);
  const bool focused = ctx && ctx->focus == n;
  BKRect r = bk_node_rect(n);
  int st = NODE_ENABLED(n) ? (bk_hovered(n) ? ST_HOVER : 0) : ST_DISABLED;
  if (focused) {
    st = ST_SELECT;
  }
  BKWCol w;
  bk_widget_state(&w, &bk_theme.textfield, st);
  bk_widget_textbut(&w, &r, st, n->corners);
  const int p = bk_pad(), lh = ta_line_h(n);
  bk_clip_push(n->x + 2, n->y + 2, n->w - 4, n->h - 4);
  t->scroll = BK_CLAMP(t->scroll, 0, BK_MAX(0, t->nlines - visible_lines(n)));
  int a = -1, b = -1;
  BK_textarea_selection(n, &a, &b);
  const float x0 = (float)(n->x + p);
  for (int line = t->scroll; line < t->nlines; line++) {
    const int y = n->y + p / 2 + (line - t->scroll) * lh;
    if (y > n->y + n->h) {
      break;
    }
    const int la = t->lines[line], lb = line_end(t, line);
    if (a >= 0 && a < lb + 1 && b > la) { /* selection on this line */
      const int s0 = BK_MAX(a, la), s1 = BK_MIN(b, lb);
      const float sx = x0 + bk_text_width(t->text + la, s0 - la, false);
      float sw = bk_text_width(t->text + s0, s1 - s0, false);
      if (b > lb) {
        sw += bk_font_size() * 0.4f; /* the newline */
      }
      bk_fill(sx, (float)y, sw, (float)lh, bk_theme.selection);
    }
    const BKRect lr = bk_rect(n->x, y, n->w, lh);
    if (focused && t->ime.len && t->caret >= la && t->caret <= lb) {
      draw_caret_line(n, la, lb, x0, &lr, w.text);
    }
    else {
      draw_line_spans(t, la, lb, x0, &lr, w.text);
    }
    if (focused && t->caret >= la && t->caret <= lb) {
      const float cx = x0 + bk_text_width(t->text + la, t->caret - la, false) +
                       bk_ime_caret_offset(&t->ime, false);
      bk_fill(floorf(cx), (float)y + 1, BK_MAX(1.0f, floorf(0.1f * bk_unit() + 0.5f)),
              (float)lh - 2, bk_theme.cursor);
      bk_ime_set_candidate(&t->ime, n->x, y + lh + 1); /* the caret line's lower edge */
    }
  }
  bk_clip_pop();
}

const BKClass bk_cls_textarea = {.name = "textarea",
                                 .flags = CLS_INTERACTIVE,
                                 .cursor = BK_CURSOR_TEXT,
                                 .init = ta_init,
                                 .free_ext = ta_free,
                                 .measure = ta_measure,
                                 .draw = ta_draw,
                                 .press = ta_press_s,
                                 .dbl = ta_dbl_s,
                                 .drag = ta_drag_s,
                                 .wheel = ta_wheel,
                                 .key = ta_key_s,
                                 .set_num = ta_set_num,
                                 .cursor_at = ta_cursor,
                                 .finish_edit = ta_finish,
                                 .view = ta_view,
                                 .set_view = ta_set_view};
