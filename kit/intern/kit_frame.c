/* Frames with Tk's geometry managers (pack, grid, place), radio buttons,
 * canvases (Tk Canvas / QPainter display lists) and images.
 *
 * The packer is Tk's ArrangePacking / ComputePackRequest (generic/tkPack.c):
 * each child takes a parcel off one side of the remaining cavity, `expand`
 * shares what is left, fill/anchor place the child inside its parcel. The
 * gridder follows tkGrid.c's model (column/row minimums from the children,
 * extra space by weight, sticky placement). Qt box/grid layouts map onto
 * the same code.
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */

#include <limits.h>
#include <math.h>
#include <stdlib.h>
#include <string.h>

#include "kit_intern.h"

#ifdef _WIN32
#  ifndef WIN32_LEAN_AND_MEAN
#    define WIN32_LEAN_AND_MEAN
#  endif
#  ifndef NOMINMAX
#    define NOMINMAX
#  endif
#  if !defined(_WIN32_WINNT) || _WIN32_WINNT < 0x0601
#    undef _WIN32_WINNT
#    define _WIN32_WINNT 0x0601 /* thread pool work objects */
#  endif
#  include <windows.h>
#endif

static int u(void)
{
  return bk_unit();
}

/* -------------------------------------------------------------------- */
/** \name Geometry info
 * \{ */

static BKGeom *geom_ensure(BKNode *n)
{
  if (!n->geom) {
    n->geom = (BKGeom *)calloc(1, sizeof(BKGeom));
    n->geom->rowspan = n->geom->columnspan = 1;
  }
  return n->geom;
}

void BK_geom_set_manager(BKNode *n, int manager)
{
  geom_ensure(n)->manager = manager;
  bk_relayout(n);
}

int BK_geom_manager(const BKNode *n)
{
  return n->geom ? n->geom->manager : BK_GEOM_NONE;
}

enum { HAS_W = 1, HAS_H = 2, HAS_RW = 4, HAS_RH = 8 };

void BK_geom_set(BKNode *n, const char *key, double v)
{
  BKGeom *g = geom_ensure(n);
  const int iv = (int)floor(v + 0.5);
#define K(name) (strcmp(key, name) == 0)
  if (K("side")) g->side = iv;
  else if (K("fill")) g->fill = iv;
  else if (K("expand")) g->expand = iv;
  else if (K("anchor")) g->anchor = iv;
  else if (K("padx0")) g->padx[0] = iv;
  else if (K("padx1")) g->padx[1] = iv;
  else if (K("pady0")) g->pady[0] = iv;
  else if (K("pady1")) g->pady[1] = iv;
  else if (K("ipadx")) g->ipadx = iv;
  else if (K("ipady")) g->ipady = iv;
  else if (K("row")) g->row = iv;
  else if (K("column")) g->column = iv;
  else if (K("rowspan")) g->rowspan = BK_MAX(1, iv);
  else if (K("columnspan")) g->columnspan = BK_MAX(1, iv);
  else if (K("sticky")) g->sticky = iv;
  else if (K("x")) g->x = (float)v;
  else if (K("y")) g->y = (float)v;
  else if (K("relx")) g->relx = (float)v;
  else if (K("rely")) g->rely = (float)v;
  else if (K("width")) { g->width = (float)v; g->has |= HAS_W; }
  else if (K("height")) { g->height = (float)v; g->has |= HAS_H; }
  else if (K("relwidth")) { g->relwidth = (float)v; g->has |= HAS_RW; }
  else if (K("relheight")) { g->relheight = (float)v; g->has |= HAS_RH; }
  else if (K("reqwidth")) g->reqw = iv;
  else if (K("reqheight")) g->reqh = iv;
  else if (K("reset_place")) { g->x = g->y = g->relx = g->rely = 0; g->has = 0; }
#undef K
  bk_relayout(n);
}

double BK_geom_get(const BKNode *n, const char *key)
{
  const BKGeom *g = n->geom;
  if (!g) {
    return 0.0;
  }
#define K(name) (strcmp(key, name) == 0)
  if (K("manager")) return g->manager;
  if (K("side")) return g->side;
  if (K("fill")) return g->fill;
  if (K("expand")) return g->expand;
  if (K("anchor")) return g->anchor;
  if (K("row")) return g->row;
  if (K("column")) return g->column;
  if (K("rowspan")) return g->rowspan;
  if (K("columnspan")) return g->columnspan;
  if (K("sticky")) return g->sticky;
  if (K("reqwidth")) return g->reqw;
  if (K("reqheight")) return g->reqh;
#undef K
  return 0.0;
}

static void child_req(BKNode *c, int *w, int *h)
{
  bk_natural(c, w, h); /* honors reqwidth/reqheight */
  if (c->geom) {
    *w += 2 * c->geom->ipadx;
    *h += 2 * c->geom->ipady;
  }
}

/* Offset of a child inside spare space for an anchor (0 = center). */
static int anchor_off(int anchor, int extra, int lo_bit, int hi_bit)
{
  const bool lo = anchor & lo_bit, hi = anchor & hi_bit;
  if (lo && !hi) {
    return 0;
  }
  if (hi && !lo) {
    return extra;
  }
  return extra / 2;
}

/** \} */

/* -------------------------------------------------------------------- */
/** \name Frame
 * \{ */

#define GRID_MAX 64

typedef struct FrameData {
  float colweight[GRID_MAX], rowweight[GRID_MAX];
  int colmin[GRID_MAX], rowmin[GRID_MAX];
} FrameData;

static void frame_init(BKNode *n)
{
  n->ext = calloc(1, sizeof(FrameData));
}

static void frame_free(BKNode *n)
{
  free(n->ext);
}

void BK_frame_grid_weight(BKNode *n, int column_axis, int index, double weight, int minsize)
{
  if (n->cls != &bk_cls_frame || index < 0 || index >= GRID_MAX) {
    return;
  }
  FrameData *d = (FrameData *)n->ext;
  if (column_axis) {
    d->colweight[index] = (float)weight;
    d->colmin[index] = minsize;
  }
  else {
    d->rowweight[index] = (float)weight;
    d->rowmin[index] = minsize;
  }
  bk_relayout(n);
}

static int frame_header_h(const BKNode *n)
{
  return (n->text && n->text[0]) ? bk_iround(1.25f * u()) : 0;
}

static void frame_insets(const BKNode *n, int *l, int *t, int *r, int *b)
{
  const int p = n->padding;
  *l = *r = *b = p;
  *t = p + frame_header_h(n);
}

static bool managed(const BKNode *c, int manager)
{
  return NODE_VISIBLE(c) && c->geom && c->geom->manager == manager;
}

static int padsum(const int p[2])
{
  return p[0] + p[1];
}

/* Tk ComputePackRequest. */
static void pack_request(BKNode *n, int *rw, int *rh)
{
  int width = 0, height = 0, maxw = 0, maxh = 0;
  for (int i = 0; i < n->nkids; i++) {
    BKNode *c = n->kids[i];
    if (!managed(c, BK_GEOM_PACK)) {
      continue;
    }
    int cw, ch;
    child_req(c, &cw, &ch);
    const BKGeom *g = c->geom;
    if (g->side <= 1) { /* top / bottom */
      maxw = BK_MAX(maxw, cw + padsum(g->padx) + width);
      height += ch + padsum(g->pady);
    }
    else {
      maxh = BK_MAX(maxh, ch + padsum(g->pady) + height);
      width += cw + padsum(g->padx);
    }
  }
  *rw = BK_MAX(maxw, width);
  *rh = BK_MAX(maxh, height);
}

static int x_expansion(BKNode *n, int from, int cavity_w)
{
  int num = 0, min_exp = cavity_w;
  for (int i = from; i < n->nkids; i++) {
    BKNode *c = n->kids[i];
    if (!managed(c, BK_GEOM_PACK)) {
      continue;
    }
    int cw, ch;
    child_req(c, &cw, &ch);
    const int child_w = cw + padsum(c->geom->padx);
    if (c->geom->side <= 1) {
      if (num) {
        min_exp = BK_MIN(min_exp, (cavity_w - child_w) / num);
      }
    }
    else {
      cavity_w -= child_w;
      if (c->geom->expand) {
        num++;
      }
    }
  }
  if (num) {
    min_exp = BK_MIN(min_exp, cavity_w / num);
  }
  return BK_MAX(0, min_exp);
}

