//! A title bar drawn by the app (gpui's `WindowControlArea`). A window opened with
//! [`TitlebarOptions::appears_transparent`](crate::TitlebarOptions) has no system title bar on
//! Windows and macOS: the app draws its own and marks its parts with
//! [`window_control_area`](crate::Div::window_control_area): the empty bar
//! ([`WindowControlArea::Drag`]) moves the window and zooms it on a double click, and the
//! maximise button ([`WindowControlArea::Max`]) is the system's to Windows, which offers its
//! snap layouts over it. The buttons' clicks are still the app's. An area covers the element's
//! subtree down to any element with listeners of its own (or a form widget): those stay the
//! app's, so a tab or a button on a drag area takes its clicks.
//!
//! After each layout the window hands the native host the rectangles that matter in paint
//! order ([`areas`]): every box an area-marked one could be hit through, each with the area of
//! its nearest marked ancestor, so the host's lookup (`BK_areas_at`, the last rectangle holding
//! the point) answers as the window's own hit test would: a menu's backdrop over the bar
//! covers it, a tab on the bar is no drag area.

use std::os::raw::c_int;

use crate::geometry::Bounds;
use crate::layout::{Frame, StyleStore};
use crate::sys;

/// The attribute [`crate::Div::window_control_area`] sets.
pub(crate) const ATTR: &str = "bui-window-area";

/// What a part of an app-drawn title bar is to the window (gpui's `WindowControlArea`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WindowControlArea {
    /// Moves the window; a double click zooms (maximises) it.
    Drag,
    /// The close button.
    Close,
    /// The maximise (zoom, restore) button.
    Max,
    /// The minimise button.
    Min,
}

impl WindowControlArea {
    pub(crate) fn name(self) -> &'static str {
        match self {
            WindowControlArea::Drag => "drag",
            WindowControlArea::Close => "close",
            WindowControlArea::Max => "max",
            WindowControlArea::Min => "min",
        }
    }

    fn parse(s: &str) -> Option<WindowControlArea> {
        Some(match s {
            "drag" => WindowControlArea::Drag,
            "close" => WindowControlArea::Close,
            "max" => WindowControlArea::Max,
            "min" => WindowControlArea::Min,
            _ => return None,
        })
    }

    fn code(self) -> c_int {
        match self {
            WindowControlArea::Drag => sys::BK_AREA_DRAG,
            WindowControlArea::Close => sys::BK_AREA_CLOSE,
            WindowControlArea::Max => sys::BK_AREA_MAX,
            WindowControlArea::Min => sys::BK_AREA_MIN,
        }
    }

    pub(crate) fn of_code(code: c_int) -> Option<WindowControlArea> {
        Some(match code {
            sys::BK_AREA_DRAG => WindowControlArea::Drag,
            sys::BK_AREA_CLOSE => WindowControlArea::Close,
            sys::BK_AREA_MAX => WindowControlArea::Max,
            sys::BK_AREA_MIN => WindowControlArea::Min,
            _ => return None,
        })
    }
}

/// The area of box `i`: its own, else its nearest marked ancestor's, unless an element on the
/// way listens to the pointer itself (a tab, a button) or is a form widget: those are the
/// app's, so whatever is clickable on a bar never drags the window.
pub(crate) fn area_of(f: &Frame, mut i: usize) -> Option<WindowControlArea> {
    loop {
        if let Some(a) = f.attr(i, ATTR).and_then(WindowControlArea::parse) {
            return Some(a);
        }
        let n = &f.nodes[i];
        if matches!(n.content, crate::layout::Content::Widget(_)) || f.el(i).is_some_and(|e| e.listeners.is_some()) {
            return None;
        }
        i = n.parent?;
    }
}

/// The area under (`x`, `y`) (logical px), as the window's own hit test finds the box there.
pub(crate) fn area_at(f: &Frame, styles: &StyleStore, x: f32, y: f32) -> Option<WindowControlArea> {
    let path = f.hit_path(x, y, styles);
    area_of(f, *path.last()?)
}

/// The rectangles the native host needs (device px, `scale` per logical px), in hit order:
/// every box that can take the pointer over the marked boxes' union, lowest first. Empty
/// when nothing is marked.
pub(crate) fn areas(f: &Frame, styles: &StyleStore, scale: f32) -> Vec<sys::BKWindowArea> {
    let hit_box = |n: &crate::layout::BoxNode| -> Option<Bounds<f32>> {
        if !n.visible || n.taffy.is_none() || !styles[n.style].pointer_events {
            return None;
        }
        Some(match n.clip {
            Some(c) => n.bounds.intersect(&c),
            None => n.bounds,
        })
    };
    // the union of the marked boxes (a rectangle around them)
    let mut union: Option<(f32, f32, f32, f32)> = None;
    for (i, n) in f.nodes.iter().enumerate() {
        if f.attr(i, ATTR).and_then(WindowControlArea::parse).is_none() {
            continue;
        }
        let Some(b) = hit_box(n) else { continue };
        let (x0, y0, x1, y1) = (b.origin.x, b.origin.y, b.origin.x + b.size.width, b.origin.y + b.size.height);
        union = Some(match union {
            None => (x0, y0, x1, y1),
            Some((a, b, c, d)) => (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
        });
    }
    let Some((ux0, uy0, ux1, uy1)) = union else { return Vec::new() };
    let within = crate::geometry::rect(ux0, uy0, ux1 - ux0, uy1 - uy0);
    // as hit_path: the highest z-index, the later box on a tie
    let mut out: Vec<(i32, usize, sys::BKWindowArea)> = Vec::new();
    for (i, n) in f.nodes.iter().enumerate() {
        let Some(b) = hit_box(n) else { continue };
        let b = b.intersect(&within);
        if b.size.width <= 0.0 || b.size.height <= 0.0 {
            continue;
        }
        let area = area_of(f, n.owner).map_or(sys::BK_AREA_CLIENT, WindowControlArea::code);
        let rect = sys::BKWindowArea {
            x: b.origin.x * scale,
            y: b.origin.y * scale,
            w: b.size.width * scale,
            h: b.size.height * scale,
            area,
        };
        out.push((f.z_of(i, styles), i, rect));
    }
    out.sort_by_key(|&(z, i, _)| (z, i));
    out.into_iter().map(|(_, _, r)| r).collect()
}

/// `BK_areas_at` over `areas` at a device-px point.
pub(crate) fn lookup(areas: &[sys::BKWindowArea], x: f32, y: f32) -> Option<WindowControlArea> {
    // SAFETY: a valid slice for the length given; the kit only reads it.
    let code = unsafe { sys::BK_areas_at(areas.as_ptr(), areas.len() as c_int, x, y) };
    WindowControlArea::of_code(code)
}
