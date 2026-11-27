//! The variable list and the chat, end to end in off-screen windows.

use blenderui::prelude::*;
use blenderui::vlist::{chat_list, synth, theme_palette, ChatList, ChatStyle, ItemSource, ListState, Options, VecSource};

type Chat = Entity<ListState<VecSource, ChatStyle>>;

struct View {
    chat: Chat,
}

impl Render for View {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        body().m_0().child(div().w(px(700.0)).h(px(400.0)).child(chat_list(&self.chat)))
    }
}

fn open(cx: &mut App, n: u64) -> (WindowId, Chat) {
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.0), px(0.0)), size: size(px(700.0), px(400.0)) })),
        ..Default::default()
    };
    let mut handle = None;
    let win = cx
        .open_window(opts, |_, cx| {
            let o = Options { start_at_end: true, ..Options::default() };
            let chat = cx.new(|cx| ListState::new(ChatList::from_messages(synth::messages(0, n), theme_palette(), 700.0, 400.0, o), cx));
            handle = Some(chat);
            cx.new(|_| View { chat })
        })
        .unwrap();
    (win.id, handle.unwrap())
}

fn frame(cx: &mut App, id: WindowId) -> (usize, usize, Vec<u32>) {
    cx.draw(id); // the first frame learns the size, the second lays out with it
    cx.draw(id);
    cx.draw(id)
}

fn run(theme: &str, f: impl FnOnce(&mut App)) {
    Application::headless().theme(theme).run(f);
}

fn distinct(img: &(usize, usize, Vec<u32>)) -> usize {
    let mut v = img.2.clone();
    v.sort_unstable();
    v.dedup();
    v.len()
}

#[test]
fn opens_at_the_newest_message_and_draws_text() {
    run("dark", |cx| {
        let (id, chat) = open(cx, 100_000);
        let img = frame(cx, id);
        let s = chat.read(cx);
        assert_eq!(s.view.placed().last().unwrap().item, 99_999);
        assert!(s.view.at_bottom());
        assert!(s.view.index().measured_items() < 3000, "only the tail was measured");
        // text, avatars and a scrollbar: far more than a flat fill
        assert!(distinct(&img) > 12, "{}", distinct(&img));
    });
}

#[test]
fn scrolling_up_moves_the_content_and_stops_following() {
    run("dark", |cx| {
        let (id, chat) = open(cx, 50_000);
        let a = frame(cx, id);
        cx.simulate_scroll(id, 300.0, 200.0, 10);
        let mut b = frame(cx, id);
        for _ in 0..60 {
            b = cx.draw(id);
        }
        assert_ne!(a.2, b.2, "the wheel scrolled");
        assert!(!chat.read(cx).view.at_bottom());
        // a new message does not drag the reader down
        let id_before = {
            let s = chat.read(cx);
            s.view.anchor_id(&s.source)
        };
        chat.update(cx, |s, _| s.push(synth::message(50_000)));
        cx.notify_all();
        frame(cx, id);
        let s = chat.read(cx);
        assert_eq!(s.view.anchor_id(&s.source), id_before);
        assert_eq!(s.source.len(), 50_001);
    });
}

#[test]
fn a_new_message_follows_at_the_bottom() {
    run("dark", |cx| {
        let (id, chat) = open(cx, 10_000);
        frame(cx, id);
        chat.update(cx, |s, _| s.push(synth::message(10_000)));
        cx.notify_all();
        frame(cx, id);
        assert_eq!(chat.read(cx).view.placed().last().unwrap().item, 10_000);
    });
}

#[test]
fn clicking_selects_a_whole_message() {
    run("dark", |cx| {
        let (id, chat) = open(cx, 10_000);
        let a = frame(cx, id);
        cx.simulate_click(id, 300.0, 380.0);
        let b = frame(cx, id);
        assert!(chat.read(cx).style.selected.is_some());
        assert_ne!(a.2, b.2, "the selection is painted");
        cx.simulate_click(id, 300.0, 380.0);
        frame(cx, id);
        assert!(chat.read(cx).style.selected.is_none());
    });
}

#[test]
fn the_background_follows_the_theme() {
    let shot = |theme: &str| {
        let mut px = 0;
        run(theme, |cx| {
            let (id, chat) = open(cx, 10_000);
            px = frame(cx, id).2[0];
            assert!(chat.read(cx).view.at_bottom());
        });
        px
    };
    assert_ne!(shot("light"), shot("dark"));
}

/// A chat as wide as the window, 300 px high, and under it a line that notes, as it is
/// painted, its own width and the width the chat's rows are laid out for then.
struct Full {
    chat: Chat,
    painted: std::rc::Rc<std::cell::RefCell<Vec<(f32, f32)>>>,
}

