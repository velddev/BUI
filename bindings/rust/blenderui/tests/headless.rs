//! Off-screen windows: layout, painting and interaction end to end.

use blenderui::prelude::*;

fn pixel(img: &(usize, usize, Vec<u32>), x: usize, y: usize) -> u32 {
    img.2[y * img.0 + x]
}

fn open<V: Render>(cx: &mut App, w: f32, h: f32, v: V) -> WindowHandle<V> {
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(w), px(h)),
        })),
        ..Default::default()
    };
    cx.open_window(opts, |_, cx| cx.new(|_| v)).unwrap()
}

struct Boxes;

impl Render for Boxes {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        body()
            .m_0()
            .child(div().id("red").w(px(50.0)).h(px(30.0)).bg(rgb(0xff0000)))
            .child(
                div().id("row").flex().gap(px(10.0)).child(div().id("a").size(px(20.0)).bg(rgb(0x00ff00)))
                    .child(div().id("b").flex_1().h(px(20.0)).bg(rgb(0x0000ff))),
            )
            .child(div().id("styled").style("width: 40px; height: 10px; background: #ffff00; margin-left: 5px"))
    }
}

#[test]
fn block_and_flex_layout_paint() {
    Application::headless().run(|cx| {
        let win = open(cx, 200.0, 100.0, Boxes);
        let id = win.id;
        let img = cx.draw(id);
        assert!(img.0 > 0);
        let s = img.0 as f32 / 200.0; // device pixels per logical pixel
        let b = cx.element_bounds(id, "red").unwrap();
        assert_eq!((b.origin.x, b.origin.y, b.size.width, b.size.height), (0.0, 0.0, 50.0, 30.0));
        let a = cx.element_bounds(id, "a").unwrap();
        let bb = cx.element_bounds(id, "b").unwrap();
        assert_eq!(a.origin.y, 30.0);
        assert_eq!(bb.origin.x, 30.0);
        assert_eq!(bb.size.width, 170.0);
        let st = cx.element_bounds(id, "styled").unwrap();
        assert_eq!((st.origin.x, st.origin.y, st.size.width), (5.0, 50.0, 40.0));
        let at = |x: f32, y: f32| pixel(&img, (x * s) as usize, (y * s) as usize);
        assert_eq!(at(10.0, 10.0), 0xff0000);
        assert_eq!(at(35.0, 40.0), 0x0000ff);
        assert_eq!(at(10.0, 40.0), 0x00ff00);
        assert_eq!(at(20.0, 55.0), 0xffff00);
        assert_eq!(at(150.0, 90.0), 0xffffff); // html background: browser default
    });
}

struct Counter {
    count: i32,
    changed: String,
}

impl Render for Counter {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(
                div().id("plus").w(px(60.0)).h(px(20.0)).bg(rgb(0x4772b3))
                    .hover(|s| s.bg(rgb(0xffffff)))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.count += 1;
                        cx.notify();
                    })),
            )
            .child(p().id("label").child(format!("count {}", self.count)))
            .child(button().id("btn").child("Add").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.count += 10;
                cx.notify();
            })))
            .child(input().id("check").input_type("checkbox").on_change(cx.listener(
                |this, e: &ChangeEvent, _, cx| {
                    this.changed = format!("checked={}", e.checked);
                    cx.notify();
                },
            )))
    }
}

#[test]
fn clicks_hover_and_widgets() {
    Application::headless().run(|cx| {
        let win = open(cx, 300.0, 200.0, Counter { count: 0, changed: String::new() });
        let id = win.id;
        cx.draw(id);
        let plus = cx.element_bounds(id, "plus").unwrap();
        cx.simulate_click(id, plus.origin.x + 5.0, plus.origin.y + 5.0);
        cx.simulate_click(id, plus.origin.x + 5.0, plus.origin.y + 5.0);
        assert_eq!(win.root().read(cx).count, 2);
        // :hover style from .hover(|s| ...)
        let img = cx.draw(id);
        let s = img.0 as f32 / 300.0;
        let at = |x: f32, y: f32| pixel(&img, (x * s) as usize, (y * s) as usize);
        assert_eq!(at(plus.origin.x + 30.0, plus.origin.y + 10.0), 0xffffff);
        // a real Blender button
        let btn = cx.element_bounds(id, "btn").unwrap();
        assert!(btn.size.width > 20.0 && btn.size.height >= 18.0, "{btn:?}");
        cx.simulate_click(id, btn.origin.x + btn.size.width / 2.0, btn.origin.y + btn.size.height / 2.0);
        assert_eq!(win.root().read(cx).count, 12);
        let check = cx.element_bounds(id, "check").unwrap();
        cx.simulate_click(id, check.origin.x + 8.0, check.origin.y + check.size.height / 2.0);
        assert_eq!(win.root().read(cx).changed, "checked=true");
    });
}

