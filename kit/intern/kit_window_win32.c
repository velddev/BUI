/* Native window host (Win32). Other platforms: kit_window_x11.c (X11),
 * kit_window_null.c (none).
 *
 * A plain HWND that owns a BuiRaster and drives a BKContext: mouse and
 * keyboard messages go straight to BK_* input, redraw requests coalesce into
 * one posted render message, and the damaged tiles are blitted with
 * BUI_raster_present_native. Nothing on the input -> render -> present path
 * leaves C. Animations run vsync-paced (DwmFlush).
 *
 * SPDX-License-Identifier: GPL-2.0-or-later */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "kit_intern.h"

#ifdef _WIN32

#  ifndef UNICODE
#    define UNICODE
#  endif
#  define WIN32_LEAN_AND_MEAN
#  include <windows.h>
#  include <windowsx.h>

#  include "kit_ime_win32.h"

#  define WM_BK_RENDER (WM_APP + 1)
#  define ID_TIP 1
#  define ID_ANIM 2
#  define ID_WAKE 3 /* BK_TIMER_CUSTOM: custom nodes' frames */
#  define ID_GPU 4  /* checks the GPU device until it is ready */
#  define ID_USER_BASE 1000

struct BKWindow {
  HWND hwnd;
  BKContext *ctx;
  BKHost prev; /* the host's own callbacks (emit) */
  BuiRaster *raster;
  BKWindowCallbacks cb;
  int render_pending, frames, cursor, tip_id, next_timer, tracking, full;
  int want_frame, frame_vsync; /* BK_window_request_frame */
  LARGE_INTEGER frame_start;    /* when the last animation frame began */
  float scale;
  int min_w, min_h, max_w, max_h; /* client size limits (logical px; 0 = none) */
  int fixed_w, fixed_h;           /* axes the user may not resize (Tk's resizable) */
  int user_sizing;                /* inside a user move/resize loop */
  DWORD style;                    /* framed style restored by "override" 0 */
  BKImeWin32 ime;                 /* the IME (GHOST_ImeWin32) */
  WPARAM last_vk;                 /* the virtual key of the last WM_KEYDOWN */
  int titlebar;                   /* "titlebar": the host draws it; no caption */
  int nc_tracking;                /* TME_NONCLIENT leave asked (the maximise button) */
  BKAreas areas;                  /* BK_window_set_areas */
};

static int g_open = 0;

/* Wake BK_window_run's GetMessage without WM_QUIT: a quit message would also
 * end any other loop on this thread (e.g. a Tk mainloop in the same app). */
static void wake_loop(void)
{
  PostThreadMessageW(GetCurrentThreadId(), WM_NULL, 0, 0);
}
static int g_quit = 0;
static int g_keep_alive = 0;
static int (*g_stop)(void) = NULL;

/* -- cross-thread wake: a message-only window on the loop's thread ------------------- */
#  define WM_BK_WAKE (WM_APP + 7)
static HWND g_wake_hwnd = NULL;
static void (*g_wake_cb)(void) = NULL;
static volatile LONG g_wake_pending = 0;

static LRESULT CALLBACK wake_proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp)
{
  if (msg == WM_BK_WAKE) {
    InterlockedExchange(&g_wake_pending, 0); /* wakes after this point post again */
    BK_image_poll(); /* decoded images: damage what shows them */
    if (g_wake_cb) {
      g_wake_cb();
    }
    return 0;
  }
  return DefWindowProcW(hwnd, msg, wp, lp);
}

int bk_window_wake_ready(void)
{
  if (g_wake_hwnd) {
    return 1;
  }
  WNDCLASSW wc = {0};
  wc.lpfnWndProc = wake_proc;
  wc.hInstance = GetModuleHandleW(NULL);
  wc.lpszClassName = L"BUIKitWake";
  RegisterClassW(&wc); /* fails harmlessly when already registered */
  g_wake_hwnd = CreateWindowExW(0, L"BUIKitWake", L"", 0, 0, 0, 0, 0, HWND_MESSAGE, NULL,
                                wc.hInstance, NULL);
  return g_wake_hwnd != NULL;
}

int BK_window_wake_init(void (*cb)(void))
{
  g_wake_cb = cb;
  return bk_window_wake_ready();
}

void BK_window_wake(void)
{
  if (g_wake_hwnd && InterlockedExchange(&g_wake_pending, 1) == 0) {
    PostMessageW(g_wake_hwnd, WM_BK_WAKE, 0, 0);
  }
}

void BK_window_set_keep_alive(int on)
{
  g_keep_alive = on;
}
static const wchar_t *k_class = L"BUIKitWindow";

int BK_window_supported(void)
{
  return 1;
}

static void dpi_awareness(void)
{
  static int done = 0;
  if (done) {
    return;
  }
  done = 1;
  HMODULE user32 = GetModuleHandleW(L"user32.dll");
  typedef BOOL(WINAPI * SetCtx)(HANDLE);
  SetCtx set_ctx = user32 ? (SetCtx)(void *)GetProcAddress(user32,
                                                           "SetProcessDpiAwarenessContext") :
                            NULL;
  if (!set_ctx || !set_ctx((HANDLE)-4 /* PER_MONITOR_AWARE_V2 */)) {
    SetProcessDPIAware();
  }
}

static float window_dpi_scale(HWND hwnd)
{
  const char *env = getenv("BUI_UI_SCALE");
  if (env && atof(env) > 0) {
    return (float)atof(env);
  }
  HMODULE user32 = GetModuleHandleW(L"user32.dll");
  typedef UINT(WINAPI * GetDpi)(HWND);
  GetDpi get = user32 ? (GetDpi)(void *)GetProcAddress(user32, "GetDpiForWindow") : NULL;
  const UINT dpi = (get && hwnd) ? get(hwnd) : 96;
  return BK_MAX(1.0f, dpi / 96.0f);
}