static int y_expansion(BKNode *n, int from, int cavity_h)
{
  int num = 0, min_exp = cavity_h;
  for (int i = from; i < n->nkids; i++) {
    BKNode *c = n->kids[i];
    if (!managed(c, BK_GEOM_PACK)) {
      continue;
    }
    int cw, ch;
    child_req(c, &cw, &ch);
    const int child_h = ch + padsum(c->geom->pady);
    if (c->geom->side >= 2) {
      if (num) {
        min_exp = BK_MIN(min_exp, (cavity_h - child_h) / num);
      }
    }
    else {
      cavity_h -= child_h;
      if (c->geom->expand) {
        num++;
      }
    }
  }
  if (num) {
    min_exp = BK_MIN(min_exp, cavity_h / num);
  }
  return BK_MAX(0, min_exp);
}

/* Tk ArrangePacking. */
static void do_pack(BKNode *n, int x0, int y0, int w, int h)
{
  int cx = x0, cy = y0, cw = w, ch = h;
  for (int i = 0; i < n->nkids; i++) {
    BKNode *c = n->kids[i];
    if (!managed(c, BK_GEOM_PACK)) {
      continue;
    }
    const BKGeom *g = c->geom;
    int rw, rh;
    child_req(c, &rw, &rh);
    int fx, fy, fw, fh;
    if (g->side <= 1) {
      fw = cw;
      fh = rh + padsum(g->pady);
      if (g->expand) {
        fh += y_expansion(n, i, ch);
      }
      fh = BK_CLAMP(fh, 0, ch);
      fx = cx;
      if (g->side == 0) {
        fy = cy;
        cy += fh;
      }
      else {
        fy = cy + ch - fh;
      }
      ch -= fh;
    }
    else {
      fw = rw + padsum(g->padx);
      fh = ch;
      if (g->expand) {
        fw += x_expansion(n, i, cw);
      }
      fw = BK_CLAMP(fw, 0, cw);
      fy = cy;
      if (g->side == 2) {
        fx = cx;
        cx += fw;
      }
      else {
        fx = cx + cw - fw;
      }
      cw -= fw;
    }
    const int aw = BK_MAX(0, fw - padsum(g->padx)), ah = BK_MAX(0, fh - padsum(g->pady));
    const int ww = ((g->fill & 1) || rw > aw) ? aw : rw;
    const int hh = ((g->fill & 2) || rh > ah) ? ah : rh;
    bk_arrange(c, fx + g->padx[0] + anchor_off(g->anchor, aw - ww, BK_ANCHOR_W, BK_ANCHOR_E),
               fy + g->pady[0] + anchor_off(g->anchor, ah - hh, BK_ANCHOR_N, BK_ANCHOR_S), ww,
               hh);
  }
}

/* -- grid -- */

static void grid_sizes(BKNode *n, int *colw, int *rowh, int *r_ncols, int *r_nrows)
{
  FrameData *d = (FrameData *)n->ext;
  int ncols = 0, nrows = 0;
  memset(colw, 0, sizeof(int) * GRID_MAX);
  memset(rowh, 0, sizeof(int) * GRID_MAX);
  /* single-span children first, then spanning ones grow the last cell */
  for (int pass = 0; pass < 2; pass++) {
    for (int i = 0; i < n->nkids; i++) {
      BKNode *c = n->kids[i];
      if (!managed(c, BK_GEOM_GRID)) {
        continue;
      }
      const BKGeom *g = c->geom;
      const int c0 = BK_CLAMP(g->column, 0, GRID_MAX - 1), r0 = BK_CLAMP(g->row, 0, GRID_MAX - 1);
      const int cs = BK_CLAMP(g->columnspan, 1, GRID_MAX - c0),
                rs = BK_CLAMP(g->rowspan, 1, GRID_MAX - r0);
      ncols = BK_MAX(ncols, c0 + cs);
      nrows = BK_MAX(nrows, r0 + rs);
      int cw, ch;
      child_req(c, &cw, &ch);
      cw += padsum(g->padx);
      ch += padsum(g->pady);
      if (pass == 0 && cs == 1) {
        colw[c0] = BK_MAX(colw[c0], cw);
      }
      if (pass == 0 && rs == 1) {
        rowh[r0] = BK_MAX(rowh[r0], ch);
      }
      if (pass == 1 && cs > 1) {
        int sum = 0;
        for (int k = c0; k < c0 + cs; k++) {
          sum += colw[k];
        }
        if (sum < cw) {
          colw[c0 + cs - 1] += cw - sum;
        }
      }
      if (pass == 1 && rs > 1) {
        int sum = 0;
        for (int k = r0; k < r0 + rs; k++) {
          sum += rowh[k];
        }
        if (sum < ch) {
          rowh[r0 + rs - 1] += ch - sum;
        }
      }
    }
  }
  for (int k = 0; k < ncols; k++) {
    colw[k] = BK_MAX(colw[k], d->colmin[k]);
  }
  for (int k = 0; k < nrows; k++) {
    rowh[k] = BK_MAX(rowh[k], d->rowmin[k]);
  }
  *r_ncols = ncols;
  *r_nrows = nrows;
}

static void grid_request(BKNode *n, int *rw, int *rh)
{
  int colw[GRID_MAX], rowh[GRID_MAX], nc, nr;
  grid_sizes(n, colw, rowh, &nc, &nr);
  *rw = *rh = 0;
  for (int k = 0; k < nc; k++) {
    *rw += colw[k];
  }
  for (int k = 0; k < nr; k++) {
    *rh += rowh[k];
  }
}

static void distribute(int *sizes, int count, const float *weights, int extra)
{
  float total = 0.0f;
  for (int k = 0; k < count; k++) {
    total += weights[k];
  }
  if (extra <= 0 || total <= 0.0f) {
    return;
  }
  int given = 0, last = -1;
  for (int k = 0; k < count; k++) {
    if (weights[k] > 0.0f) {
      const int add = (int)(extra * weights[k] / total);
      sizes[k] += add;
      given += add;
      last = k;
    }
  }
  if (last >= 0) {
    sizes[last] += extra - given; /* rounding remainder */
  }
}

static void do_grid(BKNode *n, int x0, int y0, int w, int h)
{
  FrameData *d = (FrameData *)n->ext;
  int colw[GRID_MAX], rowh[GRID_MAX], nc, nr;
  grid_sizes(n, colw, rowh, &nc, &nr);
  if (!nc) {
    return;
  }
  int sw = 0, sh = 0;
  for (int k = 0; k < nc; k++) {
    sw += colw[k];
  }
  for (int k = 0; k < nr; k++) {
    sh += rowh[k];
  }
  distribute(colw, nc, d->colweight, w - sw);
  distribute(rowh, nr, d->rowweight, h - sh);
  int colx[GRID_MAX + 1], rowy[GRID_MAX + 1];
  colx[0] = x0;
  for (int k = 0; k < nc; k++) {
    colx[k + 1] = colx[k] + colw[k];
  }
  rowy[0] = y0;
  for (int k = 0; k < nr; k++) {
    rowy[k + 1] = rowy[k] + rowh[k];
  }
  for (int i = 0; i < n->nkids; i++) {
    BKNode *c = n->kids[i];
    if (!managed(c, BK_GEOM_GRID)) {
      continue;
    }
    const BKGeom *g = c->geom;
    const int c0 = BK_CLAMP(g->column, 0, GRID_MAX - 1), r0 = BK_CLAMP(g->row, 0, GRID_MAX - 1);
    const int c1 = BK_MIN(nc, c0 + g->columnspan), r1 = BK_MIN(nr, r0 + g->rowspan);
    const int cx = colx[c0] + g->padx[0], cy = rowy[r0] + g->pady[0];
    const int cw = BK_MAX(0, colx[c1] - colx[c0] - padsum(g->padx));
    const int ch = BK_MAX(0, rowy[r1] - rowy[r0] - padsum(g->pady));
    int rw, rh;
    child_req(c, &rw, &rh);
    const int st = g->sticky;
    const int ww = ((st & BK_ANCHOR_W) && (st & BK_ANCHOR_E)) ? cw : BK_MIN(rw, cw);
    const int hh = ((st & BK_ANCHOR_N) && (st & BK_ANCHOR_S)) ? ch : BK_MIN(rh, ch);
    bk_arrange(c, cx + anchor_off(st, cw - ww, BK_ANCHOR_W, BK_ANCHOR_E),
               cy + anchor_off(st, ch - hh, BK_ANCHOR_N, BK_ANCHOR_S), ww, hh);
  }
}