struct Page;

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        html()
            .child(head().child(style().child(
                "body { margin: 0 } .card > p:first-child { color: #ff0000 } #wrap { width: 100px; }
                 ul li:nth-child(2) { height: 33px } @media (max-width: 100px) { #wrap { width: 1px } }",
            )))
            .child(body().child(
                div().id("wrap")
                    .child(div().class("card").child(p().id("first").child("one two three four five six seven")))
                    .child(ul().child(li().child("a")).child(li().id("second").child("b"))),
            ))
    }
}

#[test]
fn stylesheets_and_inline_wrapping() {
    Application::headless().run(|cx| {
        let win = open(cx, 400.0, 300.0, Page);
        let id = win.id;
        cx.draw(id);
        let wrap = cx.element_bounds(id, "wrap").unwrap();
        assert_eq!(wrap.size.width, 100.0);
        let first = cx.element_bounds(id, "first").unwrap();
        // seven words cannot fit 100px on one line: they wrap
        assert!(first.size.height > 30.0, "{first:?}");
        let second = cx.element_bounds(id, "second").unwrap();
        assert_eq!(second.size.height, 33.0);
    });
}

struct Tables;

impl Render for Tables {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        body().m_0()
            .child(style().child("#t { border-spacing: 10px 4px; width: 200px } caption { caption-side: bottom }
                                  #g { display: grid; grid: auto-flow 30px / 50px 50px; width: 100px }"))
            .child(table().id("t")
                .child(caption().id("cap").child("cap"))
                .child(tbody().child(tr().id("r1").child(td().id("c1").child("a")).child(td().id("c2").child("b")))
                    .child(tr().id("r2").child(td().child("c")).child(td().child("d")))))
            .child(div().id("g").children((0..4).map(|k| div().id(format!("g{k}")))))
    }
}

#[test]
fn tables_captions_and_grid_shorthand() {
    Application::headless().run(|cx| {
        let win = open(cx, 300.0, 300.0, Tables);
        let id = win.id;
        cx.draw(id);
        let c1 = cx.element_bounds(id, "c1").unwrap();
        let c2 = cx.element_bounds(id, "c2").unwrap();
        let r1 = cx.element_bounds(id, "r1").unwrap();
        let r2 = cx.element_bounds(id, "r2").unwrap();
        assert_eq!(c1.origin.x, 10.0, "spacing pads the table");
        assert_eq!(c2.origin.x - (c1.origin.x + c1.size.width), 10.0, "horizontal spacing");
        assert_eq!(r2.origin.y - (r1.origin.y + r1.size.height), 4.0, "vertical spacing");
        let cap = cx.element_bounds(id, "cap").unwrap();
        assert!(cap.origin.y > r2.origin.y, "caption-side: bottom");
        let g3 = cx.element_bounds(id, "g3").unwrap();
        assert_eq!((g3.origin.x - cx.element_bounds(id, "g").unwrap().origin.x, g3.size.height), (50.0, 30.0));
    });
}

struct Paint;

impl Render for Paint {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        body().m_0().child(
            canvas(|b, p| {
                p.fill(b, rgb(0x232323));
                p.quad(b, 6.0, rgba(0x4772b380), rgb(0x4772b3), 1.0);
                p.line(&[(b.origin.x, b.origin.y), (b.origin.x + 80.0, b.origin.y + 40.0)], white(), 2.0);
                p.oval(Bounds { origin: point(150.0, 10.0), size: size(20.0, 20.0) }, red(), black(), 1.0);
                p.text(b.origin.x + 4.0, b.origin.y + 4.0, "hello", 13.0, white());
            })
            .id("cv")
            .w(px(200.0))
            .h(px(100.0)),
        )
    }
}

#[test]
fn canvas_painter() {
    Application::headless().run(|cx| {
        let win = open(cx, 220.0, 120.0, Paint);
        let img = cx.draw(win.id);
        let s = img.0 as f32 / 220.0;
        let at = |x: f32, y: f32| pixel(&img, (x * s) as usize, (y * s) as usize);
        assert_eq!(cx.element_bounds(win.id, "cv").unwrap().size.width, 200.0);
        assert_eq!(at(160.0, 20.0), 0xff0000, "oval");
        assert_ne!(at(100.0, 80.0), 0x303030, "filled");
    });
}

#[test]
fn css_macro_matches_runtime_css() {
    use blenderui::css::Prop;
    let compiled = css!("display: flex; padding: 4px 8px; color: #e6a23c !important; --x: 1");
    assert_eq!(compiled.len(), 4);
    assert_eq!(compiled[0].prop, Prop::named("display"));
    assert_eq!(compiled[2].prop, Prop::named("color"));
    assert!(compiled[2].important);
    assert!(compiled[3].prop.is_custom());
    assert_eq!(compiled[3].prop.name(), "--x");
    let parsed = blenderui::css::parse_declarations("display: flex; padding: 4px 8px; color: #e6a23c !important; --x: 1");
    for (a, b) in compiled.iter().zip(&parsed) {
        assert_eq!(a, b); // same tokens, whether parsed at compile time or run time
    }
}