/* GetSystemMetricsForDpi (Windows 10 1607+), else the system DPI's metric. */
static int metric_for(HWND hwnd, int index)
{
  HMODULE user32 = GetModuleHandleW(L"user32.dll");
  typedef UINT(WINAPI * GetDpi)(HWND);
  typedef int(WINAPI * MetricFn)(int, UINT);
  GetDpi get = user32 ? (GetDpi)(void *)GetProcAddress(user32, "GetDpiForWindow") : NULL;
  MetricFn fn = user32 ? (MetricFn)(void *)GetProcAddress(user32, "GetSystemMetricsForDpi") :
                         NULL;
  return (get && fn && hwnd) ? fn(index, get(hwnd)) : GetSystemMetrics(index);
}

/* The resize border's thickness: how far a maximised window reaches past the monitor's
 * edge, and the top resize border of a window whose title bar the host draws. */
static int frame_border(HWND hwnd)
{
  return metric_for(hwnd, SM_CYFRAME) + metric_for(hwnd, SM_CXPADDEDBORDER);
}

/* The host draws the title bar: a framed window that asked for it ("titlebar"). */
static int own_titlebar(BKWindow *win)
{
  return win->titlebar && (GetWindowLongPtrW(win->hwnd, GWL_STYLE) & WS_CAPTION);
}

/* AdjustWindowRect for `win`: its content grows into the caption it no longer has. */
static void adjust_rect(BKWindow *win, RECT *r, DWORD style)
{
  const LONG top = r->top;
  AdjustWindowRect(r, style, FALSE);
  if (win->titlebar && (style & WS_CAPTION)) {
    r->top = top;
  }
}

/* A screen point in client coordinates. */
static POINT client_point(HWND hwnd, LPARAM lp)
{
  POINT p = {GET_X_LPARAM(lp), GET_Y_LPARAM(lp)};
  ScreenToClient(hwnd, &p);
  return p;
}

static wchar_t *utf8_to_wide(const char *s)
{
  const int n = MultiByteToWideChar(CP_UTF8, 0, s ? s : "", -1, NULL, 0);
  wchar_t *w = (wchar_t *)malloc(sizeof(wchar_t) * (size_t)n);
  MultiByteToWideChar(CP_UTF8, 0, s ? s : "", -1, w, n);
  return w;
}

static char *wide_to_utf8(const wchar_t *w, int len)
{
  const int n = WideCharToMultiByte(CP_UTF8, 0, w, len, NULL, 0, NULL, NULL);
  char *s = (char *)malloc((size_t)n + 1);
  WideCharToMultiByte(CP_UTF8, 0, w, len, s, n, NULL, NULL);
  s[n] = 0;
  return s;
}

/* -- host services -- */

static void host_request(void *data)
{
  BKWindow *win = (BKWindow *)data;
  if (win->hwnd && !win->render_pending) {
    win->render_pending = 1;
    PostMessageW(win->hwnd, WM_BK_RENDER, 0, 0);
  }
}

static HCURSOR cursor_handle(int c)
{
  LPCWSTR id = IDC_ARROW;
  switch (c) {
    case BK_CURSOR_EW:
      id = IDC_SIZEWE;
      break;
    case BK_CURSOR_NS:
      id = IDC_SIZENS;
      break;
    case BK_CURSOR_TEXT:
      id = IDC_IBEAM;
      break;
    case BK_CURSOR_CROSSHAIR:
      id = IDC_CROSS;
      break;
    case BK_CURSOR_HAND:
      id = IDC_HAND;
      break;
    case BK_CURSOR_MOVE:
      id = IDC_SIZEALL;
      break;
    case BK_CURSOR_NESW:
      id = IDC_SIZENESW;
      break;
    case BK_CURSOR_NWSE:
      id = IDC_SIZENWSE;
      break;
    case BK_CURSOR_NOT_ALLOWED:
      id = IDC_NO;
      break;
    case BK_CURSOR_WAIT:
      id = IDC_WAIT;
      break;
    case BK_CURSOR_PROGRESS:
      id = IDC_APPSTARTING;
      break;
    case BK_CURSOR_NONE:
      return NULL;
  }
  return LoadCursorW(NULL, id);
}

static void host_set_cursor(void *data, int cursor)
{
  BKWindow *win = (BKWindow *)data;
  win->cursor = cursor;
  SetCursor(cursor_handle(cursor));
}

static void host_warp(void *data, int x, int y)
{
  BKWindow *win = (BKWindow *)data;
  POINT p = {x, y};
  ClientToScreen(win->hwnd, &p);
  SetCursorPos(p.x, p.y);
}

static char *host_clipboard_get(void *data)
{
  BKWindow *win = (BKWindow *)data;
  char *out = NULL;
  if (!OpenClipboard(win->hwnd)) {
    return NULL;
  }
  HANDLE h = GetClipboardData(CF_UNICODETEXT);
  if (h) {
    const wchar_t *w = (const wchar_t *)GlobalLock(h);
    if (w) {
      out = wide_to_utf8(w, -1);
      GlobalUnlock(h);
    }
  }
  CloseClipboard();
  return out;
}

static void host_clipboard_set(void *data, const char *text)
{
  BKWindow *win = (BKWindow *)data;
  wchar_t *w = utf8_to_wide(text);
  const size_t bytes = (wcslen(w) + 1) * sizeof(wchar_t);
  if (OpenClipboard(win->hwnd)) {
    EmptyClipboard();
    HGLOBAL h = GlobalAlloc(GMEM_MOVEABLE, bytes);
    if (h) {
      memcpy(GlobalLock(h), w, bytes);
      GlobalUnlock(h);
      SetClipboardData(CF_UNICODETEXT, h);
    }
    CloseClipboard();
  }
  free(w);
}

static void host_schedule(void *data, int ms, int timer)
{
  BKWindow *win = (BKWindow *)data;
  if (timer == BK_TIMER_CUSTOM) { /* its own timer: tooltips keep theirs */
    SetTimer(win->hwnd, ID_WAKE, (UINT)(ms > 0 ? ms : 1), NULL);
    return;
  }
  win->tip_id = timer;
  SetTimer(win->hwnd, ID_TIP, (UINT)ms, NULL);
}

