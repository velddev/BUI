//! Measuring an item: its height from its own record and the list width.
//!
//! A [`Measurer`] describes the item's layout (how wide its text may be,
//! how tall everything that is not text is); [`TextLayout`] does the
//! wrapping and remembers it. The wrapped lines are cached per
//! `(text, wrap width, size)` in the shared bounded
//! [`TextCache`](blenderui_paint::TextCache) (the design of Blender's BLF
//! glyph cache, `blf_glyph.cc`): painting a frame is mostly cache hits, a
//! resize only refills what is on screen, and the cache never holds more
//! than its capacity. A style change (font, size, theme metrics) is
//! [`TextLayout::clear`]. An item's text may be in several faces
//! ([`Measurer::fonts`]); measured with a [`FontWidth`], it wraps where the
//! faces it is painted in fill the line.

use std::hash::{Hash, Hasher};

use blenderui_paint::{Font, TextCache};

use crate::estimate::Estimator;
use crate::fx::Fx;
use crate::source::Record;
use crate::wrap::{wrap_fonts, Advances, Lines};

/// The text measurement the host provides: width of `text` at `size`.
pub type TextWidth<'a> = &'a mut dyn FnMut(&str, f32) -> f32;

/// The text measurement in a face: width of `text` at `size` in `font`
/// (a [`PaintSink`](blenderui_paint::PaintSink)'s `text_width_font`).
pub type FontWidth<'a> = &'a mut dyn FnMut(&str, f32, Font) -> f32;

/// How an item lays out. Heights depend on the record and the width only.
pub trait Measurer {
    fn font_size(&self) -> f32;
    fn line_height(&self) -> f32;

    /// The width `rec`'s text wraps at in a list `list_width` wide.
    fn wrap_width(&self, rec: &Record<'_>, list_width: f32) -> f32;

    /// Everything of `rec`'s height that is not wrapped text (padding,
    /// header, divider, image).
    fn chrome(&self, rec: &Record<'_>) -> f32;

    /// The estimator for blocks nobody measured, for this list width.
    fn estimator(&self, list_width: f32) -> Estimator;

    /// The faces of `rec`'s text: `face(at, font)` for each place a face
    /// starts (byte offset, ascending; before the first, regular). The
    /// default: all regular.
    fn fonts(&self, rec: &Record<'_>, face: &mut dyn FnMut(usize, Font)) {
        let _ = (rec, face);
    }
}

/// A key for `rec`'s faces (0: all regular).
fn fonts_key(rec: &Record<'_>, m: &impl Measurer) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut any = false;
    m.fonts(rec, &mut |at, font| {
        any = true;
        (at, font).hash(&mut h);
    });
    if any { h.finish() | 1 } else { 0 }
}

/// Wrapping with a cache.
pub struct TextLayout {
    /// Lines with the key of the faces they were wrapped in.
    cache: TextCache<(u64, Lines)>,
    adv: Advances,
    fonts: Vec<(usize, Font)>,
    /// Lines for a text whose cache slot holds other faces (a key collision).
    spare: Lines,
}

impl TextLayout {
    /// A layout cache holding up to `capacity` wrapped texts.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self { cache: TextCache::new(capacity), adv: Advances::default(), fonts: Vec::new(), spare: Lines::one() }
    }

    /// Start a frame (ages the cache).
    pub const fn next_generation(&mut self) {
        self.cache.next_generation();
    }

    /// Make room for a screenful and the next (call with the visible count).
    pub fn ensure_capacity(&mut self, n: usize) {
        self.cache.ensure_capacity(n);
    }

    /// Forget everything (style change).
    pub fn clear(&mut self) {
        self.cache.clear();
        self.adv.reset(0.0);
    }

    /// `(hits, misses)`.
    #[must_use]
    pub const fn stats(&self) -> (u64, u64) {
        self.cache.stats()
    }

    /// The cache slot of `rec`'s text: the wrap width's bits, mixed with the
    /// faces' key when it has faces.
    const fn slot(w: f32, fonts: u64) -> u32 {
        w.to_bits() ^ (fonts as u32) ^ ((fonts >> 32) as u32)
    }

    /// The wrapped lines of `rec`'s text at `list_width`.
    pub fn lines(&mut self, rec: &Record<'_>, m: &impl Measurer, list_width: f32, tw: TextWidth<'_>) -> &Lines {
        self.lines_fonts(rec, m, list_width, &mut |s, size, _| tw(s, size))
    }

    /// [`TextLayout::lines`] measuring each face in its font.
    pub fn lines_fonts(&mut self, rec: &Record<'_>, m: &impl Measurer, list_width: f32, tw: FontWidth<'_>) -> &Lines {
        let (size, w) = (m.font_size(), m.wrap_width(rec, list_width));
        if (self.adv.size() - size).abs() > f32::EPSILON {
            self.adv.reset(size);
        }
        let key = fonts_key(rec, m);
        let (adv, fonts) = (&mut self.adv, &mut self.fonts);
        let mut make = || {
            fonts.clear();
            if key != 0 {
                m.fonts(rec, &mut |at, font| fonts.push((at, font)));
            }
            wrap_fonts(rec.text, w, fonts, adv, tw)
        };
        let (k, lines) = self.cache.get_or_insert_with(rec.text, Self::slot(w, key), size, || (key, make()));
        if *k == key {
            return lines;
        }
        self.spare = make();
        &self.spare
    }

    /// The wrapped lines if they are cached (painting reads through a shared
    /// reference; [`ListView::layout`](crate::ListView::layout) has cached what is on screen).
    #[must_use]
    pub fn peek(&self, rec: &Record<'_>, m: &impl Measurer, list_width: f32) -> Option<&Lines> {
        let key = fonts_key(rec, m);
        self.cache.peek(rec.text, Self::slot(m.wrap_width(rec, list_width), key), m.font_size())
            .filter(|(k, _)| *k == key).map(|(_, lines)| lines)
    }

    /// `rec`'s full height at `list_width`.
    pub fn height(&mut self, rec: &Record<'_>, m: &impl Measurer, list_width: f32, tw: TextWidth<'_>) -> Fx {
        self.height_fonts(rec, m, list_width, &mut |s, size, _| tw(s, size))
    }

    /// [`TextLayout::height`] measuring each face in its font.
    pub fn height_fonts(&mut self, rec: &Record<'_>, m: &impl Measurer, list_width: f32, tw: FontWidth<'_>) -> Fx {
        let lines = self.lines_fonts(rec, m, list_width, tw).count();
        Fx::from_px(f64::from(m.chrome(rec)) + lines as f64 * f64::from(m.line_height()))
    }
}