struct SheetHover;

impl Render for SheetHover {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        body().m_0()
            .child(style().child(".b { background: #ff0000 } .b:hover { background: #00ff00 } .b:hover + .b { background: #0000ff }"))
            .child(div().class("b").id("one").w(px(40.0)).h(px(20.0)))
            .child(div().class("b").id("two").w(px(40.0)).h(px(20.0)))
    }
}

#[test]
fn stylesheet_hover_restyles_across_frames() {
    Application::headless().run(|cx| {
        let win = open(cx, 100.0, 60.0, SheetHover);
        let at = |cx: &mut App, x: f32, y: f32| {
            let img = cx.draw(win.id);
            let s = img.0 as f32 / 100.0;
            pixel(&img, (x * s) as usize, (y * s) as usize)
        };
        assert_eq!(at(cx, 10.0, 10.0), 0xff0000);
        assert_eq!(at(cx, 10.0, 30.0), 0xff0000);
        cx.simulate_mouse_move(win.id, 10.0, 10.0); // hover the first box
        assert_eq!(at(cx, 10.0, 10.0), 0x00ff00);
        assert_eq!(at(cx, 10.0, 30.0), 0x0000ff, "sibling selector sees the hover");
        cx.simulate_mouse_move(win.id, 90.0, 55.0); // leave
        assert_eq!(at(cx, 10.0, 10.0), 0xff0000);
        assert_eq!(at(cx, 10.0, 30.0), 0xff0000);
    });
}

/// A view whose layout changes from state to state.
struct Morph {
    step: usize,
}

impl Render for Morph {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let s = self.step;
        let labels: Vec<String> = match s % 5 {
            0 => vec!["a".into(), "bbbbbbbb".into(), "c".into()],
            1 => vec!["a".into(), "b".into(), "c".into()],
            2 => vec!["a".into(), "c".into()],
            3 => vec!["c".into(), "a".into(), "b b b b b b b b b b b b b".into(), "d".into()],
            _ => vec![format!("{s}"); 6],
        };
        body().m_0()
            .child(div().flex().gap(px(4.0)).p(px(4.0)).bg(rgb(0x202020))
                .children(labels.into_iter().enumerate().map(|(k, t)| {
                    div().id(format!("item{k}")).px(px(6.0)).py(px(2.0)).bg(rgb(0x4772b3 + k as u32 * 0x101010))
                        .rounded(px(3.0)).child(t)
                })))
            .child(div().w(px(if s % 2 == 0 { 120.0 } else { 220.0 })).h(px(20.0)).bg(rgb(0xe6a23c)))
            .child(p().w(px(150.0)).child(if s % 3 == 0 { "short" } else { "a much longer paragraph that wraps over several lines" }))
    }
}

#[test]
fn incremental_frames_match_fresh_renders() {
    Application::headless().run(|cx| {
        let live = open(cx, 400.0, 200.0, Morph { step: 0 });
        cx.draw(live.id);
        for step in 1..12 {
            live.root().update(cx, |m, _| m.step = step);
            cx.notify_all();
            let incremental = cx.draw(live.id);
            let fresh = open(cx, 400.0, 200.0, Morph { step });
            let reference = cx.draw(fresh.id);
            assert!(incremental.2 == reference.2, "step {step}: incremental frame differs from a fresh render");
        }
    });
}

struct Number {
    number: Option<f64>,
}

impl Render for Number {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        body().m_0().child(NumberField().id("n").attr("value", "5").on_change(cx.listener(
            |this, e: &ChangeEvent, _, cx| {
                this.number = Some(e.number);
                cx.notify();
            },
        )))
    }
}

/// Backspace over a Blender field goes back to the value it was made with (not to zero).
#[test]
fn backspace_resets_to_the_value_the_field_was_made_with() {
    Application::headless().run(|cx| {
        let win = open(cx, 300.0, 100.0, Number { number: None });
        let id = win.id;
        cx.draw(id);
        let n = cx.element_bounds(id, "n").unwrap();
        let (x, y) = (n.origin.x + n.size.width / 2.0, n.origin.y + n.size.height / 2.0);
        cx.simulate_drag(id, (x, y), (x + 60.0, y));
        let moved = win.root().read(cx).number.expect("the drag changes the value");
        assert!((moved - 5.0).abs() > 0.01, "the drag should change the value, got {moved}");
        cx.simulate_mouse_move(id, x, y);
        cx.simulate_key(id, "", "BackSpace");
        let back = win.root().read(cx).number.unwrap();
        assert!((back - 5.0).abs() < 1e-9, "Backspace should give 5, got {back}");
    });
}

struct TextBoxes;

