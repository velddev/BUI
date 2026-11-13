//! Form controls are Blender's own widgets: each `<button>`, `<input>`,
//! `<select>`, `<textarea>`, `<progress>` and `<meter>` is a kit node,
//! reconciled by key across renders and placed where layout put it.

use std::collections::HashMap;
use std::ffi::{CStr, CString};

use crate::color::{parse_color, Rgba};
use crate::element::{AnyElement, Div};
use crate::events::ChangeEvent;
use crate::geometry::Bounds;
use crate::element::SharedString;
use crate::layout::{Key, UiState};
use blenderui_sys as sys;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WKind {
    Button,
    Text { mask: bool },
    Number,
    Slider,
    Checkbox,
    Radio,
    Color,
    Select,
    TextArea,
    Progress,
    Meter,
}

impl WKind {
    fn class(self) -> &'static CStr {
        match self {
            WKind::Button => c"button",
            WKind::Text { .. } => c"text",
            WKind::Number => c"number",
            WKind::Slider => c"slider",
            WKind::Checkbox => c"checkbox",
            WKind::Radio => c"radio",
            WKind::Color => c"swatch",
            WKind::Select => c"dropdown",
            WKind::TextArea => c"textarea",
            WKind::Progress => c"progress",
            WKind::Meter => c"meter",
        }
    }

    /// Default size in logical pixels when the kit's natural size is smaller.
    fn min_size(self) -> (f32, f32) {
        match self {
            WKind::Text { .. } | WKind::Number | WKind::Slider => (150.0, 20.0),
            WKind::Select => (120.0, 20.0),
            WKind::TextArea => (240.0, 64.0),
            WKind::Progress | WKind::Meter => (160.0, 20.0),
            WKind::Color => (40.0, 20.0),
            WKind::Checkbox | WKind::Radio => (20.0, 20.0),
            WKind::Button => (20.0, 20.0),
        }
    }
}

/// A textarea's caret and the other end of its selection, as byte offsets
/// into its text (`anchor == caret`: nothing selected). The kit does not
/// wrap: the lines are the text split at `\n`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextCaret {
    pub anchor: usize,
    pub caret: usize,
}

impl TextCaret {
    /// The caret at `at`, nothing selected.
    #[must_use]
    pub const fn at(at: usize) -> TextCaret {
        TextCaret { anchor: at, caret: at }
    }
}

/// What a form element asks for (compared with the last render to sync).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WidgetSpec {
    pub kind: WKind,
    pub label: String,
    pub text: Option<String>,
    pub value: Option<f64>,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub items: Vec<String>,
    pub color: Option<Rgba>,
    pub placeholder: String,
    pub disabled: bool,
    pub name: Option<String>,
    /// Drawn like a browser's form control (plain HTML outside `styled`);
    /// false: a Blender widget (inside `styled`, or a Blender component).
    pub html: bool,
    /// `<optgroup>` labels: (index of the option they come before, label).
    pub groups: Vec<(usize, String)>,
    /// `<meter>`'s low, high and optimum.
    pub meter: [Option<f64>; 3],
    /// Text size over the kit's 11px (0: the kit's size): a Blender text
    /// box's CSS `font-size`.
    pub font_scale: f32,
    /// `<textarea bui-edge-jump>`: Up on the first line moves the caret to
    /// the start, Down on the last line to the end (Shift extends).
    pub edge_jump: bool,
}

impl WidgetSpec {
    /// The kit's item list: options, with group labels (marked `\x01`) in
    /// the browser look.
    fn kit_items(&self) -> Vec<String> {
        if !self.html || self.groups.is_empty() {
            return self.items.clone();
        }
        let mut out = Vec::with_capacity(self.items.len() + self.groups.len());
        let mut g = self.groups.iter().peekable();
        for (k, it) in self.items.iter().enumerate() {
            while let Some((at, label)) = g.next_if(|(at, _)| *at <= k) {
                let _ = at;
                out.push(format!("\u{1}{label}"));
            }
            out.push(it.clone());
        }
        out.extend(g.map(|(_, l)| format!("\u{1}{l}")));
        out
    }

