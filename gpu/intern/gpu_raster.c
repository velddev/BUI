/* BUI software raster backend — see GPU_raster.h.
 * SPDX-License-Identifier: GPL-2.0-or-later */
#include "GPU_raster.h"
#include "gpu_d3d11.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

#define TILE_W 64
#define TILE_H 32

struct BuiRaster {
  int w, h;
  uint32_t *pix;  /* current frame */
  uint32_t *prev; /* last presented frame, for damage */
  int full_damage;

  /* clip, exclusive max, always inside the buffer */
  int cx0, cy0, cx1, cy1;

  int (*damage)[4];
  int damage_len, damage_cap;

  char font_family[64];
  int font_flags;

  BuiGpuSink sink;

  /* GPU mode: primitives are recorded as instances (gpu_d3d11.c) and pix /
   * prev are not allocated (read back on demand by BUI_raster_pixels). */
  BuiGpu *gpu;
  BuiGpu *gpu_pending;
  int gpu_switched; /* the renderer changed: the next frame must be full */
  struct BuiRaster *scratch; /* CPU coverage for polygons in GPU mode */
};

/* -------------------------------------------------------------------- */
/** \name Pixel helpers
 * \{ */

typedef struct Paint {
  uint32_t rgb; /* 0x00RRGGBB */
  int a;        /* 0..256 */
} Paint;

static inline int clampi(int v, int lo, int hi)
{
  return v < lo ? lo : (v > hi ? hi : v);
}

static inline float clampf(float v, float lo, float hi)
{
  return v < lo ? lo : (v > hi ? hi : v);
}

static inline Paint paint_from(const float c[4])
{
  Paint p;
  int r = (int)(clampf(c[0], 0.0f, 1.0f) * 255.0f + 0.5f);
  int g = (int)(clampf(c[1], 0.0f, 1.0f) * 255.0f + 0.5f);
  int b = (int)(clampf(c[2], 0.0f, 1.0f) * 255.0f + 0.5f);
  p.rgb = ((uint32_t)r << 16) | ((uint32_t)g << 8) | (uint32_t)b;
  p.a = (int)(clampf(c[3], 0.0f, 1.0f) * 256.0f + 0.5f);
  return p;
}

/* Blend s over d with factor f in 0..256. */
static inline uint32_t blend(uint32_t d, uint32_t s, uint32_t f)
{
  const uint32_t nf = 256 - f;
  const uint32_t rb = (((s & 0xFF00FFu) * f + (d & 0xFF00FFu) * nf) >> 8) & 0xFF00FFu;
  const uint32_t g = (((s & 0x00FF00u) * f + (d & 0x00FF00u) * nf) >> 8) & 0x00FF00u;
  return rb | g;
}

/* Write with coverage cov (0..1) scaled by paint alpha. */
static inline void put_cov(uint32_t *px, Paint p, float cov)
{
  if (cov <= 0.0f) {
    return;
  }
  int f = (int)(cov * (float)p.a + 0.5f);
  if (f >= 256) {
    *px = p.rgb;
  }
  else if (f > 0) {
    *px = blend(*px, p.rgb, (uint32_t)f);
  }
}

/* Integer bbox [x0,x1) x [y0,y1) clipped; returns 0 when empty. */
static int clip_box(const BuiRaster *r, float fx0, float fy0, float fx1, float fy1, int *x0, int *y0,
                    int *x1, int *y1)
{
  if (!(fx1 > fx0) || !(fy1 > fy0)) {
    return 0;
  }
  /* Guard against NaN / huge values before converting to int. */
  fx0 = clampf(fx0, -1e6f, 1e6f);
  fy0 = clampf(fy0, -1e6f, 1e6f);
  fx1 = clampf(fx1, -1e6f, 1e6f);
  fy1 = clampf(fy1, -1e6f, 1e6f);
  *x0 = clampi((int)floorf(fx0), r->cx0, r->cx1);
  *y0 = clampi((int)floorf(fy0), r->cy0, r->cy1);
  *x1 = clampi((int)ceilf(fx1), r->cx0, r->cx1);
  *y1 = clampi((int)ceilf(fy1), r->cy0, r->cy1);
  return *x1 > *x0 && *y1 > *y0;
}

/** \} */

/* -------------------------------------------------------------------- */
/** \name Lifetime / frame
 * \{ */

static void raster_reset_clip(BuiRaster *r)
{
  r->cx0 = 0;
  r->cy0 = 0;
  r->cx1 = r->w;
  r->cy1 = r->h;
}

static void raster_alloc(BuiRaster *r, int w, int h)
{
  free(r->pix);
  free(r->prev);
  r->w = w > 0 ? w : 1;
  r->h = h > 0 ? h : 1;
  if (r->gpu) { /* the frame lives on the GPU */
    r->pix = r->prev = NULL;
    r->full_damage = 1;
    raster_reset_clip(r);
    return;
  }
  r->pix = (uint32_t *)calloc((size_t)r->w * r->h, sizeof(uint32_t));
  r->prev = (uint32_t *)calloc((size_t)r->w * r->h, sizeof(uint32_t));
  r->full_damage = 1;
  raster_reset_clip(r);
}

static void sink_begin(void *u, int w, int h)
{
  BUI_raster_begin_frame((BuiRaster *)u, w, h);
}
static void sink_scissor(void *u, int x, int y, int w, int h)
{
  BUI_raster_set_clip((BuiRaster *)u, x, y, w, h);
}
static void sink_tris(void *u, const float (*co)[2], const float (*col)[4], int n)
{
  BUI_raster_triangles((BuiRaster *)u, co, col, n);
}
static void sink_lines(void *u, const float (*co)[2], const float (*col)[4], int n)
{
  for (int i = 0; i + 1 < n; i += 2) {
    BUI_raster_line((BuiRaster *)u, co[i][0], co[i][1], co[i + 1][0], co[i + 1][1], 1.0f, col[i]);
  }
}
static void sink_points(void *u, const float (*co)[2], const float (*col)[4], int n, float size)
{
  BUI_raster_points((BuiRaster *)u, co, col, n, size);
}
static void sink_text(void *u, float x, float y, const char *text, float size, const float rgba[4])
{
  BUI_raster_text((BuiRaster *)u, x, y, text, (int)strlen(text), size, rgba);
}
static void sink_end(void *u)
{
  BUI_raster_end_frame((BuiRaster *)u);
}

BuiRaster *BUI_raster_create(int width, int height)
{
  BuiRaster *r = (BuiRaster *)calloc(1, sizeof(BuiRaster));
  if (!r) {
    return NULL;
  }
  raster_alloc(r, width, height);
  r->sink.user = r;
  r->sink.begin_frame = sink_begin;
  r->sink.scissor = sink_scissor;
  r->sink.triangles = sink_tris;
  r->sink.lines = sink_lines;
  r->sink.points = sink_points;
  r->sink.text = sink_text;
  r->sink.end_frame = sink_end;
  return r;
}

void BUI_raster_free(BuiRaster *r)
{
  if (!r) {
    return;
  }
  BUI_raster_gpu_release(r);
  BUI_raster_free(r->scratch);
  free(r->pix);
  free(r->prev);
  free(r->damage);
  free(r);
}

const BuiGpuSink *BUI_raster_sink(BuiRaster *r)
{
  return &r->sink;
}

void BUI_raster_begin_frame(BuiRaster *r, int width, int height)
{
  if (width > 0 && height > 0 && (width != r->w || height != r->h)) {
    raster_alloc(r, width, height);
  }
  raster_reset_clip(r);
  if (r->gpu) {
    bui_gpu_begin(r->gpu, r->w, r->h);
  }
}

/* -------------------------------------------------------------------- */
/** \name GPU mode
 * \{ */

static int g_gpu_default = -1;

void BUI_gpu_set_default(int on)
{
  g_gpu_default = on ? 1 : 0;
}

int BUI_gpu_default(void)
{
  const char *env = getenv("BUI_GPU");
  if (env && env[0]) {
    return env[0] == '1' || env[0] == 'y' || env[0] == 'Y' || env[0] == 't' || env[0] == 'T';
  }
  return g_gpu_default > 0;
}

#ifndef BUI_SLUG_DEFAULT
#  define BUI_SLUG_DEFAULT 1
#endif
static int g_slug = -1;

void BUI_gpu_set_slug(int on)
{
  g_slug = on ? 1 : 0;
}

int BUI_gpu_slug(void)
{
  /* called per text run: read the environment once */
  static int env_slug = -2; /* -2 unread, -1 unset */
  if (env_slug == -2) {
    const char *env = getenv("BUI_SLUG");
    env_slug = (env && env[0]) ? (env[0] == '1' || env[0] == 'y' || env[0] == 'Y' || env[0] == 't' ||
                                  env[0] == 'T') :
                                 -1;
  }
  if (env_slug >= 0) {
    return env_slug;
  }
  return g_slug >= 0 ? g_slug : BUI_SLUG_DEFAULT;
}

int BUI_raster_gpu_attach(BuiRaster *r, void *window_handle, int wait)
{
  if (r->gpu || r->gpu_pending) {
    return 1;
  }
  r->gpu_pending = bui_gpu_create(window_handle);
  if (!r->gpu_pending) {
    return 0;
  }
  if (wait) {
    bui_gpu_state(r->gpu_pending, 1);
    BUI_raster_gpu_poll(r);
    return r->gpu != NULL;
  }
  return 1;
}

int BUI_raster_gpu_poll(BuiRaster *r)
{
  if (r->gpu_pending) {
    const int st = bui_gpu_state(r->gpu_pending, 0);
    if (st == 0) {
      return 0;
    }
    if (st > 0) {
      r->gpu = r->gpu_pending;
      r->gpu_pending = NULL;
      raster_alloc(r, r->w, r->h); /* drops the CPU frame */
      bui_gpu_begin(r->gpu, r->w, r->h);
      r->gpu_switched = 1;
    }
    else {
      bui_gpu_free(r->gpu_pending);
      r->gpu_pending = NULL;
    }
  }
  const int sw = r->gpu_switched;
  r->gpu_switched = 0;
  return sw;
}

void BUI_raster_gpu_release(BuiRaster *r)
{
  bui_gpu_free(r->gpu_pending);
  r->gpu_pending = NULL;
  if (r->gpu) {
    bui_gpu_free(r->gpu);
    r->gpu = NULL;
    r->w = r->h = 0; /* the next frame allocates the CPU frame */
  }
}

int BUI_raster_gpu_pending(const BuiRaster *r)
{
  return r->gpu_pending != NULL;
}

int BUI_raster_is_gpu(const BuiRaster *r)
{
  return r->gpu != NULL;
}

/* back to the software raster (device lost) */
static void gpu_drop(BuiRaster *r)
{
  bui_gpu_free(r->gpu);
  r->gpu = NULL;
  r->pix = r->prev = NULL;
  raster_alloc(r, r->w, r->h);
  r->gpu_switched = 1;
}

/* Instance over the clipped pixel box; NULL when nothing to draw. */
static BuiGpuInst *gpu_inst(BuiRaster *r, int kind, int x0, int y0, int x1, int y1)
{
  return bui_gpu_inst(r->gpu, kind, x0, y0, x1, y1);
}

static void v4(float d[4], float a, float b, float c, float e)
{
  d[0] = a;
  d[1] = b;
  d[2] = c;
  d[3] = e;
}

static void c4(float d[4], const float *s)
{
  if (s) {
    memcpy(d, s, sizeof(float[4]));
  }
}

/** \} */

static void damage_push(BuiRaster *r, int x, int y, int w, int h)
{
  /* Merge with the previous rect when it is the same column span directly
   * above (common for tall widgets), keeping uploads few and large. */
  if (r->damage_len > 0) {
    int *last = r->damage[r->damage_len - 1];
    if (last[0] == x && last[2] == w && last[1] + last[3] == y) {
      last[3] += h;
      return;
    }
  }
  if (r->damage_len == r->damage_cap) {
    int cap = r->damage_cap ? r->damage_cap * 2 : 64;
    int(*nd)[4] = (int(*)[4])realloc(r->damage, sizeof(int[4]) * (size_t)cap);
    if (!nd) {
      return;
    }
    r->damage = nd;
    r->damage_cap = cap;
  }
  int *d = r->damage[r->damage_len++];
  d[0] = x;
  d[1] = y;
  d[2] = w;
  d[3] = h;
}

void BUI_raster_end_frame(BuiRaster *r)
{
  r->damage_len = 0;
  if (r->gpu) {
    bui_gpu_damage(r->gpu, 0, 0, 0, 0);
    damage_push(r, 0, 0, r->w, r->h);
    r->full_damage = 0;
    return;
  }
  const size_t total = (size_t)r->w * r->h;
  if (r->full_damage) {
    damage_push(r, 0, 0, r->w, r->h);
    memcpy(r->prev, r->pix, total * sizeof(uint32_t));
    r->full_damage = 0;
    return;
  }
  for (int ty = 0; ty < r->h; ty += TILE_H) {
    const int th = (ty + TILE_H <= r->h) ? TILE_H : r->h - ty;
    int run_x = -1;
    /* One step past the last tile acts as a clean sentinel that closes a
     * run touching the right edge (for any width, not just multiples of
     * TILE_W — otherwise right-edge changes were never reported). */
    for (int tx = 0; tx < r->w + TILE_W; tx += TILE_W) {
      int dirty = 0;
      if (tx < r->w) {
        const int tw = (tx + TILE_W <= r->w) ? TILE_W : r->w - tx;
        for (int y = ty; y < ty + th; y++) {
          const size_t o = (size_t)y * r->w + tx;
          if (memcmp(r->pix + o, r->prev + o, (size_t)tw * sizeof(uint32_t)) != 0) {
            dirty = 1;
            break;
          }
        }
      }
      if (dirty && run_x < 0) {
        run_x = tx;
      }
      else if (!dirty && run_x >= 0) {
        const int end = tx < r->w ? tx : r->w;
        damage_push(r, run_x, ty, end - run_x, th);
        for (int y = ty; y < ty + th; y++) {
          const size_t o = (size_t)y * r->w + run_x;
          memcpy(r->prev + o, r->pix + o, (size_t)(end - run_x) * sizeof(uint32_t));
        }
        run_x = -1;
      }
    }
  }
}

int BUI_raster_width(const BuiRaster *r)
{
  return r->w;
}
int BUI_raster_height(const BuiRaster *r)
{
  return r->h;
}
const uint32_t *BUI_raster_pixels(const BuiRaster *r)
{
  if (r->gpu) { /* read the GPU frame back (tests, screenshots) */
    BuiRaster *m = (BuiRaster *)r;
    if (!m->prev) {
      m->prev = (uint32_t *)malloc((size_t)r->w * r->h * sizeof(uint32_t));
    }
    if (m->prev && bui_gpu_read(r->gpu, m->prev, r->w, r->h)) {
      return m->prev;
    }
  }
  return r->pix;
}

int BUI_raster_damage(const BuiRaster *r, const int (**r_rects)[4])
{
  *r_rects = (const int(*)[4])r->damage;
  return r->damage_len;
}

void BUI_raster_invalidate(BuiRaster *r)
{
  r->full_damage = 1;
}

size_t BUI_raster_view_size(void)
{
  return sizeof(BuiRaster);
}

void BUI_raster_view(const BuiRaster *src, BuiRaster *dst, int x0, int y0, int x1, int y1)
{
  *dst = *src; /* shares the pixels; drawing only touches pix inside the clip */
  dst->cx0 = clampi(x0 > src->cx0 ? x0 : src->cx0, 0, src->w);
  dst->cy0 = clampi(y0 > src->cy0 ? y0 : src->cy0, 0, src->h);
  dst->cx1 = clampi(x1 < src->cx1 ? x1 : src->cx1, dst->cx0, src->w);
  dst->cy1 = clampi(y1 < src->cy1 ? y1 : src->cy1, dst->cy0, src->h);
}

void BUI_raster_set_clip(BuiRaster *r, int x, int y, int w, int h)
{
  if (w <= 0 || h <= 0) {
    raster_reset_clip(r);
    return;
  }
  r->cx0 = clampi(x, 0, r->w);
  r->cy0 = clampi(y, 0, r->h);
  r->cx1 = clampi(x + w, 0, r->w);
  r->cy1 = clampi(y + h, 0, r->h);
}

/** \} */

/* -------------------------------------------------------------------- */
/** \name Primitives
 * \{ */

void BUI_raster_fill_rect(BuiRaster *r, float x, float y, float w, float h, const float rgba[4])
{
  int x0, y0, x1, y1;
  if (!clip_box(r, x, y, x + w, y + h, &x0, &y0, &x1, &y1)) {
    return;
  }
  if (r->gpu) {
    BuiGpuInst *in = rgba[3] > 0.0f ? gpu_inst(r, BG_RECT, x0, y0, x1, y1) : NULL;
    if (in) {
      v4(in->rect, x, y, w, h);
      c4(in->c0, rgba);
    }
    return;
  }
  const Paint p = paint_from(rgba);
  if (p.a <= 0) {
    return;
  }
  const float fx0 = x, fx1 = x + w, fy0 = y, fy1 = y + h;
  for (int py = y0; py < y1; py++) {
    const float cy = fminf(fy1, (float)py + 1.0f) - fmaxf(fy0, (float)py);
    if (cy <= 0.0f) {
      continue;
    }
    uint32_t *row = r->pix + (size_t)py * r->w;
    /* fully covered interior span */
    const int ix0 = clampi((int)ceilf(fx0), x0, x1);
    const int ix1 = clampi((int)floorf(fx1), x0, x1);
    for (int px = x0; px < ix0 && px < x1; px++) {
      const float cx = fminf(fx1, (float)px + 1.0f) - fmaxf(fx0, (float)px);
      put_cov(row + px, p, cx * cy);
    }
    if (cy >= 1.0f && p.a >= 256) {
      for (int px = ix0; px < ix1; px++) {
        row[px] = p.rgb;
      }
    }
    else {
      const int f = (int)(cy * (float)p.a + 0.5f);
      if (f >= 256) {
        for (int px = ix0; px < ix1; px++) {
          row[px] = p.rgb;
        }
      }
      else if (f > 0) {
        for (int px = ix0; px < ix1; px++) {
          row[px] = blend(row[px], p.rgb, (uint32_t)f);
        }
      }
    }
    /* trailing partial column(s); when the rect sits inside one pixel the
     * leading loop already covered it (ix1 < ix0). */
    for (int px = (ix1 > ix0 ? ix1 : ix0); px < x1; px++) {
      const float cx = fminf(fx1, (float)px + 1.0f) - fmaxf(fx0, (float)px);
      put_cov(row + px, p, cx * cy);
    }
  }
}

void BUI_raster_frame_rect(
    BuiRaster *r, float x, float y, float w, float h, float t, const float rgba[4])
{
  if (t <= 0.0f || w <= 0.0f || h <= 0.0f) {
    return;
  }
  if (t * 2.0f >= w || t * 2.0f >= h) {
    BUI_raster_fill_rect(r, x, y, w, h, rgba);
    return;
  }
  /* Non-overlapping sides so translucent borders blend once. */
  BUI_raster_fill_rect(r, x, y, w, t, rgba);
  BUI_raster_fill_rect(r, x, y + h - t, w, t, rgba);
  BUI_raster_fill_rect(r, x, y + t, t, h - 2.0f * t, rgba);
  BUI_raster_fill_rect(r, x + w - t, y + t, t, h - 2.0f * t, rgba);
}

void BUI_raster_round_rect(
    BuiRaster *r, float x, float y, float w, float h, float radius, const float rgba[4])
{
  if (w <= 0.0f || h <= 0.0f) {
    return;
  }
  const float hw = w * 0.5f, hh = h * 0.5f;
  float rad = fminf(radius, fminf(hw, hh));
  if (rad <= 0.25f) {
    BUI_raster_fill_rect(r, x, y, w, h, rgba);
    return;
  }
  int x0, y0, x1, y1;
  if (!clip_box(r, x, y, x + w, y + h, &x0, &y0, &x1, &y1)) {
    return;
  }
  if (r->gpu) {
    BuiGpuInst *in = rgba[3] > 0.0f ? gpu_inst(r, BG_WIDGET, x0, y0, x1, y1) : NULL;
    if (in) {
      v4(in->rect, x, y, w, h);
      c4(in->c0, rgba);
      c4(in->c1, rgba);
      v4(in->p, BG_WIDGET, rad, rad, 0.0f);
      v4(in->uv, BUI_CORNER_ALL, BG_W_INNER, 0.0f, 0.0f);
    }
    return;
  }
  const Paint p = paint_from(rgba);
  if (p.a <= 0) {
    return;
  }
  const float cx = x + hw, cy = y + hh;
  const float bx = hw - rad, by = hh - rad;
  const float rin = rad - 0.5f; /* coverage is exactly 1 inside this radius */
  for (int py = y0; py < y1; py++) {
    uint32_t *row = r->pix + (size_t)py * r->w;
    const float qy = fabsf((float)py + 0.5f - cy) - by;
    /* Half-width (from cx) of this row's fully covered span: the interior
     * is filled directly; the SDF only runs on the anti-aliased rim. */
    float ih = -1.0f;
    if (rin > 0.0f) {
      if (qy <= 0.0f) {
        ih = bx + rin;
      }
      else if (qy < rin) {
        ih = bx + sqrtf(rin * rin - qy * qy);
      }
    }
    int sa = x1, sb = x0; /* solid span [sa, sb) */
    if (ih >= 0.0f) {
      sa = clampi((int)ceilf(cx - ih - 0.5f), x0, x1);
      sb = clampi((int)floorf(cx + ih - 0.5f) + 1, x0, x1);
      if (sb > sa) {
        if (p.a >= 256) {
          for (int px = sa; px < sb; px++) {
            row[px] = p.rgb;
          }
        }
        else {
          for (int px = sa; px < sb; px++) {
            row[px] = blend(row[px], p.rgb, (uint32_t)p.a);
          }
        }
      }
      else {
        sa = x1;
        sb = x0;
      }
    }
    for (int px = x0; px < x1; px++) {
      if (px == sa) {
        px = sb - 1; /* skip the solid span */
        continue;
      }
      const float qx = fabsf((float)px + 0.5f - cx) - bx;
      float d;
      if (qx <= 0.0f && qy <= 0.0f) {
        d = fmaxf(qx, qy) - rad;
        if (d <= -0.5f) {
          if (p.a >= 256) {
            row[px] = p.rgb;
          }
          else {
            row[px] = blend(row[px], p.rgb, (uint32_t)p.a);
          }
          continue;
        }
      }
      else {
        const float mx = fmaxf(qx, 0.0f), my = fmaxf(qy, 0.0f);
        d = sqrtf(mx * mx + my * my) + fminf(fmaxf(qx, qy), 0.0f) - rad;
      }
      put_cov(row + px, p, clampf(0.5f - d, 0.0f, 1.0f));
    }
  }
}

void BUI_raster_circle(
    BuiRaster *r, float cx, float cy, float radius, float line_width, const float rgba[4])
{
  if (radius <= 0.0f) {
    return;
  }
  const float ext = radius + (line_width > 0.0f ? line_width : 0.0f) + 1.0f;
  int x0, y0, x1, y1;
  if (!clip_box(r, cx - ext, cy - ext, cx + ext, cy + ext, &x0, &y0, &x1, &y1)) {
    return;
  }
  if (r->gpu) {
    BuiGpuInst *in = rgba[3] > 0.0f ? gpu_inst(r, BG_ELLIPSE, x0, y0, x1, y1) : NULL;
    if (in) {
      v4(in->rect, cx, cy, radius, radius);
      c4(in->c0, rgba);
      in->p[1] = line_width;
    }
    return;
  }
  const Paint p = paint_from(rgba);
  if (p.a <= 0) {
    return;
  }
  const float half_lw = line_width * 0.5f;
  for (int py = y0; py < y1; py++) {
    uint32_t *row = r->pix + (size_t)py * r->w;
    const float dy = (float)py + 0.5f - cy;
    for (int px = x0; px < x1; px++) {
      const float dx = (float)px + 0.5f - cx;
      const float dist = sqrtf(dx * dx + dy * dy);
      float cov;
      if (line_width > 0.0f) {
        cov = clampf(half_lw - fabsf(dist - radius) + 0.5f, 0.0f, 1.0f);
      }
      else {
        cov = clampf(radius - dist + 0.5f, 0.0f, 1.0f);
      }
      put_cov(row + px, p, cov);
    }
  }
}