static void do_place(BKNode *c, int x0, int y0, int pw, int ph)
{
  const BKGeom *g = c->geom;
  int rw, rh;
  child_req(c, &rw, &rh);
  float x = g->relx * pw + g->x, y = g->rely * ph + g->y;
  float ww = g->relwidth * pw + g->width, hh = g->relheight * ph + g->height;
  if (!(g->has & (HAS_W | HAS_RW))) {
    ww = (float)rw;
  }
  if (!(g->has & (HAS_H | HAS_RH))) {
    hh = (float)rh;
  }
  /* anchor: which point of the child sits at (x, y); place defaults to nw */
  const int a = g->anchor ? g->anchor : (BK_ANCHOR_N | BK_ANCHOR_W);
  x -= anchor_off(a, (int)ww, BK_ANCHOR_W, BK_ANCHOR_E);
  y -= anchor_off(a, (int)hh, BK_ANCHOR_N, BK_ANCHOR_S);
  bk_arrange(c, x0 + (int)x, y0 + (int)y, (int)ww, (int)hh);
}

static void frame_measure(BKNode *n, int *rw, int *rh)
{
  int l, t, r, b;
  frame_insets(n, &l, &t, &r, &b);
  int pw = 0, ph = 0, gw = 0, gh = 0;
  if (!(n->flags & NF_NOPROPAGATE)) {
    pack_request(n, &pw, &ph);
    grid_request(n, &gw, &gh);
  }
  int w = BK_MAX(pw, gw), h = BK_MAX(ph, gh);
  if (frame_header_h(n)) {
    w = BK_MAX(w, (int)bk_text_width(n->text, -1, false) + 2 * u());
  }
  *rw = w + l + r;
  *rh = h + t + b;
}

static void frame_arrange(BKNode *n)
{
  int l, t, r, b;
  frame_insets(n, &l, &t, &r, &b);
  const int x0 = n->x + l, y0 = n->y + t;
  const int w = BK_MAX(0, n->w - l - r), h = BK_MAX(0, n->h - t - b);
  do_pack(n, x0, y0, w, h);
  do_grid(n, x0, y0, w, h);
  for (int i = 0; i < n->nkids; i++) {
    BKNode *c = n->kids[i];
    if (managed(c, BK_GEOM_PLACE)) {
      do_place(c, x0, y0, w, h);
    }
    else if (!c->geom || c->geom->manager == BK_GEOM_NONE) {
      c->x = c->y = c->w = c->h = 0; /* unmanaged: not shown (Tk) */
    }
  }
}

static bool shown(const BKNode *c)
{
  return NODE_VISIBLE(c) && c->geom && c->geom->manager != BK_GEOM_NONE;
}

static void frame_draw(BKNode *n)
{
  float c[4];
  const int hh = frame_header_h(n);
  if (hh) { /* LabelFrame / QGroupBox: a Blender panel (header + body) */
    const float rad = bk_theme.panel_roundness * 0.5f * u();
    bk_rgba(bk_theme.panel_back, c);
    bk_roundbox(n->x, n->y, n->w, n->h, rad, CNR_ALL, c);
    bk_rgba(bk_theme.panel_header, c);
    bk_roundbox(n->x, n->y, n->w, hh, rad, CNR_TL | CNR_TR, c);
    BKRect tr = bk_rect(n->x + (int)(0.4f * u()), n->y, n->w, hh);
    bk_text_in(&tr, n->text, bk_theme.text, BK_ALIGN_LEFT, false);
  }
  else if (n->flags & NF_HAS_BG) {
    bk_fill_f(n->x, n->y, n->w, n->h, n->color);
  }
}

/* Shared by frames and canvases: own drawing, then managed children clipped
 * to the widget (Tk children never spill out). */
static void frame_draw_tree(BKNode *n)
{
  if (!NODE_VISIBLE(n)) {
    return;
  }
  BKRect r = bk_rect(n->x - 3, n->y - 3, n->w + 6, n->h + 6);
  if (!bk_clip_visible(&r)) {
    return;
  }
  n->cls->draw(n);
  bk_clip_push(n->x, n->y, n->w, n->h + 2); /* children never spill out (Tk) */
  for (int i = 0; i < n->nkids; i++) {
    if (shown(n->kids[i])) {
      bk_draw_tree(n->kids[i]);
    }
  }
  bk_clip_pop();
}

static BKNode *frame_hit(BKNode *n, int x, int y)
{
  for (int i = n->nkids - 1; i >= 0; i--) {
    BKNode *c = n->kids[i];
    if (shown(c)) {
      BKNode *f = bk_hit(c, x, y);
      if (f) {
        return f;
      }
    }
  }
  return (n->flags & NF_EVENTS) ? n : NULL;
}

const BKClass bk_cls_frame = {.name = "frame",
                              .init = frame_init,
                              .free_ext = frame_free,
                              .measure = frame_measure,
                              .arrange = frame_arrange,
                              .draw = frame_draw,
                              .draw_tree = frame_draw_tree,
                              .hit = frame_hit};

/** \} */

/* -------------------------------------------------------------------- */
/** \name Radio button
 * \{ */

static int radio_press(BKNode *n, BKEvent *e)
{
  if (!n->value) {
    bk_set_value(n, 1); /* a radio only ever turns itself on */
  }
  return 1;
}

static void radio_measure(BKNode *n, int *rw, int *rh)
{
  *rw = u() + (int)bk_text_width(n->label, -1, false) + bk_pad();
  *rh = u();
}

static void radio_draw(BKNode *n)
{
  BKRect r = bk_node_rect(n);
  int st = NODE_ENABLED(n) ? (bk_hovered(n) ? ST_HOVER : 0) : ST_DISABLED;
  if (n->value) {
    st |= ST_SELECT;
  }
  BKWCol w;
  bk_widget_state(&w, &bk_theme.option, st);
  w.roundness = 0.5f; /* round option box */
  BKRect box = r;
  bk_widget_optionbut(&w, &box, st & ~ST_SELECT); /* no checkmark: a dot instead */
  if (n->value) {
    const int s = bk_rh(&r);
    const float cx = r.xmin + s * 0.5f - (s - 2 * bk_px()) / 6.0f, cy = r.ymin + s * 0.5f;
    float dot[4];
    bk_rgba(w.item, dot);
    bk_circle(cx, cy, s * 0.14f, dot);
  }
  uchar tc[4];
  memcpy(tc, bk_theme.option.text, 4);
  if (!NODE_ENABLED(n)) {
    tc[3] /= 2;
  }
  box.xmin -= bk_pad() / 2;
  bk_text_in(&box, n->label, tc, BK_ALIGN_LEFT, false);
}

static int radio_to_text(BKNode *n, char *buf, int size)
{
  buf[0] = n->value ? '1' : '0';
  buf[1] = 0;
  return 1;
}

const BKClass bk_cls_radio = {.name = "radio",
                              .flags = CLS_INTERACTIVE,
                              .measure = radio_measure,
                              .draw = radio_draw,
                              .press = radio_press,
                              .to_text = radio_to_text};

/** \} */

