/* Images: Blender's imbuf reduced to what a UI draws, shared by every
 * binding through BUI_kit.h.
 *
 *  imbuf.c       BKImBuf (ImBuf): one allocation per image, premultiplied
 *                0xAARRGGBB pixels (the raster's and the GPU atlas' layout),
 *                box-filter downscale (IMB_scale), EXIF orientation, fit.
 *  filetype.c    the format table (IMB_FILE_TYPES) and the load path
 *                (IMB_load_image_from_memory): probe, decode at the size
 *                drawn, scale, crop.
 *  format_*.c    one decoder per format (Wuffs, libjpeg-turbo, libwebp,
 *                Radiance HDR, PNM, nanosvg).
 *  cache.c       decoded images by (source, size, fit), within a memory
 *                budget (Blender's moviecache / TextCache generations).
 *  loader.c      the worker pool: requests by priority, cancellation,
 *                results handed to the UI thread.
 *  draw.c        drawing images on nodes and canvas items.
 *
 * Memory: every block comes from MEM_* (MEM_get_memory_in_use counts it).
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */
#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "BUI_kit.h"

#ifndef BK_MIN /* as kit_intern.h */
#  define BK_MIN(a, b) ((a) < (b) ? (a) : (b))
#  define BK_MAX(a, b) ((a) > (b) ? (a) : (b))
#  define BK_CLAMP(v, a, b) ((v) < (a) ? (a) : ((v) > (b) ? (b) : (v)))
#endif

/* Largest side and total pixels a decode may produce (validated at the
 * format boundary, before any allocation). */
#define BK_IMB_MAX_SIDE 16384
#define BK_IMB_MAX_PIXELS ((size_t)1 << 28) /* frames * w * h: 1 GiB of pixels */

struct BKImBuf {
  int x, y;       /* size in pixels (ImBuf::x, ::y) */
  int frames;     /* >= 1 */
  int ftype;      /* BK_IMAGE_* it was decoded from (BK_IMAGE_VIDEO: a movie frame) */
  int format;     /* BUI_PIXMAP_*: ARGB, or NV12 (one frame; `rect` holds its planes) */
  uint64_t stream; /* frames of one playing movie share it (one GPU texture); 0 else */
  int src_x, src_y; /* intrinsic size of the source (after EXIF orientation) */
  float crop[4];    /* the part to draw (x, y, w, h in these pixels): the visible
                     * region of a fitted decode, sub-pixel exact; else all */
  int loops;      /* animation repeat count, 0 = forever */
  uint64_t key;   /* unique per decoded image (never reused): GPU atlas key */
  size_t size;    /* bytes of this allocation (the cache budget) */
  int *delays;    /* `frames` frame durations in ms (NULL for one frame) */
  uint32_t *rect; /* frames * x * y premultiplied 0xAARRGGBB, top-down rows */
};

/* -- imbuf.c ------------------------------------------------------------------ */

/* One block: header, delays, pixels (uninitialized). NULL when the size is
 * out of range or allocation fails. */
BKImBuf *imb_alloc(int x, int y, int frames);
/* One NV12 frame (`format` BUI_PIXMAP_NV12_*; x and y even): luma, then chroma. */
BKImBuf *imb_alloc_nv12(int x, int y, int format);
/* A fresh unique key (BKImBuf keys, movie streams). */
uint64_t imb_new_key(void);
/* What the GPU renderers draw for `ib`. */
static inline BuiPixmap imb_pixmap(const BKImBuf *ib)
{
  const BuiPixmap pm = {ib->rect, ib->x, ib->y, ib->key, ib->format, ib->stream};
  return pm;
}
static inline uint32_t *imb_frame(const BKImBuf *ib, int frame)
{
  return ib->rect + (size_t)frame * ib->x * ib->y;
}
/* Box-filter (area average) resample of every frame to w x h, on
 * premultiplied pixels: the source is freed. Downscales only. */