void BUI_raster_line(
    BuiRaster *r, float x1, float y1, float x2, float y2, float width, const float rgba[4])
{
  if (width <= 0.0f) {
    width = 1.0f;
  }
  /* Axis-aligned lines are pixel-exact spans (crisp, like GL lines on the
   * pixel grid). Integer coords address the pixel to the right/below. */
  if (y1 == y2) {
    const float xa = fminf(x1, x2), xb = fmaxf(x1, x2);
    BUI_raster_fill_rect(r, xa, y1 - (width - 1.0f) * 0.5f, fmaxf(xb - xa, 1.0f), width, rgba);
    return;
  }
  if (x1 == x2) {
    const float ya = fminf(y1, y2), yb = fmaxf(y1, y2);
    BUI_raster_fill_rect(r, x1 - (width - 1.0f) * 0.5f, ya, width, fmaxf(yb - ya, 1.0f), rgba);
    return;
  }
  /* Diagonal: anti-aliased capsule between pixel centers. */
  const float ax = x1 + 0.5f, ay = y1 + 0.5f, bx = x2 + 0.5f, by = y2 + 0.5f;
  const float hw = width * 0.5f;
  int ix0, iy0, ix1, iy1;
  if (!clip_box(r, fminf(ax, bx) - hw - 1.0f, fminf(ay, by) - hw - 1.0f,
                fmaxf(ax, bx) + hw + 1.0f, fmaxf(ay, by) + hw + 1.0f, &ix0, &iy0, &ix1, &iy1))
  {
    return;
  }
  if (r->gpu) {
    BuiGpuInst *in = rgba[3] > 0.0f ? gpu_inst(r, BG_LINE, ix0, iy0, ix1, iy1) : NULL;
    if (in) {
      v4(in->rect, ax, ay, bx, by);
      c4(in->c0, rgba);
      in->p[1] = hw;
    }
    return;
  }
  const Paint p = paint_from(rgba);
  if (p.a <= 0) {
    return;
  }
  const float vx = bx - ax, vy = by - ay;
  const float len2 = vx * vx + vy * vy;
  for (int py = iy0; py < iy1; py++) {
    uint32_t *row = r->pix + (size_t)py * r->w;
    const float wy = (float)py + 0.5f - ay;
    for (int px = ix0; px < ix1; px++) {
      const float wx = (float)px + 0.5f - ax;
      const float t = clampf((wx * vx + wy * vy) / len2, 0.0f, 1.0f);
      const float ex = wx - vx * t, ey = wy - vy * t;
      const float d = sqrtf(ex * ex + ey * ey) - hw;
      put_cov(row + px, p, clampf(0.5f - d, 0.0f, 1.0f));
    }
  }
}

void BUI_raster_ellipse(BuiRaster *r,
                        float cx,
                        float cy,
                        float rx,
                        float ry,
                        float line_width,
                        const float rgba[4])
{
  rx = fabsf(rx);
  ry = fabsf(ry);
  if (rx <= 0.0f || ry <= 0.0f) {
    return;
  }
  if (fabsf(rx - ry) < 1e-3f) {
    BUI_raster_circle(r, cx, cy, rx, line_width, rgba);
    return;
  }
  const float ext = (line_width > 0.0f ? line_width : 0.0f) + 1.0f;
  int x0, y0, x1, y1;
  if (!clip_box(r, cx - rx - ext, cy - ry - ext, cx + rx + ext, cy + ry + ext, &x0, &y0, &x1,
                &y1))
  {
    return;
  }
  if (r->gpu) {
    BuiGpuInst *in = rgba[3] > 0.0f ? gpu_inst(r, BG_ELLIPSE, x0, y0, x1, y1) : NULL;
    if (in) {
      v4(in->rect, cx, cy, rx, ry);
      c4(in->c0, rgba);
      in->p[1] = line_width;
    }
    return;
  }
  const Paint p = paint_from(rgba);
  if (p.a <= 0) {
    return;
  }
  const float half_lw = line_width * 0.5f;
  for (int py = y0; py < y1; py++) {
    uint32_t *row = r->pix + (size_t)py * r->w;
    const float dy = ((float)py + 0.5f - cy) / ry;
    for (int px = x0; px < x1; px++) {
      const float dx = ((float)px + 0.5f - cx) / rx;
      /* First-order distance: (|p/r| - 1) / |grad| — accurate near the edge,
       * which is all anti-aliasing needs. */
      const float k = sqrtf(dx * dx + dy * dy);
      const float gx = dx / rx, gy = dy / ry;
      const float g = sqrtf(gx * gx + gy * gy);
      const float d = g > 0.0f ? (k - 1.0f) * k / g : -fminf(rx, ry);
      float cov;
      if (line_width > 0.0f) {
        cov = clampf(half_lw - fabsf(d) + 0.5f, 0.0f, 1.0f);
      }
      else {
        cov = clampf(0.5f - d, 0.0f, 1.0f);
      }
      put_cov(row + px, p, cov);
    }
  }
}

/* Signed distance to a rounded box with a per-corner radius mask. Pixel
 * center (sx, sy); box center (cx, cy), half size (hw, hh). */
static inline float sd_widget_box(
    float sx, float sy, float cx, float cy, float hw, float hh, float rad, int corners)
{
  const float px = sx - cx, py = sy - cy;
  int bit;
  if (py < 0.0f) {
    bit = px < 0.0f ? BUI_CORNER_TOP_LEFT : BUI_CORNER_TOP_RIGHT;
  }
  else {
    bit = px < 0.0f ? BUI_CORNER_BOTTOM_LEFT : BUI_CORNER_BOTTOM_RIGHT;
  }
  const float r = (corners & bit) ? rad : 0.0f;
  const float qx = fabsf(px) - (hw - r), qy = fabsf(py) - (hh - r);
  /* Straight edges need no sqrt; only true corner pixels do. */
  if (qx <= 0.0f || qy <= 0.0f) {
    return fmaxf(qx, qy) - r;
  }
  return sqrtf(qx * qx + qy * qy) - r;
}

void BUI_raster_widget(BuiRaster *r,
                       float x,
                       float y,
                       float w,
                       float h,
                       float radius,
                       int corners,
                       const float inner_top[4],
                       const float inner_bottom[4],
                       const float outline[4],
                       float outline_width,
                       const float emboss[4])
{
  if (w <= 0.0f || h <= 0.0f) {
    return;
  }
  const float hw = w * 0.5f, hh = h * 0.5f;
  const float rad = clampf(radius, 0.0f, fminf(hw, hh));
  const float cx = x + hw, cy = y + hh;
  const float ow = outline ? fmaxf(outline_width, 0.0f) : 0.0f;
  const int has_emboss = emboss && emboss[3] > 0.0f;
  int x0, y0, x1, y1;
  if (!clip_box(r, x - 1.0f, y - 1.0f, x + w + 1.0f, y + h + (has_emboss ? 2.0f : 1.0f), &x0, &y0,
                &x1, &y1))
  {
    return;
  }
  const float *top = inner_top ? inner_top : inner_bottom;
  const float *bot = inner_bottom ? inner_bottom : inner_top;
  if (r->gpu) {
    BuiGpuInst *in = gpu_inst(r, BG_WIDGET, x0, y0, x1, y1);
    if (in) {
      v4(in->rect, x, y, w, h);
      c4(in->c0, top);
      c4(in->c1, bot);
      if (ow > 0.0f) {
        c4(in->c2, outline);
      }
      if (has_emboss) {
        c4(in->c3, emboss);
      }
      v4(in->p, BG_WIDGET, rad, rad, 1.0f);
      v4(in->uv, (float)corners, (float)(BG_W_LEGACY | (top ? BG_W_INNER : 0)), ow, 0.0f);
    }
    return;
  }
  const int gradient = top && bot && memcmp(top, bot, sizeof(float[4])) != 0;
  Paint p_inner = top ? paint_from(top) : (Paint){0, 0};
  const Paint p_out = outline ? paint_from(outline) : (Paint){0, 0};
  const Paint p_emb = has_emboss ? paint_from(emboss) : (Paint){0, 0};
  /* Fully covered interior margin: no SDF needed farther than this from
   * every edge (skips sqrt for the bulk of each widget). */
  const float safe = rad + ow + 1.0f;

  for (int py = y0; py < y1; py++) {
    uint32_t *row = r->pix + (size_t)py * r->w;
    const float sy = (float)py + 0.5f;
    if (gradient) {
      const float t = clampf((sy - y) / h, 0.0f, 1.0f);
      float c[4];
      for (int k = 0; k < 4; k++) {
        c[k] = top[k] + (bot[k] - top[k]) * t;
      }
      p_inner = paint_from(c);
    }
    const int row_inside = (sy - y) >= safe && (y + h - sy) >= safe;
    /* Interior span of this row: filled directly, no per-pixel SDF. */
    int sa = x1, sb = x1;
    if (row_inside) {
      sa = clampi((int)ceilf(x + safe - 0.5f), x0, x1);
      sb = clampi((int)floorf(x + w - safe - 0.5f) + 1, x0, x1);
      if (sb <= sa) {
        sa = sb = x1;
      }
      else if (top) {
        if (p_inner.a >= 256) {
          for (int px = sa; px < sb; px++) {
            row[px] = p_inner.rgb;
          }
        }
        else if (p_inner.a > 0) {
          for (int px = sa; px < sb; px++) {
            row[px] = blend(row[px], p_inner.rgb, (uint32_t)p_inner.a);
          }
        }
      }
    }
    for (int px = x0; px < x1; px++) {
      if (px == sa) {
        px = sb - 1; /* skip the interior span */
        continue;
      }
      const float sx = (float)px + 0.5f;
      const float d = sd_widget_box(sx, sy, cx, cy, hw, hh, rad, corners);
      const float cov = clampf(0.5f - d, 0.0f, 1.0f);
      if (has_emboss && sy > cy) {
        /* Emboss: the same shape shifted 1px down, visible only outside. */
        const float de = sd_widget_box(sx, sy - 1.0f, cx, cy, hw, hh, rad, corners);
        const float ecov = clampf(0.5f - de, 0.0f, 1.0f) - cov;
        if (ecov > 0.0f) {
          put_cov(row + px, p_emb, ecov);
        }
      }
      if (cov <= 0.0f) {
        continue;
      }
      if (top) {
        put_cov(row + px, p_inner, cov);
      }
      if (ow > 0.0f) {
        /* Outline band: inside the edge, `ow` pixels wide. */
        const float inner_cov = clampf(0.5f - (d + ow), 0.0f, 1.0f);
        const float band = cov - inner_cov;
        if (band > 0.0f) {
          put_cov(row + px, p_out, band);
        }
      }
    }
  }
}

/* Blender's widget base (GPU_SHADER_2D_WIDGET_BASE) in software: the same
 * parameters interface_widgets.cc feeds the shader. Outer shape `rect`/`rad`,
 * inner shape `rect` inset by `pixelsize` with `radi`; the band between them
 * is the outline, the outer shape shifted down by one pixel (bottom half
 * only) is the emboss. */
void BUI_raster_widget_base(BuiRaster *r, const BuiWidgetBase *p)
{
  const float x = p->rect[0], y = p->rect[1], w = p->rect[2], h = p->rect[3];
  if (w <= 0.0f || h <= 0.0f) {
    return;
  }
  const float px1 = p->pixelsize > 0.0f ? p->pixelsize : 1.0f;
  const float hw = w * 0.5f, hh = h * 0.5f, cx = x + hw, cy = y + hh;
  const float rad = clampf(p->rad, 0.0f, fminf(hw, hh));
  const float hwi = hw - px1, hhi = hh - px1;
  const float radi = clampf(p->radi, 0.0f, fmaxf(0.0f, fminf(hwi, hhi)));
  const int has_inner = p->inner1[3] > 0.0f || p->inner2[3] > 0.0f || p->alpha_checker;
  const int has_outline = p->outline[3] > 0.0f;
  const int has_emboss = p->emboss[3] > 0.0f;
  if (!has_inner && !has_outline && !has_emboss) {
    return;
  }
  int x0, y0, x1, y1;
  if (!clip_box(r, x - 1.0f, y - 1.0f, x + w + 1.0f, y + h + px1 + 1.0f, &x0, &y0, &x1, &y1)) {
    return;
  }
  if (r->gpu) {
    BuiGpuInst *in = gpu_inst(r, BG_WIDGET, x0, y0, x1, y1);
    if (in) {
      v4(in->rect, x, y, w, h);
      c4(in->c0, p->inner1);
      c4(in->c1, p->inner2);
      if (has_outline) {
        c4(in->c2, p->outline);
      }
      if (has_emboss) {
        c4(in->c3, p->emboss);
      }
      v4(in->p, BG_WIDGET, rad, radi, px1);
      const int flags = (p->shade_horizontal ? BG_W_SHADE_H : 0) |
                        (p->alpha_checker ? BG_W_CHECKER : 0) | (has_inner ? BG_W_INNER : 0);
      v4(in->uv, (float)p->corners, (float)flags, 0.0f, 0.0f);
    }
    return;
  }
  const int gradient = memcmp(p->inner1, p->inner2, sizeof(float[4])) != 0;
  const int per_pixel_inner = p->alpha_checker || (gradient && p->shade_horizontal);
  const Paint p_out = paint_from(p->outline);
  const Paint p_emb = paint_from(p->emboss);
  Paint p_in = paint_from(p->inner1);
  /* Columns farther than this from the left/right edges never see a corner:
   * there the distance field only depends on the row (computed once). */
  const float cornerw = fmaxf(rad, radi) + px1 + 1.0f;
  const int ma = clampi((int)ceilf(x + cornerw - 0.5f), x0, x1);
  const int mb = clampi((int)floorf(x + w - cornerw - 0.5f) + 1, ma, x1);

  for (int py = y0; py < y1; py++) {
    uint32_t *row = r->pix + (size_t)py * r->w;
    const float sy = (float)py + 0.5f;
    if (gradient && !p->shade_horizontal) {
      const float t = clampf((sy - (y + px1)) / fmaxf(1.0f, h - 2.0f * px1), 0.0f, 1.0f);
      float c[4];
      for (int k = 0; k < 4; k++) {
        c[k] = p->inner1[k] + (p->inner2[k] - p->inner1[k]) * t;
      }
      p_in = paint_from(c);
    }
    /* -- middle columns: row-constant coverage -- */
    int span_a = ma, span_b = mb;
    if (per_pixel_inner) {
      span_a = span_b = x1; /* rare: checker / horizontal shading -> exact path */
    }
    if (span_a < span_b) {
      const float ay = fabsf(sy - cy);
      const float co = clampf(0.5f - (ay - hh), 0.0f, 1.0f);
      const float ci = fminf(co, clampf(0.5f - (ay - hhi), 0.0f, 1.0f));
      float ec = 0.0f;
      if (has_emboss && sy > cy) {
        ec = clampf(0.5f - (fabsf(sy - px1 - cy) - hh), 0.0f, 1.0f) - co;
      }
      const float band = has_outline ? co - ci : 0.0f;
      const int fill_inner = has_inner && ci > 0.0f;
      if (!(ec > 0.0f) && !(band > 0.0f) && fill_inner && ci >= 1.0f && p_in.a >= 256) {
        for (int i = span_a; i < span_b; i++) {
          row[i] = p_in.rgb; /* opaque interior: plain store */
        }
      }
      else if (ec > 0.0f || band > 0.0f || fill_inner) {
        const int fe = (int)(ec * (float)p_emb.a + 0.5f);
        const int fi = fill_inner ? (int)(ci * (float)p_in.a + 0.5f) : 0;
        const int fo = (int)(band * (float)p_out.a + 0.5f);
        for (int i = span_a; i < span_b; i++) {
          uint32_t d = row[i];
          if (fe > 0) {
            d = fe >= 256 ? p_emb.rgb : blend(d, p_emb.rgb, (uint32_t)fe);
          }
          if (fi > 0) {
            d = fi >= 256 ? p_in.rgb : blend(d, p_in.rgb, (uint32_t)fi);
          }
          if (fo > 0) {
            d = fo >= 256 ? p_out.rgb : blend(d, p_out.rgb, (uint32_t)fo);
          }
          row[i] = d;
        }
      }
    }
    /* -- corner columns (and everything on the exact path): full SDF -- */
    for (int px = x0; px < x1; px++) {
      if (px == span_a && span_a < span_b) {
        px = span_b - 1;
        continue;
      }
      const float sx = (float)px + 0.5f;
      const float d = sd_widget_box(sx, sy, cx, cy, hw, hh, rad, p->corners);
      const float co = clampf(0.5f - d, 0.0f, 1.0f);
      if (has_emboss && sy > cy) {
        const float de = sd_widget_box(sx, sy - px1, cx, cy, hw, hh, rad, p->corners);
        const float ec = clampf(0.5f - de, 0.0f, 1.0f) - co;
        if (ec > 0.0f) {
          put_cov(row + px, p_emb, ec);
        }
      }
      if (co <= 0.0f) {
        continue;
      }
      const float di = sd_widget_box(sx, sy, cx, cy, hwi, hhi, radi, p->corners);
      const float ci = fminf(co, clampf(0.5f - di, 0.0f, 1.0f));
      if (has_inner && ci > 0.0f) {
        if (p->alpha_checker) {
          /* checkerColorAndSize: 100/255 and 160/255, 8 px cells */
          const int cell = ((int)((sx - x) / 8.0f) + (int)((sy - y) / 8.0f)) & 1;
          const float g = cell ? 160.0f / 255.0f : 100.0f / 255.0f;
          const float gc[4] = {g, g, g, 1.0f};
          put_cov(row + px, paint_from(gc), ci);
        }
        Paint pi = p_in;
        if (gradient && p->shade_horizontal) {
          const float t = clampf((sx - (x + px1)) / fmaxf(1.0f, w - 2.0f * px1), 0.0f, 1.0f);
          float c[4];
          for (int k = 0; k < 4; k++) {
            c[k] = p->inner1[k] + (p->inner2[k] - p->inner1[k]) * t;
          }
          pi = paint_from(c);
        }
        put_cov(row + px, pi, ci);
      }
      if (has_outline && co - ci > 0.0f) {
        put_cov(row + px, p_out, co - ci);
      }
    }
  }
}

/* Premultiplied source over the frame (whose alpha byte is unused). */
static inline uint32_t premul_over(uint32_t d, uint32_t s)
{
  const uint32_t ia = 255u - (s >> 24);
  uint32_t rb = (d & 0xFF00FFu) * ia + 0x800080u;
  uint32_t g = (d & 0x00FF00u) * ia + 0x008000u;
  rb = ((rb + ((rb >> 8) & 0xFF00FFu)) >> 8) & 0xFF00FFu; /* /255, rounded */
  g = ((g + ((g >> 8) & 0x00FF00u)) >> 8) & 0x00FF00u;
  return (s & 0xFFFFFFu) + rb + g;
}

/* All four channels times f/256. */
static inline uint32_t premul_scale(uint32_t c, uint32_t f)
{
  const uint32_t ag = ((c >> 8) & 0xFF00FFu) * f & 0xFF00FF00u;
  const uint32_t rb = ((c & 0xFF00FFu) * f >> 8) & 0xFF00FFu;
  return ag | rb;
}

/* An image pixel (premultiplied) times ga/256 over the frame. */
static inline void put_image_px(uint32_t *d, uint32_t c, uint32_t ga)
{
  if (ga < 256) {
    c = premul_scale(c, ga);
  }
  if (c >= 0xFF000000u) {
    *d = c & 0xFFFFFFu;
  }
  else if (c) {
    *d = premul_over(*d, c);
  }
}

/* Bilinear tap of premultiplied pixels: fx, fy are 8-bit fractions. */
static inline uint32_t bilerp(uint32_t c00, uint32_t c10, uint32_t c01, uint32_t c11, uint32_t fx,
                              uint32_t fy)
{
  const uint32_t w11 = fx * fy >> 8, w10 = fx - w11, w01 = fy - w11, w00 = 256 - fx - fy + w11;
  const uint32_t rb = ((c00 & 0xFF00FFu) * w00 + (c10 & 0xFF00FFu) * w10 +
                       (c01 & 0xFF00FFu) * w01 + (c11 & 0xFF00FFu) * w11) >> 8 & 0xFF00FFu;
  const uint32_t ag = (((c00 >> 8) & 0xFF00FFu) * w00 + ((c10 >> 8) & 0xFF00FFu) * w10 +
                       ((c01 >> 8) & 0xFF00FFu) * w01 + ((c11 >> 8) & 0xFF00FFu) * w11) &
                      0xFF00FF00u;
  return ag | rb;
}

static uint64_t pixmap_key(const BuiPixmap *img)
{
  if (img->key) {
    return img->key;
  }
  /* content hash: an image updated in place gets a new atlas entry */
  uint64_t h = 1469598103934665603ull;
  /* in 32-bit words: NV12 holds 1.5 bytes a pixel */
  const size_t px = (size_t)img->w * img->h;
  const size_t n = img->format == BUI_PIXMAP_ARGB ? px : px * 3 / 8;
  for (size_t i = 0; i < n; i++) {
    h = (h ^ img->px[i]) * 1099511628211ull;
  }
  return h | (1ull << 63); /* never a BKImBuf key (counters) */
}

/* -- forgotten images (GPU renderers free big-image textures) ------------------- */

#define FORGET_LOG 256 /* power of two */
static uint64_t g_forget[FORGET_LOG];
static volatile long g_forget_head = 0;

void BUI_image_forget(uint64_t key)
{
#ifdef _MSC_VER
  const unsigned i = (unsigned)_InterlockedIncrement(&g_forget_head) - 1;
#else
  const unsigned i = (unsigned)__atomic_fetch_add(&g_forget_head, 1, __ATOMIC_ACQ_REL);
#endif
  g_forget[i & (FORGET_LOG - 1)] = key;
}

int bui_image_forgotten(unsigned *seen, uint64_t *r_keys, int max)
{
#ifdef _MSC_VER
  const unsigned head = (unsigned)_InterlockedOr(&g_forget_head, 0);
#else
  const unsigned head = (unsigned)__atomic_load_n(&g_forget_head, __ATOMIC_ACQUIRE);
#endif
  const unsigned n = head - *seen;
  *seen = head;
  if (n > (unsigned)max || n > FORGET_LOG / 2) {
    return -1;
  }
  for (unsigned k = 0; k < n; k++) {
    r_keys[k] = g_forget[(head - n + k) & (FORGET_LOG - 1)];
  }
  return (int)n;
}

static inline int imin(int a, int b)
{
  return a < b ? a : b;
}

/* A pixmap over the atlas (BUI_IMAGE_ATLAS_MAX): textures of its own, tiled only when a
 * side is over the device's limit. Tiles overlap by a pixel so bilinear
 * samples at their seams read real neighbours; each tile draws the part of
 * the destination whose samples fall in its core. */
static void gpu_image_big(BuiRaster *r, float x, float y, float w, float h, const BuiPixmap *img,
                          float sx, float sy, float sw, float sh, float alpha, int flags,
                          int x0, int y0, int x1, int y1)
{
  const uint64_t key = pixmap_key(img);
  const int core = bui_gpu_max_texture(r->gpu) - 2;
  if (core < 1) {
    return;
  }
  const int tiled = img->w > core + 2 || img->h > core + 2;
  if (tiled && img->format != BUI_PIXMAP_ARGB) {
    return; /* video frames are decoded within the device's limit */
  }
  const int nx = tiled ? (img->w + core - 1) / core : 1, ny = tiled ? (img->h + core - 1) / core : 1;
  const float kx = w / sw, ky = h / sh;
  for (int j = 0; j < ny; j++) {
    for (int i = 0; i < nx; i++) {
      /* core [cx0, cx1) x [cy0, cy1); texture one pixel wider where it can */
      const int cx0 = tiled ? i * core : 0, cx1 = tiled ? imin(img->w, cx0 + core) : img->w;
      const int cy0 = tiled ? j * core : 0, cy1 = tiled ? imin(img->h, cy0 + core) : img->h;
      const float u0 = fmaxf((float)cx0, sx), u1 = fminf((float)cx1, sx + sw);
      const float v0 = fmaxf((float)cy0, sy), v1 = fminf((float)cy1, sy + sh);
      if (!(u1 > u0) || !(v1 > v0)) {
        continue;
      }
      const int tx0 = cx0 > 0 ? cx0 - 1 : 0, tx1 = imin(img->w, cx1 + 1);
      const int ty0 = cy0 > 0 ? cy0 - 1 : 0, ty1 = imin(img->h, cy1 + 1);
      const uint64_t tkey = tiled ? (key ^ ((uint64_t)(j * nx + i + 1) * 0x9E3779B97F4A7C15ull)) : key;
      const int tex = tiled ? bui_gpu_image_tex(r->gpu, tkey, key, 0, BUI_PIXMAP_ARGB,
                                                img->px + (size_t)ty0 * img->w + tx0, img->w,
                                                tx1 - tx0, ty1 - ty0) :
                              bui_gpu_image_tex(r->gpu, key, img->stream ? img->stream : key,
                                                img->stream != 0, img->format, img->px, img->w,
                                                img->w, img->h);
      if (!tex) {
        continue;
      }
      /* the destination pixels whose centers sample this core */
      int bx0 = x0, by0 = y0, bx1 = x1, by1 = y1;
      if (tiled) {
        bx0 = clampi((int)ceilf(x + (u0 - sx) * kx - 0.5f), x0, x1);
        bx1 = clampi((int)ceilf(x + (u1 - sx) * kx - 0.5f), x0, x1);
        by0 = clampi((int)ceilf(y + (v0 - sy) * ky - 0.5f), y0, y1);
        by1 = clampi((int)ceilf(y + (v1 - sy) * ky - 0.5f), y0, y1);
        if (bx1 <= bx0 || by1 <= by0) {
          continue;
        }
      }
      BuiGpuInst *in = bui_gpu_inst_image(r->gpu, tex, bx0, by0, bx1, by1);
      if (in) {
        v4(in->rect, x, y, w, h);
        v4(in->uv, sx - (float)tx0, sy - (float)ty0, sw, sh);
        in->p[1] = alpha;
        in->p[2] = (flags & BUI_IMAGE_NEAREST) ? 1.0f : 0.0f;
        in->p[3] = (float)img->format; /* > 0: NV12 (luma + chroma textures) */
      }
    }
  }
}

/* -- NV12 video frames --------------------------------------------------------------
 * Converted to RGB while drawing: only the pixels drawn are converted (the
 * decoder already scaled the frame to about the size shown, so this is
 * usually the 1:1 path). Limited-range YCbCr, 16.16 fixed point:
 * {Y gain, Cr->R, Cb->G, Cr->G, Cb->B}. */