impl Render for TextBoxes {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().styled()
            .child(textarea().id("plain").attr("rows", "2"))
            .child(textarea().id("big").attr("rows", "2").style("font-size: 22px"))
            .child(input().id("field"))
            .child(input().id("big-field").style("font-size: 22px"))
            .child(textarea().id("jump").attr("bui-edge-jump", "").value("one\ntwo\nthree"))
            .child(textarea().id("stay").value("one\ntwo"))
    }
}

#[test]
fn text_boxes_take_css_font_size() {
    Application::headless().run(|cx| {
        let win = open(cx, 400.0, 600.0, TextBoxes);
        let id = win.id;
        cx.draw(id);
        let h = |cx: &mut App, el: &str| cx.element_bounds(id, el).unwrap().size.height;
        // twice the kit's 11px: twice as tall (the styled sheet's default is the kit's size)
        assert_eq!(h(cx, "big"), 2.0 * h(cx, "plain"));
        assert_eq!(h(cx, "big-field"), 2.0 * h(cx, "field"));
    });
}

#[test]
fn textarea_caret_and_edge_jump() {
    Application::headless().run(|cx| {
        let win = open(cx, 400.0, 600.0, TextBoxes);
        let id = win.id;
        cx.draw(id);
        let caret = |cx: &mut App, el: &str| win.update(cx, |_, w, _| w.textarea_caret(el)).unwrap();
        let set = |cx: &mut App, el: &str, to: TextCaret| assert!(win.update(cx, |_, w, _| w.set_textarea_caret(el, to)).unwrap());
        let focus = |cx: &mut App, el: &str| {
            let b = cx.element_bounds(id, el).unwrap();
            cx.simulate_click(id, b.origin.x + 4.0, b.origin.y + 4.0);
        };
        assert_eq!(caret(cx, "field"), None, "only textareas");
        focus(cx, "jump");
        // "one\ntwo\nthree": Up on the first line goes to the start, Shift selects
        set(cx, "jump", TextCaret::at(2));
        assert_eq!(caret(cx, "jump"), Some(TextCaret::at(2)));
        cx.simulate_key_with(id, "", "Up", false, true);
        assert_eq!(caret(cx, "jump"), Some(TextCaret { anchor: 2, caret: 0 }));
        cx.simulate_key(id, "", "Up");
        assert_eq!(caret(cx, "jump"), Some(TextCaret::at(0)));
        // other lines move as before: "two" column 1 to "one" column 1
        set(cx, "jump", TextCaret::at(5));
        cx.simulate_key(id, "", "Up");
        assert_eq!(caret(cx, "jump"), Some(TextCaret::at(1)));
        // Down on the last line goes to the end
        set(cx, "jump", TextCaret { anchor: 0, caret: 9 });
        assert_eq!(caret(cx, "jump"), Some(TextCaret { anchor: 0, caret: 9 }));
        cx.simulate_key(id, "", "Down");
        assert_eq!(caret(cx, "jump"), Some(TextCaret::at(13)));
        // offsets are clamped to the text
        set(cx, "jump", TextCaret::at(99));
        assert_eq!(caret(cx, "jump"), Some(TextCaret::at(13)));
        // without the attribute Up on the first line keeps the column
        focus(cx, "stay");
        set(cx, "stay", TextCaret::at(2));
        cx.simulate_key(id, "", "Up");
        assert_eq!(caret(cx, "stay"), Some(TextCaret::at(2)));
    });
}

/// A box with a list over it: `bui-pass-keys` leaves its keys to the app (`on_key_down` still
/// sees them) and the box neither moves its caret nor edits; without it, as before.
struct PassKeys {
    pass: bool,
    keys: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
}

impl Render for PassKeys {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let keys = self.keys.clone();
        let mut t = textarea().id("box").value("one\ntwo");
        if self.pass {
            t = t.attr("bui-pass-keys", "Up Down Return Tab Escape");
        }
        div().styled().id("root").size_full().child(t).on_key_down(move |e, _, _| keys.borrow_mut().push(e.keystroke.key.clone()))
    }
}

struct TwoBoxes {
    keys: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
}

impl Render for TwoBoxes {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let heard = |name: &'static str, keys: std::rc::Rc<std::cell::RefCell<Vec<String>>>| {
            move |e: &KeyDownEvent, _: &mut Window, _: &mut App| keys.borrow_mut().push(format!("{name} {}", e.keystroke.key))
        };
        div()
            .styled()
            .id("root")
            .size_full()
            .child(textarea().id("a").h(px(40.0)).value("one").on_key_down(heard("a", self.keys.clone())))
            .child(textarea().id("b").h(px(40.0)).value("two").on_key_down(heard("b", self.keys.clone())))
            .child(div().id("away").h(px(100.0)))
    }
}