static void host_emit(void *data, BKNode *node, int kind, void *ref, double a, double b)
{
  BKWindow *win = (BKWindow *)data;
  if (win->prev.emit) {
    win->prev.emit(win->prev.data, node, kind, ref, a, b);
  }
}

/* -- rendering -- */

typedef HRESULT(WINAPI *DwmFlushFn)(void);

static void vsync(void)
{
  static DwmFlushFn flush = NULL;
  static int loaded = 0;
  if (!loaded) {
    loaded = 1;
    HMODULE dwm = LoadLibraryW(L"dwmapi.dll");
    flush = dwm ? (DwmFlushFn)(void *)GetProcAddress(dwm, "DwmFlush") : NULL;
  }
  if (flush) {
    flush();
  }
  else {
    Sleep(8);
  }
}

/* The primary display's refresh interval in ms. */
static double refresh_ms(void)
{
  static double ms = 0.0;
  if (ms <= 0.0) {
    DEVMODEW dm;
    memset(&dm, 0, sizeof(dm));
    dm.dmSize = sizeof(dm);
    const int hz = EnumDisplaySettingsW(NULL, ENUM_CURRENT_SETTINGS, &dm) ? (int)dm.dmDisplayFrequency : 0;
    ms = 1000.0 / (hz > 1 ? hz : 60);
  }
  return ms;
}

/* The device starts on a worker thread. The timer switches the window over when it is ready. */
static void gpu_attach(BKWindow *win)
{
  if (BUI_raster_gpu_attach(win->raster, win->hwnd, 0)) {
    SetTimer(win->hwnd, ID_GPU, (UINT)refresh_ms(), NULL);
  }
}

/* Milliseconds since the current animation frame began. */
static double frame_ms(const BKWindow *win)
{
  LARGE_INTEGER now, freq;
  QueryPerformanceCounter(&now);
  QueryPerformanceFrequency(&freq);
  return (double)(now.QuadPart - win->frame_start.QuadPart) * 1000.0 / (double)freq.QuadPart;
}

/* Tells the platform what the kit's IME state needs (wm_window.cc WM_window_IME_begin / _reposition /
 * _end): on while a field takes text, restarted when another field takes over, the candidate
 * window following the caret. BK_ime_state reports only changes, so this costs nothing otherwise. */
static void ime_sync(BKWindow *win)
{
  BKImeState s;
  if (!BK_ime_state(win->ctx, &s)) {
    return;
  }
  if (!s.enabled) {
    bk_ime_win32_end(&win->ime, win->hwnd);
    return;
  }
  if (s.restart) {
    bk_ime_win32_end(&win->ime, win->hwnd); /* textedit_ime_begin ends first, then begins */
  }
  bk_ime_win32_begin(&win->ime, win->hwnd, s.x, s.y, s.x, s.y, s.restart != 0);
}

static void do_render(BKWindow *win)
{
  RECT rc;
  GetClientRect(win->hwnd, &rc);
  const int w = BK_MAX(1, rc.right - rc.left), h = BK_MAX(1, rc.bottom - rc.top);
  const int resized = BUI_raster_width(win->raster) != w || BUI_raster_height(win->raster) != h;
  /* BUI_PROFILE: kit raster and present times, averaged over 60 frames */
  static int prof = -1;
  static double acc_render, acc_present;
  static int acc_n;
  LARGE_INTEGER t0, t1, t2, freq;
  if (prof < 0) {
    prof = getenv("BUI_PROFILE") != NULL;
  }
  QueryPerformanceCounter(&t0);
  if (BUI_raster_gpu_poll(win->raster)) {
    win->full = 1; /* the renderer changed: its frame starts empty */
  }
  const int regions = BK_render(win->ctx, win->raster, w, h, win->full);
  ime_sync(win); /* the draw placed the caret */
  QueryPerformanceCounter(&t1);
  win->full = 0;
  if (regions || resized) {
    BUI_raster_present_native(win->raster, win->hwnd, resized);
    win->frames++;
  }
  QueryPerformanceCounter(&t2);
  if (prof) {
    QueryPerformanceFrequency(&freq);
    acc_render += (double)(t1.QuadPart - t0.QuadPart) * 1000.0 / (double)freq.QuadPart;
    acc_present += (double)(t2.QuadPart - t1.QuadPart) * 1000.0 / (double)freq.QuadPart;
    if (++acc_n == 60) {
      fprintf(stderr, "kit ms: raster %.2f  present %.2f  (%dx%d)\n", acc_render / 60,
              acc_present / 60, w, h);
      acc_render = acc_present = 0;
      acc_n = 0;
    }
  }
  if (BK_animating(win->ctx)) {
    vsync(); /* pace animation frames to the display */
    host_request(win);
  }
}

/* -- input mapping -- */

static int mods_now(WPARAM wp, int mouse)
{
  int m = 0;
  if (GetKeyState(VK_SHIFT) < 0) {
    m |= BK_SHIFT;
  }
  if (GetKeyState(VK_CONTROL) < 0) {
    m |= BK_CTRL;
  }
  if (GetKeyState(VK_MENU) < 0) {
    m |= BK_ALT;
  }
  if (mouse && (wp & MK_LBUTTON)) {
    m |= BK_BUTTON1;
  }
  return m;
}

static const char *vk_keysym(WPARAM vk, char *buf)
{
  switch (vk) {
    case VK_LEFT: return "Left";
    case VK_RIGHT: return "Right";
    case VK_UP: return "Up";
    case VK_DOWN: return "Down";
    case VK_HOME: return "Home";
    case VK_END: return "End";
    case VK_PRIOR: return "Prior";
    case VK_NEXT: return "Next";
    case VK_RETURN: return "Return";
    case VK_ESCAPE: return "Escape";
    case VK_BACK: return "BackSpace";
    case VK_DELETE: return "Delete";
    case VK_TAB: return "Tab";
    case VK_INSERT: return "Insert";
    case VK_SHIFT: return "Shift_L";
    case VK_CONTROL: return "Control_L";
    case VK_MENU: return "Alt_L";
    case VK_SUBTRACT: return "KP_Subtract";
    case VK_OEM_MINUS: return "minus";
    case VK_SPACE: return "space";
  }
  if (vk >= VK_F1 && vk <= VK_F24) {
    snprintf(buf, 8, "F%d", (int)(vk - VK_F1 + 1));
    return buf;
  }
  if ((vk >= 'A' && vk <= 'Z') || (vk >= '0' && vk <= '9')) {
    buf[0] = (char)((vk >= 'A' && vk <= 'Z') ? vk - 'A' + 'a' : vk);
    buf[1] = 0;
    return buf;
  }
  return NULL;
}