/* -------------------------------------------------------------------- */
/** \name Canvas (display list)
 * \{ */

typedef struct CItem {
  BKCanvasItem it;
  float *coords;
  char *text;
  BKImageRef img; /* BK_ITEM_IMAGE: rgba (copied), imbuf (borrowed) or src (cached) */
  int coords_cap; /* floats allocated in coords */
  int text_cap;   /* bytes allocated in text */
} CItem;

typedef struct CanvasData {
  CItem *items;
  int n, cap;
  /* id -> position, open addressing; rebuilt lazily after reorders/deletes */
  int *idx_key, *idx_pos;
  int idx_cap, idx_count;
  bool idx_valid;
} CanvasData;

#define IDX_EMPTY INT_MIN

static unsigned idx_hash(int id, int cap)
{
  return ((unsigned)id * 2654435761u) & (unsigned)(cap - 1);
}

static void idx_put(CanvasData *d, int id, int pos)
{
  unsigned h = idx_hash(id, d->idx_cap);
  while (d->idx_key[h] != IDX_EMPTY) {
    if (d->idx_key[h] == id) {
      return; /* duplicate id: the first one wins, like the linear search */
    }
    h = (h + 1) & (unsigned)(d->idx_cap - 1);
  }
  d->idx_key[h] = id;
  d->idx_pos[h] = pos;
  d->idx_count++;
}

static void idx_rebuild(CanvasData *d)
{
  int cap = 64;
  while (cap < d->n * 2 + 2) {
    cap *= 2;
  }
  if (cap != d->idx_cap) {
    free(d->idx_key);
    free(d->idx_pos);
    d->idx_key = (int *)malloc(sizeof(int) * (size_t)cap);
    d->idx_pos = (int *)malloc(sizeof(int) * (size_t)cap);
    d->idx_cap = cap;
  }
  for (int k = 0; k < cap; k++) {
    d->idx_key[k] = IDX_EMPTY;
  }
  d->idx_count = 0;
  for (int i = 0; i < d->n; i++) {
    idx_put(d, d->items[i].it.id, i);
  }
  d->idx_valid = true;
}

/* A new item at `pos` (keeps the index valid when there's room). */
static void idx_added(CanvasData *d, int id, int pos)
{
  if (d->idx_valid && (d->idx_count + 1) * 2 < d->idx_cap) {
    idx_put(d, id, pos);
  }
  else {
    d->idx_valid = false;
  }
}

#define CANVAS(n) ((CanvasData *)(n)->ext)

static void canvas_init(BKNode *n)
{
  n->ext = calloc(1, sizeof(CanvasData));
}

static void citem_free(CItem *c)
{
  free(c->coords);
  free(c->text);
  bk_image_ref_clear(&c->img);
}

static void canvas_free(BKNode *n)
{
  CanvasData *d = CANVAS(n);
  for (int i = 0; i < d->n; i++) {
    citem_free(&d->items[i]);
  }
  free(d->items);
  free(d->idx_key);
  free(d->idx_pos);
  free(d);
}

/* Position of item `id` (-1 if none): O(1) through the index. */
static int canvas_find(CanvasData *d, int id)
{
  if (!d->idx_valid) {
    idx_rebuild(d);
  }
  unsigned h = idx_hash(id, d->idx_cap);
  while (d->idx_key[h] != IDX_EMPTY) {
    if (d->idx_key[h] == id) {
      const int pos = d->idx_pos[h];
      if (pos < d->n && d->items[pos].it.id == id) {
        return pos;
      }
      break; /* stale: rebuild below */
    }
    h = (h + 1) & (unsigned)(d->idx_cap - 1);
  }
  if (d->idx_valid && d->idx_key[h] == IDX_EMPTY) {
    return -1;
  }
  idx_rebuild(d);
  for (int i = 0; i < d->n; i++) {
    if (d->items[i].it.id == id) {
      return i;
    }
  }
  return -1;
}

/* Copy `item` into slot `c`, reusing its coordinate and text buffers. */
static void citem_assign(CItem *c, const BKCanvasItem *item)
{
  const int nc = BK_MAX(1, item->ncoords);
  if (nc > c->coords_cap) {
    c->coords = (float *)realloc(c->coords, sizeof(float) * (size_t)nc);
    c->coords_cap = nc;
  }
  memcpy(c->coords, item->coords, sizeof(float) * (size_t)item->ncoords);
  const int tl = item->text ? (int)strlen(item->text) + 1 : 0;
  if (tl > c->text_cap) {
    c->text = (char *)realloc(c->text, (size_t)tl);
    c->text_cap = tl;
  }
  if (tl) {
    memcpy(c->text, item->text, (size_t)tl);
  }
  /* one pixel source per image item (validated here, at the boundary) */
  const int sources = (item->rgba != NULL) + (item->imbuf != NULL) + (item->src != NULL) +
                      (item->video != NULL);
  if (item->type != BK_ITEM_IMAGE || sources != 1) {
    bk_image_ref_clear(&c->img);
  }
  else if (item->video) {
    bk_image_ref_set_video(&c->img, item->video);
  }
  else if (item->imbuf) {
    bk_image_ref_set_imbuf(&c->img, item->imbuf);
  }
  else if (item->src) {
    bk_image_ref_set_source(&c->img, item->src);
  }
  else {
    bk_image_ref_set_rgba(&c->img, item->rgba, item->iw, item->ih);
  }
  c->it = *item;
  c->it.coords = c->coords;
  c->it.text = tl ? c->text : NULL;
  c->it.rgba = NULL;
  c->it.src = NULL;
  c->it.imbuf = NULL;
  c->it.video = NULL;
}

/* An image item's box in canvas space: x0 y0 x1 y1 with 4 coordinates,
 * else the intrinsic size placed at the point by the anchor. */
static bool citem_image_box(const CItem *ci, float b[4])
{
  const BKCanvasItem *it = &ci->it;
  const float *p = ci->coords;
  if (it->ncoords >= 4) {
    b[0] = BK_MIN(p[0], p[2]);
    b[1] = BK_MIN(p[1], p[3]);
    b[2] = fabsf(p[2] - p[0]);
    b[3] = fabsf(p[3] - p[1]);
    return b[2] > 0.0f && b[3] > 0.0f;
  }
  int w, h;
  if (it->ncoords < 2 || !bk_image_ref_size(&ci->img, &w, &h)) {
    return false;
  }
  b[0] = p[0] - anchor_off(it->anchor, w, BK_ANCHOR_W, BK_ANCHOR_E);
  b[1] = p[1] - anchor_off(it->anchor, h, BK_ANCHOR_N, BK_ANCHOR_S);
  b[2] = (float)w;
  b[3] = (float)h;
  return true;
}

/* The canvas-space box an item can paint into, with margins for outline
 * width, anti-aliasing and glyph overhang. False when unknown. */
static bool citem_bbox(const CItem *ci, float b[4])
{
  const BKCanvasItem *it = &ci->it;
  const float *p = ci->coords;
  if (it->ncoords < 2 || !p) {
    return false;
  }
  float x0 = p[0], x1 = p[0], y0 = p[1], y1 = p[1];
  for (int k = 2; k + 1 < it->ncoords; k += 2) {
    x0 = BK_MIN(x0, p[k]);
    x1 = BK_MAX(x1, p[k]);
    y0 = BK_MIN(y0, p[k + 1]);
    y1 = BK_MAX(y1, p[k + 1]);
  }
  float m = (it->width > 0 ? it->width : 1.0f) + 3.0f;
  if (it->type == BK_ITEM_TEXT) {
    if (!ci->text) {
      return false;
    }
    const float size = it->size > 0 ? it->size : bk_font_size();
    float tw = 0;
    BUI_text_measure(it->family ? it->family : "", it->text_flags, size, ci->text, (int)strlen(ci->text), &tw);
    const float x = p[0] - anchor_off(it->anchor, (int)tw, BK_ANCHOR_W, BK_ANCHOR_E);
    const float asc = size * 0.72f;
    const float base = p[1] + asc - anchor_off(it->anchor, (int)asc, BK_ANCHOR_N, BK_ANCHOR_S);
    x0 = x - size;
    x1 = x + tw + size;
    y0 = base - size * 1.5f;
    y1 = base + size * 0.75f;
    m = 2.0f;
  }
  else if (it->type == BK_ITEM_IMAGE) {
    float ib[4];
    if (!citem_image_box(ci, ib)) {
      return false;
    }
    x0 = ib[0];
    y0 = ib[1];
    x1 = ib[0] + ib[2];
    y1 = ib[1] + ib[3];
    m = 2.0f;
  }
  b[0] = x0 - m;
  b[1] = y0 - m;
  b[2] = x1 + m;
  b[3] = y1 + m;
  return true;
}

