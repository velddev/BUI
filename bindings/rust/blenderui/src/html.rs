//! Every HTML element as a gpui-style constructor (`div()`, `section()`,
//! `table()` ...), all building the one [`Div`] type, plus the user-agent
//! stylesheet that gives them their default look (Blender's theme).

use std::sync::OnceLock;

use crate::css::Stylesheet;
use crate::element::Div;

/// How an element is realised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagKind {
    /// Laid out and painted from CSS (most elements).
    Box,
    /// A real kit widget: Blender's control with Blender's interaction.
    Widget,
    /// Replaced content with an intrinsic size (`img`, `video` ...).
    Replaced,
    /// Metadata: not rendered (`head`, `script`, `meta` ...).
    Metadata,
    /// Line break / break opportunity.
    Break,
}

macro_rules! tags {
    ($($kind:ident: $($tag:ident),+;)*) => {
        /// Every standard HTML element (WHATWG living standard) and how BUI realises it.
        pub const TAGS: &[(&str, TagKind)] = &[$($((stringify!($tag), TagKind::$kind),)+)*];
        $($(
            #[doc = concat!("The `<", stringify!($tag), ">` element.")]
            pub fn $tag() -> Div { Div::new(stringify!($tag)) }
        )+)*
    };
}

tags! {
    Box: html, body, article, section, nav, aside, h1, h2, h3, h4, h5, h6, hgroup, header,
         footer, address, p, hr, pre, blockquote, ol, ul, menu, li, dl, dt, dd, figure,
         figcaption, main, search, div, a, em, strong, small, s, cite, q, dfn, abbr, ruby, rt, rp,
         data, time, code, var, samp, kbd, sub, sup, i, b, u, mark, bdi, bdo, span, ins, del,
         picture, table, caption, colgroup, col, tbody, thead, tfoot, tr, td, th, form, label,
         fieldset, legend, details, summary, dialog, noscript, slot, output, map, selectedcontent,
         svg, math;
    Widget: button, input, select, option, optgroup, textarea, progress, meter, datalist;
    Replaced: img, iframe, embed, object, video, audio;
    Metadata: head, title, base, link, meta, style, script, template, source, track, area;
    Break: br, wbr;
}

/// `<canvas>`: painted by Rust code, see [`crate::canvas`] (gpui's `canvas()`).
pub const CANVAS_TAG: (&str, TagKind) = ("canvas", TagKind::Replaced);

/// Elements in the WHATWG HTML element index, all constructible here.
pub const HTML_ELEMENT_COUNT: usize = 113;

/// How `tag` is realised (unknown / custom elements are boxes, as in browsers).
pub fn tag_kind(tag: &str) -> TagKind {
    if tag == "canvas" {
        return TagKind::Replaced;
    }
    TAGS.iter().find(|(t, _)| *t == tag).map(|(_, k)| *k).unwrap_or(TagKind::Box)
}

/// A custom element (`custom("my-widget")`), laid out like a `<span>`.
pub fn custom(tag: &'static str) -> Div {
    Div::new(tag)
}