static const int k_yuv[2][5] = {
    {76309, 104597, 25675, 53279, 132201}, /* BT.601 */
    {76309, 117489, 13975, 34925, 138438}, /* BT.709 */
};

static inline uint32_t yuv_argb(const int *k, int y, int u, int v)
{
  const int c = (y - 16) * k[0] + 32768;
  u -= 128;
  v -= 128;
  const int r = clampi((c + k[1] * v) >> 16, 0, 255);
  const int g = clampi((c - k[2] * u - k[3] * v) >> 16, 0, 255);
  const int b = clampi((c + k[4] * u) >> 16, 0, 255);
  return 0xFF000000u | (uint32_t)r << 16 | (uint32_t)g << 8 | (uint32_t)b;
}

void BUI_pixmap_read(const BuiPixmap *img, int y0, int rows, uint32_t *dst)
{
  if (img->format == BUI_PIXMAP_ARGB) {
    memcpy(dst, img->px + (size_t)y0 * img->w, sizeof(uint32_t) * (size_t)img->w * rows);
    return;
  }
  const int *k = k_yuv[img->format == BUI_PIXMAP_NV12_BT709];
  const uint8_t *yp = (const uint8_t *)img->px, *uvp = yp + (size_t)img->w * img->h;
  for (int y = y0; y < y0 + rows; y++) {
    const uint8_t *yr = yp + (size_t)y * img->w, *cr = uvp + (size_t)(y >> 1) * img->w;
    for (int x = 0; x < img->w; x++) {
      const uint8_t *c = cr + (x & ~1);
      *dst++ = yuv_argb(k, yr[x], c[0], c[1]);
    }
  }
}

/* 8-bit bilinear of four samples (fx, fy: 0..256). */
static inline int lerp2(int a, int b, int c, int d, int fx, int fy)
{
  const int top = a * 256 + (b - a) * fx, bot = c * 256 + (d - c) * fx;
  return (top * 256 + (bot - top) * fy + 32768) >> 16;
}

static void raster_nv12(BuiRaster *r, float x, float y, float w, float h, const BuiPixmap *img,
                        float sx, float sy, float sw, float sh, uint32_t ga, int nearest,
                        int x0, int y0, int x1, int y1)
{
  const int *k = k_yuv[img->format == BUI_PIXMAP_NV12_BT709];
  const uint8_t *yp = (const uint8_t *)img->px;
  const uint8_t *uvp = yp + (size_t)img->w * img->h;
  const int iw = img->w;
  const float kx = sw / w, ky = sh / h;
  if (fabsf(kx - 1.0f) < 1e-4f && fabsf(ky - 1.0f) < 1e-4f && x == floorf(x) && y == floorf(y) &&
      sx == floorf(sx) && sy == floorf(sy))
  {
    /* 1:1: one luma sample and its chroma pair per pixel */
    const int ox = (int)sx - (int)x, oy = (int)sy - (int)y;
    for (int py = y0; py < y1; py++) {
      uint32_t *row = r->pix + (size_t)py * r->w;
      const int ty = py + oy;
      const uint8_t *yr = yp + (size_t)ty * iw, *cr = uvp + (size_t)(ty >> 1) * iw;
      if (ga >= 256) {
        /* opaque (every video frame): the chroma terms once per pair, stored */
        for (int px = x0; px < x1; px++) {
          const int tx = px + ox;
          const uint8_t *c = cr + (tx & ~1);
          const int u = c[0] - 128, v = c[1] - 128;
          const int rv = k[1] * v + 32768, gu = 32768 - k[2] * u - k[3] * v, bu = k[4] * u + 32768;
          for (int n = (tx & 1) ? 1 : imin(2, x1 - px); n > 0; n--, px++) {
            const int l = (yr[px + ox] - 16) * k[0];
            row[px] = (uint32_t)clampi((l + rv) >> 16, 0, 255) << 16 |
                      (uint32_t)clampi((l + gu) >> 16, 0, 255) << 8 |
                      (uint32_t)clampi((l + bu) >> 16, 0, 255);
          }
          px--;
        }
        continue;
      }
      for (int px = x0; px < x1; px++) {
        const int tx = px + ox;
        const uint8_t *c = cr + (tx & ~1);
        put_image_px(row + px, yuv_argb(k, yr[tx], c[0], c[1]), ga);
      }
    }
    return;
  }
  /* scaled: bilinear luma, bilinear chroma at half resolution (texel centers
   * at .5, as BUI_raster_image and the shaders) */
  const int ix0 = (int)sx, iy0 = (int)sy;
  const int ix1 = (int)fmaxf((float)ix0, ceilf(sx + sw) - 1.0f);
  const int iy1 = (int)fmaxf((float)iy0, ceilf(sy + sh) - 1.0f);
  const float bias = nearest ? 0.0f : 0.5f;
  for (int py = y0; py < y1; py++) {
    uint32_t *row = r->pix + (size_t)py * r->w;
    const float v = sy + ((float)py + 0.5f - y) * ky;
    const float vy = v - bias, vc = v * 0.5f - 0.5f;
    const int ty = (int)floorf(vy), tc = (int)floorf(vc);
    const int fy = nearest ? 0 : (int)((vy - (float)ty) * 256.0f);
    const int fcy = (int)((vc - (float)tc) * 256.0f);
    const uint8_t *ya = yp + (size_t)clampi(ty, iy0, iy1) * iw;
    const uint8_t *yb = yp + (size_t)clampi(ty + 1, iy0, iy1) * iw;
    const uint8_t *ca = uvp + (size_t)clampi(tc, iy0 >> 1, iy1 >> 1) * iw;
    const uint8_t *cb = uvp + (size_t)clampi(tc + 1, iy0 >> 1, iy1 >> 1) * iw;
    /* texel coordinates in 16.16 fixed point, stepped per pixel (no float
     * floor per pixel: this is the path of every resized video frame) */
    const int64_t du = (int64_t)(kx * 65536.0f);
    int64_t u = (int64_t)((sx + ((float)x0 + 0.5f - x) * kx) * 65536.0f);
    const int64_t ub = (int64_t)(bias * 65536.0f);
    for (int px = x0; px < x1; px++, u += du) {
      const int64_t uy = u - ub, uc = (u >> 1) - 32768;
      const int tx = (int)(uy >> 16), tcx = (int)(uc >> 16);
      const int fx = nearest ? 0 : (int)((uy >> 8) & 255);
      const int fcx = (int)((uc >> 8) & 255);
      const int xa = clampi(tx, ix0, ix1), xb = clampi(tx + 1, ix0, ix1);
      const int ca0 = clampi(tcx, ix0 >> 1, ix1 >> 1) * 2, cb0 = clampi(tcx + 1, ix0 >> 1, ix1 >> 1) * 2;
      const int Y = lerp2(ya[xa], ya[xb], yb[xa], yb[xb], fx, fy);
      const int U = lerp2(ca[ca0], ca[cb0], cb[ca0], cb[cb0], fcx, fcy);
      const int V = lerp2(ca[ca0 + 1], ca[cb0 + 1], cb[ca0 + 1], cb[cb0 + 1], fcx, fcy);
      put_image_px(row + px, yuv_argb(k, Y, U, V), ga);
    }
  }
}

void BUI_raster_image(BuiRaster *r,
                      float x,
                      float y,
                      float w,
                      float h,
                      const BuiPixmap *img,
                      const float src[4],
                      float alpha,
                      int flags)
{
  BUI_raster_image_round(r, x, y, w, h, img, src, alpha, flags, 0.0f);
}

/* Rounded corners of an image drawn into x, y, w, h: the coverage of pixel
 * (px, py) as a fraction of 256 (256 away from the corners). */
typedef struct ImageCorners {
  float x, y, w, h, rad;
} ImageCorners;

static inline uint32_t corner_cov(const ImageCorners *k, int px, int py)
{
  const float fx = (float)px + 0.5f, fy = (float)py + 0.5f;
  const float dx = fx < k->x + k->rad ? k->x + k->rad - fx :
                   fx > k->x + k->w - k->rad ? fx - (k->x + k->w - k->rad) : 0.0f;
  const float dy = fy < k->y + k->rad ? k->y + k->rad - fy :
                   fy > k->y + k->h - k->rad ? fy - (k->y + k->h - k->rad) : 0.0f;
  if (dx <= 0.0f || dy <= 0.0f) {
    return 256u;
  }
  const float cov = clampf(k->rad - sqrtf(dx * dx + dy * dy) + 0.5f, 0.0f, 1.0f);
  return (uint32_t)(cov * 256.0f + 0.5f);
}

/* ga scaled by a corner's coverage at (px, py) (ga as is with no corners). */
static inline uint32_t corner_ga(const ImageCorners *k, int px, int py, uint32_t ga)
{
  return k ? (ga * corner_cov(k, px, py) + 128u) >> 8 : ga;
}

void BUI_raster_image_round(BuiRaster *r,
                            float x,
                            float y,
                            float w,
                            float h,
                            const BuiPixmap *img,
                            const float src[4],
                            float alpha,
                            int flags,
                            float radius)
{
  int x0, y0, x1, y1;
  if (!img || !img->px || img->w <= 0 || img->h <= 0 || !(alpha > 0.0f) ||
      !clip_box(r, x, y, x + w, y + h, &x0, &y0, &x1, &y1))
  {
    return;
  }
  /* the source rectangle, inside the image */
  float sx = 0.0f, sy = 0.0f, sw = (float)img->w, sh = (float)img->h;
  if (src) {
    sx = clampf(src[0], 0.0f, (float)img->w);
    sy = clampf(src[1], 0.0f, (float)img->h);
    sw = clampf(src[2], 0.0f, (float)img->w - sx);
    sh = clampf(src[3], 0.0f, (float)img->h - sy);
    if (!(sw > 0.0f) || !(sh > 0.0f)) {
      return;
    }
  }
  alpha = clampf(alpha, 0.0f, 1.0f);
  if (img->format != BUI_PIXMAP_ARGB) {
    if ((img->w | img->h) & 1) {
      return; /* NV12 is even-sized */
    }
    if (!r->gpu) {
      raster_nv12(r, x, y, w, h, img, sx, sy, sw, sh, (uint32_t)(alpha * 256.0f + 0.5f),
                  (flags & BUI_IMAGE_NEAREST) != 0, x0, y0, x1, y1);
      return;
    }
  }
  if (r->gpu && (img->format != BUI_PIXMAP_ARGB || img->stream || img->w > BUI_IMAGE_ATLAS_MAX ||
                 img->h > BUI_IMAGE_ATLAS_MAX))
  {
    gpu_image_big(r, x, y, w, h, img, sx, sy, sw, sh, alpha, flags, x0, y0, x1, y1);
    return;
  }
  if (r->gpu) {
    int ax, ay;
    if (bui_gpu_image(r->gpu, pixmap_key(img), img->px, img->w, img->h, &ax, &ay)) {
      BuiGpuInst *in = gpu_inst(r, BG_IMAGE, x0, y0, x1, y1);
      if (in) {
        v4(in->rect, x, y, w, h);
        v4(in->uv, (float)ax + sx, (float)ay + sy, sw, sh);
        in->p[1] = alpha;
        in->p[2] = (flags & BUI_IMAGE_NEAREST) ? 1.0f : 0.0f;
      }
    }
    return;
  }
  const uint32_t ga = (uint32_t)(alpha * 256.0f + 0.5f);
  const float kx = sw / w, ky = sh / h;
  const ImageCorners corners = {x, y, w, h, fminf(radius, 0.5f * fminf(w, h))};
  const ImageCorners *k = corners.rad > 0.25f ? &corners : NULL;
  /* 1:1 on whole pixels: a copy with blending */
  if (fabsf(kx - 1.0f) < 1e-4f && fabsf(ky - 1.0f) < 1e-4f && x == floorf(x) && y == floorf(y) &&
      sx == floorf(sx) && sy == floorf(sy))
  {
    for (int py = y0; py < y1; py++) {
      uint32_t *row = r->pix + (size_t)py * r->w;
      const uint32_t *s = img->px + (size_t)(py - (int)y + (int)sy) * img->w + (int)sx - (int)x;
      for (int px = x0; px < x1; px++) {
        put_image_px(row + px, s[px], corner_ga(k, px, py, ga));
      }
    }
    return;
  }
  const int nearest = (flags & BUI_IMAGE_NEAREST) != 0;
  const int ix0 = (int)sx, iy0 = (int)sy;
  const int ix1 = (int)fmaxf((float)ix0, ceilf(sx + sw) - 1.0f);
  const int iy1 = (int)fmaxf((float)iy0, ceilf(sy + sh) - 1.0f);
  for (int py = y0; py < y1; py++) {
    uint32_t *row = r->pix + (size_t)py * r->w;
    /* texel space, centers at .5 */
    const float v = sy + ((float)py + 0.5f - y) * ky - (nearest ? 0.0f : 0.5f);
    int ty = (int)floorf(v);
    const uint32_t fy = nearest ? 0u : (uint32_t)((v - (float)ty) * 256.0f);
    const int ta = clampi(ty, iy0, iy1), tb = clampi(ty + 1, iy0, iy1);
    const uint32_t *ra = img->px + (size_t)ta * img->w, *rb = img->px + (size_t)tb * img->w;
    for (int px = x0; px < x1; px++) {
      const float u = sx + ((float)px + 0.5f - x) * kx - (nearest ? 0.0f : 0.5f);
      const int tx = (int)floorf(u);
      uint32_t c;
      if (nearest) {
        c = ra[clampi(tx, ix0, ix1)];
      }
      else {
        const uint32_t fx = (uint32_t)((u - (float)tx) * 256.0f);
        const int xa = clampi(tx, ix0, ix1), xb = clampi(tx + 1, ix0, ix1);
        c = bilerp(ra[xa], ra[xb], rb[xa], rb[xb], fx, fy);
      }
      put_image_px(row + px, c, corner_ga(k, px, py, ga));
    }
  }
}

void BUI_raster_end_frame_region(BuiRaster *r, int rx, int ry, int rw, int rh)
{
  /* Like end_frame, but only compares tiles touching the region that was
   * redrawn: the rest of the frame is known to be unchanged. */
  if (r->full_damage || rw <= 0 || rh <= 0) {
    BUI_raster_end_frame(r);
    return;
  }
  r->damage_len = 0;
  if (r->gpu) {
    bui_gpu_damage(r->gpu, rx, ry, rw, rh);
    damage_push(r, rx, ry, rw, rh);
    return;
  }
  const int tx0 = clampi(rx, 0, r->w) / TILE_W * TILE_W;
  const int ty0 = clampi(ry, 0, r->h) / TILE_H * TILE_H;
  const int tx1 = clampi(rx + rw, 0, r->w), ty1 = clampi(ry + rh, 0, r->h);
  for (int ty = ty0; ty < ty1; ty += TILE_H) {
    const int th = (ty + TILE_H <= r->h) ? TILE_H : r->h - ty;
    int run_x = -1;
    for (int tx = tx0; tx < tx1 + TILE_W; tx += TILE_W) {
      int dirty = 0;
      if (tx < tx1) {
        const int tw = (tx + TILE_W <= r->w) ? TILE_W : r->w - tx;
        for (int y = ty; y < ty + th; y++) {
          const size_t o = (size_t)y * r->w + tx;
          if (memcmp(r->pix + o, r->prev + o, (size_t)tw * sizeof(uint32_t)) != 0) {
            dirty = 1;
            break;
          }
        }
      }
      if (dirty && run_x < 0) {
        run_x = tx;
      }
      else if (!dirty && run_x >= 0) {
        const int end = tx < r->w ? tx : r->w;
        damage_push(r, run_x, ty, end - run_x, th);
        for (int y = ty; y < ty + th; y++) {
          const size_t o = (size_t)y * r->w + run_x;
          memcpy(r->prev + o, r->pix + o, (size_t)(end - run_x) * sizeof(uint32_t));
        }
        run_x = -1;
      }
    }
  }
}

void BUI_raster_shadow(
    BuiRaster *r, float x, float y, float w, float h, float radius, float size, float alpha)
{
  if (w <= 0.0f || h <= 0.0f || size <= 0.0f || alpha <= 0.0f) {
    return;
  }
  int x0, y0, x1, y1;
  if (!clip_box(r, x - size, y - size * 0.5f, x + w + size, y + h + size * 1.5f, &x0, &y0, &x1,
                &y1))
  {
    return;
  }
  const float hw = w * 0.5f, hh = h * 0.5f;
  const float rad = clampf(radius, 0.0f, fminf(hw, hh));
  if (r->gpu) {
    BuiGpuInst *in = gpu_inst(r, BG_SHADOW, x0, y0, x1, y1);
    if (in) {
      v4(in->rect, x, y, w, h);
      v4(in->p, BG_SHADOW, rad, size, alpha);
    }
    return;
  }
  /* Light comes from above: the shadow sits slightly lower. */
  const float cx = x + hw, cy = y + hh + size * 0.25f;
  const Paint black = {0x000000u, 256};
  for (int py = y0; py < y1; py++) {
    uint32_t *row = r->pix + (size_t)py * r->w;
    const float sy = (float)py + 0.5f;
    for (int px = x0; px < x1; px++) {
      const float sx = (float)px + 0.5f;
      const float d = sd_widget_box(sx, sy, cx, cy, hw, hh, rad, BUI_CORNER_ALL);
      /* Only outside the box itself (the popup covers the inside). */
      const float dbox = sd_widget_box(sx, sy, x + hw, y + hh, hw, hh, rad, BUI_CORNER_ALL);
      if (dbox <= -0.5f || d >= size) {
        continue;
      }
      float t = 1.0f - clampf(d / size, 0.0f, 1.0f);
      t = t * t * (3.0f - 2.0f * t); /* smoothstep falloff */
      put_cov(row + px, black, t * alpha * clampf(0.5f + dbox, 0.0f, 1.0f));
    }
  }
}

#define POLY_SUBSAMPLES 4

static int cmp_float(const void *a, const void *b)
{
  const float fa = *(const float *)a, fb = *(const float *)b;
  return (fa > fb) - (fa < fb);
}

void BUI_raster_polygon(BuiRaster *r, const float (*pts)[2], int n, const float rgba[4])
{
  if (n < 3) {
    return;
  }
  float minx = pts[0][0], maxx = pts[0][0], miny = pts[0][1], maxy = pts[0][1];
  for (int i = 1; i < n; i++) {
    minx = fminf(minx, pts[i][0]);
    maxx = fmaxf(maxx, pts[i][0]);
    miny = fminf(miny, pts[i][1]);
    maxy = fmaxf(maxy, pts[i][1]);
  }
  int x0, y0, x1, y1;
  if (!clip_box(r, minx, miny, maxx, maxy, &x0, &y0, &x1, &y1)) {
    return;
  }
  if (r->gpu) {
    if (rgba[3] <= 0.0f) {
      return;
    }
    /* Exact software coverage (even-odd, anti-aliased) as a mask: white on
     * black in a scratch raster, green channel = coverage. */
    const int mw = x1 - x0, mh = y1 - y0;
    if (!r->scratch) {
      r->scratch = BUI_raster_create(mw, mh);
    }
    BuiRaster *s = r->scratch;
    if (!s) {
      return;
    }
    BUI_raster_begin_frame(s, mw > s->w ? mw : s->w, mh > s->h ? mh : s->h);
    BUI_raster_set_clip(s, 0, 0, mw, mh);
    for (int yy = 0; yy < mh; yy++) {
      memset(s->pix + (size_t)yy * s->w, 0, (size_t)mw * sizeof(uint32_t));
    }
    float(*tp)[2] = (float(*)[2])malloc(sizeof(float[2]) * (size_t)n);
    uint8_t *mask = (uint8_t *)malloc((size_t)mw * mh);
    if (tp && mask) {
      for (int i = 0; i < n; i++) {
        tp[i][0] = pts[i][0] - (float)x0;
        tp[i][1] = pts[i][1] - (float)y0;
      }
      const float white[4] = {1, 1, 1, 1};
      BUI_raster_polygon(s, (const float(*)[2])tp, n, white);
      for (int yy = 0; yy < mh; yy++) {
        const uint32_t *row = s->pix + (size_t)yy * s->w;
        for (int xx = 0; xx < mw; xx++) {
          mask[(size_t)yy * mw + xx] = (uint8_t)((row[xx] >> 8) & 0xFF);
        }
      }
      int ax, ay;
      BuiGpuInst *in = bui_gpu_mask(r->gpu, NULL, 0, mask, mw, mh, &ax, &ay) ?
                           gpu_inst(r, BG_MASK, x0, y0, x1, y1) :
                           NULL;
      if (in) {
        v4(in->rect, (float)x0, (float)y0, (float)mw, (float)mh);
        v4(in->uv, (float)ax, (float)ay, 0.0f, 0.0f);
        c4(in->c0, rgba);
      }
    }
    free(tp);
    free(mask);
    return;
  }
  const Paint p = paint_from(rgba);
  if (p.a <= 0) {
    return;
  }
  const int span = x1 - x0;
  float *cov = (float *)calloc((size_t)span + 1, sizeof(float));
  float *xs = (float *)malloc(sizeof(float) * (size_t)n);
  if (!cov || !xs) {
    free(cov);
    free(xs);
    return;
  }
  const float sub_w = 1.0f / POLY_SUBSAMPLES;
  for (int py = y0; py < y1; py++) {
    memset(cov, 0, sizeof(float) * ((size_t)span + 1));
    int any = 0;
    for (int s = 0; s < POLY_SUBSAMPLES; s++) {
      const float sy = (float)py + ((float)s + 0.5f) * sub_w;
      int nx = 0;
      for (int i = 0, j = n - 1; i < n; j = i++) {
        const float ay = pts[j][1], by = pts[i][1];
        if ((ay <= sy && by > sy) || (by <= sy && ay > sy)) {
          const float t = (sy - ay) / (by - ay);
          xs[nx++] = pts[j][0] + t * (pts[i][0] - pts[j][0]);
        }
      }
      if (nx < 2) {
        continue;
      }
      qsort(xs, (size_t)nx, sizeof(float), cmp_float);
      /* Even-odd spans with fractional horizontal coverage at the ends. */
      for (int k = 0; k + 1 < nx; k += 2) {
        const float a = fmaxf(xs[k], (float)x0), b = fminf(xs[k + 1], (float)x1);
        if (b <= a) {
          continue;
        }
        any = 1;
        const int ia = (int)floorf(a), ib = (int)floorf(b);
        if (ia == ib) {
          cov[ia - x0] += (b - a) * sub_w;
          continue;
        }
        cov[ia - x0] += ((float)ia + 1.0f - a) * sub_w;
        for (int x = ia + 1; x < ib; x++) {
          cov[x - x0] += sub_w;
        }
        if (ib < x1) {
          cov[ib - x0] += (b - (float)ib) * sub_w;
        }
      }
    }
    if (!any) {
      continue;
    }
    uint32_t *row = r->pix + (size_t)py * r->w;
    for (int x = 0; x < span; x++) {
      put_cov(row + x0 + x, p, fminf(cov[x], 1.0f));
    }
  }
  free(cov);
  free(xs);
}

static inline float edge_fn(float ax, float ay, float bx, float by, float px, float py)
{
  return (bx - ax) * (py - ay) - (by - ay) * (px - ax);
}

static inline int is_top_left(float ax, float ay, float bx, float by)
{
  /* For counter-clockwise (in y-down space: positive area) winding. */
  return (ay == by && bx < ax) || (by > ay);
}

void BUI_raster_triangles(BuiRaster *r,
                          const float (*co)[2],
                          const float (*col)[4],
                          int count)
{
  if (r->gpu) {
    const int clip[4] = {r->cx0, r->cy0, r->cx1, r->cy1};
    bui_gpu_triangles(r->gpu, co, col, count, clip);
    return;
  }
  for (int i = 0; i + 2 < count; i += 3) {
    float v[3][2] = {{co[i][0], co[i][1]}, {co[i + 1][0], co[i + 1][1]}, {co[i + 2][0], co[i + 2][1]}};
    const float *c[3] = {col[i], col[i + 1], col[i + 2]};
    float area = edge_fn(v[0][0], v[0][1], v[1][0], v[1][1], v[2][0], v[2][1]);
    if (area == 0.0f || area != area) {
      continue;
    }
    if (area < 0.0f) {
      float tv[2] = {v[1][0], v[1][1]};
      v[1][0] = v[2][0];
      v[1][1] = v[2][1];
      v[2][0] = tv[0];
      v[2][1] = tv[1];
      const float *tc = c[1];
      c[1] = c[2];
      c[2] = tc;
      area = -area;
    }
    int x0, y0, x1, y1;
    if (!clip_box(r, fminf(v[0][0], fminf(v[1][0], v[2][0])), fminf(v[0][1], fminf(v[1][1], v[2][1])),
                  fmaxf(v[0][0], fmaxf(v[1][0], v[2][0])), fmaxf(v[0][1], fmaxf(v[1][1], v[2][1])),
                  &x0, &y0, &x1, &y1))
    {
      continue;
    }
    const int flat = memcmp(c[0], c[1], sizeof(float[4])) == 0 &&
                     memcmp(c[0], c[2], sizeof(float[4])) == 0;
    const Paint flat_p = paint_from(c[0]);
    const int tl0 = is_top_left(v[1][0], v[1][1], v[2][0], v[2][1]);
    const int tl1 = is_top_left(v[2][0], v[2][1], v[0][0], v[0][1]);
    const int tl2 = is_top_left(v[0][0], v[0][1], v[1][0], v[1][1]);
    const float inv_area = 1.0f / area;
    for (int py = y0; py < y1; py++) {
      uint32_t *row = r->pix + (size_t)py * r->w;
      const float sy = (float)py + 0.5f;
      for (int px = x0; px < x1; px++) {
        const float sx = (float)px + 0.5f;
        const float w0 = edge_fn(v[1][0], v[1][1], v[2][0], v[2][1], sx, sy);
        const float w1 = edge_fn(v[2][0], v[2][1], v[0][0], v[0][1], sx, sy);
        const float w2 = edge_fn(v[0][0], v[0][1], v[1][0], v[1][1], sx, sy);
        if ((w0 < 0.0f || (w0 == 0.0f && !tl0)) || (w1 < 0.0f || (w1 == 0.0f && !tl1)) ||
            (w2 < 0.0f || (w2 == 0.0f && !tl2)))
        {
          continue;
        }
        if (flat) {
          put_cov(row + px, flat_p, 1.0f);
        }
        else {
          float rgba[4];
          const float b0 = w0 * inv_area, b1 = w1 * inv_area, b2 = w2 * inv_area;
          for (int k = 0; k < 4; k++) {
            rgba[k] = c[0][k] * b0 + c[1][k] * b1 + c[2][k] * b2;
          }
          put_cov(row + px, paint_from(rgba), 1.0f);
        }
      }
    }
  }
}

