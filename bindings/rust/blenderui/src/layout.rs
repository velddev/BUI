//! From an element tree to positioned boxes: cascade, box tree, taffy layout.
//!
//! * Block containers with inline content become wrapping flex lines of word
//!   boxes (so text, links and inline-blocks wrap like a browser's line
//!   boxes); inline elements own their words for painting and hit testing.
//! * Flex and grid containers get text runs as measured, wrapping leaves.
//! * Tables map onto flex rows and columns, lists paint their markers.
//!
//! Everything lives in flat arenas owned by the window and reused every
//! frame: boxes and element data in [`Frame`], computed styles in
//! [`StyleStore`] (shared between identical elements), the taffy tree, text
//! widths and images. Boxes refer to each other by index; nothing is
//! reference-counted.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use taffy::prelude::*;
use taffy::style::Overflow;

use crate::css::{Decl, Prop, Rule, Stylesheet, Subject};
use crate::element::{AnyElement, Div, Listeners, PaintFn, ParentElement, SharedString};
use crate::geometry::{rect, Bounds, Units};
use crate::html::{tag_kind, ua_styled_stylesheet, ua_stylesheet, TagKind};
use crate::paint::{Image, ImageCache, ImageFile};
use crate::prof::{span, Phase};
use crate::style::ContentPart;
use crate::style::{apply, Computed, DisplayKind, Fnv, FnvMap, TextAlign, TextTransform, VAlign, WhiteSpace};
use crate::text::{self, TextCache};
use crate::widgets::{WKind, WidgetSpec};

// ==========================================================================
// Keys

/// An element's identity across frames: a hash of its `.id()`, or of its
/// parent's key, tag and position.
pub(crate) type Key = u64;

fn fnv(parts: &[&[u8]]) -> u64 {
    let mut h = Fnv::default();
    for p in parts {
        h.write(p);
        h.write(&[0xff]);
    }
    h.finish()
}

/// The key of the element with `.id(id)`.
pub(crate) fn id_key(id: &str) -> Key {
    fnv(&[b"#", id.as_bytes()])
}

fn child_key(parent: Key, tag: &str, n: usize) -> Key {
    fnv(&[&parent.to_le_bytes(), tag.as_bytes(), &(n as u64).to_le_bytes()])
}

/// The key of the `<html>` element.
pub(crate) const ROOT_KEY: Key = 0x68746d6c; // "html"

/// Interaction state the cascade reads (`:hover`, `:active`, `:focus`, `details[open]`).
#[derive(Default)]
pub(crate) struct UiState {
    pub hovered: HashSet<Key>,
    pub active: HashSet<Key>,
    pub focused: Option<Key>,
    /// The focused element and its ancestors (`:focus-within`).
    pub focus_path: HashSet<Key>,
    /// The `.id()` of the focused element.
    pub focused_id: Option<String>,
    pub details_open: HashMap<Key, bool>,
    pub scroll: HashMap<Key, (f32, f32)>,
    pub checked: HashMap<Key, bool>,
}

// ==========================================================================
// Styles

/// A computed style in a [`StyleStore`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct StyleId(u32);

/// The style everything inherits from at the root.
pub(crate) const ROOT_STYLE: StyleId = StyleId(0);

/// What a computed style depends on (style sharing).
struct StyleKey {
    parent: StyleId,
    viewport: (u32, u32),
    author: u64,
    rules: Vec<usize>,
    decls: Vec<Decl>,
    fixup: u8,
}

/// Computed styles, shared by every element with the same inputs and kept
/// across frames (browsers call this style sharing). Elements refer to them
/// by [`StyleId`].
pub(crate) struct StyleStore {
    styles: Vec<Computed>,
    cache: FnvMap<u64, Vec<(StyleKey, StyleId)>>,
    /// Custom property maps the styles point into (`Computed::vars`).
    vars: crate::vars::Vars,
    /// The author sheet whose `@property` initial values seed the root
    /// style's variables.
    seeded: u64,
}

impl Default for StyleStore {
    fn default() -> Self {
        StyleStore { styles: vec![Computed::default()], cache: Default::default(), vars: Default::default(), seeded: 0 }
    }
}

impl std::ops::Index<StyleId> for StyleStore {
    type Output = Computed;
    fn index(&self, id: StyleId) -> &Computed {
        &self.styles[id.0 as usize]
    }
}

impl StyleStore {
    /// Before building a frame: drop everything when the store has grown
    /// large (ids from the previous frame are not used after this).
    fn begin_frame(&mut self) {
        if self.styles.len() > 20_000 {
            self.styles.truncate(1);
            self.cache.clear();
            self.vars.clear();
            self.seeded = 0;
        }
    }

    /// `@property --x { initial-value: .. }`: registered properties hold
    /// their initial value until something sets them (`var(--x)` finds it
    /// without a fallback). Seeded into the root style once per sheet.
    fn seed_properties(&mut self, sheet: &Stylesheet, hash: u64) {
        if self.seeded == hash {
            return;
        }
        self.seeded = hash;
        let mut map: Vec<(Prop, Box<[crate::css::Tok]>)> = sheet.properties.iter()
            .filter(|p| p.name.starts_with("--"))
            .filter_map(|p| Some((Prop::custom(&p.name), crate::css::tokenize(p.initial.as_deref()?).into_boxed_slice())))
            .collect();
        map.sort_by_key(|e| e.0);
        map.dedup_by_key(|e| e.0);
        // styles computed against the old root variables are stale
        self.styles.truncate(1);
        self.cache.clear();
        self.vars.clear();
        self.styles[0].vars = if map.is_empty() { 0 } else { self.vars.add(map.into_boxed_slice()) };
    }

    fn add(&mut self, c: Computed) -> StyleId {
        self.styles.push(c);
        StyleId((self.styles.len() - 1) as u32)
    }
}

/// The author stylesheet (all `<style>` elements), reparsed only when its text changes.
/// Tailwind classes seen so far, compiled to CSS (like Tailwind's JIT):
/// each distinct `class` attribute is scanned once.
#[derive(Default)]
pub(crate) struct TwJit {
    on: bool,
    seen: FnvMap<u64, ()>,
    known: FnvMap<String, ()>,
    rules: Vec<((u32, u32), usize, String)>,
    css: String,
    dirty: bool,
}

/// Whether class names compile as Tailwind utilities (on by default).
pub(crate) static TAILWIND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

impl TwJit {
    fn scan(&mut self, classes: &str) {
        let h = fnv(&[classes.as_bytes()]);
        if self.seen.insert(h, ()).is_some() {
            return;
        }
        for c in classes.split_whitespace() {
            if self.known.contains_key(c) {
                continue;
            }
            self.known.insert(c.to_string(), ());
            if let Some(r) = crate::tailwind::compile(c) {
                let n = self.rules.len();
                self.rules.push((r.sort, n, r.css));
                self.dirty = true;
            }
        }
    }

    /// The utilities, in Tailwind's order.
    fn css(&mut self) -> &str {
        if self.dirty {
            self.rules.sort_by_key(|r| (r.0, r.1));
            self.css.clear();
            for r in &self.rules {
                self.css.push_str(&r.2);
                self.css.push('\n');
            }
            self.dirty = false;
        }
        &self.css
    }
}

#[derive(Default)]
pub(crate) struct AuthorSheet {
    hash: u64,
    sheet: Stylesheet,
}

// ==========================================================================
// Boxes

/// What a box holds.
pub(crate) enum Content {
    Element,
    /// A word (or pre-formatted line) inside a line container: bytes
    /// `start..end` of the text of box `src`.
    Word { src: u32, start: u32, end: u32 },
    /// A whole text run in a flex/grid container (wraps when measured).
    Text(String),
    LineBreak,
    Widget(WidgetSpec),
    Replaced(Replaced),
    /// An anonymous line container (inline content between blocks).
    Anon,
}

pub(crate) enum Replaced {
    Image(Arc<Image>),
    /// `<img src>`: decoded on the kit's workers when shown; animations
    /// play, or hold the frame of the `frame` attribute.
    File(ImageFile, Option<u32>),
    /// `<video>`: its player (yours, or the window's for `src`).
    Video(std::rc::Rc<crate::video::Video>),
    Painted(PaintFn),
    Placeholder(&'static str),
}

/// The element-level data kept from a `Div` after building (moved, not copied).
pub(crate) struct ElData {
    pub tag: &'static str,
    pub id: Option<SharedString>,
    pub attrs: Vec<(SharedString, SharedString)>,
    pub listeners: Option<Box<Listeners>>,
    /// Which of the author sheet's `:has()` hold here (bit = index).
    pub has: u64,
}

const NONE: u32 = u32::MAX;

pub(crate) struct BoxNode {
    pub parent: Option<usize>,
    /// Element and text children, linked (words / breaks / line boxes point
    /// at their parent but aren't linked into its list).
    first: u32,
    last: u32,
    next: u32,
    prev: u32,
    pub content: Content,
    /// Index into [`Frame::els`].
    el: u32,
    /// The element this box belongs to (itself, or the inline element of a word).
    pub owner: usize,
    pub style: StyleId,
    pub key: Key,
    /// What selectors can see of this element (its tag, id, attributes,
    /// position and state, chained through its parent and previous sibling):
    /// equal fingerprints match the same rules.
    fp: u64,
    pub taffy: Option<NodeId>,
    pub pos: (usize, usize),
    /// Position among siblings of the same tag, and their count.
    pub tpos: (u32, u32),
    pub marker: Option<String>,
    /// Inside a `styled` subtree: matched against the Blender UA sheet.
    pub styled: bool,
    // filled after layout
    pub bounds: Bounds<f32>,
    pub clip: Option<Bounds<f32>>,
    pub opacity: f32,
    pub visible: bool,
    pub scroll_range: (f32, f32),
    pub scroll_pos: (f32, f32),
    /// This element is one layout leaf measuring its lone text child (that box).
    pub fold: Option<usize>,
    /// This text run is laid out by its parent (`fold`), not by its own node.
    pub folded: bool,
}

/// A laid-out frame. The arenas are reused by the next frame.
#[derive(Default)]
pub(crate) struct Frame {
    pub nodes: Vec<BoxNode>,
    els: Vec<ElData>,
    pub title: Option<String>,
    by_key: std::cell::OnceCell<HashMap<Key, usize>>,
}

/// Measure contexts of taffy leaves. Compared across frames: an unchanged
/// context keeps the leaf's cached layout.
#[derive(PartialEq)]
pub(crate) enum Measure {
    /// Text of the box registered under `key` this frame (already
    /// whitespace-processed and transformed); `hash` identifies the text.
    Text { key: Key, hash: u64, size: f32, flags: i32, line: f32, nowrap: bool, brk: u8 },
}

/// The persistent taffy tree's nodes by box key (with the frame they were
/// last used in). Reconciling instead of rebuilding lets taffy's cache skip
/// every subtree whose style, content and children didn't change.
#[derive(Default)]
struct Recon {
    map: FnvMap<Key, (NodeId, u32)>,
    stamp: u32,
    /// Nodes for duplicate keys this frame: removed next frame.
    extra: Vec<NodeId>,
}

/// Everything layout keeps across frames, owned by the window.
pub(crate) struct LayoutCaches {
    pub styles: StyleStore,
    pub text: TextCache,
    pub images: ImageCache,
    pub author: AuthorSheet,
    pub(crate) taffy: crate::tree::LayoutTree<Measure>,
    /// Box index of each taffy node (by slot), rebuilt in place each frame.
    box_of: Vec<u32>,
    /// Scratch for `finish`.
    acc: Vec<Option<Bounds<f32>>>,
    /// Matched rules by element fingerprint (+ viewport): selector matching
    /// runs once per distinct element, not every frame.
    matched: MatchCache,
    recon: Recon,
    /// Measured text leaves: key -> box index, this frame.
    text_box: FnvMap<Key, usize>,
    /// Content widths of blocks with `overflow-wrap: anywhere` from the last
    /// layout: their long words are split to fit before layout.
    wrap_widths: FnvMap<Key, f32>,
    tw: TwJit,
    /// `@container`: query containers' content sizes from the last layout.
    containers: Containers,
    /// CSS animations and transitions by element key.
    pub(crate) anim: crate::anim::Animations,
}

impl LayoutCaches {
    /// After a layout: record query containers' sizes. True when the
    /// window must render again (an `@container` query may match differently).
    pub(crate) fn after_layout(&mut self, frame: &Frame) -> bool {
        let queried = self.author.sheet.has_container_queries();
        let mut again = self.containers.update(frame, &self.styles, queried);
        // overflow-wrap: blocks whose width changed lay out their words again
        for n in &frame.nodes {
            let c = &self.styles[n.style];
            if c.wrap_anywhere && n.taffy.is_some() && !c.display.is_inline_level()
                && matches!(n.content, Content::Element) {
                let w = crate::paint::content_box(c, n.bounds).size.width;
                if self.wrap_widths.get(&n.key).is_none_or(|old| (old - w).abs() > 0.5) {
                    self.wrap_widths.insert(n.key, w);
                    again = true;
                }
            }
        }
        if self.wrap_widths.len() > 4096 {
            self.wrap_widths.clear();
        }
        again
    }
}

/// Query containers by element key: content-box size, `container-type`
/// and names, from the previous layout (queries see last frame's sizes;
/// a change re-renders once more).
#[derive(Default)]
pub(crate) struct Containers {
    map: FnvMap<Key, (f32, f32, u8, Option<Box<str>>)>,
    /// Hash of `map`, part of the selector-match memo.
    generation: u64,
    /// Consecutive re-renders caused by container size changes.
    chain: u8,
}

impl Containers {
    /// After a layout: record the containers; true when a re-render is needed.
    fn update(&mut self, frame: &Frame, styles: &StyleStore, queried: bool) -> bool {
        let mut map: FnvMap<Key, (f32, f32, u8, Option<Box<str>>)> = FnvMap::default();
        for n in &frame.nodes {
            let c = &styles[n.style];
            if c.container_type != 0 && n.taffy.is_some() {
                let b = crate::paint::content_box(c, n.bounds);
                map.insert(n.key, (b.size.width, b.size.height, c.container_type, c.container_name.clone()));
            }
        }
        let mut h = Fnv::default();
        let mut keys: Vec<&Key> = map.keys().collect();
        keys.sort_unstable();
        for k in keys {
            let (w, hh, t, _) = &map[k];
            (k, w.to_bits(), hh.to_bits(), t).hash(&mut h);
        }
        let generation = h.finish();
        let changed = generation != self.generation;
        self.map = map;
        self.generation = generation;
        // a query may now match differently: render again (bounded, in case
        // the result feeds back into the container's own size)
        if changed && queried && self.chain < 3 {
            self.chain += 1;
            return true;
        }
        self.chain = 0;
        false
    }
}

/// Rule indices (`Rule::order`) in the UA and author sheets.
type RuleIds = smallvec::SmallVec<[u32; 8]>;

#[derive(Default)]
struct MatchCache {
    map: FnvMap<u64, (RuleIds, RuleIds)>,
    author: u64,
}

impl Default for LayoutCaches {
    fn default() -> Self {
        LayoutCaches {
            styles: StyleStore::default(),
            text: TextCache::default(),
            images: ImageCache::default(),
            author: AuthorSheet::default(),
            taffy: Default::default(),
            box_of: Vec::new(),
            acc: Vec::new(),
            matched: MatchCache::default(),
            recon: Recon::default(),
            text_box: FnvMap::default(),
            wrap_widths: FnvMap::default(),
            tw: TwJit::default(),
            containers: Containers::default(),
            anim: Default::default(),
        }
    }
}

struct Builder<'a> {
    nodes: Vec<BoxNode>,
    els: Vec<ElData>,
    taffy: &'a mut crate::tree::LayoutTree<Measure>,
    styles: &'a mut StyleStore,
    text: &'a mut TextCache,
    images: &'a mut ImageCache,
    matched: &'a mut MatchCache,
    recon: &'a mut Recon,
    text_box: &'a mut FnvMap<Key, usize>,
    /// Set when a node changed while building the current subtree.
    changed: bool,
    /// `tpos` for the element about to be built.
    next_tpos: (u32, u32),
    author: &'a Stylesheet,
    author_hash: u64,
    containers: &'a Containers,
    /// Media query results of the UA and author sheets this frame.
    ua_media: Vec<bool>,
    ua_styled_media: Vec<bool>,
    au_media: Vec<bool>,
    state: &'a UiState,
    viewport: (f32, f32),
    title: Option<String>,
    widgets: &'a mut dyn FnMut(Key, &WidgetSpec) -> (f32, f32),
    /// CSS counters in scope: name, value, and the box whose subtree the
    /// instance is scoped to (dropped when that box is finished).
    counters: Vec<(Box<str>, i32, usize)>,
    /// Column widths of each table (by box), computed before its cells.
    table_cols: FnvMap<usize, TableCols>,
    wrap_widths: &'a FnvMap<Key, f32>,
    anim: &'a mut crate::anim::Animations,
}

