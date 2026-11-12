//! The blenderui side of painting, shared by every core-driven view (the
//! data table, the variable list): a [`PaintSink`] that appends to the
//! engine's display list ([`Painter`]), the theme palette, and a text-width
//! cache. The views' painting itself lives in the framework-independent
//! cores; this is the one place that knows how a core's calls become engine
//! calls.

use std::cell::Cell as StdCell;

use blenderui_sys as sys;
use blenderui_table::{PaintSink, Palette, Rect, Rgba as CoreRgba, TextCache};
use blenderui_vlist::Font;

use crate::color::Rgba;
use crate::geometry::{Bounds, Point, Size};
use crate::Painter;

fn theme(key: &std::ffi::CStr) -> CoreRgba {
    let mut c = [0.0f32; 4];
    // SAFETY: `c` holds the four floats the kit writes; `key` is NUL-terminated.
    unsafe { sys::BK_theme_color(key.as_ptr(), c.as_mut_ptr()) };
    CoreRgba::new(c[0], c[1], c[2], c[3])
}

/// The views' colors from the kit's theme.
pub(crate) fn current_palette() -> Palette {
    Palette::new(theme(c"region_bg"), theme(c"text"), theme(c"file_row_alternate"), theme(c"list_item.inner_sel"), theme(c"list_item.item"))
}

const fn color(c: CoreRgba) -> Rgba {
    Rgba { r: c.r, g: c.g, b: c.b, a: c.a }
}

fn bounds(r: Rect, origin: Point<f32>) -> Bounds<f32> {
    Bounds { origin: Point { x: origin.x + r.x, y: origin.y + r.y }, size: Size { width: r.w, height: r.h } }
}

/// Text widths at one scale; dropped when the scale changes.
struct Widths {
    scale: f32,
    cache: TextCache<f32>,
}

thread_local! {
    /// Kept across frames: a scrolling view shows mostly the same strings.
    static WIDTHS: StdCell<Option<Widths>> = const { StdCell::new(None) };
}

/// The engine's font flags for a face: the UI font's weight and slant, or
/// the family CSS `monospace` resolves to.
fn font_flags(font: Font) -> i32 {
    static MONO: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    match font {
        Font::Mono => *MONO.get_or_init(|| i32::from(crate::text::font_id("monospace")) << crate::text::FAMILY_SHIFT),
        f => (if f.bold() { sys::BUI_FONT_BOLD } else { 0 }) | (if f.italic() { sys::BUI_FONT_ITALIC } else { 0 }),
    }
}

/// Width of `s` at `size` logical px in `font`, measured by the text engine
/// at the device size it is drawn at.
pub(crate) fn measure(s: &str, size: f32, font: Font, scale: f32) -> f32 {
    let (fam, low) = crate::text::family(font_flags(font));
    let mut w = 0.0f32;
    // SAFETY: pointers valid for the call; `family` returns static strings.
    let ok = unsafe { sys::BUI_text_measure(fam, low, size * scale, s.as_ptr().cast(), s.len() as i32, &mut w) };
    if ok == 0 { s.chars().count() as f32 * size * 0.55 } else { w / scale }
}

/// Top of a text item of `size` px whose line box is centered on `cy`.
fn text_top(cy: f32, size: f32) -> f32 {
    let base = crate::text::baseline(cy - size * 0.5, size, size);
    crate::text::item_top(base, size)
}

/// Draws into a [`Painter`] at `origin`.
pub(crate) struct PainterSink<'a> {
    p: &'a mut Painter,
    origin: Point<f32>,
    scale: f32,
    widths: &'a mut TextCache<f32>,
}

/// Run `f` with a sink drawing into `p` at `origin` (logical px, at device `scale`).
pub(crate) fn with_sink<R>(p: &mut Painter, origin: Point<f32>, scale: f32, f: impl FnOnce(&mut PainterSink<'_>) -> R) -> R {
    let scale = scale.max(f32::MIN_POSITIVE);
    let mut w = WIDTHS.take().filter(|w| w.scale == scale).unwrap_or_else(|| Widths { scale, cache: TextCache::new(20_000) });
    w.cache.next_generation();
    let r = f(&mut PainterSink { p, origin, scale, widths: &mut w.cache });
    WIDTHS.set(Some(w));
    r
}

impl PaintSink for PainterSink<'_> {
    fn fill(&mut self, r: Rect, c: CoreRgba) {
        self.p.fill(bounds(r, self.origin), color(c));
    }
    fn stroke(&mut self, r: Rect, c: CoreRgba, width: f32) {
        self.p.quad(bounds(r, self.origin), 0.0, Rgba::TRANSPARENT, color(c), width);
    }
    fn round(&mut self, r: Rect, radius: f32, c: CoreRgba) {
        self.p.quad(bounds(r, self.origin), radius, color(c), Rgba::TRANSPARENT, 0.0);
    }
    fn line(&mut self, a: (f32, f32), b: (f32, f32), c: CoreRgba, width: f32) {
        let o = self.origin;
        self.p.line(&[(a.0 + o.x, a.1 + o.y), (b.0 + o.x, b.1 + o.y)], color(c), width);
    }
    fn text(&mut self, x: f32, cy: f32, text: &str, size: f32, c: CoreRgba) {
        self.text_font(x, cy, text, size, Font::Regular, c);
    }
    fn text_width(&mut self, text: &str, size: f32) -> f32 {
        self.text_width_font(text, size, Font::Regular)
    }
    fn text_font(&mut self, x: f32, cy: f32, text: &str, size: f32, font: Font, c: CoreRgba) {
        let top = text_top(self.origin.y + cy, size);
        self.p.styled_text(self.origin.x + x, top, text, size, font_flags(font), color(c));
    }
    fn text_width_font(&mut self, text: &str, size: f32, font: Font) -> f32 {
        let scale = self.scale;
        // the cache's font slot: the face
        *self.widths.get_or_insert_with(text, font.index() as u32, size, || measure(text, size, font, scale))
    }
    fn image(&mut self, r: Rect, src: &str) {
        self.p.image_file_cover(bounds(r, self.origin), src);
    }
    fn image_round(&mut self, r: Rect, radius: f32, src: &str) {
        self.p.image_file_round(bounds(r, self.origin), src, sys::BK_FIT_COVER, radius);
    }
    fn clipped(&mut self, r: Rect, f: &mut dyn FnMut(&mut dyn PaintSink)) {
        let (origin, scale) = (self.origin, self.scale);
        let widths = &mut *self.widths;
        self.p.with_clip(bounds(r, origin), |p| f(&mut PainterSink { p, origin, scale, widths }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_measure_as_drawn() {
        let w = |font| measure("mmmm iiii", 13.0, font, 1.0);
        // bold is wider; monospace gives "i" a whole cell
        assert!(w(Font::Bold) > w(Font::Regular), "{} {}", w(Font::Bold), w(Font::Regular));
        assert!(w(Font::Mono) > w(Font::Regular));
        assert_eq!(measure("iiii", 13.0, Font::Mono, 1.0), measure("mmmm", 13.0, Font::Mono, 1.0));
        // the sink's widths are per face
        let mut p = Painter::default();
        with_sink(&mut p, Point { x: 0.0, y: 0.0 }, 1.0, |s| {
            assert_eq!(s.text_width("mmmm iiii", 13.0), w(Font::Regular));
            assert_eq!(s.text_width_font("mmmm iiii", 13.0, Font::Mono), w(Font::Mono));
            s.text_font(0.0, 10.0, "mmmm", 13.0, Font::Bold, CoreRgba::new(1.0, 1.0, 1.0, 1.0));
        });
    }
}
