/* Native window host (macOS, Cocoa). Other platforms: kit_window_win32.c,
 * kit_window_x11.c, kit_window_null.c.
 *
 * The same BK_window_* API as kit_window_x11.c, on AppKit, ported from
 * Blender's GHOST Cocoa backend (intern/ghost/intern/GHOST_SystemCocoa.mm,
 * GHOST_WindowCocoa.mm, GHOST_WindowViewCocoa.hh): NSApplication setup, the
 * nextEventMatchingMask loop, key / modifier / mouse / scroll conversion,
 * Retina backing scale, the window delegate, NSCursor, NSPasteboard and
 * pointer warping. Redraw requests coalesce into one render per loop turn
 * and the damaged rectangles of the CPU raster are invalidated on an NSView
 * whose drawRect: draws the raster as a CGImage at the backing scale.
 * Timers, the cross-thread wake (a CFRunLoopSource) and frame pacing run in
 * the loop, like the X11 host.
 *
 * Units: like Win32 / X11, the kit sees pixels. A window of logical size
 * w x h has w * scale / px points (px = backingScaleFactor, scale = the UI
 * scale = px unless BUI_UI_SCALE is set), so the raster is w * scale pixels
 * wide and mouse positions are points * px. Screen-level geometry in wm /
 * wm_get ("pos", "screen", "pointer") is in points, top-left origin of the
 * primary screen, as Tk reports it on macOS.
 *
 * Memory: manual retain / release (compiled with -fno-objc-arc), as GHOST.
 *
 * Threads: AppKit only works on the main thread. BK_window_supported() is 0
 * on any other thread (hosts then fall back to Tk / headless), and every
 * BK_window_* call except BK_window_wake / BK_window_quit must come from the
 * main thread.
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */

#include "kit_intern.h"

#ifdef BUI_WINDOW_COCOA

#  import <Cocoa/Cocoa.h>
#  include <CoreGraphics/CoreGraphics.h>

#  include <float.h>
#  include <math.h>
#  include <pthread.h>
#  include <stdio.h>
#  include <stdlib.h>
#  include <string.h>
#  include <time.h>

#  define MAX_TIMERS 64

/* Our NSEventTypeApplicationDefined events (loop nudges), told apart from
 * other code's (Tk posts its own) by subtype and data1. */
#  define BK_NUDGE_SUBTYPE ((short)0x4255)
#  define BK_NUDGE_DATA ((NSInteger)0x42554921)

/* Trackpad scrolling: points of precise scroll per wheel notch. */
#  define BK_SCROLL_STEP 12.0

/* Mac virtual key codes (HIToolbox/Events.h kVK_*), the ones GHOST's
 * convertKey maps; spelled out so this file needs no Carbon header. */
enum {
  BK_kVK_Return = 0x24,
  BK_kVK_Tab = 0x30,
  BK_kVK_Delete = 0x33, /* backspace */
  BK_kVK_Escape = 0x35,
  BK_kVK_F17 = 0x40,
  BK_kVK_KeypadDecimal = 0x41,
  BK_kVK_KeypadMultiply = 0x43,
  BK_kVK_KeypadPlus = 0x45,
  BK_kVK_KeypadDivide = 0x4B,
  BK_kVK_KeypadEnter = 0x4C,
  BK_kVK_KeypadMinus = 0x4E,
  BK_kVK_F18 = 0x4F,
  BK_kVK_F19 = 0x50,
  BK_kVK_KeypadEquals = 0x51,
  BK_kVK_Keypad0 = 0x52,
  BK_kVK_Keypad1 = 0x53,
  BK_kVK_Keypad2 = 0x54,
  BK_kVK_Keypad3 = 0x55,
  BK_kVK_Keypad4 = 0x56,
  BK_kVK_Keypad5 = 0x57,
  BK_kVK_Keypad6 = 0x58,
  BK_kVK_Keypad7 = 0x59,
  BK_kVK_F20 = 0x5A,
  BK_kVK_Keypad8 = 0x5B,
  BK_kVK_Keypad9 = 0x5C,
  BK_kVK_F5 = 0x60,
  BK_kVK_F6 = 0x61,
  BK_kVK_F7 = 0x62,
  BK_kVK_F3 = 0x63,
  BK_kVK_F8 = 0x64,
  BK_kVK_F9 = 0x65,
  BK_kVK_F11 = 0x67,
  BK_kVK_F13 = 0x69,
  BK_kVK_F16 = 0x6A,
  BK_kVK_F14 = 0x6B,
  BK_kVK_F10 = 0x6D,
  BK_kVK_F12 = 0x6F,
  BK_kVK_F15 = 0x71,
  BK_kVK_Help = 0x72, /* Insert on PC keyboards */
  BK_kVK_Home = 0x73,
  BK_kVK_PageUp = 0x74,
  BK_kVK_ForwardDelete = 0x75,
  BK_kVK_F4 = 0x76,
  BK_kVK_End = 0x77,
  BK_kVK_F2 = 0x78,
  BK_kVK_PageDown = 0x79,
  BK_kVK_F1 = 0x7A,
  BK_kVK_LeftArrow = 0x7B,
  BK_kVK_RightArrow = 0x7C,
  BK_kVK_DownArrow = 0x7D,
  BK_kVK_UpArrow = 0x7E,
};

typedef struct BKCocoaTimer {
  int ident; /* > 0 user timer, -1 the tooltip timer, 0 free */
  double due;
} BKCocoaTimer;

@class BUIKitView;
@class BUIKitWindowDelegate;

struct BKWindow {
  NSWindow *nswin;                /* retained; nil once closed */
  BUIKitView *view;               /* the content view (owned by nswin) */
  BUIKitWindowDelegate *delegate; /* retained */
  BKContext *ctx;
  BKHost prev; /* the host's own callbacks (emit) */
  BuiRaster *raster;
  BKWindowCallbacks cb;
  int render_pending, frames, cursor, tip_id, next_timer, full;
  int want_frame, frame_vsync;
  double frame_start; /* when the last animation frame began */
  double next_frame;  /* frame pacing: no render before this (ms) */
  float scale;        /* UI scale (BK_set_scale, wm_get "scale") */
  float px;           /* backing pixels per point (2 on Retina) */
  int w, h;           /* view size in pixels */
  int min_w, min_h, max_w, max_h; /* content size limits (logical px; 0 = none) */
  int fixed_w, fixed_h;           /* axes the user may not resize (Tk's resizable) */
  int override, disabled, shown;
  NSWindowStyleMask style;    /* framed style ("override" 0 restores it) */
  NSEventModifierFlags flags; /* modifiers at the last flagsChanged */
  double scroll_x, scroll_y;  /* trackpad scroll not yet sent (points) */
  BKCocoaTimer timers[MAX_TIMERS];
  struct BKWindow *next;
};

static BKWindow *g_windows = NULL;
static int g_open = 0, g_quit = 0, g_keep_alive = 0;
static int (*g_stop)(void) = NULL;
static int g_app_ready = 0;
static int g_turn_depth = 0;
static int g_nudge_pending = 0;
static int g_cursor_hidden = 0;
static int g_ignore_momentum = 0;
static double g_warp_time = -1.0;      /* systemUptime of the last warp */
static NSEvent *g_keyup_done = nil;    /* key-up handled before sendEvent (not retained) */
static NSMutableArray *g_dead = nil;   /* closed windows, released between turns */
static id g_app_delegate = nil;
static void (*g_wake_cb)(void) = NULL;
static CFRunLoopSourceRef g_wake_src = NULL;
static int g_wake_pending = 0; /* atomic */

static void request_close(BKWindow *win);
static void close_window(BKWindow *win);
static void update_size(BKWindow *win);
static void on_backing_changed(BKWindow *win);
static void render_window(BKWindow *win);
static void apply_cursor(BKWindow *win);
static void show_cursor(int visible);
static void post_nudge(void);
static void handle_key(BUIKitView *v, NSEvent *ev, int press);
static void handle_flags(BUIKitView *v, NSEventModifierFlags flags);

static double now_ms(void)
{
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return ts.tv_sec * 1000.0 + ts.tv_nsec / 1e6;
}

/* BUI_UI_SCALE, else the backing scale (Win32's dpi / 96). */
static float env_scale(void)
{
  const char *env = getenv("BUI_UI_SCALE");
  return (env && atof(env) > 0) ? (float)atof(env) : 0.0f;
}