BKImBuf *imb_scale(BKImBuf *ib, int w, int h);
/* The x, y, w, h part of every frame (freed source). */
BKImBuf *imb_crop(BKImBuf *ib, int x, int y, int w, int h);
/* EXIF orientation 1..8 applied to every frame (freed source). */
BKImBuf *imb_orient(BKImBuf *ib, int orientation);
/* A fresh key: the pixels changed in place (GPU atlases upload again). */
void imb_rekey(BKImBuf *ib);
/* Straight RGBA (8 bits, any stride) <-> premultiplied 0xAARRGGBB rows. */
void imb_premultiply_rgba(uint32_t *dst, const uint8_t *rgba, int n);
void imb_unpremultiply(uint8_t *rgba, const uint32_t *src, int n);
/* Rows y0 .. y0 + rows of a frame as straight RGBA (any format). */
bool imb_rgba_rows(const BKImBuf *ib, int frame, int y0, int rows, uint8_t *dst);

/* Where an image of intrinsic size iw x ih shows in a bw x bh box (CSS
 * object-fit / object-position): `dst` is the visible rect inside the box
 * (box-relative), `src` the part of the image it shows (intrinsic pixels).
 * A box of 0 x 0 is the intrinsic size. */
typedef struct BKImageFit {
  int fit;          /* BK_FIT_* */
  int box_w, box_h; /* 0 x 0: intrinsic */
  float pos_x, pos_y; /* object-position, 0..1 (0.5 = centered) */
  int frame;        /* animations: 0 plays (all frames); n > 0 shows frame n - 1 */
} BKImageFit;
void bk_image_fit(const BKImageFit *f, int iw, int ih, float dst[4], float src[4]);
/* Contain within max_w x max_h (0: that side free; both 0: intrinsic). */
BKImageFit imb_fit_within(int max_w, int max_h);

/* How a decode becomes what is drawn: the whole image at `fw` x `fh` (shown
 * orientation, never above intrinsic), then the visible part `src`. */
typedef struct BKImPlan {
  int iw, ih, orient;
  int fw, fh;
  float src[4];
} BKImPlan;
/* False when nothing of the image shows. */
bool imb_plan(const BKImageFit *t, int iw, int ih, int orient, BKImPlan *r_plan);
/* Scale (to fw x fh), orient and crop a decode (freed) to the plan. */
BKImBuf *imb_finish(BKImBuf *ib, const BKImPlan *p);

/* -- filetype.c ----------------------------------------------------------------- */

typedef struct BKImFileType {
  int ftype;        /* BK_IMAGE_* */
  const char *name; /* "png" */
  bool (*is_a)(const uint8_t *mem, size_t size);
  /* Size and frame count from the header; no pixels. */
  bool (*load_header)(const uint8_t *mem, size_t size, BKImageInfo *r_info);
  /* Decode at least `want_w` x `want_h` (a hint: 0 = full size; formats that
   * decode smaller for free, JPEG DCT scaling and SVG, use it). The result
   * is in the file's orientation; src_x/src_y are set by the caller. */
  BKImBuf *(*load)(const uint8_t *mem, size_t size, int want_w, int want_h);
} BKImFileType;

extern const BKImFileType BK_IMB_FILE_TYPES[];
extern const int BK_IMB_FILE_TYPES_NUM;

const BKImFileType *imb_file_type(const uint8_t *mem, size_t size);
/* The whole decode path for one target: probe, load, scale, crop, orient. */
BKImBuf *imb_load(const uint8_t *mem, size_t size, const BKImageFit *target);
/* Reads a whole file (MEM_mallocN'd), or its first `max` bytes (0: all). */
uint8_t *imb_read_file(const char *path, size_t max, size_t *r_size);

/* -- anim.c: animated media, one frame at a time --------------------------------------
 * GIF, animated WebP and video share one codec interface (Blender's anim /
 * MovieReader: open, next frame, seek, close) and one player (play.c). */

typedef struct BKImAnimInfo {
  int w, h;          /* the frames' size: the canvas (in-memory codecs) or what a
                      * streamed codec decodes to (`output`: even, about the size asked) */
  int src_w, src_h;  /* intrinsic (display) size */
  int format;        /* BUI_PIXMAP_* of the frames */
  int loops;         /* plays the file asks for, 0 = forever */
  double duration;   /* seconds, 0 when unknown */
} BKImAnimInfo;

