/* Images on nodes and canvas items: what a BKImageRef shows, fitted into a
 * box (CSS object-fit, the same bk_image_fit the decoder used), drawn with
 * BUI_raster_image. Sources go through the cache: drawn when decoded,
 * requested at the box size otherwise (the box redraws on arrival). Videos
 * (video.c) show their current frame the same way.
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */

#include <math.h>
#include <string.h>

#include "MEM_guardedalloc.h"

#include "image.h"
#include "kit_intern.h"

void bk_image_ref_clear(BKImageRef *ref)
{
  switch (ref->kind) {
    case BK_IMREF_OWNED:
      BK_imbuf_free(ref->u.owned);
      break;
    case BK_IMREF_SOURCE:
      bk_image_source_release(ref->u.source);
      break;
    default:
      break;
  }
  memset(ref, 0, sizeof(*ref));
}

void bk_image_ref_set_rgba(BKImageRef *ref, const unsigned char *rgba, int w, int h)
{
  if (rgba && ref->kind == BK_IMREF_OWNED && ref->u.owned->x == w && ref->u.owned->y == h) {
    /* same size: convert in place; a new key only when the pixels changed */
    BKImBuf *ib = ref->u.owned;
    bool changed = false;
    for (int y = 0; y < h; y++) {
      uint32_t row[256];
      for (int x = 0; x < w; x += 256) {
        const int n = BK_MIN(256, w - x);
        uint32_t *dst = ib->rect + (size_t)y * w + x;
        imb_premultiply_rgba(row, rgba + ((size_t)y * w + x) * 4, n);
        if (memcmp(dst, row, sizeof(uint32_t) * (size_t)n) != 0) {
          memcpy(dst, row, sizeof(uint32_t) * (size_t)n);
          changed = true;
        }
      }
    }
    if (changed) {
      imb_rekey(ib);
    }
    return;
  }
  bk_image_ref_clear(ref);
  BKImBuf *ib = BK_imbuf_from_rgba(rgba, w, h, 0);
  if (ib) {
    ref->kind = BK_IMREF_OWNED;
    ref->u.owned = ib;
  }
}

void bk_image_ref_set_imbuf(BKImageRef *ref, const BKImBuf *ib)
{
  if (ref->kind == BK_IMREF_BORROWED && ref->u.borrowed == ib) {
    return;
  }
  bk_image_ref_clear(ref);
  if (ib) {
    ref->kind = BK_IMREF_BORROWED;
    ref->u.borrowed = ib;
  }
}

void bk_image_ref_set_video(BKImageRef *ref, BKVideo *v)
{
  if (ref->kind == BK_IMREF_VIDEO && ref->u.video == v) {
    return;
  }
  bk_image_ref_clear(ref);
  if (v) {
    ref->kind = BK_IMREF_VIDEO;
    ref->u.video = v;
  }
}

void bk_image_ref_set_source(BKImageRef *ref, const char *src)
{
  if (ref->kind == BK_IMREF_SOURCE && src &&
      strcmp(bk_image_source_path(ref->u.source), src) == 0)
  {
    return; /* immediate-mode hosts set the same source every frame */
  }
  BKImageSource *s = bk_image_source_get(src);
  bk_image_ref_clear(ref);
  if (s) {
    ref->kind = BK_IMREF_SOURCE;
    ref->u.source = s;
  }
}

static const BKImBuf *ref_imbuf(const BKImageRef *ref)
{
  return ref->kind == BK_IMREF_OWNED ? ref->u.owned :
         ref->kind == BK_IMREF_BORROWED ? ref->u.borrowed :
                                          NULL;
}

bool bk_image_ref_size(const BKImageRef *ref, int *r_w, int *r_h)
{
  const BKImBuf *ib = ref_imbuf(ref);
  BKImageInfo info;
  if (ib) {
    *r_w = ib->x;
    *r_h = ib->y;
    return true;
  }
  if (ref->kind == BK_IMREF_VIDEO) {
    return bk_video_size(ref->u.video, r_w, r_h);
  }
  if (ref->kind == BK_IMREF_SOURCE && bk_image_source_info(ref->u.source, &info)) {
    *r_w = info.width;
    *r_h = info.height;
    return true;
  }
  *r_w = *r_h = 0;
  return false;
}

bool bk_image_ref_resolve(const BKImageRef *ref, int fit, int frame, const float box[4],
                          BKImageDraw *r)
{
  const BKImageFit target = {fit, BK_MAX(1, (int)lroundf(box[2])), BK_MAX(1, (int)lroundf(box[3])),
                             0.5f, 0.5f, BK_MAX(0, frame)};
  if (ref->kind == BK_IMREF_SOURCE || ref->kind == BK_IMREF_VIDEO) {
    const int rect[4] = {(int)floorf(box[0]), (int)floorf(box[1]), (int)ceilf(box[0] + box[2]),
                         (int)ceilf(box[1] + box[3])};
    r->ib = ref->kind == BK_IMREF_VIDEO ? bk_video_frame(ref->u.video, &target, rect) :
                                          bk_image_cache_get(ref->u.source, &target, rect);
    if (!r->ib) {
      return false;
    }
    /* the decoded image holds the visible part (ib->crop inside it) */
    float src[4];
    bk_image_fit(&target, r->ib->src_x, r->ib->src_y, r->dst, src);
    memcpy(r->src, r->ib->crop, sizeof(r->src));
  }
  else {
    r->ib = ref_imbuf(ref);
    if (!r->ib) {
      return false;
    }
    bk_image_fit(&target, r->ib->x, r->ib->y, r->dst, r->src);
  }
  r->dst[0] += box[0];
  r->dst[1] += box[1];
  return r->dst[2] > 0.0f && r->dst[3] > 0.0f;
}