static float ui_scale(float px)
{
  const float env = env_scale();
  return env > 0.0f ? env : BK_MAX(1.0f, px);
}

/* Height of the primary screen: flips Cocoa's bottom-left screen origin to
 * the top-left one of Quartz (GHOST_SystemCocoa::setMouseCursorPosition). */
static CGFloat primary_height(void)
{
  NSScreen *primary = [[NSScreen screens] firstObject];
  return primary ? NSMaxY([primary frame]) : 0.0;
}

/* -- the AppKit classes -------------------------------------------------------------- */

/* GHOST's BlenderWindow: borderless ("override") windows still take keys. */
@interface BUIKitNSWindow : NSWindow
@end

@implementation BUIKitNSWindow
- (BOOL)canBecomeKeyWindow
{
  return YES;
}
- (BOOL)canBecomeMainWindow
{
  return YES;
}
@end

/* GHOST_WindowViewCocoa.hh: the view takes the input and draws the frame. */
@interface BUIKitView : NSView {
@public
  BKWindow *bk; /* NULL once the window closed */
  NSTrackingArea *tracking;
}
@end

/* GHOST_WindowCocoa.mm BlenderWindowDelegate. */
@interface BUIKitWindowDelegate : NSObject <NSWindowDelegate> {
@public
  BKWindow *bk;
}
@end

/* GHOST_SystemCocoa.mm CocoaAppDelegate. */
@interface BUIKitAppDelegate : NSObject <NSApplicationDelegate>
@end

static BUIKitView *view_of(NSWindow *w)
{
  NSView *cv = [w contentView];
  return [cv isKindOfClass:[BUIKitView class]] ? (BUIKitView *)cv : nil;
}

static int mods_of(NSEventModifierFlags f)
{
  /* Command is Ctrl, as Blender's macOS keymaps use it for Ctrl's shortcuts; BK_CMD and
   * BK_CONTROL_KEY say which key it was */
  int m = 0;
  if (f & NSEventModifierFlagShift) {
    m |= BK_SHIFT;
  }
  if (f & NSEventModifierFlagControl) {
    m |= BK_CTRL | BK_CONTROL_KEY;
  }
  if (f & NSEventModifierFlagCommand) {
    m |= BK_CTRL | BK_CMD;
  }
  if (f & NSEventModifierFlagOption) {
    m |= BK_ALT;
  }
  if ([NSEvent pressedMouseButtons] & 1) {
    m |= BK_BUTTON1;
  }
  return m;
}

/* An event's position in the view, in raster pixels (y down). */
static void event_px(BKWindow *win, NSEvent *ev, int *r_x, int *r_y)
{
  const NSPoint p = [win->view convertPoint:[ev locationInWindow] fromView:nil];
  *r_x = (int)floor(p.x * win->px);
  *r_y = (int)floor(p.y * win->px);
}

static CGColorSpaceRef srgb(void)
{
  static CGColorSpaceRef cs = NULL;
  if (!cs) {
    cs = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
  }
  return cs;
}

@implementation BUIKitView

- (void)dealloc
{
  [tracking release];
  [super dealloc];
}

- (BOOL)isFlipped
{
  return YES; /* y down, like the raster */
}

- (BOOL)isOpaque
{
  return YES;
}

- (BOOL)acceptsFirstResponder
{
  return YES;
}

- (BOOL)acceptsFirstMouse:(NSEvent *)event
{
  return YES;
}

/* Enter / exit and cursor updates over the whole view. */
- (void)updateTrackingAreas
{
  if (tracking) {
    [self removeTrackingArea:tracking];
    [tracking release];
    tracking = nil;
  }
  tracking = [[NSTrackingArea alloc]
      initWithRect:NSZeroRect
           options:(NSTrackingMouseEnteredAndExited | NSTrackingCursorUpdate |
                    NSTrackingActiveAlways | NSTrackingInVisibleRect)
             owner:self
          userInfo:nil];
  [self addTrackingArea:tracking];
  [super updateTrackingAreas];
}

- (void)cursorUpdate:(NSEvent *)event
{
  if (bk) {
    apply_cursor(bk);
  }
  else {
    [super cursorUpdate:event];
  }
}

/* The raster as a CGImage: 0x00RRGGBB words are little-endian XRGB. */
- (void)drawRect:(NSRect)dirty
{
  CGContextRef cg = [[NSGraphicsContext currentContext] CGContext];
  if (!cg) {
    return;
  }
  const NSRect b = [self bounds];
  BuiRaster *r = bk ? bk->raster : NULL;
  const int iw = r ? BUI_raster_width(r) : 0, ih = r ? BUI_raster_height(r) : 0;
  const CGFloat px = bk ? bk->px : 1.0;
  const CGFloat wpt = iw / px, hpt = ih / px;
  if (!r || iw <= 1 || ih <= 1 || wpt < b.size.width || hpt < b.size.height) {
    CGContextSetRGBFillColor(cg, 0.188, 0.188, 0.188, 1.0); /* Blender's background */
    CGContextFillRect(cg, NSRectToCGRect(dirty));
  }
  if (!r || iw <= 1 || ih <= 1) {
    return;
  }
  CGDataProviderRef prov = CGDataProviderCreateWithData(
      NULL, BUI_raster_pixels(r), (size_t)iw * (size_t)ih * 4, NULL);
  CGImageRef img = CGImageCreate((size_t)iw,
                                 (size_t)ih,
                                 8,
                                 32,
                                 (size_t)iw * 4,
                                 srgb(),
                                 (CGBitmapInfo)kCGImageAlphaNoneSkipFirst |
                                     kCGBitmapByteOrder32Little,
                                 prov,
                                 NULL,
                                 false,
                                 kCGRenderingIntentDefault);
  CGDataProviderRelease(prov);
  if (!img) {
    return;
  }
  /* AppKit clips to the invalidated rects (the raster's damage), so drawing
   * the whole image only updates those */
  CGContextSaveGState(cg);
  CGContextSetInterpolationQuality(cg, kCGInterpolationNone);
  CGContextSetBlendMode(cg, kCGBlendModeCopy);
  CGContextTranslateCTM(cg, 0, hpt); /* the view is flipped; images draw y up */
  CGContextScaleCTM(cg, 1.0, -1.0);
  CGContextDrawImage(cg, CGRectMake(0, 0, wpt, hpt), img);
  CGContextRestoreGState(cg);
  CGImageRelease(img);
}

/* -- mouse (GHOST_SystemCocoa::handleMouseEvent) -- */

- (void)mouseDown:(NSEvent *)ev
{
  if (!bk || bk->disabled) {
    return;
  }
  int x, y;
  event_px(bk, ev, &x, &y);
  [[self window] makeFirstResponder:self];
  BK_press(bk->ctx, x, y, mods_of([ev modifierFlags]) | BK_BUTTON1);
}

- (void)mouseUp:(NSEvent *)ev
{
  if (!bk || bk->disabled) {
    return;
  }
  int x, y;
  event_px(bk, ev, &x, &y);
  BK_release(bk->ctx, x, y, mods_of([ev modifierFlags]) & ~BK_BUTTON1);
}

- (void)rightMouseDown:(NSEvent *)ev
{
  if (!bk || bk->disabled) {
    return;
  }
  int x, y;
  event_px(bk, ev, &x, &y);
  BK_context_menu(bk->ctx, x, y, mods_of([ev modifierFlags]));
}

- (void)rightMouseUp:(NSEvent *)ev
{
  if (!bk || bk->disabled) {
    return;
  }
  int x, y;
  event_px(bk, ev, &x, &y);
  BK_button(bk->ctx, x, y, 3, 1, mods_of([ev modifierFlags]) & ~BK_BUTTON1);
}

/* GHOST convertButton: buttonNumber 2 is the middle button. */
- (void)otherMouseDown:(NSEvent *)ev
{
  if (!bk || bk->disabled || [ev buttonNumber] != 2) {
    return;
  }
  int x, y;
  event_px(bk, ev, &x, &y);
  BK_button(bk->ctx, x, y, 2, 0, mods_of([ev modifierFlags]));
}

- (void)otherMouseUp:(NSEvent *)ev
{
  if (!bk || bk->disabled || [ev buttonNumber] != 2) {
    return;
  }
  int x, y;
  event_px(bk, ev, &x, &y);
  BK_button(bk->ctx, x, y, 2, 1, mods_of([ev modifierFlags]) & ~BK_BUTTON1);
}