/// A click into a text box gives it the window's focus too: `focused_id` names it, and its
/// key listeners hear its keys with the pointer elsewhere. Escape leaves the focus alone.
#[test]
fn a_click_into_a_textarea_moves_the_windows_focus() {
    Application::headless().run(|cx| {
        let keys = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let win = open(cx, 400.0, 600.0, TwoBoxes { keys: keys.clone() });
        let id = win.id;
        cx.draw(id);
        let focused = |cx: &mut App| win.update(cx, |_, w, _| w.focused_id().map(str::to_string)).unwrap();
        let centre = |cx: &mut App, el: &str| {
            let b = cx.element_bounds(id, el).unwrap();
            (b.origin.x + b.size.width / 2.0, b.origin.y + b.size.height / 2.0)
        };
        assert_eq!(focused(cx), None);
        let (x, y) = centre(cx, "b");
        cx.simulate_click(id, x, y);
        assert_eq!(focused(cx).as_deref(), Some("b"), "at once, before another event");
        let (x, y) = centre(cx, "away");
        cx.simulate_mouse_move(id, x, y);
        cx.simulate_key(id, "", "Return");
        let (x, y) = centre(cx, "a");
        cx.simulate_click(id, x, y);
        cx.simulate_mouse_move(id, 300.0, 500.0);
        assert_eq!(focused(cx).as_deref(), Some("a"));
        cx.simulate_key(id, "", "Return");
        cx.simulate_key(id, "", "Escape");
        assert_eq!(focused(cx).as_deref(), Some("a"), "Escape leaves the window's focus");
        assert_eq!(*keys.borrow(), ["b Return", "a Return", "a Escape"]);
    });
}

#[test]
fn textarea_pass_keys_leave_keys_to_the_app() {
    Application::headless().run(|cx| {
        let keys = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let win = open(cx, 400.0, 600.0, PassKeys { pass: true, keys: keys.clone() });
        let id = win.id;
        cx.draw(id);
        let caret = |cx: &mut App| win.update(cx, |_, w, _| w.textarea_caret("box")).unwrap();
        win.update(cx, |_, w, _| w.focus_id("box")).unwrap();
        assert!(win.update(cx, |_, w, _| w.set_textarea_caret("box", TextCaret::at(5))).unwrap());
        for k in ["Down", "Up", "Up", "Return", "KP_Enter", "Tab"] {
            cx.simulate_key(id, "", k);
            assert_eq!(caret(cx), Some(TextCaret::at(5)), "{k} left the caret");
        }
        cx.simulate_key(id, "", "Escape");
        cx.simulate_key(id, "x", "x");
        assert_eq!(caret(cx), Some(TextCaret::at(6)), "Escape kept the focus: x typed");
        assert_eq!(*keys.borrow(), ["Down", "Up", "Up", "Return", "KP_Enter", "Tab", "Escape", "x"], "the app heard each");
        cx.simulate_key(id, "", "Left");
        assert_eq!(caret(cx), Some(TextCaret::at(5)), "other keys still move it");
        // the attribute gone, the box takes them again
        win.update(cx, |v, _, cx| {
            v.pass = false;
            cx.notify();
        })
        .unwrap();
        cx.draw(id);
        cx.simulate_key(id, "", "Up");
        assert_eq!(caret(cx), Some(TextCaret::at(1)), "\"two\" column 1 to \"one\" column 1");
    });
}

/// Command (`platform`) takes the text box's shortcuts as Ctrl does: ⌘A selects everything.
/// On macOS the Control key alone edits as Cocoa's text fields do (Control+A/E: the line's
/// start and end), never a shortcut.
#[test]
fn command_is_ctrl_to_text_boxes_and_control_alone_edits_on_macos() {
    Application::headless().run(|cx| {
        let win = open(cx, 400.0, 600.0, TextBoxes);
        let id = win.id;
        cx.draw(id);
        let caret = |cx: &mut App| win.update(cx, |_, w, _| w.textarea_caret("jump")).unwrap();
        let b = cx.element_bounds(id, "jump").unwrap();
        cx.simulate_click(id, b.origin.x + 4.0, b.origin.y + 4.0);
        // "one\ntwo\nthree", in "two"
        assert!(win.update(cx, |_, w, _| w.set_textarea_caret("jump", TextCaret::at(5))).unwrap());
        let cmd = Modifiers { platform: true, ..Default::default() };
        cx.simulate_key_mods(id, "", "a", cmd);
        assert_eq!(caret(cx), Some(TextCaret { anchor: 0, caret: 13 }), "⌘A: everything");
        if cfg!(target_os = "macos") {
            assert!(win.update(cx, |_, w, _| w.set_textarea_caret("jump", TextCaret::at(5))).unwrap());
            let control = Modifiers { control: true, ..Default::default() };
            cx.simulate_key_mods(id, "", "e", control);
            assert_eq!(caret(cx), Some(TextCaret::at(7)), "Control+E: the line's end");
            cx.simulate_key_mods(id, "", "a", control);
            assert_eq!(caret(cx), Some(TextCaret::at(4)), "Control+A: the line's start, not everything");
        }
    });
}