/* Redraw only where item `ci` paints (the whole canvas when that's unknown). */
static void canvas_damage(BKNode *n, const CItem *ci)
{
  BKContext *ctx = bk_ctx(n);
  float b[4];
  if (!ctx || n->w <= 0 || n->h <= 0 || !citem_bbox(ci, b)) {
    bk_invalidate(n);
    return;
  }
  const float ox = (float)(n->x - n->scroll_x), oy = (float)(n->y - n->scroll);
  const int x0 = BK_MAX(n->x, (int)floorf(ox + b[0])), y0 = BK_MAX(n->y, (int)floorf(oy + b[1]));
  const int x1 = BK_MIN(n->x + n->w, (int)ceilf(ox + b[2]));
  const int y1 = BK_MIN(n->y + n->h, (int)ceilf(oy + b[3]));
  if (x1 > x0 && y1 > y0) {
    bk_ctx_dirty_rect(ctx, bk_rect(x0, y0, x1 - x0, y1 - y0));
  }
}

static void canvas_store(BKNode *n, const BKCanvasItem *item, bool replace)
{
  if (n->cls != &bk_cls_canvas) {
    return;
  }
  CanvasData *d = CANVAS(n);
  int i = replace ? canvas_find(d, item->id) : -1;
  if (i < 0) {
    if (d->n == d->cap) {
      d->cap = d->cap ? d->cap * 2 : 16;
      d->items = (CItem *)realloc(d->items, sizeof(CItem) * (size_t)d->cap);
    }
    i = d->n++;
    memset(&d->items[i], 0, sizeof(CItem));
    idx_added(d, item->id, i);
  }
  else {
    canvas_damage(n, &d->items[i]); /* where it was */
  }
  citem_assign(&d->items[i], item);
  canvas_damage(n, &d->items[i]); /* where it is */
}

void BK_canvas_set_item(BKNode *n, const BKCanvasItem *item)
{
  canvas_store(n, item, true);
}

/* Rebuilding a whole display list (immediate-mode hosts): no id lookup. */
void BK_canvas_add_item(BKNode *n, const BKCanvasItem *item)
{
  canvas_store(n, item, false);
}

void BK_canvas_put_item(BKNode *n, int index, const BKCanvasItem *item)
{
  if (n->cls != &bk_cls_canvas) {
    return;
  }
  CanvasData *d = CANVAS(n);
  index = BK_CLAMP(index, 0, d->n);
  if (index == d->n) {
    if (d->n == d->cap) {
      d->cap = d->cap ? d->cap * 2 : 16;
      d->items = (CItem *)realloc(d->items, sizeof(CItem) * (size_t)d->cap);
    }
    memset(&d->items[d->n++], 0, sizeof(CItem));
  }
  citem_assign(&d->items[index], item);
  d->idx_valid = false; /* ids may have changed */
}

void BK_canvas_truncate(BKNode *n, int count)
{
  if (n->cls != &bk_cls_canvas) {
    return;
  }
  CanvasData *d = CANVAS(n);
  for (int i = BK_MAX(0, count); i < d->n; i++) {
    citem_free(&d->items[i]);
  }
  d->n = BK_CLAMP(count, 0, d->n);
  d->idx_valid = false;
  bk_invalidate(n);
}

void BK_canvas_delete(BKNode *n, int id)
{
  CanvasData *d = CANVAS(n);
  if (id < 0) {
    for (int i = 0; i < d->n; i++) {
      citem_free(&d->items[i]);
    }
    d->n = 0;
    d->idx_valid = false;
    bk_invalidate(n);
    return;
  }
  int i;
  while ((i = canvas_find(d, id)) >= 0) { /* through the index: no list scan */
    canvas_damage(n, &d->items[i]);
    citem_free(&d->items[i]);
    memmove(d->items + i, d->items + i + 1, sizeof(CItem) * (size_t)(d->n - i - 1));
    d->n--;
    d->idx_valid = false;
  }
}

void BK_canvas_raise(BKNode *n, int id, int to_top)
{
  CanvasData *d = CANVAS(n);
  const int i = canvas_find(d, id);
  if (i < 0) {
    return;
  }
  CItem tmp = d->items[i];
  memmove(d->items + i, d->items + i + 1, sizeof(CItem) * (size_t)(d->n - i - 1));
  if (to_top) {
    d->items[d->n - 1] = tmp;
  }
  else {
    memmove(d->items + 1, d->items, sizeof(CItem) * (size_t)(d->n - 1));
    d->items[0] = tmp;
  }
  d->idx_valid = false;
  canvas_damage(n, to_top ? &d->items[d->n - 1] : &d->items[0]);
}

int BK_canvas_count(BKNode *n)
{
  return CANVAS(n)->n;
}

static void canvas_measure(BKNode *n, int *rw, int *rh)
{
  *rw = 10 * u();
  *rh = 5 * u();
}

static void draw_polyline(BuiRaster *r, const float *pts, int npts, float ox, float oy,
                          const float c[4], float width, bool closed)
{
  for (int k = 0; k + 1 < npts; k++) {
    BUI_raster_line(r, ox + pts[2 * k], oy + pts[2 * k + 1], ox + pts[2 * k + 2],
                    oy + pts[2 * k + 3], width, c);
  }
  if (closed && npts > 2) {
    BUI_raster_line(r, ox + pts[2 * npts - 2], oy + pts[2 * npts - 1], ox + pts[0], oy + pts[1],
                    width, c);
  }
}

/* Per-item data computed before drawing (one thread): the rows an item can
 * touch, and its prepared text. */
typedef struct ItemPrep {
  float y0, y1;
  const BuiTextRun *run;
  float tx, ty;
  BKImageDraw image; /* image items: resolved (ib NULL: nothing to draw yet) */
} ItemPrep;

/* An image item placed in window space (ox, oy: the canvas origin). */
static bool citem_image_resolve(const CItem *ci, float ox, float oy, BKImageDraw *r)
{
  float b[4];
  if (!citem_image_box(ci, b)) {
    return false;
  }
  b[0] += ox;
  b[1] += oy;
  const int *c = bk_draw.clip;
  if (ci->img.kind == BK_IMREF_SOURCE &&
      (b[0] + b[2] <= c[0] || b[0] >= c[2] || b[1] + b[3] <= c[1] || b[1] >= c[3]))
  {
    return false; /* off screen: a file outside the clip is neither decoded nor played */
  }
  return bk_image_ref_resolve(&ci->img, ci->it.ncoords >= 4 ? ci->it.fit : BK_FIT_FILL,
                              ci->it.frame, b, r);
}

/* Draw one item into `r`. With `prep`, text uses the prepared run (safe on
 * any thread); without it, text is prepared here. */
