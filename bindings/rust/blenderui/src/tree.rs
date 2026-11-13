//! The layout tree: taffy's algorithms (block with floats, flexbox, grid,
//! out-of-flow positioning, rounding) over our own node storage.
//!
//! taffy's `TaffyTree` resolves every `calc()` to 0; this tree resolves
//! them (`crate::calc`), so `width: calc(100% - 2rem)` and friends work. It
//! is also leaner: nodes live in a `Vec` with a free list (no slot maps), and
//! a node's cache is only cleared when its own inputs change: the
//! reconciler in `layout.rs` marks changed subtrees itself.

use taffy::compute::{
    compute_block_layout, compute_cached_layout, compute_flexbox_layout, compute_grid_layout,
    compute_hidden_layout, compute_oof_layout, compute_root_layout, round_layout, BlockContext,
};
use taffy::prelude::*;
use taffy::tree::{Cache, DetailedLayoutInfo, Layout, LayoutInput, LayoutOutput, RunMode};
use taffy::{
    CacheTree, DetailedGridInfo, LayoutBlockContainer, LayoutContainingBlock, LayoutFlexboxContainer,
    LayoutGridContainer, LayoutPartialTree, RoundTree, TraversePartialTree, TraverseTree,
};

struct Node<C> {
    style: Style,
    ctx: Option<C>,
    children: Vec<NodeId>,
    hoisted: Vec<NodeId>,
    cache: Cache,
    unrounded: Layout,
    layout: Layout,
    detail: DetailedLayoutInfo,
    live: bool,
}

/// Nodes with an optional measure context `C` per leaf.
pub(crate) struct LayoutTree<C> {
    nodes: Vec<Node<C>>,
    free: Vec<usize>,
}

impl<C> Default for LayoutTree<C> {
    fn default() -> Self {
        LayoutTree { nodes: Vec::new(), free: Vec::new() }
    }
}

#[inline]
fn ix(id: NodeId) -> usize {
    u64::from(id) as usize
}

impl<C> LayoutTree<C> {
    fn alloc(&mut self, style: Style, ctx: Option<C>, children: &[NodeId]) -> NodeId {
        let node = Node {
            style,
            ctx,
            children: children.to_vec(),
            hoisted: Vec::new(),
            cache: Cache::new(),
            unrounded: Layout::new(),
            layout: Layout::new(),
            detail: DetailedLayoutInfo::None,
            live: true,
        };
        match self.free.pop() {
            Some(i) => {
                self.nodes[i] = node;
                NodeId::from(i)
            }
            None => {
                self.nodes.push(node);
                NodeId::from(self.nodes.len() - 1)
            }
        }
    }

    #[allow(dead_code)]
    pub fn new_leaf(&mut self, style: Style) -> NodeId {
        self.alloc(style, None, &[])
    }

    pub fn new_leaf_with_context(&mut self, style: Style, ctx: C) -> NodeId {
        self.alloc(style, Some(ctx), &[])
    }

    pub fn new_with_children(&mut self, style: Style, children: &[NodeId]) -> NodeId {
        self.alloc(style, None, children)
    }

    /// Frees a node (its children are not touched: the reconciler removes
    /// every unused node itself).
    pub fn remove(&mut self, id: NodeId) {
        let i = ix(id);
        if let Some(n) = self.nodes.get_mut(i) {
            if n.live {
                n.live = false;
                n.ctx = None;
                n.children = Vec::new();
                n.hoisted = Vec::new();
                n.detail = DetailedLayoutInfo::None;
                self.free.push(i);
            }
        }
    }

    pub fn style(&self, id: NodeId) -> &Style {
        &self.nodes[ix(id)].style
    }

    pub fn set_style(&mut self, id: NodeId, style: Style) {
        let n = &mut self.nodes[ix(id)];
        n.style = style;
        n.cache.clear();
    }

    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.nodes[ix(id)].children
    }

    pub fn set_children(&mut self, id: NodeId, children: &[NodeId]) {
        let n = &mut self.nodes[ix(id)];
        n.children.clear();
        n.children.extend_from_slice(children);
        n.cache.clear();
    }

    pub fn context(&self, id: NodeId) -> Option<&C> {
        self.nodes[ix(id)].ctx.as_ref()
    }

    pub fn set_context(&mut self, id: NodeId, ctx: Option<C>) {
        let n = &mut self.nodes[ix(id)];
        n.ctx = ctx;
        n.cache.clear();
    }

    /// Forget every node's cached layout (text measured at another scale).
    pub fn clear_caches(&mut self) {
        for n in &mut self.nodes {
            n.cache.clear();
        }
    }

    pub fn mark_dirty(&mut self, id: NodeId) {
        self.nodes[ix(id)].cache.clear();
    }

    /// The final (rounded) layout of a node.
    pub fn layout(&self, id: NodeId) -> &Layout {
        &self.nodes[ix(id)].layout
    }

    /// Lay out the tree under `root`; `measure` sizes leaves that carry a context.
    pub fn compute<M>(&mut self, root: NodeId, available: Size<AvailableSpace>, measure: M)
    where
        M: FnMut(Size<Option<f32>>, Size<AvailableSpace>, &C) -> Size<f32>,
    {
        let mut view = View { t: self, measure };
        compute_root_layout(&mut view, root, available);
        round_layout(&mut view, root);
    }
}