impl Render for Full {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (chat, painted) = (self.chat, self.painted.clone());
        let note = canvas_with(move |b, _, cx| painted.borrow_mut().push((b.size.width, chat.read(cx).view.viewport().0)));
        body().m_0().child(div().w_full().h(px(300.0)).child(chat_list(&self.chat))).child(note.w_full().h(px(1.0)))
    }
}

#[test]
fn every_frame_lays_the_rows_out_at_the_newest_window_size() {
    run("dark", |cx| {
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.0), px(0.0)), size: size(px(700.0), px(400.0)) })),
            ..Default::default()
        };
        let mut handle = None;
        let painted = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let notes = painted.clone();
        let win = cx
            .open_window(opts, |_, cx| {
                let o = Options { start_at_end: true, ..Options::default() };
                let chat = cx.new(|cx| ListState::new(ChatList::from_messages(synth::messages(0, 500), theme_palette(), 700.0, 400.0, o), cx));
                handle = Some(chat);
                cx.new(|_| Full { chat, painted: notes })
            })
            .unwrap();
        let (id, chat) = (win.id, handle.unwrap());
        let scale = cx.draw(id).0 as f32 / 700.0;
        assert_eq!(chat.read(cx).view.viewport(), (700.0, 300.0));
        // one size per frame, then several between two frames: every frame paints the rows
        // laid out for the width it is drawn at, the last one configured
        for sizes in [&[(760.0, 400.0)][..], &[(720.0, 380.0), (701.0, 420.0), (733.0, 377.0)], &[(640.0, 300.0)]] {
            for &(w, h) in sizes {
                cx.simulate_resize(id, w, h);
            }
            let (pw, ph, _) = cx.draw(id);
            let &(w, h) = sizes.last().unwrap();
            // the buffer, the layout and the paint all at the last size
            assert_eq!((pw, ph), ((w * scale).round() as usize, (h * scale).round() as usize));
            assert_eq!(painted.borrow().last().map(|p| p.0), Some(w));
        }
        let painted = painted.borrow();
        assert!(painted.len() >= 4, "{painted:?}");
        for &(box_w, rows_w) in painted.iter() {
            assert_eq!(rows_w, box_w, "a frame painted rows laid out for {rows_w} px in a {box_w} px box: {painted:?}");
        }
    });
}

/// Where the list is: its first visible item and that item's top (px estimates above it move
/// as rows get measured, so the offset alone can't tell).
fn offset(cx: &App, chat: &Chat) -> (usize, i64) {
    let v = &chat.read(cx).view;
    let p = v.placed().first().unwrap();
    (p.item, (p.y * 100.0) as i64)
}

/// Step the clock a frame at a time.
fn frames(cx: &mut App, id: WindowId, n: usize) {
    for _ in 0..n {
        cx.advance_clock(std::time::Duration::from_millis(16));
        cx.draw(id);
    }
}

#[test]
fn the_middle_button_held_and_moved_scrolls_and_its_release_stops_it() {
    run("dark", |cx| {
        let (id, chat) = open(cx, 5_000);
        frame(cx, id);
        let start = offset(cx, &chat);
        cx.simulate_middle(id, 300.0, 300.0, true);
        assert!(cx.window(id).unwrap().is_autoscrolling());
        use blenderui::autoscroll::{CURSOR_DOWN, CURSOR_STILL, CURSOR_UP};
        let shown = |cx: &App| {
            let w = cx.window(id).unwrap();
            (w.cursor_shape(), w.native_cursor())
        };
        assert_eq!(shown(cx), (CURSOR_STILL, CURSOR_STILL), "the still pan cursor, shown under the capture");
        assert_eq!(cx.window(id).unwrap().autoscroll_anchor(), Some((300.0, 300.0)));
        // in the dead zone: nothing moves
        cx.simulate_mouse_move(id, 303.0, 305.0);
        frames(cx, id, 5);
        assert_eq!(offset(cx, &chat), start);
        assert_eq!(shown(cx), (CURSOR_STILL, CURSOR_STILL), "still in the dead zone");
        cx.simulate_mouse_move(id, 300.0, 400.0);
        assert_eq!(shown(cx), (CURSOR_DOWN, CURSOR_DOWN), "below the anchor: the down pan cursor");
        // above the anchor: up, faster further out
        cx.simulate_mouse_move(id, 300.0, 200.0);
        assert_eq!(shown(cx), (CURSOR_UP, CURSOR_UP), "above the anchor: the up pan cursor");
        frames(cx, id, 10);
        let up = offset(cx, &chat);
        assert!(up.0 < start.0, "{up:?} {start:?}");
        cx.simulate_middle(id, 300.0, 200.0, false);
        let w = cx.window(id).unwrap();
        assert!(!w.is_autoscrolling());
        assert_eq!(w.autoscroll_anchor(), None);
        assert!(!(CURSOR_STILL..=CURSOR_DOWN).contains(&w.native_cursor()), "the usual cursor again: {}", w.native_cursor());
        frames(cx, id, 10);
        assert_eq!(offset(cx, &chat), up, "the release stopped it");
    });
}