void BUI_raster_points(BuiRaster *r,
                       const float (*co)[2],
                       const float (*col)[4],
                       int count,
                       float size)
{
  const float rad = fmaxf(0.5f, size * 0.5f);
  for (int i = 0; i < count; i++) {
    BUI_raster_circle(r, co[i][0], co[i][1], rad, 0.0f, col[i]);
  }
}

/* -------------------------------------------------------------------- */
/** \name Textures + textured meshes (BUI_raster_mesh)
 *
 * The registry is shared by every raster: the software path samples the
 * RGBA8 copy kept here, GPU devices upload it on first use and then only the
 * rectangles logged by later updates.
 * \{ */

#define TEX_LOG 32

typedef struct BuiTexture {
  int id;
  int w, h;
  uint8_t *rgba;
  unsigned gen; /* bumped by every update */
  struct {
    unsigned gen; /* the generation this update produced */
    int rect[4];
  } log[TEX_LOG];
  unsigned nlog; /* updates logged so far (ring index = nlog % TEX_LOG) */
} BuiTexture;

static BuiTexture **g_tex;     /* slot -> texture (NULL: free) */
static unsigned short *g_tex_serial;
static int g_tex_slots;
static unsigned g_tex_free_gen;

static BuiTexture *tex_get(int id)
{
  const int slot = (id & 0xFFFF) - 1;
  if (id <= 0 || slot < 0 || slot >= g_tex_slots) {
    return NULL;
  }
  BuiTexture *t = g_tex[slot];
  return t && t->id == id ? t : NULL;
}

static void tex_copy(BuiTexture *t, int x, int y, int w, int h, const uint8_t *src, int pitch)
{
  /* clip to the texture, moving the source along */
  if (x < 0) {
    src -= x * 4;
    w += x;
    x = 0;
  }
  if (y < 0) {
    src -= (ptrdiff_t)y * pitch;
    h += y;
    y = 0;
  }
  w = w < t->w - x ? w : t->w - x;
  h = h < t->h - y ? h : t->h - y;
  for (int row = 0; row < h; row++) {
    memcpy(t->rgba + ((size_t)(y + row) * t->w + x) * 4, src + (size_t)row * pitch,
           (size_t)w * 4);
  }
}

int BUI_texture_create(int w, int h, const void *rgba, int pitch)
{
  if (w <= 0 || h <= 0 || w > 16384 || h > 16384) {
    return 0;
  }
  BuiTexture *t = (BuiTexture *)calloc(1, sizeof(BuiTexture));
  if (!t || !(t->rgba = (uint8_t *)calloc((size_t)w * h, 4))) {
    free(t);
    return 0;
  }
  t->w = w;
  t->h = h;
  t->gen = 1;
  if (rgba) {
    tex_copy(t, 0, 0, w, h, (const uint8_t *)rgba, pitch > 0 ? pitch : w * 4);
  }
  int slot = 0;
  while (slot < g_tex_slots && g_tex[slot]) {
    slot++;
  }
  if (slot == g_tex_slots) {
    if (slot >= 0xFFFF) {
      free(t->rgba);
      free(t);
      return 0;
    }
    const int cap = g_tex_slots ? g_tex_slots * 2 : 16;
    BuiTexture **nt = (BuiTexture **)realloc(g_tex, sizeof(BuiTexture *) * (size_t)cap);
    unsigned short *ns = nt ? (unsigned short *)realloc(g_tex_serial,
                                                        sizeof(unsigned short) * (size_t)cap) :
                              NULL;
    if (nt) {
      g_tex = nt;
    }
    if (!nt || !ns) {
      free(t->rgba);
      free(t);
      return 0;
    }
    g_tex_serial = ns;
    for (int i = g_tex_slots; i < cap; i++) {
      g_tex[i] = NULL;
      g_tex_serial[i] = 0;
    }
    g_tex_slots = cap;
  }
  g_tex_serial[slot] = (unsigned short)(g_tex_serial[slot] % 0x7FFF + 1);
  t->id = ((int)g_tex_serial[slot] << 16) | (slot + 1);
  g_tex[slot] = t;
  return t->id;
}

int BUI_texture_update(int tex, int x, int y, int w, int h, const void *rgba, int pitch)
{
  BuiTexture *t = tex_get(tex);
  if (!t || !rgba) {
    return 0;
  }
  if (w <= 0 || h <= 0 || x >= t->w || y >= t->h || x + w <= 0 || y + h <= 0) {
    return 1;
  }
  tex_copy(t, x, y, w, h, (const uint8_t *)rgba, pitch > 0 ? pitch : w * 4);
  const int x0 = x > 0 ? x : 0, y0 = y > 0 ? y : 0;
  const int x1 = x + w < t->w ? x + w : t->w, y1 = y + h < t->h ? y + h : t->h;
  t->gen++;
  const unsigned k = t->nlog++ % TEX_LOG;
  t->log[k].gen = t->gen;
  t->log[k].rect[0] = x0;
  t->log[k].rect[1] = y0;
  t->log[k].rect[2] = x1 - x0;
  t->log[k].rect[3] = y1 - y0;
  return 1;
}

void BUI_texture_free(int tex)
{
  BuiTexture *t = tex_get(tex);
  if (!t) {
    return;
  }
  g_tex[(tex & 0xFFFF) - 1] = NULL;
  free(t->rgba);
  free(t);
  g_tex_free_gen++;
}

int BUI_texture_size(int tex, int *r_w, int *r_h)
{
  const BuiTexture *t = tex_get(tex);
  if (r_w) {
    *r_w = t ? t->w : 0;
  }
  if (r_h) {
    *r_h = t ? t->h : 0;
  }
  return t != NULL;
}

int bui_texture_view(int tex, BuiTexView *r_view)
{
  const BuiTexture *t = tex_get(tex);
  if (!t) {
    return 0;
  }
  r_view->w = t->w;
  r_view->h = t->h;
  r_view->rgba = t->rgba;
  r_view->gen = t->gen;
  return 1;
}

int bui_texture_changes(int tex, unsigned since, int (*r_rects)[4], int max)
{
  const BuiTexture *t = tex_get(tex);
  if (!t) {
    return -1;
  }
  const unsigned pending = t->gen - since; /* updates the caller has not seen */
  if (pending == 0) {
    return 0;
  }
  if (pending > (t->nlog < TEX_LOG ? t->nlog : TEX_LOG) || (int)pending > max) {
    return -1;
  }
  for (unsigned i = 0; i < pending; i++) {
    memcpy(r_rects[i], t->log[(t->nlog - pending + i) % TEX_LOG].rect, sizeof(int[4]));
  }
  return (int)pending;
}

unsigned bui_texture_free_gen(void)
{
  return g_tex_free_gen;
}

/* Bilinear (or nearest) sample at texture coordinates (u, v), clamped to the
 * edge, texel centers at +0.5 like the GPU samplers; out is 0..1 RGBA. */
static inline void tex_sample(const BuiTexture *t, float u, float v, int nearest, float out[4])
{
  if (nearest) {
    const int x = clampi((int)floorf(u * t->w), 0, t->w - 1);
    const int y = clampi((int)floorf(v * t->h), 0, t->h - 1);
    const uint8_t *p = t->rgba + ((size_t)y * t->w + x) * 4;
    for (int k = 0; k < 4; k++) {
      out[k] = p[k] * (1.0f / 255.0f);
    }
    return;
  }
  const float fx = u * t->w - 0.5f, fy = v * t->h - 0.5f;
  const float flx = floorf(fx), fly = floorf(fy);
  const float ax = fx - flx, ay = fy - fly;
  const int x0 = clampi((int)flx, 0, t->w - 1), x1 = clampi((int)flx + 1, 0, t->w - 1);
  const int y0 = clampi((int)fly, 0, t->h - 1), y1 = clampi((int)fly + 1, 0, t->h - 1);
  const uint8_t *r0 = t->rgba + (size_t)y0 * t->w * 4, *r1 = t->rgba + (size_t)y1 * t->w * 4;
  const uint8_t *p00 = r0 + x0 * 4, *p10 = r0 + x1 * 4, *p01 = r1 + x0 * 4, *p11 = r1 + x1 * 4;
  const float w00 = (1.0f - ax) * (1.0f - ay), w10 = ax * (1.0f - ay);
  const float w01 = (1.0f - ax) * ay, w11 = ax * ay;
  for (int k = 0; k < 4; k++) {
    out[k] = (p00[k] * w00 + p10[k] * w10 + p01[k] * w01 + p11[k] * w11) * (1.0f / 255.0f);
  }
}

/* Straight-alpha source-over of color c (0..1) into a 0x00RRGGBB pixel. */
static inline void put_straight(uint32_t *px, const float c[4])
{
  const float a = c[3];
  if (!(a > 0.0f)) {
    return;
  }
  Paint p = paint_from(c);
  if (p.a >= 256) {
    *px = p.rgb;
  }
  else if (p.a > 0) {
    *px = blend(*px, p.rgb, (uint32_t)p.a);
  }
}

static inline void col_unpack(uint32_t c, float out[4])
{
  out[0] = (float)(c & 0xFF) * (1.0f / 255.0f);
  out[1] = (float)((c >> 8) & 0xFF) * (1.0f / 255.0f);
  out[2] = (float)((c >> 16) & 0xFF) * (1.0f / 255.0f);
  out[3] = (float)(c >> 24) * (1.0f / 255.0f);
}

/* Vertex coordinate in 1/256 px (D3D's subpixel precision), kept in range. */
static inline int64_t mesh_fx(float v)
{
  v = clampf(v, -1e6f, 1e6f);
  return (int64_t)floorf(v * 256.0f + 0.5f);
}

/* One triangle, pixel centers inside (top-left rule), clipped to
 * [cx0, cx1) x [cy0, cy1). */
static void mesh_triangle(BuiRaster *r,
                          const BuiMeshVertex *va,
                          const BuiMeshVertex *vb,
                          const BuiMeshVertex *vc,
                          float ox,
                          float oy,
                          const BuiTexture *t,
                          int nearest,
                          int cx0,
                          int cy0,
                          int cx1,
                          int cy1)
{
  int64_t X[3] = {mesh_fx(va->x + ox), mesh_fx(vb->x + ox), mesh_fx(vc->x + ox)};
  int64_t Y[3] = {mesh_fx(va->y + oy), mesh_fx(vb->y + oy), mesh_fx(vc->y + oy)};
  const BuiMeshVertex *v[3] = {va, vb, vc};
  int64_t area = (X[1] - X[0]) * (Y[2] - Y[0]) - (Y[1] - Y[0]) * (X[2] - X[0]);
  if (area == 0) {
    return;
  }
  if (area < 0) { /* make it clockwise on screen (positive area, y down) */
    int64_t tx = X[1], ty = Y[1];
    X[1] = X[2];
    Y[1] = Y[2];
    X[2] = tx;
    Y[2] = ty;
    const BuiMeshVertex *tv = v[1];
    v[1] = v[2];
    v[2] = tv;
    area = -area;
  }
  int64_t mnx = X[0], mxx = X[0], mny = Y[0], mxy = Y[0];
  for (int i = 1; i < 3; i++) {
    mnx = X[i] < mnx ? X[i] : mnx;
    mxx = X[i] > mxx ? X[i] : mxx;
    mny = Y[i] < mny ? Y[i] : mny;
    mxy = Y[i] > mxy ? Y[i] : mxy;
  }
  /* pixels whose centers (px * 256 + 128) can be inside */
  int x0 = (int)((mnx - 128 + 255) >> 8), x1 = (int)((mxx - 128) >> 8) + 1;
  int y0 = (int)((mny - 128 + 255) >> 8), y1 = (int)((mxy - 128) >> 8) + 1;
  x0 = x0 > cx0 ? x0 : cx0;
  y0 = y0 > cy0 ? y0 : cy0;
  x1 = x1 < cx1 ? x1 : cx1;
  y1 = y1 < cy1 ? y1 : cy1;
  if (x0 >= x1 || y0 >= y1) {
    return;
  }
  /* edge k runs from vertex k+1 to k+2 (opposite vertex k): E > 0 inside.
   * Top-left rule: an edge owns its pixels when it is a top edge (horizontal,
   * running right) or a left edge (running up). */
  int64_t A[3], B[3], E0[3];
  int bias[3];
  const int64_t px0 = (int64_t)x0 * 256 + 128, py0 = (int64_t)y0 * 256 + 128;
  for (int k = 0; k < 3; k++) {
    const int a = (k + 1) % 3, b = (k + 2) % 3;
    const int64_t dx = X[b] - X[a], dy = Y[b] - Y[a];
    A[k] = -dy * 256; /* per pixel step in x */
    B[k] = dx * 256;  /* per pixel step in y */
    E0[k] = dx * (py0 - Y[a]) - dy * (px0 - X[a]);
    const int top_left = (dy == 0 && dx > 0) || dy < 0;
    bias[k] = top_left ? 0 : -1;
  }
  float col[3][4];
  for (int i = 0; i < 3; i++) {
    col_unpack(v[i]->col, col[i]);
  }
  const int flat_col = v[0]->col == v[1]->col && v[0]->col == v[2]->col;
  const int flat_uv = !t || (v[0]->u == v[1]->u && v[0]->u == v[2]->u && v[0]->v == v[1]->v &&
                             v[0]->v == v[2]->v);
  const double inv = 1.0 / (double)area;
  float tflat[4] = {1.0f, 1.0f, 1.0f, 1.0f}; /* the texel when uv is constant */
  if (flat_uv && t) {
    tex_sample(t, v[0]->u, v[0]->v, nearest, tflat);
  }
  float flat[4];
  for (int k = 0; k < 4; k++) {
    flat[k] = col[0][k] * tflat[k];
  }
  const int flat_all = flat_col && flat_uv;
  if (flat_all && !(flat[3] > 0.0f)) {
    return;
  }
  for (int py = y0; py < y1; py++) {
    int64_t e0 = E0[0] + B[0] * (py - y0), e1 = E0[1] + B[1] * (py - y0),
            e2 = E0[2] + B[2] * (py - y0);
    uint32_t *row = r->pix + (size_t)py * r->w;
    for (int px = x0; px < x1; px++, e0 += A[0], e1 += A[1], e2 += A[2]) {
      if (e0 + bias[0] < 0 || e1 + bias[1] < 0 || e2 + bias[2] < 0) {
        continue;
      }
      if (flat_all) {
        put_straight(row + px, flat);
        continue;
      }
      /* barycentric weights of vertices 0, 1, 2 */
      const float l0 = (float)((double)e0 * inv), l1 = (float)((double)e1 * inv);
      const float l2 = 1.0f - l0 - l1;
      float c[4];
      if (flat_col) {
        memcpy(c, col[0], sizeof(c));
      }
      else {
        for (int k = 0; k < 4; k++) {
          c[k] = col[0][k] * l0 + col[1][k] * l1 + col[2][k] * l2;
        }
      }
      if (flat_uv) {
        for (int k = 0; k < 4; k++) {
          c[k] *= tflat[k];
        }
      }
      else {
        float tx[4];
        tex_sample(t, v[0]->u * l0 + v[1]->u * l1 + v[2]->u * l2,
                   v[0]->v * l0 + v[1]->v * l1 + v[2]->v * l2, nearest, tx);
        for (int k = 0; k < 4; k++) {
          c[k] *= tx[k];
        }
      }
      put_straight(row + px, c);
    }
  }
}

void BUI_raster_mesh(BuiRaster *r,
                     const BuiMeshVertex *vtx,
                     int n_vtx,
                     const void *idx,
                     int n_idx,
                     int flags,
                     int tex,
                     const float clip[4],
                     float ox,
                     float oy)
{
  if (!vtx || !idx || n_vtx <= 0 || n_idx < 3) {
    return;
  }
  int cx0 = r->cx0, cy0 = r->cy0, cx1 = r->cx1, cy1 = r->cy1;
  if (clip) { /* a scissor: whole pixels, truncated like ImGui's backends do */
    const float fx0 = clampf(clip[0] + ox, -1e6f, 1e6f), fy0 = clampf(clip[1] + oy, -1e6f, 1e6f);
    const float fx1 = clampf(clip[2] + ox, -1e6f, 1e6f), fy1 = clampf(clip[3] + oy, -1e6f, 1e6f);
    cx0 = (int)fx0 > cx0 ? (int)fx0 : cx0;
    cy0 = (int)fy0 > cy0 ? (int)fy0 : cy0;
    cx1 = (int)fx1 < cx1 ? (int)fx1 : cx1;
    cy1 = (int)fy1 < cy1 ? (int)fy1 : cy1;
  }
  if (cx0 >= cx1 || cy0 >= cy1) {
    return;
  }
  const int idx32 = (flags & BUI_MESH_INDEX32) != 0, nearest = (flags & BUI_MESH_NEAREST) != 0;
  if (r->gpu) {
    const int box[4] = {cx0, cy0, cx1, cy1};
    bui_gpu_mesh(r->gpu, vtx, n_vtx, idx, idx32, n_idx, ox, oy, tex, nearest, box);
    return;
  }
  const BuiTexture *t = tex ? tex_get(tex) : NULL;
  const uint16_t *i16 = (const uint16_t *)idx;
  const uint32_t *i32 = (const uint32_t *)idx;
  for (int i = 0; i + 2 < n_idx; i += 3) {
    const uint32_t a = idx32 ? i32[i] : i16[i], b = idx32 ? i32[i + 1] : i16[i + 1],
                   c = idx32 ? i32[i + 2] : i16[i + 2];
    if (a >= (uint32_t)n_vtx || b >= (uint32_t)n_vtx || c >= (uint32_t)n_vtx) {
      continue;
    }
    mesh_triangle(r, &vtx[a], &vtx[b], &vtx[c], ox, oy, t, nearest, cx0, cy0, cx1, cy1);
  }
}

/** \} */

/* -------------------------------------------------------------------- */
/** \name Color widgets (HSV wheel, gradients, lit sphere)
 * \{ */

static void hsv_to_rgb_f(float h, float s, float v, float out[3])
{
  h = h - floorf(h);
  const float h6 = h * 6.0f;
  const float r = clampf(fabsf(h6 - 3.0f) - 1.0f, 0.0f, 1.0f);
  const float g = clampf(2.0f - fabsf(h6 - 2.0f), 0.0f, 1.0f);
  const float b = clampf(2.0f - fabsf(h6 - 4.0f), 0.0f, 1.0f);
  out[0] = ((r - 1.0f) * s + 1.0f) * v;
  out[1] = ((g - 1.0f) * s + 1.0f) * v;
  out[2] = ((b - 1.0f) * s + 1.0f) * v;
}

void BUI_raster_hsv_circle(BuiRaster *r, float cx, float cy, float radius, float value)
{
  int x0, y0, x1, y1;
  if (radius <= 0.0f ||
      !clip_box(r, cx - radius - 1, cy - radius - 1, cx + radius + 1, cy + radius + 1, &x0, &y0,
                &x1, &y1))
  {
    return;
  }
  if (r->gpu) {
    BuiGpuInst *in = gpu_inst(r, BG_HSV, x0, y0, x1, y1);
    if (in) {
      v4(in->rect, cx, cy, radius, value);
    }
    return;
  }
  const float inv_2pi = 0.15915494309189535f;
  for (int py = y0; py < y1; py++) {
    uint32_t *row = r->pix + (size_t)py * r->w;
    const float dy = (float)py + 0.5f - cy;
    for (int px = x0; px < x1; px++) {
      const float dx = (float)px + 0.5f - cx;
      const float dist = sqrtf(dx * dx + dy * dy);
      const float cov = clampf(radius - dist + 0.5f, 0.0f, 1.0f);
      if (cov <= 0.0f) {
        continue;
      }
      /* Blender's wheel (ui_hsvcircle_vals_from_pos): hue from the angle
       * measured from "up", red at the bottom; saturation = distance. */
      const float hue = atan2f(dx, -dy) * inv_2pi + 0.5f;
      float rgba[4] = {0, 0, 0, 1.0f};
      hsv_to_rgb_f(hue, fminf(dist / radius, 1.0f), value, rgba);
      put_cov(row + px, paint_from(rgba), cov);
    }
  }
}

void BUI_raster_gradient(BuiRaster *r,
                         float x,
                         float y,
                         float w,
                         float h,
                         const float tl[4],
                         const float tr[4],
                         const float br[4],
                         const float bl[4])
{
  int x0, y0, x1, y1;
  if (!clip_box(r, x, y, x + w, y + h, &x0, &y0, &x1, &y1)) {
    return;
  }
  if (r->gpu) {
    BuiGpuInst *in = gpu_inst(r, BG_GRADIENT, x0, y0, x1, y1);
    if (in) {
      v4(in->rect, x, y, w, h);
      c4(in->c0, tl);
      c4(in->c1, tr);
      c4(in->c2, br);
      c4(in->c3, bl);
    }
    return;
  }
  for (int py = y0; py < y1; py++) {
    uint32_t *row = r->pix + (size_t)py * r->w;
    const float v = clampf(((float)py + 0.5f - y) / h, 0.0f, 1.0f);
    float left[4], right[4];
    for (int k = 0; k < 4; k++) {
      left[k] = tl[k] + (bl[k] - tl[k]) * v;
      right[k] = tr[k] + (br[k] - tr[k]) * v;
    }
    for (int px = x0; px < x1; px++) {
      const float u = clampf(((float)px + 0.5f - x) / w, 0.0f, 1.0f);
      float c[4];
      for (int k = 0; k < 4; k++) {
        c[k] = left[k] + (right[k] - left[k]) * u;
      }
      put_cov(row + px, paint_from(c), 1.0f);
    }
  }
}

void BUI_raster_sphere(
    BuiRaster *r, float cx, float cy, float radius, const float light[3], const float rgba[4])
{
  int x0, y0, x1, y1;
  if (radius <= 0.0f ||
      !clip_box(r, cx - radius - 1, cy - radius - 1, cx + radius + 1, cy + radius + 1, &x0, &y0,
                &x1, &y1))
  {
    return;
  }
  float L[3] = {light[0], light[1], light[2]};
  const float len = sqrtf(L[0] * L[0] + L[1] * L[1] + L[2] * L[2]);
  if (len > 0.0f) {
    L[0] /= len;
    L[1] /= len;
    L[2] /= len;
  }
  else {
    L[2] = 1.0f;
  }
  /* Half vector for a viewer looking down -z. */
  float H[3] = {L[0], L[1], L[2] + 1.0f};
  const float hl = sqrtf(H[0] * H[0] + H[1] * H[1] + H[2] * H[2]);
  if (hl > 0.0f) {
    H[0] /= hl;
    H[1] /= hl;
    H[2] /= hl;
  }
  if (r->gpu) {
    BuiGpuInst *in = gpu_inst(r, BG_SPHERE, x0, y0, x1, y1);
    if (in) {
      v4(in->rect, cx, cy, radius, 0.0f);
      c4(in->c0, rgba);
      v4(in->c1, L[0], L[1], L[2], 0.0f);
      v4(in->c2, H[0], H[1], H[2], 0.0f);
    }
    return;
  }
  for (int py = y0; py < y1; py++) {
    uint32_t *row = r->pix + (size_t)py * r->w;
    const float ny = -((float)py + 0.5f - cy) / radius; /* up is +y */
    for (int px = x0; px < x1; px++) {
      const float nx = ((float)px + 0.5f - cx) / radius;
      const float d2 = nx * nx + ny * ny;
      const float cov = clampf((1.0f - sqrtf(d2)) * radius + 0.5f, 0.0f, 1.0f);
      if (cov <= 0.0f) {
        continue;
      }
      const float nz = sqrtf(fmaxf(0.0f, 1.0f - d2));
      const float diff = fmaxf(0.0f, nx * L[0] + ny * L[1] + nz * L[2]);
      float spec = fmaxf(0.0f, nx * H[0] + ny * H[1] + nz * H[2]);
      spec = spec * spec;
      spec = spec * spec;
      spec = spec * spec * spec * spec; /* ^16 */
      float c[4];
      for (int k = 0; k < 3; k++) {
        c[k] = rgba[k] * (0.12f + 0.88f * diff) + 0.6f * spec;
      }
      c[3] = rgba[3];
      put_cov(row + px, paint_from(c), cov);
    }
  }
}