/// A table's columns: widths in px, and whether the table sizes to them.
struct TableCols {
    widths: Vec<f32>,
    /// the table's width is auto: it shrinks to its columns
    fit: bool,
    fixed: bool,
}

/// Child box indices / layout nodes being assembled: on the stack for the usual few.
type Kids = smallvec::SmallVec<[usize; 8]>;
type NodeList = smallvec::SmallVec<[NodeId; 8]>;

/// The slot index of a layout node (dense: the tree reuses freed slots).
fn slot(id: NodeId) -> usize {
    u64::from(id) as usize
}

/// Children of box `i` in the arena `nodes`.
fn kids(nodes: &[BoxNode], i: usize) -> impl Iterator<Item = usize> + '_ {
    let mut k = nodes[i].first;
    std::iter::from_fn(move || {
        (k != NONE).then(|| {
            let cur = k as usize;
            k = nodes[cur].next;
            cur
        })
    })
}

fn el_attr<'a>(nodes: &[BoxNode], els: &'a [ElData], i: usize, name: &str) -> Option<&'a str> {
    let e = nodes[i].el;
    if e == NONE {
        return None;
    }
    els[e as usize].attrs.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.as_ref())
}

// -- selector subject -------------------------------------------------------------------

#[derive(Clone)]
struct NodeRef<'b> {
    nodes: &'b [BoxNode],
    els: &'b [ElData],
    state: &'b UiState,
    containers: &'b Containers,
    i: usize,
}

impl NodeRef<'_> {
    fn el(&self) -> Option<&ElData> {
        let e = self.nodes[self.i].el;
        (e != NONE).then(|| &self.els[e as usize])
    }
    fn own_attr(&self, name: &str) -> Option<&str> {
        el_attr(self.nodes, self.els, self.i, name)
    }
    /// A `::before` / `::after` box (invisible to sibling selectors).
    fn is_generated(&self, k: usize) -> bool {
        let e = self.nodes[k].el;
        e != NONE && self.els[e as usize].tag.starts_with("::")
    }
}

impl Subject for NodeRef<'_> {
    fn container(&self, name: Option<&str>) -> Option<(f32, f32)> {
        let mut cur = self.nodes[self.i].parent;
        while let Some(p) = cur {
            if let Some((w, h, ty, names)) = self.containers.map.get(&self.nodes[p].key) {
                let named = name.is_none_or(|n| names.as_deref().is_some_and(|s| s.split(' ').any(|x| x == n)));
                if named {
                    // an inline-size container has no queryable height
                    return Some((*w, if *ty == 2 { *h } else { f32::NAN }));
                }
            }
            cur = self.nodes[p].parent;
        }
        None
    }
    fn tag(&self) -> &str {
        self.el().map(|e| e.tag).unwrap_or("")
    }
    fn id(&self) -> Option<&str> {
        self.el()?.id.as_deref()
    }
    fn has_class(&self, class: &str) -> bool {
        self.own_attr("class").is_some_and(|c| c.split_whitespace().any(|w| w == class))
    }
    fn attr(&self, name: &str) -> Option<&str> {
        let n = &self.nodes[self.i];
        if name == "open" && self.tag() == "details" {
            return self.state.details_open.get(&n.key).copied()
                .unwrap_or(self.own_attr("open").is_some()).then_some("");
        }
        self.own_attr(name)
    }
    fn state(&self, pseudo: &str) -> bool {
        let key = self.nodes[self.i].key;
        match pseudo {
            "hover" => self.state.hovered.contains(&key),
            "active" => self.state.active.contains(&key),
            "focus" | "focus-visible" => self.state.focused == Some(key),
            "focus-within" => self.state.focus_path.contains(&key),
            "checked" => self.state.checked.get(&key).copied().unwrap_or(self.own_attr("checked").is_some()),
            "disabled" => self.own_attr("disabled").is_some(),
            "enabled" => self.own_attr("disabled").is_none(),
            "link" | "any-link" => self.tag() == "a" && self.own_attr("href").is_some(),
            "required" => self.own_attr("required").is_some(),
            "optional" => self.own_attr("required").is_none(),
            "read-only" => self.own_attr("readonly").is_some(),
            "read-write" => self.own_attr("readonly").is_none(),
            "open" => self.attr("open").is_some(),
            "placeholder-shown" => {
                self.own_attr("placeholder").is_some() && self.own_attr("value").is_none_or(str::is_empty)
            }
            _ => false,
        }
    }
    fn parent(&self) -> Option<Self> {
        self.nodes[self.i].parent.map(|p| NodeRef { i: p, ..self.clone() })
    }
    fn prev_sibling(&self) -> Option<Self> {
        let mut k = self.nodes[self.i].prev;
        while k != NONE {
            if self.nodes[k as usize].el != NONE && !self.is_generated(k as usize) {
                return Some(NodeRef { i: k as usize, ..self.clone() });
            }
            k = self.nodes[k as usize].prev;
        }
        None
    }
    fn position(&self) -> (usize, usize) {
        self.nodes[self.i].pos
    }
    fn type_position(&self) -> (usize, usize) {
        let t = self.nodes[self.i].tpos;
        (t.0 as usize, t.1 as usize)
    }
    fn is_empty(&self) -> bool {
        self.nodes[self.i].first == NONE
    }
    fn has(&self, id: u16) -> bool {
        id < 64 && self.el().is_some_and(|e| e.has & (1 << id) != 0)
    }
    fn same(&self, o: &Self) -> bool {
        self.i == o.i
    }
    fn next_sibling(&self) -> Option<Self> {
        let mut k = self.nodes[self.i].next;
        while k != NONE {
            if self.nodes[k as usize].el != NONE && !self.is_generated(k as usize) {
                return Some(NodeRef { i: k as usize, ..self.clone() });
            }
            k = self.nodes[k as usize].next;
        }
        None
    }
    fn first_child(&self) -> Option<Self> {
        let k = self.nodes[self.i].first;
        if k == NONE {
            return None;
        }
        let c = NodeRef { i: k as usize, ..self.clone() };
        if self.nodes[k as usize].el != NONE && !self.is_generated(k as usize) { Some(c) } else { c.next_sibling() }
    }
}

/// An element of a not-yet-built subtree (for `:has()`): its tag, id,
/// attributes and tree links, flattened from the declarative children.
struct Flat<'d> {
    tag: &'static str,
    id: Option<&'d str>,
    attrs: &'d [(SharedString, SharedString)],
    parent: u32,
    first: u32,
    next: u32,
    prev: u32,
    pos: (usize, usize),
    tpos: (usize, usize),
    key: Key,
}

/// (same-tag siblings before `k`, same-tag siblings in all) for `:has()`
/// subtrees (small: linear scans are fine there).
fn type_counts(els: &[&Div], k: usize) -> (usize, usize) {
    let tag = els[k].tag;
    let before = els[..k].iter().filter(|e| e.tag == tag).count();
    (before, before + els[k..].iter().filter(|e| e.tag == tag).count())
}

/// Flatten `div`'s element subtree (index 0 = `div` itself, the anchor).
fn flatten<'d>(div: &'d Div, key: Key, out: &mut Vec<Flat<'d>>) {
    out.push(Flat { tag: div.tag, id: None, attrs: &[], parent: NONE, first: NONE, next: NONE, prev: NONE,
                    pos: (1, 1), tpos: (1, 1), key });
    fn kids<'d>(div: &'d Div, me: u32, key: Key, out: &mut Vec<Flat<'d>>) {
        let els: smallvec::SmallVec<[&'d Div; 16]> = div.children.iter().filter_map(|c| match c {
            AnyElement::Element(d) => Some(d),
            _ => None,
        }).collect();
        let mut prev = NONE;
        for (k, d) in els.iter().enumerate() {
            let ck = match &d.id {
                Some(id) => id_key(id),
                None => child_key(key, d.tag, k + 1),
            };
            let (before, total) = type_counts(&els, k);
            let idx = out.len() as u32;
            out.push(Flat { tag: d.tag, id: d.id.as_deref(), attrs: &d.attrs, parent: me, first: NONE, next: NONE,
                            prev, pos: (k + 1, els.len()), tpos: (before + 1, total), key: ck });
            if prev == NONE {
                out[me as usize].first = idx;
            } else {
                out[prev as usize].next = idx;
            }
            prev = idx;
            kids(d, idx, ck, out);
        }
    }
    kids(div, 0, key, out);
}

#[derive(Clone)]
struct DivSubject<'f, 'd> {
    flat: &'f [Flat<'d>],
    state: &'f UiState,
    i: usize,
}

impl DivSubject<'_, '_> {
    fn at(&self, k: u32) -> Option<Self> {
        (k != NONE).then(|| DivSubject { i: k as usize, ..self.clone() })
    }
}