static LRESULT CALLBACK wndproc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp)
{
  BKWindow *win = (BKWindow *)GetWindowLongPtrW(hwnd, GWLP_USERDATA);
  if (!win) {
    return DefWindowProcW(hwnd, msg, wp, lp);
  }
  BKContext *ctx = win->ctx;
  switch (msg) {
    case WM_BK_RENDER:
      { /* a new size: the host lays out for it before this frame, so no frame shows the
         * last layout at the new size (centred text would jump back and forth) */
        RECT rc;
        GetClientRect(win->hwnd, &rc);
        if (win->cb.on_frame && (BUI_raster_width(win->raster) != BK_MAX(1, rc.right - rc.left) ||
                                 BUI_raster_height(win->raster) != BK_MAX(1, rc.bottom - rc.top)))
        {
          win->want_frame = 1;
        }
      }
      if (win->want_frame && win->cb.on_frame) {
        /* render_pending is still set: edits made here join this frame */
        win->want_frame = 0;
        QueryPerformanceCounter(&win->frame_start);
        win->cb.on_frame(win->cb.data, win);
      }
      win->render_pending = 0;
      do_render(win);
      if (win->want_frame) { /* the host asked for the next frame */
        bk_frame_ms = frame_ms(win);
        bk_frame_budget_ms = win->frame_vsync ? refresh_ms() : 0.0;
        if (win->frame_vsync && bk_frame_ms < refresh_ms() - 1.5) {
          vsync(); /* early: wait for the next refresh; late: start right away */
        }
        host_request(win);
      }
      else {
        bk_frame_ms = 0.0; /* the animation ended */
      }
      return 0;
    case WM_PAINT: {
      PAINTSTRUCT ps;
      BeginPaint(hwnd, &ps);
      EndPaint(hwnd, &ps);
      win->full = 1;
      do_render(win);
      BUI_raster_present_native(win->raster, hwnd, 1);
      return 0;
    }
    case WM_ERASEBKGND:
      return 1;
    case WM_NCCALCSIZE:
      if (wp && own_titlebar(win)) {
        /* no caption: the content starts at the window's top; the side and bottom resize
         * borders stay the system's (invisible since Windows 10, with its shadow). A
         * maximised window reaches past the monitor by the border, so it starts below it. */
        NCCALCSIZE_PARAMS *p = (NCCALCSIZE_PARAMS *)lp;
        const LONG top = p->rgrc[0].top;
        const LRESULT r = DefWindowProcW(hwnd, msg, wp, lp);
        p->rgrc[0].top = top + (IsZoomed(hwnd) ? frame_border(hwnd) : 0);
        return r;
      }
      break;
    case WM_NCHITTEST:
      if (own_titlebar(win)) {
        const LRESULT sys = DefWindowProcW(hwnd, msg, wp, lp); /* side and bottom borders */
        if (sys != HTCLIENT) {
          return sys;
        }
        const POINT p = client_point(hwnd, lp);
        const int area = BK_areas_at(win->areas.v, win->areas.n, (float)p.x, (float)p.y);
        switch (BK_frame_hit(area, p.y, frame_border(hwnd), IsZoomed(hwnd))) {
          case BK_HIT_TOP:
            return HTTOP;
          case BK_HIT_CAPTION:
            return HTCAPTION;
          case BK_HIT_MAXBUTTON:
            return HTMAXBUTTON; /* Windows 11 offers its snap layouts on hover */
          default:
            return HTCLIENT;
        }
      }
      break;
    /* The maximise button is HTMAXBUTTON, so its pointer messages are non-client ones: the
     * host hears them as its own, so the button hovers and clicks as the others do. */
    case WM_NCMOUSEMOVE:
      if (own_titlebar(win)) {
        if (wp == HTMAXBUTTON) {
          if (!win->nc_tracking) {
            TRACKMOUSEEVENT t = {sizeof(t), TME_LEAVE | TME_NONCLIENT, hwnd, 0};
            TrackMouseEvent(&t);
            win->nc_tracking = 1;
          }
          const POINT p = client_point(hwnd, lp);
          BK_motion(ctx, p.x, p.y, mods_now(0, 0));
        }
        else {
          BK_motion(ctx, -100000, -100000, 0); /* the caption: nothing of the host's */
        }
      }
      break;
    case WM_NCMOUSELEAVE:
      if (win->nc_tracking) {
        win->nc_tracking = 0;
        if (GetCapture() != hwnd) { /* wherever it went: the content's hover follows */
          POINT p;
          GetCursorPos(&p);
          ScreenToClient(hwnd, &p);
          BK_motion(ctx, p.x, p.y, 0);
        }
      }
      break;
    case WM_NCLBUTTONDOWN:
    case WM_NCLBUTTONDBLCLK:
      if (own_titlebar(win) && wp == HTMAXBUTTON) {
        const POINT p = client_point(hwnd, lp);
        BK_press(ctx, p.x, p.y, mods_now(0, 0) | BK_BUTTON1);
        return 0; /* not the system's: the host's click maximises */
      }
      break;
    case WM_NCLBUTTONUP:
      if (own_titlebar(win) && wp == HTMAXBUTTON) {
        const POINT p = client_point(hwnd, lp);
        BK_release(ctx, p.x, p.y, mods_now(0, 0));
        return 0;
      }
      break;
    case WM_SIZE:
      host_request(win);
      return 0;
    case WM_ACTIVATE: /* BK_window_focused changed: the host's on_frame reads it */
      BK_window_request_frame(win, 0);
      break; /* DefWindowProc sets the keyboard focus */
    case WM_DPICHANGED: {
      /* the window's scale follows the monitor unless BUI_UI_SCALE pins it; the
       * Rust layer takes the new scale on its next frame (Window::take_native_scale) */
      if (!getenv("BUI_UI_SCALE")) {
        win->scale = HIWORD(wp) / 96.0f;
        BK_set_scale(win->scale);
      }
      const RECT *r = (const RECT *)lp;
      SetWindowPos(hwnd, NULL, r->left, r->top, r->right - r->left, r->bottom - r->top,
                   SWP_NOZORDER | SWP_NOACTIVATE);
      BK_node_relayout(ctx->root);
      host_request(win);
      return 0;
    }
    case WM_SETCURSOR:
      if (LOWORD(lp) == HTCLIENT) {
        SetCursor(cursor_handle(win->cursor));
        return TRUE;
      }
      break;
    case WM_MOUSEMOVE:
      if (!win->tracking) {
        TRACKMOUSEEVENT t = {sizeof(t), TME_LEAVE, hwnd, 0};
        TrackMouseEvent(&t);
        win->tracking = 1;
      }
      BK_motion(ctx, GET_X_LPARAM(lp), GET_Y_LPARAM(lp), mods_now(wp, 1));
      return 0;
    case WM_MOUSELEAVE:
      win->tracking = 0;
      if (GetCapture() != hwnd) {
        BK_motion(ctx, -100000, -100000, 0);
      }
      return 0;
    case WM_LBUTTONDOWN:
      SetCapture(hwnd);
      SetFocus(hwnd);
      BK_press(ctx, GET_X_LPARAM(lp), GET_Y_LPARAM(lp), mods_now(wp, 1));
      return 0;
    case WM_LBUTTONUP:
      ReleaseCapture();
      BK_release(ctx, GET_X_LPARAM(lp), GET_Y_LPARAM(lp), mods_now(wp, 1));
      return 0;
    case WM_RBUTTONDOWN:
      BK_context_menu(ctx, GET_X_LPARAM(lp), GET_Y_LPARAM(lp), mods_now(wp, 1));
      return 0;
    case WM_RBUTTONUP:
    case WM_MBUTTONDOWN:
    case WM_MBUTTONUP:
      BK_button(ctx, GET_X_LPARAM(lp), GET_Y_LPARAM(lp), msg == WM_RBUTTONUP ? 3 : 2,
                msg != WM_MBUTTONDOWN, mods_now(wp, 1));
      return 0;
    case WM_XBUTTONDOWN: /* the side buttons; TRUE: handled, no WM_APPCOMMAND after */
    case WM_XBUTTONUP:
      BK_button(ctx, GET_X_LPARAM(lp), GET_Y_LPARAM(lp),
                GET_XBUTTON_WPARAM(wp) == XBUTTON1 ? BK_BUTTON_BACK : BK_BUTTON_FORWARD,
                msg == WM_XBUTTONUP, mods_now(GET_KEYSTATE_WPARAM(wp), 1));
      return TRUE;
    case WM_KEYUP:
    case WM_SYSKEYUP: {
      char buf[8];
      const char *ks = vk_keysym(wp, buf);
      if (ks) {
        BK_key_release(ctx, ks, mods_now(0, 0));
      }
      break;
    }
    case WM_ENTERSIZEMOVE:
    case WM_EXITSIZEMOVE:
      win->user_sizing = msg == WM_ENTERSIZEMOVE;
      break;
    case WM_GETMINMAXINFO: {
      MINMAXINFO *mm = (MINMAXINFO *)lp;
      if (win->user_sizing && (win->fixed_w || win->fixed_h)) { /* only the user is limited */
        RECT wr;
        GetWindowRect(hwnd, &wr);
        if (win->fixed_w) {
          mm->ptMinTrackSize.x = mm->ptMaxTrackSize.x = wr.right - wr.left;
        }
        if (win->fixed_h) {
          mm->ptMinTrackSize.y = mm->ptMaxTrackSize.y = wr.bottom - wr.top;
        }
        return 0;
      }
      const DWORD style = (DWORD)GetWindowLongPtrW(hwnd, GWL_STYLE);
      if (win->min_w || win->min_h) {
        RECT r = {0, 0, (LONG)(win->min_w * win->scale), (LONG)(win->min_h * win->scale)};
        adjust_rect(win, &r, style);
        mm->ptMinTrackSize.x = win->min_w ? r.right - r.left : mm->ptMinTrackSize.x;
        mm->ptMinTrackSize.y = win->min_h ? r.bottom - r.top : mm->ptMinTrackSize.y;
      }
      if (win->max_w || win->max_h) {
        RECT r = {0, 0, (LONG)(win->max_w * win->scale), (LONG)(win->max_h * win->scale)};
        adjust_rect(win, &r, style);
        mm->ptMaxTrackSize.x = win->max_w ? r.right - r.left : mm->ptMaxTrackSize.x;
        mm->ptMaxTrackSize.y = win->max_h ? r.bottom - r.top : mm->ptMaxTrackSize.y;
      }
      return 0;
    }
    case WM_MOUSEWHEEL: {
      POINT p = {GET_X_LPARAM(lp), GET_Y_LPARAM(lp)};
      ScreenToClient(hwnd, &p);
      /* precise: touchpads send fractions of a notch (WHEEL_DELTA) */
      BK_scroll(ctx, p.x, p.y, 0.0f, (float)GET_WHEEL_DELTA_WPARAM(wp) / WHEEL_DELTA,
                mods_now(0, 0));
      return 0;
    }
    case WM_MOUSEHWHEEL: { /* tilt wheel / sideways swipe: > 0 right */
      POINT p = {GET_X_LPARAM(lp), GET_Y_LPARAM(lp)};
      ScreenToClient(hwnd, &p);
      BK_scroll(ctx, p.x, p.y, (float)GET_WHEEL_DELTA_WPARAM(wp) / WHEEL_DELTA, 0.0f,
                mods_now(0, 0));
      return 0;
    }
    /* -- input methods: GHOST_SystemWin32.cc's IME cases, see kit_ime_win32.h -- */
    case WM_INPUTLANGCHANGE:
      bk_ime_win32_update_input_language(&win->ime);
      bk_ime_win32_update_conversion_status(&win->ime, hwnd);
      break;
    case WM_IME_NOTIFY: /* conversion status when the IME or its input mode changes */
      if (wp == IMN_SETOPENSTATUS || wp == IMN_SETCONVERSIONMODE) {
        bk_ime_win32_update_conversion_status(&win->ime, hwnd);
      }
      break;
    case WM_IME_SETCONTEXT:
      bk_ime_win32_update_input_language(&win->ime);
      bk_ime_win32_update_conversion_status(&win->ime, hwnd);
      bk_ime_win32_create_window(&win->ime, hwnd);
      bk_ime_win32_cleanup_composition(&win->ime, hwnd);
      bk_ime_win32_check_first(&win->ime, hwnd);
      break;
    case WM_IME_STARTCOMPOSITION: {
      BKImeEvent e;
      bk_ime_win32_create_window(&win->ime, hwnd);
      bk_ime_win32_reset_composition(&win->ime);
      bk_ime_win32_empty_event(&win->ime, &e);
      BK_ime_event(ctx, BK_IME_COMPOSITION_START, &e);
      return 0; /* no DefWindowProc: the kit draws the composition, not the system */
    }
    case WM_IME_COMPOSITION: {
      BKImeEvent e;
      bk_ime_win32_update_window(&win->ime, hwnd);
      bk_ime_win32_update_info(&win->ime, hwnd, &e);
      BK_ime_event(ctx, BK_IME_COMPOSITION, &e);
      return 0;
    }
    case WM_IME_ENDCOMPOSITION: {
      BKImeEvent e;
      bk_ime_win32_reset_composition(&win->ime);
      bk_ime_win32_destroy_window(&win->ime, hwnd);
      bk_ime_win32_empty_event(&win->ime, &e);
      BK_ime_event(ctx, BK_IME_COMPOSITION_END, &e);
      return 0;
    }
    case WM_KEYDOWN:
    case WM_SYSKEYDOWN: {
      win->last_vk = wp;
      char buf[8];
      const int mods = mods_now(0, 0);
      const char *ks = vk_keysym(wp, buf);
      const int is_char_key = ks && ((wp >= 'A' && wp <= 'Z') || (wp >= '0' && wp <= '9') ||
                                     wp == VK_SPACE || wp == VK_OEM_MINUS);
      /* printable keys arrive as WM_CHAR unless Ctrl/Alt are held */
      if (ks && (!is_char_key || (mods & (BK_CTRL | BK_ALT)))) {
        if (BK_key(ctx, "", ks, mods)) {
          return 0;
        }
      }
      if (msg == WM_SYSKEYDOWN) {
        break; /* Alt+F4 and the system menu */
      }
      return 0;
    }
    case WM_CHAR: {
      static wchar_t high = 0;
      wchar_t wc[3] = {(wchar_t)wp, 0, 0};
      if (wp >= 0xD800 && wp <= 0xDBFF) {
        high = (wchar_t)wp;
        return 0;
      }
      if (wp >= 0xDC00 && wp <= 0xDFFF && high) {
        wc[0] = high;
        wc[1] = (wchar_t)wp;
        high = 0;
      }
      if (wc[0] < 32 || wc[0] == 127 || (GetKeyState(VK_CONTROL) < 0 && GetKeyState(VK_MENU) >= 0)) {
        return 0; /* control characters: handled as keys */
      }
      /* a character key the IME handles itself must not also be typed (IsImeKeyEvent) */
      if (wc[0] < 0x80 &&
          bk_ime_win32_is_ime_key_event(&win->ime, (char)wc[0], win->last_vk == VK_DECIMAL))
      {
        return 0;
      }
      char *text = wide_to_utf8(wc, -1);
      const char *keysym = (text[0] == ' ' && !text[1]) ? "space" :
                           (text[0] == '-' && !text[1]) ? "minus" :
                                                          text;
      BK_key(ctx, text, keysym, mods_now(0, 0));
      free(text);
      return 0;
    }
    case WM_TIMER:
      if (wp == ID_TIP) {
        KillTimer(hwnd, ID_TIP);
        BK_timer(ctx, win->tip_id);
      }
      else if (wp == ID_WAKE) {
        KillTimer(hwnd, ID_WAKE);
        BK_timer(ctx, BK_TIMER_CUSTOM);
      }
      else if (wp == ID_GPU) {
        /* a GPU device that is ready switches an idle window over too */
        if (!BUI_raster_gpu_pending(win->raster)) {
          KillTimer(hwnd, ID_GPU);
        }
        else if (BUI_raster_gpu_poll(win->raster)) {
          KillTimer(hwnd, ID_GPU);
          win->full = 1;
          host_request(win);
        }
      }
      else if (wp >= ID_USER_BASE) {
        KillTimer(hwnd, wp);
        if (win->cb.on_timer) {
          win->cb.on_timer(win->cb.data, win, (int)(wp - ID_USER_BASE));
        }
      }
      return 0;
    case WM_CLOSE:
      if (win->cb.on_close && !win->cb.on_close(win->cb.data, win)) {
        return 0;
      }
      DestroyWindow(hwnd);
      return 0;
    case WM_DESTROY:
      win->hwnd = NULL;
      if (--g_open <= 0) {
        wake_loop();
      }
      return 0;
  }
  return DefWindowProcW(hwnd, msg, wp, lp);
}