/** \} */

void BUI_raster_set_font(BuiRaster *r, const char *family, int flags)
{
  if (!family) {
    family = "";
  }
  strncpy(r->font_family, family, sizeof(r->font_family) - 1);
  r->font_family[sizeof(r->font_family) - 1] = '\0';
  r->font_flags = flags;
}

/** \} */

/* -------------------------------------------------------------------- */
/** \name Text: glyph-run cache over a font backend
 *
 * One cache for every platform: a run is a (font slot, UTF-8 string) pair
 * with its advance and, once drawn, an 8-bit coverage mask. Two backends
 * fill slots and masks:
 *  - GDI (Windows, with BUI_TEXT_GDI): the system's hinted grayscale AA.
 *  - Portable (stb_truetype, the default everywhere): Inter is embedded as
 *    the default face; others come from BUI_text_add_font(), the BUI_FONT /
 *    BUI_FONT_DIR environment variables, or the platform's font folders.
 * \{ */

#ifdef _WIN32
#  ifndef WIN32_LEAN_AND_MEAN
#    define WIN32_LEAN_AND_MEAN
#  endif
#  include <windows.h>
#endif

#if defined(_WIN32) && defined(BUI_TEXT_GDI)
#  define TEXT_GDI 1
#else
#  define TEXT_GDI 0
#endif

typedef struct FontSlot {
  char family[64];
  int px;
  int flags;
  int ascent;
  int height;
#if TEXT_GDI
  HFONT font;
#else
  int face;
  float scale;     /* font units -> pixels */
  int synth;       /* BUI_FONT_BOLD / ITALIC flags the face lacks */
  float embolden;  /* synthetic bold: extra pixels per glyph */
  void *ft_size;   /* FreeType: this slot's FT_Size */
  float opsz;      /* FreeType: optical size (Blender: the text size) */
  int fallback;    /* the emoji face at this size, for glyphs the face lacks
                    * (Blender's BLF fallback fonts): -2 not tried, -1 none */
#endif
} FontSlot;

#define MAX_FONTS 48
static FontSlot g_fonts[MAX_FONTS];
static int g_font_count = 0;

/* TextRun.gid: the glyph is the slot's fallback face's (FontSlot.fallback) */
#define GID_FALLBACK (1u << 31)

/* One layer of a colour (COLR) glyph: an outline glyph of its face in one colour. */
typedef struct GlyphLayer {
  int glyph;
  float rgba[4];
} GlyphLayer;
#define GLYPH_LAYERS_MAX 256

typedef struct TextRun {
  uint64_t hash;
  int font;
  int len;
  char *text;
  float advance;
  /* mask, built lazily on first draw */
  int built;
  int color;          /* has colour glyphs: they're drawn after the mask, not in it */
  int mw, mh, ox, oy; /* ox / oy: left padding / baseline row baked into the mask */
  uint8_t *mask;
  /* glyphs (portable engine; for GPU text and colour glyphs): ids and pen
   * x, built lazily */
  int ng; /* -1: not built yet */
  uint32_t *gid; /* glyph index | GID_FALLBACK */
  float *gx;
} TextRun;

#define RUN_CAP 8192 /* power of two */
#define RUN_MAX_FILL 6000
static TextRun *g_runs = NULL;
static int g_run_count = 0;
static unsigned g_text_gen = 1; /* bumped whenever runs or fonts are freed */

/* backend */
static int be_font_open(FontSlot *s, const char *family, int flags, int px);
static void be_font_close(FontSlot *s);
static float be_measure(const FontSlot *s, const char *utf8, int len);
static void be_build(TextRun *e, const FontSlot *s);
static void be_flush_glyphs(void);

static void runs_flush(void)
{
  g_text_gen++;
  if (!g_runs) {
    return;
  }
  for (int i = 0; i < RUN_CAP; i++) {
    free(g_runs[i].text);
    free(g_runs[i].mask);
    free(g_runs[i].gid);
    free(g_runs[i].gx);
  }
  memset(g_runs, 0, sizeof(TextRun) * RUN_CAP);
  g_run_count = 0;
}

static void fonts_flush(void)
{
  g_text_gen++;
  for (int i = 0; i < g_font_count; i++) {
    be_font_close(&g_fonts[i]);
  }
  g_font_count = 0;
  be_flush_glyphs();
  runs_flush(); /* runs reference font slots */
}

/* The open slot for family + flags at `px` pixels, or -1. */
static int font_find(const char *family, int flags, int px)
{
  for (int i = 0; i < g_font_count; i++) {
    if (g_fonts[i].px == px && g_fonts[i].flags == flags &&
        strcmp(g_fonts[i].family, family) == 0)
    {
      return i;
    }
  }
  return -1;
}

/* Opens a new slot (the table must have room), -1 when the font can't open. */
static int font_new(const char *family, int flags, int px)
{
  FontSlot *s = &g_fonts[g_font_count];
  memset(s, 0, sizeof(*s));
  strncpy(s->family, family, sizeof(s->family) - 1);
  s->px = px;
  s->flags = flags;
  if (!be_font_open(s, family, flags, px)) {
    return -1;
  }
  return g_font_count++;
}

static int font_get(const char *family, int flags, float size)
{
  if (!family || !family[0] || strcmp(family, "TkDefaultFont") == 0) {
    family = "";
  }
  int px = (int)(size + 0.5f);
  if (px < 1) {
    px = 1;
  }
  if (px > 512) {
    px = 512;
  }
  const int i = font_find(family, flags, px);
  if (i >= 0) {
    return i;
  }
  if (g_font_count == MAX_FONTS) {
    fonts_flush();
  }
  return font_new(family, flags, px);
}

static uint64_t run_hash(int font, const char *s, int len)
{
  uint64_t h = 1469598103934665603ull ^ (uint64_t)(font * 2654435761u);
  for (int i = 0; i < len; i++) {
    h ^= (uint8_t)s[i];
    h *= 1099511628211ull;
  }
  return h ? h : 1;
}

static TextRun *run_get(int font, const char *s, int len)
{
  if (!g_runs) {
    g_runs = (TextRun *)calloc(RUN_CAP, sizeof(TextRun));
    if (!g_runs) {
      return NULL;
    }
  }
  const uint64_t h = run_hash(font, s, len);
  int idx = (int)(h & (RUN_CAP - 1));
  for (;;) {
    TextRun *e = &g_runs[idx];
    if (e->hash == 0) {
      break;
    }
    if (e->hash == h && e->font == font && e->len == len && memcmp(e->text, s, (size_t)len) == 0) {
      return e;
    }
    idx = (idx + 1) & (RUN_CAP - 1);
  }
  if (g_run_count >= RUN_MAX_FILL) {
    runs_flush();
    return run_get(font, s, len);
  }
  TextRun *e = &g_runs[idx];
  e->text = (char *)malloc((size_t)len + 1);
  if (!e->text) {
    return NULL;
  }
  memcpy(e->text, s, (size_t)len);
  e->text[len] = 0;
  e->hash = h;
  e->font = font;
  e->len = len;
  e->ng = -1;
  g_run_count++;
  e->advance = be_measure(&g_fonts[font], s, len);
  return e;
}

int BUI_text_native(void)
{
  /* the portable engine reports 1 once a face can be found */
  return font_get("", 0, 12.0f) >= 0;
}

int BUI_text_measure(
    const char *family, int flags, float size, const char *utf8, int len, float *r_width)
{
  *r_width = 0.0f;
  if (len <= 0) {
    return 1;
  }
  const int f = font_get(family, flags, size);
  if (f < 0) {
    return 0;
  }
  TextRun *e = run_get(f, utf8, len);
  if (!e) {
    return 0;
  }
  *r_width = e->advance;
  return 1;
}

unsigned BUI_text_generation(void)
{
  return g_text_gen;
}

const BuiTextRun *BUI_text_prepare(
    const char *family, int flags, float size, const char *utf8, int len, float *r_width)
{
  if (r_width) {
    *r_width = 0.0f;
  }
  if (len <= 0) {
    return NULL;
  }
  const int f = font_get(family, flags, size);
  if (f < 0) {
    return NULL;
  }
  TextRun *e = run_get(f, utf8, len);
  if (!e) {
    return NULL;
  }
  if (r_width) {
    *r_width = e->advance;
  }
  if (!e->built) {
    e->built = 1;
    if (e->advance > 0.0f) {
      be_build(e, &g_fonts[e->font]);
    }
  }
  return e->mask || e->color ? (const BuiTextRun *)e : NULL; /* NULL: whitespace-only */
}

int BUI_raster_text(BuiRaster *r,
                    float x,
                    float y,
                    const char *utf8,
                    int len,
                    float size,
                    const float rgba[4])
{
  if (len <= 0) {
    return 1;
  }
  if (font_get(r->font_family, r->font_flags, size) < 0) {
    return 0;
  }
  const BuiTextRun *run = BUI_text_prepare(r->font_family, r->font_flags, size, utf8, len, NULL);
  if (run) {
    BUI_raster_text_run(r, x, y, run, rgba);
  }
  return 1;
}

static int gpu_text_glyphs(BuiRaster *r, TextRun *e, float x, float y, const float rgba[4]);

static int run_glyphs(TextRun *e);
static int glyph_draw_info(const TextRun *e, int i, int *r_face, float *r_scale, float *r_embolden,
                           float *r_shear);
static const void *glyph_bitmap(const TextRun *e, int i, int glyph, float pen, int *r_x, int *r_y,
                                int *r_w, int *r_h, const uint8_t **r_bits);
static int glyph_layers(const TextRun *e, int i, const float fg[4], GlyphLayer *r_layers, int max);

/* Blends an 8-bit coverage mask at (dx, dy) in one colour. */
static void mask_blend(BuiRaster *r, const uint8_t *mask, int mw, int mh, int dx, int dy, Paint p)
{
  const int x0 = clampi(dx, r->cx0, r->cx1), x1 = clampi(dx + mw, r->cx0, r->cx1);
  const int y0 = clampi(dy, r->cy0, r->cy1), y1 = clampi(dy + mh, r->cy0, r->cy1);
  for (int py = y0; py < y1; py++) {
    uint32_t *row = r->pix + (size_t)py * r->w;
    const uint8_t *m = mask + (size_t)(py - dy) * mw - dx;
    for (int px = x0; px < x1; px++) {
      const uint32_t cov = m[px];
      if (cov == 0) {
        continue;
      }
      const uint32_t fac = ((uint32_t)p.a * (cov + (cov >> 7))) >> 8;
      if (fac >= 256) {
        row[px] = p.rgb;
      }
      else if (fac > 0) {
        row[px] = blend(row[px], p.rgb, fac);
      }
    }
  }
}

/* Software raster of a run's colour glyphs (kept out of its mask): each
 * layer's coverage bitmap in the layer's colour, at the glyph's pen. */
static void raster_color_glyphs(BuiRaster *r, TextRun *e, float x, float y, const float rgba[4])
{
  if (!run_glyphs(e)) {
    return;
  }
  const int ox = (int)floorf(x + 0.5f), base = (int)floorf(y + 0.5f);
  GlyphLayer layers[GLYPH_LAYERS_MAX];
  for (int i = 0; i < e->ng; i++) {
    const int n = glyph_layers(e, i, rgba, layers, GLYPH_LAYERS_MAX);
    for (int k = 0; k < n && k < GLYPH_LAYERS_MAX; k++) {
      int gx, gy, gw, gh;
      const uint8_t *bits;
      if (glyph_bitmap(e, i, layers[k].glyph, e->gx[i], &gx, &gy, &gw, &gh, &bits)) {
        mask_blend(r, bits, gw, gh, ox + gx, base + gy, paint_from(layers[k].rgba));
      }
    }
  }
}

void BUI_raster_text_run(BuiRaster *r, float x, float y, const BuiTextRun *run, const float rgba[4])
{
  const TextRun *e = (const TextRun *)run;
  const Paint p = paint_from(rgba);
  if (!e || !(e->mask || e->color) || p.a <= 0) {
    return;
  }
  if (r->gpu && gpu_text_glyphs(r, (TextRun *)e, x, y, rgba)) {
    return;
  }
  const int dx = (int)floorf(x + 0.5f) - e->ox;
  const int dy = (int)floorf(y + 0.5f) - e->oy;
  if (r->gpu) {
    const int x0 = clampi(dx, r->cx0, r->cx1), x1 = clampi(dx + e->mw, r->cx0, r->cx1);
    const int y0 = clampi(dy, r->cy0, r->cy1), y1 = clampi(dy + e->mh, r->cy0, r->cy1);
    int ax, ay;
    BuiGpuInst *in = e->mask && x1 > x0 && y1 > y0 &&
                             bui_gpu_mask(r->gpu, e, g_text_gen, e->mask, e->mw, e->mh, &ax, &ay) ?
                         gpu_inst(r, BG_MASK, x0, y0, x1, y1) :
                         NULL;
    if (in) {
      v4(in->rect, (float)dx, (float)dy, (float)e->mw, (float)e->mh);
      v4(in->uv, (float)ax, (float)ay, 0.0f, 0.0f);
      c4(in->c0, rgba);
    }
    return;
  }
  if (e->mask) {
    mask_blend(r, e->mask, e->mw, e->mh, dx, dy, p);
  }
  if (e->color) {
    raster_color_glyphs(r, (TextRun *)e, x, y, rgba);
  }
}

/* GPU text, one instance per glyph (per layer of a colour glyph): Slug
 * (outline curves evaluated in the pixel shader) when the renderer supports
 * it, else glyph bitmaps from the shared glyph cache packed into the GPU
 * atlas (Blender's BLF approach). Returns 0 to fall back to the run's mask
 * (no glyph data). */
static int gpu_text_glyphs(BuiRaster *r, TextRun *e, float x, float y, const float rgba[4])
{
  if (!run_glyphs(e) || e->ng == 0) {
    return e->ng == 0;
  }
  const FontSlot *fs = &g_fonts[e->font];
  const float ox = floorf(x + 0.5f), base = floorf(y + 0.5f);
  const int slug = BUI_gpu_slug() && bui_gpu_slug_on(r->gpu);
  GlyphLayer layers[GLYPH_LAYERS_MAX];
  for (int i = 0; i < e->ng; i++) {
    int face;
    float scale, embolden, shear;
    if (!glyph_draw_info(e, i, &face, &scale, &embolden, &shear)) {
      return 0;
    }
    const int passes = embolden > 0.0f ? 1 + (int)(embolden + 0.5f) : 1; /* synthetic bold */
    const float pen = ox + e->gx[i];
    int n = glyph_layers(e, i, rgba, layers, GLYPH_LAYERS_MAX);
    if (n == 0) { /* a plain glyph: one layer, the text colour */
      layers[0].glyph = (int)(e->gid[i] & ~GID_FALLBACK);
      c4(layers[0].rgba, rgba);
      n = 1;
    }
    for (int l = 0; l < n && l < GLYPH_LAYERS_MAX; l++) {
      const int glyph = layers[l].glyph;
      const float *color = layers[l].rgba;
      if (slug) {
        float bbox[4], band[4];
        int loc[4];
        if (!bui_gpu_slug_glyph(r->gpu, face, glyph, 0 /* faces are never freed */, bbox, band, loc)) {
          continue; /* empty glyph (space) */
        }
        /* screen box of the glyph (font units, y up -> pixels, y down), plus shear and AA */
        const float top = base - bbox[3] * scale, bottom = base - bbox[1] * scale;
        const float lean = shear * (bbox[3] > 0.0f ? bbox[3] : 0.0f) * scale;
        for (int k = 0; k < passes; k++) {
          const float gx = pen + (float)k;
          int x0, y0, x1, y1;
          if (!clip_box(r, gx + bbox[0] * scale - 1.0f, top - 1.0f, gx + bbox[2] * scale + lean + 1.0f,
                        bottom + 1.0f, &x0, &y0, &x1, &y1)) {
            continue;
          }
          BuiGpuInst *in = gpu_inst(r, BG_SLUG, x0, y0, x1, y1);
          if (in) {
            v4(in->rect, gx, base, scale, shear);
            c4(in->c0, color);
            v4(in->c1, band[0], band[1], band[2], band[3]);
            v4(in->uv, (float)loc[0], (float)loc[1], (float)loc[2], (float)loc[3]);
          }
        }
      }
      else {
        int gx, gy, gw, gh;
        const uint8_t *bits;
        const void *key = glyph_bitmap(e, i, glyph, e->gx[i], &gx, &gy, &gw, &gh, &bits);
        int ax, ay;
        if (!key || gw <= 0 || gh <= 0 || !bui_gpu_mask(r->gpu, key, g_text_gen, bits, gw, gh, &ax, &ay)) {
          continue;
        }
        for (int k = 0; k < passes; k++) {
          const int px = (int)ox + gx + k, py = (int)base + gy;
          int x0, y0, x1, y1;
          if (!clip_box(r, (float)px, (float)py, (float)(px + gw), (float)(py + gh), &x0, &y0, &x1, &y1)) {
            continue;
          }
          BuiGpuInst *in = gpu_inst(r, BG_MASK, x0, y0, x1, y1);
          if (in) {
            v4(in->rect, (float)px, (float)py, (float)gw, (float)gh);
            v4(in->uv, (float)ax, (float)ay, 0.0f, 0.0f);
            c4(in->c0, color);
          }
        }
      }
    }
  }
  if (fs->flags & BUI_FONT_UNDERLINE) {
    const float t = fs->px / 14 > 1 ? (float)(fs->px / 14) : 1.0f;
    const float uy = base + (float)((fs->height - fs->ascent) / 2);
    BUI_raster_fill_rect(r, ox, uy, e->advance, t, rgba);
  }
  return 1;
}

/* Text runs for GPU renderers: mask size, left padding and ascent. */
const uint8_t *BUI_text_run_mask(const BuiTextRun *run, int *r_w, int *r_h, int *r_ox, int *r_ascent)
{
  const TextRun *e = (const TextRun *)run;
  if (!e || !e->mask) {
    return NULL;
  }
  *r_w = e->mw;
  *r_h = e->mh;
  *r_ox = e->ox;
  *r_ascent = e->oy;
  return e->mask;
}

/** \} */

#if TEXT_GDI
/* -------------------------------------------------------------------- */
/** \name Text backend: GDI
 * \{ */

#  ifndef BUI_NO_INTER
#    include "../extern/font_inter.h"
const void *BUI_text_default_font(int *r_len)
{
  if (r_len) {
    *r_len = (int)sizeof(g_font_inter_variable);
  }
  return g_font_inter_variable;
}
#  else
const void *BUI_text_default_font(int *r_len)
{
  if (r_len) {
    *r_len = 0;
  }
  return NULL;
}
#  endif

static HDC g_dc = NULL;

static HDC text_dc(void)
{
  if (!g_dc) {
    g_dc = CreateCompatibleDC(NULL);
    SetBkMode(g_dc, TRANSPARENT);
    SetTextAlign(g_dc, TA_LEFT | TA_TOP | TA_NOUPDATECP);
  }
  return g_dc;
}

static int be_font_open(FontSlot *s, const char *family, int flags, int px)
{
  if (!family[0]) {
    family = "Segoe UI";
  }
  wchar_t wfam[64];
  if (MultiByteToWideChar(CP_UTF8, 0, family, -1, wfam, 64) == 0) {
    wcscpy(wfam, L"Segoe UI");
  }
  HFONT font = CreateFontW(-px, 0, 0, 0, (flags & BUI_FONT_BOLD) ? FW_BOLD : FW_NORMAL,
                           (flags & BUI_FONT_ITALIC) ? TRUE : FALSE,
                           (flags & BUI_FONT_UNDERLINE) ? TRUE : FALSE, FALSE, DEFAULT_CHARSET,
                           OUT_TT_PRECIS, CLIP_DEFAULT_PRECIS, ANTIALIASED_QUALITY,
                           DEFAULT_PITCH | FF_DONTCARE, wfam);
  if (!font) {
    return 0;
  }
  s->font = font;
  HDC dc = text_dc();
  HGDIOBJ old = SelectObject(dc, font);
  TEXTMETRICW tm;
  GetTextMetricsW(dc, &tm);
  SelectObject(dc, old);
  s->ascent = tm.tmAscent;
  s->height = tm.tmHeight;
  return 1;
}

static void be_font_close(FontSlot *s)
{
  DeleteObject(s->font);
}

static void be_flush_glyphs(void) {}

static int run_glyphs(TextRun *e)
{
  (void)e;
  return 0;
}
static int glyph_draw_info(const TextRun *e, int i, int *r_face, float *r_scale, float *r_embolden,
                           float *r_shear)
{
  (void)e;
  (void)i;
  (void)r_face;
  (void)r_scale;
  (void)r_embolden;
  (void)r_shear;
  return 0;
}
static const void *glyph_bitmap(const TextRun *e, int i, int glyph, float pen, int *r_x, int *r_y,
                                int *r_w, int *r_h, const uint8_t **r_bits)
{
  (void)e;
  (void)i;
  (void)glyph;
  (void)pen;
  (void)r_x;
  (void)r_y;
  (void)r_w;
  (void)r_h;
  (void)r_bits;
  return NULL;
}
static int glyph_layers(const TextRun *e, int i, const float fg[4], GlyphLayer *r_layers, int max)
{
  (void)e;
  (void)i;
  (void)fg;
  (void)r_layers;
  (void)max;
  return 0;
}
int bui_text_glyph_outline(int face, int glyph, float **r_curves, int *r_n, float r_bbox[4])
{
  (void)face;
  (void)glyph;
  (void)r_bbox;
  *r_curves = NULL;
  *r_n = 0;
  return 0;
}

static wchar_t *to_wide(const char *s, int len, int *r_n)
{
  int n = MultiByteToWideChar(CP_UTF8, 0, s, len, NULL, 0);
  wchar_t *w = (wchar_t *)malloc(sizeof(wchar_t) * (size_t)(n + 1));
  if (!w) {
    *r_n = 0;
    return NULL;
  }
  MultiByteToWideChar(CP_UTF8, 0, s, len, w, n);
  w[n] = 0;
  *r_n = n;
  return w;
}

static float be_measure(const FontSlot *s, const char *utf8, int len)
{
  int wn;
  wchar_t *w = to_wide(utf8, len, &wn);
  HDC dc = text_dc();
  HGDIOBJ old = SelectObject(dc, s->font);
  SIZE sz = {0, 0};
  if (w && wn > 0) {
    GetTextExtentPoint32W(dc, w, wn, &sz);
  }
  SelectObject(dc, old);
  free(w);
  return (float)sz.cx;
}

static void be_build(TextRun *e, const FontSlot *fs)
{
  int wn;
  wchar_t *w = to_wide(e->text, e->len, &wn);
  if (!w || wn == 0) {
    free(w);
    return;
  }
  const int pad = 2 + fs->px / 4; /* bearings / italic overhang */
  const int mw = (int)e->advance + pad * 2;
  const int mh = fs->height + 1;
  BITMAPINFO bmi;
  memset(&bmi, 0, sizeof(bmi));
  bmi.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
  bmi.bmiHeader.biWidth = mw;
  bmi.bmiHeader.biHeight = -mh;
  bmi.bmiHeader.biPlanes = 1;
  bmi.bmiHeader.biBitCount = 32;
  bmi.bmiHeader.biCompression = BI_RGB;
  void *bits = NULL;
  HDC dc = text_dc();
  HBITMAP bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &bits, NULL, 0);
  if (!bmp || !bits) {
    free(w);
    return;
  }
  memset(bits, 0, (size_t)mw * mh * 4);
  HGDIOBJ old_bmp = SelectObject(dc, bmp);
  HGDIOBJ old_font = SelectObject(dc, fs->font);
  SetTextColor(dc, RGB(255, 255, 255));
  TextOutW(dc, pad, 0, w, wn);
  GdiFlush();
  e->mask = (uint8_t *)malloc((size_t)mw * mh);
  if (e->mask) {
    const uint32_t *src = (const uint32_t *)bits;
    for (int i = 0; i < mw * mh; i++) {
      /* grayscale AA: channels match; take green (perceptual) */
      e->mask[i] = (uint8_t)((src[i] >> 8) & 0xFF);
    }
    e->mw = mw;
    e->mh = mh;
    e->ox = pad;
    e->oy = fs->ascent;
  }
  SelectObject(dc, old_font);
  SelectObject(dc, old_bmp);
  DeleteObject(bmp);
  free(w);
}

int BUI_text_add_font(const char *family, int flags, const void *data, int len)
{
  (void)flags;
  /* private to this process; addressable by the font's own family name */
  DWORD n = 0;
  HANDLE h = AddFontMemResourceEx((void *)data, (DWORD)len, NULL, &n);
  (void)family;
  if (h && n > 0) {
    fonts_flush();
    return 1;
  }
  return 0;
}

/** \} */

#else /* portable */
/* -------------------------------------------------------------------- */
/** \name Text backend: stb_truetype
 * \{ */

#  if defined(__GNUC__) || defined(__clang__)
#    pragma GCC diagnostic push
#    pragma GCC diagnostic ignored "-Wunused-function"
#    pragma GCC diagnostic ignored "-Wsign-compare"
#    pragma GCC diagnostic ignored "-Wconversion"
#    pragma GCC diagnostic ignored "-Wshadow"
#    pragma GCC diagnostic ignored "-Wcast-qual"
#  endif
#  define STBTT_STATIC
#  define STB_TRUETYPE_IMPLEMENTATION
#  include "../extern/stb_truetype.h"
#  if defined(__GNUC__) || defined(__clang__)
#    pragma GCC diagnostic pop
#  endif
#  include <stdio.h>