impl Subject for DivSubject<'_, '_> {
    fn tag(&self) -> &str {
        self.flat[self.i].tag
    }
    fn id(&self) -> Option<&str> {
        self.flat[self.i].id
    }
    fn has_class(&self, class: &str) -> bool {
        self.attr("class").is_some_and(|c| c.split_whitespace().any(|w| w == class))
    }
    fn attr(&self, name: &str) -> Option<&str> {
        self.flat[self.i].attrs.iter().find(|(k, _)| k.as_ref() == name).map(|(_, v)| v.as_ref())
    }
    fn state(&self, pseudo: &str) -> bool {
        let key = self.flat[self.i].key;
        match pseudo {
            "hover" => self.state.hovered.contains(&key),
            "active" => self.state.active.contains(&key),
            "focus" | "focus-visible" => self.state.focused == Some(key),
            "focus-within" => self.state.focus_path.contains(&key),
            "checked" => self.state.checked.get(&key).copied().unwrap_or(self.attr("checked").is_some()),
            "disabled" => self.attr("disabled").is_some(),
            "enabled" => self.attr("disabled").is_none(),
            "required" => self.attr("required").is_some(),
            "link" | "any-link" => self.tag() == "a" && self.attr("href").is_some(),
            _ => false,
        }
    }
    fn parent(&self) -> Option<Self> {
        self.at(self.flat[self.i].parent)
    }
    fn prev_sibling(&self) -> Option<Self> {
        self.at(self.flat[self.i].prev)
    }
    fn next_sibling(&self) -> Option<Self> {
        self.at(self.flat[self.i].next)
    }
    fn first_child(&self) -> Option<Self> {
        self.at(self.flat[self.i].first)
    }
    fn position(&self) -> (usize, usize) {
        self.flat[self.i].pos
    }
    fn type_position(&self) -> (usize, usize) {
        self.flat[self.i].tpos
    }
    fn is_empty(&self) -> bool {
        self.flat[self.i].first == NONE
    }
    fn same(&self, o: &Self) -> bool {
        self.i == o.i
    }
}

// -- building -------------------------------------------------------------------------------

fn collect_styles(el: &AnyElement, css: &mut String, title: &mut Option<String>, jit: &mut TwJit) {
    if let AnyElement::Element(d) = el {
        if jit.on {
            if let Some((_, classes)) = d.attrs.iter().find(|(k, _)| k.as_ref() == "class") {
                jit.scan(classes);
            }
        }
        let text = || d.children.iter().filter_map(|c| match c {
            AnyElement::Text(t) => Some(t.as_ref()),
            _ => None,
        }).collect::<String>();
        match d.tag {
            "style" => css.push_str(&text()),
            "title" if title.is_none() => *title = Some(text()),
            _ => {}
        }
        for c in &d.children {
            collect_styles(c, css, title, jit);
        }
    }
}