    /// Option index to the kit's item index, and back.
    fn to_kit(&self, option: f64) -> f64 {
        if !self.html {
            return option;
        }
        option + self.groups.iter().filter(|(at, _)| (*at as f64) <= option).count() as f64
    }
    fn from_kit(&self, item: f64) -> f64 {
        if !self.html {
            return item;
        }
        let mut labels = 0.0;
        for (at, _) in &self.groups {
            if (*at as f64) + labels <= item {
                labels += 1.0;
            }
        }
        item - labels
    }
}

fn text_of(children: &[AnyElement]) -> Option<String> {
    let mut s = String::new();
    for c in children {
        match c {
            AnyElement::Text(t) => s.push_str(t),
            AnyElement::Element(d) if matches!(d.tag, "span" | "b" | "i" | "em" | "strong") => {
                s.push_str(&text_of(&d.children)?)
            }
            AnyElement::Element(_) => return None,
        }
    }
    Some(s.split_whitespace().collect::<Vec<_>>().join(" "))
}

impl WidgetSpec {
    fn new(kind: WKind, attrs: &Attrs) -> WidgetSpec {
        let num = |k: &str| attr(attrs, k).and_then(|v| v.trim().parse::<f64>().ok());
        WidgetSpec {
            kind,
            label: String::new(),
            text: None,
            value: None,
            min: num("min").unwrap_or(f64::NEG_INFINITY),
            max: num("max").unwrap_or(f64::INFINITY),
            step: num("step").unwrap_or(1.0),
            items: Vec::new(),
            color: None,
            placeholder: attr(attrs, "placeholder").unwrap_or("").to_owned(),
            disabled: attr(attrs, "disabled").is_some(),
            name: attr(attrs, "name").map(str::to_owned),
            html: false,
            groups: Vec::new(),
            meter: [None; 3],
            font_scale: 0.0,
            edge_jump: false,
        }
    }