void bk_image_draw_resolved(BuiRaster *r, const BKImageDraw *d, float alpha, float radius)
{
  const BuiPixmap pm = imb_pixmap(d->ib);
  BUI_raster_image_round(r, d->dst[0], d->dst[1], d->dst[2], d->dst[3], &pm, d->src, alpha, 0, radius);
}

void bk_image_ref_draw(const BKImageRef *ref, int fit, int frame, const float box[4], float alpha)
{
  BKImageDraw d;
  if (bk_image_ref_resolve(ref, fit, frame, box, &d)) {
    bk_image_draw_resolved(bk_draw.r, &d, alpha, 0.0f);
  }
}

void bk_imbuf_draw(const BKImBuf *ib, float x, float y, float w, float h, float alpha)
{
  const BuiPixmap pm = imb_pixmap(ib);
  BUI_raster_image(bk_draw.r, x, y, w, h, &pm, NULL, alpha, 0);
}

/* -------------------------------------------------------------------- */
/** \name Nodes (BUI_kit.h)
 * \{ */

void BK_node_set_image(BKNode *n, const unsigned char *rgba, int w, int h)
{
  if (rgba && w > 0 && h > 0) {
    const bool resized = n->image.kind != BK_IMREF_OWNED || n->image.u.owned->x != w ||
                         n->image.u.owned->y != h;
    bk_image_ref_set_rgba(&n->image, rgba, w, h);
    if (resized) {
      bk_relayout(n);
    }
  }
  else {
    bk_image_ref_clear(&n->image);
    bk_relayout(n);
  }
  bk_invalidate(n);
}

void BK_node_set_image_source(BKNode *n, const char *src, int fit)
{
  bk_image_ref_set_source(&n->image, src);
  n->image_fit = fit;
  bk_relayout(n);
  bk_invalidate(n);
}

void BK_node_set_image_frame(BKNode *n, int frame)
{
  if (n->image_frame != BK_MAX(0, frame)) {
    n->image_frame = BK_MAX(0, frame);
    bk_invalidate(n);
  }
}

void BK_node_set_imbuf(BKNode *n, const BKImBuf *ib)
{
  bk_image_ref_set_imbuf(&n->image, ib);
  bk_relayout(n);
  bk_invalidate(n);
}

bool bk_node_image_size(const BKNode *n, int *r_w, int *r_h)
{
  return n->image.kind != BK_IMREF_NONE && bk_image_ref_size(&n->image, r_w, r_h);
}

void bk_node_image_draw(const BKNode *n, const BKRect *r, float alpha)
{
  /* labels and buttons (Tk image=, QLabel pixmap): the intrinsic size, centered */
  int w, h;
  if (bk_node_image_size(n, &w, &h)) {
    const float box[4] = {(float)(r->xmin + (bk_rw(r) - w) / 2), (float)(r->ymin + (bk_rh(r) - h) / 2),
                          (float)w, (float)h};
    bk_image_ref_draw(&n->image, BK_FIT_FILL, n->image_frame, box, alpha);
  }
}

static void image_measure(BKNode *n, int *rw, int *rh)
{
  if (!bk_node_image_size(n, rw, rh)) {
    *rw = *rh = bk_unit();
  }
}

static void image_draw(BKNode *n)
{
  /* the "image" class (<img>, Image components): fitted into the node */
  const float box[4] = {(float)n->x, (float)n->y, (float)n->w, (float)n->h};
  bk_image_ref_draw(&n->image, n->image_fit, n->image_frame, box, NODE_ENABLED(n) ? 1.0f : 0.5f);
}

const BKClass bk_cls_image = {.name = "image", .measure = image_measure, .draw = image_draw};

/** \} */

/* -------------------------------------------------------------------- */
/** \name Textures (Dear ImGui, textured meshes)
 * \{ */

/* A frame as a new texture (`tex` 0) or into texture `tex`. BUI_texture_*
 * holds straight RGBA (Dear ImGui's convention). */
static int texture_put(const BKImBuf *ib, int frame, int tex)
{
  int tw, th;
  if (!ib || frame < 0 || frame >= ib->frames ||
      (tex && (!BUI_texture_size(tex, &tw, &th) || tw != ib->x || th != ib->y)))
  {
    return 0;
  }
  /* bands of rows through the stack: no allocation per frame (video textures) */
  uint8_t band[64 * 1024];
  const int step = (int)sizeof(band) / (ib->x * 4);
  if (step < 1) {
    return 0;
  }
  const int t = tex ? tex : BUI_texture_create(ib->x, ib->y, NULL, 0);
  for (int y = 0; t && y < ib->y; y += step) {
    const int n = BK_MIN(step, ib->y - y);
    if (!imb_rgba_rows(ib, frame, y, n, band) || !BUI_texture_update(t, 0, y, ib->x, n, band, 0)) {
      if (!tex) {
        BUI_texture_free(t);
      }
      return 0;
    }
  }
  return t;
}

int BK_imbuf_texture(const BKImBuf *ib, int frame)
{
  return texture_put(ib, frame, 0);
}

int BK_imbuf_texture_update(const BKImBuf *ib, int frame, int tex)
{
  return tex > 0 && texture_put(ib, frame, tex);
}

/** \} */