impl<'a> Builder<'a> {
    fn subject(&self, i: usize) -> NodeRef<'_> {
        NodeRef { nodes: &self.nodes, els: &self.els, state: self.state, containers: self.containers, i }
    }

    fn st(&self, i: usize) -> &Computed {
        &self.styles[self.nodes[i].style]
    }

    /// `<video src>`'s player: muted autoplay unless `autoplay="false"` (React's
    /// `autoPlay={false}`);
    /// `loop`, `poster` (the HTML attributes).
    fn video_src(&mut self, i: usize) -> Option<std::rc::Rc<crate::video::Video>> {
        let attrs = crate::video::VideoAttrs {
            autoplay: self.attr(i, "autoplay").or_else(|| self.attr(i, "autoPlay")) != Some("false"), // React: autoPlay
            looped: self.attr(i, "loop").is_some_and(|v| v != "false"),
            poster: self.attr(i, "poster").map(str::to_owned),
        };
        let src = self.attr(i, "src")?.to_owned();
        self.images.videos.get(&src, attrs)
    }

    fn attr(&self, i: usize, name: &str) -> Option<&str> {
        el_attr(&self.nodes, &self.els, i, name)
    }

    /// The computed style of element `i`, shared with every element (this
    /// frame or earlier) that had the same parent style, matched rules and
    /// declarations. `fixup` adjusts `display` for widgets and flex/grid
    /// items before the result is shared.
    fn cascade(&mut self, i: usize, div: &Div, parent: StyleId, fixup: u8) -> StyleId {
        let _s = span(Phase::Cascade);
        let (ua, au) = self.matched_rules(i);
        let decls = self.declared(i, div, &ua, &au);
        let viewport = (self.viewport.0.to_bits(), self.viewport.1.to_bits());
        let rule_ids = || ua.iter().map(|r| *r as *const Rule as usize).chain(au.iter().map(|r| r.order));
        // probe without building a key: hash the inputs, compare in place
        let hash = {
            let mut h = Fnv::default();
            parent.hash(&mut h);
            viewport.hash(&mut h);
            self.author_hash.hash(&mut h);
            fixup.hash(&mut h);
            for r in rule_ids() {
                r.hash(&mut h);
            }
            for d in &decls {
                d.probe_hash(&mut h);
            }
            h.finish()
        };
        let same = |k: &StyleKey| {
            k.parent == parent
                && k.viewport == viewport
                && k.author == self.author_hash
                && k.fixup == fixup
                && k.rules.len() == ua.len() + au.len()
                && k.rules.iter().copied().eq(rule_ids())
                && k.decls.len() == decls.len()
                && k.decls.iter().zip(&decls).all(|(a, d)| a == *d)
        };
        if let Some(&(_, id)) = self.styles.cache.get(&hash).and_then(|b| b.iter().find(|(k, _)| same(k))) {
            return id;
        }
        let key = StyleKey {
            parent,
            viewport,
            author: self.author_hash,
            rules: rule_ids().collect(),
            decls: decls.iter().map(|d| (*d).clone()).collect(),
            fixup,
        };
        let mut c = self.compute(parent, &decls);
        match fixup {
            1 if c.display == DisplayKind::Inline => c.display = DisplayKind::InlineBlock,
            2 if c.display == DisplayKind::Inline => c.display = DisplayKind::Block,
            _ => {}
        }
        let id = self.styles.add(c);
        self.styles.cache.entry(hash).or_default().push((key, id));
        id
    }

    /// CSS animations and transitions of element `i` (`anim.rs`): a private
    /// copy of its shared style `style` with the animated values on top.
    /// Never cached, so the shared styles stay what the cascade made them.
    fn animate(&mut self, i: usize, div: &Div, parent: StyleId, style: StyleId) -> StyleId {
        let key = self.nodes[i].key;
        if !self.anim.wants(key, &self.styles[style]) {
            return style;
        }
        let important: smallvec::SmallVec<[Prop; 8]> = {
            let (ua, au) = self.matched_rules(i);
            self.declared(i, div, &ua, &au).iter().filter(|d| d.important).map(|d| d.prop).collect()
        };
        let base = &self.styles[style];
        let env = crate::anim::Env {
            parent: &self.styles[parent],
            viewport: self.viewport,
            vars: self.styles.vars.get(base.vars),
            sheet: self.author,
            media: &self.au_media,
            important: &important,
        };
        match self.anim.apply(key, base, &env) {
            Some(c) => self.styles.add(c),
            None => style,
        }
    }

    /// The UA and author rules matching element `i`: from the memo when an
    /// element with the same fingerprint was matched before.
    fn matched_rules(&mut self, i: usize) -> (crate::css::Matches<'static>, crate::css::Matches<'a>) {
        let (ua_sheet, ua_media) = self.ua_for(i);
        let author: &'a Stylesheet = self.author;
        let memo = {
            let mut h = Fnv::default();
            self.nodes[i].fp.hash(&mut h);
            self.nodes[i].styled.hash(&mut h);
            self.containers.generation.hash(&mut h);
            self.viewport.0.to_bits().hash(&mut h);
            self.viewport.1.to_bits().hash(&mut h);
            h.finish()
        };
        if let Some((u, a)) = self.matched.map.get(&memo) {
            return (u.iter().map(|&k| &ua_sheet.rules[k as usize]).collect(),
                    a.iter().map(|&k| &author.rules[k as usize]).collect());
        }
        let subj = self.subject(i);
        let ua = ua_sheet.matches(&subj, ua_media, None);
        let au = author.matches(&subj, &self.au_media, None);
        self.matched.map.insert(memo, (ua.iter().map(|r| r.order as u32).collect(),
                                       au.iter().map(|r| r.order as u32).collect()));
        (ua, au)
    }

    /// The UA sheet for box `i` (plain, or Blender's inside `styled`) and
    /// its media query results.
    fn ua_for(&self, i: usize) -> (&'static Stylesheet, &Vec<bool>) {
        if self.nodes[i].styled {
            (ua_styled_stylesheet(), &self.ua_styled_media)
        } else {
            (ua_stylesheet(), &self.ua_media)
        }
    }

    /// The fingerprint of element `i` (see [`BoxNode::fp`]).
    fn fingerprint(&self, i: usize) -> u64 {
        let n = &self.nodes[i];
        let mut h = Fnv::default();
        n.parent.map(|p| self.nodes[p].fp).unwrap_or(0).hash(&mut h);
        // the previous element sibling (text in between doesn't count)
        let mut k = n.prev;
        while k != NONE && self.nodes[k as usize].el == NONE {
            k = self.nodes[k as usize].prev;
        }
        (if k == NONE { 0 } else { self.nodes[k as usize].fp }).hash(&mut h);
        n.pos.hash(&mut h);
        n.tpos.hash(&mut h);
        if let Some(e) = (n.el != NONE).then(|| &self.els[n.el as usize]) {
            e.has.hash(&mut h);
            e.tag.hash(&mut h);
            e.id.as_deref().hash(&mut h);
            for (a, v) in &e.attrs {
                a.hash(&mut h);
                v.hash(&mut h);
            }
        }
        let st = self.state;
        let key = n.key;
        (st.hovered.contains(&key), st.active.contains(&key), st.focused == Some(key),
         st.focus_path.contains(&key), st.checked.get(&key).copied(), st.details_open.get(&key).copied())
            .hash(&mut h);
        h.finish()
    }

    /// Every declaration that applies to element `i`, in cascade order.
    fn declared<'d>(&self, i: usize, div: &'d Div, ua: &[&'d Rule], au: &[&'d Rule])
        -> smallvec::SmallVec<[&'d Decl; 24]> {
        let key = self.nodes[i].key;
        let mut decls = smallvec::SmallVec::new();
        decls.extend(ua.iter().flat_map(|r| r.decls.iter()));
        decls.extend(au.iter().flat_map(|r| r.decls.iter()));
        decls.extend(div.decls.iter());
        if let Some(st) = &div.states {
            if self.state.hovered.contains(&key) {
                decls.extend(st.hover.iter());
            }
            if self.state.active.contains(&key) {
                decls.extend(st.active.iter());
            }
            if self.state.focused == Some(key) {
                decls.extend(st.focus.iter());
            }
        }
        decls
    }

    fn compute(&mut self, parent: StyleId, decls: &[&Decl]) -> Computed {
        // custom properties first: other declarations may reference them
        let pvars = self.styles[parent].vars;
        let vars = if decls.iter().any(|d| d.prop.is_custom()) {
            let mut map: Vec<(Prop, Box<[crate::css::Tok]>)> = self.styles.vars.get(pvars).to_vec();
            for important in [false, true] {
                for d in decls.iter().filter(|d| d.important == important && d.prop.is_custom()) {
                    let v: Box<[crate::css::Tok]> = if crate::vars::uses_vars(&d.value) {
                        match crate::vars::substitute(&d.value, &map) {
                            Some(v) => v.into_boxed_slice(),
                            None => Box::new([]), // guaranteed-invalid: falls back
                        }
                    } else {
                        d.value.to_vec().into_boxed_slice()
                    };
                    match map.binary_search_by_key(&d.prop, |e| e.0) {
                        Ok(i) => map[i].1 = v,
                        Err(i) => map.insert(i, (d.prop, v)),
                    }
                }
            }
            self.styles.vars.add(map.into_boxed_slice())
        } else {
            pvars
        };
        let parent = &self.styles[parent];
        let mut c = Computed::inherit(parent);
        c.vars = vars;
        let map = self.styles.vars.get(vars);
        let subst = |d: &Decl| -> Option<Decl> {
            crate::vars::substitute(&d.value, map).map(|v| Decl {
                prop: d.prop,
                value: crate::css::Value::Owned(v.into_boxed_slice()),
                important: d.important,
            })
        };
        const FONT_SIZE: Prop = Prop::named("font-size");
        const FONT: Prop = Prop::named("font");
        let font_first = |d: &&Decl| d.prop == FONT_SIZE || d.prop == FONT;
        let units = |fs: f32| Units { font_size: fs, viewport: self.viewport };
        let run = |c: &mut Computed, u: Units, d: &Decl| {
            if crate::vars::uses_vars(&d.value) {
                if let Some(d) = subst(d) {
                    apply(c, parent, u, &d);
                }
            } else {
                apply(c, parent, u, d);
            }
        };
        for important in [false, true] {
            // font-size first: em lengths in the other properties depend on it
            for d in decls.iter().filter(|d| d.important == important && font_first(d)) {
                run(&mut c, units(parent.font_size), d);
            }
        }
        let u = units(c.font_size);
        for important in [false, true] {
            for d in decls.iter().filter(|d| d.important == important && !font_first(d)) {
                run(&mut c, u, d);
            }
        }
        c
    }

    fn units(&self, font_size: f32) -> Units {
        Units { font_size, viewport: self.viewport }
    }

    /// A new box. `link`: append it to its parent's child list (elements and
    /// text); words, breaks and line boxes only point at their parent.
    fn push(&mut self, parent: Option<usize>, content: Content, owner: Option<usize>, style: StyleId,
            key: Key, link: bool) -> usize {
        let i = self.nodes.len();
        let prev = match parent {
            Some(p) if link => self.nodes[p].last,
            _ => NONE,
        };
        self.nodes.push(BoxNode {
            parent,
            first: NONE,
            last: NONE,
            next: NONE,
            prev,
            content,
            el: NONE,
            owner: owner.unwrap_or(i),
            style,
            key,
            fp: 0,
            taffy: None,
            pos: (1, 1),
            tpos: (1, 1),
            marker: None,
            styled: false,
            bounds: rect(0.0, 0.0, 0.0, 0.0),
            clip: None,
            opacity: 1.0,
            visible: true,
            scroll_range: (0.0, 0.0),
            scroll_pos: (0.0, 0.0),
            fold: None,
            folded: false,
        });
        if let (Some(p), true) = (parent, link) {
            if prev == NONE {
                self.nodes[p].first = i as u32;
            } else {
                self.nodes[prev as usize].next = i as u32;
            }
            self.nodes[p].last = i as u32;
        }
        i
    }

    /// Remove the last box (an element that turned out not to render).
    fn pop(&mut self) {
        let n = self.nodes.pop().expect("a box to pop");
        if let Some(p) = n.parent {
            let i = self.nodes.len() as u32;
            if self.nodes[p].last == i {
                self.nodes[p].last = n.prev;
                if n.prev == NONE {
                    self.nodes[p].first = NONE;
                } else {
                    self.nodes[n.prev as usize].next = NONE;
                }
            }
        }
        if n.el != NONE && n.el as usize == self.els.len() - 1 {
            self.els.pop();
        }
    }

    /// Build an element; returns its index (None when not rendered).
    fn element(&mut self, mut div: Div, parent: Option<usize>, key: Key, pos: (usize, usize)) -> Option<usize> {
        let parent_style = parent.map(|p| self.nodes[p].style).unwrap_or(ROOT_STYLE);
        let i = self.push(parent, Content::Element, None, parent_style, key, true);
        self.nodes[i].pos = pos;
        self.nodes[i].tpos = std::mem::replace(&mut self.next_tpos, (1, 1));
        self.nodes[i].el = self.els.len() as u32;
        self.els.push(ElData {
            tag: div.tag,
            id: div.id.take(),
            attrs: std::mem::take(&mut div.attrs),
            listeners: std::mem::take(&mut div.listeners),
            has: 0,
        });
        if !self.author.has.is_empty() {
            // :has() looks at the element's subtree, which isn't built yet:
            // evaluate it on the declarative children instead
            let mut bits = 0u64;
            let mut flat = Vec::new();
            for (id, entry) in self.author.has.iter().enumerate().take(64) {
                if !entry.host_matches(&self.subject(i)) {
                    continue;
                }
                if flat.is_empty() {
                    flatten(&div, key, &mut flat);
                }
                let anchor = DivSubject { flat: &flat, state: self.state, i: 0 };
                if crate::css::has_match(&entry.rels, &anchor) {
                    bits |= 1 << id;
                }
            }
            if let Some(e) = self.els.last_mut() {
                e.has = bits;
            }
        }
        self.nodes[i].fp = self.fingerprint(i);
        // `styled` (any value but "false") turns Blender's theme on for the subtree
        self.nodes[i].styled = match self.els.last().and_then(|e| e.attrs.iter().find(|(n, _)| n == "styled")) {
            Some((_, v)) => v.as_ref() != "false",
            None => parent.is_some_and(|p| self.nodes[p].styled),
        };
        let kind = tag_kind(div.tag);
        // widgets and replaced elements are atomic inlines; flex/grid items are blockified
        let fixup = if kind == TagKind::Widget || kind == TagKind::Replaced {
            1
        } else if parent.is_some_and(|p| matches!(self.st(p).display,
                                                  DisplayKind::Flex | DisplayKind::InlineFlex
                                                  | DisplayKind::Grid | DisplayKind::InlineGrid)) {
            2
        } else {
            0
        };
        let style = self.cascade(i, &div, parent_style, fixup);
        if self.styles[style].display == DisplayKind::None || kind == TagKind::Metadata {
            self.pop();
            return None;
        }
        let style = self.animate(i, &div, parent_style, style);
        self.nodes[i].style = style;
        self.counter_ops(style, parent.unwrap_or(usize::MAX));
        // attributes moved into ElData: widgets read them back through the arena
        div.attrs = Vec::new();
        match kind {
            TagKind::Widget => {
                let spec = WidgetSpec::from_element(&div, self.els.last().map(|e| &e.attrs[..]).unwrap_or(&[]),
                                                    key, self.state);
                if let Some(mut spec) = spec {
                    // browser-look controls in plain HTML; Blender widgets inside
                    // `styled` and for the Blender components (`bui-blender`)
                    spec.html = !self.nodes[i].styled && self.attr(i, "bui-blender").is_none();
                    // a Blender text box draws CSS `font-size` (the styled sheet's 11px
                    // is the kit's own size)
                    if self.nodes[i].styled && matches!(spec.kind, WKind::Text { .. } | WKind::TextArea) {
                        spec.font_scale = self.styles[style].font_size / 11.0;
                    }
                    self.nodes[i].content = Content::Widget(spec);
                } else {
                    self.children(i, div.children);
                }
            }
            TagKind::Replaced => {
                let r = if let Some(f) = div.paint.take() {
                    Replaced::Painted(f)
                } else if let Some(img) = div.image.take() {
                    Replaced::Image(img)
                } else if let Some(v) = div.video.take() {
                    Replaced::Video(v)
                } else if div.tag == "video" && let Some(v) = self.video_src(i) {
                    Replaced::Video(v)
                } else if let Some(file) = self.attr(i, "src").map(str::to_owned).and_then(|s| self.images.load(&s)) {
                    Replaced::File(file, self.attr(i, "frame").and_then(|v| v.trim().parse().ok()))
                } else {
                    Replaced::Placeholder(div.tag)
                };
                self.nodes[i].content = Content::Replaced(r);
            }
            TagKind::Break => {
                self.nodes[i].content = Content::LineBreak;
            }
            TagKind::Box | TagKind::Metadata => {
                let mut children = div.children;
                if div.tag == "details" {
                    let open = self.subject(i).attr("open").is_some();
                    if !open {
                        children.retain(|c| matches!(c, AnyElement::Element(d) if d.tag == "summary"));
                    }
                }
                if div.tag == "q" {
                    let (open, close) = self.st(i).quotes.clone()
                        .unwrap_or_else(|| ("\u{201c}".into(), "\u{201d}".into()));
                    children.insert(0, AnyElement::Text(open.into()));
                    children.push(AnyElement::Text(close.into()));
                }
                let pseudo = self.author.uses_pseudo() || self.ua_for(i).0.uses_pseudo();
                if pseudo {
                    self.generated(i, crate::css::PseudoElement::Before);
                }
                self.children(i, children);
                if pseudo {
                    self.generated(i, crate::css::PseudoElement::After);
                }
            }
        }
        // counters this element's children created go out of scope
        while self.counters.last().is_some_and(|c| c.2 == i) {
            self.counters.pop();
        }
        Some(i)
    }

    /// Apply `counter-reset`, then `-increment`, then `-set` of style `sid`; new counters are scoped to `scope` (the parent: an element's
    /// counter is seen by its following siblings).
    fn counter_ops(&mut self, sid: StyleId, scope: usize) {
        let Some(ops) = self.styles[sid].counters.clone() else { return };
        for (name, v) in &ops.reset {
            // a sibling's reset replaces the instance a previous sibling made
            let same_scope = self.counters.iter_mut().rev().take_while(|c| c.2 == scope).find(|c| c.0 == *name);
            match same_scope {
                Some(c) => c.1 = *v,
                None => self.counters.push((name.clone(), *v, scope)),
            }
        }
        let find = |b: &mut Self, name: &str| -> usize {
            match b.counters.iter().rposition(|c| &*c.0 == name) {
                Some(k) => k,
                None => {
                    // using a counter that doesn't exist instantiates it here
                    b.counters.push((name.into(), 0, scope));
                    b.counters.len() - 1
                }
            }
        };
        for (name, v) in &ops.increment {
            let k = find(self, name);
            self.counters[k].1 = self.counters[k].1.wrapping_add(*v);
        }
        for (name, v) in &ops.set {
            let k = find(self, name);
            self.counters[k].1 = *v;
        }
    }

    /// The `::before` / `::after` box of element `i`, when a rule gives it content.
    fn generated(&mut self, i: usize, pe: crate::css::PseudoElement) {
        let (ua_sheet, ua_media) = self.ua_for(i);
        let author: &'a Stylesheet = self.author;
        let (ua, au) = {
            let subj = self.subject(i);
            (ua_sheet.matches(&subj, ua_media, Some(pe)), author.matches(&subj, &self.au_media, Some(pe)))
        };
        if ua.is_empty() && au.is_empty() {
            return;
        }
        let parent = self.nodes[i].style;
        let tag = if pe == crate::css::PseudoElement::Before { "::before" } else { "::after" };
        // shared like element styles: same parent + same rules = same style
        let hash = {
            let mut h = Fnv::default();
            (parent, tag, self.author_hash).hash(&mut h);
            for r in ua.iter().chain(au.iter()) {
                (*r as *const Rule as usize).hash(&mut h);
            }
            h.finish()
        };
        let rule_ids: Vec<usize> = ua.iter().chain(au.iter()).map(|r| *r as *const Rule as usize).collect();
        let fixup = if pe == crate::css::PseudoElement::Before { 3 } else { 4 };
        let viewport = (self.viewport.0.to_bits(), self.viewport.1.to_bits());
        let found = self.styles.cache.get(&hash).and_then(|b| b.iter().find(|(k, _)| {
            k.parent == parent && k.fixup == fixup && k.viewport == viewport && k.author == self.author_hash
                && k.rules == rule_ids
        })).map(|(_, id)| *id);
        let sid = match found {
            Some(id) => id,
            None => {
                let decls: smallvec::SmallVec<[&Decl; 24]> =
                    ua.iter().chain(au.iter()).flat_map(|r| r.decls.iter()).collect();
                let mut c = self.compute(parent, &decls);
                if matches!(self.st(i).display, DisplayKind::Flex | DisplayKind::InlineFlex
                            | DisplayKind::Grid | DisplayKind::InlineGrid) && c.display == DisplayKind::Inline {
                    c.display = DisplayKind::Block; // flex / grid items are blockified
                }
                let id = self.styles.add(c);
                let key = StyleKey { parent, viewport, author: self.author_hash, rules: rule_ids, decls: Vec::new(), fixup };
                self.styles.cache.entry(hash).or_default().push((key, id));
                id
            }
        };
        let st = &self.styles[sid];
        if st.content.is_none() || st.display == DisplayKind::None {
            return;
        }
        self.counter_ops(sid, i);
        let st = &self.styles[sid];
        let Some(parts) = st.content.as_ref() else { return };
        let quotes = st.quotes.clone().unwrap_or_else(|| ("\u{201c}".into(), "\u{201d}".into()));
        let mut text = String::new();
        for p in parts.iter() {
            match p {
                ContentPart::Text(t) => text.push_str(t),
                ContentPart::Attr(n) => text.push_str(self.attr(i, n).unwrap_or("")),
                ContentPart::OpenQuote => text.push_str(&quotes.0),
                ContentPart::CloseQuote => text.push_str(&quotes.1),
                ContentPart::Counter(name, ls) => {
                    let v = self.counters.iter().rev().find(|c| c.0 == *name).map_or(0, |c| c.1);
                    text.push_str(&ls.format(v));
                }
                ContentPart::Counters(name, sep, ls) => {
                    let all: Vec<String> = self.counters.iter().filter(|c| c.0 == *name)
                        .map(|c| ls.format(c.1)).collect();
                    text.push_str(&if all.is_empty() { ls.format(0) } else { all.join(sep) });
                }
            }
        }
        let key = child_key(self.nodes[i].key, tag, 0);
        let g = self.push(Some(i), Content::Element, None, sid, key, true);
        self.nodes[g].el = self.els.len() as u32;
        self.els.push(ElData { tag, id: None, attrs: Vec::new(), listeners: None, has: 0 });
        if !text.is_empty() {
            self.push(Some(g), Content::Text(text), Some(g), sid, child_key(key, "#text", 1), true);
        }
    }

    /// Children of element `i`: elements and text, keyed by position.
    fn children(&mut self, i: usize, mut children: Vec<AnyElement>) {
        // list markers: painted outside the box, or inline text when `inside`
        if self.st(i).display == DisplayKind::ListItem {
            let ls = self.st(i).list_style;
            let tag = self.subject(i).tag().to_owned();
            let marker = if tag == "summary" {
                let details = self.nodes[i].parent.unwrap_or(i);
                Some(if self.subject(details).attr("open").is_some() { "\u{25be}" } else { "\u{25b8}" }.into())
            } else {
                let idx = self.nodes[i].pos.0 + self.nodes[i].parent
                    .and_then(|p| self.attr(p, "start"))
                    .and_then(|s| s.parse::<usize>().ok()).map(|s| s - 1).unwrap_or(0);
                ls.marker(idx).or_else(|| self.st(i).list_image.as_ref().map(|_| String::new()))
            };
            match marker {
                Some(m) if self.st(i).marker_inside => {
                    children.insert(0, AnyElement::Text(format!("{m}\u{a0}").into()));
                }
                m => self.nodes[i].marker = m,
            }
        }
        let count = children.iter().filter(|c| matches!(c, AnyElement::Element(_))).count();
        // positions among siblings of the same tag (`:nth-of-type`, `:last-of-type`):
        // totals per tag (few distinct tags), then running counters, O(n)
        let mut tag_counts: smallvec::SmallVec<[(&'static str, u32, u32); 8]> = smallvec::SmallVec::new();
        for c in &children {
            if let AnyElement::Element(d) = c {
                match tag_counts.iter_mut().find(|t| std::ptr::eq(t.0, d.tag) || t.0 == d.tag) {
                    Some(t) => t.1 += 1,
                    None => tag_counts.push((d.tag, 1, 0)),
                }
            }
        }
        let (mut k, mut n) = (0, 0);
        let parent_key = self.nodes[i].key;
        for child in children {
            n += 1;
            match child {
                AnyElement::Element(d) => {
                    k += 1;
                    let key = match &d.id {
                        Some(id) => id_key(id),
                        None => child_key(parent_key, d.tag, k),
                    };
                    if let Some(t) = tag_counts.iter_mut().find(|t| std::ptr::eq(t.0, d.tag) || t.0 == d.tag) {
                        t.2 += 1;
                        self.next_tpos = (t.2, t.1);
                    }
                    self.element(d, Some(i), key, (k, count));
                }
                AnyElement::Text(t) => {
                    let style = self.nodes[i].style;
                    self.push(Some(i), Content::Text(t.into_owned()), Some(i), style,
                              child_key(parent_key, "#text", n), true);
                }
            }
        }
    }

    // -- taffy ------------------------------------------------------------------------------

    fn is_block_level(&self, i: usize) -> bool {
        match &self.nodes[i].content {
            Content::Element => !self.st(i).display.is_inline_level(),
            _ => false,
        }
    }

    fn base_style(&self, i: usize) -> Style {
        let c = self.st(i);
        let mut s = c.layout.clone();
        s.display = match c.display {
            DisplayKind::Flex | DisplayKind::InlineFlex => Display::Flex,
            DisplayKind::Grid | DisplayKind::InlineGrid => Display::Grid,
            DisplayKind::Table | DisplayKind::TableRowGroup => {
                s.flex_direction = FlexDirection::Column;
                let sp = if c.border_collapse { 0.0 } else { c.border_spacing.1 };
                s.gap.height = LengthPercentage::length(sp);
                if c.display == DisplayKind::Table && !c.border_collapse {
                    let pad = |v: LengthPercentage, add: f32| match v.into_raw() {
                        r if r.tag() == taffy::CompactLength::LENGTH_TAG => LengthPercentage::length(r.value() + add),
                        _ => v,
                    };
                    let (h, v) = c.border_spacing;
                    s.padding = Rect { left: pad(s.padding.left, h), right: pad(s.padding.right, h),
                                       top: pad(s.padding.top, v), bottom: pad(s.padding.bottom, v) };
                }
                Display::Flex
            }
            DisplayKind::TableRow => {
                s.flex_direction = FlexDirection::Row;
                s.gap.width = LengthPercentage::length(if c.border_collapse { 0.0 } else { c.border_spacing.0 });
                Display::Flex
            }
            _ if c.layout.display == Display::FlowRoot => Display::FlowRoot,
            _ => Display::Block,
        };
        if c.display == DisplayKind::Table {
            if let Some(tc) = self.table_cols.get(&i).filter(|t| t.fit && !t.widths.is_empty()) {
                // auto width: the table shrinks to its columns (up to its container)
                let sp = if c.border_collapse { 0.0 } else { c.border_spacing.0 };
                let w: f32 = tc.widths.iter().sum::<f32>() + sp * (tc.widths.len() + 1) as f32;
                let pad = |v: LengthPercentage| match v.into_raw() {
                    r if r.tag() == taffy::CompactLength::LENGTH_TAG => r.value(),
                    _ => 0.0,
                };
                let extra = pad(s.padding.left) + pad(s.padding.right) + pad(s.border.left) + pad(s.border.right);
                s.size.width = Dimension::length((w + extra).ceil());
                if s.max_size.width == LengthPercentageAuto::auto() {
                    s.max_size.width = LengthPercentageAuto::percent(1.0);
                }
            }
        }
        if c.display == DisplayKind::TableCell && s.size.width == Dimension::auto() {
            let span = self.attr(i, "colspan").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).max(1);
            match self.cell_columns(i, span) {
                // every cell of a column gets the column's width: rows line up
                Some((w, fit, fixed)) if w > 0.0 || fixed => {
                    s.flex_basis = Dimension::length(w);
                    s.flex_grow = if fit && !fixed { 0.0 } else { w.max(1.0) };
                    s.flex_shrink = w.max(1.0);
                    if fixed {
                        s.min_size.width = LengthPercentageAuto::length(0.0); // fixed tables don't widen for content
                    }
                }
                _ => {
                    s.flex_grow = span as f32;
                    s.flex_shrink = 1.0;
                    s.flex_basis = Dimension::percent(0.0);
                }
            }
        }
        s
    }

    /// The rows of table `t` (in row groups or directly inside).
    fn table_rows(&self, t: usize) -> Vec<usize> {
        let mut rows = Vec::new();
        for k in kids(&self.nodes, t) {
            if !matches!(self.nodes[k].content, Content::Element) {
                continue;
            }
            match self.st(k).display {
                DisplayKind::TableRow => rows.push(k),
                DisplayKind::TableRowGroup => rows.extend(kids(&self.nodes, k)
                    .filter(|&r| matches!(self.nodes[r].content, Content::Element)
                            && self.st(r).display == DisplayKind::TableRow)),
                _ => {}
            }
        }
        rows
    }

    fn row_cells(&self, r: usize) -> Vec<usize> {
        kids(&self.nodes, r).filter(|&c| matches!(self.nodes[c].content, Content::Element | Content::Widget(_))
                                     && self.st(c).display == DisplayKind::TableCell).collect()
    }

    /// The single-line width of everything in box `k` (its text, in its fonts).
    fn max_content(&mut self, k: usize) -> f32 {
        let mut w = 0.0f32;
        let list: Vec<usize> = kids(&self.nodes, k).collect();
        for c in list {
            match &self.nodes[c].content {
                Content::Text(t) => {
                    let st = &self.styles[self.nodes[c].style];
                    let (size, flags) = (st.font_size, text::flags(st));
                    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
                    w += self.text.width(&t, size, flags) + st.letter_spacing * t.chars().count() as f32;
                }
                Content::Element => {
                    let inner = self.max_content(c);
                    let st = self.st(c);
                    let u = self.units(st.font_size);
                    let px = |v: LengthPercentage| match v.into_raw() {
                        r if r.tag() == taffy::CompactLength::LENGTH_TAG => r.value(),
                        _ => 0.0,
                    };
                    let edges = px(st.layout.padding.left) + px(st.layout.padding.right)
                        + px(st.layout.border.left) + px(st.layout.border.right);
                    let explicit = match st.layout.size.width.into_raw() {
                        r if r.tag() == taffy::CompactLength::LENGTH_TAG => Some(r.value()),
                        _ => None,
                    };
                    let _ = u;
                    let own = explicit.unwrap_or(inner + edges);
                    // blocks stack; inline content continues the line
                    if st.display.is_inline_level() || st.display == DisplayKind::Inline {
                        w += own;
                    } else {
                        w = w.max(own);
                    }
                }
                _ => {}
            }
        }
        w
    }

    /// Column widths of table `t`: the widest single-line content of each
    /// column (auto layout), or the first row's cell widths (fixed).
    fn table_columns(&mut self, t: usize) {
        let fixed = self.st(t).table_fixed;
        let fit = self.st(t).layout.size.width == Dimension::auto();
        let mut widths: Vec<f32> = Vec::new();
        let rows = self.table_rows(t);
        for (ri, r) in rows.iter().enumerate() {
            if fixed && ri > 0 {
                break;
            }
            let mut col = 0usize;
            for c in self.row_cells(*r) {
                let span = self.attr(c, "colspan").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).max(1);
                let st = self.st(c);
                let px = |v: LengthPercentage| match v.into_raw() {
                    r if r.tag() == taffy::CompactLength::LENGTH_TAG => r.value(),
                    _ => 0.0,
                };
                let edges = px(st.layout.padding.left) + px(st.layout.padding.right)
                    + px(st.layout.border.left) + px(st.layout.border.right);
                let explicit = match st.layout.size.width.into_raw() {
                    r if r.tag() == taffy::CompactLength::LENGTH_TAG => Some(r.value()),
                    _ => None,
                };
                let w = if fixed { explicit.unwrap_or(0.0) } else { explicit.unwrap_or(0.0).max(self.max_content(c) + edges) };
                if widths.len() < col + span {
                    widths.resize(col + span, 0.0);
                }
                if span == 1 {
                    widths[col] = widths[col].max(w.ceil());
                }
                col += span;
            }
        }
        if fixed {
            // unspecified columns share what the specified ones leave
            let given: f32 = widths.iter().sum();
            let free = widths.iter().filter(|w| **w == 0.0).count();
            if free > 0 {
                let total = match self.st(t).layout.size.width.into_raw() {
                    r if r.tag() == taffy::CompactLength::LENGTH_TAG => r.value(),
                    _ => self.viewport.0,
                };
                let each = ((total - given) / free as f32).max(0.0);
                widths.iter_mut().filter(|w| **w == 0.0).for_each(|w| *w = each);
            }
        }
        self.table_cols.insert(t, TableCols { widths, fit, fixed });
    }

    /// The width (px) of the columns cell `c` spans, and its table's mode.
    fn cell_columns(&self, c: usize, span: usize) -> Option<(f32, bool, bool)> {
        let row = self.nodes[c].parent?;
        let mut t = self.nodes[row].parent?;
        if self.st(t).display == DisplayKind::TableRowGroup {
            t = self.nodes[t].parent?;
        }
        let tc = self.table_cols.get(&t)?;
        let mut col = 0usize;
        for k in self.row_cells(row) {
            if k == c {
                break;
            }
            col += self.attr(k, "colspan").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).max(1);
        }
        let end = (col + span).min(tc.widths.len());
        if col >= end {
            return None;
        }
        let sp = self.st(t).border_spacing.0;
        let w = tc.widths[col..end].iter().sum::<f32>() + sp * (end - col - 1) as f32;
        Some((w, tc.fit, tc.fixed))
    }

    /// Make line-container style: a wrapping flex row aligned per text-align.
    fn line_style(c: &Computed, mut s: Style) -> Style {
        s.display = Display::Flex;
        s.flex_direction = FlexDirection::Row;
        // nowrap / pre text arrives as unbreakable pieces; wrapping only
        // happens at forced breaks (<br>, newlines) and between inline boxes
        s.flex_wrap = FlexWrap::Wrap;
        s.align_items = AlignItems::FLEX_END;
        s.align_content = AlignContent::FLEX_START;
        s.justify_content = match c.text_align {
            TextAlign::Center => JustifyContent::CENTER,
            TextAlign::Right => JustifyContent::FLEX_END,
            _ => JustifyContent::FLEX_START,
        };
        s
    }

    /// The taffy node for `key` with this style, measure context and
    /// children: last frame's node, updated only where something differs
    /// (which marks it dirty), or a new one.
    fn node(&mut self, key: Key, style: Style, ctx: Option<Measure>, children: &[NodeId]) -> NodeId {
        let stamp = self.recon.stamp;
        let reuse = match self.recon.map.get_mut(&key) {
            Some((id, st)) if *st != stamp => {
                *st = stamp;
                Some(*id)
            }
            _ => None,
        };
        let t = &mut *self.taffy;
        if let Some(id) = reuse {
            if *t.style(id) != style {
                t.set_style(id, style);
                self.changed = true;
            }
            if t.children(id) != children {
                t.set_children(id, children);
                self.changed = true;
            }
            if t.context(id) != ctx.as_ref() {
                t.set_context(id, ctx);
                self.changed = true;
            }
            return id;
        }
        let id = match ctx {
            Some(m) => t.new_leaf_with_context(style, m),
            None => t.new_with_children(style, children),
        };
        self.changed = true;
        match self.recon.map.entry(key) {
            std::collections::hash_map::Entry::Occupied(_) => self.recon.extra.push(id), // duplicate key
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert((id, stamp));
            }
        }
        id
    }

    fn leaf(&mut self, i: usize, style: Style) -> NodeId {
        let id = self.node(self.nodes[i].key, style, None, &[]);
        self.nodes[i].taffy = Some(id);
        id
    }

    /// Replace the text of box `k` by its whitespace-processed, transformed form.
    fn transform_text(&mut self, k: usize, c: StyleId) {
        let (tt, ws, tab) = (self.styles[c].text_transform, self.styles[c].white_space, self.styles[c].tab_size);
        let (rtl, ovr) = (self.styles[c].layout.direction == taffy::style::Direction::Rtl, self.styles[c].bidi_override);
        if let Content::Text(t) = &self.nodes[k].content {
            let t = match transform(t, tt, ws, tab) {
                std::borrow::Cow::Owned(s) => s,
                std::borrow::Cow::Borrowed(s) => s.to_owned(),
            };
            let t = text::bidi_visual(&t, rtl, ovr).unwrap_or(t);
            self.nodes[k].content = Content::Text(t);
        }
    }

    /// The measure context for the text of box `node`, registered under `key`.
    fn text_measure(&mut self, key: Key, node: usize, c: StyleId, nowrap: bool) -> Measure {
        self.text_box.insert(key, node);
        let hash = match &self.nodes[node].content {
            Content::Text(t) => fnv(&[t.as_bytes()]),
            _ => 0,
        };
        let c = &self.styles[c];
        Measure::Text { key, hash, size: c.font_size, flags: text::flags(c), line: c.line_px(), nowrap,
                        brk: text::break_mode(c) }
    }

    /// The taffy node for box `i` (and its subtree). Marks it dirty when
    /// anything below changed (taffy's own propagation follows parent links,
    /// which reconciling can leave stale).
    fn layout_node(&mut self, i: usize) -> NodeId {
        let outer = std::mem::replace(&mut self.changed, false);
        let id = self.layout_node_inner(i);
        if self.changed {
            self.taffy.mark_dirty(id);
        }
        self.changed |= outer;
        id
    }

    fn layout_node_inner(&mut self, i: usize) -> NodeId {
        if self.st(i).display == DisplayKind::Table {
            self.table_columns(i);
        }
        let mut base = self.base_style(i);
        if i == 0 {
            // the root fills the viewport
            base.min_size = Size { width: LengthPercentageAuto::length(self.viewport.0),
                                   height: LengthPercentageAuto::length(self.viewport.1) };
        }
        let sid = self.nodes[i].style;
        match &self.nodes[i].content {
            Content::Widget(spec) => {
                let (w, h) = (self.widgets)(self.nodes[i].key, spec);
                let mut s = base;
                if s.size.width == Dimension::auto() {
                    s.size.width = Dimension::length(w);
                }
                if s.size.height == Dimension::auto() {
                    s.size.height = Dimension::length(h);
                }
                return self.leaf(i, s);
            }
            Content::Replaced(r) => {
                let (iw, ih) = match r {
                    Replaced::Image(img) => (img.width as f32, img.height as f32),
                    Replaced::File(file, _) => (file.width as f32, file.height as f32),
                    Replaced::Video(v) => v.size().map_or((300.0, 150.0), |(w, h)| (w as f32, h as f32)),
                    _ => (300.0, 150.0),
                };
                let attr = |n: &str| self.attr(i, n).and_then(|v| v.trim_end_matches("px").parse::<f32>().ok());
                let mut s = base;
                let (aw, ah) = (attr("width"), attr("height"));
                if s.size.width == Dimension::auto() {
                    s.size.width = Dimension::length(aw.unwrap_or(match ah {
                        Some(h) if ih > 0.0 => h * iw / ih,
                        _ => iw,
                    }));
                }
                if s.size.height == Dimension::auto() {
                    s.size.height = Dimension::length(ah.unwrap_or(match aw {
                        Some(w) if iw > 0.0 => w * ih / iw,
                        _ => ih,
                    }));
                }
                return self.leaf(i, s);
            }
            Content::LineBreak => {
                let s = Style { size: Size { width: Dimension::percent(1.0), height: Dimension::length(0.0) },
                                flex_shrink: 0.0, ..Default::default() };
                return self.leaf(i, s);
            }
            Content::Text(_) => {
                // a text run as a flex/grid item: measured, wrapping. The box
                // keeps the transformed text, so paint draws what was measured.
                self.transform_text(i, sid);
                let nowrap = matches!(self.styles[sid].white_space, WhiteSpace::NoWrap | WhiteSpace::Pre);
                let key = self.nodes[i].key;
                let m = self.text_measure(key, i, sid, nowrap);
                let id = self.node(key, Style::default(), Some(m), &[]);
                self.nodes[i].taffy = Some(id);
                return id;
            }
            _ => {}
        }
        if let Some(k) = self.foldable_text(i) {
            // one leaf for the box and its text: the text is measured as the
            // box's content and placed inside it after layout
            self.transform_text(k, sid);
            let nowrap = self.styles[sid].white_space == WhiteSpace::NoWrap;
            let key = self.nodes[i].key;
            let m = self.text_measure(key, k, sid, nowrap);
            let mut s = base;
            s.display = Display::Block;
            let id = self.node(key, s, Some(m), &[]);
            self.nodes[i].taffy = Some(id);
            self.nodes[i].fold = Some(k);
            self.nodes[k].folded = true;
            return id;
        }
        let c = self.styles[sid].clone_display();
        let flex_like = matches!(c.display, DisplayKind::Flex | DisplayKind::InlineFlex
                                 | DisplayKind::Grid | DisplayKind::InlineGrid
                                 | DisplayKind::Table | DisplayKind::TableRowGroup
                                 | DisplayKind::TableRow);
        let mut list: Kids = kids(&self.nodes, i).collect();
        let ordered = |b: &Builder, k: usize| {
            let st = b.st(k);
            if st.display == DisplayKind::TableCaption && st.caption_bottom { i32::MAX } else { st.order }
        };
        if list.iter().any(|&k| ordered(self, k) != 0) {
            list.sort_by_key(|&k| ordered(self, k));
        }
        let mut out: NodeList = NodeList::new();
        if flex_like {
            for k in list {
                if let Content::Text(t) = &self.nodes[k].content {
                    if t.trim().is_empty() && self.st(k).white_space == WhiteSpace::Normal {
                        continue;
                    }
                }
                self.child_nodes(k, &mut out);
            }
            let id = self.node(self.nodes[i].key, base, None, &out);
            self.nodes[i].taffy = Some(id);
            return id;
        }
        // block container: runs of inline content become line boxes
        if list.iter().all(|&k| !self.is_block_level(k)) {
            self.inline_items(&list, i, &mut out, true);
            let line = Self::line_style(&self.styles[sid], base);
            let id = self.node(self.nodes[i].key, line, None, &out);
            self.nodes[i].taffy = Some(id);
            return id;
        }
        let mut run: Kids = Kids::new();
        for k in list {
            if self.is_block_level(k) {
                self.flush_run(i, &mut run, &mut out);
                self.child_nodes(k, &mut out);
            } else {
                run.push(k);
            }
        }
        self.flush_run(i, &mut run, &mut out);
        let id = self.node(self.nodes[i].key, base, None, &out);
        self.nodes[i].taffy = Some(id);
        id
    }

    /// The lone text child of element `i`, when laying the pair out as one
    /// measured leaf gives the same result as two nodes (or a line of words).
    fn foldable_text(&self, i: usize) -> Option<usize> {
        let n = &self.nodes[i];
        if !matches!(n.content, Content::Element) || n.first == NONE || n.first != n.last {
            return None;
        }
        let k = n.first as usize;
        match &self.nodes[k].content {
            Content::Text(t) if !t.trim().is_empty() => {}
            _ => return None,
        }
        let c = self.st(i);
        let s = &c.layout;
        let display_ok = match c.display {
            DisplayKind::Block | DisplayKind::InlineBlock | DisplayKind::ListItem
            | DisplayKind::TableCell | DisplayKind::TableCaption => true,
            DisplayKind::Flex | DisplayKind::InlineFlex => {
                s.flex_wrap == FlexWrap::NoWrap
                    && matches!(s.flex_direction, FlexDirection::Row | FlexDirection::Column)
            }
            _ => false,
        };
        (display_ok
            && c.letter_spacing == 0.0
            && c.word_spacing == 0.0
            && c.text_indent == crate::geometry::Length::Px(0.0)
            && c.vertical_align == VAlign::Baseline
            && !c.small_caps
            && matches!(c.white_space, WhiteSpace::Normal | WhiteSpace::NoWrap)
            && s.overflow.x == Overflow::Visible
            && s.overflow.y == Overflow::Visible)
            .then_some(k)
    }

    fn child_nodes(&mut self, k: usize, out: &mut NodeList) {
        if self.st(k).display == DisplayKind::Contents {
            let list: Kids = kids(&self.nodes, k).collect();
            for c in list {
                self.child_nodes(c, out);
            }
            return;
        }
        out.push(self.layout_node(k));
    }

    /// An anonymous line box for inline content between blocks.
    fn flush_run(&mut self, parent: usize, run: &mut Kids, out: &mut NodeList) {
        let only_space = run.iter().all(|&k| matches!(&self.nodes[k].content,
                                                      Content::Text(t) if t.trim().is_empty()));
        if run.is_empty() || only_space {
            run.clear();
            return;
        }
        let style = self.nodes[parent].style;
        let key = child_key(self.nodes[parent].key, "#anon", out.len());
        let anon = self.push(Some(parent), Content::Anon, Some(parent), style, key, false);
        let mut line = NodeList::new();
        let items: Kids = std::mem::take(run);
        self.inline_items(&items, parent, &mut line, true);
        let ls = Self::line_style(&self.styles[style], Style::default());
        let id = self.node(key, ls, None, &line);
        if self.changed {
            self.taffy.mark_dirty(id);
        }
        self.nodes[anon].taffy = Some(id);
        out.push(id);
    }

    /// Flatten inline content into line items: words, atomic inline boxes, breaks.
    fn inline_items(&mut self, list: &[usize], owner: usize, line: &mut NodeList, first: bool) {
        let mut first_word = first;
        for &k in list {
            match &self.nodes[k].content {
                Content::Text(_) => {
                    let n = line.len();
                    self.words(k, owner, first_word, line);
                    if line.len() > n {
                        first_word = false;
                    }
                }
                Content::Element if matches!(self.st(k).display, DisplayKind::Inline | DisplayKind::Contents) => {
                    let inner: Kids = kids(&self.nodes, k).collect();
                    self.inline_items(&inner, k, line, first_word);
                    first_word = false;
                }
                _ => {
                    self.child_nodes(k, line);
                    first_word = false;
                }
            }
        }
    }

    /// Word boxes for text box `src`, with the style of its inline owner. The
    /// box keeps the processed text; words are byte ranges into it.
    fn words(&mut self, src: usize, owner: usize, first: bool, line: &mut NodeList) {
        let sid = self.nodes[owner].style;
        let c = &self.styles[sid];
        let (ws, tt, size, letter, word_sp) = (c.white_space, c.text_transform, c.font_size, c.letter_spacing, c.word_spacing);
        let (tab, break_all) = (c.tab_size, c.break_all);
        let flags = text::flags(c);
        let line_h = c.line_px();
        let indent = if first { c.text_indent.resolve(&self.units(size), 0.0) } else { 0.0 };
        // the processed text, kept in the box
        let Content::Text(raw) = std::mem::replace(&mut self.nodes[src].content, Content::Anon) else { return };
        let mut text = match transform(&raw, tt, ws, tab) {
            std::borrow::Cow::Borrowed(_) => raw,
            std::borrow::Cow::Owned(s) => s,
        };
        // bidirectional text: laid out in visual order
        let (rtl, ovr) = (c.layout.direction == taffy::style::Direction::Rtl, c.bidi_override);
        if let Some(v) = text::bidi_visual(&text, rtl, ovr) {
            text = v;
        }
        if ws == WhiteSpace::NoWrap {
            text = collapse(&text);
        }
        // pieces: byte range, trailing space, forced break after
        let mut pieces: smallvec::SmallVec<[(usize, usize, bool, bool); 16]> = smallvec::SmallVec::new();
        let base = text.as_ptr() as usize;
        let range = |w: &str| (w.as_ptr() as usize - base, w.as_ptr() as usize - base + w.len());
        match ws {
            WhiteSpace::Pre | WhiteSpace::PreWrap | WhiteSpace::PreLine => {
                let lines: Vec<&str> = text.split('\n').collect();
                for (n, l) in lines.iter().enumerate() {
                    let brk = n + 1 < lines.len();
                    if ws == WhiteSpace::Pre {
                        let (a, b) = range(l);
                        pieces.push((a, b, false, brk));
                    } else {
                        let words: Vec<&str> = l.split_whitespace().collect();
                        if words.is_empty() {
                            let (a, _) = range(l);
                            pieces.push((a, a, false, brk));
                        }
                        for (j, w) in words.iter().enumerate() {
                            let (a, b) = range(w);
                            pieces.push((a, b, j + 1 < words.len(), brk && j + 1 == words.len()));
                        }
                    }
                }
            }
            WhiteSpace::NoWrap => pieces.push((0, text.len(), false, false)),
            WhiteSpace::Normal => {
                let lead = text.starts_with(char::is_whitespace) && !first;
                let words: Vec<&str> = text.split_whitespace().collect();
                let trail = text.ends_with(char::is_whitespace);
                for (j, w) in words.iter().enumerate() {
                    let (mut a, b) = range(w);
                    if j == 0 && lead {
                        a -= 1; // one leading space, drawn with the word
                    }
                    pieces.push((a, b, j + 1 < words.len() || trail, false));
                }
            }
        }
        // overflow-wrap: a word wider than its block (as laid out last
        // frame) is cut where it has to be
        let anywhere = self.styles[sid].wrap_anywhere && !break_all
            && ws != WhiteSpace::NoWrap && ws != WhiteSpace::Pre;
        if anywhere {
            let mut blk = owner;
            while self.st(blk).display.is_inline_level() || self.st(blk).display == DisplayKind::Inline {
                match self.nodes[blk].parent {
                    Some(p) => blk = p,
                    None => break,
                }
            }
            let max = self.wrap_widths.get(&self.nodes[blk].key).copied().unwrap_or(self.viewport.0).max(1.0);
            let mut out: smallvec::SmallVec<[(usize, usize, bool, bool); 16]> = smallvec::SmallVec::new();
            for &(a, b, trailing, brk) in &pieces {
                let piece = &text[a..b];
                if self.text.width(piece, size, flags) <= max {
                    out.push((a, b, trailing, brk));
                    continue;
                }
                let mut start = a;
                let mut acc = String::new();
                for (o, ch) in piece.char_indices() {
                    acc.push(ch);
                    if self.text.width(&acc, size, flags) > max && acc.chars().count() > 1 {
                        out.push((start, a + o, false, false));
                        start = a + o;
                        acc.clear();
                        acc.push(ch);
                    }
                }
                out.push((start, b, trailing, brk));
            }
            pieces = out;
        }
        if break_all && ws != WhiteSpace::NoWrap && ws != WhiteSpace::Pre {
            let mut chars: smallvec::SmallVec<[(usize, usize, bool, bool); 16]> = smallvec::SmallVec::new();
            for &(a, b, trailing, brk) in &pieces {
                let piece = &text[a..b];
                let n = piece.char_indices().count();
                for (j, (o, ch)) in piece.char_indices().enumerate() {
                    let last = j + 1 == n;
                    chars.push((a + o, a + o + ch.len_utf8(), trailing && last, brk && last));
                }
                if n == 0 {
                    chars.push((a, b, trailing, brk));
                }
            }
            pieces = chars;
        }
        let space = self.text.width(" ", size, flags) + word_sp;
        let src_key = self.nodes[src].key;
        for (n, &(a, b, trailing, brk)) in pieces.iter().enumerate() {
            let piece = &text[a..b];
            // a leading space becomes a plain space for measuring and drawing
            let w = self.text.width(piece, size, flags)
                + letter * piece.chars().count() as f32
                + if trailing { space } else { 0.0 };
            let content = Content::Word { src: src as u32, start: a as u32, end: b as u32 };
            let node = self.push(Some(owner), content, Some(owner), sid, child_key(src_key, "#w", n), false);
            let s = Style {
                size: Size { width: Dimension::length(w.ceil()), height: Dimension::length(line_h) },
                flex_shrink: 0.0,
                margin: Rect { left: LengthPercentageAuto::length(if n == 0 { indent } else { 0.0 }),
                               ..Rect::zero() },
                ..Default::default()
            };
            line.push(self.leaf(node, s));
            if brk {
                let br = self.push(Some(owner), Content::LineBreak, Some(owner), sid,
                                   child_key(src_key, "#br", n), false);
                line.push(self.leaf(br, Style {
                    size: Size { width: Dimension::percent(1.0), height: Dimension::length(0.0) },
                    flex_shrink: 0.0,
                    ..Default::default()
                }));
            }
        }
        self.nodes[src].content = Content::Text(text);
    }
}

