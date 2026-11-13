# bui_kit - Blender's interface as a retained-mode C runtime: nodes, layout,
# input handling (interface_handlers.cc rules) and widget drawing ported from
# interface_widgets.cc, rendered into a BuiRaster with damage tracking.
#
# SPDX-License-Identifier: GPL-2.0-or-later

add_library(bui_kit STATIC
  intern/kit_core.c
  intern/kit_draw.c
  intern/kit_theme_presets.c
  intern/kit_icons_data.c
  intern/kit_widgets.c
  intern/kit_views.c
  intern/kit_menus.c
  intern/kit_color.c
  intern/kit_frame.c
  intern/kit_textarea.c
  intern/kit_ime.c
  intern/kit_ime_win32.c
  intern/kit_window_win32.c
  intern/kit_window_x11.c
  intern/kit_window_null.c
  intern/kit_icons_svg.c
  intern/kit_regions.c
  intern/kit_nodes.c
  intern/kit_custom.c
  intern/kit_filebrowser.c # without BUI_FILEBROWSER: BK_file_dialog's stub only
  # images and video (Blender's imbuf design): see intern/image/image.h
  intern/image/imbuf.c
  intern/image/filetype.c
  intern/image/anim.c
  intern/image/play.c
  intern/image/format_svg.c
  intern/image/cache.c
  intern/image/loader.c
  intern/image/draw.c
  intern/image/video.c
  intern/image/movie.c
  # Blender's UI sources copied and edited for the kit (C++; see blender/bk_blender.h)
  intern/blender/interface_string_search.cc
  intern/blender/interface_utils.cc
  intern/blender/interface_undo.cc
  extern/nanosvg/blender_nanosvg.c
  extern/nanosvg/blender_raster.c
)
# Blender's icons: its SVGs (kit_icons_svg.c) rasterized with its nanosvg.
target_include_directories(bui_kit PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/extern/nanosvg)
set_source_files_properties(extern/nanosvg/blender_nanosvg.c extern/nanosvg/blender_raster.c
                            PROPERTIES COMPILE_OPTIONS "$<IF:$<C_COMPILER_ID:MSVC>,/w,-w>")
add_library(bui::kit ALIAS bui_kit)
# the optional parts this build has (features.json): BUI_IMAGE_*, BUI_VIDEO*, ...
bui_features(bui_kit)

if(BUI_FILEBROWSER) # Blender's File Browser (editors/space_file)
  target_sources(bui_kit PRIVATE
    intern/kit_filebrowser_paths.c
    intern/kit_filebrowser_posix.c
    intern/kit_filebrowser_win32.c
    intern/kit_filelist.c
    intern/kit_filelist_view.c
    intern/kit_fileops.c
    intern/kit_fsmenu.c)
endif()

# -- image decoders (third party, decoder parts only; see extern/*/README.bui) --
set(_bui_quiet "$<IF:$<C_COMPILER_ID:MSVC>,/w,-w>")
foreach(_f HDR PNM)
  if(BUI_IMAGE_${_f})
    string(TOLOWER ${_f} _lf)
    target_sources(bui_kit PRIVATE intern/image/format_${_lf}.c)
  endif()
endforeach()
# Wuffs: one translation unit (format_wuffs.c includes wuffs-v0.4.c) with the
# modules of the formats this build has
if(BUI_IMAGE_PNG OR BUI_IMAGE_GIF OR BUI_IMAGE_BMP OR BUI_IMAGE_TGA OR BUI_IMAGE_QOI)
  target_sources(bui_kit PRIVATE intern/image/format_wuffs.c)
  target_include_directories(bui_kit PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/extern/wuffs)
  set_source_files_properties(intern/image/format_wuffs.c PROPERTIES COMPILE_OPTIONS "${_bui_quiet}")
