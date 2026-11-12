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