impl Computed {
    /// The few fields `layout_node` reads after building children (avoids
    /// holding a borrow of the style store across recursion).
    fn clone_display(&self) -> DisplayOnly {
        DisplayOnly { display: self.display }
    }
}

struct DisplayOnly {
    display: DisplayKind,
}

fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for w in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

/// Whitespace processing and `text-transform`; borrows when nothing changes.
fn transform(s: &str, t: TextTransform, ws: WhiteSpace, tab: u8) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    let collapse_ws = (ws == WhiteSpace::Normal || ws == WhiteSpace::NoWrap) && s.contains(['\n', '\t', '\r']);
    // preserved tabs advance to the next multiple of `tab-size` spaces
    let expand_tabs = matches!(ws, WhiteSpace::Pre | WhiteSpace::PreWrap) && s.contains('\t');
    if !collapse_ws && !expand_tabs && t == TextTransform::None {
        return Cow::Borrowed(s);
    }
    let s = if collapse_ws {
        s.replace(['\n', '\t', '\r'], " ")
    } else if expand_tabs {
        let mut out = String::with_capacity(s.len() + 16);
        let mut col = 0usize;
        for ch in s.chars() {
            match ch {
                '\t' if tab > 0 => {
                    let n = tab as usize - col % tab as usize;
                    out.extend(std::iter::repeat_n(' ', n));
                    col += n;
                }
                '\t' => {}
                '\n' => {
                    out.push('\n');
                    col = 0;
                }
                c => {
                    out.push(c);
                    col += 1;
                }
            }
        }
        out
    } else {
        s.to_string()
    };
    Cow::Owned(match t {
        TextTransform::None => s,
        TextTransform::Upper => s.to_uppercase(),
        TextTransform::Lower => s.to_lowercase(),
        TextTransform::Capitalize => s
            .split(' ')
            .map(|w| {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
            })
            .collect::<Vec<String>>()
            .join(" "),
    })
}

