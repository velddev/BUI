# Changelog

All notable changes to **blenderui** (BUI). The format follows
[Keep a Changelog](https://keepachangelog.com); versions follow
[Semantic Versioning](https://semver.org).

## [Unreleased]

### Added
- **Tooltips (Rust):** an element's `title` shows in a `<bui-tooltip>` after the pointer rests on it
  (`Window::set_tooltip_delay`, 1 s by default), hidden on leave, on a click or key until the pointer leaves, and
  while the app turns them off (`Window::set_tooltips`). It hangs below the pointer inside the window, flipped
  above without room, and stylesheets style it like any element.

## [0.5.0] - 2026-11-08

### Added
- **Lua bindings** (`BUI_BUILD_LUA_BINDINGS`, off by default; pull requests #19 and #20 by seclorum). A standalone
  `bui-lua` executable (with a REPL) and a `bui.so`/`bui.dll` loadable module for stock Lua 5.4, LuaJIT or LuaRocks.
  Every BUI widget, event, tree view, file browser, menu (submenus included) and node editor is available from
  `require("bui")`, the bindings check the kit version at load time and expose `bui._VERSION`, and seven test
  suites (`bui_lua_*`) run under `ctest`. 26 example scripts in `bindings/lua/examples/`: `hello`, `counter`,
  `todos`, `gallery`, `forms`, `dashboard`, `anim`, `lua_table_editor`, `editor_screenshot`, `box`, `canvas`,
  `curve`, `enum`, `grid`, `histogram`, `listview`, `menubar`, `popover`, `ramp`, `split`, `textarea`, `menus`,
  `color_picker`, `filebrowser_demo`, `image_viewer` and `data_table`. CI builds and tests it on Linux and Windows, and it
  now builds on Windows (a `linenoise` stand-in and a `setenv` shim).
- **npm on Linux and macOS:** the prebuilt addon and `bui-runtime` ship for Linux x64 (glibc 2.35 or later) and
  macOS (arm64 and x64) as well as Windows x64, one package per platform: `@blender-ui/native-<tag>`
  (`bindings/node/platform.js`). `@blender-ui/native` lists them as `optionalDependencies`, so an install gets
  only its own (about 29 MB). The install command does not change. With `--omit=optional` the addon is missing,
  and the load error says so. `release.yml` builds them on `ubuntu-22.04`, `macos-15`, `macos-15-intel` and
  Windows, then publishes the platform packages before `@blender-ui/native`. On macOS, JavaScript still runs
  headless. (concprog, pull request #16)
- **Native Wayland windows (experimental).** `kit/intern/kit_window_wayland.c` is a Wayland host with the same
  `BK_window_*` API, modelled on Blender's `GHOST_SystemWayland`: xdg-shell toplevels with server-side
  decorations when offered, a `wl_seat` with xkbcommon (keys, key repeat), the pointer, the clipboard through
  `wl_data_device`, cursors, the output's buffer scale, and `wl_surface.frame` pacing into `wl_shm` buffers
  (only damaged rectangles are copied). It uses Blender's `wayland_dynload` (`kit/extern/wayland_dynload`), so
  libwayland loads at run time and nothing is linked; libxkbcommon is `dlopen()`ed. A Wayland session gets it;
  `BUI_WINDOW_BACKEND=x11|wayland` picks one. It draws on the CPU, so with the GPU renderer on (`BUI_GPU=1`, or
  `BUI_gpu_set_default(1)` before the first window) the session keeps X11 (XWayland): the X11 host and its GPU start
  (the worker-thread device, the idle switch) are unchanged. Builds without the Wayland headers,
  `wayland-scanner` and `wayland-protocols` (or with `-DBUI_WAYLAND=OFF`) are X11 only, as before. Not done:
  IME, client-side decorations (no title bar on GNOME), pointer warping.
- **Input methods (IME) on Windows**, ported from Blender (`GHOST_ImeWin32`, the text button's IME handling in
  `interface_handlers.cc` and its drawing in `interface_widgets.cc`). Chinese, Japanese and Korean input now
  works in text fields and text areas: the composition is drawn underlined at the caret (a thicker line under
  the clause being converted), the candidate window follows the caret instead of the top left corner of the
  window (issue #14, reported by YuFu-Onship), and the result is typed in like any other text. The IME is on only while a field is being
  edited, so shortcuts keep working with a CJK keyboard; number fields and password fields take no IME, as in
  Blender. A composition that is still going when another field takes over stays in the field it was in.
- **C API changes:** one platform independent IME API for every window host and binding: `BKImeEvent`,
  `BK_ime_event()`, `BKImeState`, `BK_ime_state()` and `BK_ime_composing()` (`BUI_kit.h`). Only the Win32 host
  uses it so far; X11 and Cocoa do nothing and are unchanged. `bench/ime_bench.c` measures what it costs
  (nothing measurable on the paths that did not change: keystroke + repaint 18.5 vs 18.4 us, idle frame 35 ns,
  the host's poll 7 ns).

### Changed
- **Website:** the benchmarks page has a Windows / Linux switch, with the Linux numbers (start time and battery, frame
  times, JavaScript, React and Svelte, and Rust compile times); the docs have a Lua section, a Wayland section on the
  window hosts page and a macOS that is no longer marked experimental; the README's first-app and language-guide
  sections collapse (`<details>`).
- **Linux GPU renderer start** is 50 to 80 percent sooner (the first frame no longer waits for the OpenGL driver):
  for example Rust 169 to 61 ms, JavaScript 207 to 101 ms, Python components 216 to 96 ms, and the same gpui code
  110 to 23 ms. CPU renderer start, frame time and memory are unchanged. The numbers are in the website's
  benchmarks page, the Linux tab.
- **Lua host:** CI builds `bui-lua` and runs its `bui_lua_*` tests on Linux and Windows (a new `lua` job); the option
  is in the CMake options tables (README, `docs/`, the website) and the website has a Lua page; the lowercase
  `find_package(lua)` fallback is config-mode only, which removes CMake's package-name warning.
- **macOS is no longer labelled experimental.** Three people have tried the Cocoa host by hand and it works, so
  the README, the website and the docs say so (the platform tables, the FAQ, the limits, the roadmap, the landing
  page). What is still true there is unchanged: the CPU raster only (no Metal renderer), and JavaScript windows
  are headless. The native Wayland host is now the only experimental window host.
- **macOS docs:** three people have now tried the Cocoa host by hand: two Apple Silicon Macs and an Intel MacBook Pro
  ([piaoboo](https://github.com/piaoboo), issue #17: the PyPI wheel works, the source build passes 25/25 tests). The
  guide and the README, website and docs that said "two Apple Silicon Macs" or "nobody has tried an Intel Mac" now
  say so. [DanNoby](https://github.com/DanNoby)'s 2026-11-07 log (issue #6) confirms the duplicate Objective-C class
  fix: `test_cocoa_classes_come_from_buipy` and the other 20 `buipy_imgui` tests pass and there is no `objc:` warning.
  The Xcode toolchain note now says it depends on Command Line Tools older than the SDK, not on the macOS version.
- **README and website:** the FAQ says macOS has been tried by hand on two Apple Silicon Macs (it said nobody
  had checked), and a new FAQ entry and the roadmap list what people asked for: Vue and Angular (adapters
  over `@blender-ui/native`), and Lua (a community pull request is in review), Ruby and Go through cgo
  (over the C kit). None of them exists yet.
- **Linux GPU start:** the OpenGL context and device are created on a worker thread, as on Direct3D 11. The first frames draw on the CPU. A window switches to the GPU when the device is ready, also when it is idle (Linux and Windows; `BUI_raster_gpu_pending`). (concprog, pull request #10)
- **Benchmarks on Linux:** `bindings/rust/bench/common/measure.rs` measures CPU time (`CLOCK_PROCESS_CPUTIME_ID`) and memory (`/proc/self/status`) on Linux, so `bui-bench`, `bench_frames` and `cargo test` build there; Windows is unchanged. (concprog, pull request #10)

### Fixed
- **Python: an intermittent crash (access violation) when a native window and Tk share a thread.** The C callbacks the kit
  makes into Python (host hooks, window timer and close, reference counting) ran without the GIL when a window message
  was dispatched from inside Tk's `update()`, which releases the GIL around Tcl and whose notifier dispatches every
  window message of the thread. They now take the GIL first. It showed as `buipy_tk_compat` failing about one run in
  ten on Windows CI (and one in forty on v0.4.1); it passed 120 runs in a row after the fix.
- **Node: no window and no message.** When the addon could not get a window system (no `DISPLAY` or a display that
  does not answer, no Wayland socket) it fell back to headless silently, and a window that could not be created
  was dropped without a word: the app ran with nothing on screen and nothing in the log. The addon now prints why
  to stderr (and a window that cannot open is reported closed, so the app can end). `BK_window_supported` on Linux also
  asks Wayland only when the session prefers it, so it no longer says yes through a default Wayland socket that the
  window host would not use.
- **Node and Bun:** a relative `BUI_NODE_ADDON` path starts at the working directory, not at the package folder. The load error now shows the cause. (concprog, pull request #16)
- **Rust windows:** a dirty window with a pending frame request waits for that frame instead of a 0 ms timer, so `requestAnimationFrame` callbacks run once per frame. (concprog, pull request #10)
- **X11:** a frame callback that closes its window no longer renders into the closed window. (concprog, pull request #10)
- **X11:** a window that closes while its GPU device starts releases the device before the X window is destroyed (`BUI_raster_gpu_release`), so Mesa no longer prints `failed to create drawable`. (concprog, pull request #10)
- **CSS colors:** `color-mix()` in `hsl`, `hwb`, `lch` and `oklch` treats the hue of white, grays, black and `transparent` as missing (CSS Color 4 powerless hue), so the mix keeps the other color's hue: `color-mix(in hsl, white, blue)` is a light blue, not pink. (HEOJUNFO, pull request #18)
- **CMake subprojects:** `BUI_kit.h`'s include path is found when BUI is built as a subproject of another CMake
  project (Francis Giraldeau, pull request #12).
- **Debug builds:** `core/intern/list_sort_impl.h` (Blender's `list_sort`) declares its helpers `inline`, which in C99
  gives the linker nothing to call: a debug link failed. They are now `BLI_INLINE`, Blender's `static inline` (concprog, pull request #15).
- **Windows CI:** the Python Tk-compatibility test is no longer flaky (the GIL fix above), and the image-cache test no
  longer requires a timing-dependent number of cancelled decodes.

### Thanks
- [seclorum](https://github.com/seclorum): the Lua bindings and their examples (#19, #20).
- [concprog](https://github.com/concprog): the Linux GPU start, X11 frame pacing and Linux benchmarks (#10), the debug
  link fix (#15), and the npm packages for Linux and macOS with the `BUI_NODE_ADDON` fix (#16).
- [HEOJUNFO](https://github.com/HEOJUNFO): `color-mix()` and powerless hues (#18).
- [giraldeau](https://github.com/giraldeau): building BUI as a CMake subproject (#12).
- [YuFu-Onship](https://github.com/YuFu-Onship): the input method (IME) report that led to IME support (#14).
- [piaoboo](https://github.com/piaoboo): the Intel Mac test report (#17).
- [DanNoby](https://github.com/DanNoby): macOS testing and the Objective-C log (#6), and everyone who reported problems on
  Discord, including the Linux Mint user whose silent Node app led to the error messages.

## [0.4.1] - 2026-11-06

### Added
- **The C API is versioned.** `BUI_kit.h` has `BUI_KIT_VERSION_MAJOR` / `_MINOR` / `_PATCH`,
  `BUI_KIT_VERSION_STRING`, `BUI_KIT_VERSION` (0.4.0 is 400) and `BUI_KIT_VERSION_AT_LEAST`, the library
  reports its own with `BK_version()` and `BK_version_string()` (Python: `buipy.kit_version()`), and
  `BUI_DEPRECATED` is there for the day a function is retired. The number is BUI's own: `check_versions.py`
  refuses a release when the header differs, and `bump_version.py` moves it. The stability policy (no
  promise before 1.0, semantic versioning after, what counts as ABI) is in the header and in the C API
  guide. Changes that break the C API are listed here under "C API changes" from now on.
- **Third-party notices** (`THIRD_PARTY_NOTICES.md`), shipped with the wheels, the npm packages and the crates.

### Changed
- **License, following Blender's own wording:** BUI as a whole is under the GNU GPL, version 3 or
  later (the `LICENSE` file was already the GPLv3 text), and individual files keep their compatible
  `GPL-2.0-or-later` header, as Blender's do. The Python package, the Rust crates, the npm packages,
  the README and the website said `GPL-2.0-or-later` for the whole; they now say `GPL-3.0-or-later`.
- **README:** a lead-in sentence before each screenshot that isn't the output of the snippet above it,
  plainer headings ("Pick your language", "Python", "Rust: HTML and CSS, without a browser"), and a
  corrected "Who wrote it?" (the code is written by agents, a human directs and reviews it).
- **Limitations, said plainly:** a list in the README (Status and roadmap) and a new website page,
  covering accessibility (none yet), text and input methods, the non-native look, GPU backends,
  platforms, retained mode with no visual editor, and that the numbers come from one machine. The
  FAQ answers "is it immediate mode?" and "is it accessible?"; accessibility joins the roadmap.

- **macOS:** the testing guide now lists both testers ([Krish Kalaria](https://github.com/krishkalaria12): issue #3, fixed by
  pull request #4; [DanNoby](https://github.com/DanNoby): 17/17 tests, issue #6), explains the Command Line Tools versus
  SDK mismatch (`ld: tapi error: malformed file`: select the full Xcode toolchain and delete
  `build/`), and records a bug from his log (fixed below: Objective-C printed "Class BUIKitView is
  implemented in both"). The docs and the website say two Apple Silicon Macs.
- **Python:** Dear ImGui (`blenderui.imgui`) is compiled into the `buipy` extension; the separate
  `_buiimgui` extension is gone (the module is still named `_buiimgui`, created by
  `buipy.imgui_module()`). The public API is unchanged.
- **CI:** the Apple Silicon runner is `macos-15` (GitHub retired `macos-14`).
- **Releases:** `tools/bump_version.py --npm patch` moves every published npm package by that kind from its
  own version (native 0.3.0 to 0.3.1, svelte 0.1.0 to 0.1.1; a package that isn't on npm yet keeps its first
  version), and `release.yml` skips a package that is not on npm yet with a clear warning instead of failing
  (a new package's first publish is manual, then it gets a trusted publisher; `tools/release.py` shows it as
  MANUAL). A release is also refused when `THIRD_PARTY_NOTICES.md` is out of date.
- **Svelte** now publishes from CI like the other npm packages (its trusted publisher is set up).

### Fixed
- **Backspace over a Blender value resets it to its default, not to zero** (number fields, sliders,
  vector fields, checkboxes; the right-click "Reset to Default Value" already did, and the docs always
  said Backspace resets to the default). A widget's default is the value it was made with, or an
  explicit `default`, in every language: the kit records the first value a widget gets as its default
  (`NF_DEFAULT_SET`) and the dead "zero" flag of `reset` is gone. Text fields still clear.
- **Website:** the interactive panel on the home page is labelled as a browser sketch, and it now does
  what the text next to it says: dragging down across X, Y and Z edits all three, and Ctrl+C / Ctrl+V
  over a field copy and paste a value.
- **macOS:** Python printed `objc: Class BUIKitView is implemented in both buipy... and
  _buiimgui...` (also `BUIKitNSWindow`, `BUIKitWindowDelegate`, `BUIKitAppDelegate`) when both
  extensions were loaded: each linked the whole kit, Cocoa host included, so the classes were
  registered twice (reported by [DanNoby](https://github.com/DanNoby), issue #6). A process now
  holds one kit: `buipy` and Dear ImGui are one binary on every platform, which also means one
  theme and one set of kit state instead of two.

## [0.4.0] - 2026-11-06

### Added
- **Video, in every language** (`kit/intern/image`: `video.c`, `movie*.c`). One codec
  interface for GIF, WebP and video (Blender's MovieReader shape: open, output size, next,
  seek, close) and one clock (`play.c`: pause, seek, rate, loop, end). The OS decodes,
  loaded at run time so a machine without it still starts: Windows Media Foundation
  (`IMFSourceReader` with a DXVA Direct3D 11 device manager; decoding and scaling on the
  GPU), Linux GStreamer (`decodebin ! videoconvert ! videoscale ! appsink`, `dlopen`ed;
  VA-API through its `va` plugin), macOS AVFoundation (written, **untested**: no Mac).
  Frames are NV12 at the size drawn, two per player reused (no allocation per frame), in
  one GPU texture pair per player updated in place (YCbCr to RGB in the Direct3D 11 and
  OpenGL shaders; the software raster converts what it draws); off screen, clipped or in a
  window that isn't rendering, nothing is decoded. C: `BK_video_*` (new, play, pause,
  seek, rate, loop, poster, controls, state, time, duration, size, listener, texture,
  backends), the `"video"` node with Blender-style controls (play/pause button, scrubber
  with the time; `BK_node_set_video`), canvas items' `video`. Rust: `<video src loop
  poster autoplay>`, `video().video(handle)`, `blenderui::Video` (play, pause, seek,
  set_rate, set_loop, time, duration, state, `on_state`, texture), `Painter::video`.
  Python: `bui.Video` and the `Video` component (`controls`, `on_ended`). React /
  @blender-ui/native / `bui build`: `<video>`. Dear ImGui: `BK_video_texture`. Muted
  autoplay by default; no audio yet.
- **GPU NV12 pixmaps and stream textures**: `BuiPixmap.format` (`BUI_PIXMAP_NV12_BT601`
  / `_BT709`) and `.stream` (frames of one stream update one texture in place), and
  `BUI_pixmap_read`. The GPU parity test covers them (Direct3D 11 and OpenGL).
- **Pay only for what you use: one feature table** (`features.json`): image formats,
  video, the GPU renderers, FreeType, the file browser and Dear ImGui are CMake options
  (`features.cmake`), cargo features of `blenderui-sys` and `blenderui` (same names;
  build.rs maps them and checks the list), and what `bui build` reads. Off, a part isn't
  compiled; its API stays. `bui build` links the smallest prebuilt runtime (`base`,
  `media`, `full`) with the features an app uses: `--features`, then package.json
  `"bui": {"features": [...]}`, then detection in the bundle (`<video>`, media file
  extensions, `gpu:`, file dialogs).
- `bench/video` (`bui_video_bench`, `make_clips.sh`) and `tests/video_test.c`,
  `bindings/rust/blenderui/tests/video.rs` (counting allocator), `features.rs`,
  `bindings/python/tests/test_video.py`.
- **Images, once in C for every binding** (`kit/intern/image`, after Blender's imbuf: one
  image type, `BKImBuf`, and a format table): PNG, GIF (animated), BMP, TGA and QOI with
  Wuffs (memory-safe), JPEG with libjpeg-turbo 3.1.2 (EXIF orientation, CMYK, DCT-scaled
  thumbnails), WebP with libwebp 1.6.0 (lossy, lossless, animated), Radiance HDR (after
  Blender's), PNM (ASCII and binary) and SVG (the kit's nanosvg; `BUI_IMAGE_SVG`, on).
  Decoded at the size drawn into premultiplied pixels, on a worker pool (visible first,
  prefetch, cancellation), into one memory-budgeted cache (`BK_image_set_budget`, 256 MiB);
  an arriving image redraws only its rectangle, and layout uses the header's size.
  C: `BK_imbuf_*`, `BK_image_*`, `BK_node_set_image_source`, `BK_node_set_imbuf`,
  canvas items with `src` / `imbuf` / `fit`. Rust: `<img>` and CSS background images in
  every format, `Image::open` / `decode` / `from_rgba` / `texture`,
  `Painter::image_file(_cover)`, `image_stats`, `set_image_budget`, an image column in the
  data table (`CellRender::Image`), chat-list media (`Record::media`), and a
  `PaintSink::image`. Python: `blenderui.image_decode` / `image_probe` / `image_texture` /
  `texture_free` / `image_stats` / `set_image_budget`, the `Image` component, and tkinter
  `PhotoImage` reading JPEG, WebP, BMP, TGA, QOI, HDR and SVG too (`format="gif -index n"`
  picks a frame). Dear ImGui: textures from images (`image_texture`, `BK_imbuf_texture`).
- Blender's `MEM_get_memory_in_use` / `MEM_get_memory_blocks_in_use` leak counters, and
  its condition variables and thread pool (`BLI_condition_*`, `BLI_threadpool_*`) in core.
- `bench/images`: the decoder comparison and the thumbnail-grid benchmark (`bui_image_bench`).
- **Animated GIF and WebP play**, decoded one frame at a time at the size drawn (the cache
  holds the shown frame, the next one and the decoder; no all-frames limit), each frame for
  its delay, looping as the file says (GIF disposal and blending, WebP's canvas). Frames
  advance on the context's frame timer and repaint only their rectangle; an animation off
  screen, clipped away, in a row that isn't shown or in a window that isn't rendering
  pauses (no decoding, timers or renders). `frame` holds one frame: the `<img frame>`
  attribute (Rust, React), `Painter::image_file_frame`, `Image(frame=)`,
  `BK_node_set_image_frame`, canvas items' `frame`. Reduced motion holds them all:
  `blenderui::set_reduced_motion` (CSS `prefers-reduced-motion: reduce` then matches),
  `bui.set_reduced_motion`, `BK_set_animations(0)`. Stepping animations yourself (Dear
  ImGui): `BK_anim_open` / `BK_anim_next` / `BK_imbuf_texture_update`, Rust
  `blenderui::Animation` and `Image::update_texture`, Python `blenderui.AnimatedTexture`.
  One frame alone: `BK_imbuf_decode_frame`, `image_decode_frame` (tkinter `-index n` uses it).
- **Images over 2047 px on the GPU**: a texture of their own on Direct3D 11 and OpenGL
  (tiled only past the device limit; `BUI_GPU_MAX_TEXTURE` forces it for tests), freed
  when the kit frees the image (`BUI_image_forget`) or after 120 unused frames, within
  256 MiB. The parity test compares them with the CPU raster, whole and tiled.
- `BKImageStats.decode_ms` (time the workers spent decoding) and `MEM_get_allocations_total`.
- `bench/images`: `bui_image_bench <dir> anim <count>`, many animations at once.
- **Svelte** (new npm package `@blender-ui/svelte` 0.1.0, Svelte 5.57): Svelte 5 components in
  native windows, with feature parity with `@blender-ui/react`: `render(Component, options)`,
  `createRoot(window)`, `act(fn)`, Blender's ten widgets as components, BUI's signals (each a
  Svelte store, `$count`), headless tests and screenshots, Node.js (`--import
  @blender-ui/svelte/register`), Bun (`--preload`) and `bui build` / `bui run`. Runes, snippets,
  bindings, keyed `{#each}`, `{#await}`, `{#key}`, context, lifecycle and scoped `<style>`
  blocks work; transitions run their start and end without animating in between. Svelte has
  no custom-renderer API, so its compiled components run on `@blender-ui/native`'s DOM.
- **`@blender-ui/native`, a fuller DOM:** a window-less `document` whose nodes are detached until
  inserted into a window's tree (then created natively with their subtree), `DocumentFragment`,
  `Comment` (an anchor that takes no space), `cloneNode`, `before` / `after` / `append` /
  `prepend` / `replaceWith`, `contains`, `isConnected`, `el.style` with `cssText` /
  `setProperty` / `removeProperty`, `el.classList`, DOM properties that reflect attributes
  (`disabled`, `hidden`, `value`, `checked`, `type` ...) on `HTMLElement`, `dispatchEvent`,
  `event.composedPath()` and `cancelBubble`. Nodes made by `document` are freed natively once
  removed and dropped.
- **A shared layer for framework bindings:** `@blender-ui/native/host` (`open`, `blenderWidgets`,
  `applyProps`: what React's binding did on its own), `@blender-ui/native/globals` (the browser
  globals DOM frameworks read), and `bui build` bundling with the Bun plugin of every
  `@blender-ui/*` package that declares one (`"bui": { "bunPlugin" }`). A binding for another
  framework is a thin adapter over these.
- **Benchmarks:** `bench/cells.js --svelte [--signals]` and `bench/startup.js --svelte`;
  `cells.js` also reports `js_us_per_update` (500 updates back to back, each applied by the
  framework and sent to the UI thread: the JavaScript cost of an update alone) and `heap_mb`.
- **Svelte, faster updates:** compiled components import `@blender-ui/svelte/internal`, which
  serves Svelte's runtime with `set_text`, `set_attribute`, `set_class`, `set_style` and `clsx`
  writing straight into BUI's nodes (each used only while Svelte's own has the shape it was
  written for; a test compares them on the same inputs). A BUI signal passed as a value
  (`{label}`, `style={style}`, `class={cls}`) binds its node: a write updates it without Svelte
  running, as React's signal props do.
- **Svelte, faster start on Node.js and Bun:** with Bun installed, `register` bundles Svelte's
  client runtime once (per Svelte version) into `node_modules/.cache/@blender-ui/svelte` and
  serves it from there; the compile cache is found from the working directory up, and cache
  keys no longer load `node:crypto`.
- **`@blender-ui/native`, slimmer nodes:** attributes are a flat array made by the first one
  (not a `Map` per element), `tagName` is derived from `localName`, and children, signal
  bindings, listeners and form state live on the prototype until a node has them; a text node
  is five fields. About 0.7 MB less JavaScript heap for a 3,000-cell grid, in React and Svelte
  alike.
- **Measured** (3,000 cells, one session, Node 26 / Bun 1.4 / QuickJS): an update costs 17 / 19 /
  36 µs of JavaScript in Svelte with signals (was 87 / 86 / 793; React with signals 23 / 17 / 40),
  65 / 76 / 790 µs with runes (was 85 / 78 / 790); a Svelte app's first frame is 135 ms on Node.js
  and 121 ms on Bun (was 393 / 203; React 125 / 107). Svelte still keeps 10–17 MB more than React
  with signals at 3,000 cells: its own effects and each-block items. Details in
  docs/javascript.md.

### Changed
- `BUI_raster_image` takes a `BuiPixmap` (premultiplied pixels with a key), a source
  rectangle and flags; it filters bilinearly (exact copies at 1:1) on the CPU and on both
  GPU renderers, and the GPU atlas finds images by key instead of hashing their pixels
  every frame.
- `blenderui::Image` holds the kit's decoded image instead of a `Vec<u8>` (`width` and
  `height` stay public; `from_png` is deprecated in favour of `open`; `image_rgba` takes any
  `AsRef<[u8]>`). The automation `App::draw` waits for image decodes it started.
- tkinter `PhotoImage` decodes through the kit (the Python PNG/PPM decoders and the
  round trip through the real Tk for GIF are gone).
- Images whose source rectangle is outside the canvas clip are neither decoded nor played.
- **Builds**: every C file compiles with function and data sections (`-ffunction-sections
  -fdata-sections`, MSVC `/Gy /Gw`) so linkers drop unused code, and MSVC Release builds
  get `/O2` explicitly: under cmake-rs (`blenderui-sys`) the kit was compiled unoptimized.
  The Node addon and the QuickJS runtime build with the new `dist` profile (fat LTO, one
  codegen unit): `bui-runtime.exe` went from 8,693,760 to 7,097,856 bytes, and an app
  made with `bui build` is about 7 MB (the docs said about 5 MB; that figure predated the
  image decoders). The Node test now checks that `bui build` adds only the bundle and
  holds the runtime to its measured size plus 5 %.
- **`@blender-ui/native`:** siblings are a linked list (insert, remove and `nextSibling` are
  O(1); `childNodes` returns a new array), and listener maps are made on the first listener.
  Re-rendering 3,000 cells per frame from React costs a little less CPU.
- **`@blender-ui/native`:** reading `el.style` returns the inline style object (its `cssText`
  is the old string); setting it still takes a string, an object or a signal.
  `addEventListener` accepts any event type (those BUI doesn't report only fire through
  `dispatchEvent`) instead of throwing.
- **`@blender-ui/react`** uses `@blender-ui/native/host` for its props, events, widgets and
  window options; its API and behaviour are unchanged.
- **`bui build`** bundles with `Bun.build` (and the framework plugins it finds) instead of the
  `bun build` command line.
- **`bench/startup.js`** counts launches that fail at exit (after their first frame, a
  sporadic crash on Bun at quit) instead of aborting.

### Removed
- The `png` crate as a dependency of `blenderui` (the website-screenshot examples still use
  it to write PNGs, as a dev-dependency).

## [0.3.0] - 2026-11-06

### Added
- **shadcn Data Table and virtual list for Rust** (`blenderui::shadcn`): the API of
  shadcn/ui's Data Table (TanStack `ColumnDef`, sorting, column and global filters,
  pagination, row selection, column visibility, controlled and uncontrolled state) as
  `<DataTable columns={..} data={..} />` / `DataTable::new`, with `DataTableColumnHeader`,
  `DataTablePagination`, `DataTableViewOptions` and `DataTableToolbar`, on the painted table;
  cells are typed (text with a number format, checkbox, badge, progress, row actions), so a
  million rows stay O(visible). `VirtualList` and `ChatList` with TanStack Virtual's names
  (`estimate_size`, `overscan`, `get_item_key`, `scroll_to_index`, `scroll_to_offset`,
  `measure_element`, `on_range_change`) over the variable-height list. The logic is in
  `blenderui_table::tanstack` and `blenderui_vlist::rows`.

### Changed
- **macOS:** the docs and the website now say the Cocoa host has been tried by hand on an Apple
  Silicon Mac, by [Krish Kalaria](https://github.com/krishkalaria12) (issue #3, pull request #4).
- **Website:** a video of the variable-height list (downloaded only when played), shadcn pages,
  and a crates.io link in the footer.

## [0.2.0] - 2026-11-05

### Added (latest: summary)
The newest work. Details for some items follow in the sections below.

- **Data table and variable-height list for Rust** (new crates `blenderui-table`,
  `blenderui-vlist` and `blenderui-paint`, used by `blenderui`): a virtualized data table
  (a million rows by twenty columns, painted from typed columns, sorting, selection,
  resizable columns) and a chat-grade list with items of different heights (a two-level height
  index, scrolling anchored to an item, opens at the newest message, a file-backed source for
  ten million messages), both on Blender's own virtualization ideas. See the benchmarks in the
  docs.
- **Release pipeline:** one tag publishes the Rust crates (crates.io), the npm packages (trusted
  publishing) and the Python wheels (PyPI, trusted publishing), with a version check first.
- **Text:** glyphs sit on the same baseline in every size, so labels are centered in pills and
  buttons.
- **Dear ImGui on BUI** (`bindings/imgui/`): the real Dear ImGui 1.93.1 WIP
  (vendored, MIT) on a BUI platform + renderer backend (`imgui_impl_bui`),
  hosted in a kit `custom` node: BUI's windows and input, the software raster,
  Direct3D 11 or OpenGL 3.3, damage-driven frames (idle costs nothing),
  Blender's theme colors and metrics and the Inter font
  (`ImGui_ImplBUI_StyleBlender`).
  - **Bindings generated from `imgui.h`:** C (`cimgui.h`, all 399 functions,
    CMake target `bui_cimgui`), Python (`import blenderui.imgui`, the
    `_buiimgui` extension, 366 of 399 functions) and Rust (the
    `blenderui-imgui` crate: FFI for 385 of 399, a safe layer for 366, and a
    runner). C++ uses `imgui.h` unchanged (`bui::imgui`).
  - **Blender widgets:** with `-DBUI_IMGUI_BLENDER_WIDGETS=ON` (Rust feature
    `blender-widgets`), an edited `imgui_widgets.cpp` makes ImGui's widgets
    Blender's: `DragFloat` is the drag-to-scrub number field, `SliderFloat`
    the number slider, checkboxes drag-toggle, menus and combos get Blender's
    menus. Window title bars, tooltips, plain popups, tables and the color
    picker keep ImGui's drawing.
  - Examples: `examples/imgui_demo.cpp`, `imgui_demo_c.c`,
    `imgui_blender_compare.cpp`, `bindings/rust/blenderui-imgui/examples/demo.rs`.
- **Linux GPU renderer on OpenGL 3.3 core** (`gpu/intern/gpu_gl.c`): the same
  instances, atlases, Slug text and uber-shader as Direct3D 11, translated to
  GLSL; GLX on the X11 window's connection, EGL (then a GLX pbuffer) offscreen;
  `libGL`, `libEGL` and `libX11` are loaded at run time, so no GL headers are
  needed and BUI falls back to the CPU raster where OpenGL is missing. The GPU
  parity test passes on Mesa's software OpenGL and on an NVIDIA GPU.
  `BUI_GPU_EGL` picks the EGL platform.
- **macOS: a native Cocoa window host** (`kit/intern/kit_window_cocoa.m`),
  ported from Blender's GHOST Cocoa backend, with Retina scaling. Experimental:
  checked by CI and tried by hand on an Apple Silicon Mac by [Krish Kalaria](https://github.com/krishkalaria12) (issue #3,
  pull request #4), which found the fixes listed below (see `docs/macos-testing.md`). Python can
  fall back to the Tk window with `BUI_HOST=tk`; JavaScript stays headless on
  macOS.
- **Light theme:** Blender's Dark (default) and Light presets, generated by
  `tools/gen_theme.py` from Blender's `Blender_Dark.xml` / `Blender_Light.xml`
  over `userdef_default_theme.c` (`kit/intern/kit_theme_presets.c`). Switch at
  run time with `BK_set_theme("light")` (C), `bui.set_theme("light")`
  (Python, also forwarded to the ImGui extension), `blenderui::set_theme` /
  `Application::theme` (Rust), `new App({ theme })` / `app.setTheme` and
  React's `render(..., { theme })` (JavaScript), or with `BUI_THEME=light`
  without code. Widgets, panels, the region background, the `styled` HTML
  sheet and ImGui's Blender style all follow. `BK_theme_name` /
  `BK_theme_color` read the active theme.
- **Three looks in Rust and JavaScript:** plain HTML now looks like a browser's
  (black text on white, Chrome-style form controls, in Inter); `styled` gives
  any subtree Blender's look and Blender's widgets; and ten **Blender
  components** (`Button`, `Checkbox`, `Radio`, `NumberField`, `NumberSlider`,
  `TextField`, `TextArea`, `ColorSwatch`, `Dropdown`, `ProgressBar`) are
  Blender's widgets anywhere (`<NumberSlider>` in `rsx!` and JSX,
  `NumberSlider()` in builders, `win.createElement("NumberSlider")`).
- **CSS effects, animations and transitions** (Rust and JavaScript):
  `@keyframes` animations and transitions (every longhand, easing functions,
  interruptible transitions), `filter` and `backdrop-filter`, blend modes,
  `clip-path`, masks, 2D transforms and `border-image`. Frames render only
  while something moves; headless windows step a virtual clock.
- **Blender's string search** (`kit/intern/blender/interface_string_search.cc`,
  `core/intern/string_search.cc`): menu search ranks by word initials and
  forgives typos ("ad cu" finds "Add Cube"), with Blender's recent searches
  ranked first for short queries (`BK_recent_searches_*`).
- **Blender's expression evaluator** (`core/intern/expr_pylike_eval.cc`): type
  math into number fields (`2*pi`, `min(4, 3)`, `x if c else y`);
  `BK_eval_number` from C.
- **Text-edit undo** (`kit/intern/blender/interface_undo.cc`): Ctrl+Z /
  Ctrl+Shift+Z inside a text field while you type.
- **Blender's float display precision** (`kit/intern/blender/interface_utils.cc`,
  `UI_calc_float_precision`): small values show the decimals they need.
- **Panel animation:** panels collapse and open with Blender's animation.
- **Documentation website:** <https://bui.preetham.top>, with tutorials per
  language, concepts, guides, references and benchmarks, and new screenshots
  in dark and light (`website/public/images/shots/`, regenerated by
  `python tools/gen_doc_images.py --site`).
- **`blenderui` on crates.io** (`cargo add blenderui`); `blenderui-imgui` is
  used from git for now.

### Changed (closer to Blender 5.x)
- **Text is rendered like Blender's BLF:** BUI now vendors FreeType 2.13.3 (`gpu/extern/freetype`, CMake `BUI_TEXT_FREETYPE`, on by default) and renders as `blenfont/intern/blf_glyph.cc` does:
  - light hinting;
  - Blender's gamma-1.43 coverage table;
  - the variable Inter at the text's optical size, clamped to the axis as Blender does, so UI text gets the 14 pt design instead of the static 18 pt cut;
  - bold as the font's weight-700 instance.

  Kerning stays off, matching Blender, whose FreeType is built without `GPOS` kerning.
- **Text shadows:** widget text gets Blender's 1 px drop shadow, and panel titles its soft 3×3 one (`uiFontStyle.shadow`).
- **Icons are Blender's own:** the 781 SVGs from `release/datafiles/icons_svg` (`tools/gen_svg_icons.py` → `kit/intern/kit_icons_svg.c`) are rasterized with Blender's nanosvg, the way `blf_glyph_cache_add_svg` does. Mono icons use the text color or their theme color (object orange, modifier blue …), and color icons keep their colors.
- **The theme follows Blender 5.x** (`userdef_default_theme.c`): every widget color set, the navigation-bar background, panel outline and menu shadow.
- **Panels** are ported from `interface_panel.cc`:
  - chevron icons and the grip icon;
  - 1.25-unit headers for sub-panels too;
  - sub-panels indented 0.7 units and inset 0.1 units, with no separator line;
  - no hover tint.
- **Properties navigation bar:** 1.4-unit tabs in aligned groups, using `wcol_tab` colors. `SideTabs` takes `None` between pages to start a group.
- **Widgets:**
  - unit values drop trailing zeros ("0 m", "90°");
  - unembossed buttons draw no hover backdrop;
  - toggles show icons at alpha 0.75 when off;
  - checked checkbox labels use `text_sel`;
  - menus are at least 9 units wide;
  - tree views indent 0.7 units, with disclosure-triangle icons;
  - scrollbars are `V2D_SCROLL_WIDTH` wide.
- **Number fields and sliders drag as in Blender** (`interface_handlers.cc`):
  - **Number fields** (`numedit_but_NUM`): a float moves one click step per pixel (Shift: a tenth of that). An integer moves 1, ½ or 1/16 per pixel, depending on its range.
  - **Sliders** (`numedit_but_SLI`) follow the pointer across their width from where the drag began (Shift: 1/20 the speed). When the drag ends, the pointer is put back at the value's position.
  - **Ctrl snapping uses Blender's rules** (Ctrl+Shift is finer):
    - unbounded floats snap to whole units, degrees to 10°;
    - sliders snap to 0.1, 1 or 10 depending on their range;
    - integers snap to multiples of 10.
  - **Limits:** past a limit the drag's starting point moves with the pointer, so dragging back responds at once.
  - **Threshold:** the movement inside the drag threshold is ignored, so values don't jump when a drag starts.
  - **Clicks:** Ctrl+click types a value. A drag that changed nothing counts as a click.
  - **Escape** cancels a drag and restores the values.
- **Dropdown chevrons** are drawn as thick as the shader's menu triangle bars.
- **Checkboxes:** the checkmark is the one Blender's widget shader draws (two 0.14-unit bars, `ROUNDBOX_TRIA_CHECK`), so it fits inside the box. Label text starts right after the box.
- **Labels without icons** are inset one pixel instead of a text margin (`BUT_NO_TEXT_PADDING`), so right-aligned property labels line up as in Blender.
- **`Split`** is its own `split` layout, with its columns `columnspace` (0.4 units) apart, as `Layout::split` spaces them.
- **Panel bodies** are laid out as `ED_region_panels_layout_ex` lays them out:
  - the panel's own content is inset by `panelspace` on every side;
  - sub-panels are full width, with no gaps between them;
  - header widgets start at `panel_label_offset`, and the title follows them.

### Added (components)
- **`NodeEditor` / `Node`** (`kit/intern/kit_nodes.c`): Blender's node editor canvas, ported from `space_node` (`node_draw.cc`, `drawnode.cc`, `node_select.cc`, `node_relationships.cc`) and `view2d_ops.cc`.
  - **Drawing:**
    - nodes with headers in the theme's node-class colors and outlines for selected and active nodes;
    - sockets colored by type;
    - number fields on unlinked inputs;
    - bezier links (`noodle_curving` 4), colored by socket type;
    - Blender's dot grid.
  - **Editing:** select (Shift toggles), move (Ctrl snaps), box select, drag sockets to link, drag a linked input to pick its link up, Ctrl+RMB to cut links, drag input fields to change values.
  - **View and keys:** MMB pans and Ctrl+MMB zooms; the wheel zooms about the cursor (Ctrl/Shift+wheel pans). X / Delete deletes, A / Alt+A selects or deselects all, H collapses, Home frames all.
- **`Area` / `AreaHeader`**: an editor area. A 1.3-unit header (`HEADERY`) with the editor-type menu, items laid out from `UI_HEADER_OFFSET`, and `Spacer()`s pushing the rest right; the editor fills the space below.
- **Middle and right mouse buttons for editors:** classes flagged `CLS_BUTTONS` get middle and right presses as press, drag and release, with `BK_BUTTON2` / `BK_BUTTON3` in the event's modifiers.
- **`Sidebar`**: an editor's N-panel. Panels with a `category` show under vertical tabs with rotated labels (`panel_category_tabs_draw_all`); Ctrl+Wheel cycles them.
- **`fill=True`**: in a column, the component takes the spare height, as a region fills its area.
- **`tools/blender_compare.py`**: renders BUI's version of a piece of Blender's UI next to a capture from a real Blender. **`tools/png_pixels.py`** reads pixels from PNGs.

### Added (file browser)
- **Blender's File Browser** (`kit/intern/kit_filebrowser.c` and the `kit_file*` / `kit_fsmenu.c` files), ported from `editors/space_file`. The Blender sources are in `ui/space_file` for reference.
  - **Node:** `BK_node_new("filebrowser")`, with properties for the folder, file name, mode (open, save, folder), multiple selection, filters, sorting and search. It is the whole browser:
    - bookmark panels: Volumes, System, Bookmarks (saved to `bookmarks.txt`) and Recent;
    - the folder path bar;
    - the file list: Name, Date Modified and Size columns; click a header to sort;
    - the execute bar.
  - **Keys:** Blender's file browser keymap. Return, Backspace, Alt+arrows for history, Ctrl+H for hidden files, Ctrl+B to bookmark, I for a new folder, F2 to rename, R to refresh, A / Alt+A to (de)select all.
  - **Dialog:** `BK_file_dialog` shows the browser in its own window and returns the chosen paths.
  - **Speed:** dates, sizes and icons are computed when a row is first drawn, and hover redraws only the changed rows.
  - **Platforms:** Linux, BSD and macOS (`kit_filebrowser_posix.c`), and Windows (`kit_filebrowser_win32.c`).
- **Python:** `bui.filebrowser.ask("open" | "save" | "directory", ...)`. `tkinter.filedialog` (through `bui.tksyntax`) and `QFileDialog` (through `bui.qtsyntax`) now show this browser. Set `BUI_FILEDIALOG=native` to get the operating system's dialogs instead.
- **Theme:** the `space_file` colors (`file_back`, `file_text`, `file_header`, `file_row_alternate`).

### Added (platforms)
- **Linux: native windows on X11** (`kit/intern/kit_window_x11.c`, the same `BK_window_*` API as Win32). Wayland sessions run it through XWayland.
  - **Input:** button and key mapping after Blender's `GHOST_SystemX11`; key repeat filtered.
  - **Window:** `Xft.dpi` scale, `CLIPBOARD` copy and paste, cursors (hidden while scrubbing), pointer warp, size hints.
  - **Loop:** timers, frame pacing and a pipe for cross-thread wakes, in one `select()` loop.
  - **Drawing:** damaged rectangles are put with `XPutImage`.
- The window host is now split by platform: `kit_window_win32.c`, `kit_window_x11.c` and `kit_window_null.c` (others).
- **The C libraries and the Python module build on Linux:** missing POSIX headers fixed, `libm` linked, position-independent code. All C and Python suites pass on Linux (Kali under WSLg), including a new native-window smoke test (`tests/window_smoke.c`).

### Added (JavaScript: `@blender-ui/native` 0.2.0, `@blender-ui/react` 0.1.0)
- **`@blender-ui/native`** (`bindings/node`, napi-rs crate `blenderui-node`): BUI from JavaScript in native windows, GPUIX style, on **Node.js and Bun**.
  - A DOM-like API: `createElement`, `setAttribute`, `appendChild`, `insertBefore`, `addEventListener` (events bubble; `stopPropagation` works), `requestAnimationFrame`, `getBoundingClientRect`, `screenshot`.
  - BUI runs on its own UI thread, so JavaScript is never blocked and an idle UI uses no CPU.
  - Each tick sends only the changes: one `Int32Array` of ops and a string table per window. Events return through a thread-safe function.
  - Animation frames are vsync-paced with one rebuild per frame.
  - Headless mode for tests.
  - **Signals** (the Rust hooks' `Signal` / `Memo` / `Effect`): `signal`, `memo`, `effect`, `batch`, `untrack`, `startTransition`, `ref`. A signal bound to an attribute, `style` or text updates that node alone. Also usable without the addon (`@blender-ui/native/signals`).
    - Push-pull and glitch-free.
    - Unobserved memos hold no subscriptions.
    - Transitions run their effects after urgent ones, in 5 ms slices.
- **`@blender-ui/react`** (`bindings/react`): a React 19.3 renderer (react-reconciler 0.34).
  - **Hooks by React's names, backed by signals:** `useState`, `useReducer`, `useRef`, `useMemo`, `useCallback`, `useEffectEvent`, `useEffect` / `useLayoutEffect` / `useInsertionEffect`, `useSyncExternalStore`, `useTransition`, `useDeferredValue`, `useActionState` and `useOptimistic` take React's arguments and run at React's moments.
    - Passed as a prop or child, their state updates that node without a re-render.
    - Read while rendering, it re-renders the component, as with React.
    - At 3,000 cells, React with signals costs about what the plain API does: about a quarter less CPU per frame and memory than re-rendering (production builds).
  - **Conformance suite:** runs each hook beside React's original (development and production builds, StrictMode). It fails when React adds a hook that isn't covered, which caught React 19.2's `useEffectEvent`.
  - **JSX runtime** (`"jsxImportSource": "@blender-ui/react"`): render tracking for components, and types that let host props take signals.
  - **Host components:** HTML host components, `className` / `style` objects / events, and one native batch per commit. `<Suspense>`, `<Activity>`, actions and `<ViewTransition>` (without animation) work.
- **Standalone executables** (`bui build`, in `@blender-ui/native`):
  - **Default:** `bui-runtime` (new crate `blenderui-qjs`), BUI with an embedded QuickJS engine (4.9 MB). It runs both packages unchanged, with JavaScript on the UI thread. An app is the runtime with its bundle appended: about 5 MB, first frame in about 70 ms at 19 MB of RAM, and no console window on Windows.
  - **`--bun`:** compiles a Bun executable with the addon embedded.
  - **`bui run`:** runs an app on the QuickJS runtime without building.
- **`blenderui-jsview`** (new crate): the op decoder, mirrored tree, event listeners and window operations, shared by the napi addon and the QuickJS runtime.
- **Style objects holding signals** (`style={{ color: colorSignal }}`) are bound: only that node's style updates.
- **Startup benchmark** (`bench/startup.js`): first frame, launch to exit, RAM and size, for Node, Bun, the Bun executable and the QuickJS executable.
- **Benchmarks** for JavaScript (`bindings/node/bench/cells.js`, Node or Bun) measure CPU with the process cycle counter, as the Rust benchmark does.
- **Rust:**
  - `UiSender` / `App::ui_sender()`: run closures on the UI thread from any thread.
  - `Application::keep_alive`, `App::run_pending`, `App::on_window_closed`, `App::close_window`, `Div::key`, `blenderui::native_windows()`.
  - `css::Value::Shared`, and `StyleBlock` for `&[Decl]`, for pre-parsed styles.
- **C:** `BK_window_wake_init` / `BK_window_wake` (a cross-thread wake through a message-only window) and `BK_window_set_keep_alive`.

### Fixed
- **GPU text:** `BUI_gpu_slug()` read the environment on every text run, which pushed the GPU raster from 1.6 to 9.4 ms per frame at 3,000 cells. It now reads it once.
- **`WindowHandle::update`:** `cx.notify()` inside the closure didn't re-render that window.
- **Python, hidden Tk root:** BUI's hidden Tk root (started before the first window on macOS, and by the Tk host) became `tkinter._default_root`, so a real tkinter program's `StringVar()` bound to the hidden interpreter and its widgets never saw the value.
- **tkinter on HiDPI:** a window fitting its content (and `simpledialog` windows) was scaled twice, so it came out twice too large on Retina displays and other scaled screens.
- **tkinter on macOS:** `Menu` defaults to `tearoff=0`, like Tk on Aqua.
- **`geometry()` round trip:** `root.geometry(root.geometry())` doubled the window on Retina displays and other scaled screens, and on macOS moved it down by the title bar. The getter now reports logical pixels, and on macOS `wm("pos")` sets the content's position, the one `wm_get("pos")` reports.
- **macOS, Python:** Quit (Cmd+Q, the Dock) did nothing while Tk owned `NSApp`, the default when tkinter is installed. It now asks every BUI window to close, as without Tk.

### Changed
- **Text looks like Blender's:** Inter, Blender's UI font, is embedded and is the default face on every platform. The portable stb_truetype engine is now the default on Windows too. `-DBUI_TEXT_GDI=ON` (Rust: the `gdi-text` feature) restores GDI and system fonts. `BUI_TEXT_STB` and `portable-text` are now no-ops.

### Added
- **Slug as a flag:** GPU text is Slug or the per-glyph atlas, chosen at build time (CMake `BUI_SLUG`, Rust feature `slug`, both on by default) or at run time (`BUI_gpu_set_slug`, `bui.set_slug`, `Application::slug` / `blenderui::set_slug`, `BUI_SLUG=1/0`).
- **Optional props and keys** (Rust): `Option<T>` props and `children` may be left out in `rsx!`, and a missing required prop is a compile error that names it. `key={..}` ties a component's hook state to the key instead of its position. Struct components opt in with `#[derive(Props)]`.
- **CSS** (Rust):
  - **Applied at-rules:** `@container` size queries (`container-type`, `container-name`, `container`), `@font-face` (loads local font files) and `@property` initial values.
  - **Images:** `background-image: url()` with `background-size`, `-position` (including `-x` / `-y`) and `-repeat`, plus the `background` shorthand; `object-fit` and `object-position` on `<img>`.
  - **Font families:** `font-family` now selects the face (registered, installed, or generic `monospace` / `serif`).
  - **Longhands and aliases:** logical border longhands, `text-wrap-mode`, `white-space-collapse`, `scale` and `all`.
  - **Parsed (no effect yet):** about 100 more properties, including scroll margins and padding, overscroll, OpenType font features, masks and `contain-intrinsic-*`.
  - **Coverage:** 207 of 745 W3C properties now render (was 176), 177 more parse (was 75), and CSS 2.1 is at 82 of 90.
- **Slug GPU text:** with the GPU renderer, glyphs are drawn from their outlines with Eric Lengyel's Slug algorithm: one instance per glyph, exact coverage in the pixel shader, cached curve and band textures, no CPU rasterization. `BUI_SLUG=0` falls back to per-glyph bitmaps in the GPU atlas (Blender's approach).
- **Components and hooks** (Rust, compatible with zopra, React and Leptos):
  - `#[component]` turns a function into a component, with lowercase or uppercase names and `&str` props.
  - `use_state` returns `count()` / `set_count(..)` closures that need no `cx`.
  - Signals: `use_signal`, `Signal`/`RwSignal`, and `signal`/`create_signal` with read and write halves.
  - Auto-tracked `Memo` and `Effect`; `use_effect` / `use_effect!` and `use_memo` with explicit dependencies.
  - Context: `provide_context` / `use_context`.
  - State is tied to the call site, like React, and freed when its component stops rendering. Setters re-render their window. No `Rc` or `RefCell`.
- **`rsx!` / `view!` markup** (Rust): JSX syntax lowered to builder calls at compile time, in about 470 lines with no dependencies.
  - **Syntax:** any tag or custom element; text, `{expr}` and `{...spread}` children; `for`, `if` and `match` in markup; fragments; and struct components with children.
  - **Uniform lowering:** `{expr}` attributes call the builder method of the same name, so the whole API is reachable without per-attribute code.
  - **Events:** listener closures may omit the `(event, window, cx)` parameters.
  - **Attributify:** bare Tailwind utilities are attributes too: `<div p-4 bg-[#4772b3] hover:bg-sky-500/50>`. They compile to classes, while HTML boolean attributes stay attributes.
  - **Did-you-mean errors:** a misspelled utility, variant, root or value is a compile error with suggestions, in `rsx!` and `tw!` (`blenderui_tailwind::suggest`).
  - **Variant attributes:** every Tailwind variant works as an attribute prefix: `hover:class="..."`, `md:dark:hover:style="..."`, `group-hover:color="red"`, `active:bg={red()}`.
- **Modern CSS in the Rust crate**:
  - **Layout:** [taffy](https://github.com/DioxusLabs/taffy) 0.6 → 0.15 behind a layout tree of our own. taffy's built-in tree resolves `calc()` to 0; ours evaluates it. That brings mixed-unit `calc()` everywhere (`calc(100% - 2rem)`, grid tracks, `fit-content(calc())`), floats and `clear`, `direction: rtl`, `grid-template-areas` with named lines and areas, the sizing keywords (`min-content`, `max-content`, `fit-content`, `stretch`), `display: flow-root`, real `position: static` / `fixed` with containing blocks, `contain`, `flex-wrap: balance`, and `safe` / `unsafe` alignment.
  - **Math:** `calc()`, `min()`, `max()`, `clamp()`, `round()` (all four strategies), `mod()`, `rem()`, `abs()`, `sign()`, trigonometry, `pow()`, `sqrt()`, `hypot()`, `log()`, `exp()`, the constants `e`/`pi`/`infinity`, and every length unit (`vmin`/`vmax`, `dvh`/`svh`/`lvh`, `ch`, `ex`, `lh`, `rlh`, `cap`, `cq*` ...) plus angles. Expressions are interned once.
  - **Custom properties:** `var()` with fallbacks, nesting inside functions, inheritance and cycle detection. `env()` is supported too. Variable maps live in a shared arena, not in reference-counted values.
  - **Colors (CSS Color 4/5):** modern `rgb()`/`hsl()` syntax with `none`; `hwb()`, `lab()`, `lch()`, `oklab()` and `oklch()`; `color()` in every predefined space; `color-mix()` with every interpolation space and hue method; relative colors (`oklch(from var(--c) calc(l + .1) c h)`); `light-dark()`; system colors; CSS gamut mapping into sRGB.
  - **Selectors Level 4:** full selector lists in `:is()`, `:where()` and `:not()`; relative `:has()` (evaluated on the element's declarative subtree before it is built); `:nth-child(An+B of S)`; the `*-of-type` family; `:lang()`; `:dir()`; attribute `i`/`s` flags; CSS escapes; nesting with `&`; and pseudo-elements, with `::before` / `::after` generating boxes (the `content` property with strings, `attr()` and quotes).
  - **At-rules:**
    - Media Queries 4: ranges, `not`/`or`, every desktop feature.
    - `@supports`, including `selector()`.
    - `@layer`: cascade layers, both the statement and the block form.
    - `@scope`.
    - Parsed but not applied yet: `@container`, `@font-face`, `@keyframes` and `@property`.
- **Tailwind CSS v4** (`blenderui-tailwind` crate): class names compile to CSS on first use, like Tailwind's JIT, using the v4 default theme.
  - **Variants:** state and structural ones, pseudo-elements, `group-*` and `peer-*` (including named), `has-*`, `not-*`, `in-*`, `data-*`, `aria-*`, `nth-*`, `*` and `**`, the breakpoints with their `max-*` and `min-[…]` forms, container queries, `dark`, `motion-*`, `contrast-*`, `print`, orientation, `supports-*`, `rtl`/`ltr`, `open`, `inert`, and arbitrary `[&…]`.
  - **Values:** arbitrary values and properties, `(--var)`, negatives, `/opacity` and `!important`.
  - **Order:** utilities sort the way Tailwind sorts them.
  - **Usage:** `.class("md:hover:bg-sky-500/50")` just works; `tw!("…")` checks classes at compile time; `blenderui::set_tailwind(false)` opts out.
- **GPU renderer** (Direct3D 11, opt-in): Rust `Application::new().gpu(true)`
  or `WindowOptions { gpu: Some(true), .. }`, tkinter `Tk(gpu=True)` or
  `bui.set_gpu()`, C `BUI_gpu_set_default` / `BK_window_set_gpu`, or
  `BUI_GPU=1` for any app. Modeled on Blender's instanced widget-base
  batches (`interface_widgets.cc`): every primitive (rects, Blender widget
  bases, circles, lines, shadows, gradients, HSV wheels, images, text runs)
  is one 128-byte instance of a single uber-shader, so a frame of thousands
  of widgets is a few `DrawInstanced` calls. Text runs and images live in
  atlases. Frames render into a persistent target, so the kit's damage
  rectangles still bound the work, and they present through a flip-model
  swap chain with dirty rects. The device is created on a worker thread
  (the first frame never waits for the driver), and a lost device falls
  back to the software raster. The pixel shader evaluates the software
  raster's coverage formulas, so both renderers draw the same pixels
  (`gpu_parity` test: 10 of 172,800 pixels differ by more than 3/255, all on
  hard triangle edges). Shaders live in `gpu/shaders/bui_ui.hlsl`, with
  precompiled bytecode checked in, so builds need no shader compiler.
- **Portable text engine**: stb_truetype behind the same glyph-run cache as
  GDI, so text renders on Linux, macOS and wasm. It is also available on
  Windows with `-DBUI_TEXT_STB=ON` or the `portable-text` Rust feature.
  Fonts come from the platform's font folders (Segoe UI, DejaVu, Liberation,
  Noto, Helvetica and others, with generic `sans`/`serif`/`mono`), from
  `BUI_FONT` / `BUI_FONT_DIR`, or from `BUI_text_add_font` (Rust
  `blenderui::add_font`), which wasm needs. Missing bold and italic faces
  are synthesized. A per-glyph bitmap cache keeps changing strings cheap.
  Layout matches GDI, and the 3000-cell benchmark costs the same CPU.
- `BUI_text_run_mask`, `BUI_raster_gpu_attach`/`_poll`/`is_gpu`.
- **Rust crates** (`bindings/rust/`): `blenderui-sys` (FFI over `BUI_kit.h`,
  builds the kit with CMake) and `blenderui`: gpui syntax (`Application`,
  `Render`, `Context`, `Entity`, `cx.listener`, ~1,300 `Styled` methods), all
  113 HTML elements, CSS (stylesheets, selectors, cascade, 173 rendered
  properties), taffy layout (block, inline line boxes, flexbox, grid, tables),
  form controls as Blender widgets, headless rendering for tests. See
  [docs/rust.md](docs/rust.md).
- Kit canvas: per-item clip rectangles and text font flags, and
  `BK_canvas_add_item` (append without an id lookup) for immediate-mode hosts.
- **Parallel canvas rasterisation**: large display lists draw in horizontal
  bands on the Win32 thread pool, with pixels identical to serial drawing
  (`BUI_RASTER_THREADS` sets the thread count). New raster APIs back it:
  `BUI_raster_view`, `BUI_text_prepare`, `BUI_raster_text_run` and
  `BUI_text_generation`.
- `BK_window_request_frame` and the `on_frame` window callback: animation
  frames that rebuild right before the render. They are vsync-paced, and a
  frame that is already late doesn't wait for another vblank.
- The `opaque` node flag: the kit skips its region clear under an opaque root.
- The Rust crate gains `Window::request_animation_frame`, `set_vsync` and
  `presented_frames`, and `BUI_PROFILE=1` per-phase timings.
- `bindings/rust/bench`: blenderui vs gpui vs gpui-fast benchmark (FPS, CPU, GPU, RAM).

- Rust: `css!` (crate `blenderui-macros`) checks CSS blocks at compile time;
  `.style()` takes its output or run-time text.

### Changed
- Rust: no `Rc` / `RefCell`. Frames are index-linked arenas reused across
  frames, styles live in a per-window store, listeners are owned `Box`es.
  `Entity<T>` is a `Copy` handle into an app-owned slot map: `read`
  returns `&T`, `WeakEntity::upgrade` became `upgrade_in(cx)`, and
  `cx.release(entity)` drops a value. Elements are no longer `Clone`.
- Rust: style methods resolve property names at compile time (a `const`
  property table) and build typed values without formatting or parsing.

### Performance
- Kit canvas (tkinter, Qt, Rust): every item type rasterizes in parallel
  (lines, arcs and outlines included); item edits redraw only the item's old
  and new boxes; items are found by id through a hash index; replacing an
  item reuses its buffers. tkinter: 2000 items, one moving: 9.4 → 0.08 ms
  render per frame; all moving: 11.1 → 3.6 ms.
- tkinter facade: colors parsed once; lines delete only arrowheads they made.
- Rust: frame-to-frame reuse: selector matches memoized by element
  fingerprint, a persistent taffy tree reconciled by key (unchanged subtrees
  keep their cached layout). 3000 elements of which one changes: 42 % less
  CPU per frame than when all change.
- Rust: fewer allocations: 32-byte tokens, smaller elements with inline
  children, reused painter / text buffer / layout scratch, stack-backed
  small lists.
- Fill-only rounded canvas rects use the span-filling rounded-rect routine
  (about 2.5× faster).
- Left-anchored canvas text is no longer measured before drawing.
- Rust crate: style sharing between identical elements, interned and typed
  CSS values, an indexed stylesheet, allocation-free text measuring.
  Animating 3000 elements went from 12 to about 50 fps, at 29 ms CPU per
  frame (from 40).
- Lone text runs fold into their box as one layout leaf. Canvas raster
  threads adapt to the frame deadline, items are bucketed by band, and
  `BK_canvas_put_item` / `BK_canvas_truncate` update a canvas in place
  (the latter invalidates once).

### Changed
- The C sources build with MSVC (`/utf-8`, no `typeof` / `[[nodiscard]]`
  outside GCC and Clang).

## [0.1.0] - first release

### Added
- **C kit** (`kit/`): Blender's UI as a C runtime. It has the node tree,
  Tk's packer and gridder in C, input and interaction (hover, drag grabs,
  continuous cursor, menus, tooltips), widget drawing ported from Blender's
  `interface_widgets.cc`, and damage-driven raster rendering. Also: undo and
  redo, animation, the text area, canvas, scroll views, scrollbars and
  multi-select.
- **Native window host** for Windows (Win32): DPI-aware, vsync'd
  animation, clipboard, cursors, window-manager features. Other platforms use
  a stdlib-Tk-hosted window.
- **`bui.tksyntax`**: the complete tkinter API (7846/7846 public names of
  the tkinter and ttk classes), plus `ttk`, `messagebox`, `filedialog`,
  `simpledialog`, `colorchooser`, `font` and `scrolledtext`. Run existing apps
  unchanged with `python -m bui.tksyntax app.py`, `blenderui-tk app.py` or
  `install()`.
- **`bui.qtsyntax`**: a Qt-style API subset (widgets, layouts, signals,
  `QPainter`, menus, timers).
- **`bui.components`**: 45 Blender components (layouts, values, views,
  menus, pie menus, the color picker, editors, templates).
- Packaging as `blenderui` (scikit-build-core), with `import blenderui` as
  an alias of `bui`.