- (void)motion:(NSEvent *)ev
{
  if (!bk || bk->disabled) {
    return;
  }
  /* GHOST: after a warp, moves stamped (just) before it still carry the old
   * position; skip them or a continuous drag jumps */
  if ([ev timestamp] < g_warp_time + 0.003) {
    return;
  }
  int x, y;
  event_px(bk, ev, &x, &y);
  BK_motion(bk->ctx, x, y, mods_of([ev modifierFlags]));
}

- (void)mouseMoved:(NSEvent *)ev
{
  [self motion:ev];
}

- (void)mouseDragged:(NSEvent *)ev
{
  [self motion:ev];
}

- (void)rightMouseDragged:(NSEvent *)ev
{
  [self motion:ev];
}

- (void)otherMouseDragged:(NSEvent *)ev
{
  [self motion:ev];
}

- (void)mouseExited:(NSEvent *)ev
{
  if (!bk) {
    return;
  }
  if ([NSEvent pressedMouseButtons] == 0) { /* a drag keeps the pointer (capture) */
    show_cursor(1);
    [[NSCursor arrowCursor] set];
    BK_motion(bk->ctx, -100000, -100000, 0);
  }
}

/* GHOST_SystemCocoa::handleMouseEvent NSEventTypeScrollWheel: wheels give
 * one notch per event; trackpads (events with a phase) give precise deltas,
 * summed here into notches. Horizontal scrolling is Shift+wheel, as the X11
 * host maps buttons 6 / 7. */
- (void)scrollWheel:(NSEvent *)ev
{
  if (!bk || bk->disabled) {
    return;
  }
  const NSEventPhase momentum = [ev momentumPhase], phase = [ev phase];
  /* GHOST: momentum scrolling after a key press would turn into another
   * action (e.g. Ctrl+scroll); ignore it */
  if (momentum != NSEventPhaseNone) {
    if (g_ignore_momentum) {
      return;
    }
  }
  else {
    g_ignore_momentum = 0;
  }
  int x, y;
  event_px(bk, ev, &x, &y);
  const int mods = mods_of([ev modifierFlags]) & ~BK_BUTTON1;
  if (phase == NSEventPhaseNone && momentum == NSEventPhaseNone) {
    const CGFloat dx = [ev deltaX], dy = [ev deltaY];
    if (dx != 0.0) {
      /* macOS turns Shift+wheel into horizontal deltas: Shift+wheel again */
      BK_wheel(bk->ctx, x, y, dx > 0.0 ? 1 : -1, mods | BK_SHIFT);
    }
    if (dy != 0.0 && bk) {
      BK_wheel(bk->ctx, x, y, dy > 0.0 ? 1 : -1, mods);
    }
    return;
  }
  if (phase == NSEventPhaseBegan) {
    bk->scroll_x = bk->scroll_y = 0.0;
  }
  /* 10.7+ precise deltas, in points (GHOST: scrollingDeltaX / Y) */
  CGFloat dx = [ev scrollingDeltaX], dy = [ev scrollingDeltaY];
  if (![ev hasPreciseScrollingDeltas]) {
    dx *= BK_SCROLL_STEP; /* line deltas */
    dy *= BK_SCROLL_STEP;
  }
  bk->scroll_x += dx;
  bk->scroll_y += dy;
  for (int i = 0; i < 8 && bk && fabs(bk->scroll_y) >= BK_SCROLL_STEP; i++) {
    const int d = bk->scroll_y > 0.0 ? 1 : -1;
    bk->scroll_y -= d * BK_SCROLL_STEP;
    BK_wheel(bk->ctx, x, y, d, mods);
  }
  for (int i = 0; i < 8 && bk && fabs(bk->scroll_x) >= BK_SCROLL_STEP; i++) {
    const int d = bk->scroll_x > 0.0 ? 1 : -1;
    bk->scroll_x -= d * BK_SCROLL_STEP;
    BK_wheel(bk->ctx, x, y, d, mods | BK_SHIFT);
  }
}

/* -- keys (GHOST_WindowViewCocoa.hh keyDown: / HANDLE_KEY_EVENT) -- */

/* Handling keyDown: here (not calling super) also keeps AppKit from beeping. */
- (void)keyDown:(NSEvent *)ev
{
  handle_key(self, ev, 1);
}

- (void)keyUp:(NSEvent *)ev
{
  if (ev != g_keyup_done) {
    handle_key(self, ev, 0);
  }
}

- (void)flagsChanged:(NSEvent *)ev
{
  handle_flags(self, [ev modifierFlags]);
}

@end

@implementation BUIKitWindowDelegate

/* GHOST: let the host decide; we close the window ourselves when it agrees. */
- (BOOL)windowShouldClose:(NSWindow *)sender
{
  if (bk) {
    request_close(bk);
  }
  return NO;
}

- (void)windowDidResize:(NSNotification *)notification
{
  if (bk) {
    update_size(bk);
  }
}

/* GHOST: macOS sends no resize when switching between zoomed and full-screen. */
- (void)windowDidEnterFullScreen:(NSNotification *)notification
{
  if (bk) {
    update_size(bk);
  }
}

- (void)windowDidExitFullScreen:(NSNotification *)notification
{
  if (bk) {
    update_size(bk);
  }
}

/* GHOST: GHOST_kEventNativeResolutionChange (moved to a screen with another
 * backing scale). */
- (void)windowDidChangeBackingProperties:(NSNotification *)notification
{
  if (bk) {
    on_backing_changed(bk);
  }
}

- (void)windowDidBecomeKey:(NSNotification *)notification
{
  if (bk) {
    bk->flags = [NSEvent modifierFlags]; /* modifiers may have changed elsewhere */
    BK_window_request_frame(bk, 0);       /* BK_window_focused changed */
  }
}

- (void)windowDidResignKey:(NSNotification *)notification
{
  if (bk) {
    BK_window_request_frame(bk, 0); /* BK_window_focused changed */
  }
}

- (void)windowDidDeminiaturize:(NSNotification *)notification
{
  if (bk) {
    bk->full = 1;
    bk->render_pending = 1;
  }
}

@end

@implementation BUIKitAppDelegate

/* GHOST's CocoaAppDelegate: Cmd+Q asks every window to close as its close
 * button would (on_close may refuse); the loop ends when none is left. */
- (NSApplicationTerminateReply)applicationShouldTerminate:(NSApplication *)sender
{
  BKWindow *next;
  for (BKWindow *w = g_windows; w; w = next) {
    next = w->next;
    if (w->nswin) {
      request_close(w);
    }
  }
  if (g_open == 0) {
    g_quit = 1;
    post_nudge();
  }
  return NSTerminateCancel;
}

/* Quit on an NSApp someone else made (see route_quit): with no window of ours
 * left, the quit is theirs. */
- (void)quit:(id)sender
{
  if (g_open == 0) {
    [NSApp terminate:sender];
  }
  else {
    [self applicationShouldTerminate:NSApp];
  }
}

- (void)quitEvent:(NSAppleEventDescriptor *)event withReply:(NSAppleEventDescriptor *)reply
{
  [self quit:nil];
}

- (BOOL)applicationSupportsSecureRestorableState:(NSApplication *)app
{
  return YES;
}

@end

/* -- NSApplication (GHOST_SystemCocoa::init) ------------------------------------------ */

static NSMenuItem *menu_add(NSMenu *menu, NSString *title, SEL action, NSString *key,
                            NSEventModifierFlags mask)
{
  NSMenuItem *item = [menu addItemWithTitle:title action:action keyEquivalent:key];
  [item setKeyEquivalentModifierMask:mask];
  return item;
}

/* An NSApp someone else made quits on its own terms. Tk's terminate: only
 * queues a Tcl event, which our loop doesn't service (and Tk would then exit
 * without asking the windows). So its Quit menu item and the Quit Apple Event
 * (the Dock's Quit, logout) come to our delegate. */
static void route_quit(void)
{
  NSMenu *app_menu = [[[[NSApp mainMenu] itemArray] firstObject] submenu];
  for (NSMenuItem *item in [app_menu itemArray]) {
    if ([item action] == @selector(terminate:)) {
      [item setTarget:g_app_delegate];
      [item setAction:@selector(quit:)];
    }
  }
  [[NSAppleEventManager sharedAppleEventManager]
      setEventHandler:g_app_delegate
          andSelector:@selector(quitEvent:withReply:)
        forEventClass:'aevt'
           andEventID:'quit'];
}