/// Build, cascade and lay out `root` in a `viewport` (logical pixels),
/// reusing the arenas of `recycle` (the previous frame) and `caches`.
pub(crate) fn layout(root: AnyElement, viewport: (f32, f32), state: &UiState, caches: &mut LayoutCaches,
                     recycle: Frame, widgets: &mut dyn FnMut(Key, &WidgetSpec) -> (f32, f32)) -> Frame {
    let mut css = String::new();
    let mut title = None;
    caches.tw.on = TAILWIND.load(std::sync::atomic::Ordering::Relaxed);
    collect_styles(&root, &mut css, &mut title, &mut caches.tw);
    if caches.tw.on && !caches.tw.rules.is_empty() {
        css.push('\n');
        css.push_str(caches.tw.css());
    }
    let css_hash = fnv(&[css.as_bytes()]);
    if css_hash != caches.author.hash {
        caches.author = AuthorSheet { hash: css_hash, sheet: Stylesheet::parse(&css) };
        load_font_faces(&caches.author.sheet);
    }
    caches.styles.begin_frame();
    caches.styles.seed_properties(&caches.author.sheet, caches.author.hash);
    // the taffy tree persists: drop last frame's duplicates, start a new stamp
    for id in caches.recon.extra.drain(..) {
        caches.taffy.remove(id);
    }
    caches.recon.stamp = caches.recon.stamp.wrapping_add(1);
    caches.text_box.clear();
    let Frame { mut nodes, mut els, .. } = recycle;
    nodes.clear();
    els.clear();
    if caches.matched.author != caches.author.hash || caches.matched.map.len() > 50_000 {
        caches.matched.map.clear();
        caches.matched.author = caches.author.hash;
    }
    let LayoutCaches { styles, text, images, author, taffy, box_of, acc, matched, recon, text_box, containers,
                       wrap_widths, anim, .. } = caches;
    let mut b = Builder {
        nodes,
        els,
        taffy,
        styles,
        text,
        images,
        matched,
        recon,
        text_box,
        containers,
        wrap_widths,
        changed: false,
        next_tpos: (1, 1),
        author: &author.sheet,
        author_hash: author.hash,
        ua_media: ua_stylesheet().media_results(&crate::css::MediaEnv::viewport(viewport)),
        ua_styled_media: ua_styled_stylesheet().media_results(&crate::css::MediaEnv::viewport(viewport)),
        au_media: author.sheet.media_results(&crate::css::MediaEnv::viewport(viewport)),
        state,
        viewport,
        title,
        widgets,
        counters: Vec::new(),
        table_cols: FnvMap::default(),
        anim,
    };
    // the document: <html><body> unless the app built them itself
    let root_div = match root {
        AnyElement::Element(d) if d.tag == "html" => d,
        AnyElement::Element(d) if d.tag == "body" => crate::html::html().child(d),
        other => {
            let mut body = crate::html::body();
            body.children.push(other);
            crate::html::html().child(body)
        }
    };
    let s = span(Phase::Build);
    let root = match b.element(root_div, None, ROOT_KEY, (1, 1)) {
        Some(r) => r,
        None => b.push(None, Content::Element, None, ROOT_STYLE, ROOT_KEY, false),
    };
    drop(s);
    let s = span(Phase::Boxes);
    let root_id = b.layout_node(root);
    drop(s);
    let s = span(Phase::Taffy);
    // nodes not used this frame
    let stamp = b.recon.stamp;
    let taffy_ref = &mut *b.taffy;
    b.recon.map.retain(|_, (id, st)| {
        if *st == stamp {
            return true;
        }
        taffy_ref.remove(*id);
        false
    });
    let avail = Size { width: AvailableSpace::Definite(viewport.0),
                       height: AvailableSpace::Definite(viewport.1) };
    let Builder { nodes, els, taffy, styles, text: tc, title, text_box, .. } = b;
    taffy.compute(root_id, avail, |known, available, ctx| {
            match ctx {
                Measure::Text { key, size, flags, line, nowrap, brk, .. } => {
                    let text = match text_box.get(key).map(|&k| &nodes[k].content) {
                        Some(Content::Text(t)) => t.as_str(),
                        _ => "",
                    };
                    if !text.contains([' ', '\n']) && *brk == 0 {
                        // one word: no wrapping to do
                        let w = tc.width(text, *size, *flags);
                        return Size { width: known.width.unwrap_or(w.ceil()),
                                      height: known.height.unwrap_or(*line) };
                    }
                    let max_w = known.width.unwrap_or(match available.width {
                        AvailableSpace::Definite(w) => w,
                        AvailableSpace::MinContent => 0.0,
                        AvailableSpace::MaxContent => f32::INFINITY,
                    });
                    let lines = tc.wrap(text, *size, *flags, if *nowrap { f32::INFINITY } else { max_w }, *brk);
                    let w = lines.iter().map(|l| tc.width(l, *size, *flags)).fold(0.0, f32::max);
                    Size { width: known.width.unwrap_or(w.ceil()),
                           height: known.height.unwrap_or(*line * lines.len().max(1) as f32) }
                }
            }
        });
    drop(s);
    let _s = span(Phase::Place);
    let mut frame = Frame { nodes, els, title, by_key: Default::default() };
    box_of.clear();
    for (i, n) in frame.nodes.iter().enumerate() {
        if let Some(t) = n.taffy {
            let s = slot(t);
            if s >= box_of.len() {
                box_of.resize(s + 1, NONE);
            }
            box_of[s] = i as u32;
        }
    }
    let mut walk = Walk { t: taffy, box_of, state, vp: viewport, styles, text: tc };
    walk.place(&mut frame.nodes, root_id, 0.0, 0.0, None, (0.0, 0.0));
    finish(&mut frame.nodes, styles, acc);
    frame
}