typedef struct BKImAnimCodec {
  const char *name;
  /* Streamed codecs (video) read the file themselves, by path, as it plays:
   * nothing is loaded whole. Others get the whole file in memory. */
  bool streamed;
  /* The file starts like one this codec reads (a few dozen bytes). */
  bool (*is_a)(const uint8_t *head, size_t size);
  /* `path` (streamed) or `mem` (kept by the caller while open). Fills the
   * info: a streamed codec its intrinsic size, duration and format. */
  void *(*open)(const char *path, const uint8_t *mem, size_t size, BKImAnimInfo *r_info);
  /* Streamed codecs: decode whole frames at about w x h from now on (the
   * size drawn: video scales on the decoder, in hardware where there is
   * one), setting info->w, h. In-memory codecs: NULL (their canvas). */
  bool (*output)(void *state, int w, int h, BKImAnimInfo *r_info);
  /* 1 with the next frame in `dst` (the info's size and format), its time
   * and duration in seconds (pts < 0: unknown, frames follow each other);
   * 0 past the last; -1 on errors. */
  int (*next)(void *state, BKImBuf *dst, double *r_pts, double *r_dur);
  /* The next frame is the one showing at `t`, or one before it (a keyframe;
   * in-memory codecs restart): anim.c skips forward to `t`. */
  bool (*seek)(void *state, double t);
  size_t (*bytes)(const void *state); /* decoder state (estimate for OS decoders) */
  void (*close)(void *state);
} BKImAnimCodec;
extern const BKImAnimCodec imb_gif_codec, imb_webp_codec;
/* Video backends (movie_*.c), each compiled when the build has it. */
extern const BKImAnimCodec imb_movie_mf, imb_movie_avf, imb_movie_gst;
/* Those compiled in (movie.c: the build's choice), tried in order; NULL-ended. */
extern const BKImAnimCodec *const imb_movie_codecs[];
/* The first bytes look like a movie container (MP4/MOV, WebM/MKV, AVI, ASF,
 * Ogg, MPEG-PS/TS, FLV). */
bool imb_is_a_movie(const uint8_t *head, size_t size);
/* The in-memory codec for an animation (GIF always, animated WebP). */
const BKImAnimCodec *imb_anim_codec(const uint8_t *mem, size_t size);

typedef struct BKImAnim BKImAnim;
/* Takes `file` (MEM-allocated; freed with the animation, or now on failure). */
BKImAnim *imb_anim_open(uint8_t *file, size_t size, const BKImageFit *target);
/* A movie (streamed codecs; NULL when none reads the file). */
BKImAnim *imb_movie_open(const char *path, const BKImageFit *target);
void imb_anim_free(BKImAnim *a);
const BKImAnimInfo *imb_anim_info(const BKImAnim *a);
int imb_anim_frames(const BKImAnim *a); /* from the header (at least what was decoded) */
size_t imb_anim_bytes(const BKImAnim *a); /* its decoder state and file */
/* Frames to `target` from now on (a movie reopens its decoder at the new
 * size, where it was). False when it can't (the old size stays). */
bool imb_anim_retarget(BKImAnim *a, const BKImageFit *target);
/* The next frame at the target size, wrapping (`r_wrapped`) after the last.
 * `reuse` (taken; NULL for none): a frame no longer shown, which a movie
 * decodes into when it fits (no allocation per frame). */
BKImBuf *imb_anim_next(BKImAnim *a, BKImBuf *reuse, double *r_pts, double *r_dur, bool *r_wrapped);
/* The next imb_anim_next returns the frame showing at `t` seconds. */
void imb_anim_seek(BKImAnim *a, double t);
/* Frame `frame` alone, or every frame (synchronous API; at most 128 MiB). */
BKImBuf *imb_anim_frame(const uint8_t *mem, size_t size, const BKImageFit *target, int frame);
BKImBuf *imb_anim_all(const uint8_t *mem, size_t size, const BKImageFit *target);

/* -- format_*.c: the decoders ------------------------------------------------------ */

