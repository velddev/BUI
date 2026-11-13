//! A title bar drawn by the app: the areas the native host is handed agree with the window's
//! own hit test (a tab on the bar is no drag area, a plain box on it is, a menu's backdrop covers the bar), and the
//! buttons zoom and minimise the window.

use blenderui::prelude::*;

struct Bar {
    menu: bool,
}

impl Render for Bar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let zoomed = window.is_maximized();
        let area = |id: &'static str, css: &'static str, a: WindowControlArea| div().id(id).style(css).window_control_area(a);
        body()
            .m_0()
            .style("position: relative")
            .child(
                area("bar", "display: flex; height: 48px; width: 400px", WindowControlArea::Drag)
                    .child(div().id("tab").style("margin: 10px 0 0 100px; width: 80px; height: 28px")
                        .on_click(cx.listener(|_, _: &ClickEvent, _, _| {}))
                        .child(div().id("label").style("width: 40px; height: 10px")))
                    .child(div().id("plain").style("margin-top: 10px; width: 20px; height: 20px"))
                    .child(div().style("flex-grow: 1"))
                    .child(area("min", "width: 46px; height: 48px", WindowControlArea::Min))
                    .child(
                        area(if zoomed { "restore" } else { "max" }, "width: 46px; height: 48px", WindowControlArea::Max)
                            .on_click(cx.listener(|_, _: &ClickEvent, window, _| window.zoom_window())),
                    )
                    .child(area("close", "width: 46px; height: 48px", WindowControlArea::Close)),
            )
            .child(div().id("body").style("width: 400px; height: 100px"))
            .children(self.menu.then(|| {
                div().id("backdrop").style("position: absolute; top: 0; left: 0; width: 400px; height: 300px; z-index: 4")
            }))
    }
}

fn open(cx: &mut App, own: bool, menu: bool) -> WindowHandle<Bar> {
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.0), px(0.0)), size: size(px(400.0), px(200.0)) })),
        titlebar: Some(TitlebarOptions { title: Some("Bar".into()), appears_transparent: own, ..Default::default() }),
        ..Default::default()
    };
    cx.open_window(opts, |_, cx| cx.new(|_| Bar { menu })).unwrap()
}

fn at(cx: &App, w: &WindowHandle<Bar>, x: f32, y: f32) -> (Option<WindowControlArea>, Option<WindowControlArea>) {
    let win = cx.window(w.id).unwrap();
    (win.control_area_at(point(px(x), px(y))), win.control_area_hit(point(px(x), px(y))))
}

#[test]
fn the_host_is_handed_what_the_window_hit_tests() {
    Application::headless().run(|cx| {
        let w = open(cx, true, false);
        cx.draw(w.id);
        assert!(cx.window(w.id).unwrap().has_own_titlebar());
        use WindowControlArea::*;
        for (x, y, want) in [
            (20.0, 20.0, Some(Drag)),
            (120.0, 20.0, None),  // the tab
            (110.0, 15.0, None),  // its label
            (120.0, 5.0, Some(Drag)), // above it
            (185.0, 15.0, Some(Drag)), // a box with no listeners drags
            (260.0, 47.0, Some(Drag)),
            (270.0, 20.0, Some(Min)),
            (330.0, 0.0, Some(Max)),
            (399.0, 47.0, Some(Close)),
            (200.0, 48.0, None), // below the bar
            (200.0, 120.0, None),
        ] {
            let (host, hit) = at(cx, &w, x, y);
            assert_eq!(host, want, "host at {x},{y}");
            assert_eq!(hit, want, "hit at {x},{y}");
        }
        // a grid over the bar: always the same answer
        for x in (0..400).step_by(7) {
            for y in (0..60).step_by(3) {
                let (host, hit) = at(cx, &w, x as f32, y as f32);
                assert_eq!(host, hit, "at {x},{y}");
            }
        }
    });
}

#[test]
fn a_backdrop_over_the_bar_covers_it() {
    Application::headless().run(|cx| {
        let w = open(cx, true, true);
        cx.draw(w.id);
        for (x, y) in [(20.0, 20.0), (330.0, 20.0), (380.0, 10.0)] {
            assert_eq!(at(cx, &w, x, y), (None, None), "at {x},{y}");
        }
    });
}

#[test]
fn windows_with_the_systems_title_bar_have_no_areas() {
    Application::headless().run(|cx| {
        let w = open(cx, false, false);
        cx.draw(w.id);
        let win = cx.window(w.id).unwrap();
        assert!(!win.has_own_titlebar());
        assert_eq!(win.control_area_at(point(px(20.0), px(20.0))), None);
        assert_eq!(win.control_area_hit(point(px(20.0), px(20.0))), Some(WindowControlArea::Drag), "the hit test still sees the mark");
    });
}

#[test]
fn the_maximise_button_zooms_and_restores() {
    Application::headless().run(|cx| {
        let w = open(cx, true, false);
        cx.draw(w.id);
        assert!(!cx.window(w.id).unwrap().is_maximized());
        cx.simulate_click(w.id, 330.0, 20.0);
        cx.draw(w.id);
        assert!(cx.window(w.id).unwrap().is_maximized());
        assert!(cx.element_bounds(w.id, "restore").is_some(), "drawn as restore");
        cx.simulate_click(w.id, 330.0, 20.0);
        cx.draw(w.id);
        assert!(!cx.window(w.id).unwrap().is_maximized());
        w.update(cx, |_, window, _| window.minimize_window());
        assert!(cx.window(w.id).unwrap().is_minimized());
    });
}