BKWindow *BK_window_new(BKContext *ctx, const char *title, int width, int height)
{
  dpi_awareness();
  static int registered = 0;
  HINSTANCE inst = GetModuleHandleW(NULL);
  if (!registered) {
    WNDCLASSEXW wc = {sizeof(wc)};
    wc.lpfnWndProc = wndproc;
    wc.hInstance = inst;
    wc.hCursor = NULL; /* WM_SETCURSOR */
    wc.lpszClassName = k_class;
    wc.hIcon = LoadIconW(NULL, (LPCWSTR)IDI_APPLICATION);
    if (!RegisterClassExW(&wc)) {
      return NULL;
    }
    registered = 1;
  }
  BKWindow *win = (BKWindow *)calloc(1, sizeof(BKWindow));
  bk_ime_win32_init(&win->ime);
  win->ctx = ctx;
  win->prev = ctx->host;
  win->raster = BUI_raster_create(1, 1);
  win->next_timer = 1;
  win->scale = window_dpi_scale(NULL);
  {
    /* system DPI until the window exists */
    HDC dc = GetDC(NULL);
    const int dpi = GetDeviceCaps(dc, LOGPIXELSX);
    ReleaseDC(NULL, dc);
    if (!getenv("BUI_UI_SCALE")) {
      win->scale = BK_MAX(1.0f, dpi / 96.0f);
    }
  }
  BK_set_scale(win->scale);
  RECT r = {0, 0, (LONG)(width * win->scale), (LONG)(height * win->scale)};
  AdjustWindowRect(&r, WS_OVERLAPPEDWINDOW, FALSE);
  wchar_t *wt = utf8_to_wide(title);
  win->hwnd = CreateWindowExW(0, k_class, wt, WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                              CW_USEDEFAULT, CW_USEDEFAULT, r.right - r.left, r.bottom - r.top,
                              NULL, NULL, inst, NULL);
  free(wt);
  if (!win->hwnd) {
    BUI_raster_free(win->raster);
    free(win);
    return NULL;
  }
  SetWindowLongPtrW(win->hwnd, GWLP_USERDATA, (LONG_PTR)win);
  g_open++;
  if (BUI_gpu_default()) {
    gpu_attach(win);
  }
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
  if (win->hwnd) {
    SetWindowLongPtrW(win->hwnd, GWLP_USERDATA, 0);
    DestroyWindow(win->hwnd);
    if (--g_open <= 0) {
      wake_loop();
    }
  }
  if (win->ctx) {
    BK_context_set_host(win->ctx, &win->prev);
  }
  BUI_raster_free(win->raster);
  bk_ime_win32_free(&win->ime);
  bk_areas_free(&win->areas);
  free(win);
}