bool imb_is_a_png(const uint8_t *mem, size_t size);
bool imb_is_a_gif(const uint8_t *mem, size_t size);
bool imb_is_a_bmp(const uint8_t *mem, size_t size);
bool imb_is_a_tga(const uint8_t *mem, size_t size);
bool imb_is_a_qoi(const uint8_t *mem, size_t size);
bool imb_header_wuffs(const uint8_t *mem, size_t size, BKImageInfo *r_info);
BKImBuf *imb_load_wuffs(const uint8_t *mem, size_t size, int want_w, int want_h);

bool imb_is_a_pnm(const uint8_t *mem, size_t size);
bool imb_header_pnm(const uint8_t *mem, size_t size, BKImageInfo *r_info);
BKImBuf *imb_load_pnm(const uint8_t *mem, size_t size, int want_w, int want_h);

bool imb_is_a_jpeg(const uint8_t *mem, size_t size);
bool imb_header_jpeg(const uint8_t *mem, size_t size, BKImageInfo *r_info);
BKImBuf *imb_load_jpeg(const uint8_t *mem, size_t size, int want_w, int want_h);
/* EXIF orientation (1..8, 1 when absent) of a JPEG. */
int imb_jpeg_orientation(const uint8_t *mem, size_t size);

bool imb_is_a_webp(const uint8_t *mem, size_t size);
bool imb_webp_animated(const uint8_t *mem, size_t size); /* VP8X animation flag */
bool imb_header_webp(const uint8_t *mem, size_t size, BKImageInfo *r_info);
BKImBuf *imb_load_webp(const uint8_t *mem, size_t size, int want_w, int want_h);

bool imb_is_a_hdr(const uint8_t *mem, size_t size);
bool imb_header_hdr(const uint8_t *mem, size_t size, BKImageInfo *r_info);
BKImBuf *imb_load_hdr(const uint8_t *mem, size_t size, int want_w, int want_h);

bool imb_is_a_svg(const uint8_t *mem, size_t size);
/* Blender's icons (kit_draw.c) and SVG files: rasterized within want_w x want_h. */
BKImBuf *imb_svg_rasterize(const char *svg, int want_w, int want_h);
bool imb_header_svg(const uint8_t *mem, size_t size, BKImageInfo *r_info);
BKImBuf *imb_load_svg(const uint8_t *mem, size_t size, int want_w, int want_h);

/* -- cache.c / loader.c / draw.c: the kit side (UI thread) --------------------------- */

/* A source string interned once (hash + copy); nodes and items keep these. */
typedef struct BKImageSource BKImageSource;
BKImageSource *bk_image_source_get(const char *src); /* adds a reference */
void bk_image_source_release(BKImageSource *s);
const char *bk_image_source_path(const BKImageSource *s);
/* Intrinsic size from the file header (read once, then cached). */
bool bk_image_source_info(BKImageSource *s, BKImageInfo *r_info);

/* Decode priorities: what is drawn first, then prefetches. */
enum { BK_IMAGE_PRIORITY_PREFETCH = 0, BK_IMAGE_PRIORITY_VISIBLE = 1 };

/* Drawing passes (kit_core.c BK_render): images requested inside a pass
 * belong to `ctx`; a pending image that a pass over its whole rectangle
 * didn't ask for again is no longer shown there, and a request nobody shows
 * is cancelled. */
void bk_image_frame_begin(void); /* a drawn frame: next generation, evict, cancel */
void bk_image_pass_begin(BKContext *ctx, int x0, int y0, int x1, int y1);
void bk_image_pass_end(void);
void bk_image_forget_context(BKContext *ctx);
/* Animations: frames whose time came are swapped in (their rectangles
 * redraw), the next ones decoded ahead; only for what is on screen. */
void bk_image_tick(void);
/* When `ctx` next needs bk_image_tick (bk_time seconds; 0: never). */
double bk_image_next_due(BKContext *ctx);
/* The decoded image for (source, target) if cached (used this generation);
 * otherwise NULL, with a decode queued (visible priority) whose arrival
 * redraws `rect` (window pixels, x0 y0 x1 y1) of the pass' context. */
const BKImBuf *bk_image_cache_get(BKImageSource *s, const BKImageFit *target, const int rect[4]);
void bk_image_cache_prefetch(BKImageSource *s, const BKImageFit *target);