#[test]
fn focus_id_gives_a_textarea_the_kit_focus_back() {
    Application::headless().run(|cx| {
        let win = open(cx, 400.0, 600.0, TextBoxes);
        let id = win.id;
        cx.draw(id);
        let caret = |cx: &mut App| win.update(cx, |_, w, _| w.textarea_caret("jump")).unwrap();
        let b = cx.element_bounds(id, "jump").unwrap();
        cx.simulate_click(id, b.origin.x + 4.0, b.origin.y + 4.0);
        assert!(win.update(cx, |_, w, _| w.set_textarea_caret("jump", TextCaret::at(0))).unwrap());
        // Escape: the textarea gives the focus up, typing goes nowhere
        cx.simulate_key(id, "", "Escape");
        cx.simulate_key(id, "x", "x");
        assert_eq!(caret(cx), Some(TextCaret::at(0)));
        // focus_id hands it the keyboard again, without a click
        win.update(cx, |_, w, _| w.focus_id("jump"));
        cx.simulate_key(id, "x", "x");
        assert_eq!(caret(cx), Some(TextCaret::at(1)));
    });
}

/// A single-line field the app controls: it drops the line breaks Enter types.
#[derive(Default)]
struct OneLine {
    text: String,
}

impl Render for OneLine {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().styled().child(
            textarea().id("line").value(self.text.clone()).on_change(cx.listener(|this, e: &ChangeEvent, _, cx| {
                this.text = e.value.replace(['\n', '\r'], "");
                cx.notify();
            })),
        )
    }
}

/// A controlled text box shows the app's value after every edit, even when that value is the
/// one it gave before: the line break Enter typed is gone, so BackSpace takes the last letter.
#[test]
fn a_controlled_text_box_drops_what_the_app_dropped() {
    Application::headless().run(|cx| {
        let win = open(cx, 400.0, 300.0, OneLine::default());
        let id = win.id;
        cx.draw(id);
        let b = cx.element_bounds(id, "line").unwrap();
        cx.simulate_click(id, b.origin.x + 4.0, b.origin.y + 4.0);
        for c in ["a", "b"] {
            cx.simulate_key(id, c, c);
            cx.draw(id);
        }
        cx.simulate_key(id, "\r", "Return");
        cx.draw(id);
        assert_eq!(win.root().read(cx).text, "ab");
        let shown = win.update(cx, |_, w, _| w.textarea_caret("line")).unwrap();
        assert_eq!(shown, Some(TextCaret::at(2)), "the line break is gone from the box too");
        cx.simulate_key(id, "", "BackSpace");
        cx.draw(id);
        assert_eq!(win.root().read(cx).text, "a", "BackSpace takes the b, not an unseen line break");
        // typing goes on where it was
        cx.simulate_key(id, "c", "c");
        cx.draw(id);
        assert_eq!(win.root().read(cx).text, "ac");
    });
}

struct Scroller;

impl Render for Scroller {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        body().m_0().bg(rgb(0x000000)).child(
            div().id("scroller").w(px(100.0)).h(px(100.0)).overflow_y_scroll().bg(rgb(0x000000))
                .child(div().w(px(100.0)).h(px(400.0))),
        )
    }
}

#[test]
fn overlay_scrollbars_show_while_scrolling_then_fade() {
    Application::headless().run(|cx| {
        let win = open(cx, 200.0, 100.0, Scroller);
        let id = win.id;
        let lane = |cx: &mut App| {
            let img = cx.draw(id);
            let s = img.0 as f32 / 200.0;
            // the thumb's middle: 3px in from the edge, 5px wide; it takes no layout width
            (10..90).map(|y| pixel(&img, (94.5 * s) as usize, (y as f32 * s) as usize)).find(|&p| p != 0)
        };
        assert_eq!(cx.element_bounds(id, "scroller").unwrap().size.width, 100.0);
        assert_eq!(lane(cx), None, "hidden until scrolled");
        cx.simulate_scroll(id, 50.0, 50.0, -1);
        assert!(lane(cx).is_some(), "shown while scrolling");
        cx.advance_clock(std::time::Duration::from_millis(700));
        assert!(lane(cx).is_some(), "held");
        cx.advance_clock(std::time::Duration::from_millis(400));
        assert_eq!(lane(cx), None, "faded out");
        cx.simulate_mouse_move(id, 97.0, 50.0);
        assert!(lane(cx).is_some(), "the pointer near the edge shows it");
        cx.advance_clock(std::time::Duration::from_millis(5000));
        assert!(lane(cx).is_some(), "and holds it while it rests there");
        cx.simulate_mouse_move(id, 150.0, 50.0);
        cx.advance_clock(std::time::Duration::from_millis(1100));
        assert_eq!(lane(cx), None, "gone a hold after it leaves");
    });
}