/// The user-agent stylesheet: the browsers' defaults (WHATWG rendering
/// section: black text on white). An element with the `styled` attribute
/// (`div().styled()`, `<div styled>`) and everything inside it get Blender's
/// theme (dark, or light with `BUI_THEME` / `set_theme`) instead. Outside
/// `styled`, form controls are drawn like a browser's; inside it they are
/// Blender's widgets, and the Blender components always are.
pub const UA_CSS: &str = r#"
html { display: block; background-color: #ffffff; color: #000000; font-size: 16px; }
body { display: block; margin: 8px; }
address, article, aside, blockquote, dd, details, dialog, div, dl, dt, fieldset, figcaption,
figure, footer, form, h1, h2, h3, h4, h5, h6, header, hgroup, hr, legend, main, menu, nav, ol,
p, pre, search, section, ul, noscript, optgroup, option { display: block; }
head, title, base, link, meta, style, script, template, source, track, area, datalist, wbr,
map, [hidden], input[type=hidden], col, colgroup { display: none; }
li, summary { display: list-item; }
p, dl, pre { margin: 1em 0; }
figure, blockquote { margin: 1em 40px; }
h1 { font-size: 2em; margin: 0.67em 0; font-weight: bold; }
h2 { font-size: 1.5em; margin: 0.83em 0; font-weight: bold; }
h3 { font-size: 1.17em; margin: 1em 0; font-weight: bold; }
h4 { font-size: 1em; margin: 1.33em 0; font-weight: bold; }
h5 { font-size: 0.83em; margin: 1.67em 0; font-weight: bold; }
h6 { font-size: 0.67em; margin: 2.33em 0; font-weight: bold; }
b, strong, th, dt, caption { font-weight: bold; }
i, em, cite, dfn, var, address { font-style: italic; }
u, ins { text-decoration: underline; }
s, del { text-decoration: line-through; }
small { font-size: smaller; }
sub { vertical-align: sub; font-size: smaller; }
sup { vertical-align: super; font-size: smaller; }
code, kbd, samp, pre, var { font-family: monospace; }
pre { white-space: pre; }
mark { background-color: #ffff00; color: #000000; }
a { color: #0000ee; text-decoration: underline; cursor: pointer; }
abbr[title] { text-decoration: underline dotted; }
rt { font-size: 50%; }
ul, menu { list-style-type: disc; padding-left: 40px; margin: 1em 0; }
ol { list-style-type: decimal; padding-left: 40px; margin: 1em 0; }
ul ul, ol ul, menu ul { list-style-type: circle; margin: 0; }
ul ul ul, ol ol ul { list-style-type: square; }
ol ol, ul ol { margin: 0; }
dd { margin-left: 40px; }
hr { border: 1px inset #808080; margin: 0.5em 0; }
table { display: table; border-spacing: 2px; }
thead, tbody, tfoot { display: table-row-group; }
tr { display: table-row; }
td, th { display: table-cell; padding: 1px; }
th { text-align: center; }
caption { display: table-caption; text-align: center; }
fieldset { margin: 0 2px; padding: 0.35em 0.75em 0.625em; border: 2px groove #c0c0c0; }
legend { padding: 0 2px; }
summary { cursor: pointer; }
dialog { position: absolute; left: 10%; right: 10%; top: 15%; padding: 1em;
         background-color: #ffffff; color: #000000; border: 2px solid #000000; z-index: 100; }
dialog:not([open]) { display: none; }
img, video, iframe, embed, object, audio, canvas, svg, math { display: inline-block; }
button, input, select, textarea, progress, meter { display: inline-block; }
audio:not([controls]) { display: none; }
iframe { border: 2px inset #808080; }
label { cursor: default; }
output { display: inline; }
bui-tooltip { display: block; position: fixed; z-index: 1000; pointer-events: none; max-width: 400px;
              padding: 4px 8px; background-color: #191919; color: #e6e6e6; border: 1px solid #000000;
              border-radius: 4px; font-size: 12px; white-space: pre-line;
              box-shadow: 0 2px 6px rgba(0, 0, 0, 0.4); }

"#;

/// The user-agent stylesheet inside `styled` subtrees: the same defaults in
/// Blender's dark theme. The `styled` element itself gets the theme's
/// background, text colour and 13px UI font size.
pub const UA_STYLED_CSS: &str = r#"
html { display: block; }
[styled] { background-color: #303030; color: #e6e6e6; font-size: 13px; font-family: system-ui; }
body { display: block; margin: 8px; }
address, article, aside, blockquote, dd, details, dialog, div, dl, dt, fieldset, figcaption,
figure, footer, form, h1, h2, h3, h4, h5, h6, header, hgroup, hr, legend, main, menu, nav, ol,
p, pre, search, section, ul, noscript, optgroup, option { display: block; }
head, title, base, link, meta, style, script, template, source, track, area, datalist, wbr,
map, [hidden], input[type=hidden], col, colgroup { display: none; }
li, summary { display: list-item; }
p, dl, figure, pre { margin: 0.6em 0; }
blockquote { margin: 0.6em 0; padding: 2px 0 2px 12px; border-left: 3px solid #4772b3; color: #b8b8b8; }
h1 { font-size: 2em; margin: 0.5em 0; font-weight: bold; }
h2 { font-size: 1.5em; margin: 0.5em 0; font-weight: bold; }
h3 { font-size: 1.17em; margin: 0.6em 0; font-weight: bold; }
h4 { font-size: 1em; margin: 0.8em 0; font-weight: bold; }
h5 { font-size: 0.83em; margin: 1em 0; font-weight: bold; }
h6 { font-size: 0.67em; margin: 1.2em 0; font-weight: bold; }
b, strong, th, legend, dt, caption { font-weight: bold; }
i, em, cite, dfn, var, address { font-style: italic; }
u, ins { text-decoration: underline; }
s, del { text-decoration: line-through; color: #999999; }
small { font-size: smaller; }
sub { vertical-align: sub; font-size: smaller; }
sup { vertical-align: super; font-size: smaller; }
code, kbd, samp, pre, var { font-family: monospace; }
code, kbd, samp { background-color: #232323; border-radius: 3px; }
kbd { border: 1px solid #555555; padding: 0 3px; }
pre { white-space: pre; background-color: #232323; padding: 6px 8px; border-radius: 4px; }
mark { background-color: #e6a23c; color: #1d1d1d; }
a { color: #5294e2; text-decoration: underline; cursor: pointer; }
a:hover { color: #78aef0; }
abbr[title] { text-decoration: underline; }
q { font-style: italic; }
rt { font-size: 50%; }
ul, menu { list-style-type: disc; padding-left: 28px; margin: 0.6em 0; }
ol { list-style-type: decimal; padding-left: 28px; margin: 0.6em 0; }
ul ul, ol ul, menu ul { list-style-type: circle; margin: 0; }
ul ul ul, ol ol ul { list-style-type: square; }
ol ol, ul ol { margin: 0; }
dd { margin-left: 28px; }
hr { border-top: 1px solid #484848; margin: 8px 0; }
table { display: table; border-spacing: 2px; }
thead, tbody, tfoot { display: table-row-group; }
tr { display: table-row; }
td, th { display: table-cell; padding: 2px 6px; }
th { text-align: center; }
thead { background-color: #282828; }
caption { display: table-caption; text-align: center; padding: 2px; }
fieldset { margin: 4px 0; padding: 6px 10px 8px; border: 1px solid #484848; border-radius: 4px; }
legend { padding: 0 2px; margin-bottom: 4px; }
details { margin: 2px 0; }
summary { cursor: pointer; font-weight: bold; padding: 2px 0; }
dialog { position: absolute; left: 10%; right: 10%; top: 15%; padding: 12px 16px;
         background-color: #353535; border: 1px solid #555555; border-radius: 6px;
         box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5); z-index: 100; }
dialog:not([open]) { display: none; }
figcaption { font-size: 0.9em; color: #b8b8b8; }
img, video, iframe, embed, object, audio, canvas, svg, math { display: inline-block; }
button, input, select, textarea, progress, meter { display: inline-block; }
input, textarea { font-size: 11px; }
audio:not([controls]) { display: none; }
iframe, embed, object, video { border: 1px solid #484848; }
label { cursor: default; }
output { display: inline; }
"#;

/// The parsed user-agent stylesheet (shared, parsed once).
pub fn ua_stylesheet() -> &'static Stylesheet {
    static SHEET: OnceLock<Stylesheet> = OnceLock::new();
    SHEET.get_or_init(|| Stylesheet::parse(UA_CSS))
}

/// [`UA_STYLED_CSS`]'s colours in Blender's Light theme (Blender_Light.xml:
/// the properties editor's #b3b3b3 back and dark text), in sheet order.
const UA_STYLED_LIGHT: &[(&str, &str)] = &[
    ("background-color: #303030; color: #e6e6e6;", "background-color: #b3b3b3; color: #1a1a1a;"),
    ("color: #b8b8b8; }", "color: #3d3d3d; }"),
    ("color: #999999; }", "color: #4d4d4d; }"),
    ("background-color: #232323; border-radius: 3px;", "background-color: #c7c7c7; border-radius: 3px;"),
    ("border: 1px solid #555555; padding: 0 3px;", "border: 1px solid #8c8c8c; padding: 0 3px;"),
    ("background-color: #232323; padding: 6px 8px;", "background-color: #c7c7c7; padding: 6px 8px;"),
    ("a { color: #5294e2;", "a { color: #1f5bb5;"),
    ("a:hover { color: #78aef0; }", "a:hover { color: #3a78d6; }"),
    ("border-top: 1px solid #484848;", "border-top: 1px solid #8c8c8c;"),
    ("thead { background-color: #282828; }", "thead { background-color: #a6a6a6; }"),
    ("border: 1px solid #484848; border-radius: 4px;", "border: 1px solid #8c8c8c; border-radius: 4px;"),
    ("background-color: #353535; border: 1px solid #555555;", "background-color: #cccccc; border: 1px solid #8c8c8c;"),
    ("box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);", "box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);"),
    ("figcaption { font-size: 0.9em; color: #b8b8b8; }", "figcaption { font-size: 0.9em; color: #3d3d3d; }"),
    ("iframe, embed, object, video { border: 1px solid #484848; }", "iframe, embed, object, video { border: 1px solid #8c8c8c; }"),
];

/// The `styled` user-agent stylesheet for a theme (`"dark"` or `"light"`).
pub fn ua_styled_css(theme: &str) -> String {
    let mut css = UA_STYLED_CSS.to_string();
    if theme == "light" {
        for (dark, light) in UA_STYLED_LIGHT {
            debug_assert!(css.contains(dark), "UA_STYLED_LIGHT out of date: {dark}");
            css = css.replacen(dark, light, 1);
        }
    }
    css
}

/// The parsed `styled` sheet for the active theme ([`crate::set_theme`],
/// `BUI_THEME`): [`UA_STYLED_CSS`] or its light variant (each parsed once).
pub fn ua_styled_stylesheet() -> &'static Stylesheet {
    static DARK: OnceLock<Stylesheet> = OnceLock::new();
    static LIGHT: OnceLock<Stylesheet> = OnceLock::new();
    if crate::theme_name() == "light" {
        LIGHT.get_or_init(|| Stylesheet::parse(&ua_styled_css("light")))
    } else {
        DARK.get_or_init(|| Stylesheet::parse(UA_STYLED_CSS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_standard_element_exists() {
        // the WHATWG element index (113 with <canvas>) plus the svg / math roots
        let html = TAGS.iter().filter(|(t, _)| !matches!(*t, "svg" | "math")).count();
        assert_eq!(html + 1, HTML_ELEMENT_COUNT);
        let mut names: Vec<&str> = TAGS.iter().map(|(t, _)| *t).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), TAGS.len(), "duplicate tags");
        assert!(ua_stylesheet().rules.len() > 80);
    }
}
