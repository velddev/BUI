//! The paint vocabulary shared by BUI's framework-independent view cores
//! (`blenderui-table`, `blenderui-vlist`): geometry and colors ([`geom`]),
//! the [`PaintSink`] a UI framework implements once, the text faces it
//! paints in ([`Font`]), and the bounded [`TextCache`]. One source of truth;
//! the cores re-export what their own users need. `std` only.

#![allow(clippy::float_cmp, clippy::many_single_char_names, clippy::cast_possible_wrap)]

pub mod font;
pub mod geom;
pub mod palette;
pub mod sink;
pub mod testing;
pub mod text_cache;

pub use font::Font;
pub use geom::{Point, Rect, Rgba};
pub use palette::Palette;
pub use sink::PaintSink;
pub use text_cache::TextCache;

#[cfg(test)]
#[global_allocator]
static ALLOC: testing::Counting = testing::Counting;