static int app_init(void)
{
  if (g_app_ready) {
    return 1;
  }
  if (!pthread_main_np()) {
    return 0;
  }
  @autoreleasepool {
    const BOOL created = (NSApp == nil);
    [NSApplication sharedApplication]; /* initializes NSApp */
    if (created) {
      /* a plain executable (python, node, a C program) is no app bundle:
       * become a regular app, with a Dock icon, a menu bar and key focus */
      [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
    }
    if ([NSApp mainMenu] == nil) {
      NSString *name = [[NSProcessInfo processInfo] processName];
      NSMenu *bar = [[NSMenu alloc] init];
      NSMenu *app_menu = [[NSMenu alloc] initWithTitle:name];
      menu_add(app_menu, [@"Hide " stringByAppendingString:name], @selector(hide:), @"h",
               NSEventModifierFlagCommand);
      menu_add(app_menu, @"Hide Others", @selector(hideOtherApplications:), @"h",
               NSEventModifierFlagOption | NSEventModifierFlagCommand);
      [app_menu addItemWithTitle:@"Show All"
                          action:@selector(unhideAllApplications:)
                   keyEquivalent:@""];
      [app_menu addItem:[NSMenuItem separatorItem]];
      menu_add(app_menu, [@"Quit " stringByAppendingString:name], @selector(terminate:), @"q",
               NSEventModifierFlagCommand);
      NSMenuItem *item = [[NSMenuItem alloc] init];
      [item setSubmenu:app_menu];
      [bar addItem:item];
      [item release];
      [app_menu release];

      NSMenu *window_menu = [[NSMenu alloc] initWithTitle:@"Window"];
      menu_add(window_menu, @"Minimize", @selector(performMiniaturize:), @"m",
               NSEventModifierFlagCommand);
      [window_menu addItemWithTitle:@"Zoom" action:@selector(performZoom:) keyEquivalent:@""];
      menu_add(window_menu, @"Enter Full Screen", @selector(toggleFullScreen:), @"f",
               NSEventModifierFlagControl | NSEventModifierFlagCommand);
      menu_add(window_menu, @"Close", @selector(performClose:), @"w",
               NSEventModifierFlagCommand);
      item = [[NSMenuItem alloc] init];
      [item setSubmenu:window_menu];
      [bar addItem:item];
      [item release];

      [NSApp setMainMenu:bar];
      [NSApp setWindowsMenu:window_menu];
      [window_menu release];
      [bar release];
    }
    g_app_delegate = [[BUIKitAppDelegate alloc] init]; /* kept: NSApp doesn't retain it */
    if ([NSApp delegate] == nil) {
      [NSApp setDelegate:g_app_delegate];
    }
    if (!created) {
      route_quit();
    }
    /* GHOST: no automatic window tabbing */
    [NSWindow setAllowsAutomaticWindowTabbing:NO];
    if (created) {
      /* whoever made NSApp (e.g. Tk) already finished launching it */
      [NSApp finishLaunching];
    }
    g_dead = [[NSMutableArray alloc] init];
  }
  g_app_ready = 1;
  return 1;
}

/* Native windows need the main thread and a window-server (GUI) session. */
int BK_window_supported(void)
{
  if (!pthread_main_np()) {
    return 0;
  }
  static int session = -1;
  if (session < 0) {
    CFDictionaryRef d = CGSessionCopyCurrentDictionary(); /* NULL over plain ssh */
    session = d != NULL;
    if (d) {
      CFRelease(d);
    }
  }
  return session;
}

/* -- host callbacks -------------------------------------------------------------------- */

static void host_request(void *data)
{
  BKWindow *win = (BKWindow *)data;
  if (win->nswin) {
    win->render_pending = 1;
  }
}

/* GHOST_WindowCocoa::getStandardCursor */
static NSCursor *ns_cursor(int c)
{
  switch (c) {
    case BK_CURSOR_EW:
      return [NSCursor resizeLeftRightCursor];
    case BK_CURSOR_NS:
      return [NSCursor resizeUpDownCursor];
    case BK_CURSOR_TEXT:
      return [NSCursor IBeamCursor];
    case BK_CURSOR_CROSSHAIR:
      return [NSCursor crosshairCursor];
    case BK_CURSOR_HAND:
      return [NSCursor pointingHandCursor];
    case BK_CURSOR_MOVE:
      return [NSCursor openHandCursor];
    case BK_CURSOR_NOT_ALLOWED:
      return [NSCursor operationNotAllowedCursor];
  }
  return [NSCursor arrowCursor];
}

/* GHOST_WindowCocoa::loadCursor: hide / unhide are counted, keep them paired. */
static void show_cursor(int visible)
{
  if (visible && g_cursor_hidden) {
    [NSCursor unhide];
    g_cursor_hidden = 0;
  }
  else if (!visible && !g_cursor_hidden) {
    [NSCursor hide];
    g_cursor_hidden = 1;
  }
}

static void apply_cursor(BKWindow *win)
{
  show_cursor(win->cursor != BK_CURSOR_NONE);
  if (win->cursor != BK_CURSOR_NONE) {
    [ns_cursor(win->cursor) set];
  }
}

static int pointer_inside(BKWindow *win)
{
  const NSPoint p = [win->view convertPoint:[win->nswin mouseLocationOutsideOfEventStream]
                                   fromView:nil];
  return NSPointInRect(p, [win->view bounds]);
}

/* GHOST_WindowCocoa::setWindowCursorShape */
static void host_set_cursor(void *data, int c)
{
  BKWindow *win = (BKWindow *)data;
  win->cursor = BK_CLAMP(c, 0, 7);
  if (!win->nswin) {
    return;
  }
  @autoreleasepool {
    /* like a window cursor elsewhere: only while the pointer is over the view
     * (or dragging from it); cursorUpdate: applies it on entry */
    if (pointer_inside(win) || [NSEvent pressedMouseButtons] != 0) {
      apply_cursor(win);
    }
  }
}

/* GHOST_SystemCocoa::setMouseCursorPosition: Quartz takes global top-left
 * coordinates; CGAssociateMouseAndMouseCursorPosition drops the 500 ms of
 * suppressed mouse events after a synthetic move. */
static void host_warp(void *data, int x, int y)
{
  BKWindow *win = (BKWindow *)data;
  if (!win->nswin) {
    return;
  }
  @autoreleasepool {
    const NSPoint in_view = NSMakePoint(x / win->px, y / win->px);
    const NSPoint in_window = [win->view convertPoint:in_view toView:nil];
    const NSRect on_screen = [win->nswin
        convertRectToScreen:NSMakeRect(in_window.x, in_window.y, 0, 0)];
    CGWarpMouseCursorPosition(
        CGPointMake(on_screen.origin.x, primary_height() - on_screen.origin.y));
    CGAssociateMouseAndMouseCursorPosition(true);
    g_warp_time = [[NSProcessInfo processInfo] systemUptime];
  }
}

/* GHOST_SystemCocoa::getClipboard */
static char *host_clipboard_get(void *data)
{
  char *out = NULL;
  @autoreleasepool {
    NSString *s = [[NSPasteboard generalPasteboard] stringForType:NSPasteboardTypeString];
    const char *u = s ? [s UTF8String] : NULL;
    if (u) {
      out = strdup(u);
    }
  }
  return out;
}

/* GHOST_SystemCocoa::putClipboard */
static void host_clipboard_set(void *data, const char *text)
{
  @autoreleasepool {
    NSPasteboard *pb = [NSPasteboard generalPasteboard];
    [pb declareTypes:[NSArray arrayWithObject:NSPasteboardTypeString] owner:nil];
    NSString *s = [NSString stringWithUTF8String:text ? text : ""];
    if (s) {
      [pb setString:s forType:NSPasteboardTypeString];
    }
  }
}

static BKCocoaTimer *timer_slot(BKWindow *win, int ident)
{
  for (int i = 0; i < MAX_TIMERS; i++) {
    if (win->timers[i].ident == ident) {
      return &win->timers[i];
    }
  }
  for (int i = 0; i < MAX_TIMERS; i++) {
    if (win->timers[i].ident == 0) {
      return &win->timers[i];
    }
  }
  return NULL;
}

static void host_schedule(void *data, int ms, int timer)
{
  BKWindow *win = (BKWindow *)data;
  /* slot -1: the tooltip; -2: custom nodes' frames (BK_TIMER_CUSTOM) */
  const int slot = timer == BK_TIMER_CUSTOM ? -2 : -1;
  if (slot == -1) {
    win->tip_id = timer;
  }
  BKCocoaTimer *t = timer_slot(win, slot);
  if (t) {
    t->ident = slot;
    t->due = now_ms() + ms;
  }
}

static void host_emit(void *data, BKNode *node, int kind, void *ref, double a, double b)
{
  BKWindow *win = (BKWindow *)data;
  if (win->prev.emit) {
    win->prev.emit(win->prev.data, node, kind, ref, a, b);
  }
}

/* -- rendering ----------------------------------------------------------------------- */

/* The display's refresh interval (ProMotion: 120 Hz). */
static double refresh_ms(void)
{
  static double ms = 0.0;
  if (ms > 0.0) {
    return ms;
  }
  double hz = 0.0;
#  if defined(MAC_OS_VERSION_12_0) && MAC_OS_X_VERSION_MAX_ALLOWED >= MAC_OS_VERSION_12_0
  if (@available(macOS 12.0, *)) {
    hz = (double)[[NSScreen mainScreen] maximumFramesPerSecond];
  }
#  endif
  if (hz <= 1.0) {
    CGDisplayModeRef mode = CGDisplayCopyDisplayMode(CGMainDisplayID());
    if (mode) {
      hz = CGDisplayModeGetRefreshRate(mode); /* 0 on some built-in panels */
      CGDisplayModeRelease(mode);
    }
  }
  ms = 1000.0 / (hz > 1.0 ? hz : 60.0);
  return ms;
}

/* Invalidate the damaged rectangles (pixels -> points) and draw them now. */
static void present(BKWindow *win, int all)
{
  BUIKitView *v = win->view;
  if (!v) {
    return;
  }
  const CGFloat px = win->px;
  if (all) {
    [v setNeedsDisplay:YES];
  }
  else {
    const int(*rects)[4];
    const int n = BUI_raster_damage(win->raster, &rects);
    for (int i = 0; i < n; i++) {
      const CGFloat x0 = floor(rects[i][0] / px), y0 = floor(rects[i][1] / px);
      const CGFloat x1 = ceil((rects[i][0] + rects[i][2]) / px);
      const CGFloat y1 = ceil((rects[i][1] + rects[i][3]) / px);
      [v setNeedsDisplayInRect:NSMakeRect(x0, y0, x1 - x0, y1 - y0)];
    }
  }
  [v displayIfNeeded];
}

static void do_render(BKWindow *win)
{
  const int w = BK_MAX(1, win->w), h = BK_MAX(1, win->h);
  const int resized = BUI_raster_width(win->raster) != w || BUI_raster_height(win->raster) != h;
  const int regions = BK_render(win->ctx, win->raster, w, h, win->full || resized);
  if (regions || resized || win->full) {
    present(win, resized || win->full);
    win->frames++;
  }
  win->full = 0;
}

static void render_window(BKWindow *win)
{
  const double t0 = now_ms();
  /* a new size: the host lays out for it before this frame, so no frame shows the last
   * layout at the new size (centred text would jump back and forth while the window is
   * dragged wider or narrower) */
  if (win->cb.on_frame && (BUI_raster_width(win->raster) != BK_MAX(1, win->w) ||
                           BUI_raster_height(win->raster) != BK_MAX(1, win->h)))
  {
    win->want_frame = 1;
  }
  if (win->want_frame && win->cb.on_frame) {
    /* render_pending is still set: edits made here join this frame */
    win->want_frame = 0;
    win->frame_start = t0;
    win->cb.on_frame(win->cb.data, win);
    if (!win->nswin) {
      return; /* closed by the callback */
    }
  }
  win->render_pending = 0;
  @autoreleasepool {
    do_render(win);
  }
  int paced = 0;
  if (win->want_frame) { /* the host asked for the next frame */
    bk_frame_ms = now_ms() - win->frame_start;
    bk_frame_budget_ms = win->frame_vsync ? refresh_ms() : 0.0;
    win->render_pending = 1;
    paced = win->frame_vsync;
  }
  else {
    bk_frame_ms = 0.0; /* the animation ended */
  }
  if (BK_animating(win->ctx)) {
    win->render_pending = 1;
    paced = 1;
  }
  /* the next frame one refresh after this one began: early frames wait,
   * late ones start right away (Win32's DwmFlush pacing) */
  win->next_frame = paced ? t0 + refresh_ms() : 0.0;
}

/* -- input mapping (GHOST_SystemCocoa::handleKeyEvent / convertKey) ------------------- */

/* Keys without text, by key code: X keysym names, as Tk and BUI use them. */
static const char *special_keysym(unsigned short code)
{
  switch (code) {
    case BK_kVK_Return: return "Return";
    case BK_kVK_Tab: return "Tab";
    case BK_kVK_Delete: return "BackSpace";
    case BK_kVK_ForwardDelete: return "Delete";
    case BK_kVK_Escape: return "Escape";
    case BK_kVK_Help: return "Insert";
    case BK_kVK_Home: return "Home";
    case BK_kVK_End: return "End";
    case BK_kVK_PageUp: return "Prior";
    case BK_kVK_PageDown: return "Next";
    case BK_kVK_LeftArrow: return "Left";
    case BK_kVK_RightArrow: return "Right";
    case BK_kVK_UpArrow: return "Up";
    case BK_kVK_DownArrow: return "Down";
    case BK_kVK_KeypadEnter: return "KP_Enter";
    case BK_kVK_F1: return "F1";
    case BK_kVK_F2: return "F2";
    case BK_kVK_F3: return "F3";
    case BK_kVK_F4: return "F4";
    case BK_kVK_F5: return "F5";
    case BK_kVK_F6: return "F6";
    case BK_kVK_F7: return "F7";
    case BK_kVK_F8: return "F8";
    case BK_kVK_F9: return "F9";
    case BK_kVK_F10: return "F10";
    case BK_kVK_F11: return "F11";
    case BK_kVK_F12: return "F12";
    case BK_kVK_F13: return "F13";
    case BK_kVK_F14: return "F14";
    case BK_kVK_F15: return "F15";
    case BK_kVK_F16: return "F16";
    case BK_kVK_F17: return "F17";
    case BK_kVK_F18: return "F18";
    case BK_kVK_F19: return "F19";
    case BK_kVK_F20: return "F20";
  }
  return NULL;
}

/* Keypad keys: X's KP_ keysyms, with their text (as XLookupString gives). */
static const char *keypad_keysym(unsigned short code)
{
  switch (code) {
    case BK_kVK_Keypad0: return "KP_0";
    case BK_kVK_Keypad1: return "KP_1";
    case BK_kVK_Keypad2: return "KP_2";
    case BK_kVK_Keypad3: return "KP_3";
    case BK_kVK_Keypad4: return "KP_4";
    case BK_kVK_Keypad5: return "KP_5";
    case BK_kVK_Keypad6: return "KP_6";
    case BK_kVK_Keypad7: return "KP_7";
    case BK_kVK_Keypad8: return "KP_8";
    case BK_kVK_Keypad9: return "KP_9";
    case BK_kVK_KeypadDecimal: return "KP_Decimal";
    case BK_kVK_KeypadMultiply: return "KP_Multiply";
    case BK_kVK_KeypadPlus: return "KP_Add";
    case BK_kVK_KeypadMinus: return "KP_Subtract";
    case BK_kVK_KeypadDivide: return "KP_Divide";
    case BK_kVK_KeypadEquals: return "KP_Equal";
  }
  return NULL;
}

static int is_text_char(unichar c)
{
  /* control characters and AppKit's function-key range (0xF700..0xF8FF) */
  return c >= 0x20 && c != 0x7f && !(c >= 0xF700 && c <= 0xF8FF);
}

static void copy_utf8(char *dst, size_t size, NSString *s)
{
  const char *u = s ? [s UTF8String] : NULL;
  dst[0] = 0;
  if (u) {
    strncpy(dst, u, size - 1);
    dst[size - 1] = 0;
  }
}

static void handle_key(BUIKitView *v, NSEvent *ev, int press)
{
  BKWindow *win = v ? v->bk : NULL;
  if (!win || win->disabled) {
    return;
  }
  g_ignore_momentum = 1; /* GHOST: a key press ends momentum scrolling */
  @autoreleasepool {
    const NSEventModifierFlags f = [ev modifierFlags];
    const int mods = mods_of(f);
    const unsigned short code = [ev keyCode];
    char ks_buf[32], text[32];
    text[0] = 0;
    const char *ks = special_keysym(code);
    const int special = ks != NULL;
    if (!ks) {
      ks = keypad_keysym(code);
    }
    if (!ks) {
      /* the character ignoring modifiers except Shift, as convertKey uses it
       * (letters and punctuation follow the keyboard layout) */
      NSString *ign = [ev charactersIgnoringModifiers];
      ks = "";
      if ([ign length] > 0) {
        const unichar c = [ign characterAtIndex:0];
        if (c == ' ') {
          ks = "space";
        }
        else if (c == '-') {
          ks = "minus";
        }
        else if (is_text_char(c)) {
          copy_utf8(ks_buf, sizeof(ks_buf), ign);
          ks = ks_buf;
        }
      }
    }
    if (!press) {
      BK_key_release(win->ctx, ks, mods);
    }
    else {
      /* GHOST: no text with Command held, nor for arrows / F-keys */
      if (!special && !(f & NSEventModifierFlagCommand)) {
        NSString *chars = [ev characters]; /* empty for dead keys */
        if ([chars length] > 0 && is_text_char([chars characterAtIndex:0])) {
          copy_utf8(text, sizeof(text), chars);
        }
      }
      BK_key(win->ctx, text, ks, mods);
    }
  }
}

/* GHOST_SystemCocoa::handleKeyEvent NSEventTypeFlagsChanged: modifier
 * presses / releases as keys (Command is Tk's Meta_L). */
static void handle_flags(BUIKitView *v, NSEventModifierFlags flags)
{
  static const struct {
    NSEventModifierFlags bit;
    const char *keysym;
  } keys[] = {{NSEventModifierFlagShift, "Shift_L"},
              {NSEventModifierFlagControl, "Control_L"},
              {NSEventModifierFlagOption, "Alt_L"},
              {NSEventModifierFlagCommand, "Meta_L"}};
  BKWindow *win = v ? v->bk : NULL;
  if (!win) {
    return;
  }
  const NSEventModifierFlags old = win->flags;
  win->flags = flags;
  g_ignore_momentum = 1;
  if (win->disabled) {
    return;
  }
  const int mods = mods_of(flags);
  for (int i = 0; i < 4 && v->bk; i++) {
    if ((flags ^ old) & keys[i].bit) {
      if (flags & keys[i].bit) {
        BK_key(v->bk->ctx, "", keys[i].keysym, mods);
      }
      else {
        BK_key_release(v->bk->ctx, keys[i].keysym, mods);
      }
    }
  }
}

/* -- windows --------------------------------------------------------------------------- */

/* The view's size in pixels (GHOST_WindowCocoa::updateDrawingSize). */
static void update_size(BKWindow *win)
{
  if (!win->view) {
    return;
  }
  const NSRect b = [win->view convertRectToBacking:[win->view bounds]];
  const int w = (int)lround(b.size.width), h = (int)lround(b.size.height);
  if (w != win->w || h != win->h) {
    win->w = w;
    win->h = h;
    win->render_pending = 1;
  }
  /* GHOST: live resize is a modal loop that doesn't return to ours; draw now */
  if ([win->view inLiveResize] && win->render_pending) {
    render_window(win);
  }
}

static void on_backing_changed(BKWindow *win)
{
  const float px = (float)[win->nswin backingScaleFactor];
  if (px > 0.0f && px != win->px) {
    win->px = px;
    if (env_scale() <= 0.0f) {
      win->scale = ui_scale(px);
      BK_set_scale(win->scale);
      if (win->ctx->root) {
        BK_node_relayout(win->ctx->root);
      }
    }
  }
  win->full = 1;
  win->render_pending = 1;
  update_size(win);
}

static void request_close(BKWindow *win)
{
  if (!win->cb.on_close || win->cb.on_close(win->cb.data, win)) {
    close_window(win);
  }
}

/* Close (GHOST_WindowCocoa::~GHOST_WindowCocoa). AppKit may still be inside
 * this window's methods (we can be called from its delegate or view), so
 * the window and delegate are released between loop turns. */
static void close_window(BKWindow *win)
{
  if (!win->nswin) {
    return;
  }
  NSWindow *nw = win->nswin;
  win->nswin = nil;
  if (win->view) {
    win->view->bk = NULL;
    win->view = nil;
  }
  if (win->delegate) {
    win->delegate->bk = NULL;
  }
  [nw setDelegate:nil];
  if (win->cursor == BK_CURSOR_NONE) {
    show_cursor(1);
  }
  [nw orderOut:nil];
  [nw close];
  [g_dead addObject:nw];
  [nw release];
  if (win->delegate) {
    [g_dead addObject:win->delegate];
    [win->delegate release];
    win->delegate = nil;
  }
  g_open--;
  /* GHOST: make the front-most remaining window key */
  NSArray *left = [NSApp orderedWindows];
  for (NSUInteger i = 0; i < [left count]; i++) {
    NSWindow *o = [left objectAtIndex:i];
    if (o != nw && [o isVisible] && view_of(o)) {
      [o makeKeyWindow];
      break;
    }
  }
}

BKWindow *BK_window_new(BKContext *ctx, const char *title, int width, int height)
{
  if (!BK_window_supported() || !app_init()) {
    return NULL;
  }
  BKWindow *win = (BKWindow *)calloc(1, sizeof(BKWindow));
  win->ctx = ctx;
  win->prev = ctx->host;
  win->raster = BUI_raster_create(1, 1);
  win->next_timer = 1;
  @autoreleasepool {
    NSScreen *screen = [NSScreen mainScreen];
    win->px = screen ? (float)[screen backingScaleFactor] : 1.0f;
    if (win->px <= 0.0f) {
      win->px = 1.0f;
    }
    win->scale = ui_scale(win->px);
    BK_set_scale(win->scale);
    const CGFloat k = win->scale / win->px; /* logical px -> points */
    const NSRect rect = NSMakeRect(0, 0, BK_MAX(1.0, width * k), BK_MAX(1.0, height * k));
    /* GHOST_WindowCocoa::GHOST_WindowCocoa */
    const NSWindowStyleMask style = NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                                    NSWindowStyleMaskMiniaturizable |
                                    NSWindowStyleMaskResizable;
    BUIKitNSWindow *nw = [[BUIKitNSWindow alloc] initWithContentRect:rect
                                                           styleMask:style
                                                             backing:NSBackingStoreBuffered
                                                               defer:NO];
    if (!nw) {
      BUI_raster_free(win->raster);
      free(win);
      return NULL;
    }
    [nw setReleasedWhenClosed:NO]; /* released by close_window */
    win->style = style;
    BUIKitView *view = [[BUIKitView alloc] initWithFrame:rect];
    view->bk = win;
    [nw setContentView:view];
    [nw setInitialFirstResponder:view];
    [nw makeFirstResponder:view];
    [view release]; /* the window keeps it */
    win->view = view;
    BUIKitWindowDelegate *delegate = [[BUIKitWindowDelegate alloc] init];
    delegate->bk = win;
    [nw setDelegate:delegate];
    win->delegate = delegate;
    [nw setAcceptsMouseMovedEvents:YES];
    [nw setCollectionBehavior:NSWindowCollectionBehaviorFullScreenPrimary];
    [nw center];
    win->nswin = nw;
    const float px = (float)[nw backingScaleFactor];
    if (px > 0.0f && px != win->px) { /* it opened on another screen */
      win->px = px;
      win->scale = ui_scale(px);
      BK_set_scale(win->scale);
    }
    win->flags = [NSEvent modifierFlags];
  }
  win->next = g_windows;
  g_windows = win;
  g_open++;
  BK_window_set_title(win, title);
  update_size(win);
  BKHost host = {win,           host_request,       host_set_cursor, host_warp,
                 host_clipboard_get, host_clipboard_set, host_schedule, host_emit};
  BK_context_set_host(ctx, &host);
  return win;
}

void BK_window_free(BKWindow *win)
{
  if (!win) {
    return;
  }
  @autoreleasepool {
    close_window(win);
  }
  for (BKWindow **p = &g_windows; *p; p = &(*p)->next) {
    if (*p == win) {
      *p = win->next;
      break;
    }
  }
  if (win->ctx) {
    BK_context_set_host(win->ctx, &win->prev);
  }
  BUI_raster_free(win->raster);
  free(win);
}

void BK_window_set_callbacks(BKWindow *win, const BKWindowCallbacks *cb)
{
  win->cb = *cb;
}

void BK_window_show(BKWindow *win)
{
  if (!win->nswin) {
    return;
  }
  @autoreleasepool {
    if (!win->shown) {
      win->shown = 1;
      [NSApp activateIgnoringOtherApps:YES]; /* started from a terminal: come to the front */
    }
    [win->nswin makeKeyAndOrderFront:nil];
  }
  win->full = 1;
  win->render_pending = 1;
}

void BK_window_close(BKWindow *win)
{
  @autoreleasepool {
    close_window(win);
  }
}

int BK_window_is_open(BKWindow *win)
{
  return win && win->nswin != nil;
}

/* GHOST_WindowCocoa::setTitle */
void BK_window_set_title(BKWindow *win, const char *title)
{
  if (!win->nswin) {
    return;
  }
  @autoreleasepool {
    NSString *t = [NSString stringWithUTF8String:title ? title : ""];
    [win->nswin setTitle:t ? t : @""];
  }
}

/* Content size limits in points; fixed axes are pinned (Tk's resizable). */
static void apply_limits(BKWindow *win)
{
  const CGFloat k = win->scale / win->px;
  const NSSize cur = [win->view bounds].size;
  NSSize mn = NSMakeSize(win->min_w * k, win->min_h * k);
  NSSize mx = NSMakeSize(win->max_w ? win->max_w * k : FLT_MAX,
                         win->max_h ? win->max_h * k : FLT_MAX);
  if (win->fixed_w) {
    mn.width = mx.width = cur.width;
  }
  if (win->fixed_h) {
    mn.height = mx.height = cur.height;
  }
  [win->nswin setContentMinSize:mn];
  [win->nswin setContentMaxSize:mx];
}

/* GHOST_WindowCocoa::setClientSize */
void BK_window_set_size(BKWindow *win, int width, int height)
{
  if (!win->nswin) {
    return;
  }
  @autoreleasepool {
    const CGFloat k = win->scale / win->px;
    [win->nswin setContentSize:NSMakeSize(BK_MAX(1.0, width * k), BK_MAX(1.0, height * k))];
    if (win->fixed_w || win->fixed_h) {
      apply_limits(win);
    }
    update_size(win);
  }
  win->render_pending = 1;
}

void BK_window_get_size(BKWindow *win, int *w, int *h)
{
  *w = win->nswin ? win->w : 0;
  *h = win->nswin ? win->h : 0;
}

BuiRaster *BK_window_raster(BKWindow *win)
{
  return win->raster;
}

void BK_window_set_gpu(BKWindow *win, int on)
{
  (void)win;
  (void)on; /* the GPU renderer is Direct3D 11: the Cocoa host presents the CPU raster */
}

int BK_window_gpu(BKWindow *win)
{
  return 0;
}

int BK_window_frames(BKWindow *win)
{
  return win->frames;
}

void BK_window_render_now(BKWindow *win)
{
  if (win->nswin) {
    win->full = 1;
    render_window(win);
  }
}

int BK_window_timer(BKWindow *win, int ms)
{
  const int ident = win->next_timer++;
  BKCocoaTimer *t = timer_slot(win, ident);
  if (t) {
    t->ident = ident;
    t->due = now_ms() + BK_MAX(0, ms);
  }
  return ident;
}

void BK_window_cancel_timer(BKWindow *win, int ident)
{
  for (int i = 0; i < MAX_TIMERS; i++) {
    if (win->timers[i].ident == ident) {
      win->timers[i].ident = 0;
    }
  }
}

void BK_window_request_frame(BKWindow *win, int vsync)
{
  win->want_frame = 1;
  win->frame_vsync = vsync;
  win->render_pending = 1;
}

void *BK_window_handle(BKWindow *win)
{
  return (void *)win->nswin; /* NSWindow * */
}

/* Window-manager requests (Tk's wm), after GHOST_WindowCocoa::setState /
 * setOrder. Positions are in points, top-left origin. */
void BK_window_wm(BKWindow *win, const char *key, double a, double b)
{
  NSWindow *nw = win->nswin;
  if (!nw) {
    return;
  }
  @autoreleasepool {
    if (!strcmp(key, "pos")) { /* the content's top-left, as wm_get reports it */
      const NSRect f = [nw frame];
      const CGFloat title = NSMaxY(f) - NSMaxY([nw contentRectForFrameRect:f]);
      [nw setFrameTopLeftPoint:NSMakePoint(a, primary_height() - b + title)];
    }
    else if (!strcmp(key, "minsize") || !strcmp(key, "maxsize")) {
      const int mini = key[1] == 'i';
      *(mini ? &win->min_w : &win->max_w) = (int)a;
      *(mini ? &win->min_h : &win->max_h) = (int)b;
      apply_limits(win);
    }
    else if (!strcmp(key, "resizable")) {
      win->fixed_w = a == 0.0;
      win->fixed_h = b == 0.0;
      if (win->fixed_w && win->fixed_h) {
        win->style &= ~NSWindowStyleMaskResizable;
      }
      else {
        win->style |= NSWindowStyleMaskResizable;
      }
      if (!win->override) {
        [nw setStyleMask:win->style];
      }
      apply_limits(win);
    }
    else if (!strcmp(key, "topmost")) {
      [nw setLevel:a ? NSFloatingWindowLevel : NSNormalWindowLevel];
    }
    else if (!strcmp(key, "alpha")) {
      [nw setAlphaValue:BK_CLAMP(a, 0.0, 1.0)];
    }
    else if (!strcmp(key, "override")) {
      win->override = a != 0.0;
      [nw setStyleMask:win->override ? NSWindowStyleMaskBorderless : win->style];
      update_size(win);
    }
    else if (!strcmp(key, "state")) {
      switch (BK_CLAMP((int)a, 0, 3)) {
        case 1:
          [nw miniaturize:nil];
          break;
        case 2:
          [nw orderOut:nil];
          break;
        case 3:
          if (![nw isVisible]) {
            [nw makeKeyAndOrderFront:nil];
          }
          if (![nw isZoomed]) {
            [nw zoom:nil];
          }
          break;
        default:
          if ([nw isMiniaturized]) {
            [nw deminiaturize:nil];
          }
          else if ([nw isZoomed]) {
            [nw zoom:nil];
          }
          if (![nw isVisible]) {
            [nw makeKeyAndOrderFront:nil];
          }
          break;
      }
    }
    else if (!strcmp(key, "lift")) {
      [nw orderFront:nil];
    }
    else if (!strcmp(key, "lower")) {
      [nw orderBack:nil];
    }
    else if (!strcmp(key, "disabled")) { /* modal dialogs disable their parent */
      win->disabled = a != 0.0;
    }
    else if (!strcmp(key, "focus")) {
      [NSApp activateIgnoringOtherApps:YES];
      [nw makeKeyAndOrderFront:nil];
    }
  }
}

int BK_window_focused(BKWindow *win)
{
  if (!pthread_main_np()) {
    return -1;
  }
  return win->nswin && [win->nswin isKeyWindow];
}

/* The token is an xdg-activation token: macOS has no use for it. */
void BK_window_activate(BKWindow *win, const char *token)
{
  NSWindow *nw = win->nswin;
  if (!nw || !pthread_main_np()) {
    return;
  }
  (void)token;
  @autoreleasepool {
    if ([nw isMiniaturized]) {
      [nw deminiaturize:nil];
    }
    [NSApp activateIgnoringOtherApps:YES];
    [nw makeKeyAndOrderFront:nil];
  }
}

int BK_window_wm_get(BKWindow *win, const char *key, double *r_a, double *r_b)
{
  *r_a = *r_b = 0.0;
  if (!pthread_main_np()) {
    return 0;
  }
  int ok = 0;
  @autoreleasepool {
    if (!strcmp(key, "screen")) {
      NSScreen *s = (win && win->nswin && [win->nswin screen]) ? [win->nswin screen] :
                                                                 [NSScreen mainScreen];
      if (s) {
        *r_a = [s frame].size.width;
        *r_b = [s frame].size.height;
      }
      ok = 1;
    }
    else if (!strcmp(key, "scale")) {
      NSScreen *s = [NSScreen mainScreen];
      *r_a = *r_b = win ? win->scale : ui_scale(s ? (float)[s backingScaleFactor] : 1.0f);
      ok = 1;
    }
    else if (!strcmp(key, "pointer")) {
      const NSPoint p = [NSEvent mouseLocation];
      *r_a = p.x;
      *r_b = primary_height() - p.y;
      ok = 1;
    }
    else if (win && win->nswin) {
      if (!strcmp(key, "pos")) { /* the content's top-left on screen */
        const NSRect c = [win->nswin contentRectForFrameRect:[win->nswin frame]];
        *r_a = c.origin.x;
        *r_b = primary_height() - NSMaxY(c);
        ok = 1;
      }
      else if (!strcmp(key, "state")) { /* GHOST_WindowCocoa::getState */
        NSWindow *nw = win->nswin;
        *r_a = [nw isMiniaturized] ? 1 : ![nw isVisible] ? 2 : [nw isZoomed] ? 3 : 0;
        ok = 1;
      }
    }
  }
  return ok;
}

/* -- the loop (GHOST_SystemCocoa::processEvents) --------------------------------------- */

static void run_timers(void)
{
  const double t = now_ms();
  for (BKWindow *win = g_windows; win; win = win->next) {
    for (int i = 0; i < MAX_TIMERS && win->nswin; i++) {
      BKCocoaTimer *tm = &win->timers[i];
      if (tm->ident != 0 && tm->due <= t) {
        const int ident = tm->ident;
        tm->ident = 0;
        if (ident == -1) {
          BK_timer(win->ctx, win->tip_id);
        }
        else if (ident == -2) {
          BK_timer(win->ctx, BK_TIMER_CUSTOM);
        }
        else if (win->cb.on_timer) {
          win->cb.on_timer(win->cb.data, win, ident);
        }
      }
    }
  }
}

static void render_due(void)
{
  const double t = now_ms();
  for (BKWindow *win = g_windows; win; win = win->next) {
    if (win->nswin && win->render_pending && t >= win->next_frame) {
      render_window(win);
    }
  }
}

/* Milliseconds until the next timer or frame (-1: nothing to wait for). */
static double next_wait(void)
{
  double wait = -1.0;
  const double t = now_ms();
  for (BKWindow *win = g_windows; win; win = win->next) {
    if (!win->nswin) {
      continue;
    }
    if (win->render_pending) {
      const double d = BK_MAX(0.0, win->next_frame - t);
      wait = wait < 0 ? d : BK_MIN(wait, d);
    }
    for (int i = 0; i < MAX_TIMERS; i++) {
      if (win->timers[i].ident != 0) {
        const double d = BK_MAX(0.0, win->timers[i].due - t);
        wait = wait < 0 ? d : BK_MIN(wait, d);
      }
    }
  }
  return wait;
}

/* An application-defined event that ends a wait in nextEventMatchingMask. */
static void post_nudge(void)
{
  if (g_nudge_pending || NSApp == nil) {
    return;
  }
  g_nudge_pending = 1;
  @autoreleasepool {
    NSEvent *e = [NSEvent otherEventWithType:NSEventTypeApplicationDefined
                                    location:NSZeroPoint
                               modifierFlags:0
                                   timestamp:0
                                windowNumber:0
                                     context:nil
                                     subtype:BK_NUDGE_SUBTYPE
                                       data1:BK_NUDGE_DATA
                                       data2:0];
    [NSApp postEvent:e atStart:NO];
  }
}

static void handle_event(NSEvent *ev)
{
  const NSEventType type = [ev type];
  if (type == NSEventTypeApplicationDefined && [ev subtype] == BK_NUDGE_SUBTYPE &&
      [ev data1] == BK_NUDGE_DATA)
  {
    g_nudge_pending = 0;
    return;
  }
  /* GHOST: Control+(Shift+)Tab never reaches the view (AppKit uses it to
   * move between views); handle it directly */
  if (type == NSEventTypeKeyDown && [ev keyCode] == BK_kVK_Tab &&
      ([ev modifierFlags] & NSEventModifierFlagControl))
  {
    handle_key(view_of([ev window]), ev, 1);
    return;
  }
  /* GHOST: NSApp swallows key-ups while Command / Option are held */
  if (type == NSEventTypeKeyUp &&
      ([ev modifierFlags] & (NSEventModifierFlagCommand | NSEventModifierFlagOption)))
  {
    handle_key(view_of([ev window]), ev, 0);
    g_keyup_done = ev; /* in case it does reach keyUp: */
  }
  [NSApp sendEvent:ev];
  g_keyup_done = nil;
}

/* One turn: events, timers, renders; blocks up to `block_ms` (-1: forever). */
static void turn(double block_ms)
{
  g_turn_depth++;
  @autoreleasepool {
    if (g_turn_depth == 1) {
      [g_dead removeAllObjects]; /* nothing of theirs is on the stack now */
    }
    g_nudge_pending = 0; /* another loop (Tk, a modal panel) may have eaten ours */
    NSDate *until = block_ms < 0.0  ? [NSDate distantFuture] :
                    block_ms == 0.0 ? [NSDate distantPast] :
                                      [NSDate dateWithTimeIntervalSinceNow:block_ms / 1000.0];
    NSEvent *ev = [NSApp nextEventMatchingMask:NSEventMaskAny
                                     untilDate:until
                                        inMode:NSDefaultRunLoopMode
                                       dequeue:YES];
    for (int n = 0; ev && n < 512; n++) {
      handle_event(ev);
      ev = [NSApp nextEventMatchingMask:NSEventMaskAny
                              untilDate:[NSDate distantPast]
                                 inMode:NSDefaultRunLoopMode
                                dequeue:YES];
    }
    if (ev) {
      handle_event(ev);
    }
    run_timers();
    render_due();
  }
  g_turn_depth--;
}

void BK_window_set_stop_check(int (*stop)(void))
{
  g_stop = stop;
}

/* Callable from any thread. */
void BK_window_quit(void)
{
  g_quit = 1;
  if (pthread_main_np()) {
    post_nudge();
  }
  else {
    BK_window_wake();
  }
}

void BK_window_run(void)
{
  /* keep-alive loops (driven through BK_window_wake) may start before any
   * window exists */
  if (!BK_window_supported() || !app_init()) {
    return;
  }
  g_quit = 0;
  while ((g_open > 0 || g_keep_alive) && !g_quit) {
    turn(next_wait());
    if (g_stop && g_stop()) {
      break;
    }
  }
}

int BK_window_pump(void)
{
  if (g_app_ready && pthread_main_np()) {
    turn(0.0);
  }
  return g_open;
}

/* The wake: a run-loop source on the main run loop, in the common modes so
 * it also fires inside AppKit's own loops (live resize, modal panels), as
 * Win32's message-only window does inside modal move / resize loops. */
static void wake_perform(void *info)
{
  (void)info;
  if (__atomic_exchange_n(&g_wake_pending, 0, __ATOMIC_ACQ_REL)) {
    BK_image_poll(); /* decoded images: damage what shows them */
    if (g_wake_cb) {
      g_wake_cb();
    }
  }
  @autoreleasepool {
    render_due(); /* draw what it changed even inside AppKit's loops */
    post_nudge(); /* and let our loop recompute its wait */
  }
}

int bk_window_wake_ready(void)
{
  if (!g_wake_src) {
    CFRunLoopSourceContext c;
    memset(&c, 0, sizeof(c));
    c.perform = wake_perform;
    g_wake_src = CFRunLoopSourceCreate(kCFAllocatorDefault, 0, &c);
    if (!g_wake_src) {
      return 0;
    }
    CFRunLoopAddSource(CFRunLoopGetMain(), g_wake_src, kCFRunLoopCommonModes);
  }
  return 1;
}

int BK_window_wake_init(void (*cb)(void))
{
  g_wake_cb = cb;
  return bk_window_wake_ready();
}

/* Thread-safe: wakes the loop, which calls the wake callback. */
void BK_window_wake(void)
{
  if (g_wake_src && !__atomic_exchange_n(&g_wake_pending, 1, __ATOMIC_ACQ_REL)) {
    CFRunLoopSourceSignal(g_wake_src);
    CFRunLoopWakeUp(CFRunLoopGetMain());
  }
}

void BK_window_set_keep_alive(int on)
{
  g_keep_alive = on;
}

#endif /* BUI_WINDOW_COCOA */