/* FreeType (BUI_TEXT_FT, the default): glyphs are loaded, hinted and
 * rasterized the way Blender's BLF does (blenfont/intern/blf_glyph.cc):
 * light hinting, the variable Inter at the text's optical size, gamma 1.43.
 * stb_truetype still reads cmaps, metrics and kern tables. */
#  ifdef BUI_TEXT_FT
#    define TEXT_FT 1
#    include <ft2build.h>
#    include FT_FREETYPE_H
#    include FT_MULTIPLE_MASTERS_H
#    include FT_OUTLINE_H
#    include FT_SIZES_H
#    include FT_ADVANCES_H
#    include FT_COLOR_H
static FT_Library g_ft = NULL;
#  else
#    define TEXT_FT 0
#  endif

typedef struct Face {
  char key[96]; /* file path, or "mem:<family>" */
  unsigned char *data;
  stbtt_fontinfo info;
  int ascent, descent, line_gap; /* font units */
#  if TEXT_FT
  FT_Face ft;
  FT_MM_Var *mm;            /* variation axes (NULL: not a variable font) */
  int ax_wght, ax_opsz;     /* axis indices, -1 when absent */
  float wght;               /* this face's weight on the wght axis */
  float set_wght, set_opsz; /* the design coordinates last set */
  FT_Color *palette;        /* colour (COLR / CPAL) glyphs: palette 0, else NULL */
  unsigned ncolors;
#  endif
} Face;

#  define MAX_FACES 32
static Face g_faces[MAX_FACES];
static int g_face_count = 0;

/* registered (in-memory) fonts: family + style -> face */
typedef struct FaceReg {
  char family[64];
  int style; /* BUI_FONT_BOLD | BUI_FONT_ITALIC */
  int face;
} FaceReg;
static FaceReg g_regs[MAX_FACES];
static int g_reg_count = 0;

static int ieq(const char *a, const char *b)
{
  for (; *a && *b; a++, b++) {
    int ca = (unsigned char)*a, cb = (unsigned char)*b;
    if (ca >= 'A' && ca <= 'Z') {
      ca += 32;
    }
    if (cb >= 'A' && cb <= 'Z') {
      cb += 32;
    }
    if (ca != cb) {
      return 0;
    }
  }
  return *a == *b;
}

#  if TEXT_FT
/* Index of the variation axis `tag` (e.g. 'wght'), or -1. */
static int ft_axis(const FT_MM_Var *mm, FT_ULong tag)
{
  for (FT_UInt i = 0; mm && i < mm->num_axis; i++) {
    if (mm->axis[i].tag == tag) {
      return (int)i;
    }
  }
  return -1;
}

static void ft_face_open(Face *f, const unsigned char *data, long len)
{
  f->ft = NULL;
  f->mm = NULL;
  f->ax_wght = f->ax_opsz = -1;
  f->wght = 400.0f;
  f->set_wght = f->set_opsz = -1.0f;
  if (!g_ft && FT_Init_FreeType(&g_ft) != 0) {
    g_ft = NULL;
    return;
  }
  if (FT_New_Memory_Face(g_ft, data, (FT_Long)len, 0, &f->ft) != 0) {
    f->ft = NULL;
    return;
  }
  if (FT_HAS_MULTIPLE_MASTERS(f->ft) && FT_Get_MM_Var(f->ft, &f->mm) == 0) {
    f->ax_wght = ft_axis(f->mm, FT_MAKE_TAG('w', 'g', 'h', 't'));
    f->ax_opsz = ft_axis(f->mm, FT_MAKE_TAG('o', 'p', 's', 'z'));
    if (f->ax_wght >= 0) {
      f->wght = (float)f->mm->axis[f->ax_wght].def / 65536.0f;
    }
  }
  /* colour fonts (COLR v0 layers, e.g. Segoe UI Emoji): the first palette */
  FT_Palette_Data pal;
  f->palette = NULL;
  f->ncolors = 0;
  if (FT_Palette_Data_Get(f->ft, &pal) == 0 && pal.num_palettes > 0 &&
      FT_Palette_Select(f->ft, 0, &f->palette) == 0 && f->palette)
  {
    f->ncolors = pal.num_palette_entries;
  }
}

/* Make `s` the active size of its face and set the variation axes it needs:
 * the face's weight, and the optical size as Blender sets it
 * (blf_glyph_set_variation_optical_size: the size, clamped to the axis). */
static FT_Face ft_use(const FontSlot *s)
{
  Face *f = &g_faces[s->face];
  if (!f->ft || !s->ft_size) {
    return NULL;
  }
  FT_Activate_Size((FT_Size)s->ft_size);
  if (f->mm && (f->set_wght != f->wght || f->set_opsz != s->opsz)) {
    FT_Fixed coords[16];
    const FT_UInt n = f->mm->num_axis < 16 ? f->mm->num_axis : 16;
    if (FT_Get_Var_Design_Coordinates(f->ft, n, coords) == 0) {
      if (f->ax_wght >= 0 && f->ax_wght < (int)n) {
        coords[f->ax_wght] = (FT_Fixed)(f->wght * 65536.0f);
      }
      if (f->ax_opsz >= 0 && f->ax_opsz < (int)n) {
        const FT_Var_Axis *a = &f->mm->axis[f->ax_opsz];
        FT_Fixed v = (FT_Fixed)(s->opsz * 65536.0f);
        v = v < a->minimum ? a->minimum : v > a->maximum ? a->maximum : v;
        coords[f->ax_opsz] = v;
      }
      FT_Set_Var_Design_Coordinates(f->ft, n, coords);
    }
    f->set_wght = f->wght;
    f->set_opsz = s->opsz;
  }
  return f->ft;
}
#  endif

static int face_init(Face *f, unsigned char *data, long len)
{
  const int off = stbtt_GetFontOffsetForIndex(data, 0);
  if (off < 0 || !stbtt_InitFont(&f->info, data, off)) {
    return 0;
  }
#  if TEXT_FT
  ft_face_open(f, data, len);
#  else
  (void)len;
#  endif
  f->data = data;
  /* GDI's metrics: the OS/2 table's Windows ascent/descent when present */
  int a, d, g;
  if (stbtt_GetFontVMetricsOS2(&f->info, &a, &d, &g)) {
    int wa = 0, wd = 0;
    const stbtt_uint32 os2 = stbtt__find_table(f->info.data, (stbtt_uint32)f->info.fontstart, "OS/2");
    if (os2) {
      wa = ttUSHORT(f->info.data + os2 + 74);
      wd = ttUSHORT(f->info.data + os2 + 76);
    }
    if (wa > 0 || wd > 0) {
      a = wa;
      d = -wd;
    }
  }
  else {
    stbtt_GetFontVMetrics(&f->info, &a, &d, &g);
  }
  f->ascent = a;
  f->descent = d;
  f->line_gap = g;
  return 1;
}

static int face_load_file(const char *path)
{
  for (int i = 0; i < g_face_count; i++) {
    if (strcmp(g_faces[i].key, path) == 0) {
      return i;
    }
  }
  if (g_face_count == MAX_FACES || strlen(path) >= sizeof(g_faces[0].key)) {
    return -1;
  }
  FILE *fp = fopen(path, "rb");
  if (!fp) {
    return -1;
  }
  fseek(fp, 0, SEEK_END);
  const long n = ftell(fp);
  fseek(fp, 0, SEEK_SET);
  unsigned char *data = n > 0 ? (unsigned char *)malloc((size_t)n) : NULL;
  if (!data || fread(data, 1, (size_t)n, fp) != (size_t)n) {
    free(data);
    fclose(fp);
    return -1;
  }
  fclose(fp);
  Face *f = &g_faces[g_face_count];
  memset(f, 0, sizeof(*f));
  if (!face_init(f, data, n)) {
    free(data);
    return -1;
  }
  strcpy(f->key, path);
  return g_face_count++;
}

#  ifndef BUI_NO_INTER
#    include "../extern/font_inter.h"

/* Inter, embedded: the default UI face (Blender's), so text looks the same
 * on every platform. The variable font, as Blender ships it: with FreeType,
 * regular and bold are its weights 400 and 700 (BLF_BOLD: +300); without,
 * bold is synthesized. `bold`: 0 regular, 1 bold. -1 when it can't load. */
static int face_inter(int bold)
{
  static int ids[2] = {-2, -2};
  if (ids[bold] == -2) {
    ids[bold] = -1;
    if (g_face_count < MAX_FACES) {
      Face *f = &g_faces[g_face_count];
      memset(f, 0, sizeof(*f));
      /* stb_truetype and FreeType only read the data */
      unsigned char *data = (unsigned char *)g_font_inter_variable;
      if (face_init(f, data, (long)sizeof(g_font_inter_variable))) {
#    if TEXT_FT
        f->wght = bold ? 700.0f : 400.0f;
#    endif
        strcpy(f->key, bold ? "embed:inter-bold" : "embed:inter");
        ids[bold] = g_face_count++;
      }
    }
  }
  return ids[bold];
}

const void *BUI_text_default_font(int *r_len)
{
  if (r_len) {
    *r_len = (int)sizeof(g_font_inter_variable);
  }
  return g_font_inter_variable;
}
#  else
const void *BUI_text_default_font(int *r_len)
{
  if (r_len) {
    *r_len = 0;
  }
  return NULL;
}
#  endif

int BUI_text_add_font(const char *family, int flags, const void *data, int len)
{
  if (!data || len <= 0 || g_face_count == MAX_FACES || g_reg_count == MAX_FACES) {
    return 0;
  }
  unsigned char *copy = (unsigned char *)malloc((size_t)len);
  if (!copy) {
    return 0;
  }
  memcpy(copy, data, (size_t)len);
  Face *f = &g_faces[g_face_count];
  memset(f, 0, sizeof(*f));
  if (!face_init(f, copy, len)) {
    free(copy);
    return 0;
  }
  snprintf(f->key, sizeof(f->key), "mem:%s", family ? family : "");
  FaceReg *r = &g_regs[g_reg_count++];
  strncpy(r->family, family ? family : "", sizeof(r->family) - 1);
  r->family[sizeof(r->family) - 1] = '\0';
  r->style = flags & (BUI_FONT_BOLD | BUI_FONT_ITALIC);
  r->face = g_face_count++;
  fonts_flush(); /* the new face may replace a fallback */
  return 1;
}

/* Font folders, most specific first. */
static int font_dirs(char dirs[][260], int max)
{
  int n = 0;
  const char *env = getenv("BUI_FONT_DIR");
  if (env && env[0] && n < max) {
    snprintf(dirs[n++], 260, "%s", env);
  }
#  ifdef _WIN32
  const char *win = getenv("WINDIR");
  snprintf(dirs[n++], 260, "%s/Fonts", win ? win : "C:/Windows");
  const char *local = getenv("LOCALAPPDATA");
  if (local && n < max) {
    snprintf(dirs[n++], 260, "%s/Microsoft/Windows/Fonts", local);
  }
#  elif defined(__APPLE__)
  static const char *mac[] = {"/System/Library/Fonts", "/System/Library/Fonts/Supplemental",
                              "/Library/Fonts"};
  for (int i = 0; i < 3 && n < max; i++) {
    snprintf(dirs[n++], 260, "%s", mac[i]);
  }
  const char *home = getenv("HOME");
  if (home && n < max) {
    snprintf(dirs[n++], 260, "%s/Library/Fonts", home);
  }
#  else
  static const char *nix[] = {
      "/usr/share/fonts/truetype/dejavu", "/usr/share/fonts/dejavu",
      "/usr/share/fonts/TTF",             "/usr/share/fonts/truetype/liberation",
      "/usr/share/fonts/liberation",      "/usr/share/fonts/truetype/noto",
      "/usr/share/fonts/noto",            "/usr/share/fonts/google-noto",
      "/usr/share/fonts/truetype",        "/usr/local/share/fonts",
      "/usr/share/fonts"};
  for (int i = 0; i < (int)(sizeof(nix) / sizeof(nix[0])) && n < max; i++) {
    snprintf(dirs[n++], 260, "%s", nix[i]);
  }
  const char *home = getenv("HOME");
  if (home && n + 1 < max) {
    snprintf(dirs[n++], 260, "%s/.local/share/fonts", home);
    snprintf(dirs[n++], 260, "%s/.fonts", home);
  }
#  endif
  return n;
}

/* File names per generic family and style (regular, bold, italic, both). */
typedef struct FamilyFiles {
  const char *names; /* '|'-separated aliases */
  const char *files[4];
} FamilyFiles;

static const FamilyFiles g_families[] = {
    {"|sans|sans-serif|inter|segoe ui|system-ui|helvetica|arial|dejavu sans|liberation sans|noto "
     "sans|tkdefaultfont|tktextfont|tkheadingfont|tkcaptionfont|tkmenufont|",
     {"segoeui.ttf|DejaVuSans.ttf|LiberationSans-Regular.ttf|NotoSans-Regular.ttf|Arial.ttf|"
      "arial.ttf|Helvetica.ttc|SFNS.ttf",
      "segoeuib.ttf|DejaVuSans-Bold.ttf|LiberationSans-Bold.ttf|NotoSans-Bold.ttf|Arial "
      "Bold.ttf|arialbd.ttf",
      "segoeuii.ttf|DejaVuSans-Oblique.ttf|LiberationSans-Italic.ttf|NotoSans-Italic.ttf|Arial "
      "Italic.ttf|ariali.ttf",
      "segoeuiz.ttf|DejaVuSans-BoldOblique.ttf|LiberationSans-BoldItalic.ttf|NotoSans-"
      "BoldItalic.ttf|Arial Bold Italic.ttf|arialbi.ttf"}},
    {"|mono|monospace|courier|courier new|consolas|menlo|dejavu sans mono|liberation "
     "mono|tkfixedfont|",
     {"consola.ttf|DejaVuSansMono.ttf|LiberationMono-Regular.ttf|NotoSansMono-Regular.ttf|"
      "Menlo.ttc|cour.ttf",
      "consolab.ttf|DejaVuSansMono-Bold.ttf|LiberationMono-Bold.ttf|NotoSansMono-Bold.ttf|"
      "courbd.ttf",
      "consolai.ttf|DejaVuSansMono-Oblique.ttf|LiberationMono-Italic.ttf|couri.ttf",
      "consolaz.ttf|DejaVuSansMono-BoldOblique.ttf|LiberationMono-BoldItalic.ttf|courbi.ttf"}},
    {"|serif|times|times new roman|georgia|dejavu serif|liberation serif|",
     {"times.ttf|DejaVuSerif.ttf|LiberationSerif-Regular.ttf|NotoSerif-Regular.ttf|Times New "
      "Roman.ttf|Times.ttc",
      "timesbd.ttf|DejaVuSerif-Bold.ttf|LiberationSerif-Bold.ttf|NotoSerif-Bold.ttf|Times New "
      "Roman Bold.ttf",
      "timesi.ttf|DejaVuSerif-Italic.ttf|LiberationSerif-Italic.ttf|NotoSerif-Italic.ttf|Times "
      "New Roman Italic.ttf",
      "timesbi.ttf|DejaVuSerif-BoldItalic.ttf|LiberationSerif-BoldItalic.ttf|NotoSerif-"
      "BoldItalic.ttf|Times New Roman Bold Italic.ttf"}},
    /* the per-glyph fallback for symbols the UI face lacks (FontSlot.fallback):
     * outline emoji fonts, colour (COLR v0) or not; bitmap-only ones (CBDT,
     * sbix: Noto Color Emoji, Apple Color Emoji) can't load */
    {"|emoji|segoe ui emoji|twemoji|twemoji mozilla|noto emoji|openmoji|",
     {"seguiemj.ttf|TwemojiMozilla.ttf|Twemoji.ttf|NotoEmoji-Regular.ttf|OpenMoji-Black.ttf", "",
      "", ""}},
};
#  define FAMILY_EMOJI 3

static int family_index(const char *family)
{
  if (!family[0]) {
    return 0;
  }
  char probe[72];
  snprintf(probe, sizeof(probe), "|%s|", family);
  for (int i = 0; probe[i]; i++) {
    if (probe[i] >= 'A' && probe[i] <= 'Z') {
      probe[i] = (char)(probe[i] + 32);
    }
  }
  for (int i = 0; i < (int)(sizeof(g_families) / sizeof(g_families[0])); i++) {
    if (strstr(g_families[i].names, probe)) {
      return i;
    }
  }
  return -1;
}

/* The default face (empty, generic sans or Tk's named fonts), as opposed to
 * a named system family. */
static int family_generic(const char *family)
{
  static const char *names =
      "|sans|sans-serif|inter|system-ui|ui-sans-serif|tkdefaultfont|tktextfont|tkheadingfont|"
      "tkcaptionfont|tkmenufont|tksmallcaptionfont|tkiconfont|tktooltipfont|";
  char probe[72];
  if (!family[0]) {
    return 1;
  }
  snprintf(probe, sizeof(probe), "|%s|", family);
  for (int i = 0; probe[i]; i++) {
    if (probe[i] >= 'A' && probe[i] <= 'Z') {
      probe[i] = (char)(probe[i] + 32);
    }
  }
  return strstr(names, probe) != NULL;
}

/* First '|'-separated file found in any font folder. */
static int face_from_files(const char *files, char dirs[][260], int ndirs)
{
  const char *p = files;
  while (*p) {
    const char *end = strchr(p, '|');
    const int n = end ? (int)(end - p) : (int)strlen(p);
    for (int d = 0; d < ndirs; d++) {
      char path[600];
      snprintf(path, sizeof(path), "%s/%.*s", dirs[d], n, p);
      const int f = face_load_file(path);
      if (f >= 0) {
        return f;
      }
    }
    p += n + (end ? 1 : 0);
  }
  return -1;
}

/* A registered (BUI_text_add_font) face of the family: the exact style,
 * else any style of the family; -1 when none. */
static int face_registered(const char *family, int style, int *r_have)
{
  for (int pass = 0; pass < 2; pass++) {
    for (int i = 0; i < g_reg_count; i++) {
#    ifdef BUI_NO_INTER
      const int default_face = !family[0] && i == 0; /* no embedded UI font */
#    else
      const int default_face = 0;
#    endif
      if ((ieq(g_regs[i].family, family) || default_face) &&
          (pass == 1 || g_regs[i].style == style))
      {
        *r_have = g_regs[i].style;
        return g_regs[i].face;
      }
    }
  }
  return -1;
}

/* The face for family + style; *r_have gets the style bits it really has. */
static int face_find(const char *family, int style, int *r_have)
{
  /* 1. registered fonts */
  const int reg = face_registered(family, style, r_have);
  if (reg >= 0) {
    return reg;
  }
  /* 2. an explicit file */
  const size_t fl = strlen(family);
  if (fl > 4 && (ieq(family + fl - 4, ".ttf") || ieq(family + fl - 4, ".otf") ||
                 ieq(family + fl - 4, ".ttc")))
  {
    const int f = face_load_file(family);
    if (f >= 0) {
      *r_have = 0;
      return f;
    }
  }
  char dirs[16][260];
  const int nd = font_dirs(dirs, 16);
  /* 3. BUI_FONT overrides the default face */
  const char *env = (!family[0] || family_index(family) == 0) ? getenv("BUI_FONT") : NULL;
  if (env && env[0]) {
    const int f = face_load_file(env);
    if (f >= 0) {
      *r_have = 0;
      return f;
    }
  }
  /* 4. a known generic family, then "<family>.ttf", then the default */
  int fam = family_index(family);
  int generic = family_generic(family);
  if (fam < 0) {
    char files[200];
    char squashed[64];
    int k = 0;
    for (const char *c = family; *c && k < 63; c++) {
      if (*c != ' ') {
        squashed[k++] = *c;
      }
    }
    squashed[k] = 0;
    snprintf(files, sizeof(files), "%s.ttf|%s.ttf|%s.otf|%s-Regular.ttf", family, squashed,
             family, squashed);
    const int f = face_from_files(files, dirs, nd);
    if (f >= 0) {
      *r_have = 0;
      return f;
    }
    fam = 0;
    generic = 1; /* unknown family: the default face */
  }
  const int want = (style & BUI_FONT_BOLD ? 1 : 0) | (style & BUI_FONT_ITALIC ? 2 : 0);
#  ifdef BUI_NO_INTER
  (void)generic;
#  else
  if (fam == 0 && generic) { /* Inter (italic synthesized, like Blender) */
    const int f = face_inter(want & 1);
    if (f >= 0) {
      /* FreeType instances the bold weight; stb_truetype can't (synthesized) */
      *r_have = TEXT_FT && g_faces[f].ft ? (style & BUI_FONT_BOLD) : 0;
      return f;
    }
  }
#  endif
  int f = face_from_files(g_families[fam].files[want], dirs, nd);
  if (f >= 0) {
    *r_have = style;
    return f;
  }
  if (want) { /* synthesize the style from the regular face */
    f = face_from_files(g_families[fam].files[0], dirs, nd);
    if (f >= 0) {
      *r_have = 0;
      return f;
    }
  }
  if (fam != 0) {
    return face_find("", style, r_have);
  }
  if (g_reg_count > 0) {
    *r_have = g_regs[0].style;
    return g_regs[0].face;
  }
  return -1;
}

static int be_font_open(FontSlot *s, const char *family, int flags, int px)
{
  const int style = flags & (BUI_FONT_BOLD | BUI_FONT_ITALIC);
  int have = 0;
  const int face = face_find(family, style, &have);
  if (face < 0) {
    return 0;
  }
  const Face *f = &g_faces[face];
  s->face = face;
  s->scale = stbtt_ScaleForMappingEmToPixels(&f->info, (float)px); /* GDI: -px = em height */
  s->ascent = (int)ceilf((float)f->ascent * s->scale - 0.01f);
  s->height = s->ascent + (int)ceilf((float)-f->descent * s->scale - 0.01f);
  s->synth = style & ~have;
  s->embolden = (s->synth & BUI_FONT_BOLD) ? (px < 24 ? 1.0f : (float)px / 24.0f) : 0.0f;
  s->ft_size = NULL;
  s->opsz = (float)px;
  s->fallback = -2;
#  if TEXT_FT
  FT_Size size;
  if (f->ft && FT_New_Size(f->ft, &size) == 0) {
    FT_Activate_Size(size);
    if (FT_Set_Pixel_Sizes(f->ft, 0, (FT_UInt)px) == 0) {
      s->ft_size = size;
    }
    else {
      FT_Done_Size(size);
    }
  }
#  endif
  return 1;
}

/* The emoji face: registered as "emoji", else a known file; -1 when there
 * is none (unlike face_find("emoji"), which would settle for the default). */
static int face_emoji(void)
{
  int have;
  const int reg = face_registered("emoji", 0, &have);
  if (reg >= 0) {
    return reg;
  }
  char dirs[16][260];
  const int nd = font_dirs(dirs, 16);
  return face_from_files(g_families[FAMILY_EMOJI].files[0], dirs, nd);
}

/* The slot's fallback: the emoji face at its pixel size, opened on first
 * use (its own glyph bitmaps, FT_Size and advances). NULL when none. */
static const FontSlot *slot_fallback(const FontSlot *s)
{
  FontSlot *w = &g_fonts[s - g_fonts]; /* the table entry: slots are const to walks */
  if (w->fallback == -2) {
    w->fallback = -1;
    const int face = face_emoji();
    if (face >= 0 && face != w->face) {
      int fb = font_find("emoji", 0, w->px);
      if (fb < 0 && g_font_count < MAX_FONTS) { /* no room: no fallback (never flush under a walk) */
        fb = font_new("emoji", 0, w->px);
      }
      w->fallback = fb;
      if (fb >= 0) {
        g_fonts[fb].fallback = -1;
      }
    }
  }
  return w->fallback >= 0 ? &g_fonts[w->fallback] : NULL;
}

static void be_font_close(FontSlot *s)
{
#  if TEXT_FT
  if (s->ft_size) {
    FT_Done_Size((FT_Size)s->ft_size);
    s->ft_size = NULL;
  }
#  else
  (void)s;
#  endif
}

/* UTF-8 decode; invalid bytes map to U+FFFD. */
static int utf8_next(const char *s, int len, int *i)
{
  const unsigned char c = (unsigned char)s[*i];
  int cp, n;
  if (c < 0x80) {
    cp = c;
    n = 1;
  }
  else if ((c & 0xE0) == 0xC0) {
    cp = c & 0x1F;
    n = 2;
  }
  else if ((c & 0xF0) == 0xE0) {
    cp = c & 0x0F;
    n = 3;
  }
  else if ((c & 0xF8) == 0xF0) {
    cp = c & 0x07;
    n = 4;
  }
  else {
    (*i)++;
    return 0xFFFD;
  }
  if (*i + n > len) {
    *i = len;
    return 0xFFFD;
  }
  for (int k = 1; k < n; k++) {
    const unsigned char cc = (unsigned char)s[*i + k];
    if ((cc & 0xC0) != 0x80) {
      *i += k;
      return 0xFFFD;
    }
    cp = (cp << 6) | (cc & 0x3F);
  }
  *i += n;
  return cp;
}

/* Glyph bitmaps by (slot, glyph, quarter-pixel phase). Rasterized once,
 * composited into every run that uses them. */