static void canvas_item_draw(BuiRaster *r, const CItem *ci, float ox, float oy, const ItemPrep *prep)
{
  const BKCanvasItem *it = &ci->it;
  if (it->hidden) {
    return;
  }
  const float *p = ci->coords;
  const float lw = it->width > 0 ? it->width : 1.0f;
  const bool fill = it->fill[3] > 0, outline = it->outline[3] > 0;
  switch (it->type) {
    case BK_ITEM_RECT:
    case BK_ITEM_ROUNDRECT: {
      if (it->ncoords < 4) {
        return;
      }
      const float x0 = BK_MIN(p[0], p[2]), y0 = BK_MIN(p[1], p[3]);
      const float w = fabsf(p[2] - p[0]), h = fabsf(p[3] - p[1]);
      if ((it->type == BK_ITEM_ROUNDRECT || it->radius > 0) && fill && !outline) {
        /* fill only: the span-filled rounded rect (AA work on the rim only) */
        BUI_raster_round_rect(r, ox + x0, oy + y0, w, h, it->radius, it->fill);
      }
      else if (it->type == BK_ITEM_ROUNDRECT || it->radius > 0) {
        BuiWidgetBase wb = {{ox + x0, oy + y0, w, h}, it->radius, BK_MAX(0, it->radius - lw), CNR_ALL};
        if (fill) {
          memcpy(wb.inner1, it->fill, sizeof(float[4]));
          memcpy(wb.inner2, it->fill, sizeof(float[4]));
        }
        if (outline) {
          memcpy(wb.outline, it->outline, sizeof(float[4]));
        }
        wb.pixelsize = lw;
        BUI_raster_widget_base(r, &wb);
      }
      else {
        if (fill) {
          BUI_raster_fill_rect(r, ox + x0, oy + y0, w, h, it->fill);
        }
        if (outline) {
          BUI_raster_frame_rect(r, ox + x0, oy + y0, w, h, lw, it->outline);
        }
      }
      break;
    }
    case BK_ITEM_OVAL: {
      if (it->ncoords < 4) {
        return;
      }
      const float cx = ox + (p[0] + p[2]) * 0.5f, cy = oy + (p[1] + p[3]) * 0.5f;
      const float rx = fabsf(p[2] - p[0]) * 0.5f, ry = fabsf(p[3] - p[1]) * 0.5f;
      if (fill) {
        BUI_raster_ellipse(r, cx, cy, rx, ry, 0.0f, it->fill);
      }
      if (outline) {
        BUI_raster_ellipse(r, cx, cy, rx, ry, lw, it->outline);
      }
      break;
    }
    case BK_ITEM_LINE: {
      const float *c = it->fill[3] > 0 ? it->fill : it->outline;
      draw_polyline(r, p, it->ncoords / 2, ox, oy, c, lw, false);
      break;
    }
    case BK_ITEM_POLYGON: {
      const int np = it->ncoords / 2;
      if (np >= 3 && fill) {
        float(*pts)[2] = (float(*)[2])malloc(sizeof(float[2]) * (size_t)np);
        for (int k = 0; k < np; k++) {
          pts[k][0] = ox + p[2 * k];
          pts[k][1] = oy + p[2 * k + 1];
        }
        BUI_raster_polygon(r, (const float(*)[2])pts, np, it->fill);
        free(pts);
      }
      if (outline) {
        draw_polyline(r, p, np, ox, oy, it->outline, lw, true);
      }
      break;
    }
    case BK_ITEM_ARC: {
      if (it->ncoords < 4) {
        return;
      }
      /* Tk pieslice: center + points along the arc (degrees ccw from 3 o'clock) */
      const float cx = ox + (p[0] + p[2]) * 0.5f, cy = oy + (p[1] + p[3]) * 0.5f;
      const float rx = fabsf(p[2] - p[0]) * 0.5f, ry = fabsf(p[3] - p[1]) * 0.5f;
      const int seg = 48;
      float pts[50][2];
      pts[0][0] = cx;
      pts[0][1] = cy;
      for (int k = 0; k <= seg; k++) {
        const float a = (it->start + it->extent * k / seg) * 3.14159265f / 180.0f;
        pts[k + 1][0] = cx + rx * cosf(a);
        pts[k + 1][1] = cy - ry * sinf(a);
      }
      if (fill) {
        BUI_raster_polygon(r, (const float(*)[2])pts, seg + 2, it->fill);
      }
      if (outline) {
        for (int k = 1; k <= seg; k++) {
          BUI_raster_line(r, pts[k][0], pts[k][1], pts[k + 1][0], pts[k + 1][1], lw, it->outline);
        }
      }
      break;
    }
    case BK_ITEM_TEXT: {
      if (!ci->text || it->ncoords < 2) {
        return;
      }
      const float size = it->size > 0 ? it->size : bk_font_size();
      const int len = (int)strlen(ci->text);
      float tw = 0;
      if (!(it->anchor & BK_ANCHOR_W)) { /* the width only moves centred / right text */
        BUI_text_measure(it->family ? it->family : "", it->text_flags, size, ci->text, len, &tw);
      }
      float x = ox + p[0], y = oy + p[1];
      x -= anchor_off(it->anchor, (int)tw, BK_ANCHOR_W, BK_ANCHOR_E);
      /* y: baseline from the anchor (cap height ~0.72 em) */
      const float asc = size * 0.72f;
      y += asc - anchor_off(it->anchor, (int)asc, BK_ANCHOR_N, BK_ANCHOR_S);
      const float *c = it->fill[3] > 0 ? it->fill : it->outline;
      if (prep) {
        BUI_raster_text_run(r, prep->tx, prep->ty, prep->run, c);
      }
      else {
        BUI_raster_set_font(r, it->family ? it->family : "", it->text_flags);
        BUI_raster_text(r, floorf(x + 0.5f), floorf(y + 0.5f), ci->text, len, size, c);
      }
      break;
    }
    case BK_ITEM_IMAGE: {
      BKImageDraw d;
      if (prep ? prep->image.ib != NULL : citem_image_resolve(ci, ox, oy, &d)) {
        bk_image_draw_resolved(r, prep ? &prep->image : &d, 1.0f, it->radius);
      }
      break;
    }
  }
}

/* -- parallel canvas drawing ------------------------------------------------
 * Large display lists are drawn in horizontal bands on the thread pool. Each
 * band draws every item that touches it, in list order, into a view clipped
 * to the band, so the pixels equal serial drawing. Text is prepared first on
 * this thread (the glyph cache is not thread-safe); lines, arcs and outlined
 * polygons use the kit's global drawing state and keep a canvas serial. */

#define PAR_MIN_ITEMS 64
/* Every extra thread costs CPU (wake-ups, bands overlapping items), so an
 * animation uses the fewest threads that still meet the frame deadline, and
 * all of them only when it can't be met. */
#define PAR_SLACK 0.7 /* aim for 70 % of the refresh interval */

double bk_frame_ms = 0.0, bk_frame_budget_ms = 0.0;
static double g_raster_ms = 0.0; /* the canvas's last draw time */
static int g_raster_threads = 1; /* ... and how many threads drew it */

/* Measured wall-time speedup of n threads (8 threads ~3.5x). */
static double par_speedup(int n)
{
  return n / (1.0 + 0.18 * (n - 1));
}

static double now_ms(void)
{
#ifdef _WIN32
  static LARGE_INTEGER freq;
  LARGE_INTEGER t;
  if (!freq.QuadPart) {
    QueryPerformanceFrequency(&freq);
  }
  QueryPerformanceCounter(&t);
  return (double)t.QuadPart * 1000.0 / (double)freq.QuadPart;
#else
  return 0.0;
#endif
}

/* Threads for this draw: during a vsync'd animation, the fewest that keep the
 * frame within budget; otherwise (first frames, input, uncapped) all of them. */
static int threads_wanted(int max_threads)
{
  if (bk_frame_ms <= 0.0 || bk_frame_budget_ms <= 0.0 || g_raster_ms <= 0.0) {
    return max_threads;
  }
  const double serial = g_raster_ms * par_speedup(g_raster_threads);
  const double room = bk_frame_budget_ms * PAR_SLACK - (bk_frame_ms - g_raster_ms);
  if (room <= 0.0) {
    return max_threads; /* the frame is late anyway: draw as fast as possible */
  }
  for (int n = 1; n < max_threads; n++) {
    if (serial / par_speedup(n) <= room) {
      return n;
    }
  }
  return max_threads;
}

static bool item_parallel_safe(const BKCanvasItem *it)
{
  (void)it; /* every item type draws into the band's own raster view */
  return true;
}