/// The tree plus the measure function for one layout pass.
struct View<'t, C, M> {
    t: &'t mut LayoutTree<C>,
    measure: M,
}

impl<C, M> View<'_, C, M>
where
    M: FnMut(Size<Option<f32>>, Size<AvailableSpace>, &C) -> Size<f32>,
{
    fn child_layout(&mut self, id: NodeId, inputs: LayoutInput, block: Option<&mut BlockContext<'_>>)
        -> LayoutOutput {
        if inputs.run_mode == RunMode::PerformHiddenLayout {
            self.t.nodes[ix(id)].layout = Layout::with_order(0);
            return compute_hidden_layout(self, id);
        }
        compute_cached_layout(self, id, inputs, |tree, id, inputs| {
            let n = &tree.t.nodes[ix(id)];
            let mut out = match (n.style.display, !n.children.is_empty()) {
                (Display::None, _) => compute_hidden_layout(tree, id),
                (Display::Block, true) => compute_block_layout(tree, id, inputs, block),
                (Display::FlowRoot, true) => compute_block_layout(tree, id, inputs, None),
                (Display::Flex, true) => compute_flexbox_layout(tree, id, inputs),
                (Display::Grid, true) => compute_grid_layout(tree, id, inputs),
                (_, false) => {
                    let n = &tree.t.nodes[ix(id)];
                    let measure = &mut tree.measure;
                    match &n.ctx {
                        Some(c) => taffy::compute_leaf_layout(inputs, &n.style, resolve_calc,
                                                              |known, avail| measure(known, avail, c)),
                        None => taffy::compute_leaf_layout(inputs, &n.style, resolve_calc,
                                                           |_, _| Size::ZERO),
                    }
                }
            };
            if inputs.run_mode == RunMode::PerformLayout {
                compute_oof_layout(tree, id, &mut out);
            }
            out
        })
    }
}

/// taffy's calc callback: the pointer is a `&'static crate::calc::Resolved`.
fn resolve_calc(ptr: *const (), basis: f32) -> f32 {
    // SAFETY: every calc pointer in a style comes from `crate::calc::intern`,
    // which leaks its values: they live for the rest of the process.
    unsafe { crate::calc::Resolved::from_ptr(ptr) }.eval(basis)
}

impl<C, M> TraversePartialTree for View<'_, C, M>
where
    M: FnMut(Size<Option<f32>>, Size<AvailableSpace>, &C) -> Size<f32>,
{
    type ChildIter<'a> = std::iter::Copied<std::slice::Iter<'a, NodeId>> where Self: 'a;

    #[inline(always)]
    fn child_ids(&self, id: NodeId) -> Self::ChildIter<'_> {
        self.t.nodes[ix(id)].children.iter().copied()
    }

    #[inline(always)]
    fn child_count(&self, id: NodeId) -> usize {
        self.t.nodes[ix(id)].children.len()
    }

    #[inline(always)]
    fn get_child_id(&self, id: NodeId, k: usize) -> NodeId {
        self.t.nodes[ix(id)].children[k]
    }
}

impl<C, M> TraverseTree for View<'_, C, M> where M: FnMut(Size<Option<f32>>, Size<AvailableSpace>, &C) -> Size<f32> {}

impl<C, M> LayoutPartialTree for View<'_, C, M>
where
    M: FnMut(Size<Option<f32>>, Size<AvailableSpace>, &C) -> Size<f32>,
{
    type CoreContainerStyle<'a> = &'a Style where Self: 'a;
    type CustomIdent = String;

    #[inline(always)]
    fn get_core_container_style(&self, id: NodeId) -> Self::CoreContainerStyle<'_> {
        &self.t.nodes[ix(id)].style
    }

    #[inline(always)]
    fn set_unrounded_layout(&mut self, id: NodeId, layout: &Layout) {
        self.t.nodes[ix(id)].unrounded = *layout;
    }

    #[inline(always)]
    fn resolve_calc_value(&self, ptr: *const (), basis: f32) -> f32 {
        resolve_calc(ptr, basis)
    }

    #[inline(always)]
    fn compute_child_layout(&mut self, id: NodeId, inputs: LayoutInput) -> LayoutOutput {
        self.child_layout(id, inputs, None)
    }
}