#[test]
fn a_still_middle_click_autoscrolls_until_escape_a_click_or_the_wheel() {
    run("dark", |cx| {
        let (id, chat) = open(cx, 5_000);
        frame(cx, id);
        for exit in 0..3 {
            cx.simulate_middle(id, 300.0, 300.0, true);
            cx.simulate_middle(id, 300.0, 300.0, false);
            assert!(cx.window(id).unwrap().is_autoscrolling(), "sticky");
            let before = offset(cx, &chat);
            cx.simulate_mouse_move(id, 300.0, 150.0);
            frames(cx, id, 8);
            let moved = offset(cx, &chat);
            assert!(moved.0 < before.0, "{exit}: {moved:?} {before:?}");
            match exit {
                0 => cx.simulate_key(id, "", "Escape"),
                1 => cx.simulate_click(id, 300.0, 150.0),
                _ => cx.simulate_scroll(id, 300.0, 150.0, 0),
            }
            assert!(!cx.window(id).unwrap().is_autoscrolling(), "{exit}");
            let after = offset(cx, &chat);
            frames(cx, id, 8);
            assert_eq!(offset(cx, &chat), after, "{exit}: stopped");
        }
    });
}

#[test]
fn autoscroll_ends_when_the_window_loses_focus_held_or_sticky() {
    run("dark", |cx| {
        let (id, chat) = open(cx, 5_000);
        frame(cx, id);
        cx.simulate_activation(id, true);
        for held in [true, false] {
            cx.simulate_middle(id, 300.0, 300.0, true);
            if !held {
                cx.simulate_middle(id, 300.0, 300.0, false);
            }
            assert!(cx.window(id).unwrap().is_autoscrolling(), "held {held}");
            cx.simulate_mouse_move(id, 300.0, 150.0);
            frames(cx, id, 8);
            cx.simulate_activation(id, false);
            assert!(!cx.window(id).unwrap().is_autoscrolling(), "held {held}: focus lost");
            let after = offset(cx, &chat);
            frames(cx, id, 8);
            assert_eq!(offset(cx, &chat), after, "held {held}: stopped");
            if held {
                cx.simulate_middle(id, 300.0, 150.0, false);
                assert!(!cx.window(id).unwrap().is_autoscrolling(), "the late release starts nothing");
            }
            cx.simulate_activation(id, true);
        }
    });
}

#[test]
fn autoscroll_stops_cleanly_at_the_end_and_asks_for_no_frames() {
    run("dark", |cx| {
        let (id, chat) = open(cx, 2_000);
        frame(cx, id);
        // at the end already: down has nothing to scroll
        let end = offset(cx, &chat);
        cx.simulate_middle(id, 300.0, 100.0, true);
        cx.simulate_mouse_move(id, 300.0, 390.0);
        frames(cx, id, 6);
        assert_eq!(offset(cx, &chat), end);
        assert!(!cx.window(id).unwrap().needs_render(), "idle at the end");
        cx.simulate_middle(id, 300.0, 390.0, false);
    });
}

/// Shift + wheel (and a sideways swipe, which the kit sends as one) goes sideways: the list,
/// which has no sideways scroll, stays where it is, and its parents' listeners hear it.
#[test]
fn shift_and_the_wheel_leave_the_list_where_it_is() {
    run("dark", |cx| {
        let (id, chat) = open(cx, 5_000);
        frame(cx, id);
        let before = offset(cx, &chat);
        let shift = Modifiers { shift: true, ..Modifiers::default() };
        cx.simulate_scroll_mods(id, 300.0, 200.0, 5, shift);
        frames(cx, id, 30);
        assert_eq!(offset(cx, &chat), before, "Shift + wheel left the list alone");
        assert!(chat.read(cx).view.at_bottom());
        // the plain wheel still scrolls it
        cx.simulate_scroll(id, 300.0, 200.0, 5);
        frames(cx, id, 30);
        assert_ne!(offset(cx, &chat), before);
    });
}
