//! [`TableState`]: the entity around a [`TableCore`], like gpui-component's
//! `TableState`. The core (in the `blenderui-table` crate) holds the model
//! (scroll, widths, order, sort, selection ...), the delegate, and the glue
//! shared by every adapter; what is here is what needs blenderui: the
//! [`Host`] that reaches the delegate's hooks and the window, and turning
//! engine input into the core's.
//!
//! The core's API is reachable on the state through `Deref`
//! (`state.rows()`, `state.selected_row()`, `state.set_selected_row(r, cx)`,
//! `state.sort_by(..)` ...).

use std::marker::PhantomData;
use std::ops::{Deref, DerefMut, Range};

use blenderui_table::{table_option_builders, Button, Column, ColumnGroup, ColumnSort, Host, Key, Mods, Point, TableCore};

use super::delegate::TableDelegate;
use crate::{App, Bounds, Context, Entity, KeyDownEvent, Modifiers, MouseButton, Pixels, SharedString, Window};

/// Reaches the delegate trait, the window and the entity context.
pub struct BuiHost<D>(PhantomData<fn() -> D>);

impl<D: TableDelegate> Host for BuiHost<D> {
    type Delegate = D;
    type Window = Window;
    type Cx<'a> = Context<'a, TableState<D>>;

    fn columns(d: &D, cx: &Self::Cx<'_>) -> Vec<Column> {
        let app: &App = cx;
        (0..d.columns_count(app)).map(|i| d.column(i, app)).collect()
    }
    fn group_headers(d: &D, cx: &Self::Cx<'_>) -> Option<Vec<Vec<ColumnGroup>>> {
        d.group_headers(cx)
    }
    fn rows_count(d: &D, cx: &Self::Cx<'_>) -> usize {
        d.rows_count(cx)
    }
    fn loading(d: &D, cx: &Self::Cx<'_>) -> bool {
        d.loading(cx)
    }
    fn sort_by_mask(d: &D) -> bool {
        d.sort_by_mask()
    }
    fn perform_sort(d: &mut D, col: usize, sort: ColumnSort, window: &mut Window, cx: &mut Self::Cx<'_>) {
        d.perform_sort(col, sort, window, cx);
    }
    fn move_column(d: &mut D, col: usize, to: usize, window: &mut Window, cx: &mut Self::Cx<'_>) {
        d.move_column(col, to, window, cx);
    }
    fn fit_column(d: &mut D, col: usize, window: &mut Window, cx: &mut Self::Cx<'_>) -> Option<f32> {
        d.fit_column(col, window, cx).map(|p| p.0)
    }
    fn visible_rows_changed(d: &mut D, rows: Range<usize>, window: &mut Window, cx: &mut Self::Cx<'_>) {
        d.visible_rows_changed(rows, window, cx);
    }
    fn visible_columns_changed(d: &mut D, cols: Range<usize>, window: &mut Window, cx: &mut Self::Cx<'_>) {
        d.visible_columns_changed(cols, window, cx);
    }
    fn has_more(d: &D, cx: &Self::Cx<'_>) -> bool {
        d.has_more(cx)
    }
    fn load_more_threshold(d: &D) -> usize {
        d.load_more_threshold()
    }
    fn load_more(d: &mut D, window: &mut Window, cx: &mut Self::Cx<'_>) {
        d.load_more(window, cx);
    }
    fn write_clipboard(window: &mut Window, _: &mut Self::Cx<'_>, text: &str) {
        window.write_clipboard(text);
    }
    fn notify(cx: &mut Self::Cx<'_>) {
        cx.notify();
    }
    fn refresh(window: &mut Window) {
        window.refresh();
    }
}

/// The table.
pub struct TableState<D: TableDelegate> {
    core: TableCore<BuiHost<D>>,
    /// Where the table was last laid out (window coordinates).
    pub(crate) bounds: Bounds<f32>,
    pub(crate) id: SharedString,
}

impl<D: TableDelegate> Deref for TableState<D> {
    type Target = TableCore<BuiHost<D>>;
    fn deref(&self) -> &Self::Target {
        &self.core
    }
}

impl<D: TableDelegate> DerefMut for TableState<D> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.core
    }
}

const fn mods(m: Modifiers) -> Mods {
    Mods { shift: m.shift, control: m.control || m.platform, alt: m.alt }
}

impl<D: TableDelegate> TableState<D> {
    pub fn new(delegate: D, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let id = SharedString::from(format!("bui-table-{}", cx.entity().entity_id()));
        Self { core: TableCore::new(delegate, cx), bounds: Bounds::default(), id }
    }

    // gpui-component's builders
    table_option_builders!();

    /// Called while rendering with the table's last bounds (see
    /// [`TableCore::sync`]).
    pub fn sync(&mut self, bounds: Bounds<f32>, unit: f32, scale: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.bounds = bounds;
        self.core.sync(bounds.size.width, bounds.size.height, unit, scale, window, cx);
    }

    /// The width of delegate column `col_ix`.
    pub fn col_width(&self, col_ix: usize) -> Option<Pixels> {
        self.core.col_width(col_ix).map(Pixels)
    }

    // -- engine input -> the core's ---------------------------------------------------------------------
    fn local(&self, pos: crate::Point<f32>) -> Point {
        Point { x: pos.x - self.bounds.origin.x, y: pos.y - self.bounds.origin.y }
    }

    /// Pointer pressed at window position `pos`.
    pub fn mouse_down(&mut self, pos: crate::Point<f32>, button: MouseButton, m: Modifiers, clicks: usize,
                      window: &mut Window, cx: &mut Context<Self>) {
        let b = match button {
            MouseButton::Left => Button::Left,
            MouseButton::Right => Button::Right,
            MouseButton::Middle => Button::Middle,
            MouseButton::Back | MouseButton::Forward => return, // the window's navigation
        };
        let p = self.local(pos);
        self.core.pointer_down(p, b, mods(m), clicks, window, cx);
    }

    /// Pointer moved to window position `pos`. Returns whether to redraw.
    pub fn mouse_move(&mut self, pos: crate::Point<f32>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let p = self.local(pos);
        self.core.pointer_move(p, window, cx)
    }

    /// A key while the table has focus.
    pub fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let k = &e.keystroke;
        self.core.key_down(Key::parse(&k.key), mods(k.modifiers), window, cx)
    }
}

/// A handle type for docs and signatures.
pub type TableHandle<D> = Entity<TableState<D>>;