impl<C, M> LayoutContainingBlock for View<'_, C, M>
where
    M: FnMut(Size<Option<f32>>, Size<AvailableSpace>, &C) -> Size<f32>,
{
    type OofItemStyle<'a> = &'a Style where Self: 'a;

    #[inline(always)]
    fn get_oof_item_style(&self, id: NodeId) -> Self::OofItemStyle<'_> {
        &self.t.nodes[ix(id)].style
    }

    #[inline(always)]
    fn clear_hoisted_children(&mut self, id: NodeId) {
        self.t.nodes[ix(id)].hoisted.clear();
    }

    #[inline(always)]
    fn add_hoisted_children(&mut self, id: NodeId, hoisted: &[NodeId]) {
        self.t.nodes[ix(id)].hoisted.extend_from_slice(hoisted);
    }

    #[inline(always)]
    fn get_detailed_layout_info(&self, id: NodeId) -> &DetailedLayoutInfo {
        &self.t.nodes[ix(id)].detail
    }
}

impl<C, M> CacheTree for View<'_, C, M>
where
    M: FnMut(Size<Option<f32>>, Size<AvailableSpace>, &C) -> Size<f32>,
{
    #[inline(always)]
    fn cache_get(&mut self, id: NodeId, input: &LayoutInput) -> Option<LayoutOutput> {
        self.t.nodes[ix(id)].cache.get(input)
    }

    #[inline(always)]
    fn cache_store(&mut self, id: NodeId, input: &LayoutInput, out: LayoutOutput) {
        self.t.nodes[ix(id)].cache.store(input, out)
    }

    #[inline(always)]
    fn cache_clear(&mut self, id: NodeId) {
        self.t.nodes[ix(id)].cache.clear();
    }
}

impl<C, M> LayoutBlockContainer for View<'_, C, M>
where
    M: FnMut(Size<Option<f32>>, Size<AvailableSpace>, &C) -> Size<f32>,
{
    type BlockContainerStyle<'a> = &'a Style where Self: 'a;
    type BlockItemStyle<'a> = &'a Style where Self: 'a;

    #[inline(always)]
    fn get_block_container_style(&self, id: NodeId) -> Self::BlockContainerStyle<'_> {
        &self.t.nodes[ix(id)].style
    }

    #[inline(always)]
    fn get_block_child_style(&self, id: NodeId) -> Self::BlockItemStyle<'_> {
        &self.t.nodes[ix(id)].style
    }

    #[inline(always)]
    fn compute_block_child_layout(&mut self, id: NodeId, inputs: LayoutInput,
                                  block: Option<&mut BlockContext<'_>>) -> LayoutOutput {
        self.child_layout(id, inputs, block)
    }
}

impl<C, M> LayoutFlexboxContainer for View<'_, C, M>
where
    M: FnMut(Size<Option<f32>>, Size<AvailableSpace>, &C) -> Size<f32>,
{
    type FlexboxContainerStyle<'a> = &'a Style where Self: 'a;
    type FlexboxItemStyle<'a> = &'a Style where Self: 'a;

    #[inline(always)]
    fn get_flexbox_container_style(&self, id: NodeId) -> Self::FlexboxContainerStyle<'_> {
        &self.t.nodes[ix(id)].style
    }

    #[inline(always)]
    fn get_flexbox_child_style(&self, id: NodeId) -> Self::FlexboxItemStyle<'_> {
        &self.t.nodes[ix(id)].style
    }
}

impl<C, M> LayoutGridContainer for View<'_, C, M>
where
    M: FnMut(Size<Option<f32>>, Size<AvailableSpace>, &C) -> Size<f32>,
{
    type GridContainerStyle<'a> = &'a Style where Self: 'a;
    type GridItemStyle<'a> = &'a Style where Self: 'a;

    #[inline(always)]
    fn get_grid_container_style(&self, id: NodeId) -> Self::GridContainerStyle<'_> {
        &self.t.nodes[ix(id)].style
    }

    #[inline(always)]
    fn get_grid_child_style(&self, id: NodeId) -> Self::GridItemStyle<'_> {
        &self.t.nodes[ix(id)].style
    }

    #[inline(always)]
    fn set_detailed_grid_info(&mut self, id: NodeId, info: DetailedGridInfo) {
        self.t.nodes[ix(id)].detail = DetailedLayoutInfo::Grid(Box::new(info));
    }
}

impl<C, M> RoundTree for View<'_, C, M>
where
    M: FnMut(Size<Option<f32>>, Size<AvailableSpace>, &C) -> Size<f32>,
{
    #[inline(always)]
    fn get_unrounded_layout(&self, id: NodeId) -> Layout {
        self.t.nodes[ix(id)].unrounded
    }

    #[inline(always)]
    fn set_final_layout(&mut self, id: NodeId, layout: &Layout) {
        self.t.nodes[ix(id)].layout = *layout;
    }

    #[inline(always)]
    fn is_out_of_flow(&self, id: NodeId) -> bool {
        let s = &self.t.nodes[ix(id)].style;
        s.position.is_out_of_flow() && s.display != Display::None
    }

    #[inline(always)]
    fn hoisted_child_count(&self, id: NodeId) -> usize {
        self.t.nodes[ix(id)].hoisted.len()
    }

    #[inline(always)]
    fn get_hoisted_child_id(&self, id: NodeId, k: usize) -> NodeId {
        self.t.nodes[ix(id)].hoisted[k]
    }
}