#[test]
fn a_middle_drag_autoscrolls_a_scroll_container_and_not_plain_space() {
    Application::headless().run(|cx| {
        let win = open(cx, 200.0, 100.0, Scroller);
        let id = win.id;
        cx.draw(id);
        let at = |cx: &mut App| cx.window(id).unwrap().scroll_offset("scroller").1;
        // beside the container nothing scrolls: no autoscroll
        cx.simulate_middle(id, 150.0, 20.0, true);
        assert!(!cx.window(id).unwrap().is_autoscrolling());
        cx.simulate_middle(id, 150.0, 20.0, false);
        cx.simulate_middle(id, 50.0, 20.0, true);
        assert!(cx.window(id).unwrap().is_autoscrolling());
        cx.simulate_mouse_move(id, 50.0, 90.0);
        for _ in 0..10 {
            cx.advance_clock(std::time::Duration::from_millis(16));
            cx.draw(id);
        }
        let down = at(cx);
        assert!(down > 0.0, "{down}");
        // to the end and no further: it stops asking for frames
        for _ in 0..200 {
            cx.advance_clock(std::time::Duration::from_millis(16));
            cx.draw(id);
        }
        assert_eq!(at(cx), 300.0);
        cx.draw(id);
        assert!(!cx.window(id).unwrap().needs_render());
        cx.simulate_middle(id, 50.0, 90.0, false);
        assert!(!cx.window(id).unwrap().is_autoscrolling());
    });
}

/// A box that hears the wheel and the middle button's press itself (a pan), noting the presses.
struct Panner {
    downs: std::rc::Rc<std::cell::Cell<usize>>,
}

impl Render for Panner {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let downs = self.downs.clone();
        body().m_0().bg(rgb(0x000000)).child(
            div().id("pan").w(px(100.0)).h(px(100.0))
                .on_scroll_wheel(|_, _, _| {})
                .on_mouse_down(MouseButton::Middle, move |_, _, _| downs.set(downs.get() + 1)),
        )
    }
}

#[test]
fn a_middle_press_on_what_hears_it_is_a_press_and_no_autoscroll() {
    Application::headless().run(|cx| {
        let downs = std::rc::Rc::new(std::cell::Cell::new(0));
        let win = open(cx, 200.0, 100.0, Panner { downs: downs.clone() });
        let id = win.id;
        cx.draw(id);
        cx.simulate_middle(id, 50.0, 50.0, true);
        assert!(!cx.window(id).unwrap().is_autoscrolling(), "the box takes the middle button");
        assert_eq!(downs.get(), 1, "and hears the press");
        cx.simulate_middle(id, 50.0, 50.0, false);
        assert!(!cx.window(id).unwrap().is_autoscrolling());
    });
}

/// A text box and a plain box, noting each mouse-down's click count.
struct Triple {
    downs: std::rc::Rc<std::cell::RefCell<Vec<(&'static str, usize)>>>,
}

impl Render for Triple {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (a, b) = (self.downs.clone(), self.downs.clone());
        div().styled().id("root").size_full()
            .child(textarea().id("box").attr("rows", "4").value("one two\nthree four\nfive")
                .on_mouse_down(MouseButton::Left, move |e, _, _| a.borrow_mut().push(("box", e.click_count))))
            .child(div().id("pad").w(px(100.0)).h(px(40.0))
                .on_mouse_down(MouseButton::Left, move |e, _, _| b.borrow_mut().push(("pad", e.click_count))))
    }
}

#[test]
fn a_triple_click_selects_a_text_box_line_and_counts_on() {
    Application::headless().run(|cx| {
        let downs = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let win = open(cx, 400.0, 600.0, Triple { downs: downs.clone() });
        let id = win.id;
        cx.draw(id);
        let caret = |cx: &mut App| win.update(cx, |_, w, _| w.textarea_caret("box")).unwrap();
        let b = cx.element_bounds(id, "box").unwrap();
        // on the first line ("one two"), a little in
        let (x, y) = (b.origin.x + 8.0, b.origin.y + 6.0);
        let pause = || std::thread::sleep(std::time::Duration::from_millis(400));
        cx.simulate_click(id, x, y);
        cx.simulate_click(id, x, y);
        assert_eq!(caret(cx), Some(TextCaret { anchor: 0, caret: 3 }), "a double click: the word");
        cx.simulate_click(id, x, y);
        assert_eq!(caret(cx), Some(TextCaret { anchor: 0, caret: 7 }), "a triple click: the line, no newline");
        cx.simulate_click(id, x, y);
        assert_eq!(caret(cx), Some(TextCaret { anchor: 0, caret: 7 }), "a fourth keeps it");
        assert_eq!(*downs.borrow(), [("box", 3), ("box", 3)], "the box hears its triple clicks only");
        downs.borrow_mut().clear();
        pause();
        cx.simulate_click(id, x, y);
        assert_eq!(caret(cx).map(|c| c.anchor == c.caret), Some(true), "after a pause, a first click again");
        // a plain box hears every press's count, on past two
        pause();
        let p = cx.element_bounds(id, "pad").unwrap();
        let (x, y) = (p.origin.x + 10.0, p.origin.y + 10.0);
        for _ in 0..4 {
            cx.simulate_click(id, x, y);
        }
        assert_eq!(*downs.borrow(), [("pad", 1), ("pad", 2), ("pad", 3), ("pad", 4)]);
    });
}