    /// The widget for a form element, or None to lay it out as a box
    /// (a `<button>` holding elements, `<option>` outside a select ...).
    /// `attrs`: the element's attributes (already moved out of `d`).
    pub(crate) fn from_element(d: &Div, attrs: &Attrs, key: Key, state: &UiState) -> Option<WidgetSpec> {
        let num = |k: &str| attr(attrs, k).and_then(|v| v.trim().parse::<f64>().ok());
        let checked = || {
            state.checked.get(&key).copied().unwrap_or(attr(attrs, "checked").is_some())
        };
        let mut s;
        match d.tag {
            "button" => {
                s = WidgetSpec::new(WKind::Button, attrs);
                s.label = text_of(&d.children)?;
            }
            "input" => {
                let ty = attr(attrs, "type").unwrap_or("text").to_ascii_lowercase();
                let value = attr(attrs, "value").map(str::to_owned);
                match ty.as_str() {
                    "button" | "submit" | "reset" => {
                        s = WidgetSpec::new(WKind::Button, attrs);
                        s.label = value.unwrap_or_else(|| match ty.as_str() {
                            "submit" => "Submit".into(),
                            "reset" => "Reset".into(),
                            _ => String::new(),
                        });
                    }
                    "file" => {
                        s = WidgetSpec::new(WKind::Button, attrs);
                        s.label = "Browse\u{2026}".into();
                    }
                    "checkbox" => {
                        s = WidgetSpec::new(WKind::Checkbox, attrs);
                        s.value = Some(checked() as i32 as f64);
                    }
                    "radio" => {
                        s = WidgetSpec::new(WKind::Radio, attrs);
                        s.value = Some(checked() as i32 as f64);
                    }
                    "number" => {
                        s = WidgetSpec::new(WKind::Number, attrs);
                        s.value = num("value");
                    }
                    "range" => {
                        s = WidgetSpec::new(WKind::Slider, attrs);
                        if s.min.is_infinite() {
                            s.min = 0.0;
                        }
                        if s.max.is_infinite() {
                            s.max = 100.0;
                        }
                        s.value = Some(num("value").unwrap_or((s.min + s.max) / 2.0));
                    }
                    "color" => {
                        s = WidgetSpec::new(WKind::Color, attrs);
                        s.color = Some(value.as_deref().and_then(parse_color)
                            .unwrap_or(crate::color::black()));
                    }
                    "image" | "hidden" => return None,
                    _ => {
                        // text, password, email, url, search, tel, date, time ...
                        s = WidgetSpec::new(WKind::Text { mask: ty == "password" }, attrs);
                        s.text = value;
                    }
                }
            }
            "select" => {
                s = WidgetSpec::new(WKind::Select, attrs);
                let mut selected = None;
                let want = attr(attrs, "value");
                fn options<'a>(c: &'a [AnyElement], out: &mut Vec<&'a Div>, groups: &mut Vec<(usize, String)>) {
                    for e in c {
                        if let AnyElement::Element(d) = e {
                            match d.tag {
                                "option" => out.push(d),
                                "optgroup" => {
                                    groups.push((out.len(), d.get_attr("label").unwrap_or("").to_owned()));
                                    options(&d.children, out, groups)
                                }
                                _ => {}
                            }
                        }
                    }
                }
                let mut opts = Vec::new();
                options(&d.children, &mut opts, &mut s.groups);
                for (k, o) in opts.iter().enumerate() {
                    let label = text_of(&o.children).unwrap_or_default();
                    let value = o.get_attr("value").map(str::to_owned).unwrap_or_else(|| label.clone());
                    if o.get_attr("selected").is_some() || want == Some(value.as_str()) {
                        selected = Some(k);
                    }
                    s.items.push(label);
                }
                s.value = Some(selected.unwrap_or(0) as f64);
            }
            "textarea" => {
                s = WidgetSpec::new(WKind::TextArea, attrs);
                s.edge_jump = attr(attrs, "bui-edge-jump").is_some();
                s.text = Some(attr(attrs, "value").map(str::to_owned)
                    .unwrap_or_else(|| d.children.iter().filter_map(|c| match c {
                        AnyElement::Text(t) => Some(t.as_ref()),
                        _ => None,
                    }).collect()));
            }
            "progress" => {
                s = WidgetSpec::new(WKind::Progress, attrs);
                let max = num("max").unwrap_or(1.0);
                // no value: indeterminate (a negative factor)
                s.value = Some(match num("value") {
                    Some(v) => (v / max.max(1e-9)).clamp(0.0, 1.0),
                    None => -1.0,
                });
                s.label = text_of(&d.children).unwrap_or_default();
            }
            "meter" => {
                s = WidgetSpec::new(WKind::Meter, attrs);
                s.min = num("min").unwrap_or(0.0);
                s.max = num("max").unwrap_or(1.0);
                s.value = Some(num("value").unwrap_or(0.0).clamp(s.min, s.max));
                s.meter = [num("low"), num("high"), num("optimum")];
                s.label = text_of(&d.children).unwrap_or_default();
            }
            _ => return None,
        }
        Some(s)
    }
}

type Attrs = [(SharedString, SharedString)];

fn attr<'a>(attrs: &'a Attrs, name: &str) -> Option<&'a str> {
    attrs.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.as_ref())
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

pub(crate) struct WidgetNode {
    pub node: *mut sys::BKNode,
    pub spec: WidgetSpec,
    seen: bool,
    /// The user changed the text since the last render, and the app heard it: the next render's
    /// value wins even when it is the one the app gave before (see [`Widgets::edited`]).
    edited: bool,
}