void BK_window_set_callbacks(BKWindow *win, const BKWindowCallbacks *cb)
{
  win->cb = *cb;
}

void BK_window_show(BKWindow *win)
{
  if (win->hwnd) {
    ShowWindow(win->hwnd, SW_SHOW);
    UpdateWindow(win->hwnd);
  }
}

void BK_window_close(BKWindow *win)
{
  if (win->hwnd) {
    DestroyWindow(win->hwnd);
  }
}

int BK_window_is_open(BKWindow *win)
{
  return win && win->hwnd != NULL;
}

void BK_window_set_title(BKWindow *win, const char *title)
{
  if (win->hwnd) {
    wchar_t *w = utf8_to_wide(title);
    SetWindowTextW(win->hwnd, w);
    free(w);
  }
}

void BK_window_set_size(BKWindow *win, int width, int height)
{
  if (!win->hwnd) {
    return;
  }
  RECT r = {0, 0, (LONG)(width * win->scale), (LONG)(height * win->scale)};
  adjust_rect(win, &r, (DWORD)GetWindowLongPtrW(win->hwnd, GWL_STYLE));
  SetWindowPos(win->hwnd, NULL, 0, 0, r.right - r.left, r.bottom - r.top,
               SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
}

void BK_window_get_size(BKWindow *win, int *w, int *h)
{
  RECT rc = {0, 0, 0, 0};
  if (win->hwnd) {
    GetClientRect(win->hwnd, &rc);
  }
  *w = rc.right;
  *h = rc.bottom;
}

BuiRaster *BK_window_raster(BKWindow *win)
{
  return win->raster;
}

void BK_window_set_gpu(BKWindow *win, int on)
{
  if (win && on) {
    gpu_attach(win);
    host_request(win);
  }
}

int BK_window_gpu(BKWindow *win)
{
  return win && BUI_raster_is_gpu(win->raster);
}

int BK_window_frames(BKWindow *win)
{
  return win->frames;
}

void BK_window_render_now(BKWindow *win)
{
  if (win->hwnd) {
    win->render_pending = 0;
    do_render(win);
  }
}

int BK_window_timer(BKWindow *win, int ms)
{
  const int id = win->next_timer++;
  if (win->hwnd) {
    SetTimer(win->hwnd, ID_USER_BASE + (UINT_PTR)id, (UINT)BK_MAX(0, ms), NULL);
  }
  return id;
}

void BK_window_request_frame(BKWindow *win, int vsync)
{
  win->want_frame = 1;
  win->frame_vsync = vsync;
  host_request(win);
}

void BK_window_cancel_timer(BKWindow *win, int id)
{
  if (win->hwnd) {
    KillTimer(win->hwnd, ID_USER_BASE + (UINT_PTR)id);
  }
}

void *BK_window_handle(BKWindow *win)
{
  return win->hwnd;
}

int BK_window_focused(BKWindow *win)
{
  return win->hwnd && GetForegroundWindow() == win->hwnd;
}

/* Restore, then take the foreground. Windows only lets the foreground thread hand it over, so
 * attach to that thread's input for the call. The xdg-activation token has no meaning here. */
void BK_window_activate(BKWindow *win, const char *token)
{
  HWND h = win->hwnd;
  if (!h) {
    return;
  }
  (void)token;
  if (IsIconic(h)) {
    ShowWindow(h, SW_RESTORE);
  }
  else if (!IsWindowVisible(h)) {
    ShowWindow(h, SW_SHOW);
  }
  HWND fg = GetForegroundWindow();
  const DWORD me = GetCurrentThreadId();
  const DWORD fg_thread = fg ? GetWindowThreadProcessId(fg, NULL) : 0;
  const BOOL attached = fg_thread && fg_thread != me && AttachThreadInput(fg_thread, me, TRUE);
  SetForegroundWindow(h);
  BringWindowToTop(h);
  SetFocus(h);
  if (attached) {
    AttachThreadInput(fg_thread, me, FALSE);
  }
}

static void set_style(HWND hwnd, DWORD style)
{
  SetWindowLongPtrW(hwnd, GWL_STYLE, (LONG_PTR)(style | (GetWindowLongPtrW(hwnd, GWL_STYLE) &
                                                         WS_VISIBLE)));
  SetWindowPos(hwnd, NULL, 0, 0, 0, 0,
               SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
}

void BK_window_wm(BKWindow *win, const char *key, double a, double b)
{
  HWND h = win->hwnd;
  if (!h) {
    return;
  }
  if (!strcmp(key, "pos")) {
    SetWindowPos(h, NULL, (int)a, (int)b, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
  }
  else if (!strcmp(key, "minsize") || !strcmp(key, "maxsize")) {
    int *w = key[1] == 'i' ? &win->min_w : &win->max_w, *hh = key[1] == 'i' ? &win->min_h :
                                                                              &win->max_h;
    *w = (int)a;
    *hh = (int)b;
  }
  else if (!strcmp(key, "resizable")) {
    DWORD style = (DWORD)GetWindowLongPtrW(h, GWL_STYLE) & ~WS_VISIBLE;
    if (style & WS_CAPTION) {
      style = (a || b) ? (style | WS_THICKFRAME | WS_MAXIMIZEBOX) :
                         (style & ~(WS_THICKFRAME | WS_MAXIMIZEBOX));
      set_style(h, style);
    }
    win->fixed_w = a == 0.0;
    win->fixed_h = b == 0.0;
  }
  else if (!strcmp(key, "topmost")) {
    SetWindowPos(h, a ? HWND_TOPMOST : HWND_NOTOPMOST, 0, 0, 0, 0,
                 SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
  }
  else if (!strcmp(key, "alpha")) {
    const LONG_PTR ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
    SetWindowLongPtrW(h, GWL_EXSTYLE, ex | WS_EX_LAYERED);
    SetLayeredWindowAttributes(h, 0, (BYTE)(BK_CLAMP(a, 0.0, 1.0) * 255.0 + 0.5), LWA_ALPHA);
  }
  else if (!strcmp(key, "override")) {
    if (!win->style) {
      win->style = (DWORD)GetWindowLongPtrW(h, GWL_STYLE) & ~WS_VISIBLE;
    }
    set_style(h, a ? WS_POPUP | WS_CLIPCHILDREN : win->style);
  }
  else if (!strcmp(key, "state")) {
    static const int show[4] = {SW_RESTORE, SW_MINIMIZE, SW_HIDE, SW_MAXIMIZE};
    ShowWindow(h, show[BK_CLAMP((int)a, 0, 3)]);
  }
  else if (!strcmp(key, "lift") || !strcmp(key, "lower")) {
    SetWindowPos(h, key[1] == 'i' ? HWND_TOP : HWND_BOTTOM, 0, 0, 0, 0,
                 SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
  }
  else if (!strcmp(key, "disabled")) { /* modal dialogs disable their parent */
    EnableWindow(h, a == 0.0);
  }
  else if (!strcmp(key, "focus")) {
    SetForegroundWindow(h);
    SetFocus(h);
  }
  else if (!strcmp(key, "titlebar")) {
    if (win->titlebar == (a != 0.0)) {
      return;
    }
    RECT rc; /* the content keeps its size: the window loses (or gets back) the caption */
    GetClientRect(h, &rc);
    win->titlebar = a != 0.0;
    set_style(h, (DWORD)GetWindowLongPtrW(h, GWL_STYLE) & ~WS_VISIBLE);
    if (!IsZoomed(h) && !IsIconic(h)) {
      RECT r = {0, 0, rc.right, rc.bottom};
      adjust_rect(win, &r, (DWORD)GetWindowLongPtrW(h, GWL_STYLE));
      SetWindowPos(h, NULL, 0, 0, r.right - r.left, r.bottom - r.top,
                   SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
  }
}

void BK_window_set_areas(BKWindow *win, const BKWindowArea *areas, int count)
{
  bk_areas_set(&win->areas, areas, count);
}

int BK_window_wm_get(BKWindow *win, const char *key, double *r_a, double *r_b)
{
  *r_a = *r_b = 0.0;
  if (!strcmp(key, "screen")) {
    *r_a = GetSystemMetrics(SM_CXSCREEN);
    *r_b = GetSystemMetrics(SM_CYSCREEN);
    return 1;
  }
  if (!strcmp(key, "pointer")) {
    POINT p;
    GetCursorPos(&p);
    *r_a = p.x;
    *r_b = p.y;
    return 1;
  }
  if (!strcmp(key, "scale")) {
    *r_a = win ? win->scale : 1.0;
    return 1;
  }
  if (!win || !win->hwnd) {
    return 0;
  }
  if (!strcmp(key, "pos")) { /* client origin on screen */
    POINT p = {0, 0};
    ClientToScreen(win->hwnd, &p);
    *r_a = p.x;
    *r_b = p.y;
    return 1;
  }
  if (!strcmp(key, "state")) {
    *r_a = IsIconic(win->hwnd) ? 1 : !IsWindowVisible(win->hwnd) ? 2 : IsZoomed(win->hwnd) ? 3 :
                                                                                           0;
    return 1;
  }
  return 0;
}

void BK_window_set_stop_check(int (*stop)(void))
{
  g_stop = stop;
}

void BK_window_quit(void)
{
  g_quit = 1;
  wake_loop();
}

void BK_window_run(void)
{
  g_quit = 0;
  MSG msg;
  while ((g_open > 0 || g_keep_alive) && !g_quit && GetMessageW(&msg, NULL, 0, 0) > 0) {
    TranslateMessage(&msg);
    DispatchMessageW(&msg);
    if (g_stop && g_stop()) {
      break;
    }
  }
}

int BK_window_pump(void)
{
  MSG msg;
  while (PeekMessageW(&msg, NULL, 0, 0, PM_REMOVE)) {
    if (msg.message == WM_QUIT) {
      break;
    }
    TranslateMessage(&msg);
    DispatchMessageW(&msg);
    if (g_stop && g_stop()) {
      break;
    }
  }
  return g_open;
}

#endif /* _WIN32 */
