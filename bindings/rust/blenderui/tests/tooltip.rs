//! Tooltips: an element's `title` after the pointer rests on it.

use std::time::Duration;

use blenderui::prelude::*;

struct Titled {
    tips: bool,
}

impl Render for Titled {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        window.set_tooltip_delay(Duration::from_secs(2));
        window.set_tooltips(self.tips);
        body()
            .m_0()
            .child(div().id("a").w(px(60.0)).h(px(20.0)).title("Save the file"))
            .child(div().id("b").w(px(60.0)).h(px(20.0)).title("Open a file").child(div().id("b-inner").size(px(10.0))))
            .child(div().id("plain").w(px(60.0)).h(px(20.0)))
            .child(div().id("low").mt(px(30.0)).w(px(60.0)).h(px(20.0)).title("Near the bottom"))
    }
}

fn open(cx: &mut App) -> WindowHandle<Titled> {
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(200.0), px(120.0)),
        })),
        ..Default::default()
    };
    cx.open_window(opts, |_, cx| cx.new(|_| Titled { tips: true })).unwrap()
}

fn tip(cx: &App, win: &WindowHandle<Titled>) -> Option<String> {
    cx.window(win.id)?.tooltip().map(str::to_owned)
}

fn wait(cx: &mut App, ms: u64) {
    cx.advance_clock(Duration::from_millis(ms));
}

struct StyledDoc;

impl Render for StyledDoc {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        html().styled().child(body().m_0().child(div().id("a").w(px(60.0)).h(px(20.0)).title("Save the file")))
    }
}

#[test]
fn a_styled_document_places_it_too() {
    Application::headless().run(|cx| {
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.0), px(0.0)),
                size: size(px(200.0), px(120.0)),
            })),
            ..Default::default()
        };
        let win = cx.open_window(opts, |_, cx| cx.new(|_| StyledDoc)).unwrap();
        cx.draw(win.id);
        cx.simulate_mouse_move(win.id, 10.0, 10.0);
        wait(cx, 1000);
        assert_eq!(cx.window(win.id).unwrap().tooltip(), Some("Save the file"));
        let b = cx.element_bounds(win.id, "bui-tooltip").unwrap();
        assert_eq!((b.origin.x, b.origin.y), (10.0, 30.0));
        assert!(b.size.width < 150.0, "shrinks to its text: {b:?}");
    });
}

#[test]
fn a_title_shows_after_the_delay_and_hides_on_leave_click_key_and_off() {
    Application::headless().run(|cx| {
        let win = open(cx);
        let id = win.id;
        cx.draw(id);
        cx.simulate_mouse_move(id, 10.0, 10.0);
        wait(cx, 1900);
        assert_eq!(tip(cx, &win), None, "not before the delay");
        wait(cx, 100);
        assert_eq!(tip(cx, &win).as_deref(), Some("Save the file"));
        // drawn below the pointer, styled by the UA sheet
        let b = cx.element_bounds(id, "bui-tooltip").expect("the tooltip is in the document");
        assert_eq!((b.origin.x, b.origin.y), (10.0, 30.0));
        assert!(b.size.width > 40.0 && b.size.height > 12.0);

        // another element starts the wait again; inside it, the same element
        cx.simulate_mouse_move(id, 10.0, 30.0);
        assert_eq!(tip(cx, &win), None, "leaving hides it at once");
        wait(cx, 1000);
        cx.simulate_mouse_move(id, 5.0, 25.0);
        wait(cx, 999);
        assert_eq!(tip(cx, &win), None);
        wait(cx, 1);
        assert_eq!(tip(cx, &win).as_deref(), Some("Open a file"), "a child shows its ancestor's title");

        // an element without one shows nothing
        cx.simulate_mouse_move(id, 10.0, 50.0);
        wait(cx, 3000);
        assert_eq!(tip(cx, &win), None);

        // a click hides it until the pointer leaves
        cx.simulate_mouse_move(id, 10.0, 10.0);
        wait(cx, 2000);
        assert!(tip(cx, &win).is_some());
        cx.simulate_click(id, 12.0, 10.0);
        assert_eq!(tip(cx, &win), None);
        wait(cx, 5000);
        assert_eq!(tip(cx, &win), None, "hushed while the pointer stays");
        cx.simulate_mouse_move(id, 10.0, 50.0);
        cx.simulate_mouse_move(id, 10.0, 10.0);
        wait(cx, 2000);
        assert!(tip(cx, &win).is_some(), "back after leaving and coming again");

        // so does a key
        cx.simulate_key(id, "x", "x");
        assert_eq!(tip(cx, &win), None);

        // the app turns them off: nothing shows, nothing waits
        win.update(cx, |v, _, cx| {
            v.tips = false;
            cx.notify();
        });
        cx.simulate_mouse_move(id, 10.0, 25.0);
        wait(cx, 3000);
        assert_eq!(tip(cx, &win), None);
        win.update(cx, |v, _, cx| {
            v.tips = true;
            cx.notify();
        });
        wait(cx, 1);
        assert_eq!(tip(cx, &win).as_deref(), Some("Open a file"));

        // no room below the pointer: above it, inside the window
        cx.simulate_mouse_move(id, 190.0, 50.0);
        cx.simulate_mouse_move(id, 50.0, 105.0);
        wait(cx, 2000);
        assert_eq!(tip(cx, &win).as_deref(), Some("Near the bottom"));
        let b = cx.element_bounds(id, "bui-tooltip").unwrap();
        assert!(b.origin.y + b.size.height <= 105.0, "flipped above the pointer: {b:?}");
        assert!(b.origin.x + b.size.width <= 196.0, "inside the window: {b:?}");
    });
}