/// The kit widgets of one window, keyed by element key.
pub(crate) struct Widgets {
    pub nodes: HashMap<Key, WidgetNode>,
    /// Set while Rust writes values, so the kit's change echoes are ignored.
    pub syncing: bool,
}

impl Widgets {
    pub(crate) fn new() -> Widgets {
        Widgets { nodes: HashMap::new(), syncing: false }
    }

    /// Ensure the kit node for `key` exists and matches `spec`; returns its
    /// natural size in logical pixels.
    pub(crate) fn ensure(&mut self, parent: *mut sys::BKNode, key: Key, spec: &WidgetSpec, scale: f32)
        -> (f32, f32) {
        if self.nodes.get(&key).is_some_and(|w| w.spec.kind != spec.kind) {
            let w = self.nodes.remove(&key).unwrap();
            // SAFETY: the node is ours and attached to `parent`.
            unsafe { sys::BK_node_free(w.node) };
        }
        self.syncing = true;
        let entry = self.nodes.entry(key).or_insert_with(|| {
            // SAFETY: plain constructor calls on a fresh node.
            let node = unsafe { sys::BK_node_new(spec.kind.class().as_ptr()) };
            unsafe {
                sys::BK_node_add(parent, node, -1);
                sys::BK_geom_set_manager(node, sys::BK_GEOM_PLACE);
            }
            let w = WidgetNode { node, spec: spec.clone(), seen: true, edited: false };
            sync(node, None, spec);
            w
        });
        let text_was = entry.spec.text.clone();
        if entry.spec != *spec {
            sync(entry.node, Some(&entry.spec), spec);
            entry.spec = spec.clone();
        }
        // A controlled text box: the app heard the user's edit and rendered the same value as
        // before (it dropped a typed line break, say), so the spec didn't change, but the box
        // still shows the edit. Put the app's value back, as a browser's controlled input does.
        if std::mem::take(&mut entry.edited)
            && let Some(t) = spec.text.as_deref().filter(|t| text_was.as_deref() == Some(*t))
            && matches!(spec.kind, WKind::Text { .. } | WKind::TextArea)
        {
            let n = entry.node;
            // SAFETY: live node; the returned string is copied at once.
            let shown = unsafe { CStr::from_ptr(sys::BK_get_str(n, c"value".as_ptr())).to_string_lossy().into_owned() };
            if shown != t {
                let prop = if spec.kind == WKind::TextArea { c"value" } else { c"text" };
                let c = cstr(t);
                unsafe { sys::BK_set_str(n, prop.as_ptr(), c.as_ptr()) };
            }
        }
        entry.seen = true;
        self.syncing = false;
        let mut nat = [0i32; 2];
        // SAFETY: valid node, two-int out array.
        unsafe { sys::BK_node_natural(entry.node, nat.as_mut_ptr()) };
        // browser-look controls keep the browser's sizes
        let (mw, mh) = if spec.html { (0.0, 0.0) } else { spec.kind.min_size() };
        ((nat[0] as f32 / scale).max(mw), (nat[1] as f32 / scale).max(mh))
    }

    /// Place every widget at its laid-out bounds; free those not rendered.
    pub(crate) fn place(&mut self, at: &HashMap<Key, (Bounds<f32>, Option<Bounds<f32>>, bool)>,
                        scale: f32) {
        self.nodes.retain(|key, w| {
            if !w.seen || !at.contains_key(key) {
                // SAFETY: frees our node (detaching it from the canvas).
                unsafe { sys::BK_node_free(w.node) };
                return false;
            }
            w.seen = false;
            let (b, clip, visible) = at[key];
            let shown = visible && clip.is_none_or(|c| {
                let i = c.intersect(&b);
                i.size.width > 0.0 && i.size.height > 0.0
            });
            unsafe {
                sys::BK_geom_set(w.node, c"x".as_ptr(), (b.origin.x * scale).round() as f64);
                sys::BK_geom_set(w.node, c"y".as_ptr(), (b.origin.y * scale).round() as f64);
                sys::BK_geom_set(w.node, c"width".as_ptr(), (b.size.width * scale).round() as f64);
                sys::BK_geom_set(w.node, c"height".as_ptr(), (b.size.height * scale).round() as f64);
                sys::BK_set_num(w.node, c"visible".as_ptr(), shown as i32 as f64);
            }
            true
        });
    }