/* A private entry: one player's (a <video>), outside the shared table and
 * the LRU (never evicted), freed by its owner (video.c). It loads at its
 * first draw, with `play` (taken) as its clock; `notify` hears every state
 * change of the clock (loaded, ended, error) on the UI thread. */
typedef struct BKImEntry BKImEntry;
typedef struct BKImPlay BKImPlay; /* play.c */
BKImEntry *bk_image_entry_new(BKImageSource *s, BKImPlay *play, void (*notify)(void *owner),
                              void *owner);
void bk_image_entry_free(BKImEntry *e);
/* As bk_image_cache_get; the entry follows the size it is drawn at. */
const BKImBuf *bk_image_entry_get(BKImEntry *e, const BKImageFit *target, const int rect[4]);
BKImPlay *bk_image_entry_play(BKImEntry *e);
/* A control changed the clock: step now, redraw where it shows. */
void bk_image_entry_changed(BKImEntry *e);

/* play.c: the one clock of everything that plays (GIF, WebP, video), owned
 * by its cache entry. Frames advance only after the shown one was drawn. */
typedef enum BKImPlayState {
  BK_PLAY_LOADING = 0, /* no frame yet */
  BK_PLAY_PLAYING,
  BK_PLAY_PAUSED,
  BK_PLAY_ENDED, /* the last play's last frame shows */
  BK_PLAY_ERROR,
} BKImPlayState;
/* `loops` overrides the file's play count (-1: the file's; 0: forever). */
BKImPlay *imb_play_new(bool paused, int loops);
void imb_play_free(BKImPlay *p);
size_t imb_play_bytes(const BKImPlay *p);
/* The first frame arrived with its decoder (taken). */
void imb_play_start(BKImPlay *p, BKImAnim *anim, double pts, double dur);
void imb_play_fail(BKImPlay *p); /* the file didn't open */
/* A step request to make: the decoder (NULL: nothing to decode now), the
 * buffer to decode into (or NULL) and where to seek first (< 0: no seek).
 * The step owns them until it comes back (step_done / step_cancelled). */
BKImAnim *imb_play_take_step(BKImPlay *p, BKImBuf **r_reuse, double *r_seek);
/* The step came back: the decoder, and the next frame (NULL: an error). */
void imb_play_step_done(BKImPlay *p, BKImAnim *anim, BKImBuf *next, double pts, double dur,
                        bool wrapped);
void imb_play_step_cancelled(BKImPlay *p, BKImAnim *anim, BKImBuf *reuse); /* nothing decoded */
void imb_play_drawn(BKImPlay *p, double now); /* the shown frame reached the screen */
double imb_play_due(const BKImPlay *p);       /* when to advance (0: not waiting on time) */
/* The frame to show from `now`, or NULL; the one it replaces goes back
 * through imb_play_recycle. */
BKImBuf *imb_play_advance(BKImPlay *p, double now);
void imb_play_recycle(BKImPlay *p, BKImBuf *old); /* kept for a movie's next decode, else freed */
/* Controls (the <video> element's). */
void imb_play_pause(BKImPlay *p, bool paused);
void imb_play_seek(BKImPlay *p, double t);
void imb_play_set_rate(BKImPlay *p, double rate); /* > 0 */
void imb_play_set_loops(BKImPlay *p, int loops);  /* 0: forever, -1: the file's */
BKImPlayState imb_play_state(const BKImPlay *p);
double imb_play_time(const BKImPlay *p);     /* seconds, of the frame shown */
double imb_play_duration(const BKImPlay *p); /* seconds, 0 when unknown */
bool imb_play_size(const BKImPlay *p, int *r_w, int *r_h); /* intrinsic, once loaded */

/* draw.c: what a node or canvas item shows. One of an owned copy (set from
 * RGBA), a borrowed BKImBuf (the host keeps it alive) or a source decoded
 * through the cache. */