typedef struct GlyphBmp {
  uint32_t key; /* 0 = empty */
  int16_t w, h, xoff, yoff;
  uint8_t *bits;
} GlyphBmp;

#  define GLYPH_CAP 4096 /* power of two */
#  define GLYPH_MAX_FILL 3000
static GlyphBmp *g_glyphs = NULL;
static int g_glyph_count = 0;

static void be_flush_glyphs(void)
{
  g_text_gen++; /* glyph bitmaps are GPU atlas keys */
  if (!g_glyphs) {
    return;
  }
  for (int i = 0; i < GLYPH_CAP; i++) {
    free(g_glyphs[i].bits);
  }
  memset(g_glyphs, 0, sizeof(GlyphBmp) * GLYPH_CAP);
  g_glyph_count = 0;
}

static const GlyphBmp *glyph_get(int slot, const FontSlot *s, int glyph, int phase)
{
  if (!g_glyphs) {
    g_glyphs = (GlyphBmp *)calloc(GLYPH_CAP, sizeof(GlyphBmp));
    if (!g_glyphs) {
      return NULL;
    }
  }
  const uint32_t key = (((uint32_t)slot + 1) << 24) ^ ((uint32_t)glyph << 2) ^ (uint32_t)phase;
  uint32_t idx = (key * 2654435761u) >> 20 & (GLYPH_CAP - 1);
  for (;;) {
    GlyphBmp *g = &g_glyphs[idx];
    if (g->key == 0) {
      break;
    }
    if (g->key == key) {
      return g;
    }
    idx = (idx + 1) & (GLYPH_CAP - 1);
  }
  if (g_glyph_count >= GLYPH_MAX_FILL) {
    be_flush_glyphs();
    return glyph_get(slot, s, glyph, phase);
  }
  GlyphBmp *g = &g_glyphs[idx];
  const stbtt_fontinfo *info = &g_faces[s->face].info;
  int w = 0, h = 0, xo = 0, yo = 0;
#  if TEXT_FT
  FT_Face ft = ft_use(s);
  if (ft) {
    g->bits = NULL;
    /* blf_glyph_load / blf_glyph_render: light hinting, smooth light render */
    if (FT_Load_Glyph(ft, (FT_UInt)glyph, FT_LOAD_NO_BITMAP | FT_LOAD_TARGET_LIGHT) == 0 &&
        ft->glyph->format == FT_GLYPH_FORMAT_OUTLINE)
    {
      FT_Outline *o = &ft->glyph->outline;
      if (s->embolden > 0.0f) {
        FT_Outline_EmboldenXY(o, (FT_Pos)(s->embolden * 64.0f), 0);
      }
      FT_Outline_Translate(o, (FT_Pos)phase * 16, 0); /* quarter-pixel position */
      if (FT_Render_Glyph(ft->glyph, FT_RENDER_MODE_LIGHT) == 0) {
        const FT_Bitmap *b = &ft->glyph->bitmap;
        w = (int)b->width;
        h = (int)b->rows;
        xo = ft->glyph->bitmap_left;
        yo = -ft->glyph->bitmap_top;
        if (w > 0 && h > 0 && b->pixel_mode == FT_PIXEL_MODE_GRAY) {
          g->bits = (uint8_t *)malloc((size_t)w * (size_t)h);
          if (g->bits) {
            /* Blender's blf_glyph_gamma: powf(c / 256, 1 / 1.43) * 256 */
            static const uint8_t gamma[256] = {
                0,   5,   9,   11,  14,  16,  19,  21,  23,  25,  26,  28,  30,  32,  34,  35,  37,  38,
                40,  41,  43,  44,  46,  47,  49,  50,  52,  53,  54,  56,  57,  58,  60,  61,  62,  64,
                65,  66,  67,  69,  70,  71,  72,  73,  75,  76,  77,  78,  79,  80,  82,  83,  84,  85,
                86,  87,  88,  89,  91,  92,  93,  94,  95,  96,  97,  98,  99,  100, 101, 102, 103, 104,
                105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122,
                123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 133, 134, 135, 136, 137, 138, 139,
                140, 141, 142, 143, 143, 144, 145, 146, 147, 148, 149, 150, 151, 151, 152, 153, 154, 155,
                156, 157, 157, 158, 159, 160, 161, 162, 163, 163, 164, 165, 166, 167, 168, 168, 169, 170,
                171, 172, 173, 173, 174, 175, 176, 177, 178, 178, 179, 180, 181, 182, 182, 183, 184, 185,
                186, 186, 187, 188, 189, 190, 190, 191, 192, 193, 194, 194, 195, 196, 197, 198, 198, 199,
                200, 201, 201, 202, 203, 204, 205, 205, 206, 207, 208, 208, 209, 210, 211, 211, 212, 213,
                214, 214, 215, 216, 217, 217, 218, 219, 220, 220, 221, 222, 223, 223, 224, 225, 226, 226,
                227, 228, 229, 229, 230, 231, 231, 232, 233, 234, 234, 235, 236, 237, 237, 238, 239, 239,
                240, 241, 242, 242, 243, 244, 244, 245, 246, 247, 247, 248, 249, 249, 250, 251, 251, 252,
                253, 254, 254, 255};
            for (int y = 0; y < h; y++) {
              const uint8_t *row = b->buffer + (ptrdiff_t)y * b->pitch;
              for (int x = 0; x < w; x++) {
                g->bits[y * w + x] = gamma[row[x]];
              }
            }
          }
        }
      }
    }
    g->key = key;
    g->w = (int16_t)(g->bits ? w : 0);
    g->h = (int16_t)(g->bits ? h : 0);
    g->xoff = (int16_t)xo;
    g->yoff = (int16_t)yo;
    g_glyph_count++;
    return g;
  }
#  endif
  g->bits = stbtt_GetGlyphBitmapSubpixel(
      info, s->scale, s->scale, (float)phase * 0.25f, 0.0f, glyph, &w, &h, &xo, &yo);
  if (g->bits) {
    /* stb's coverage is linear; blending it in sRGB thins strokes. Lift it
     * (gamma 1/1.45, close to how GDI's grayscale AA reads). */
    static uint8_t lut[256];
    if (!lut[255]) {
      for (int i = 0; i < 256; i++) {
        lut[i] = (uint8_t)(255.0f * powf((float)i / 255.0f, 1.0f / 1.45f) + 0.5f);
      }
    }
    for (int i = 0; i < w * h; i++) {
      g->bits[i] = lut[g->bits[i]];
    }
  }
  g->key = key;
  g->w = (int16_t)w;
  g->h = (int16_t)h;
  g->xoff = (int16_t)xo;
  g->yoff = (int16_t)yo;
  g_glyph_count++;
  return g;
}

/* Format characters that are zero-width and missing from most UI faces
 * (joiners, variation selectors, the keycap and tag characters of emoji
 * sequences): nothing to draw, rather than a .notdef box. */
static int cp_is_format(int cp)
{
  return (cp >= 0x200B && cp <= 0x200F) || cp == 0x20E3 || cp == 0xFEFF ||
         (cp >= 0xFE00 && cp <= 0xFE0F) || (cp >= 0xE0020 && cp <= 0xE007F) ||
         (cp >= 0xE0100 && cp <= 0xE01EF);
}

/* The advance of a glyph of slot `s`: the FreeType instance in use (weight,
 * optical size), unhinted; else the font's metrics. */
static float glyph_advance(const FontSlot *s, int glyph)
{
#  if TEXT_FT
  FT_Face ft = ft_use(s);
  FT_Fixed ftadv;
  if (ft && FT_Get_Advance(ft, (FT_UInt)glyph, FT_LOAD_NO_HINTING, &ftadv) == 0) {
    return (float)ftadv / 65536.0f + s->embolden;
  }
#  endif
  int adv, lsb;
  stbtt_GetGlyphHMetrics(&g_faces[s->face].info, glyph, &adv, &lsb);
  return s->scale * (float)adv + s->embolden;
}

/* Emoji sequences (ZWJ sequences, skin tones, keycaps, families, couples):
 * the emoji face's own OpenType layout applied to each run of glyphs the walk
 * takes from it, so U+1F468 U+200D U+1F9B2 draws as one bald man rather than
 * a man and a bald head. GSUB features `ccmp`, `liga`, `clig`, `rlig`: single
 * (1), ligature (4), chained context by coverage (6.3), extension (7). GPOS
 * features `dist`, `mark`, `mkmk` (Segoe UI Emoji builds families and
 * mixed-tone couples from parts placed this way): single adjustment (1),
 * mark to base (4), mark to mark (6), chained context by coverage (8.3),
 * extension (9), horizontally only. Other lookup types and lookup flags are
 * skipped. A minimal shaper: enough for colour emoji fonts, not for complex
 * scripts, and only ever used on the emoji face. */

/* A glyph of a walk: its slot and id; for a format character (joiner,
 * keycap, tag) the id it came in as, so one no ligature took can be dropped;
 * then GPOS's horizontal adjustments in font units (placement, advance) and
 * the glyph it is attached to, `back` glyphs before it (0: none). */
typedef struct WalkGlyph {
  const FontSlot *s;
  int glyph;
  int fmt;
  int dx, dadv, back;
  float x;
} WalkGlyph;

#  define OT_LOOKUPS_MAX 128

/* A face's GSUB or GPOS: the table's offset (0 = none) and the lookups of
 * the features read, in lookup-list order. Read once per face. */
typedef struct OtLayout {
  int read;
  uint32_t table;
  int n;
  uint16_t lookups[OT_LOOKUPS_MAX];
} OtLayout;
static OtLayout g_layout[2][MAX_FACES]; /* [0] GSUB, [1] GPOS */

static int ot_u16(const uint8_t *d, uint32_t o)
{
  return (d[o] << 8) | d[o + 1];
}

static int ot_s16(const uint8_t *d, uint32_t o)
{
  return (int16_t)ot_u16(d, o);
}

static uint32_t ot_u32(const uint8_t *d, uint32_t o)
{
  return ((uint32_t)d[o] << 24) | ((uint32_t)d[o + 1] << 16) | ((uint32_t)d[o + 2] << 8) | d[o + 3];
}

/* Glyph `g`'s index in the coverage table at `o`, -1 when it isn't covered. */
static int ot_coverage(const uint8_t *d, uint32_t o, int g)
{
  const int fmt = ot_u16(d, o), n = ot_u16(d, o + 2);
  int lo = 0, hi = n - 1;
  while (lo <= hi) {
    const int mid = (lo + hi) / 2;
    if (fmt == 1) {
      const int v = ot_u16(d, o + 4 + (uint32_t)mid * 2);
      if (v == g) {
        return mid;
      }
      if (v < g) {
        lo = mid + 1;
      }
      else {
        hi = mid - 1;
      }
    }
    else if (fmt == 2) {
      const uint32_t r = o + 4 + (uint32_t)mid * 6;
      const int start = ot_u16(d, r), end = ot_u16(d, r + 2);
      if (g < start) {
        hi = mid - 1;
      }
      else if (g > end) {
        lo = mid + 1;
      }
      else {
        return ot_u16(d, r + 4) + g - start;
      }
    }
    else {
      return -1;
    }
  }
  return -1;
}

/* Glyph `g`'s class in the class definition at `o` (0 when none). */
static int ot_class(const uint8_t *d, uint32_t o, int g)
{
  const int fmt = ot_u16(d, o);
  if (fmt == 1) {
    const int start = ot_u16(d, o + 2), n = ot_u16(d, o + 4);
    return g >= start && g < start + n ? ot_u16(d, o + 6 + (uint32_t)(g - start) * 2) : 0;
  }
  if (fmt == 2) {
    int lo = 0, hi = ot_u16(d, o + 2) - 1;
    while (lo <= hi) {
      const int mid = (lo + hi) / 2;
      const uint32_t r = o + 4 + (uint32_t)mid * 6;
      if (g < ot_u16(d, r)) {
        hi = mid - 1;
      }
      else if (g > ot_u16(d, r + 2)) {
        lo = mid + 1;
      }
      else {
        return ot_u16(d, r + 4);
      }
    }
  }
  return 0;
}

static int ot_tag(const uint8_t *d, uint32_t o, const char *tags)
{
  for (const char *t = tags; *t; t += 4) {
    if (memcmp(d + o, t, 4) == 0) {
      return 1;
    }
  }
  return 0;
}

static const OtLayout *face_layout(int face, int gpos)
{
  OtLayout *gi = &g_layout[gpos][face];
  if (gi->read) {
    return gi;
  }
  gi->read = 1;
  const stbtt_fontinfo *info = &g_faces[face].info;
  const uint8_t *d = info->data;
  const uint32_t t = stbtt__find_table((stbtt_uint8 *)info->data, (stbtt_uint32)info->fontstart,
                                       gpos ? "GPOS" : "GSUB");
  if (!t) {
    return gi;
  }
  const char *features = gpos ? "distmarkmkmk" : "ccmpligacligrlig";
  /* the default script (DFLT, else latn, else the first) and its default language */
  const uint32_t scripts = t + (uint32_t)ot_u16(d, t + 4);
  const int nscripts = ot_u16(d, scripts);
  uint32_t script = 0;
  for (int pass = 0; pass < 3 && !script; pass++) {
    for (int i = 0; i < nscripts && !script; i++) {
      const uint32_t r = scripts + 2 + (uint32_t)i * 6;
      if (pass == 2 || ot_tag(d, r, pass == 0 ? "DFLT" : "latn")) {
        script = scripts + (uint32_t)ot_u16(d, r + 4);
      }
    }
  }
  if (!script || !ot_u16(d, script)) {
    return gi;
  }
  const uint32_t lang = script + (uint32_t)ot_u16(d, script);
  const uint32_t flist = t + (uint32_t)ot_u16(d, t + 6);
  const int nlookups = ot_u16(d, t + (uint32_t)ot_u16(d, t + 8)); /* the lookup list's count */
  static uint8_t used[8192]; /* lookup indices, as bits (walks run on one thread) */
  memset(used, 0, sizeof(used));
  const int nf = ot_u16(d, lang + 4);
  for (int i = 0; i < nf; i++) {
    const int fi = ot_u16(d, lang + 6 + (uint32_t)i * 2);
    const uint32_t r = flist + 2 + (uint32_t)fi * 6;
    if (!ot_tag(d, r, features)) {
      continue;
    }
    const uint32_t f = flist + (uint32_t)ot_u16(d, r + 4);
    const int nl = ot_u16(d, f + 2);
    for (int j = 0; j < nl; j++) {
      const int li = ot_u16(d, f + 4 + (uint32_t)j * 2);
      if (li < nlookups) {
        used[li >> 3] |= (uint8_t)(1 << (li & 7));
      }
    }
  }
  for (int li = 0; li < nlookups && gi->n < OT_LOOKUPS_MAX; li++) {
    if (used[li >> 3] & (1 << (li & 7))) {
      gi->lookups[gi->n++] = (uint16_t)li;
    }
  }
  gi->table = t;
  return gi;
}

/* A run being shaped: the face's data, the table, which one, the glyphs. */
typedef struct OtRun {
  const uint8_t *d;
  uint32_t t;
  int gpos;
  uint32_t gdef_classes; /* GDEF's glyph classes (3 = mark), 0 when none */
  WalkGlyph *g;
  int n;
} OtRun;

static int ot_apply_at(OtRun *r, int lookup, int pos, int *r_len, int depth);

/* A value record's horizontal parts (font units) and its size in bytes. */
static int ot_value(const uint8_t *d, uint32_t o, int fmt, int *r_dx, int *r_dadv)
{
  int at = 0;
  *r_dx = *r_dadv = 0;
  for (int bit = 0; bit < 8; bit++) {
    if (fmt & (1 << bit)) {
      if (bit == 0) {
        *r_dx = ot_s16(d, o + (uint32_t)at);
      }
      else if (bit == 2) {
        *r_dadv = ot_s16(d, o + (uint32_t)at);
      }
      at += 2;
    }
  }
  return at;
}

static int ot_is_mark(const OtRun *r, int i)
{
  return r->gdef_classes && ot_class(r->d, r->gdef_classes, r->g[i].glyph) == 3;
}

/* `glyph` at `at` attached to the glyph `base` (before it) by the anchors
 * at `mark_anchor` and `base_anchor`. */
static void ot_attach(OtRun *r, int at, int base, uint32_t mark_anchor, uint32_t base_anchor)
{
  r->g[at].back = at - base;
  r->g[at].dx = ot_s16(r->d, base_anchor + 2) - ot_s16(r->d, mark_anchor + 2);
}

/* One subtable of type `type` at `o` on glyph `pos`: whether it applied, and
 * how many glyphs it consumed (`*r_len`, after any ligature). */
static int ot_subtable(OtRun *r, int type, uint32_t o, int pos, int *r_len, int depth)
{
  const uint8_t *d = r->d;
  WalkGlyph *g = r->g;
  const int fmt = ot_u16(d, o);
  *r_len = 1;
  if (type == (r->gpos ? 9 : 7) && fmt == 1) { /* extension: the real subtable, 32-bit offset */
    return ot_subtable(r, ot_u16(d, o + 2), o + ot_u32(d, o + 4), pos, r_len, depth);
  }
  if (type == (r->gpos ? 8 : 6) && fmt == 3) { /* chained context, by coverage */
    uint32_t p = o + 2;
    const int nb = ot_u16(d, p);
    if (pos < nb) {
      return 0;
    }
    for (int i = 0; i < nb; i++) {
      if (ot_coverage(d, o + (uint32_t)ot_u16(d, p + 2 + (uint32_t)i * 2), g[pos - 1 - i].glyph) < 0) {
        return 0;
      }
    }
    p += 2 + (uint32_t)nb * 2;
    const int ni = ot_u16(d, p);
    const uint32_t in = p;
    p += 2 + (uint32_t)ni * 2;
    const int na = ot_u16(d, p);
    if (ni < 1 || pos + ni + na > r->n) {
      return 0;
    }
    for (int i = 0; i < ni; i++) {
      if (ot_coverage(d, o + (uint32_t)ot_u16(d, in + 2 + (uint32_t)i * 2), g[pos + i].glyph) < 0) {
        return 0;
      }
    }
    for (int i = 0; i < na; i++) {
      if (ot_coverage(d, o + (uint32_t)ot_u16(d, p + 2 + (uint32_t)i * 2), g[pos + ni + i].glyph) < 0) {
        return 0;
      }
    }
    p += 2 + (uint32_t)na * 2;
    const int nrec = ot_u16(d, p);
    int len = ni; /* the input, as nested ligatures shorten it */
    for (int i = 0; i < nrec && depth < 4; i++) {
      const int at = ot_u16(d, p + 2 + (uint32_t)i * 4);
      if (at < len) {
        const int before = r->n;
        int used;
        ot_apply_at(r, ot_u16(d, p + 4 + (uint32_t)i * 4), pos + at, &used, depth + 1);
        len -= before - r->n;
      }
    }
    *r_len = len > 0 ? len : 1;
    return 1;
  }
  if (r->gpos) {
    if (type == 1) { /* single adjustment */
      const int ci = ot_coverage(d, o + (uint32_t)ot_u16(d, o + 2), g[pos].glyph);
      const int vf = ot_u16(d, o + 4);
      int dx, dadv;
      if (ci < 0) {
        return 0;
      }
      if (fmt == 1) {
        ot_value(d, o + 6, vf, &dx, &dadv);
      }
      else if (fmt == 2 && ci < ot_u16(d, o + 6)) {
        const int size = ot_value(d, o + 8, vf, &dx, &dadv);
        ot_value(d, o + 8 + (uint32_t)(ci * size), vf, &dx, &dadv);
      }
      else {
        return 0;
      }
      g[pos].dx += dx;
      g[pos].dadv += dadv;
      return 1;
    }
    if ((type == 4 || type == 6) && fmt == 1) { /* mark to base, mark to mark */
      const int mi = ot_coverage(d, o + (uint32_t)ot_u16(d, o + 2), g[pos].glyph);
      if (mi < 0) {
        return 0;
      }
      int base = pos - 1;
      while (type == 4 && base >= 0 && ot_is_mark(r, base)) {
        base--;
      }
      const int bi = base >= 0 ? ot_coverage(d, o + (uint32_t)ot_u16(d, o + 4), g[base].glyph) : -1;
      const int classes = ot_u16(d, o + 6);
      const uint32_t marks = o + (uint32_t)ot_u16(d, o + 8), bases = o + (uint32_t)ot_u16(d, o + 10);
      if (bi < 0 || mi >= ot_u16(d, marks) || bi >= ot_u16(d, bases)) {
        return 0;
      }
      const int cls = ot_u16(d, marks + 2 + (uint32_t)mi * 4);
      const int ma = ot_u16(d, marks + 4 + (uint32_t)mi * 4);
      const int ba = cls < classes ? ot_u16(d, bases + 2 + (uint32_t)(bi * classes + cls) * 2) : 0;
      if (!ma || !ba) {
        return 0;
      }
      ot_attach(r, pos, base, marks + (uint32_t)ma, bases + (uint32_t)ba);
      return 1;
    }
    return 0;
  }
  if (type == 1) { /* single */
    const int ci = ot_coverage(d, o + (uint32_t)ot_u16(d, o + 2), g[pos].glyph);
    if (ci < 0) {
      return 0;
    }
    if (fmt == 1) {
      g[pos].glyph = (g[pos].glyph + ot_s16(d, o + 4)) & 0xFFFF;
    }
    else if (fmt == 2 && ci < ot_u16(d, o + 4)) {
      g[pos].glyph = ot_u16(d, o + 6 + (uint32_t)ci * 2);
    }
    else {
      return 0;
    }
    g[pos].fmt = 0;
    return 1;
  }
  if (type == 4 && fmt == 1) { /* ligature: the first that matches (the font puts longer first) */
    const int ci = ot_coverage(d, o + (uint32_t)ot_u16(d, o + 2), g[pos].glyph);
    if (ci < 0 || ci >= ot_u16(d, o + 4)) {
      return 0;
    }
    const uint32_t set = o + (uint32_t)ot_u16(d, o + 6 + (uint32_t)ci * 2);
    const int nlig = ot_u16(d, set);
    for (int i = 0; i < nlig; i++) {
      const uint32_t lig = set + (uint32_t)ot_u16(d, set + 2 + (uint32_t)i * 2);
      const int comps = ot_u16(d, lig + 2);
      if (comps < 1 || pos + comps > r->n) {
        continue;
      }
      int k = 1;
      while (k < comps && g[pos + k].glyph == ot_u16(d, lig + 4 + (uint32_t)(k - 1) * 2)) {
        k++;
      }
      if (k == comps) {
        g[pos].glyph = ot_u16(d, lig);
        g[pos].fmt = 0;
        memmove(g + pos + 1, g + pos + comps, sizeof(WalkGlyph) * (size_t)(r->n - pos - comps));
        r->n -= comps - 1;
        return 1;
      }
    }
  }
  return 0;
}

/* Lookup `lookup` at glyph `pos`: its first subtable that applies. */
static int ot_apply_at(OtRun *r, int lookup, int pos, int *r_len, int depth)
{
  const uint32_t list = r->t + (uint32_t)ot_u16(r->d, r->t + 8);
  if (lookup >= ot_u16(r->d, list)) {
    return 0;
  }
  const uint32_t l = list + (uint32_t)ot_u16(r->d, list + 2 + (uint32_t)lookup * 2);
  const int type = ot_u16(r->d, l), nsub = ot_u16(r->d, l + 4);
  for (int i = 0; i < nsub; i++) {
    const uint32_t o = l + (uint32_t)ot_u16(r->d, l + 6 + (uint32_t)i * 2);
    if (ot_subtable(r, type, o, pos, r_len, depth)) {
      return 1;
    }
  }
  return 0;
}

/* The face's features of one table over `g` (`n` glyphs, all of the face):
 * the glyphs left. */
static int ot_run(int face, int gpos, WalkGlyph *g, int n)
{
  const OtLayout *gi = face_layout(face, gpos);
  if (!gi->table || n < (gpos ? 1 : 2)) {
    return n;
  }
  const stbtt_fontinfo *info = &g_faces[face].info;
  OtRun r = {info->data, gi->table, gpos, 0, g, n};
  const uint32_t gdef = stbtt__find_table((stbtt_uint8 *)info->data, (stbtt_uint32)info->fontstart, "GDEF");
  if (gdef && ot_u16(r.d, gdef + 4)) {
    r.gdef_classes = gdef + (uint32_t)ot_u16(r.d, gdef + 4);
  }
  for (int k = 0; k < gi->n; k++) {
    for (int pos = 0; pos < r.n;) {
      int len = 1;
      if (!ot_apply_at(&r, gi->lookups[k], pos, &len, 0)) {
        len = 1;
      }
      pos += len;
    }
  }
  return r.n;
}

/* Joined parts that belong to the emoji before them: joiners, keycaps, tags. */
static int cp_is_joiner(int cp)
{
  return cp == 0x200D || cp == 0x20E3 || (cp >= 0xE0020 && cp <= 0xE007F);
}

/* Walks the glyphs of a run: each glyph's slot (the run's, or its fallback
 * for characters the face lacks) and pen position in pixels (unrounded). */
typedef void (*GlyphFn)(void *user, const FontSlot *s, int glyph, float pen);