    pub(crate) fn key_of(&self, node: *mut sys::BKNode) -> Option<Key> {
        self.nodes.iter().find(|(_, w)| w.node == node).map(|(k, _)| *k)
    }

    /// The control's current value, as an HTML `change` event sees it.
    pub(crate) fn read(&self, key: Key) -> ChangeEvent {
        let Some(w) = self.nodes.get(&key) else { return ChangeEvent::default() };
        let n = w.node;
        // SAFETY: live node; returned strings are copied immediately.
        unsafe {
            let num = sys::BK_get_num(n, c"value".as_ptr());
            let text = CStr::from_ptr(sys::BK_get_str(n, c"value".as_ptr())).to_string_lossy().into_owned();
            match w.spec.kind {
                WKind::Text { .. } | WKind::TextArea => ChangeEvent { value: text, checked: false, number: 0.0 },
                WKind::Checkbox | WKind::Radio => ChangeEvent {
                    value: if num != 0.0 { "on".into() } else { String::new() },
                    checked: num != 0.0,
                    number: num,
                },
                WKind::Select => {
                    let num = w.spec.from_kit(num);
                    ChangeEvent { value: w.spec.items.get(num as usize).cloned().unwrap_or_default(), checked: false,
                                  number: num }
                }
                WKind::Color => {
                    let mut c = [0f32; 4];
                    sys::BK_get_color(n, c"value".as_ptr(), c.as_mut_ptr());
                    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                    ChangeEvent { value: format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2])),
                                  checked: false, number: 0.0 }
                }
                _ => ChangeEvent { value: format_num(num), checked: false, number: num },
            }
        }
    }

    /// Radio groups: turning one on turns the others with its name off.
    pub(crate) fn radio_group(&self, key: Key) -> Vec<Key> {
        let Some(name) = self.nodes.get(&key).and_then(|w| w.spec.name.clone()) else { return vec![] };
        self.nodes.iter()
            .filter(|(k, w)| **k != key && w.spec.kind == WKind::Radio && w.spec.name.as_ref() == Some(&name))
            .map(|(k, _)| *k)
            .collect()
    }

    /// The text box `key` was edited by the user and a listener heard it: its next render puts
    /// the app's value back if the box shows something else.
    pub(crate) fn edited(&mut self, key: Key) {
        if let Some(w) = self.nodes.get_mut(&key)
            && matches!(w.spec.kind, WKind::Text { .. } | WKind::TextArea)
        {
            w.edited = true;
        }
    }

    /// The kit node of the textarea `key`.
    pub(crate) fn textarea(&self, key: Key) -> Option<*mut sys::BKNode> {
        self.nodes.get(&key).filter(|w| w.spec.kind == WKind::TextArea).map(|w| w.node)
    }

    pub(crate) fn set_value(&mut self, key: Key, v: f64) {
        if let Some(w) = self.nodes.get(&key) {
            self.syncing = true;
            unsafe { sys::BK_set_num(w.node, c"value".as_ptr(), v) };
            self.syncing = false;
        }
    }

    pub(crate) fn free_all(&mut self) {
        for (_, w) in self.nodes.drain() {
            unsafe { sys::BK_node_free(w.node) };
        }
    }
}

fn format_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 { format!("{}", v as i64) } else { format!("{v}") }
}