endif()
# Video: the OS backend of this platform, loaded at run time; dav1d / FFmpeg
# when asked for (they link those libraries)
if(BUI_VIDEO)
  if(WIN32)
    target_sources(bui_kit PRIVATE intern/image/movie_mf.c)
    target_compile_definitions(bui_kit PRIVATE BUI_VIDEO_MF)
  elseif(APPLE)
    target_sources(bui_kit PRIVATE intern/image/movie_avf.m)
    set_source_files_properties(intern/image/movie_avf.m PROPERTIES COMPILE_OPTIONS "-fno-objc-arc")
    target_compile_definitions(bui_kit PRIVATE BUI_VIDEO_AVF)
    # weak: a system without them still starts (one argument each for the linker)
    target_link_libraries(bui_kit PUBLIC "-Wl,-weak_framework,AVFoundation"
                                         "-Wl,-weak_framework,CoreMedia"
                                         "-Wl,-weak_framework,CoreVideo")
  else()
    target_sources(bui_kit PRIVATE intern/image/movie_gst.c)
    target_compile_definitions(bui_kit PRIVATE BUI_VIDEO_GST)
    target_link_libraries(bui_kit PUBLIC ${CMAKE_DL_LIBS})
  endif()
endif()
if(BUI_IMAGE_JPEG) # libjpeg-turbo 3.1.2: the 8-bit decompressor, no SIMD (it needs NASM)
  file(GLOB _bui_jpeg_src extern/libjpeg-turbo/src/*.c extern/libjpeg-turbo/src/wrapper/*.c)
  list(FILTER _bui_jpeg_src EXCLUDE REGEX "/(jdcolext|jdcol565|jdmrgext|jdmrg565|jstdhuff|jdapistd|jdcoefct|jdcolor|jddctmgr|jddiffct|jdlossls|jdmainct|jdmerge|jdpostct|jdsample|jidctflt|jidctfst|jidctint|jidctred|jutils)\\.c$")
  add_library(bui_jpeg OBJECT ${_bui_jpeg_src}
    extern/libjpeg-turbo/bui/jmem_bui.c extern/libjpeg-turbo/bui/jdecode_8bit_only.c)
  target_include_directories(bui_jpeg PRIVATE extern/libjpeg-turbo/bui extern/libjpeg-turbo/src
    ${BUI_SOURCE_DIR}/core/include)
  target_compile_options(bui_jpeg PRIVATE ${_bui_quiet} $<$<NOT:$<C_COMPILER_ID:MSVC>>:-O2>)
  target_sources(bui_kit PRIVATE intern/image/format_jpeg.c $<TARGET_OBJECTS:bui_jpeg>)
  target_include_directories(bui_kit PRIVATE extern/libjpeg-turbo/bui extern/libjpeg-turbo/src)
endif()
if(BUI_IMAGE_WEBP) # libwebp 1.6.0: decoder + demux
  file(GLOB _bui_webp_src extern/libwebp/src/dec/*.c extern/libwebp/src/dsp/*.c
    extern/libwebp/src/utils/*.c extern/libwebp/src/demux/*.c)
  add_library(bui_webp OBJECT ${_bui_webp_src})
  target_include_directories(bui_webp PRIVATE extern/libwebp)
  # the decode threads call libwebp at once: its DSP init takes a lock with pthreads
  # (upstream's Windows path is lock-free by design)
  if(NOT WIN32)
    target_compile_definitions(bui_webp PRIVATE WEBP_USE_THREAD)
  endif()
  target_compile_options(bui_webp PRIVATE ${_bui_quiet} $<$<NOT:$<C_COMPILER_ID:MSVC>>:-O2>)
  target_sources(bui_kit PRIVATE intern/image/format_webp.c $<TARGET_OBJECTS:bui_webp>)
  target_include_directories(bui_kit PRIVATE extern/libwebp)
endif()
# worker threads: bui_core links the platform's threads (Threads::Threads)
target_include_directories(bui_kit PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/include)
target_include_directories(bui_kit PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/intern)
# bui_core: Blender's color math and UTF-8 cursor stepping; bui_gpu: raster.
target_link_libraries(bui_kit PUBLIC bui::gpu bui::core)
if(NOT MSVC)
  target_compile_options(bui_kit PRIVATE -O2)
  target_link_libraries(bui_kit PRIVATE m)
endif()
if(WIN32)
  target_link_libraries(bui_kit PRIVATE user32 gdi32 imm32)
elseif(APPLE)
  # Native windows on macOS: an AppKit host after Blender's GHOST Cocoa backend.
  # Objective-C (enable_language(OBJC) is in the top-level CMakeLists.txt) with
  # manual retain/release, as GHOST; kit_window_null.c compiles to nothing.
  target_sources(bui_kit PRIVATE intern/kit_window_cocoa.m)
  set_source_files_properties(intern/kit_window_cocoa.m PROPERTIES COMPILE_OPTIONS "-fno-objc-arc")
  target_compile_definitions(bui_kit PRIVATE BUI_WINDOW_COCOA)
  target_link_libraries(bui_kit PUBLIC "-framework Cocoa" "-framework CoreGraphics" objc)
elseif(UNIX AND NOT APPLE)
  # Native windows on X11 (Wayland sessions run it through XWayland).
  # Only the headers are needed: libX11 is dlopen()ed at run time, so builds
  # (and wheels) run without X11 installed and open windows where it is.
  find_path(BUI_X11_INCLUDE X11/Xlib.h)
  if(BUI_X11_INCLUDE)
    target_compile_definitions(bui_kit PRIVATE BUI_WINDOW_X11)
    target_include_directories(bui_kit PRIVATE ${BUI_X11_INCLUDE})
    target_link_libraries(bui_kit PUBLIC ${CMAKE_DL_LIBS})
  else()
    message(STATUS "BUI: X11 headers not found; no native Linux windows (Tk / headless only)")
  endif()

  # Native Wayland windows, as Blender's GHOST_SystemWayland (xdg-shell, wl_shm, wl_seat with
  # xkbcommon, wl_data_device). Like X11, nothing is linked: Blender's wayland_dynload loads
  # libwayland-client / libwayland-cursor and the host dlopen()s libxkbcommon when the first
  # window opens. Building needs the headers, wayland-scanner and wayland-protocols; without
  # them (or with -DBUI_WAYLAND=OFF) windows are X11 only, as before. A session with
  # WAYLAND_DISPLAY gets Wayland windows (CPU renderer), unless the GPU renderer is on: that is
  # OpenGL through GLX, so it keeps X11 (XWayland) until Wayland has its own EGL renderer.
  option(BUI_WAYLAND "Native Wayland windows (libwayland is loaded at run time)" ON)
  if(BUI_WAYLAND AND BUI_X11_INCLUDE)
    find_package(PkgConfig QUIET)
    if(PkgConfig_FOUND)
      pkg_check_modules(BUI_WL QUIET wayland-client wayland-cursor xkbcommon wayland-protocols)
      pkg_get_variable(BUI_WL_PROTOCOLS wayland-protocols pkgdatadir)
      find_program(BUI_WL_SCANNER wayland-scanner)
    endif()
    if(BUI_WL_FOUND AND BUI_WL_PROTOCOLS AND BUI_WL_SCANNER)
      set(_wl_gen ${CMAKE_CURRENT_BINARY_DIR}/wayland)
      file(MAKE_DIRECTORY ${_wl_gen})
      set(_wl_src)
      foreach(_p stable/xdg-shell/xdg-shell unstable/xdg-decoration/xdg-decoration-unstable-v1
                 staging/xdg-activation/xdg-activation-v1)
        get_filename_component(_n ${_p} NAME)
        add_custom_command(
          OUTPUT ${_wl_gen}/${_n}-client-protocol.h ${_wl_gen}/${_n}-client-protocol.c
          COMMAND ${BUI_WL_SCANNER} client-header ${BUI_WL_PROTOCOLS}/${_p}.xml
                  ${_wl_gen}/${_n}-client-protocol.h
          COMMAND ${BUI_WL_SCANNER} private-code ${BUI_WL_PROTOCOLS}/${_p}.xml
                  ${_wl_gen}/${_n}-client-protocol.c
          DEPENDS ${BUI_WL_PROTOCOLS}/${_p}.xml)
        list(APPEND _wl_src ${_wl_gen}/${_n}-client-protocol.c)
      endforeach()
      target_sources(bui_kit PRIVATE intern/kit_window_wayland.c intern/kit_window_linux.c
        extern/wayland_dynload/wayland_dynload_client.c
        extern/wayland_dynload/wayland_dynload_cursor.c
        extern/wayland_dynload/wayland_dynload_utils.c ${_wl_src})
      target_compile_definitions(bui_kit PRIVATE BUI_WINDOW_WAYLAND)
      target_include_directories(bui_kit PRIVATE ${_wl_gen} extern/wayland_dynload
        ${BUI_WL_INCLUDE_DIRS})
    else()
      message(STATUS "BUI: Wayland headers, wayland-scanner or wayland-protocols not found; "
                     "Linux windows are X11 (Wayland sessions through XWayland)")
    endif()
  endif()
endif()