typedef enum BKImageRefKind {
  BK_IMREF_NONE = 0,
  BK_IMREF_OWNED,
  BK_IMREF_BORROWED,
  BK_IMREF_SOURCE,
  BK_IMREF_VIDEO, /* a BKVideo (borrowed: its owner frees it) */
} BKImageRefKind;
typedef struct BKImageRef {
  BKImageRefKind kind;
  union {
    BKImBuf *owned;
    const BKImBuf *borrowed;
    BKImageSource *source;
    BKVideo *video;
  } u;
} BKImageRef;
void bk_image_ref_clear(BKImageRef *ref);
/* Straight RGBA, copied (premultiplied); same-size updates reuse the buffer. */
void bk_image_ref_set_rgba(BKImageRef *ref, const unsigned char *rgba, int w, int h);
void bk_image_ref_set_imbuf(BKImageRef *ref, const BKImBuf *ib);
void bk_image_ref_set_source(BKImageRef *ref, const char *src);
void bk_image_ref_set_video(BKImageRef *ref, BKVideo *v);

/* video.c: a BKVideo's frame for drawing at `target` (NULL until the first
 * frame; then its poster, if any, is what shows), and its intrinsic size. */
const BKImBuf *bk_video_frame(BKVideo *v, const BKImageFit *target, const int rect[4]);
bool bk_video_size(const BKVideo *v, int *r_w, int *r_h);
/* Intrinsic size (a source reads its header once). */
bool bk_image_ref_size(const BKImageRef *ref, int *r_w, int *r_h);

/* An image placed for drawing: `dst` in window pixels, `src` in its pixels. */
typedef struct BKImageDraw {
  const BKImBuf *ib;
  float dst[4], src[4];
} BKImageDraw;
/* Fits `ref` into `box` (x, y, w, h window pixels) with `fit` (BK_FIT_*);
 * false when there is nothing to draw (yet: a source still decoding). */
bool bk_image_ref_resolve(const BKImageRef *ref, int fit, int frame, const float box[4],
                          BKImageDraw *r);
/* `radius`: the drawn image's corners rounded (0: square) */
void bk_image_draw_resolved(BuiRaster *r, const BKImageDraw *d, float alpha, float radius);
/* resolve + draw into the frame being rendered */
void bk_image_ref_draw(const BKImageRef *ref, int fit, int frame, const float box[4], float alpha);
void bk_imbuf_draw(const BKImBuf *ib, float x, float y, float w, float h, float alpha);

/* loader.c: a finished request travels back to the cache through these. */
typedef struct BKImRequest BKImRequest;
struct BKImRequest {
  BKImRequest *prev, *next; /* loader queue links */
  BKImageSource *source; /* referenced while queued */
  BKImageFit target;
  int priority;
  unsigned wanted;  /* the cache generation that last asked */
  int state;        /* BK_IMREQ_* */
  BKImBuf *result;  /* NULL when decoding failed */
  void *entry;      /* the cache entry waiting for it (cache.c) */
  BKImAnim *anim;   /* a step: the animation to decode the next frame of; a
                     * first load: set when the file plays (frames > 1, movies) */
  BKImBuf *reuse;   /* a step: the frame to decode into (taken) */
  double seek;      /* a step: seek there first (seconds; < 0: no seek) */
  bool step;        /* the next frame of something playing (else a first load) */
  bool movie;       /* a first load of a movie (streamed codecs, imb_movie_open) */
  double pts, dur;  /* the result's time and duration (seconds), and whether */
  bool wrapped;     /* it starts a new play */
};
enum { BK_IMREQ_QUEUED, BK_IMREQ_RUNNING, BK_IMREQ_DONE, BK_IMREQ_CANCELLED };
/* Queue `req` (ownership moves to the loader until it comes back). */
void bk_loader_push(BKImRequest *req);
/* Requests that finished or were cancelled; ownership returns to the caller. */
BKImRequest *bk_loader_take_done(void);
/* Remove a queued request (it comes back through take_done as cancelled);
 * one already running finishes. */
void bk_loader_cancel(BKImRequest *req);
/* Re-sort a queued request after its priority or generation changed. */
void bk_loader_touch(BKImRequest *req, int priority, unsigned wanted);
/* Requests queued or running, and the time workers spent decoding (seconds). */
int bk_loader_pending(double *r_decode_s);
void bk_loader_exit(void);
/* Called (on a worker thread) when results are ready. */
void bk_loader_notify(void);