/// Push the properties of `spec` that differ from `old` into the kit node.
fn sync(n: *mut sys::BKNode, old: Option<&WidgetSpec>, spec: &WidgetSpec) {
    // SAFETY: `n` is a live node; C strings outlive each call.
    unsafe {
        let set_num = |k: &CStr, v: f64| { sys::BK_set_num(n, k.as_ptr(), v); };
        let set_str = |k: &CStr, v: &str| { let c = cstr(v); sys::BK_set_str(n, k.as_ptr(), c.as_ptr()); };
        if old.is_none_or(|o| o.html != spec.html) {
            set_num(c"html", spec.html as i32 as f64);
        }
        if old.is_none_or(|o| o.disabled != spec.disabled) {
            set_num(c"enabled", (!spec.disabled) as i32 as f64);
        }
        if old.is_none_or(|o| o.label != spec.label) && !spec.label.is_empty() {
            match spec.kind {
                WKind::Button | WKind::Progress => set_str(c"text", &spec.label),
                _ => set_str(c"label", &spec.label),
            }
        }
        // new nodes start at the kit's size (0) and without the jump
        if old.map_or(0.0, |o| o.font_scale) != spec.font_scale {
            set_num(c"font_scale", f64::from(spec.font_scale));
        }
        if old.is_some_and(|o| o.edge_jump) != spec.edge_jump {
            set_num(c"edge_jump", spec.edge_jump as i32 as f64);
        }
        if old.is_none_or(|o| o.placeholder != spec.placeholder) && !spec.placeholder.is_empty() {
            set_str(c"placeholder", &spec.placeholder);
        }
        if let WKind::Text { mask: true } = spec.kind {
            if old.is_none() {
                set_num(c"mask", 1.0);
            }
        }
        if spec.kind == WKind::Meter && old.is_none_or(|o| (o.min, o.max, o.meter) != (spec.min, spec.max, spec.meter)) {
            set_num(c"min", spec.min);
            set_num(c"max", spec.max);
            for (k, v) in [c"low", c"high", c"optimum"].iter().zip(spec.meter) {
                if let Some(v) = v {
                    set_num(k, v);
                }
            }
        }
        if old.is_none_or(|o| (o.min, o.max, o.step) != (spec.min, spec.max, spec.step))
            && matches!(spec.kind, WKind::Number | WKind::Slider)
        {
            if spec.min.is_finite() {
                set_num(c"min", spec.min);
            }
            if spec.max.is_finite() {
                set_num(c"max", spec.max);
            }
            set_num(c"step", spec.step);
            if spec.step.fract() == 0.0 {
                set_num(c"precision", 0.0);
            }
        }
        if old.is_none_or(|o| (&o.items, &o.groups, o.html) != (&spec.items, &spec.groups, spec.html))
            && spec.kind == WKind::Select {
            let items = spec.kit_items();
            let cs: Vec<CString> = items.iter().map(|s| cstr(s)).collect();
            let ptrs: Vec<*const std::ffi::c_char> = cs.iter().map(|c| c.as_ptr()).collect();
            sys::BK_set_strs(n, c"items".as_ptr(), ptrs.as_ptr(), ptrs.len() as i32);
        }
        if old.is_none_or(|o| o.text != spec.text) {
            if let Some(t) = &spec.text {
                // the textarea takes its whole text as "value"
                set_str(if spec.kind == WKind::TextArea { c"value" } else { c"text" }, t);
            }
        }
        if old.is_none_or(|o| (o.value, o.html) != (spec.value, spec.html)) {
            if let Some(v) = spec.value {
                let v = match spec.kind {
                    WKind::Select => spec.to_kit(v),
                    // indeterminate progress exists in the browser look only
                    WKind::Progress if !spec.html => v.max(0.0),
                    _ => v,
                };
                set_num(if spec.kind == WKind::Progress { c"factor" } else { c"value" }, v);
            }
        }
        if old.is_none_or(|o| o.color != spec.color) {
            if let Some(c) = spec.color {
                sys::BK_set_color(n, c"value".as_ptr(), c.to_array().as_ptr());
            }
        }
    }
}
