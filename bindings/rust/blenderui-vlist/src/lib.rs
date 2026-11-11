//! A variable-height virtualized list for chat-grade UIs, one design for
//! 100 items and for 10M: the logic once, for every UI framework.
//!
//! | Blender | here |
//! |---|---|
//! | `textview_draw` (`space_info/textview.cc`): walk from the newest line backwards, stop past the view top | [`ListView`]: anchor-based position, [`view::Pos::Bottom`] opens at the newest item touching only the last screenful |
//! | `textview_wrap_offsets`: wrap a line where it is drawn | [`wrap`]: the same pass in pixels, cached per `(text, width, size)` ([`measure::TextLayout`]), ASCII from an advance table |
//! | `OffsetIndices`, `accumulate_counts_to_offsets` (`BLI_offset_indices.hh`): static prefix sums | [`index::HeightIndex`]: prefix sums per block under a Fenwick tree, updatable |
//! | BLF glyph cache (`blf_glyph.cc`) | the shared bounded [`blenderui_paint::TextCache`] |
//! | `UI_widgetbase_draw_cache`: batched quads | direct painting of the placed items through [`PaintSink`] |
//!
//! Layering: [`source::ItemSource`] (data) and [`measure::Measurer`]
//! (layout) feed a [`ListView`] (state, index, one frame), which paints
//! through an [`paint::ItemPainter`] into a [`PaintSink`]. [`chat`] is a
//! Measurer + ItemPainter for messages. A framework adapter owns a view
//! next to its source, translates events into view calls and implements
//! `PaintSink`. `std` only.

// Deliberate: heights are fixed-point integers converted at the edges; short
// names in the geometry; exact float comparisons only against sentinels.
#![allow(clippy::float_cmp, clippy::many_single_char_names, clippy::cast_possible_wrap, clippy::items_after_statements)]

pub mod chat;
pub mod estimate;
pub mod fx;
pub mod index;
pub mod list;
pub mod log;
pub mod measure;
pub mod paint;
pub mod rows;
pub mod scrollbar;
pub mod source;
pub mod synth;
pub mod view;
pub mod wrap;

pub use blenderui_paint::{Font, PaintSink, Palette, Point, Rect, Rgba, TextCache};
pub use chat::{ChatList, ChatStyle};
pub use list::{Hit, List};
pub use estimate::Estimator;
pub use fx::Fx;
pub use index::{HeightIndex, IndexError};
pub use measure::{FontWidth, Measurer, TextLayout, TextWidth};
pub use paint::ItemPainter;
pub use rows::{CountSource, RowList, RowStyle};
pub use source::{ItemSource, Message, Page, Record, VecSource};
pub use view::{Align, Budget, ListView, Nav, Options, Placed, Pos};
pub use wrap::Lines;

#[cfg(test)]
#[global_allocator]
static ALLOC: blenderui_paint::testing::Counting = blenderui_paint::testing::Counting;
