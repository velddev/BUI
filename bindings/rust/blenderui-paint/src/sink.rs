//! The one trait a framework implements to be painted into.

use crate::font::Font;
use crate::geom::{Rect, Rgba};

/// Where a view paints. Coordinates are logical pixels relative to the
/// view's top-left; the sink adds its origin. Implemented once per
/// framework.
pub trait PaintSink {
    /// A filled rectangle.
    fn fill(&mut self, r: Rect, c: Rgba);
    /// A rectangle's border, drawn inside it.
    fn stroke(&mut self, r: Rect, c: Rgba, width: f32);
    /// A rounded filled rectangle.
    fn round(&mut self, r: Rect, radius: f32, c: Rgba);
    /// A straight line.
    fn line(&mut self, a: (f32, f32), b: (f32, f32), c: Rgba, width: f32);
    /// Text starting at `x`, its line box vertically centered on `cy`.
    fn text(&mut self, x: f32, cy: f32, text: &str, size: f32, c: Rgba);
    /// The width of `text` at `size`.
    fn text_width(&mut self, text: &str, size: f32) -> f32;
    /// [`PaintSink::text`] in `font`. The default draws it regular, for
    /// frameworks with one face.
    fn text_font(&mut self, x: f32, cy: f32, text: &str, size: f32, font: Font, c: Rgba) {
        let _ = font;
        self.text(x, cy, text, size, c);
    }
    /// [`PaintSink::text_width`] in `font` (what [`PaintSink::text_font`] draws).
    fn text_width_font(&mut self, text: &str, size: f32, font: Font) -> f32 {
        let _ = font;
        self.text_width(text, size)
    }
    /// Run `f` with drawing restricted to `r` (intersected with any clip in
    /// force). It receives the sink to draw into, which may be a different
    /// object than `self` (a framework scopes its clip with a closure).
    fn clipped(&mut self, r: Rect, f: &mut dyn FnMut(&mut dyn PaintSink));
    /// An image file (a path) covering `r` (CSS `object-fit: cover`). Views
    /// call it only for what is on screen, so only visible images load; a
    /// framework decodes asynchronously and shows it when ready (blenderui:
    /// the kit's image loader). The default is a placeholder, for frameworks
    /// without images.
    fn image(&mut self, r: Rect, src: &str) {
        let _ = src;
        self.round(r, 4.0, Rgba::new(0.5, 0.5, 0.5, 0.25));
    }
}
