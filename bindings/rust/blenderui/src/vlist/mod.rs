//! A variable-height virtualized list, chat-grade, from 100 items to 10M+:
//! one design, drawn the way Blender's text view draws (from the newest
//! line, only what is visible), with a Fenwick-over-blocks height index.
//!
//! Everything that is not blenderui lives in the `blenderui-vlist` crate,
//! once: the height index, anchor-based scrolling, measuring and wrapping,
//! the item sources (in memory, file-backed) and the chat look. This module
//! is the adapter:
//!
//! * [`ListState`]: the entity around a [`List`] (the core's items + style +
//!   view), which lays a frame out while rendering and answers input;
//! * [`VariableList`] ([`variable_list`]): the element, one painted leaf that
//!   draws the state ([`crate::sink`] turns the core's paint calls into the
//!   engine's display list), plus pointer, wheel and keyboard input;
//! * [`chat_list`]: a [`VariableList`] over a [`ChatList`] with whole-message
//!   selection wired.
//!
//! ```ignore
//! let chat = cx.new(|cx| ListState::new(ChatList::from_messages(msgs, palette, 600.0, 400.0, opts), cx));
//! // in render
//! chat_list(&chat).size_full()
//! // from anywhere
//! chat.update(cx, |s, _| s.push(message));
//! ```

mod element;
mod state;

pub use blenderui_vlist::{
    Align, Budget, ChatList, ChatStyle, CountSource, Font, FontWidth, Hit, ItemPainter, ItemSource, List, ListView, Measurer, Message, Nav, Options, Record,
    RowList, RowStyle, VecSource,
};
pub use blenderui_vlist::synth;
pub use element::{chat_list, variable_list, VariableList};
pub use state::{ListState, ViewInfo};

/// The colors of the engine's current theme, for a list style's palette.
#[must_use]
pub fn theme_palette() -> blenderui_vlist::Palette {
    crate::sink::current_palette()
}