/* Rows an item can touch, and its prepared text; false if text was not ready. */
static bool item_prepare(const CItem *ci, float ox, float oy, ItemPrep *pp)
{
  const BKCanvasItem *it = &ci->it;
  const float *p = ci->coords;
  const float pad = (it->width > 0 ? it->width : 1.0f) + 2.0f;
  pp->run = NULL;
  pp->y0 = 1e30f;
  pp->y1 = -1e30f;
  for (int k = 1; k < it->ncoords; k += 2) {
    pp->y0 = BK_MIN(pp->y0, oy + p[k] - pad);
    pp->y1 = BK_MAX(pp->y1, oy + p[k] + pad);
  }
  if (it->type == BK_ITEM_TEXT && ci->text && it->ncoords >= 2) {
    const float size = it->size > 0 ? it->size : bk_font_size();
    float tw = 0;
    pp->run = BUI_text_prepare(it->family ? it->family : "", it->text_flags, size, ci->text, (int)strlen(ci->text), &tw);
    float x = ox + p[0], y = oy + p[1];
    x -= anchor_off(it->anchor, (int)tw, BK_ANCHOR_W, BK_ANCHOR_E);
    const float asc = size * 0.72f;
    y += asc - anchor_off(it->anchor, (int)asc, BK_ANCHOR_N, BK_ANCHOR_S);
    pp->tx = floorf(x + 0.5f);
    pp->ty = floorf(y + 0.5f);
    pp->y0 = y - size * 1.5f - 2.0f;
    pp->y1 = y + size + 2.0f;
  }
  else if (it->type == BK_ITEM_IMAGE) {
    pp->image.ib = NULL;
    if (citem_image_resolve(ci, ox, oy, &pp->image)) {
      pp->y0 = pp->image.dst[1] - 1.0f;
      pp->y1 = pp->image.dst[1] + pp->image.dst[3] + 1.0f;
    }
    else {
      pp->image.ib = NULL;
      pp->y1 = pp->y0 - 1.0f; /* nothing to draw */
    }
  }
  if (it->clip_on) {
    pp->y0 = BK_MAX(pp->y0, oy + it->clip[1] - 1.0f);
    pp->y1 = BK_MIN(pp->y1, oy + it->clip[1] + it->clip[3] + 1.0f);
  }
  return true;
}

typedef struct BandJob {
  const CanvasData *d;
  const ItemPrep *prep;
  const BuiRaster *base;
  int clip[4]; /* the canvas clip, x0 y0 x1 y1 */
  float ox, oy;
  int nbands, band_h;
  const int *start; /* band b draws items idx[start[b] .. start[b + 1]) */
  const int *idx;
  volatile long next;
} BandJob;

static void band_draw(const BandJob *j, int b)
{
  const int y0 = j->clip[1] + b * j->band_h, y1 = BK_MIN(j->clip[3], y0 + j->band_h);
  if (y1 <= y0) {
    return;
  }
  BuiRaster *view = (BuiRaster *)malloc(BUI_raster_view_size());
  if (!view) {
    return;
  }
  for (int s = j->start[b]; s < j->start[b + 1]; s++) {
    const int i = j->idx[s];
    const CItem *ci = &j->d->items[i];
    const BKCanvasItem *it = &ci->it;
    int cx0 = j->clip[0], cy0 = y0, cx1 = j->clip[2], cy1 = y1;
    if (it->clip_on) {
      cx0 = BK_MAX(cx0, (int)floorf(j->ox + it->clip[0]));
      cy0 = BK_MAX(cy0, (int)floorf(j->oy + it->clip[1]));
      cx1 = BK_MIN(cx1, (int)floorf(j->ox + it->clip[0]) + (int)ceilf(it->clip[2]));
      cy1 = BK_MIN(cy1, (int)floorf(j->oy + it->clip[1]) + (int)ceilf(it->clip[3]));
      if (cx1 <= cx0 || cy1 <= cy0) {
        continue;
      }
    }
    BUI_raster_view(j->base, view, cx0, cy0, cx1, cy1);
    canvas_item_draw(view, ci, j->ox, j->oy,
                     it->type == BK_ITEM_TEXT || it->type == BK_ITEM_IMAGE ? &j->prep[i] : NULL);
  }
  free(view);
}

static int par_threads(void)
{
  static int n = -1;
  if (n < 0) {
    const char *env = getenv("BUI_RASTER_THREADS");
    n = env ? atoi(env) : 0;
#ifdef _WIN32
    if (n <= 0) {
      SYSTEM_INFO si;
      GetSystemInfo(&si);
      n = (int)si.dwNumberOfProcessors;
    }
#endif
    n = BK_CLAMP(n, 1, 8);
  }
  return n;
}

#ifdef _WIN32
static BandJob g_job;
static VOID CALLBACK band_worker(PTP_CALLBACK_INSTANCE inst, PVOID ctx, PTP_WORK work)
{
  (void)inst;
  (void)work;
  BandJob *j = (BandJob *)ctx;
  long b;
  while ((b = InterlockedIncrement(&j->next) - 1) < j->nbands) {
    band_draw(j, (int)b);
  }
}
#endif

/* Draw `d` in bands on several threads; false when it has to be drawn serially. */
static bool canvas_draw_parallel(const CanvasData *d, float ox, float oy, int threads)
{
  if (threads < 2 || d->n < PAR_MIN_ITEMS) {
    return false;
  }
  for (int i = 0; i < d->n; i++) {
    if (!item_parallel_safe(&d->items[i].it)) {
      return false;
    }
  }
#ifdef _WIN32
  ItemPrep *prep = (ItemPrep *)malloc(sizeof(ItemPrep) * (size_t)d->n);
  if (!prep) {
    return false;
  }
  /* phase 1: rows and text, on this thread; redo if the glyph cache was flushed */
  bool ready = false;
  for (int attempt = 0; attempt < 3 && !ready; attempt++) {
    const unsigned gen = BUI_text_generation();
    for (int i = 0; i < d->n; i++) {
      item_prepare(&d->items[i], ox, oy, &prep[i]);
    }
    ready = BUI_text_generation() == gen;
  }
  if (!ready) {
    free(prep);
    return false;
  }
  /* phase 2: bands on the pool, this thread included */
  static PTP_WORK work = NULL;
  if (!work) {
    work = CreateThreadpoolWork(band_worker, &g_job, NULL);
    if (!work) {
      free(prep);
      return false;
    }
  }
  BandJob *j = &g_job;
  j->d = d;
  j->prep = prep;
  j->base = bk_draw.r;
  memcpy(j->clip, bk_draw.clip, sizeof(j->clip));
  j->ox = ox;
  j->oy = oy;
  j->nbands = threads * 3; /* more bands than threads: balances uneven rows */
  j->band_h = BK_MAX(1, (j->clip[3] - j->clip[1] + j->nbands - 1) / j->nbands);
  /* bucket items by band once (in list order), so each band reads only its
   * own items instead of scanning the whole list */
  const int nb = j->nbands;
  int *start = (int *)calloc((size_t)nb + 1, sizeof(int));
  int *range = (int *)malloc(sizeof(int) * 2 * (size_t)d->n);
  if (!start || !range) {
    free(start);
    free(range);
    free(prep);
    return false;
  }
  long total = 0;
  for (int i = 0; i < d->n; i++) {
    const ItemPrep *pp = &prep[i];
    int b0 = (int)floorf((pp->y0 - (float)j->clip[1]) / (float)j->band_h);
    int b1 = (int)floorf((pp->y1 - (float)j->clip[1]) / (float)j->band_h);
    b0 = BK_MAX(b0, 0);
    b1 = BK_MIN(b1, nb - 1);
    if (d->items[i].it.hidden || b1 < b0) {
      b0 = 1;
      b1 = 0; /* nothing to draw */
    }
    range[2 * i] = b0;
    range[2 * i + 1] = b1;
    for (int b = b0; b <= b1; b++) {
      start[b + 1]++;
    }
    total += b1 >= b0 ? b1 - b0 + 1 : 0;
  }
  for (int b = 0; b < nb; b++) {
    start[b + 1] += start[b];
  }
  int *idx = (int *)malloc(sizeof(int) * (size_t)BK_MAX(1, total));
  int *fill = (int *)malloc(sizeof(int) * (size_t)nb);
  if (!idx || !fill) {
    free(start);
    free(range);
    free(idx);
    free(fill);
    free(prep);
    return false;
  }
  memcpy(fill, start, sizeof(int) * (size_t)nb);
  for (int i = 0; i < d->n; i++) {
    for (int b = range[2 * i]; b <= range[2 * i + 1]; b++) {
      idx[fill[b]++] = i;
    }
  }
  free(fill);
  free(range);
  j->start = start;
  j->idx = idx;
  j->next = 0;
  for (int t = 1; t < threads; t++) {
    SubmitThreadpoolWork(work);
  }
  band_worker(NULL, j, NULL);
  WaitForThreadpoolWorkCallbacks(work, FALSE);
  free(start);
  free(idx);
  free(prep);
  return true;
#else
  (void)ox;
  (void)oy;
  return false;
#endif
}