/// Positions follow the taffy tree (words live in line containers, which may
/// be anonymous); translate and scroll offsets shift whole subtrees.
struct Walk<'a> {
    t: &'a crate::tree::LayoutTree<Measure>,
    box_of: &'a [u32],
    state: &'a UiState,
    vp: (f32, f32),
    styles: &'a StyleStore,
    text: &'a mut TextCache,
}

impl Walk<'_> {
    /// `cb`: the origin of the containing block of absolutely positioned
    /// descendants (taffy places those relative to it, not to their parent).
    fn place(&mut self, nodes: &mut [BoxNode], id: NodeId, ox: f32, oy: f32, clip: Option<Bounds<f32>>,
             cb: (f32, f32)) {
        let l = *self.t.layout(id);
        let (mut x, mut y) = (ox + l.location.x, oy + l.location.y);
        let (w, h) = (l.size.width, l.size.height);
        let mut child_clip = clip;
        let (mut sx, mut sy) = (0.0, 0.0);
        if let Some(i) = self.box_of.get(slot(id)).filter(|&&b| b != NONE).map(|&b| b as usize) {
            let st = &self.styles[nodes[i].style];
            let u = Units { font_size: st.font_size, viewport: self.vp };
            x += st.translate.0.resolve(&u, w);
            y += st.translate.1.resolve(&u, h);
            if matches!(nodes[i].content, Content::Word { .. }) {
                y += match st.vertical_align {
                    VAlign::Sub => st.font_size * 0.25,
                    VAlign::Super => -st.font_size * 0.35,
                    VAlign::Baseline => 0.0,
                };
            }
            nodes[i].bounds = rect(x, y, w, h);
            nodes[i].clip = clip;
            if let Some(cr) = st.clip_rect.filter(|_| matches!(st.layout.position, Position::Absolute | Position::Fixed)) {
                // edges from the border box's top-left corner; auto = that edge
                let e = |k: usize, auto: f32, basis: f32| cr[k].map_or(auto, |l| l.resolve(&u, basis));
                let (top, right, bottom, left) = (e(0, 0.0, h), e(1, w, w), e(2, h, h), e(3, 0.0, w));
                let r = rect(x + left, y + top, (right - left).max(0.0), (bottom - top).max(0.0));
                let r = match clip {
                    Some(c) => c.intersect(&r),
                    None => r,
                };
                nodes[i].clip = Some(r);
                child_clip = Some(r);
            }
            let ov = st.layout.overflow;
            if ov.x != Overflow::Visible || ov.y != Overflow::Visible {
                let b = l.border;
                let pad = rect(x + b.left, y + b.top, (w - b.left - b.right).max(0.0),
                               (h - b.top - b.bottom).max(0.0));
                child_clip = Some(match clip {
                    Some(c) => c.intersect(&pad),
                    None => pad,
                });
                if ov.x == Overflow::Scroll || ov.y == Overflow::Scroll {
                    let axis = |o: Overflow, content: f32, view: f32| {
                        if o == Overflow::Scroll { (content - view).max(0.0) } else { 0.0 }
                    };
                    let range = (axis(ov.x, l.scrollable_overflow_rect.right, pad.size.width),
                                 axis(ov.y, l.scrollable_overflow_rect.bottom, pad.size.height));
                    nodes[i].scroll_range = range;
                    let (ax, ay) = self.state.scroll.get(&nodes[i].key).copied().unwrap_or((0.0, 0.0));
                    sx = ax.clamp(0.0, range.0);
                    sy = ay.clamp(0.0, range.1);
                    nodes[i].scroll_pos = (sx, sy);
                }
            }
            if let Some(k) = nodes[i].fold {
                place_folded_text(nodes, i, k, rect(x, y, w, h), &l, self.styles, self.text);
            }
        }
        let inner = (x - sx, y - sy);
        let cb = if self.t.style(id).position == Position::Static { cb } else { inner };
        for &k in self.t.children(id) {
            let (kx, ky) = match self.t.style(k).position {
                Position::Absolute => cb,
                Position::Fixed => (0.0, 0.0),
                _ => inner,
            };
            self.place(nodes, k, kx, ky, child_clip, cb);
        }
    }
}