/// Paints green while the window has the keyboard focus, grey otherwise, and records what
/// each render saw.
struct Focus {
    seen: std::rc::Rc<std::cell::RefCell<Vec<Option<bool>>>>,
}

impl Render for Focus {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let active = window.is_active();
        self.seen.borrow_mut().push(active);
        let color = if active == Some(true) { 0x00ff00 } else { 0x808080 };
        body().m_0().child(div().id("dot").size(px(20.0)).bg(rgb(color)))
    }
}

#[test]
fn activation_re_renders_with_the_focus_state() {
    Application::headless().run(|cx| {
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let win = open(cx, 100.0, 50.0, Focus { seen: seen.clone() });
        let id = win.id;
        let img = cx.draw(id);
        let s = img.0 as f32 / 100.0;
        let dot = |img: &(usize, usize, Vec<u32>)| pixel(img, (10.0 * s) as usize, (10.0 * s) as usize);
        assert_eq!(cx.window(id).unwrap().is_active(), None, "headless windows can't tell at first");
        assert_eq!(dot(&img), 0x808080);
        assert_eq!(seen.borrow().last(), Some(&None));

        cx.simulate_activation(id, true);
        assert_eq!(cx.window(id).unwrap().is_active(), Some(true));
        assert_eq!(seen.borrow().last(), Some(&Some(true)), "the change re-rendered the window");
        assert_eq!(dot(&cx.draw(id)), 0x00ff00);

        let renders = seen.borrow().len();
        cx.simulate_activation(id, true); // no change, no render
        assert_eq!(seen.borrow().len(), renders);

        cx.simulate_activation(id, false);
        assert_eq!(cx.window(id).unwrap().is_active(), Some(false));
        assert_eq!(seen.borrow().last(), Some(&Some(false)));
        assert_eq!(dot(&cx.draw(id)), 0x808080);

        cx.activate_window(id, Some("token")); // headless: becomes active
        assert_eq!(cx.window(id).unwrap().is_active(), Some(true));
        cx.activate_window(id, None);
        assert_eq!(cx.window(id).unwrap().is_active(), Some(true));
    });
}

/// A text box and a plain box; the root notes back and forward presses and releases, the box
/// any press.
struct Nav {
    heard: std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>,
}

impl Render for Nav {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let h = |s: &'static str| {
            let heard = self.heard.clone();
            move || heard.borrow_mut().push(s)
        };
        let (a, b, c, d, e) = (h("back"), h("forward"), h("back up"), h("forward up"), h("any"));
        div().styled().id("root").size_full()
            .on_mouse_down(MouseButton::Back, move |_, _, _| a())
            .on_mouse_down(MouseButton::Forward, move |_, _, _| b())
            .on_mouse_up(MouseButton::Back, move |_, _, _| c())
            .on_mouse_up(MouseButton::Forward, move |_, _, _| d())
            .child(textarea().id("box").attr("rows", "2").value("text"))
            .child(div().id("pad").w(px(100.0)).h(px(40.0))
                .on_any_mouse_down(move |_, _, _| e()))
    }
}

#[test]
fn back_and_forward_buttons_reach_the_window_wherever_the_pointer_is() {
    Application::headless().run(|cx| {
        let heard = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let win = open(cx, 400.0, 300.0, Nav { heard: heard.clone() });
        let id = win.id;
        cx.draw(id);
        let b = cx.element_bounds(id, "box").unwrap();
        let p = cx.element_bounds(id, "pad").unwrap();
        let r = cx.element_bounds(id, "root").unwrap();
        let free = (r.origin.x + r.size.width - 10.0, p.origin.y + 10.0); // beside the pad
        for (x, y) in [(b.origin.x + 8.0, b.origin.y + 6.0), (p.origin.x + 10.0, p.origin.y + 10.0), free] {
            cx.simulate_navigate(id, x, y, false, true);
            cx.simulate_navigate(id, x, y, false, false);
            cx.simulate_navigate(id, x, y, true, true);
            cx.simulate_navigate(id, x, y, true, false);
            assert_eq!(*heard.borrow(), ["back", "back up", "forward", "forward up"], "at {x},{y}");
            heard.borrow_mut().clear();
        }
    });
}