static void canvas_draw(BKNode *n)
{
  if (n->flags & NF_HAS_BG) {
    bk_fill_f(n->x, n->y, n->w, n->h, n->color);
  }
  CanvasData *d = CANVAS(n);
  bk_clip_push(n->x, n->y, n->w, n->h);
  const float ox = (float)(n->x - n->scroll_x), oy = (float)(n->y - n->scroll);
  const double t0 = now_ms();
  const int *clip = bk_draw.clip;
  const long region = (long)BK_MAX(0, clip[2] - clip[0]) * BK_MAX(0, clip[3] - clip[1]);
  const bool small = region * 4 < (long)n->w * n->h; /* a damage rectangle, not the canvas */
  /* on the GPU, drawing only records instances: one thread is fastest */
  const int threads = (d->n >= PAR_MIN_ITEMS && !small && !BUI_raster_is_gpu(bk_draw.r)) ?
                          threads_wanted(par_threads()) :
                          1;
  if (bk_draw.clip[2] > bk_draw.clip[0] && bk_draw.clip[3] > bk_draw.clip[1] &&
      canvas_draw_parallel(d, ox, oy, threads))
  {
    g_raster_ms = now_ms() - t0;
    g_raster_threads = threads;
    bk_clip_pop();
    return;
  }
  for (int i = 0; i < d->n; i++) {
    const BKCanvasItem *it = &d->items[i].it;
    float b[4];
    if (small && citem_bbox(&d->items[i], b) &&
        (ox + b[2] < clip[0] || ox + b[0] >= clip[2] || oy + b[3] < clip[1] || oy + b[1] >= clip[3]))
    {
      continue; /* entirely outside the region being redrawn */
    }
    if (it->clip_on) { /* overflow: hidden / scroll containers */
      bk_clip_push((int)floorf(ox + it->clip[0]), (int)floorf(oy + it->clip[1]),
                   (int)ceilf(it->clip[2]), (int)ceilf(it->clip[3]));
      canvas_item_draw(bk_draw.r, &d->items[i], ox, oy, NULL);
      bk_clip_pop();
    }
    else {
      canvas_item_draw(bk_draw.r, &d->items[i], ox, oy, NULL);
    }
  }
  if (d->n >= PAR_MIN_ITEMS) {
    g_raster_ms = now_ms() - t0;
    g_raster_threads = 1;
  }
  bk_clip_pop();
}

/* Offscreen rasterization for CSS effects (filters, blend modes, masks,
 * clip-path, rotate/skew): draws `items` (device pixels, as BK_canvas_put_item
 * takes them) with the canvas' own item code into a w x h buffer whose top
 * left is at (x0, y0), and writes straight-alpha RGBA8 to `rgba`. The CPU
 * raster has no alpha channel, so the items are drawn on black and on white:
 * alpha = 1 - (white - black), colour = black / alpha (exact for coverage
 * antialiasing). Declared in the Rust sys crate, not in BUI_kit.h.
 * Returns 1 on success. */
int BK_canvas_rasterize(const BKCanvasItem *items, int n, float x0, float y0, int w, int h,
                        unsigned char *rgba)
{
  if (w <= 0 || h <= 0 || w > 8192 || h > 8192 || !rgba) {
    return 0;
  }
  CItem *ci = (CItem *)calloc((size_t)BK_MAX(n, 1), sizeof(CItem));
  if (!ci) {
    return 0;
  }
  for (int i = 0; i < n; i++) {
    citem_assign(&ci[i], &items[i]);
  }
  BuiRaster *r = BUI_raster_create(w, h);
  uint32_t *black = (uint32_t *)malloc(sizeof(uint32_t) * (size_t)w * (size_t)h);
  int ok = r && black;
  for (int pass = 0; ok && pass < 2; pass++) {
    const float bg[4] = {(float)pass, (float)pass, (float)pass, 1.0f};
    BUI_raster_begin_frame(r, w, h);
    BUI_raster_set_clip(r, 0, 0, w, h);
    BUI_raster_fill_rect(r, 0, 0, (float)w, (float)h, bg);
    for (int i = 0; i < n; i++) {
      const BKCanvasItem *it = &ci[i].it;
      if (it->clip_on) {
        BUI_raster_set_clip(r, (int)floorf(it->clip[0] - x0), (int)floorf(it->clip[1] - y0),
                            (int)ceilf(it->clip[2]), (int)ceilf(it->clip[3]));
      }
      canvas_item_draw(r, &ci[i], -x0, -y0, NULL);
      if (it->clip_on) {
        BUI_raster_set_clip(r, 0, 0, w, h);
      }
    }
    BUI_raster_end_frame(r);
    const uint32_t *px = BUI_raster_pixels(r);
    if (pass == 0) {
      memcpy(black, px, sizeof(uint32_t) * (size_t)w * (size_t)h);
      continue;
    }
    for (size_t k = 0; k < (size_t)w * (size_t)h; k++) {
      const uint32_t b = black[k], wt = px[k];
      unsigned char *o = rgba + 4 * k;
      /* per channel: white - black = 255 * (1 - alpha); use the mean */
      const int dr = (int)((wt >> 16) & 255) - (int)((b >> 16) & 255);
      const int dg = (int)((wt >> 8) & 255) - (int)((b >> 8) & 255);
      const int db = (int)(wt & 255) - (int)(b & 255);
      const int a = BK_CLAMP(255 - (dr + dg + db) / 3, 0, 255);
      o[3] = (unsigned char)a;
      if (a == 0) {
        o[0] = o[1] = o[2] = 0;
        continue;
      }
      o[0] = (unsigned char)BK_MIN(255, (int)((b >> 16) & 255) * 255 / a);
      o[1] = (unsigned char)BK_MIN(255, (int)((b >> 8) & 255) * 255 / a);
      o[2] = (unsigned char)BK_MIN(255, (int)(b & 255) * 255 / a);
    }
  }
  free(black);
  if (r) {
    BUI_raster_free(r);
  }
  for (int i = 0; i < n; i++) {
    citem_free(&ci[i]);
  }
  free(ci);
  return ok;
}

/* create_window: place-managed children sit at canvas coordinates, scrolled
 * with the items. */
static void canvas_arrange(BKNode *n)
{
  for (int i = 0; i < n->nkids; i++) {
    BKNode *c = n->kids[i];
    if (managed(c, BK_GEOM_PLACE)) {
      do_place(c, n->x - n->scroll_x, n->y - n->scroll, n->w, n->h);
    }
    else {
      c->x = c->y = c->w = c->h = 0;
    }
  }
}

const BKClass bk_cls_canvas = {.name = "canvas",
                               .init = canvas_init,
                               .free_ext = canvas_free,
                               .measure = canvas_measure,
                               .arrange = canvas_arrange,
                               .draw = canvas_draw,
                               .draw_tree = frame_draw_tree,
                               .hit = frame_hit};

/** \} */