/// Put a folded text run where its own flex item / line box would have gone:
/// inside the content box, aligned like a lone flex item (or by `text-align`
/// in a block, which paint applies across the full width).
fn place_folded_text(nodes: &mut [BoxNode], i: usize, k: usize, b: Bounds<f32>, l: &taffy::Layout,
                     styles: &StyleStore, tc: &mut TextCache) {
    let c = &styles[nodes[i].style];
    let cb = rect(b.origin.x + l.border.left + l.padding.left, b.origin.y + l.border.top + l.padding.top,
                  (b.size.width - l.border.left - l.border.right - l.padding.left - l.padding.right).max(0.0),
                  (b.size.height - l.border.top - l.border.bottom - l.padding.top - l.padding.bottom).max(0.0));
    let Content::Text(t) = &nodes[k].content else { return };
    let flags = text::flags(c);
    let line = c.line_px();
    let brk = text::break_mode(c);
    let (tw, th) = if !t.contains([' ', '\n']) && brk == 0 {
        (tc.width(t, c.font_size, flags).ceil(), line)
    } else {
        let max = if c.white_space == WhiteSpace::NoWrap { f32::INFINITY } else { cb.size.width + 0.5 };
        let lines = tc.wrap(t, c.font_size, flags, max, brk);
        let w = lines.iter().map(|s| tc.width(s, c.font_size, flags)).fold(0.0, f32::max);
        (w.ceil(), line * lines.len().max(1) as f32)
    };
    let tw = if c.white_space == WhiteSpace::NoWrap { tw } else { tw.min(cb.size.width) };
    let s = &c.layout;
    // offset of a box of `size` in `room` for a start / center / end choice
    let at = |room: f32, size: f32, pos: u8| match pos {
        1 => (room - size) / 2.0,
        2 => room - size,
        _ => 0.0,
    };
    use taffy::style::{AlignContentKeyword as Ac, AlignItemsKeyword as Ai};
    let justify = match s.justify_content.keyword() {
        Ac::Center | Ac::SpaceAround | Ac::SpaceEvenly => 1,
        Ac::End | Ac::FlexEnd => 2,
        _ => 0,
    };
    let align = match s.align_items.keyword() {
        Ai::Center => 1,
        Ai::End | Ai::FlexEnd => 2,
        _ => 0,
    };
    let stretch = matches!(s.align_items.keyword(), Ai::Normal | Ai::Stretch);
    let tb = match c.display {
        DisplayKind::Flex | DisplayKind::InlineFlex if s.flex_direction == FlexDirection::Row => {
            rect(cb.origin.x + at(cb.size.width, tw, justify), cb.origin.y + at(cb.size.height, th, align), tw, th)
        }
        DisplayKind::Flex | DisplayKind::InlineFlex => {
            let w = if stretch { cb.size.width } else { tw };
            rect(cb.origin.x + at(cb.size.width, w, align), cb.origin.y + at(cb.size.height, th, justify), w, th)
        }
        _ => rect(cb.origin.x, cb.origin.y, cb.size.width, th), // block: text-align at paint
    };
    nodes[k].bounds = tb;
}

/// Inherited opacity / visibility (parents precede children in the arena),
/// and bounds of boxes without a layout node (inline elements): the union of
/// their pieces (children follow parents, so a reverse pass sees them first).
fn finish(nodes: &mut [BoxNode], styles: &StyleStore, acc: &mut Vec<Option<Bounds<f32>>>) {
    for i in 0..nodes.len() {
        let (po, pv, pc) = match nodes[i].parent {
            Some(p) => (nodes[p].opacity, nodes[p].visible, nodes[p].clip),
            None => (1.0, true, None),
        };
        let st = &styles[nodes[i].style];
        let own = matches!(nodes[i].content, Content::Element | Content::Widget(_) | Content::Replaced(_));
        nodes[i].opacity = if own { po * st.opacity } else { po };
        nodes[i].visible = pv && st.visible;
        if nodes[i].taffy.is_none() {
            nodes[i].clip = pc;
        }
    }
    acc.clear();
    acc.resize(nodes.len(), None);
    for i in (0..nodes.len()).rev() {
        if nodes[i].taffy.is_none() {
            if let Some(a) = acc[i] {
                nodes[i].bounds = a;
            }
        }
        let Some(p) = nodes[i].parent else { continue };
        if nodes[p].taffy.is_some() {
            continue;
        }
        let cb = nodes[i].bounds;
        if cb.size.width <= 0.0 && cb.size.height <= 0.0 {
            continue;
        }
        acc[p] = Some(match acc[p] {
            None => cb,
            Some(a) => {
                let x0 = a.origin.x.min(cb.origin.x);
                let y0 = a.origin.y.min(cb.origin.y);
                let x1 = (a.origin.x + a.size.width).max(cb.origin.x + cb.size.width);
                let y1 = (a.origin.y + a.size.height).max(cb.origin.y + cb.size.height);
                rect(x0, y0, x1 - x0, y1 - y0)
            }
        });
    }
}

impl Frame {
    /// Element boxes by key, built on first use (events and tests need it,
    /// rendering doesn't).
    pub(crate) fn by_key(&self) -> &HashMap<Key, usize> {
        self.by_key.get_or_init(|| {
            self.nodes.iter().enumerate().filter(|(_, n)| n.el != NONE).map(|(i, n)| (n.key, i)).collect()
        })
    }

    /// The element data of box `i`.
    pub(crate) fn el(&self, i: usize) -> Option<&ElData> {
        let e = self.nodes[i].el;
        (e != NONE).then(|| &self.els[e as usize])
    }

    pub(crate) fn attr(&self, i: usize, name: &str) -> Option<&str> {
        el_attr(&self.nodes, &self.els, i, name)
    }

    /// Element children of box `i`.
    pub(crate) fn children_of(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        kids(&self.nodes, i).filter(|&k| matches!(self.nodes[k].content, Content::Element | Content::Widget(_)
                                                  | Content::Replaced(_)))
    }

    /// Box `i` has no content (only collapsible whitespace): `empty-cells`.
    pub(crate) fn is_empty(&self, i: usize) -> bool {
        kids(&self.nodes, i).all(|k| match &self.nodes[k].content {
            Content::Text(t) => t.trim().is_empty(),
            Content::Anon => kids(&self.nodes, k).next().is_none(),
            _ => false,
        })
    }

    /// The text a box draws: a text run's text, or a word's slice of it.
    pub(crate) fn text(&self, i: usize) -> &str {
        match &self.nodes[i].content {
            Content::Text(t) => t,
            Content::Word { src, start, end } => match &self.nodes[*src as usize].content {
                Content::Text(t) => &t[*start as usize..*end as usize],
                _ => "",
            },
            _ => "",
        }
    }

    /// Element boxes from the root down to the topmost box under (x, y).
    pub(crate) fn hit_path(&self, x: f32, y: f32, styles: &StyleStore) -> Vec<usize> {
        let mut best: Option<(i32, usize)> = None;
        for (i, n) in self.nodes.iter().enumerate() {
            if !n.visible || n.taffy.is_none() || !styles[n.style].pointer_events {
                continue;
            }
            if n.clip.is_some_and(|c| !c.contains(x, y)) || !n.bounds.contains(x, y) {
                continue;
            }
            // later boxes paint over earlier ones; z-index lifts a subtree
            let z = self.z_of(i, styles);
            if best.is_none_or(|(bz, _)| z >= bz) {
                best = Some((z, i));
            }
        }
        let mut path = Vec::new();
        let mut cur = best.map(|(_, b)| self.nodes[b].owner);
        while let Some(i) = cur {
            if self.nodes[i].el != NONE {
                path.push(i);
            }
            cur = self.nodes[i].parent;
        }
        path.reverse();
        path
    }

    /// The effective stacking level of a box (its nearest z-index).
    pub(crate) fn z_of(&self, mut i: usize, styles: &StyleStore) -> i32 {
        loop {
            if let Some(z) = styles[self.nodes[i].style].z_index {
                return z;
            }
            match self.nodes[i].parent {
                Some(p) => i = p,
                None => return 0,
            }
        }
    }
}

/// `@font-face`: registers each face's first `url()` that loads (paths are
/// relative to the working directory), once per process. Styles computed
/// before a face loaded are recomputed because the sheet changed.
fn load_font_faces(sheet: &Stylesheet) {
    static LOADED: std::sync::Mutex<Vec<(String, bool, bool)>> = std::sync::Mutex::new(Vec::new());
    for face in &sheet.font_faces {
        let key = (face.family.to_ascii_lowercase(), face.bold, face.italic);
        if LOADED.lock().unwrap_or_else(|e| e.into_inner()).contains(&key) {
            continue;
        }
        let loaded = face.src.iter().any(|path| {
            std::fs::read(path).is_ok_and(|bytes| crate::add_font(&face.family, face.bold, face.italic, &bytes))
        });
        if loaded {
            LOADED.lock().unwrap_or_else(|e| e.into_inner()).push(key);
        }
    }
}
