//! A DPI change mid-run (Windows' `WM_DPICHANGED`): the layout follows the kit's
//! new scale, so a text box's text stays the size of the CSS text beside it.

use blenderui::prelude::*;

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

/// Device-pixel (first row, last row) of white ink inside the logical box `b`.
fn ink_rows(img: &(usize, usize, Vec<u32>), b: Bounds<f32>, s: f32) -> Option<(usize, usize)> {
    let (x0, y0) = ((b.origin.x * s) as usize, (b.origin.y * s) as usize);
    let (x1, y1) = (((b.origin.x + b.size.width) * s) as usize, ((b.origin.y + b.size.height) * s) as usize);
    let mut r: Option<(usize, usize)> = None;
    for y in y0..y1.min(img.1) {
        for x in x0..x1.min(img.0) {
            if (img.2[y * img.0 + x] >> 16) & 0xff > 160 {
                r = Some(r.map_or((y, y), |(t, bo)| (t.min(y), bo.max(y))));
            }
        }
    }
    r
}

struct Both;

impl Render for Both {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().styled().style("background: #000; color: #fff; padding: 10px; font-size: 14px")
            .child(div().id("css").style("height: 40px").child("HHHH"))
            .child(textarea().id("box").attr("rows", "1").style("font-size: 14px").value("HHHH"))
    }
}

#[test]
fn a_text_box_follows_a_scale_change() {
    Application::headless().run(|cx| {
        let id = open(cx, 240.0, 120.0, Both).id;
        let caps = |cx: &mut App, s: f32| {
            let img = cx.draw(id);
            assert_eq!(img.0, (240.0 * s).round() as usize, "drawn at scale {s}");
            let (css, tb) = (cx.element_bounds(id, "css").unwrap(), cx.element_bounds(id, "box").unwrap());
            let h = |r: Option<(usize, usize)>| r.map(|(t, b)| b - t + 1).expect("no text drawn");
            (h(ink_rows(&img, css, s)), h(ink_rows(&img, tb, s)))
        };
        let (css, tb) = caps(cx, 1.0);
        assert!(css.abs_diff(tb) <= 1, "at 1: CSS capitals {css} px, the text box's {tb} px");
        for s in [1.5, 1.25, 2.0] {
            cx.simulate_scale_change(id, s);
            let (css, tb) = caps(cx, s);
            assert!(css.abs_diff(tb) <= 1, "at {s}: CSS capitals {css} px, the text box's {tb} px");
            let want = (10.0 * s).round() as usize; // Segoe/Inter capitals are ~0.7 em
            assert!(css.abs_diff(want) <= 2, "at {s}: CSS capitals {css} px, about {want} wanted");
        }
    });
}