static float glyph_walk(const FontSlot *s, const char *utf8, int len, GlyphFn fn, void *user)
{
  const stbtt_fontinfo *info = &g_faces[s->face].info;
  WalkGlyph stack[128];
  WalkGlyph *g = stack;
  int cap = 128, n = 0;
  /* the glyphs, each from the face or its fallback */
  int i = 0;
  int cp = i < len ? utf8_next(utf8, len, &i) : -1;
  int zwj = 0; /* the last glyph is a joiner of the emoji face */
  while (cp >= 0) {
    const int next = i < len ? utf8_next(utf8, len, &i) : -1; /* one ahead: U+FE0F */
    if (n == cap) {
      WalkGlyph *ng = (WalkGlyph *)malloc(sizeof(WalkGlyph) * (size_t)cap * 2);
      if (!ng) {
        break;
      }
      memcpy(ng, g, sizeof(WalkGlyph) * (size_t)n);
      if (g != stack) {
        free(g);
      }
      g = ng;
      cap *= 2;
    }
    const FontSlot *prev_s = n ? g[n - 1].s : NULL;
    const int in_emoji = prev_s && prev_s != s; /* just after a glyph of the emoji face */
    const int was_zwj = zwj;
    zwj = 0;
    if (cp_is_joiner(cp) && in_emoji) {
      /* a joiner, keycap or tag in an emoji sequence: the emoji face's, for its ligatures */
      const int fbg = stbtt_FindGlyphIndex(&g_faces[prev_s->face].info, cp);
      if (fbg) {
        g[n++] = (WalkGlyph){prev_s, fbg, fbg, 0, 0, 0, 0.0f};
        zwj = cp == 0x200D;
      }
      cp = next;
      continue;
    }
    if ((cp >= 0xFE00 && cp <= 0xFE0F) || (cp >= 0xE0100 && cp <= 0xE01EF)) {
      /* variation selectors only choose the presentation (the look ahead below) */
      cp = next;
      continue;
    }
    const FontSlot *gs = s;
    int glyph = stbtt_FindGlyphIndex(info, cp == '\t' ? ' ' : cp);
    if (glyph == 0 && cp_is_format(cp)) { /* nothing to draw */
      cp = next;
      continue;
    }
    if (cp >= 0x20 && (glyph == 0 || next == 0xFE0F || was_zwj)) {
      /* the face lacks it, U+FE0F asks for the emoji presentation, or a
       * joiner ties it to an emoji: the emoji face (no kerning across faces) */
      const FontSlot *fb = slot_fallback(s);
      const int fbg = fb ? stbtt_FindGlyphIndex(&g_faces[fb->face].info, cp) : 0;
      if (fbg) {
        glyph = fbg;
        gs = fb;
      }
    }
    g[n++] = (WalkGlyph){gs, glyph, 0, 0, 0, 0, 0.0f};
    cp = next;
  }
  /* each run of the emoji face's glyphs shaped: its ligatures, the joiners
   * none took dropped, then its positioning */
  for (int a = 0; a < n;) {
    int b = a + 1;
    while (b < n && g[b].s == g[a].s) {
      b++;
    }
    if (g[a].s != s) {
      int m = ot_run(g[a].s->face, 0, g + a, b - a);
      int kept = 0;
      for (int k = 0; k < m; k++) {
        if (!(g[a + k].fmt && g[a + k].fmt == g[a + k].glyph)) {
          g[a + kept++] = g[a + k];
        }
      }
      m = ot_run(g[a].s->face, 1, g + a, kept);
      if (m < b - a) {
        memmove(g + a + m, g + b, sizeof(WalkGlyph) * (size_t)(n - b));
        n -= b - a - m;
        b = a + m;
      }
    }
    a = b;
  }
  /* pen positions, kerned within a face; attached glyphs placed on theirs */
  float pen = 0.0f;
  for (int k = 0; k < n; k++) {
    const FontSlot *gs = g[k].s;
    if (g[k].back) {
      g[k].x = g[k - g[k].back].x + gs->scale * (float)g[k].dx;
    }
    else {
      if (k && g[k - 1].s == gs && !g[k - 1].back) {
        pen += gs->scale * (float)stbtt_GetGlyphKernAdvance(&g_faces[gs->face].info, g[k - 1].glyph, g[k].glyph);
      }
      g[k].x = pen + gs->scale * (float)g[k].dx;
      pen += glyph_advance(gs, g[k].glyph) + gs->scale * (float)g[k].dadv;
    }
    if (fn) {
      fn(user, gs, g[k].glyph, g[k].x);
    }
  }
  if (g != stack) {
    free(g);
  }
  return pen;
}

static float be_measure(const FontSlot *s, const char *utf8, int len)
{
  return floorf(glyph_walk(s, utf8, len, NULL, NULL) + 0.5f);
}

typedef struct GlyphList {
  TextRun *e;
  int cap;
} GlyphList;

static void list_glyph(void *user, const FontSlot *s, int glyph, float pen)
{
  GlyphList *l = (GlyphList *)user;
  TextRun *e = l->e;
  if (e->ng == l->cap) {
    l->cap = l->cap ? l->cap * 2 : 16;
    uint32_t *g = (uint32_t *)realloc(e->gid, sizeof(uint32_t) * (size_t)l->cap);
    float *x = (float *)realloc(e->gx, sizeof(float) * (size_t)l->cap);
    if (!g || !x) {
      e->gid = g ? g : e->gid;
      e->gx = x ? x : e->gx;
      return;
    }
    e->gid = g;
    e->gx = x;
  }
  e->gid[e->ng] = (uint32_t)glyph | (s == &g_fonts[e->font] ? 0u : GID_FALLBACK);
  e->gx[e->ng] = pen;
  e->ng++;
}

/* The run's glyph ids and pen positions (built once). */
static int run_glyphs(TextRun *e)
{
  if (e->ng < 0) {
    e->ng = 0;
    GlyphList l = {e, 0};
    glyph_walk(&g_fonts[e->font], e->text, e->len, list_glyph, &l);
  }
  return 1;
}

/* The slot glyph `i` of a run draws with: the run's, or its fallback. */
static const FontSlot *run_slot(const TextRun *e, int i)
{
  const FontSlot *s = &g_fonts[e->font];
  return (e->gid[i] & GID_FALLBACK) && s->fallback >= 0 ? &g_fonts[s->fallback] : s;
}

static int glyph_draw_info(const TextRun *e, int i, int *r_face, float *r_scale, float *r_embolden,
                           float *r_shear)
{
  const FontSlot *s = run_slot(e, i);
  *r_face = s->face;
  *r_scale = s->scale;
  *r_embolden = s->embolden;
  *r_shear = (s->synth & BUI_FONT_ITALIC) ? 0.2f : 0.0f;
  return 1;
}

/* Glyph `glyph` of the face glyph `i` of a run draws with (the glyph itself
 * or one of its colour layers), as a cached bitmap at the pen's
 * quarter-pixel phase: its offset from the run origin (baseline), size, and
 * a stable key. */
static const void *glyph_bitmap(const TextRun *e, int i, int glyph, float pen, int *r_x, int *r_y,
                                int *r_w, int *r_h, const uint8_t **r_bits)
{
  const FontSlot *s = run_slot(e, i);
  const float px = floorf(pen * 4.0f + 0.5f) * 0.25f;
  const int whole = (int)floorf(px);
  const int phase = (int)((px - (float)whole) * 4.0f) & 3;
  const GlyphBmp *g = glyph_get((int)(s - g_fonts), s, glyph, phase);
  if (!g || !g->bits) {
    return NULL;
  }
  *r_x = whole + g->xoff;
  *r_y = g->yoff;
  *r_w = g->w;
  *r_h = g->h;
  *r_bits = g->bits;
  return g;
}

/* The colour (COLR v0) layers of `glyph` of a face, bottom to top, in the
 * colours of its palette (`fg`, the text colour, where the font says so;
 * the text's alpha applies to all). Returns how many there are, storing up
 * to `max`; 0 for a plain glyph. */
static int face_layers(int face, int glyph, const float fg[4], GlyphLayer *r_layers, int max)
{
#  if TEXT_FT
  const Face *f = &g_faces[face];
  if (!f->palette) {
    return 0;
  }
  FT_LayerIterator it;
  FT_UInt lg, ci;
  int n = 0;
  it.p = NULL;
  while (FT_Get_Color_Glyph_Layer(f->ft, (FT_UInt)glyph, &lg, &ci, &it)) {
    if (n < max) {
      GlyphLayer *l = &r_layers[n];
      l->glyph = (int)lg;
      if (ci == 0xFFFF || ci >= f->ncolors) { /* the foreground */
        c4(l->rgba, fg);
      }
      else {
        const FT_Color c = f->palette[ci];
        v4(l->rgba, (float)c.red / 255.0f, (float)c.green / 255.0f, (float)c.blue / 255.0f,
           (float)c.alpha / 255.0f * fg[3]);
      }
    }
    n++;
  }
  return n;
#  else
  (void)face;
  (void)glyph;
  (void)fg;
  (void)r_layers;
  (void)max;
  return 0;
#  endif
}

static int glyph_layers(const TextRun *e, int i, const float fg[4], GlyphLayer *r_layers, int max)
{
  return face_layers(run_slot(e, i)->face, (int)(e->gid[i] & ~GID_FALLBACK), fg, r_layers, max);
}

#  if TEXT_FT
/* FT_Outline_Decompose into the same quadratic list as below. */
typedef struct OutlineSink {
  float *c;
  int n, cap;
  float cx, cy;
} OutlineSink;

static int osink_push(OutlineSink *o, float x1, float y1, float x2, float y2, float x3, float y3)
{
  if (o->n == o->cap) {
    o->cap = o->cap ? o->cap * 2 : 64;
    float *c = (float *)realloc(o->c, sizeof(float) * 6 * (size_t)o->cap);
    if (!c) {
      return 1;
    }
    o->c = c;
  }
  float *q = o->c + 6 * o->n++;
  q[0] = x1; q[1] = y1; q[2] = x2; q[3] = y2; q[4] = x3; q[5] = y3;
  return 0;
}

static int osink_move(const FT_Vector *to, void *u)
{
  OutlineSink *o = (OutlineSink *)u;
  o->cx = (float)to->x;
  o->cy = (float)to->y;
  return 0;
}

static int osink_line(const FT_Vector *to, void *u)
{
  OutlineSink *o = (OutlineSink *)u;
  const float x = (float)to->x, y = (float)to->y;
  const int r = osink_push(o, o->cx, o->cy, x, y, x, y);
  o->cx = x;
  o->cy = y;
  return r;
}

static int osink_conic(const FT_Vector *ctl, const FT_Vector *to, void *u)
{
  OutlineSink *o = (OutlineSink *)u;
  const float x = (float)to->x, y = (float)to->y;
  const int r = osink_push(o, o->cx, o->cy, (float)ctl->x, (float)ctl->y, x, y);
  o->cx = x;
  o->cy = y;
  return r;
}

static int osink_cubic(const FT_Vector *c1, const FT_Vector *c2, const FT_Vector *to, void *u)
{
  /* split at t = 1/2; each half approximated by one quadratic */
  OutlineSink *o = (OutlineSink *)u;
  const float p0x = o->cx, p0y = o->cy, p1x = (float)c1->x, p1y = (float)c1->y;
  const float p2x = (float)c2->x, p2y = (float)c2->y, x = (float)to->x, y = (float)to->y;
  const float m01x = (p0x + p1x) / 2, m01y = (p0y + p1y) / 2, m12x = (p1x + p2x) / 2,
              m12y = (p1y + p2y) / 2, m23x = (p2x + x) / 2, m23y = (p2y + y) / 2;
  const float ax = (m01x + m12x) / 2, ay = (m01y + m12y) / 2, bx = (m12x + m23x) / 2,
              by = (m12y + m23y) / 2, mx = (ax + bx) / 2, my = (ay + by) / 2;
  int r = osink_push(o, p0x, p0y, (3 * (m01x + ax) - p0x - mx) / 4, (3 * (m01y + ay) - p0y - my) / 4,
                     mx, my);
  r |= osink_push(o, mx, my, (3 * (bx + m23x) - mx - x) / 4, (3 * (by + m23y) - my - y) / 4, x, y);
  o->cx = x;
  o->cy = y;
  return r;
}

/* The outline of the instance the face draws (its weight; UI optical size). */
static int ft_glyph_outline(int face, int glyph, float **r_curves, int *r_n, float r_bbox[4])
{
  Face *f = &g_faces[face];
  FontSlot tmp;
  memset(&tmp, 0, sizeof(tmp));
  tmp.face = face;
  tmp.opsz = 14.0f; /* outlines are size independent: the UI's optical size */
  FT_Size size = NULL;
  if (FT_New_Size(f->ft, &size) != 0) {
    return 0;
  }
  tmp.ft_size = size;
  ft_use(&tmp);
  int ok = 0;
  if (FT_Load_Glyph(f->ft, (FT_UInt)glyph, FT_LOAD_NO_SCALE | FT_LOAD_NO_BITMAP | FT_LOAD_NO_HINTING) == 0 &&
      f->ft->glyph->format == FT_GLYPH_FORMAT_OUTLINE)
  {
    static const FT_Outline_Funcs funcs = {osink_move, osink_line, osink_conic, osink_cubic, 0, 0};
    OutlineSink o = {NULL, 0, 0, 0.0f, 0.0f};
    if (FT_Outline_Decompose(&f->ft->glyph->outline, &funcs, &o) == 0 && o.n > 0) {
      FT_BBox b;
      FT_Outline_Get_CBox(&f->ft->glyph->outline, &b);
      r_bbox[0] = (float)b.xMin;
      r_bbox[1] = (float)b.yMin;
      r_bbox[2] = (float)b.xMax;
      r_bbox[3] = (float)b.yMax;
      *r_curves = o.c;
      *r_n = o.n;
      ok = 1;
    }
    else {
      free(o.c);
    }
  }
  FT_Done_Size(size);
  f->set_opsz = -1.0f; /* the next slot sets its own */
  return ok;
}
#  endif

/* A glyph's outline as quadratic curves (x1 y1 x2 y2 x3 y3, font units,
 * y up) and its box; lines are {p1, p2, p2}, cubics are split into
 * quadratics. Returns 0 for empty glyphs. `*r_curves` is malloc'd. */
int bui_text_glyph_outline(int face, int glyph, float **r_curves, int *r_n, float r_bbox[4])
{
  *r_curves = NULL;
  *r_n = 0;
  if (face < 0 || face >= g_face_count) {
    return 0;
  }
#  if TEXT_FT
  if (g_faces[face].ft) {
    return ft_glyph_outline(face, glyph, r_curves, r_n, r_bbox);
  }
#  endif
  const stbtt_fontinfo *info = &g_faces[face].info;
  stbtt_vertex *v = NULL;
  const int nv = stbtt_GetGlyphShape(info, glyph, &v);
  if (nv <= 0) {
    return 0;
  }
  float *c = (float *)malloc(sizeof(float) * 6 * (size_t)nv * 2);
  if (!c) {
    stbtt_FreeShape(info, v);
    return 0;
  }
  int n = 0;
  float cx = 0, cy = 0, sx = 0, sy = 0;
  for (int k = 0; k < nv; k++) {
    const float x = v[k].x, y = v[k].y;
    float *q;
    switch (v[k].type) {
      case STBTT_vmove:
        if ((cx != sx || cy != sy) && n > 0) { /* close the previous contour */
          q = c + 6 * n++;
          q[0] = cx; q[1] = cy; q[2] = sx; q[3] = sy; q[4] = sx; q[5] = sy;
        }
        cx = sx = x;
        cy = sy = y;
        break;
      case STBTT_vline:
        q = c + 6 * n++;
        q[0] = cx; q[1] = cy; q[2] = x; q[3] = y; q[4] = x; q[5] = y;
        cx = x;
        cy = y;
        break;
      case STBTT_vcurve:
        q = c + 6 * n++;
        q[0] = cx; q[1] = cy; q[2] = v[k].cx; q[3] = v[k].cy; q[4] = x; q[5] = y;
        cx = x;
        cy = y;
        break;
      case STBTT_vcubic: {
        /* split at t = 1/2; each half approximated by one quadratic */
        const float p0x = cx, p0y = cy, p1x = v[k].cx, p1y = v[k].cy, p2x = v[k].cx1, p2y = v[k].cy1;
        const float m01x = (p0x + p1x) / 2, m01y = (p0y + p1y) / 2, m12x = (p1x + p2x) / 2,
                    m12y = (p1y + p2y) / 2, m23x = (p2x + x) / 2, m23y = (p2y + y) / 2;
        const float ax = (m01x + m12x) / 2, ay = (m01y + m12y) / 2, bx = (m12x + m23x) / 2,
                    by = (m12y + m23y) / 2, mx = (ax + bx) / 2, my = (ay + by) / 2;
        q = c + 6 * n++;
        q[0] = p0x; q[1] = p0y;
        q[2] = (3 * (m01x + ax) - p0x - mx) / 4; q[3] = (3 * (m01y + ay) - p0y - my) / 4;
        q[4] = mx; q[5] = my;
        q = c + 6 * n++;
        q[0] = mx; q[1] = my;
        q[2] = (3 * (bx + m23x) - mx - x) / 4; q[3] = (3 * (by + m23y) - my - y) / 4;
        q[4] = x; q[5] = y;
        cx = x;
        cy = y;
        break;
      }
      default:
        break;
    }
  }
  if ((cx != sx || cy != sy) && n > 0) {
    float *q = c + 6 * n++;
    q[0] = cx; q[1] = cy; q[2] = sx; q[3] = sy; q[4] = sx; q[5] = sy;
  }
  stbtt_FreeShape(info, v);
  int x0, y0, x1, y1;
  stbtt_GetGlyphBox(info, glyph, &x0, &y0, &x1, &y1);
  r_bbox[0] = (float)x0;
  r_bbox[1] = (float)y0;
  r_bbox[2] = (float)x1;
  r_bbox[3] = (float)y1;
  if (n == 0) {
    free(c);
    return 0;
  }
  *r_curves = c;
  *r_n = n;
  return 1;
}

typedef struct BuildCtx {
  uint8_t *mask;
  int mw, mh, ox, ascent;
  float shear; /* synthetic italic: pixels per pixel above the baseline */
  int color;   /* saw a colour glyph (left out: drawn in its own colours) */
} BuildCtx;

static void build_glyph(void *user, const FontSlot *s, int glyph, float pen)
{
  BuildCtx *c = (BuildCtx *)user;
  if (face_layers(s->face, glyph, NULL, NULL, 0) > 0) {
    c->color = 1;
    return;
  }
  const float px = floorf(pen * 4.0f + 0.5f) * 0.25f;
  const int whole = (int)floorf(px);
  const int phase = (int)((px - (float)whole) * 4.0f) & 3;
  const GlyphBmp *g = glyph_get((int)(s - g_fonts), s, glyph, phase);
  if (!g || !g->bits) {
    return;
  }
  const int gx = c->ox + whole + g->xoff;
  const int gy = c->ascent + g->yoff;
  for (int y = 0; y < g->h; y++) {
    const int my = gy + y;
    if (my < 0 || my >= c->mh) {
      continue;
    }
    uint8_t *row = c->mask + (size_t)my * c->mw;
    const uint8_t *src = g->bits + (size_t)y * g->w;
    if (c->shear == 0.0f) {
      for (int x = 0; x < g->w; x++) {
        const int mx = gx + x;
        if (mx >= 0 && mx < c->mw && src[x]) {
          const int v = row[mx] + src[x];
          row[mx] = (uint8_t)(v > 255 ? 255 : v);
        }
      }
    }
    else {
      /* shear about the baseline, splitting coverage between two pixels */
      const float sh = (float)(c->ascent - my) * c->shear;
      const int si = (int)floorf(sh);
      const int fr = (int)((sh - (float)si) * 256.0f);
      for (int x = 0; x < g->w; x++) {
        if (!src[x]) {
          continue;
        }
        const int mx = gx + x + si;
        const int hi = (src[x] * fr) >> 8, lo = src[x] - hi;
        if (mx >= 0 && mx < c->mw) {
          const int v = row[mx] + lo;
          row[mx] = (uint8_t)(v > 255 ? 255 : v);
        }
        if (mx + 1 >= 0 && mx + 1 < c->mw) {
          const int v = row[mx + 1] + hi;
          row[mx + 1] = (uint8_t)(v > 255 ? 255 : v);
        }
      }
    }
  }
}

static void be_build(TextRun *e, const FontSlot *fs)
{
  const int pad = 2 + fs->px / 4; /* bearings / italic overhang */
  const int mw = (int)e->advance + pad * 2;
  /* tall enough for the fallback face too (measuring opened it, if used) */
  const FontSlot *fb = fs->fallback >= 0 ? &g_fonts[fs->fallback] : fs;
  const int ascent = fs->ascent > fb->ascent ? fs->ascent : fb->ascent;
  const int descent = fs->height - fs->ascent > fb->height - fb->ascent ? fs->height - fs->ascent :
                                                                           fb->height - fb->ascent;
  const int mh = ascent + descent + 1;
  uint8_t *mask = (uint8_t *)calloc((size_t)mw * mh, 1);
  if (!mask) {
    return;
  }
  BuildCtx c = {mask, mw, mh, pad, ascent, (fs->synth & BUI_FONT_ITALIC) ? 0.2f : 0.0f, 0};
  glyph_walk(fs, e->text, e->len, build_glyph, &c);
  e->color = c.color;
  if (fs->embolden > 0.0f) { /* synthetic bold: smear right */
    const int k = (int)(fs->embolden + 0.5f);
    for (int y = 0; y < mh; y++) {
      uint8_t *row = mask + (size_t)y * mw;
      for (int x = mw - 1; x >= k; x--) {
        int v = row[x];
        for (int j = 1; j <= k; j++) {
          v = row[x - j] > v ? row[x - j] : v;
        }
        row[x] = (uint8_t)v;
      }
    }
  }
  if (fs->flags & BUI_FONT_UNDERLINE) {
    const int t = fs->px / 14 > 1 ? fs->px / 14 : 1;
    const int y0 = ascent + (fs->height - fs->ascent) / 2;
    for (int y = y0; y < y0 + t && y < mh; y++) {
      memset(mask + (size_t)y * mw + pad, 255, (size_t)e->advance);
    }
  }
  int any = 0;
  for (int i = 0; i < mw * mh && !any; i++) {
    any = mask[i] != 0;
  }
  if (!any) {
    free(mask);
    return;
  }
  e->mask = mask;
  e->mw = mw;
  e->mh = mh;
  e->ox = pad;
  e->oy = ascent;
}

/** \} */
#endif /* text backends */

/* -------------------------------------------------------------------- */
/** \name Native present
 * \{ */

#ifdef _WIN32
int BUI_raster_present_native(BuiRaster *r, void *window_handle, int full)
{
  if (r->gpu) {
    if (!bui_gpu_present(r->gpu, full)) {
      gpu_drop(r); /* device lost: the software raster takes over */
    }
    return 1;
  }
  HWND hwnd = (HWND)window_handle;
  if (!hwnd || !IsWindow(hwnd)) {
    return 0;
  }
  HDC hdc = GetDC(hwnd);
  if (!hdc) {
    return 0;
  }
  BITMAPINFO bmi;
  memset(&bmi, 0, sizeof(bmi));
  bmi.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
  bmi.bmiHeader.biWidth = r->w;
  bmi.bmiHeader.biPlanes = 1;
  bmi.bmiHeader.biBitCount = 32;
  bmi.bmiHeader.biCompression = BI_RGB;

  int whole[1][4] = {{0, 0, r->w, r->h}};
  const int(*rects)[4] = full ? (const int(*)[4])whole : (const int(*)[4])r->damage;
  const int n = full ? 1 : r->damage_len;
  /* Each rect is copied into its own tight top-down DIB and blitted whole
   * (src origin 0,0, all scanlines). Sub-rect source offsets into a
   * top-down DIB are interpreted inconsistently by GDI, which left partial
   * updates on screen stale. */
  uint32_t *tmp = NULL;
  size_t tmp_cap = 0;
  for (int i = 0; i < n; i++) {
    const int rx = rects[i][0], ry = rects[i][1], rw = rects[i][2], rh = rects[i][3];
    if (rw <= 0 || rh <= 0) {
      continue;
    }
    const uint32_t *bits = r->pix;
    if (!(rx == 0 && rw == r->w)) {
      const size_t need = (size_t)rw * rh;
      if (need > tmp_cap) {
        uint32_t *nt = (uint32_t *)realloc(tmp, need * sizeof(uint32_t));
        if (!nt) {
          continue;
        }
        tmp = nt;
        tmp_cap = need;
      }
      for (int row = 0; row < rh; row++) {
        memcpy(tmp + (size_t)row * rw, r->pix + (size_t)(ry + row) * r->w + rx,
               (size_t)rw * sizeof(uint32_t));
      }
      bits = tmp;
    }
    else {
      bits = r->pix + (size_t)ry * r->w; /* full-width band is contiguous */
    }
    bmi.bmiHeader.biWidth = rw;
    bmi.bmiHeader.biHeight = -rh;
    SetDIBitsToDevice(hdc, rx, ry, (DWORD)rw, (DWORD)rh, 0, 0, 0, (UINT)rh, bits, &bmi,
                      DIB_RGB_COLORS);
  }
  free(tmp);
  GdiFlush();
  ReleaseDC(hwnd, hdc);
  return 1;
}
#else
int BUI_raster_present_native(BuiRaster *r, void *window_handle, int full)
{
  if (r->gpu) {
    if (!bui_gpu_present(r->gpu, full)) {
      gpu_drop(r);
    }
    return 1;
  }
  (void)window_handle;
  (void)full;
  return 0;
}
#endif

/** \} */
